//! Deterministic in-memory mock adapters for tests and local development.
//! Compiled only with `--features mock` or in tests; never in a release build.

use crate::asset::{AssetId, ChainId, TokenRef};
use crate::domain::{AuthorizationId, PaymentAttemptId, WalletCredentialId};
use crate::quote::{Confidentiality, DepositMode, SwapType, VerifiedQuote};
use crate::status::OneClickStatus;
use crate::traits::{
    AdapterError, Allowance, AllowanceKey, DepositPlan, EnrollmentChallenge, GrantRequest, OneClickAdapter,
    PaymentAuthorizer, PreparedGrant, QuoteSpec, Receipt, SessionGrantPort, Simulation, SourceExecutor, SupportedAsset,
    SwapStatus,
};
use alloy_primitives::B256;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Scripted behaviour for the next call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockBehaviour {
    Ok,
    Timeout,
    Unauthorized,
    RateLimited,
    Malformed,
    NoRoute,
    /// Return a quote whose signature failed verification.
    BadSignature,
    /// Return a `public` quote even though `basic` was requested.
    NonConfidential,
}

pub struct MockOneClick {
    pub now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    pub behaviour: Mutex<MockBehaviour>,
    pub slippage_bps: u32,
    pub statuses: Mutex<HashMap<String, OneClickStatus>>,
    used_auth_refs: Mutex<HashSet<String>>,
    counter: Mutex<u64>,
}

impl Default for MockOneClick {
    fn default() -> Self {
        Self {
            now: Box::new(Utc::now),
            behaviour: Mutex::new(MockBehaviour::Ok),
            slippage_bps: 50,
            statuses: Mutex::new(HashMap::new()),
            used_auth_refs: Mutex::new(HashSet::new()),
            counter: Mutex::new(0),
        }
    }
}

impl MockOneClick {
    #[must_use]
    pub fn with_clock(now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        Self {
            now: Box::new(now),
            ..Self::default()
        }
    }

    pub fn set_behaviour(&self, b: MockBehaviour) {
        *self.behaviour.lock().unwrap() = b;
    }

    pub fn set_status(&self, deposit_address: &str, status: OneClickStatus) {
        self.statuses.lock().unwrap().insert(deposit_address.to_owned(), status);
    }

    fn take_behaviour(&self) -> MockBehaviour {
        let mut b = self.behaviour.lock().unwrap();
        std::mem::replace(&mut *b, MockBehaviour::Ok)
    }

    fn deterministic_deposit_address(&self) -> String {
        let mut c = self.counter.lock().unwrap();
        *c += 1;
        format!("0x{:040x}", 0xDE90_5170_u64 + *c)
    }

    /// Build a quote, applying the scripted behaviour exactly once.
    fn build_quote(&self, spec: &QuoteSpec, dry: bool) -> Result<VerifiedQuote, AdapterError> {
        let behaviour = self.take_behaviour();
        match behaviour {
            MockBehaviour::Timeout => return Err(AdapterError::Timeout),
            MockBehaviour::Unauthorized => return Err(AdapterError::Unauthorized),
            MockBehaviour::RateLimited => return Err(AdapterError::RateLimited),
            MockBehaviour::Malformed => return Err(AdapterError::Malformed("mock: truncated JSON".into())),
            MockBehaviour::NoRoute => return Err(AdapterError::NoRoute("mock: no liquidity".into())),
            MockBehaviour::Ok | MockBehaviour::BadSignature | MockBehaviour::NonConfidential => {}
        }

        if !dry {
            let mut used = self.used_auth_refs.lock().unwrap();
            if !used.insert(spec.authorization_ref.clone()) {
                return Err(AdapterError::Rejected("authorization_ref already used".into()));
            }
        }

        let now = (self.now)();
        // Mock pricing: 1:1 output, plus a slippage buffer on input.
        let min_amount_in = spec.output_amount;
        let buffer = min_amount_in
            .bps_ceil(self.slippage_bps)
            .map_err(|e| AdapterError::Other(e.to_string()))?;
        let amount_in = min_amount_in
            .checked_add(buffer)
            .map_err(|e| AdapterError::Other(e.to_string()))?;

        Ok(VerifiedQuote {
            correlation_id: format!("mock-{}", spec.authorization_ref),
            signature: "ed25519:MOCKSIGNATURE".into(),
            signature_verified: behaviour != MockBehaviour::BadSignature,
            dry,
            swap_type: SwapType::ExactOutput,
            confidentiality: if behaviour == MockBehaviour::NonConfidential {
                Confidentiality::Public
            } else {
                spec.confidentiality
            },
            deposit_mode: DepositMode::Simple,
            origin_asset_id: spec.source_asset.one_click_asset_id.clone(),
            destination_asset_id: spec.destination_asset.one_click_asset_id.clone(),
            recipient: spec.recipient.to_string(),
            refund_to: spec.refund_to.to_string(),
            amount_in,
            min_amount_in,
            amount_out: spec.output_amount,
            min_amount_out: spec.output_amount,
            deposit_address: if dry {
                None
            } else {
                Some(self.deterministic_deposit_address())
            },
            deposit_memo: None,
            quote_timestamp: now,
            deadline: if dry { None } else { Some(spec.deadline) },
            time_estimate_seconds: 120,
            raw_response_json: "{\"mock\":true}".into(),
        })
    }
}

#[async_trait]
impl OneClickAdapter for MockOneClick {
    async fn assets(&self) -> Result<Vec<SupportedAsset>, AdapterError> {
        match self.take_behaviour() {
            MockBehaviour::Timeout => return Err(AdapterError::Timeout),
            MockBehaviour::Malformed => return Err(AdapterError::Malformed("mock".into())),
            _ => {}
        }
        Ok(vec![
            SupportedAsset {
                asset: AssetId {
                    chain_id: ChainId::BASE,
                    token: TokenRef::Erc20 {
                        address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
                    },
                    decimals: 6,
                    one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
                },
                symbol: "USDC".into(),
            },
            SupportedAsset {
                asset: AssetId {
                    chain_id: ChainId::ARBITRUM,
                    token: TokenRef::Erc20 {
                        address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
                    },
                    decimals: 6,
                    one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
                },
                symbol: "USDC".into(),
            },
        ])
    }

    async fn preview(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        self.build_quote(spec, true)
    }

    async fn executable_quote(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        self.build_quote(spec, false)
    }

    async fn verify(&self, raw_response_json: &str) -> Result<VerifiedQuote, AdapterError> {
        Err(AdapterError::Other(format!(
            "mock verify not supported for {} bytes",
            raw_response_json.len()
        )))
    }

    async fn status(&self, deposit_address: &str, _deposit_memo: Option<&str>) -> Result<SwapStatus, AdapterError> {
        match self.take_behaviour() {
            MockBehaviour::Timeout => return Err(AdapterError::Timeout),
            MockBehaviour::Malformed => return Err(AdapterError::Malformed("mock".into())),
            _ => {}
        }
        let status = self
            .statuses
            .lock()
            .unwrap()
            .get(deposit_address)
            .copied()
            .unwrap_or(OneClickStatus::PendingDeposit);
        let destination_tx_hashes = if status == OneClickStatus::Success {
            vec![format!("0x{:064x}", 0xDE57_u64)]
        } else {
            vec![]
        };
        Ok(SwapStatus {
            status,
            updated_at: (self.now)(),
            origin_tx_hashes: vec![],
            destination_tx_hashes,
            amount_in: None,
            amount_out: None,
            refunded_amount: None,
        })
    }

    async fn submit_deposit_tx(&self, _deposit_address: &str, _tx_hash: &str) -> Result<(), AdapterError> {
        Ok(())
    }
}

/// In-memory [`SourceExecutor`] for M4 flow tests. Records the last plan so
/// callers can assert the quote deposit address was bound; the same attempt
/// id always returns the same hash.
pub struct MockSourceExecutor {
    pub behaviour: Mutex<MockBehaviour>,
    executed: Mutex<HashMap<PaymentAttemptId, B256>>,
    pub last_plan: Mutex<Option<DepositPlan>>,
    receipts: Mutex<HashMap<B256, Receipt>>,
}

impl Default for MockSourceExecutor {
    fn default() -> Self {
        Self {
            behaviour: Mutex::new(MockBehaviour::Ok),
            executed: Mutex::new(HashMap::new()),
            last_plan: Mutex::new(None),
            receipts: Mutex::new(HashMap::new()),
        }
    }
}

impl MockSourceExecutor {
    fn hash_for(plan: &DepositPlan) -> B256 {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(plan.attempt_id.to_string().as_bytes());
        hasher.update(plan.to.as_slice());
        hasher.update(plan.amount.to_string().as_bytes());
        B256::from_slice(&hasher.finalize())
    }
}

#[async_trait]
impl SourceExecutor for MockSourceExecutor {
    async fn simulate(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError> {
        match *self.behaviour.lock().unwrap() {
            MockBehaviour::Timeout => return Err(AdapterError::Timeout),
            MockBehaviour::Malformed => return Err(AdapterError::Malformed("mock".into())),
            _ => {}
        }
        Ok(Simulation {
            ok: true,
            gas_estimate: 120_000,
            max_fee_wei: plan.max_fee_wei,
            sender_balance: plan.amount,
            notes: vec!["mock simulate".into()],
        })
    }

    async fn execute_deposit(&self, plan: &DepositPlan) -> Result<B256, AdapterError> {
        if plan.approval_ref.trim().is_empty() {
            return Err(AdapterError::Rejected("empty approval_ref".into()));
        }
        *self.last_plan.lock().unwrap() = Some(plan.clone());
        let mut executed = self.executed.lock().unwrap();
        if let Some(existing) = executed.get(&plan.attempt_id).copied() {
            return Ok(existing);
        }
        let hash = Self::hash_for(plan);
        executed.insert(plan.attempt_id, hash);
        self.receipts.lock().unwrap().insert(
            hash,
            Receipt {
                tx_hash: hash,
                block_number: 1,
                success: true,
                gas_used: 120_000,
            },
        );
        Ok(hash)
    }

    async fn receipt(&self, _chain_id: ChainId, tx_hash: B256) -> Result<Option<Receipt>, AdapterError> {
        Ok(self.receipts.lock().unwrap().get(&tx_hash).cloned())
    }
}

/// In-memory [`SessionGrantPort`] with the worker's semantics: one allowance
/// per payer/chain/token, a use spent once per executed attempt. Accepts any
/// 65-byte hex signature; real recovery is exercised in the worker's tests.
#[derive(Default)]
pub struct MockGrants {
    prepared: Mutex<HashMap<String, GrantRequest>>,
    active: Mutex<HashMap<AllowanceKey, Allowance>>,
    spent: Mutex<HashSet<PaymentAttemptId>>,
    /// Every allowance ever enabled, for assertions.
    pub enabled: Mutex<Vec<Allowance>>,
}

impl MockGrants {
    /// Mirror of the worker's `consume` on a successful execute.
    pub fn spend(&self, plan: &DepositPlan) {
        let TokenRef::Erc20 { address: token } = plan.asset.token else {
            return;
        };
        if !self.spent.lock().unwrap().insert(plan.attempt_id) {
            return;
        }
        let key = AllowanceKey {
            owner: plan.from,
            chain_id: plan.chain_id,
            token,
            mandate: plan.mandate,
        };
        let mut active = self.active.lock().unwrap();
        if let Some(a) = active.get_mut(&key) {
            a.uses_remaining = a.uses_remaining.saturating_sub(1);
            if a.uses_remaining == 0 {
                active.remove(&key);
            }
        }
    }
}

fn unix_now() -> u64 {
    u64::try_from(Utc::now().timestamp()).unwrap_or(0)
}

#[async_trait]
impl SessionGrantPort for MockGrants {
    async fn prepare_grant(&self, req: &GrantRequest) -> Result<PreparedGrant, AdapterError> {
        if req.uses == 0 || req.uses > 20 || !(600..=30 * 24 * 3600).contains(&req.validity_secs) {
            return Err(AdapterError::Rejected(
                "invalid_allowance: uses or validity out of range".into(),
            ));
        }
        let id = format!("grant-{}", self.prepared.lock().unwrap().len() + 1);
        self.prepared.lock().unwrap().insert(id.clone(), req.clone());
        Ok(PreparedGrant {
            grant_id: id,
            typed_data: serde_json::json!({
                "primaryType": "Enable",
                "domain": {"chainId": req.key.chain_id.0, "verifyingContract": req.key.owner.to_string()},
                "message": {
                    "token": req.key.token.to_string(),
                    "maxAmount": req.max_amount.to_string(),
                    "uses": req.uses,
                },
            }),
            session_signer: alloy_primitives::Address::repeat_byte(0x5e),
            uses: req.uses,
            valid_until: unix_now() + req.validity_secs,
            expires_at: Utc::now() + Duration::minutes(10),
        })
    }

    async fn allowance(&self, key: &AllowanceKey) -> Result<Option<Allowance>, AdapterError> {
        let mut active = self.active.lock().unwrap();
        if active.get(key).is_some_and(|a| a.valid_until <= unix_now()) {
            active.remove(key);
        }
        Ok(active.get(key).cloned())
    }

    async fn revoke_allowance(&self, key: &AllowanceKey) -> Result<bool, AdapterError> {
        Ok(self.active.lock().unwrap().remove(key).is_some())
    }

    async fn enable_grant(&self, grant_id: &str, signature: &str) -> Result<Allowance, AdapterError> {
        let hex_ok = signature
            .strip_prefix("0x")
            .is_some_and(|h| h.len() == 130 && h.bytes().all(|b| b.is_ascii_hexdigit()));
        if !hex_ok {
            return Err(AdapterError::Rejected("signature does not recover to owner".into()));
        }
        let req = self
            .prepared
            .lock()
            .unwrap()
            .remove(grant_id)
            .ok_or_else(|| AdapterError::Rejected("unknown or used grant".into()))?;
        let enabled = Allowance {
            key: req.key,
            max_amount: req.max_amount,
            uses_remaining: req.uses,
            valid_until: unix_now() + req.validity_secs,
        };
        self.active.lock().unwrap().insert(req.key, enabled.clone());
        self.enabled.lock().unwrap().push(enabled.clone());
        Ok(enabled)
    }
}

/// Combined mock used by hosted-checkout API tests.
#[derive(Default)]
pub struct MockCheckout {
    pub quotes: MockOneClick,
    pub executor: MockSourceExecutor,
    pub grants: MockGrants,
}

#[async_trait]
impl SessionGrantPort for MockCheckout {
    async fn prepare_grant(&self, req: &GrantRequest) -> Result<PreparedGrant, AdapterError> {
        self.grants.prepare_grant(req).await
    }
    async fn enable_grant(&self, grant_id: &str, signature: &str) -> Result<Allowance, AdapterError> {
        self.grants.enable_grant(grant_id, signature).await
    }
    async fn allowance(&self, key: &AllowanceKey) -> Result<Option<Allowance>, AdapterError> {
        self.grants.allowance(key).await
    }
    async fn revoke_allowance(&self, key: &AllowanceKey) -> Result<bool, AdapterError> {
        self.grants.revoke_allowance(key).await
    }
}

#[async_trait]
impl OneClickAdapter for MockCheckout {
    async fn assets(&self) -> Result<Vec<SupportedAsset>, AdapterError> {
        self.quotes.assets().await
    }
    async fn preview(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        self.quotes.preview(spec).await
    }
    async fn executable_quote(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        self.quotes.executable_quote(spec).await
    }
    async fn verify(&self, raw_response_json: &str) -> Result<VerifiedQuote, AdapterError> {
        self.quotes.verify(raw_response_json).await
    }
    async fn status(&self, deposit_address: &str, deposit_memo: Option<&str>) -> Result<SwapStatus, AdapterError> {
        self.quotes.status(deposit_address, deposit_memo).await
    }
    async fn submit_deposit_tx(&self, deposit_address: &str, tx_hash: &str) -> Result<(), AdapterError> {
        self.quotes.submit_deposit_tx(deposit_address, tx_hash).await
    }
}

#[async_trait]
impl SourceExecutor for MockCheckout {
    async fn simulate(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError> {
        self.executor.simulate(plan).await
    }
    async fn execute_deposit(&self, plan: &DepositPlan) -> Result<B256, AdapterError> {
        let tx = self.executor.execute_deposit(plan).await?;
        self.grants.spend(plan);
        Ok(tx)
    }
    async fn receipt(&self, chain_id: ChainId, tx_hash: B256) -> Result<Option<Receipt>, AdapterError> {
        self.executor.receipt(chain_id, tx_hash).await
    }
}

/// Dev-only authorizer: accepts the fixed code `"000000"` once per attempt.
/// Must never be reachable in a deployed build (see `ALLOW_MOCK_AUTHENTICATOR`).
#[derive(Default)]
pub struct MockAuthorizer {
    verified: Mutex<HashMap<AuthorizationId, (PaymentAttemptId, bool)>>,
    attempts_seen: Mutex<HashSet<PaymentAttemptId>>,
}

#[async_trait]
impl PaymentAuthorizer for MockAuthorizer {
    async fn enroll(&self, wallet: WalletCredentialId) -> Result<EnrollmentChallenge, AdapterError> {
        Ok(EnrollmentChallenge {
            wallet_credential_id: wallet,
            provisioning_secret_display: "otpauth://totp/MOCK?secret=MOCK".into(),
            expires_at: Utc::now() + Duration::minutes(5),
        })
    }

    async fn verify_for_attempt(
        &self,
        _wallet: WalletCredentialId,
        attempt: PaymentAttemptId,
        amount_bound_hash: &str,
        code: &str,
    ) -> Result<AuthorizationId, AdapterError> {
        if code != "000000" || amount_bound_hash.is_empty() {
            return Err(AdapterError::Rejected("invalid code".into()));
        }
        if !self.attempts_seen.lock().unwrap().insert(attempt) {
            return Err(AdapterError::Rejected("attempt already authorized".into()));
        }
        let id = AuthorizationId::new();
        self.verified.lock().unwrap().insert(id, (attempt, false));
        Ok(id)
    }

    async fn consume(&self, authorization: AuthorizationId) -> Result<(), AdapterError> {
        let mut m = self.verified.lock().unwrap();
        match m.get_mut(&authorization) {
            None => Err(AdapterError::Rejected("unknown authorization".into())),
            Some((_, true)) => Err(AdapterError::Rejected("authorization already consumed".into())),
            Some(slot) => {
                slot.1 = true;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::BaseUnits;
    use crate::quote::{validate_quote, QuoteExpectation};

    fn spec() -> QuoteSpec {
        QuoteSpec {
            source_asset: AssetId {
                chain_id: ChainId::BASE,
                token: TokenRef::Erc20 {
                    address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
            },
            destination_asset: AssetId {
                chain_id: ChainId::ARBITRUM,
                token: TokenRef::Erc20 {
                    address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
            },
            output_amount: BaseUnits::from_u128(1_000_000),
            recipient: "0x1111111111111111111111111111111111111111".parse().unwrap(),
            refund_to: "0x2222222222222222222222222222222222222222".parse().unwrap(),
            confidentiality: Confidentiality::Basic,
            slippage_bps: 100,
            deadline: Utc::now() + Duration::hours(2),
            authorization_ref: "auth-1".into(),
        }
    }

    fn expectation(s: &QuoteSpec) -> QuoteExpectation {
        QuoteExpectation {
            source_asset: s.source_asset.clone(),
            destination_asset: s.destination_asset.clone(),
            recipient: s.recipient.to_string(),
            refund_to: s.refund_to.to_string(),
            output_amount: s.output_amount,
            max_debit: BaseUnits::from_u128(1_050_000),
            max_slippage_bps: 100,
            require_confidentiality: Confidentiality::Basic,
            require_executable: true,
            min_remaining_validity: Duration::minutes(10),
            max_quote_age: Duration::minutes(5),
        }
    }

    #[tokio::test]
    async fn mock_quote_passes_business_validation() {
        let m = MockOneClick::default();
        let s = spec();
        let q = m.executable_quote(&s).await.unwrap();
        validate_quote(&q, &expectation(&s), Utc::now()).unwrap();
        assert!(q.deposit_address.is_some());
    }

    #[tokio::test]
    async fn mock_rejects_auth_ref_replay() {
        let m = MockOneClick::default();
        let s = spec();
        m.executable_quote(&s).await.unwrap();
        let err = m.executable_quote(&s).await.unwrap_err();
        assert!(matches!(err, AdapterError::Rejected(_)));
    }

    #[tokio::test]
    async fn mock_failure_modes_surface_as_adapter_errors() {
        let m = MockOneClick::default();
        let s = spec();
        for (b, pred) in [
            (
                MockBehaviour::Timeout,
                (|e: &AdapterError| matches!(e, AdapterError::Timeout)) as fn(&AdapterError) -> bool,
            ),
            (MockBehaviour::Unauthorized, |e| matches!(e, AdapterError::Unauthorized)),
            (MockBehaviour::RateLimited, |e| matches!(e, AdapterError::RateLimited)),
            (MockBehaviour::Malformed, |e| matches!(e, AdapterError::Malformed(_))),
            (MockBehaviour::NoRoute, |e| matches!(e, AdapterError::NoRoute(_))),
        ] {
            m.set_behaviour(b.clone());
            let err = m.preview(&s).await.unwrap_err();
            assert!(pred(&err), "{b:?} produced {err:?}");
        }
    }

    #[tokio::test]
    async fn poisoned_quotes_fail_validation() {
        let m = MockOneClick::default();
        let s = spec();
        m.set_behaviour(MockBehaviour::BadSignature);
        let q = m.preview(&s).await.unwrap();
        let mut exp = expectation(&s);
        exp.require_executable = false;
        assert!(validate_quote(&q, &exp, Utc::now()).is_err());

        m.set_behaviour(MockBehaviour::NonConfidential);
        let q = m.preview(&s).await.unwrap();
        assert!(validate_quote(&q, &exp, Utc::now()).is_err());
    }

    #[tokio::test]
    async fn mock_authorizer_is_single_use_per_attempt() {
        let a = MockAuthorizer::default();
        let w = WalletCredentialId::new();
        let attempt = PaymentAttemptId::new();
        assert!(a.verify_for_attempt(w, attempt, "h", "123456").await.is_err());
        let id = a.verify_for_attempt(w, attempt, "h", "000000").await.unwrap();
        assert!(a.verify_for_attempt(w, attempt, "h", "000000").await.is_err());
        a.consume(id).await.unwrap();
        assert!(a.consume(id).await.is_err());
    }

    #[tokio::test]
    async fn mock_source_executor_is_idempotent_per_attempt() {
        let exec = MockSourceExecutor::default();
        let plan = DepositPlan {
            attempt_id: PaymentAttemptId::new(),
            chain_id: ChainId::SEPOLIA,
            asset: AssetId {
                chain_id: ChainId::SEPOLIA,
                token: TokenRef::Erc20 {
                    address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "x".into(),
            },
            from: "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap(),
            to: "0x76b4c56085ED136a8744D52bE956396624a730E8".parse().unwrap(),
            amount: BaseUnits::from_u128(1_020_000),
            max_fee_wei: BaseUnits::from_u128(1),
            approval_ref: "appr".into(),
            mandate: None,
        };
        let a = exec.execute_deposit(&plan).await.unwrap();
        let b = exec.execute_deposit(&plan).await.unwrap();
        assert_eq!(a, b);
        assert!(exec.receipt(ChainId::SEPOLIA, a).await.unwrap().unwrap().success);
    }
}
