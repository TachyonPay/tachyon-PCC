//! Per-payer spending allowances (M11).
//!
//! The payer's wallet signs the Kernel enable payload (EIP-712) once for a
//! fresh session key that may make up to `uses` transfers of one token, each
//! capped at `max_amount`, until it expires. Later checkouts with the same
//! wallet, chain and token need only a TOTP code. The worker holds the session
//! key and is the source of truth; the on-chain policies are the hard limit.

use crate::{fail, ApiResult, AppState, ErrorBody};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::TokenRef;
use pcc_payments::domain::PaymentAttempt;
use pcc_payments::traits::{AdapterError, Allowance, AllowanceKey, CheckoutPorts, GrantRequest};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const DEFAULT_USES: u32 = 5;
pub const MAX_USES: u32 = 20;
pub const DEFAULT_VALID_DAYS: u32 = 7;
pub const MAX_VALID_DAYS: u32 = 30;

/// Every field is optional; defaults are this payment's max debit, 5 uses, 7 days.
#[derive(Default, Deserialize, ToSchema)]
pub struct PrepareGrantRequest {
    /// Per-payment cap in token base units. May not be below this payment's max debit.
    pub max_amount: Option<String>,
    /// Payments the allowance covers (1..=20).
    pub uses: Option<u32>,
    /// Days until it expires (1..=30).
    pub valid_days: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct SessionGrantView {
    pub grant_id: String,
    /// Pass unchanged to `eth_signTypedData_v4`.
    #[schema(value_type = Object)]
    pub typed_data: serde_json::Value,
    pub session_signer: String,
    pub chain_id: u64,
    pub token: String,
    /// Per-payment cap in token base units.
    pub max_amount: String,
    pub uses: u32,
    /// Worst case the allowance can move: `max_amount * uses`.
    pub total_exposure: String,
    pub valid_until: u64,
    /// The payer must sign before this.
    pub expires_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct EnableGrantRequest {
    pub grant_id: String,
    pub signature: String,
}

#[derive(Serialize, ToSchema)]
pub struct AllowanceView {
    pub chain_id: u64,
    pub token: String,
    pub max_amount: String,
    pub uses_remaining: u32,
    pub valid_until: u64,
    /// Whether it can pay this checkout as currently priced.
    pub covers_this_payment: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RevokeResponse {
    /// False when there was no active allowance.
    pub revoked: bool,
}

fn now_unix() -> u64 {
    u64::try_from(Utc::now().timestamp()).unwrap_or(0)
}

fn ports(state: &AppState) -> ApiResult<&dyn CheckoutPorts> {
    state.integrations.as_deref().ok_or_else(|| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "unconfigured",
            "integrations not configured",
        )
    })
}

/// The allowance that could pay `attempt`: its wallet, chain and source token.
pub fn key_of(attempt: &PaymentAttempt) -> ApiResult<AllowanceKey> {
    match attempt.source_asset.token {
        TokenRef::Erc20 { address } => Ok(AllowanceKey {
            owner: attempt.source_wallet,
            chain_id: attempt.source_asset.chain_id,
            token: address,
            mandate: None,
        }),
        TokenRef::Native => Err(fail(
            StatusCode::BAD_REQUEST,
            "unsupported_asset",
            "native sources are not supported",
        )),
    }
}

fn signed_in_attempt(state: &AppState, session_id: &str) -> ApiResult<PaymentAttempt> {
    state
        .store
        .get_session(session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    state
        .store
        .attempt_for_session(session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))
}

fn priced_attempt(state: &AppState, session_id: &str) -> ApiResult<PaymentAttempt> {
    let attempt = signed_in_attempt(state, session_id)?;
    if attempt.status != pcc_payments::status::PaymentAttemptStatus::Quoted {
        return Err(fail(
            StatusCode::CONFLICT,
            "attempt_locked",
            "attempt is already authorized",
        ));
    }
    if attempt.max_debit.is_zero() {
        return Err(fail(
            StatusCode::CONFLICT,
            "preview_required",
            "no priced quote for the selected source asset; reselect it to retry",
        ));
    }
    Ok(attempt)
}

fn adapter_fail(e: &AdapterError) -> (StatusCode, Json<ErrorBody>) {
    match e {
        AdapterError::Rejected(m) => fail(StatusCode::UNPROCESSABLE_ENTITY, "grant_rejected", m),
        other => fail(StatusCode::BAD_GATEWAY, "grant_unavailable", &other.to_string()),
    }
}

/// True when the payer's active allowance can pay `attempt` as priced right now.
pub async fn attempt_covered(ports: &dyn CheckoutPorts, attempt: &PaymentAttempt) -> ApiResult<bool> {
    let key = key_of(attempt)?;
    let active = ports.allowance(&key).await.map_err(|e| adapter_fail(&e))?;
    Ok(active.is_some_and(|a| a.covers(&key, attempt.max_debit, now_unix())))
}

fn view(a: &Allowance, attempt: &PaymentAttempt) -> AllowanceView {
    AllowanceView {
        chain_id: a.key.chain_id.0,
        token: a.key.token.to_string(),
        max_amount: a.max_amount.to_string(),
        uses_remaining: a.uses_remaining,
        valid_until: a.valid_until,
        covers_this_payment: key_of(attempt).is_ok_and(|k| a.covers(&k, attempt.max_debit, now_unix())),
    }
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/session-grant", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    request_body = PrepareGrantRequest,
    responses((status = 200, body = SessionGrantView), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
              (status = 409, body = ErrorBody), (status = 422, body = ErrorBody)))]
pub async fn prepare_grant(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PrepareGrantRequest>,
) -> ApiResult<Json<SessionGrantView>> {
    let attempt = priced_attempt(&state, &session_id)?;
    let ports = ports(&state)?;
    let key = key_of(&attempt)?;
    let max_amount = match body.max_amount.as_deref() {
        None => attempt.max_debit,
        Some(s) => s.parse::<BaseUnits>().map_err(|_| {
            fail(
                StatusCode::BAD_REQUEST,
                "invalid_amount",
                "max_amount must be base units",
            )
        })?,
    };
    if max_amount < attempt.max_debit {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "cap_below_payment",
            "per-payment cap is below this payment's max debit",
        ));
    }
    let uses = body.uses.unwrap_or(DEFAULT_USES);
    let days = body.valid_days.unwrap_or(DEFAULT_VALID_DAYS);
    if !(1..=MAX_USES).contains(&uses) || !(1..=MAX_VALID_DAYS).contains(&days) {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "invalid_allowance",
            "uses must be 1..=20 and valid_days 1..=30",
        ));
    }
    let total_exposure = max_amount
        .as_u256()
        .checked_mul(alloy_primitives::U256::from(uses))
        .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "invalid_amount", "cap too large"))?;
    let prepared = ports
        .prepare_grant(&GrantRequest {
            key,
            max_amount,
            uses,
            validity_secs: u64::from(days) * 24 * 3600,
        })
        .await
        .map_err(|e| adapter_fail(&e))?;
    state.store.set_pending_grant(attempt.id, prepared.grant_id.clone());
    Ok(Json(SessionGrantView {
        grant_id: prepared.grant_id,
        typed_data: prepared.typed_data,
        session_signer: prepared.session_signer.to_string(),
        chain_id: key.chain_id.0,
        token: key.token.to_string(),
        max_amount: max_amount.to_string(),
        uses: prepared.uses,
        total_exposure: total_exposure.to_string(),
        valid_until: prepared.valid_until,
        expires_at: prepared.expires_at.to_rfc3339(),
    }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/session-grant/enable", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    request_body = EnableGrantRequest,
    responses((status = 200, body = AllowanceView), (status = 401, body = ErrorBody),
              (status = 409, body = ErrorBody), (status = 422, body = ErrorBody)))]
pub async fn enable_grant(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<EnableGrantRequest>,
) -> ApiResult<Json<AllowanceView>> {
    let attempt = priced_attempt(&state, &session_id)?;
    let ports = ports(&state)?;
    if !state.store.is_pending_grant(attempt.id, &body.grant_id) {
        return Err(fail(
            StatusCode::CONFLICT,
            "grant_mismatch",
            "no pending allowance with that id for this checkout",
        ));
    }
    let enabled = ports
        .enable_grant(&body.grant_id, &body.signature)
        .await
        .map_err(|e| adapter_fail(&e))?;
    state.store.clear_pending_grant(attempt.id);
    let key = key_of(&attempt)?;
    if !enabled.covers(&key, attempt.max_debit, now_unix()) {
        return Err(fail(
            StatusCode::CONFLICT,
            "grant_mismatch",
            "enabled allowance does not match this checkout's wallet, token or amount",
        ));
    }
    Ok(Json(view(&enabled, &attempt)))
}

#[utoipa::path(get, path = "/v1/checkout/{session_id}/session-grant", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    responses((status = 200, body = AllowanceView), (status = 401, body = ErrorBody),
              (status = 404, body = ErrorBody, description = "no active allowance for this wallet and source asset")))]
pub async fn allowance_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<AllowanceView>> {
    let attempt = signed_in_attempt(&state, &session_id)?;
    let ports = ports(&state)?;
    let key = key_of(&attempt)?;
    let active = ports.allowance(&key).await.map_err(|e| adapter_fail(&e))?;
    active.map(|a| Json(view(&a, &attempt))).ok_or_else(|| {
        fail(
            StatusCode::NOT_FOUND,
            "no_allowance",
            "no active allowance for this wallet and source asset",
        )
    })
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/session-grant/revoke", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    responses((status = 200, body = RevokeResponse), (status = 401, body = ErrorBody)))]
pub async fn revoke_allowance(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<RevokeResponse>> {
    let attempt = signed_in_attempt(&state, &session_id)?;
    let ports = ports(&state)?;
    let key = key_of(&attempt)?;
    let revoked = ports.revoke_allowance(&key).await.map_err(|e| adapter_fail(&e))?;
    if revoked {
        tracing::info!(owner = %key.owner, chain = key.chain_id.0, token = %key.token, "payer revoked allowance");
    }
    Ok(Json(RevokeResponse { revoked }))
}
