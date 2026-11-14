//! In-memory merchant / checkout store for the M6/M7 API. Postgres schema
//! lives in `pcc-storage` migrations; this store is what tests and the local
//! demo process use.

use alloy_primitives::Address;
use chrono::{DateTime, Duration, Utc};
use pcc_auth::{hash_api_key, verify_api_key};
use pcc_merchants::{new_payment_intent, new_payout_config};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::AssetId;
use pcc_payments::domain::{
    Merchant, MerchantId, MerchantStatus, PaymentAttempt, PaymentAttemptId, PaymentEvent, PaymentEventId,
    PaymentIntent, PaymentIntentId, PayoutConfig, WalletCredentialId, WebhookDelivery, WebhookDeliveryId,
    WebhookDeliveryStatus,
};
use pcc_payments::quote::VerifiedQuote;
use pcc_payments::status::{PaymentAttemptStatus, PaymentIntentStatus};
use pcc_payments::traits::DepositPlan;
use pcc_settlement::webhook::{assert_webhook_url, sign_body, WebhookError, WebhookPolicy};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct CheckoutSession {
    pub id: String,
    pub merchant_id: MerchantId,
    pub intent_id: PaymentIntentId,
    pub expires_at: DateTime<Utc>,
    pub success_url: Option<String>,
}

/// Payer-signed autopay: the merchant may charge without the payer present,
/// within these limits. The worker's on-chain allowance is the hard limit.
#[derive(Clone, Debug)]
pub struct Mandate {
    pub id: Uuid,
    pub merchant_id: MerchantId,
    pub owner: Address,
    pub asset: AssetId,
    /// Per-charge cap in source-token base units.
    pub max_amount: BaseUnits,
    pub charges: u32,
    pub charges_made: u32,
    pub interval_secs: u64,
    pub valid_until: DateTime<Utc>,
    pub last_charge_at: Option<DateTime<Utc>>,
    pub status: MandateStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MandateStatus {
    AwaitingSignature { grant_id: String },
    Active,
    Revoked,
}

/// Why a charge cannot run now.
#[derive(Debug, PartialEq, Eq)]
pub enum ChargeRefusal {
    NotFound,
    NotActive,
    Expired,
    Exhausted,
    TooSoon { retry_after_secs: u64 },
}

#[derive(Clone, Debug)]
pub struct SiweChallenge {
    pub nonce: String,
    pub address: Address,
    pub expires_at: DateTime<Utc>,
    /// Exact EIP-4361 text issued; verify accepts only this message.
    pub message: String,
}

/// A fully specified deposit held until an operator approves it (M10).
#[derive(Clone, Debug)]
pub struct PendingApproval {
    pub plan: DepositPlan,
    pub intent_id: PaymentIntentId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct DeliveryRecord {
    delivery: WebhookDelivery,
    payload: String,
    url: Option<String>,
}

/// A browser the payer chose to remember: resumes sign-in without a wallet signature.
#[derive(Clone, Debug)]
struct DeviceRecord {
    owner: Address,
    wallet: WalletCredentialId,
    expires_at: DateTime<Utc>,
}

pub const DEVICE_TTL_DAYS: i64 = 30;
const DEVICE_PREFIX: &str = "pcc_dev_";

fn device_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub struct Store {
    merchants: Mutex<HashMap<MerchantId, Merchant>>,
    names: Mutex<HashMap<MerchantId, String>>,
    keys: Mutex<HashMap<String, MerchantId>>,
    payouts: Mutex<HashMap<MerchantId, PayoutConfig>>,
    intents: Mutex<HashMap<PaymentIntentId, PaymentIntent>>,
    attempts: Mutex<HashMap<PaymentAttemptId, PaymentAttempt>>,
    session_attempt: Mutex<HashMap<String, PaymentAttemptId>>,
    sessions: Mutex<HashMap<String, CheckoutSession>>,
    events: Mutex<Vec<PaymentEvent>>,
    deliveries: Mutex<HashMap<WebhookDeliveryId, DeliveryRecord>>,
    webhook_secrets: Mutex<HashMap<MerchantId, String>>,
    siwe: Mutex<HashMap<String, SiweChallenge>>,
    wallets: Mutex<HashMap<WalletCredentialId, Address>>,
    session_wallet: Mutex<HashMap<String, WalletCredentialId>>,
    owner_wallet: Mutex<HashMap<Address, WalletCredentialId>>,
    pending: Mutex<HashMap<PaymentAttemptId, PendingApproval>>,
    /// Allowance prepared in this checkout and awaiting the payer's signature.
    pending_grants: Mutex<HashMap<PaymentAttemptId, String>>,
    mandates: Mutex<HashMap<Uuid, Mandate>>,
    /// Charge result per intent, so an idempotent replay returns the first outcome.
    charges: Mutex<HashMap<PaymentIntentId, serde_json::Value>>,
    /// Keyed by SHA-256 of the device token; the token itself is never stored.
    devices: Mutex<HashMap<[u8; 32], DeviceRecord>>,
    /// SHA-256 of unused one-time merchant invites.
    invites: Mutex<HashSet<[u8; 32]>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            merchants: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            keys: Mutex::new(HashMap::new()),
            payouts: Mutex::new(HashMap::new()),
            intents: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            session_attempt: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            deliveries: Mutex::new(HashMap::new()),
            webhook_secrets: Mutex::new(HashMap::new()),
            siwe: Mutex::new(HashMap::new()),
            wallets: Mutex::new(HashMap::new()),
            session_wallet: Mutex::new(HashMap::new()),
            owner_wallet: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            pending_grants: Mutex::new(HashMap::new()),
            mandates: Mutex::new(HashMap::new()),
            charges: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            invites: Mutex::new(HashSet::new()),
        }
    }
}

impl Store {
    pub fn register_merchant(
        &self,
        name: &str,
        api_key: &str,
        payout: PayoutConfig,
        webhook_secret: &str,
    ) -> MerchantId {
        let id = payout.merchant_id;
        let merchant = Merchant {
            id,
            status: MerchantStatus::Active,
            api_key_hash: hash_api_key(api_key),
            webhook_url: None,
            webhook_secret_ref: Some("memory".into()),
        };
        self.keys.lock().expect("store").insert(hash_api_key(api_key), id);
        self.merchants.lock().expect("store").insert(id, merchant);
        self.names.lock().expect("store").insert(id, name.to_owned());
        self.payouts.lock().expect("store").insert(id, payout);
        self.webhook_secrets
            .lock()
            .expect("store")
            .insert(id, webhook_secret.to_owned());
        id
    }

    pub fn authenticate(&self, presented: &str) -> Option<MerchantId> {
        let hash = hash_api_key(presented);
        let keys = self.keys.lock().expect("store");
        let id = keys.get(&hash).copied()?;
        let merchants = self.merchants.lock().expect("store");
        let m = merchants.get(&id)?;
        if m.status != MerchantStatus::Active {
            return None;
        }
        if verify_api_key(presented, &m.api_key_hash) {
            Some(id)
        } else {
            None
        }
    }

    /// Replace the merchant's API key; the old key stops working immediately.
    pub fn rotate_api_key(&self, actor: MerchantId, new_key: &str) -> bool {
        let mut merchants = self.merchants.lock().expect("store");
        let Some(m) = merchants.get_mut(&actor) else {
            return false;
        };
        let mut keys = self.keys.lock().expect("store");
        keys.retain(|_, id| *id != actor);
        let hash = hash_api_key(new_key);
        keys.insert(hash.clone(), actor);
        m.api_key_hash = hash;
        true
    }

    pub fn rotate_webhook_secret(&self, actor: MerchantId, new_secret: &str) -> bool {
        if !self.merchants.lock().expect("store").contains_key(&actor) {
            return false;
        }
        self.webhook_secrets
            .lock()
            .expect("store")
            .insert(actor, new_secret.to_owned());
        true
    }

    pub fn payout(&self, actor: MerchantId) -> Option<PayoutConfig> {
        self.payouts.lock().expect("store").get(&actor).cloned()
    }

    pub fn merchant(&self, id: MerchantId) -> Option<(Merchant, String)> {
        let m = self.merchants.lock().expect("store").get(&id).cloned()?;
        let name = self.names.lock().expect("store").get(&id).cloned().unwrap_or_default();
        Some((m, name))
    }

    pub fn set_webhook_url(&self, actor: MerchantId, url: String, policy: WebhookPolicy) -> Result<(), WebhookError> {
        assert_webhook_url(&url, policy)?;
        let mut merchants = self.merchants.lock().expect("store");
        let m = merchants.get_mut(&actor).ok_or(WebhookError::Malformed)?;
        m.webhook_url = Some(url);
        Ok(())
    }

    pub fn upsert_payout(
        &self,
        actor: MerchantId,
        asset: AssetId,
        receiving: Address,
    ) -> Result<PayoutConfig, pcc_merchants::MerchantError> {
        let cfg = new_payout_config(actor, asset, receiving)?;
        self.payouts.lock().expect("store").insert(actor, cfg.clone());
        Ok(cfg)
    }

    pub fn create_intent(
        &self,
        actor: MerchantId,
        merchant_order_id: &str,
        output: BaseUnits,
        idempotency_key: &str,
        success_url: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(PaymentIntent, CheckoutSession), String> {
        {
            let intents = self.intents.lock().expect("store");
            if let Some(existing) = intents
                .values()
                .find(|i| i.merchant_id == actor && i.idempotency_key == idempotency_key)
            {
                let session = self
                    .sessions
                    .lock()
                    .expect("store")
                    .values()
                    .find(|s| s.intent_id == existing.id)
                    .cloned()
                    .ok_or_else(|| "missing session for existing intent".to_owned())?;
                return Ok((existing.clone(), session));
            }
        }
        let payouts = self.payouts.lock().expect("store");
        let payout = payouts.get(&actor).ok_or_else(|| "no payout config".to_owned())?;
        let intent = new_payment_intent(
            actor,
            payout,
            merchant_order_id,
            output,
            idempotency_key,
            now,
            Duration::minutes(30),
        )
        .map_err(|e| e.to_string())?;
        drop(payouts);
        self.intents.lock().expect("store").insert(intent.id, intent.clone());
        let session = CheckoutSession {
            id: Uuid::new_v4().to_string(),
            merchant_id: actor,
            intent_id: intent.id,
            expires_at: intent.expires_at,
            success_url,
        };
        self.sessions
            .lock()
            .expect("store")
            .insert(session.id.clone(), session.clone());
        Ok((intent, session))
    }

    pub fn get_intent(&self, actor: MerchantId, id: PaymentIntentId) -> Option<PaymentIntent> {
        self.intents
            .lock()
            .expect("store")
            .get(&id)
            .filter(|i| i.merchant_id == actor)
            .cloned()
    }

    pub fn list_intents(&self, actor: MerchantId) -> Vec<PaymentIntent> {
        self.intents
            .lock()
            .expect("store")
            .values()
            .filter(|i| i.merchant_id == actor)
            .cloned()
            .collect()
    }

    pub fn session_for_intent(&self, intent_id: PaymentIntentId) -> Option<CheckoutSession> {
        self.sessions
            .lock()
            .expect("store")
            .values()
            .find(|s| s.intent_id == intent_id)
            .cloned()
    }

    pub fn get_session(&self, id: &str, now: DateTime<Utc>) -> Option<(CheckoutSession, PaymentIntent, String)> {
        let sessions = self.sessions.lock().expect("store");
        let session = sessions.get(id)?.clone();
        if now > session.expires_at {
            return None;
        }
        let intent = self.intents.lock().expect("store").get(&session.intent_id)?.clone();
        let name = self
            .names
            .lock()
            .expect("store")
            .get(&session.merchant_id)
            .cloned()
            .unwrap_or_default();
        Some((session, intent, name))
    }

    pub fn put_siwe(&self, session_id: &str, challenge: SiweChallenge) {
        self.siwe
            .lock()
            .expect("store")
            .insert(session_id.to_owned(), challenge);
    }

    pub fn siwe(&self, session_id: &str) -> Option<SiweChallenge> {
        self.siwe.lock().expect("store").get(session_id).cloned()
    }

    pub fn take_siwe(&self, session_id: &str) -> Option<SiweChallenge> {
        self.siwe.lock().expect("store").remove(session_id)
    }

    /// One TOTP credential per wallet address, reused across checkouts.
    pub fn credential_for_owner(&self, owner: Address) -> WalletCredentialId {
        *self.owner_wallet.lock().expect("store").entry(owner).or_default()
    }

    pub fn bind_wallet(&self, session_id: &str, wallet: WalletCredentialId, owner: Address) {
        self.wallets.lock().expect("store").insert(wallet, owner);
        self.session_wallet
            .lock()
            .expect("store")
            .insert(session_id.to_owned(), wallet);
    }

    pub fn wallet_for_session(&self, session_id: &str) -> Option<(WalletCredentialId, Address)> {
        let id = *self.session_wallet.lock().expect("store").get(session_id)?;
        let addr = *self.wallets.lock().expect("store").get(&id)?;
        Some((id, addr))
    }

    pub fn ensure_attempt(
        &self,
        session: &CheckoutSession,
        intent: &PaymentIntent,
        source: Address,
        source_asset: AssetId,
        max_debit: BaseUnits,
    ) -> Option<PaymentAttempt> {
        if let Some(id) = self.session_attempt.lock().expect("store").get(&session.id).copied() {
            if let Some(existing) = self.attempts.lock().expect("store").get(&id).cloned() {
                if existing.source_wallet == source {
                    return Some(existing);
                }
                // Another wallet signed in: never let it act on the first wallet's attempt.
                if existing.status != PaymentAttemptStatus::Quoted {
                    return None;
                }
                self.pending_grants.lock().expect("store").remove(&existing.id);
            }
        }
        let attempt = PaymentAttempt {
            id: PaymentAttemptId::new(),
            payment_intent_id: intent.id,
            source_wallet: source,
            source_asset,
            quoted_input: max_debit,
            max_debit,
            quote_signature: "checkout-unquoted".into(),
            quote_raw_response: "{}".into(),
            quote_expires_at: intent.expires_at,
            deposit_address: None,
            deposit_memo: None,
            source_tx_hash: None,
            destination_tx_hash: None,
            approval_ref: None,
            status: PaymentAttemptStatus::Quoted,
        };
        self.attempts.lock().expect("store").insert(attempt.id, attempt.clone());
        self.session_attempt
            .lock()
            .expect("store")
            .insert(session.id.clone(), attempt.id);
        Some(attempt)
    }

    /// Returns the token once; only its hash is kept.
    pub fn issue_device(&self, wallet: WalletCredentialId, owner: Address, now: DateTime<Utc>) -> String {
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let token = format!("{DEVICE_PREFIX}{}", hex::encode(raw));
        self.devices.lock().expect("store").insert(
            device_hash(&token),
            DeviceRecord {
                owner,
                wallet,
                expires_at: now + Duration::days(DEVICE_TTL_DAYS),
            },
        );
        token
    }

    pub fn device(&self, token: &str, now: DateTime<Utc>) -> Option<(WalletCredentialId, Address)> {
        if !token.starts_with(DEVICE_PREFIX) {
            return None;
        }
        let mut devices = self.devices.lock().expect("store");
        let key = device_hash(token);
        let record = devices.get(&key)?.clone();
        if now >= record.expires_at {
            devices.remove(&key);
            return None;
        }
        Some((record.wallet, record.owner))
    }

    /// One-time merchant invite. Only the hash is kept.
    pub fn issue_invite(&self) -> String {
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let token = format!("pcc_inv_{}", hex::encode(raw));
        self.invites
            .lock()
            .expect("store")
            .insert(Sha256::digest(token.as_bytes()).into());
        token
    }

    pub fn invite_pending(&self, token: &str) -> bool {
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.invites.lock().expect("store").contains(&key)
    }

    pub fn consume_invite(&self, token: &str) -> bool {
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.invites.lock().expect("store").remove(&key)
    }

    pub fn forget_device(&self, token: &str) -> bool {
        self.devices
            .lock()
            .expect("store")
            .remove(&device_hash(token))
            .is_some()
    }

    /// Switch the source asset of a not-yet-authorized attempt. Amounts reset
    /// to zero until a fresh preview prices the new asset in its own units.
    pub fn set_source_asset(&self, id: PaymentAttemptId, asset: AssetId) -> Option<PaymentAttempt> {
        let mut attempts = self.attempts.lock().expect("store");
        let a = attempts.get_mut(&id)?;
        if a.status != PaymentAttemptStatus::Quoted {
            return None;
        }
        a.source_asset = asset;
        a.quoted_input = BaseUnits::from_u128(0);
        a.max_debit = BaseUnits::from_u128(0);
        "checkout-unquoted".clone_into(&mut a.quote_signature);
        "{}".clone_into(&mut a.quote_raw_response);
        a.deposit_address = None;
        a.deposit_memo = None;
        let out = a.clone();
        drop(attempts);
        // A prepared allowance names one token; a new source abandons it.
        self.pending_grants.lock().expect("store").remove(&id);
        Some(out)
    }

    pub fn apply_preview(&self, id: PaymentAttemptId, quote: &VerifiedQuote, max_debit: BaseUnits) {
        if let Some(a) = self.attempts.lock().expect("store").get_mut(&id) {
            a.quoted_input = quote.amount_in;
            a.max_debit = max_debit;
            a.quote_signature.clone_from(&quote.signature);
            a.quote_raw_response.clone_from(&quote.raw_response_json);
            if let Some(deadline) = quote.deadline {
                a.quote_expires_at = deadline;
            }
        }
    }

    pub fn apply_executable(&self, id: PaymentAttemptId, quote: &VerifiedQuote) {
        if let Some(a) = self.attempts.lock().expect("store").get_mut(&id) {
            a.quoted_input = quote.amount_in;
            a.quote_signature.clone_from(&quote.signature);
            a.quote_raw_response.clone_from(&quote.raw_response_json);
            a.deposit_address.clone_from(&quote.deposit_address);
            a.deposit_memo.clone_from(&quote.deposit_memo);
            if let Some(deadline) = quote.deadline {
                a.quote_expires_at = deadline;
            }
        }
    }

    pub fn mark_broadcast(&self, id: PaymentAttemptId, tx_hash: &str, approval_ref: &str) {
        let intent_id = {
            let mut attempts = self.attempts.lock().expect("store");
            let Some(a) = attempts.get_mut(&id) else {
                return;
            };
            a.source_tx_hash = Some(tx_hash.to_owned());
            a.approval_ref = Some(approval_ref.to_owned());
            a.status = PaymentAttemptStatus::DepositBroadcast;
            a.payment_intent_id
        };
        if let Some(intent) = self.intents.lock().expect("store").get_mut(&intent_id) {
            if let Ok(next) = intent.status.transition(PaymentIntentStatus::Processing) {
                intent.status = next;
            }
        }
    }

    pub fn set_pending_grant(&self, id: PaymentAttemptId, grant_id: String) {
        self.pending_grants.lock().expect("store").insert(id, grant_id);
    }

    pub fn put_mandate(&self, mandate: Mandate) {
        self.mandates.lock().expect("store").insert(mandate.id, mandate);
    }

    pub fn mandate(&self, id: Uuid) -> Option<Mandate> {
        self.mandates.lock().expect("store").get(&id).cloned()
    }

    pub fn set_mandate_status(&self, id: Uuid, status: MandateStatus) -> Option<Mandate> {
        let mut all = self.mandates.lock().expect("store");
        let m = all.get_mut(&id)?;
        m.status = status;
        Some(m.clone())
    }

    /// Newest first.
    pub fn mandates_where(&self, keep: impl Fn(&Mandate) -> bool) -> Vec<Mandate> {
        let mut out: Vec<_> = self
            .mandates
            .lock()
            .expect("store")
            .values()
            .filter(|m| keep(m))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.valid_until.cmp(&a.valid_until));
        out
    }

    /// Atomically checks the mandate's limits and reserves one charge.
    pub fn reserve_charge(
        &self,
        id: Uuid,
        merchant: MerchantId,
        now: DateTime<Utc>,
    ) -> Result<(Mandate, Option<DateTime<Utc>>), ChargeRefusal> {
        let mut all = self.mandates.lock().expect("store");
        let m = all
            .get_mut(&id)
            .filter(|m| m.merchant_id == merchant)
            .ok_or(ChargeRefusal::NotFound)?;
        if m.status != MandateStatus::Active {
            return Err(ChargeRefusal::NotActive);
        }
        if now >= m.valid_until {
            return Err(ChargeRefusal::Expired);
        }
        if m.charges_made >= m.charges {
            return Err(ChargeRefusal::Exhausted);
        }
        if let Some(last) = m.last_charge_at {
            let next = last + Duration::seconds(i64::try_from(m.interval_secs).unwrap_or(i64::MAX));
            if now < next {
                let wait = (next - now).num_seconds().max(1);
                return Err(ChargeRefusal::TooSoon {
                    retry_after_secs: u64::try_from(wait).unwrap_or(1),
                });
            }
        }
        let previous = m.last_charge_at;
        m.charges_made += 1;
        m.last_charge_at = Some(now);
        Ok((m.clone(), previous))
    }

    /// Undo a reservation when nothing was sent (quote or preflight failed).
    pub fn release_charge(&self, id: Uuid, previous_last_charge: Option<DateTime<Utc>>) {
        if let Some(m) = self.mandates.lock().expect("store").get_mut(&id) {
            m.charges_made = m.charges_made.saturating_sub(1);
            m.last_charge_at = previous_last_charge;
        }
    }

    pub fn charge_result(&self, intent: PaymentIntentId) -> Option<serde_json::Value> {
        self.charges.lock().expect("store").get(&intent).cloned()
    }

    pub fn put_charge_result(&self, intent: PaymentIntentId, result: serde_json::Value) {
        self.charges.lock().expect("store").insert(intent, result);
    }

    pub fn is_pending_grant(&self, id: PaymentAttemptId, grant_id: &str) -> bool {
        self.pending_grants
            .lock()
            .expect("store")
            .get(&id)
            .is_some_and(|g| g == grant_id)
    }

    pub fn clear_pending_grant(&self, id: PaymentAttemptId) {
        self.pending_grants.lock().expect("store").remove(&id);
    }

    pub fn put_pending(&self, pending: PendingApproval) {
        self.pending
            .lock()
            .expect("store")
            .insert(pending.plan.attempt_id, pending);
    }

    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        let mut out: Vec<PendingApproval> = self.pending.lock().expect("store").values().cloned().collect();
        out.sort_by_key(|p| p.expires_at);
        out
    }

    pub fn pending_approval(&self, id: PaymentAttemptId) -> Option<PendingApproval> {
        self.pending.lock().expect("store").get(&id).cloned()
    }

    /// Single-use: the first caller gets the plan, later callers get `None`.
    pub fn take_pending(&self, id: PaymentAttemptId) -> Option<PendingApproval> {
        self.pending.lock().expect("store").remove(&id)
    }

    /// Attempts with a broadcast deposit that have not reached a terminal state.
    pub fn in_flight_attempts(&self) -> Vec<PaymentAttempt> {
        self.attempts
            .lock()
            .expect("store")
            .values()
            .filter(|a| {
                a.deposit_address.is_some()
                    && matches!(
                        a.status,
                        PaymentAttemptStatus::DepositBroadcast
                            | PaymentAttemptStatus::DepositSeen
                            | PaymentAttemptStatus::Processing
                    )
            })
            .cloned()
            .collect()
    }

    pub fn intent_by_id(&self, id: PaymentIntentId) -> Option<PaymentIntent> {
        self.intents.lock().expect("store").get(&id).cloned()
    }

    /// Forward-only attempt transition that keeps the intent in step. Returns
    /// `None` when the status is unchanged or the transition is illegal.
    pub fn advance_attempt(
        &self,
        id: PaymentAttemptId,
        to: PaymentAttemptStatus,
        destination_tx: Option<&str>,
    ) -> Option<(PaymentAttempt, PaymentIntent)> {
        let attempt = {
            let mut attempts = self.attempts.lock().expect("store");
            let a = attempts.get_mut(&id)?;
            if a.status == to {
                return None;
            }
            a.status = a.status.transition(to).ok()?;
            if let Some(tx) = destination_tx {
                a.destination_tx_hash = Some(tx.to_owned());
            }
            a.clone()
        };
        let intent_to = match to {
            PaymentAttemptStatus::Settled => PaymentIntentStatus::Succeeded,
            PaymentAttemptStatus::Refunded | PaymentAttemptStatus::Failed | PaymentAttemptStatus::Expired => {
                PaymentIntentStatus::RequiresPayment
            }
            _ => PaymentIntentStatus::Processing,
        };
        let mut intents = self.intents.lock().expect("store");
        let intent = intents.get_mut(&attempt.payment_intent_id)?;
        if let Ok(next) = intent.status.transition(intent_to) {
            intent.status = next;
        }
        Some((attempt, intent.clone()))
    }

    pub fn get_attempt(&self, id: PaymentAttemptId) -> Option<PaymentAttempt> {
        self.attempts.lock().expect("store").get(&id).cloned()
    }

    pub fn attempt_for_session(&self, session_id: &str) -> Option<PaymentAttempt> {
        let id = *self.session_attempt.lock().expect("store").get(session_id)?;
        self.get_attempt(id)
    }

    pub fn mark_attempt_authorized(&self, id: PaymentAttemptId) {
        if let Some(a) = self.attempts.lock().expect("store").get_mut(&id) {
            a.status = PaymentAttemptStatus::Authorized;
        }
    }

    pub fn enqueue_event(&self, intent: &PaymentIntent, r#type: &str, payload: &str) -> WebhookDeliveryId {
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        let event = PaymentEvent {
            id: PaymentEventId::new(),
            payment_intent_id: intent.id,
            r#type: r#type.to_owned(),
            payload_hash: hex::encode(hasher.finalize()),
            created_at: Utc::now(),
        };
        let delivery = WebhookDelivery {
            id: WebhookDeliveryId::new(),
            event_id: event.id,
            attempt: 0,
            status: WebhookDeliveryStatus::Pending,
            next_retry_at: Some(Utc::now()),
        };
        let url = self
            .merchants
            .lock()
            .expect("store")
            .get(&intent.merchant_id)
            .and_then(|m| m.webhook_url.clone());
        self.events.lock().expect("store").push(event);
        self.deliveries.lock().expect("store").insert(
            delivery.id,
            DeliveryRecord {
                delivery: delivery.clone(),
                payload: payload.to_owned(),
                url,
            },
        );
        delivery.id
    }

    /// Pending deliveries whose retry time has passed. Each returned delivery
    /// is leased until `now + lease` so concurrent senders do not double-post;
    /// `mark_delivered` then sets the real next retry.
    pub fn claim_due_deliveries(
        &self,
        now: DateTime<Utc>,
        lease: Duration,
    ) -> Vec<(WebhookDeliveryId, String, String, String)> {
        let pending: Vec<DeliveryRecord> = self
            .deliveries
            .lock()
            .expect("store")
            .values_mut()
            .filter(|rec| {
                rec.delivery.status == WebhookDeliveryStatus::Pending
                    && rec.url.is_some()
                    && rec.delivery.next_retry_at.is_none_or(|t| t <= now)
            })
            .map(|rec| {
                rec.delivery.next_retry_at = Some(now + lease);
                rec.clone()
            })
            .collect();
        let events = self.events.lock().expect("store").clone();
        let intents: HashMap<PaymentIntentId, MerchantId> = self
            .intents
            .lock()
            .expect("store")
            .values()
            .map(|i| (i.id, i.merchant_id))
            .collect();
        let secrets = self.webhook_secrets.lock().expect("store").clone();
        let mut out = Vec::new();
        for rec in pending {
            let Some(url) = rec.url.clone() else {
                continue;
            };
            let Some(event) = events.iter().find(|e| e.id == rec.delivery.event_id) else {
                continue;
            };
            let Some(merchant_id) = intents.get(&event.payment_intent_id) else {
                continue;
            };
            let Some(secret) = secrets.get(merchant_id) else {
                continue;
            };
            let signature = sign_body(secret.as_bytes(), rec.payload.as_bytes());
            out.push((rec.delivery.id, url, rec.payload, signature));
        }
        out
    }

    pub fn sign_payload(&self, merchant: MerchantId, body: &str) -> Option<String> {
        let secrets = self.webhook_secrets.lock().expect("store");
        let secret = secrets.get(&merchant)?;
        Some(sign_body(secret.as_bytes(), body.as_bytes()))
    }

    pub fn delivery_payload(&self, id: WebhookDeliveryId) -> Option<(WebhookDelivery, String)> {
        self.deliveries
            .lock()
            .expect("store")
            .get(&id)
            .map(|r| (r.delivery.clone(), r.payload.clone()))
    }

    pub fn signed_events(
        &self,
        actor: MerchantId,
        intent_id: PaymentIntentId,
    ) -> Option<Vec<(String, String, String, WebhookDeliveryStatus)>> {
        let intent = self.get_intent(actor, intent_id)?;
        let events = self.events.lock().expect("store");
        let deliveries = self.deliveries.lock().expect("store");
        let secret = self.webhook_secrets.lock().expect("store").get(&actor).cloned()?;
        let mut out = Vec::new();
        for event in events.iter().filter(|e| e.payment_intent_id == intent.id) {
            if let Some(rec) = deliveries.values().find(|r| r.delivery.event_id == event.id) {
                let signature = sign_body(secret.as_bytes(), rec.payload.as_bytes());
                out.push((
                    event.r#type.clone(),
                    rec.payload.clone(),
                    signature,
                    rec.delivery.status,
                ));
            }
        }
        Some(out)
    }

    pub fn mark_delivered(&self, id: WebhookDeliveryId, ok: bool) {
        let mut deliveries = self.deliveries.lock().expect("store");
        if let Some(rec) = deliveries.get_mut(&id) {
            rec.delivery.attempt += 1;
            rec.delivery.status = if ok {
                WebhookDeliveryStatus::Delivered
            } else if rec.delivery.attempt >= pcc_settlement::backoff::WEBHOOK_MAX_ATTEMPTS {
                WebhookDeliveryStatus::Failed
            } else {
                WebhookDeliveryStatus::Pending
            };
            rec.delivery.next_retry_at = if rec.delivery.status == WebhookDeliveryStatus::Pending {
                Some(Utc::now() + pcc_settlement::backoff::webhook_retry_delay(rec.delivery.attempt))
            } else {
                None
            };
        }
    }
}
