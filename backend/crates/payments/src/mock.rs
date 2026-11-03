//! Deterministic in-memory mock adapters for tests and local development.
//! Compiled only with `--features mock` or in tests; never in a release build.

use crate::asset::{AssetId, ChainId, TokenRef};
use crate::domain::{AuthorizationId, PaymentAttemptId, WalletCredentialId};
use crate::quote::{Confidentiality, DepositMode, SwapType, VerifiedQuote};
use crate::status::OneClickStatus;
use crate::traits::{
    AdapterError, EnrollmentChallenge, OneClickAdapter, PaymentAuthorizer, QuoteSpec, SupportedAsset, SwapStatus,
};
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
        Ok(SwapStatus {
            status,
            updated_at: (self.now)(),
            origin_tx_hashes: vec![],
            destination_tx_hashes: vec![],
            amount_in: None,
            amount_out: None,
            refunded_amount: None,
        })
    }

    async fn submit_deposit_tx(&self, _deposit_address: &str, _tx_hash: &str) -> Result<(), AdapterError> {
        Ok(())
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
}
