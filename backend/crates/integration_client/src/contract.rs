//! Versioned internal API contract (v1). DTOs are camelCase for TypeScript
//! ergonomics and are converted to domain types in [`crate::client`].

use pcc_payments::amount::BaseUnits;
use pcc_payments::quote::{Confidentiality, DepositMode, SwapType};
use pcc_payments::status::OneClickStatus;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

pub const INTERNAL_API_VERSION: &str = "1.0.0";
pub const BASE_PATH: &str = "/internal/v1";

/// Stable machine-readable error codes returned by the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Missing/invalid internal bearer token.
    Unauthorized,
    /// Request failed DTO validation in the worker.
    InvalidRequest,
    /// `authorizationRef` was already used for an executable quote.
    AuthRefReused,
    UpstreamTimeout,
    UpstreamUnauthorized,
    UpstreamRateLimited,
    UpstreamRejected,
    UpstreamUnavailable,
    /// Upstream payload failed schema or signature verification.
    MalformedUpstream,
    SignatureInvalid,
    NoRoute,
    /// Session-key UserOp failed policy / quote-bind admission.
    SessionRejected,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub service: String,
    pub api_version: String,
    /// Installed `@defuse-protocol/one-click-sdk-typescript` version.
    pub one_click_sdk_version: String,
    /// Installed `@zerodev/sdk` version.
    pub zerodev_sdk_version: String,
}

/// Raw 1Click `/v0/tokens` entry (passed through; Rust does the chain mapping).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenEntry {
    pub asset_id: String,
    pub blockchain: String,
    pub symbol: String,
    pub decimals: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
    /// USD price as a decimal string (informational only; never used for amounts).
    pub price_usd: String,
    pub price_updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetsResponse {
    pub tokens: Vec<TokenEntry>,
    pub fetched_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequestDto {
    /// `true` = preview (no deposit address). `false` requires an unused `authorizationRef`.
    pub dry: bool,
    /// Single-use reference issued by Rust for this quote request.
    pub authorization_ref: String,
    pub origin_asset_id: String,
    pub destination_asset_id: String,
    /// EXACT_OUTPUT amount in destination base units.
    pub amount_out: BaseUnits,
    pub recipient: String,
    pub refund_to: String,
    pub confidentiality: Confidentiality,
    pub slippage_bps: u32,
    /// ISO-8601 deadline forwarded to 1Click.
    pub deadline: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedQuoteDto {
    pub correlation_id: String,
    pub signature: String,
    /// Result of `verifyQuoteSignature()` in the worker. Rust re-checks this flag
    /// and still applies its own business rules.
    pub signature_verified: bool,
    pub dry: bool,
    pub swap_type: SwapType,
    pub confidentiality: Confidentiality,
    pub deposit_mode: DepositMode,
    pub origin_asset_id: String,
    pub destination_asset_id: String,
    pub recipient: String,
    pub refund_to: String,
    pub amount_in: BaseUnits,
    pub min_amount_in: BaseUnits,
    pub amount_out: BaseUnits,
    pub min_amount_out: BaseUnits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_memo: Option<String>,
    pub quote_timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub time_estimate_seconds: u64,
    /// Exact JSON returned by 1Click, stored immutably for disputes.
    pub raw_response_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRequestDto {
    pub raw_response_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusQuery {
    pub deposit_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_memo: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SwapStatusDto {
    pub status: OneClickStatus,
    pub updated_at: String,
    #[serde(default)]
    pub origin_tx_hashes: Vec<String>,
    #[serde(default)]
    pub destination_tx_hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_in: Option<BaseUnits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_out: Option<BaseUnits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refunded_amount: Option<BaseUnits>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DepositSubmittedDto {
    pub deposit_address: String,
    pub tx_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AckResponse {
    pub ok: bool,
}

/// Deposit the session-key executor may perform. `to` and `quote_deposit_address`
/// must be the same 1Click deposit address from the verified quote; the worker
/// refuses a mismatch (quote-address substitution).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DepositPlanDto {
    pub attempt_id: String,
    pub chain_id: u64,
    /// ERC-20 contract on the origin chain. Native deposits are refused.
    pub token: String,
    pub from: String,
    pub to: String,
    /// Verified quote deposit address; must equal `to`.
    pub quote_deposit_address: String,
    pub amount: BaseUnits,
    pub max_fee_wei: BaseUnits,
    pub approval_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas_limit: Option<String>,
    /// Autopay mandate the allowance belongs to; absent = the payer's checkout allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SimulationDto {
    pub ok: bool,
    pub gas_estimate: u64,
    pub max_fee_wei: BaseUnits,
    pub sender_balance: BaseUnits,
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResponseDto {
    pub user_op_hash: String,
    pub tx_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptDto {
    pub tx_hash: String,
    pub block_number: u64,
    pub success: bool,
    pub gas_used: u64,
}

/// Ask the worker for a spending allowance the payer will sign once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantPrepareDto {
    pub owner: String,
    pub chain_id: u64,
    /// ERC-20 the session key may transfer.
    pub token: String,
    /// Inclusive cap for each transfer.
    pub max_amount: BaseUnits,
    /// Transfers allowed in total (1..=20); enforced on-chain by a rate-limit policy.
    pub uses: u32,
    /// Seconds until the allowance expires (600..=30 days).
    pub validity_sec: u64,
    /// Autopay mandate the allowance belongs to; absent = the payer's checkout allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantPreparedDto {
    pub grant_id: String,
    /// EIP-712 payload for `eth_signTypedData_v4`; integers are decimal strings.
    #[schema(value_type = Object)]
    pub typed_data: serde_json::Value,
    pub session_signer: String,
    pub uses: u32,
    pub valid_until: u64,
    pub expires_at: String,
}

/// One allowance is identified by payer, chain and token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceKeyDto {
    pub owner: String,
    pub chain_id: u64,
    pub token: String,
    /// Autopay mandate the allowance belongs to; absent = the payer's checkout allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokedDto {
    /// False when there was no active allowance to revoke.
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantEnableDto {
    pub grant_id: String,
    pub signature: String,
}

/// An active allowance as the worker tracks it. The on-chain policies are authoritative.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceDto {
    pub owner: String,
    pub chain_id: u64,
    pub token: String,
    /// Inclusive cap for each transfer.
    pub max_amount: BaseUnits,
    pub uses_remaining: u32,
    pub valid_until: u64,
    /// Autopay mandate the allowance belongs to; absent = the payer's checkout allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Path declarations. These functions exist only to carry `utoipa::path`
// metadata; the worker implements them in TypeScript.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
mod paths {
    use super::{
        AckResponse, AllowanceDto, AllowanceKeyDto, AssetsResponse, DepositPlanDto, DepositSubmittedDto, ErrorResponse,
        ExecuteResponseDto, GrantEnableDto, GrantPrepareDto, GrantPreparedDto, HealthResponse, QuoteRequestDto,
        ReceiptDto, RevokedDto, SimulationDto, SwapStatusDto, VerifiedQuoteDto, VerifyRequestDto,
    };

    #[utoipa::path(post, path = "/internal/v1/aa/grants/prepare", tag = "aa",
        request_body = GrantPrepareDto,
        responses(
            (status = 200, body = GrantPreparedDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 422, body = ErrorResponse, description = "owner not delegated to Kernel on that chain")))]
    pub fn aa_grant_prepare() {}

    #[utoipa::path(post, path = "/internal/v1/aa/grants/enable", tag = "aa",
        request_body = GrantEnableDto,
        responses(
            (status = 200, body = AllowanceDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 422, body = ErrorResponse, description = "signature does not recover to the owner, or grant expired")))]
    pub fn aa_grant_enable() {}

    #[utoipa::path(post, path = "/internal/v1/aa/grants/status", tag = "aa",
        request_body = AllowanceKeyDto,
        responses(
            (status = 200, body = AllowanceDto),
            (status = 401, body = ErrorResponse),
            (status = 404, body = ErrorResponse, description = "no active allowance (never enabled, used up, expired, revoked, or worker restarted)")))]
    pub fn aa_grant_status() {}

    #[utoipa::path(post, path = "/internal/v1/aa/grants/revoke", tag = "aa",
        request_body = AllowanceKeyDto,
        responses(
            (status = 200, body = RevokedDto),
            (status = 401, body = ErrorResponse)))]
    pub fn aa_grant_revoke() {}

    #[utoipa::path(get, path = "/internal/v1/health", tag = "meta",
        responses((status = 200, body = HealthResponse)))]
    pub fn health() {}

    #[utoipa::path(get, path = "/internal/v1/near/assets", tag = "near",
        responses(
            (status = 200, body = AssetsResponse),
            (status = 401, body = ErrorResponse),
            (status = 502, body = ErrorResponse),
            (status = 504, body = ErrorResponse)))]
    pub fn near_assets() {}

    #[utoipa::path(post, path = "/internal/v1/near/quote", tag = "near",
        request_body = QuoteRequestDto,
        responses(
            (status = 200, body = VerifiedQuoteDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 409, body = ErrorResponse, description = "authorizationRef reused"),
            (status = 422, body = ErrorResponse, description = "no route / signature invalid"),
            (status = 502, body = ErrorResponse),
            (status = 504, body = ErrorResponse)))]
    pub fn near_quote() {}

    #[utoipa::path(post, path = "/internal/v1/near/verify", tag = "near",
        request_body = VerifyRequestDto,
        responses(
            (status = 200, body = VerifiedQuoteDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 422, body = ErrorResponse)))]
    pub fn near_verify() {}

    #[utoipa::path(get, path = "/internal/v1/near/status", tag = "near",
        params(("depositAddress" = String, Query), ("depositMemo" = Option<String>, Query)),
        responses(
            (status = 200, body = SwapStatusDto),
            (status = 401, body = ErrorResponse),
            (status = 404, body = ErrorResponse),
            (status = 502, body = ErrorResponse),
            (status = 504, body = ErrorResponse)))]
    pub fn near_status() {}

    #[utoipa::path(post, path = "/internal/v1/near/deposit-submitted", tag = "near",
        request_body = DepositSubmittedDto,
        responses(
            (status = 200, body = AckResponse),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 502, body = ErrorResponse)))]
    pub fn near_deposit_submitted() {}

    #[utoipa::path(post, path = "/internal/v1/aa/simulate", tag = "aa",
        request_body = DepositPlanDto,
        responses(
            (status = 200, body = SimulationDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse)))]
    pub fn aa_simulate() {}

    #[utoipa::path(post, path = "/internal/v1/aa/execute", tag = "aa",
        request_body = DepositPlanDto,
        responses(
            (status = 200, body = ExecuteResponseDto),
            (status = 400, body = ErrorResponse),
            (status = 401, body = ErrorResponse),
            (status = 422, body = ErrorResponse, description = "session policy / quote-bind rejected")))]
    pub fn aa_execute() {}

    #[utoipa::path(get, path = "/internal/v1/aa/receipt", tag = "aa",
        params(("chainId" = u64, Query), ("txHash" = String, Query)),
        responses(
            (status = 200, body = ReceiptDto),
            (status = 401, body = ErrorResponse),
            (status = 404, body = ErrorResponse)))]
    pub fn aa_receipt() {}
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "PCC Internal Integration API",
        version = "1.0.0",
        description = "Private Rust -> TS worker boundary. Loopback only. Bearer token required. Not a public API."
    ),
    paths(
        paths::health,
        paths::near_assets,
        paths::near_quote,
        paths::near_verify,
        paths::near_status,
        paths::near_deposit_submitted,
        paths::aa_simulate,
        paths::aa_execute,
        paths::aa_receipt,
        paths::aa_grant_prepare,
        paths::aa_grant_enable,
        paths::aa_grant_status,
        paths::aa_grant_revoke
    ),
    components(schemas(
        ErrorCode, ErrorBody, ErrorResponse, HealthResponse, TokenEntry, AssetsResponse,
        QuoteRequestDto, VerifiedQuoteDto, VerifyRequestDto, StatusQuery, SwapStatusDto,
        DepositSubmittedDto, AckResponse, DepositPlanDto, SimulationDto, ExecuteResponseDto,
        ReceiptDto, GrantPrepareDto, GrantPreparedDto, GrantEnableDto, AllowanceDto, AllowanceKeyDto, RevokedDto,
        BaseUnits, Confidentiality, SwapType, DepositMode, OneClickStatus
    )),
    tags((name = "meta"), (name = "near"), (name = "aa"))
)]
pub struct InternalApiDoc;

impl InternalApiDoc {
    /// Pretty JSON of the contract, deterministic for diffing in CI.
    pub fn to_pretty_json() -> String {
        let mut doc = Self::openapi();
        INTERNAL_API_VERSION.clone_into(&mut doc.info.version);
        serde_json::to_string_pretty(&doc).expect("openapi serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed contract must match the Rust types exactly. Regenerate with
    /// `cargo run -p pcc-integration-client --bin export-internal-openapi`.
    #[test]
    fn committed_openapi_matches_rust_types() {
        let committed = include_str!("../openapi/internal-v1.json");
        let generated = InternalApiDoc::to_pretty_json();
        assert_eq!(
            committed.trim(),
            generated.trim(),
            "openapi/internal-v1.json is stale; regenerate it and re-run TS type generation"
        );
    }

    #[test]
    fn dto_roundtrip_is_camel_case_strings() {
        let dto = QuoteRequestDto {
            dry: true,
            authorization_ref: "a".into(),
            origin_asset_id: "o".into(),
            destination_asset_id: "d".into(),
            amount_out: BaseUnits::from_u128(5),
            recipient: "0x1".into(),
            refund_to: "0x2".into(),
            confidentiality: Confidentiality::Basic,
            slippage_bps: 100,
            deadline: "2026-01-01T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["authorizationRef"], "a");
        assert_eq!(v["amountOut"], "5");
        assert_eq!(v["confidentiality"], "basic");
    }

    #[test]
    fn deposit_plan_dto_is_camel_case_strings() {
        let dto = DepositPlanDto {
            attempt_id: "11111111-1111-1111-1111-111111111111".into(),
            chain_id: 11_155_111,
            token: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".into(),
            from: "0x1".into(),
            to: "0x2".into(),
            quote_deposit_address: "0x2".into(),
            amount: BaseUnits::from_u128(1_020_000),
            max_fee_wei: BaseUnits::from_u128(1),
            approval_ref: "appr".into(),
            gas_limit: None,
            mandate_id: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["quoteDepositAddress"], "0x2");
        assert_eq!(v["amount"], "1020000");
        assert_eq!(v["chainId"], 11_155_111);
        assert!(v.get("gasLimit").is_none());
    }

    #[test]
    fn unknown_error_code_is_rejected() {
        let bad = r#"{"error":{"code":"SOMETHING_NEW","message":"x"}}"#;
        assert!(serde_json::from_str::<ErrorResponse>(bad).is_err());
    }
}
