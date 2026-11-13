//! Adapter traits. Implementations:
//! - `OneClickAdapter`: `pcc-integration-client` (HTTP to the private TS worker), `mock::MockOneClick`.
//! - `SourceExecutor`: `pcc-evm` plain EOA executor (M2); `IntegrationClient` session-key UserOp executor (M4).
//! - `PaymentAuthorizer`: `pcc-auth` TOTP (M5); `mock::MockAuthorizer` (dev only).
//! - `SigningProvider`: signs only a bounded, pre-approved operation.

use crate::amount::BaseUnits;
use crate::asset::{AssetId, ChainId};
use crate::domain::{AuthorizationId, PaymentAttemptId, WalletCredentialId};
use crate::quote::{Confidentiality, VerifiedQuote};
use crate::status::OneClickStatus;
use alloy_primitives::{Address, B256};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("upstream unavailable: {0}")]
    Unavailable(String),
    #[error("upstream timeout")]
    Timeout,
    #[error("upstream rejected the request: {0}")]
    Rejected(String),
    #[error("unauthorized against upstream")]
    Unauthorized,
    #[error("rate limited by upstream")]
    RateLimited,
    #[error("malformed upstream response: {0}")]
    Malformed(String),
    #[error("no route/quote available: {0}")]
    NoRoute(String),
    #[error("internal boundary schema mismatch: {0}")]
    Schema(String),
    #[error("{0}")]
    Other(String),
}

/// Request for a quote. Rust always fills `recipient` from the payout snapshot
/// and `refund_to` from the payer wallet; the browser never supplies these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteSpec {
    pub source_asset: AssetId,
    pub destination_asset: AssetId,
    pub output_amount: BaseUnits,
    pub recipient: Address,
    pub refund_to: Address,
    pub confidentiality: Confidentiality,
    pub slippage_bps: u32,
    pub deadline: DateTime<Utc>,
    /// Single-use reference issued by Rust; TS rejects reuse.
    pub authorization_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedAsset {
    pub asset: AssetId,
    pub symbol: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapStatus {
    pub status: OneClickStatus,
    pub updated_at: DateTime<Utc>,
    pub origin_tx_hashes: Vec<String>,
    pub destination_tx_hashes: Vec<String>,
    pub amount_in: Option<BaseUnits>,
    pub amount_out: Option<BaseUnits>,
    pub refunded_amount: Option<BaseUnits>,
}

#[async_trait]
pub trait OneClickAdapter: Send + Sync {
    /// Supported EVM assets (already filtered to mapped chains).
    async fn assets(&self) -> Result<Vec<SupportedAsset>, AdapterError>;
    /// `dry: true` quote: no deposit address, used for previews.
    async fn preview(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError>;
    /// `dry: false` quote: allocates a deposit address. Only after authorization.
    async fn executable_quote(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError>;
    /// Re-verify a stored raw quote (e.g. after restart).
    async fn verify(&self, raw_response_json: &str) -> Result<VerifiedQuote, AdapterError>;
    async fn status(&self, deposit_address: &str, deposit_memo: Option<&str>) -> Result<SwapStatus, AdapterError>;
    async fn submit_deposit_tx(&self, deposit_address: &str, tx_hash: &str) -> Result<(), AdapterError>;
}

/// A bounded deposit the executor may perform. Built by Rust from a validated
/// quote; executors must refuse anything not matching their own checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositPlan {
    pub attempt_id: PaymentAttemptId,
    pub chain_id: ChainId,
    pub asset: AssetId,
    pub from: Address,
    pub to: Address,
    pub amount: BaseUnits,
    pub max_fee_wei: BaseUnits,
    /// Owner's explicit per-transaction approval reference.
    pub approval_ref: String,
    /// Autopay mandate whose allowance pays this; `None` = the checkout allowance.
    #[serde(default)]
    pub mandate: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Simulation {
    pub ok: bool,
    pub gas_estimate: u64,
    pub max_fee_wei: BaseUnits,
    pub sender_balance: BaseUnits,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub tx_hash: B256,
    pub block_number: u64,
    pub success: bool,
    pub gas_used: u64,
}

#[async_trait]
pub trait SourceExecutor: Send + Sync {
    async fn simulate(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError>;
    /// Broadcasts exactly once; must be idempotent per `plan.attempt_id`.
    async fn execute_deposit(&self, plan: &DepositPlan) -> Result<B256, AdapterError>;
    async fn receipt(&self, chain_id: ChainId, tx_hash: B256) -> Result<Option<Receipt>, AdapterError>;
}

/// One checkout allowance per payer, chain and token, plus one per autopay mandate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AllowanceKey {
    pub owner: Address,
    pub chain_id: ChainId,
    pub token: Address,
    pub mandate: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRequest {
    pub key: AllowanceKey,
    /// Inclusive cap for each transfer.
    pub max_amount: BaseUnits,
    pub uses: u32,
    pub validity_secs: u64,
}

/// An allowance awaiting the payer's EIP-712 signature.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedGrant {
    pub grant_id: String,
    pub typed_data: serde_json::Value,
    pub session_signer: Address,
    pub uses: u32,
    pub valid_until: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowance {
    pub key: AllowanceKey,
    pub max_amount: BaseUnits,
    pub uses_remaining: u32,
    pub valid_until: u64,
}

impl Allowance {
    /// Whether this allowance can pay `amount` of `key`'s token at `now_unix`.
    #[must_use]
    pub fn covers(&self, key: &AllowanceKey, amount: BaseUnits, now_unix: u64) -> bool {
        self.key == *key && self.max_amount >= amount && self.uses_remaining > 0 && self.valid_until > now_unix
    }
}

/// Per-payer spending allowances: the payer's wallet signs the Kernel enable
/// payload once; the executor never holds the payer's key and counts uses.
#[async_trait]
pub trait SessionGrantPort: Send + Sync {
    async fn prepare_grant(&self, req: &GrantRequest) -> Result<PreparedGrant, AdapterError>;
    async fn enable_grant(&self, grant_id: &str, signature: &str) -> Result<Allowance, AdapterError>;
    /// `None` when nothing is active (never enabled, used up, expired, revoked, or the executor restarted).
    async fn allowance(&self, key: &AllowanceKey) -> Result<Option<Allowance>, AdapterError>;
    /// Destroys the session key; returns false when nothing was active.
    async fn revoke_allowance(&self, key: &AllowanceKey) -> Result<bool, AdapterError>;
}

/// Combined quote + session-deposit ports used by hosted checkout.
pub trait CheckoutPorts: OneClickAdapter + SourceExecutor + SessionGrantPort {}

impl<T: OneClickAdapter + SourceExecutor + SessionGrantPort> CheckoutPorts for T {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentChallenge {
    pub wallet_credential_id: WalletCredentialId,
    /// Shown once to the user (e.g. otpauth URI). Never logged or stored.
    pub provisioning_secret_display: String,
    pub expires_at: DateTime<Utc>,
}

#[async_trait]
pub trait PaymentAuthorizer: Send + Sync {
    async fn enroll(&self, wallet: WalletCredentialId) -> Result<EnrollmentChallenge, AdapterError>;
    /// Verify a code for one attempt; binds to `amount_bound_hash`.
    async fn verify_for_attempt(
        &self,
        wallet: WalletCredentialId,
        attempt: PaymentAttemptId,
        amount_bound_hash: &str,
        code: &str,
    ) -> Result<AuthorizationId, AdapterError>;
    /// Consume a verified authorization exactly once.
    async fn consume(&self, authorization: AuthorizationId) -> Result<(), AdapterError>;
}

/// Signs only operations that were already approved and bounded upstream.
#[async_trait]
pub trait SigningProvider: Send + Sync {
    fn address(&self) -> Address;
    async fn sign_approved_operation(&self, plan: &DepositPlan, payload_hash: B256) -> Result<Vec<u8>, AdapterError>;
}
