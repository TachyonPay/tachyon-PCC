//! Wallet-linked TOTP authorizer. Secrets are wrapped at rest and never logged.
//! A verified code is bound to one `PaymentAttempt` via `amount_bound_hash`
//! and consumed exactly once.

use crate::totp::{self, unwrap_secret, wrap_secret};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use pcc_payments::domain::{AuthorizationId, PaymentAttemptId, WalletCredentialId};
use pcc_payments::traits::{AdapterError, EnrollmentChallenge, PaymentAuthorizer};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::minutes(5);
const ENROLL_TTL: Duration = Duration::minutes(5);
const AUTH_TTL: Duration = Duration::minutes(5);

struct Slot {
    owner: String,
    wrapped_secret: Vec<u8>,
    pending: bool,
    enroll_expires: DateTime<Utc>,
    used_timesteps: HashSet<u64>,
    failures: u32,
    locked_until: Option<DateTime<Utc>>,
}

struct AuthRecord {
    amount_bound_hash: String,
    consumed: bool,
    expires_at: DateTime<Utc>,
}

pub struct TotpAuthorizer {
    master: Vec<u8>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    slots: Mutex<HashMap<WalletCredentialId, Slot>>,
    auths: Mutex<HashMap<AuthorizationId, AuthRecord>>,
    /// Attempts that already received a verified authorization.
    attempts_bound: Mutex<HashSet<PaymentAttemptId>>,
}

impl TotpAuthorizer {
    #[must_use]
    pub fn new(master: Vec<u8>) -> Self {
        Self::with_clock(master, Box::new(Utc::now))
    }

    #[must_use]
    pub fn with_clock(master: Vec<u8>, now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>) -> Self {
        Self {
            master,
            now,
            slots: Mutex::new(HashMap::new()),
            auths: Mutex::new(HashMap::new()),
            attempts_bound: Mutex::new(HashSet::new()),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    pub fn bind_owner(&self, wallet: WalletCredentialId, owner: &str) {
        let mut slots = self.slots.lock().unwrap();
        slots.entry(wallet).or_insert_with(|| Slot {
            owner: owner.to_owned(),
            wrapped_secret: Vec::new(),
            pending: true,
            enroll_expires: self.now() + ENROLL_TTL,
            used_timesteps: HashSet::new(),
            failures: 0,
            locked_until: None,
        });
        if let Some(s) = slots.get_mut(&wallet) {
            owner.clone_into(&mut s.owner);
        }
    }

    /// Drops a confirmed secret so the wallet can enroll a new authenticator.
    /// The caller must already have proved control of the wallet (SIWE).
    pub fn reset_enrollment(&self, wallet: WalletCredentialId) -> bool {
        let mut slots = self.slots.lock().unwrap();
        let Some(slot) = slots.get_mut(&wallet) else {
            return false;
        };
        if slot.pending || slot.wrapped_secret.is_empty() {
            return false;
        }
        slot.wrapped_secret.clear();
        slot.pending = true;
        slot.used_timesteps.clear();
        slot.failures = 0;
        slot.locked_until = None;
        slot.enroll_expires = self.now();
        true
    }

    /// True once the wallet confirmed a TOTP secret; later payments only need a code.
    pub fn is_enrolled(&self, wallet: WalletCredentialId) -> bool {
        self.slots
            .lock()
            .unwrap()
            .get(&wallet)
            .is_some_and(|s| !s.pending && !s.wrapped_secret.is_empty())
    }

    pub fn confirm_enrollment(&self, wallet: WalletCredentialId, owner: &str, code: &str) -> Result<(), AdapterError> {
        let now = self.now();
        let mut slots = self.slots.lock().unwrap();
        let slot = slots
            .get_mut(&wallet)
            .ok_or_else(|| AdapterError::Rejected("unknown wallet".into()))?;
        if slot.owner != owner {
            return Err(AdapterError::Rejected("non-owner enrollment".into()));
        }
        if !slot.pending {
            return Err(AdapterError::Rejected("already enrolled".into()));
        }
        if now > slot.enroll_expires {
            return Err(AdapterError::Rejected("enrollment expired".into()));
        }
        let secret = unwrap_secret(&self.master, wallet.0.as_bytes(), &slot.wrapped_secret)
            .map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        let bytes = totp::decode_secret(&secret).map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        totp::verify(&bytes, code, u64::try_from(now.timestamp()).unwrap_or(0))
            .map_err(|_| AdapterError::Rejected("wrong OTP".into()))?;
        slot.pending = false;
        Ok(())
    }

    pub fn generate_current_code_for_tests(&self, wallet: WalletCredentialId) -> Result<String, AdapterError> {
        let now = self.now();
        let slots = self.slots.lock().unwrap();
        let slot = slots
            .get(&wallet)
            .ok_or_else(|| AdapterError::Rejected("unknown wallet".into()))?;
        let secret = unwrap_secret(&self.master, wallet.0.as_bytes(), &slot.wrapped_secret)
            .map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        let bytes = totp::decode_secret(&secret).map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        Ok(format!(
            "{:06}",
            totp::code_at(&bytes, u64::try_from(now.timestamp()).unwrap_or(0))
        ))
    }
}

#[async_trait]
impl PaymentAuthorizer for TotpAuthorizer {
    async fn enroll(&self, wallet: WalletCredentialId) -> Result<EnrollmentChallenge, AdapterError> {
        let now = self.now();
        let secret = totp::generate_secret();
        let wrapped = wrap_secret(&self.master, wallet.0.as_bytes(), &secret);
        let mut slots = self.slots.lock().unwrap();
        if slots.get(&wallet).is_some_and(|s| !s.pending) {
            return Err(AdapterError::Rejected("already enrolled".into()));
        }
        let owner = slots.get(&wallet).map(|s| s.owner.clone()).unwrap_or_default();
        slots.insert(
            wallet,
            Slot {
                owner,
                wrapped_secret: wrapped,
                pending: true,
                enroll_expires: now + ENROLL_TTL,
                used_timesteps: HashSet::new(),
                failures: 0,
                locked_until: None,
            },
        );
        Ok(EnrollmentChallenge {
            wallet_credential_id: wallet,
            provisioning_secret_display: totp::otpauth_uri(&wallet.to_string(), &secret),
            expires_at: now + ENROLL_TTL,
        })
    }

    async fn verify_for_attempt(
        &self,
        wallet: WalletCredentialId,
        attempt: PaymentAttemptId,
        amount_bound_hash: &str,
        code: &str,
    ) -> Result<AuthorizationId, AdapterError> {
        if amount_bound_hash.is_empty() {
            return Err(AdapterError::Rejected("empty amount_bound_hash".into()));
        }
        let now = self.now();
        let unix = u64::try_from(now.timestamp()).unwrap_or(0);
        let mut slots = self.slots.lock().unwrap();
        let slot = slots
            .get_mut(&wallet)
            .ok_or_else(|| AdapterError::Rejected("unknown wallet".into()))?;
        if slot.pending {
            return Err(AdapterError::Rejected("wallet not enrolled".into()));
        }
        if slot.locked_until.is_some_and(|t| now < t) {
            return Err(AdapterError::Rejected("locked out".into()));
        }
        let secret = unwrap_secret(&self.master, wallet.0.as_bytes(), &slot.wrapped_secret)
            .map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        let bytes = totp::decode_secret(&secret).map_err(|_| AdapterError::Rejected("corrupt TOTP slot".into()))?;
        let Ok(ts) = totp::verify(&bytes, code, unix) else {
            slot.failures = slot.failures.saturating_add(1);
            if slot.failures >= MAX_FAILURES {
                slot.locked_until = Some(now + LOCKOUT);
            }
            return Err(AdapterError::Rejected("wrong OTP".into()));
        };
        if !slot.used_timesteps.insert(ts) {
            return Err(AdapterError::Rejected("OTP replay".into()));
        }
        slot.failures = 0;
        drop(slots);

        let mut bound = self.attempts_bound.lock().unwrap();
        if !bound.insert(attempt) {
            return Err(AdapterError::Rejected("attempt already authorized".into()));
        }
        drop(bound);

        let id = AuthorizationId::new();
        self.auths.lock().unwrap().insert(
            id,
            AuthRecord {
                amount_bound_hash: amount_bound_hash.to_owned(),
                consumed: false,
                expires_at: now + AUTH_TTL,
            },
        );
        Ok(id)
    }

    async fn consume(&self, authorization: AuthorizationId) -> Result<(), AdapterError> {
        let now = self.now();
        let mut auths = self.auths.lock().unwrap();
        let rec = auths
            .get_mut(&authorization)
            .ok_or_else(|| AdapterError::Rejected("unknown authorization".into()))?;
        if rec.consumed {
            return Err(AdapterError::Rejected("authorization already consumed".into()));
        }
        if now > rec.expires_at {
            return Err(AdapterError::Rejected("authorization expired".into()));
        }
        rec.consumed = true;
        Ok(())
    }
}

impl TotpAuthorizer {
    /// Refuse consume when the caller presents a different binding hash
    /// (amount / merchant changed after OTP).
    pub async fn consume_bound(
        &self,
        authorization: AuthorizationId,
        amount_bound_hash: &str,
    ) -> Result<(), AdapterError> {
        {
            let auths = self.auths.lock().unwrap();
            let rec = auths
                .get(&authorization)
                .ok_or_else(|| AdapterError::Rejected("unknown authorization".into()))?;
            if rec.amount_bound_hash != amount_bound_hash {
                return Err(AdapterError::Rejected("amount_bound_hash mismatch".into()));
            }
        }
        self.consume(authorization).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn clock(origin: DateTime<Utc>, offset_secs: &'static AtomicI64) -> Box<dyn Fn() -> DateTime<Utc> + Send + Sync> {
        Box::new(move || origin + Duration::seconds(offset_secs.load(Ordering::SeqCst)))
    }

    #[tokio::test]
    async fn enroll_confirm_and_single_use_per_attempt() {
        let a = TotpAuthorizer::new(b"test-master".to_vec());
        let w = WalletCredentialId::new();
        a.bind_owner(w, "0xabc");
        let ch = a.enroll(w).await.unwrap();
        assert!(ch.provisioning_secret_display.starts_with("otpauth://"));
        let code = a.generate_current_code_for_tests(w).unwrap();
        a.confirm_enrollment(w, "0xabc", &code).unwrap();
        let attempt = PaymentAttemptId::new();
        let id = a.verify_for_attempt(w, attempt, "hash-1", &code).await.unwrap();
        assert!(a.verify_for_attempt(w, attempt, "hash-1", &code).await.is_err());
        a.consume(id).await.unwrap();
        assert!(a.consume(id).await.is_err());
    }

    #[tokio::test]
    async fn confirmed_secret_is_never_replaced() {
        let a = TotpAuthorizer::new(b"m".to_vec());
        let w = WalletCredentialId::new();
        a.bind_owner(w, "0xabc");
        assert!(!a.is_enrolled(w));
        a.enroll(w).await.unwrap();
        a.enroll(w).await.expect("an unconfirmed enrollment can be restarted");
        assert!(!a.is_enrolled(w));
        let code = a.generate_current_code_for_tests(w).unwrap();
        a.confirm_enrollment(w, "0xabc", &code).unwrap();
        assert!(a.is_enrolled(w));
        a.bind_owner(w, "0xabc");
        assert!(a.is_enrolled(w), "a later SIWE keeps the enrollment");
        let err = a.enroll(w).await.unwrap_err();
        assert!(err.to_string().contains("already enrolled"));
        assert!(a.is_enrolled(w));
    }

    #[tokio::test]
    async fn non_owner_enrollment_and_expired_qr() {
        static OFFSET: AtomicI64 = AtomicI64::new(0);
        let origin = Utc::now();
        let a = TotpAuthorizer::with_clock(b"m".to_vec(), clock(origin, &OFFSET));
        let w = WalletCredentialId::new();
        a.bind_owner(w, "0xowner");
        a.enroll(w).await.unwrap();
        let code = a.generate_current_code_for_tests(w).unwrap();
        assert!(a.confirm_enrollment(w, "0xthief", &code).is_err());
        OFFSET.store(400, Ordering::SeqCst);
        assert!(a.confirm_enrollment(w, "0xowner", &code).is_err());
    }

    #[tokio::test]
    async fn wrong_otp_lockout_and_replay() {
        let a = TotpAuthorizer::new(b"m".to_vec());
        let w = WalletCredentialId::new();
        a.bind_owner(w, "o");
        a.enroll(w).await.unwrap();
        let good = a.generate_current_code_for_tests(w).unwrap();
        a.confirm_enrollment(w, "o", &good).unwrap();
        for _ in 0..MAX_FAILURES {
            let _ = a.verify_for_attempt(w, PaymentAttemptId::new(), "h", "000000").await;
        }
        assert!(a
            .verify_for_attempt(w, PaymentAttemptId::new(), "h", &good)
            .await
            .unwrap_err()
            .to_string()
            .contains("locked"));
    }

    #[tokio::test]
    async fn two_payments_cannot_share_one_code() {
        let a = TotpAuthorizer::new(b"m".to_vec());
        let w = WalletCredentialId::new();
        a.bind_owner(w, "o");
        a.enroll(w).await.unwrap();
        let code = a.generate_current_code_for_tests(w).unwrap();
        a.confirm_enrollment(w, "o", &code).unwrap();
        a.verify_for_attempt(w, PaymentAttemptId::new(), "h1", &code)
            .await
            .unwrap();
        let err = a
            .verify_for_attempt(w, PaymentAttemptId::new(), "h2", &code)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("replay") || err.to_string().contains("already"));
    }

    #[tokio::test]
    async fn changed_binding_hash_is_rejected_on_consume() {
        let a = TotpAuthorizer::new(b"m".to_vec());
        let w = WalletCredentialId::new();
        a.bind_owner(w, "o");
        a.enroll(w).await.unwrap();
        let code = a.generate_current_code_for_tests(w).unwrap();
        a.confirm_enrollment(w, "o", &code).unwrap();
        let id = a
            .verify_for_attempt(w, PaymentAttemptId::new(), "hash-orig", &code)
            .await
            .unwrap();
        assert!(a.consume_bound(id, "hash-changed").await.is_err());
        a.consume_bound(id, "hash-orig").await.unwrap();
    }
}
