//! Autopay mandates: merchant-initiated charges with no payer present.
//!
//! The payer sets one up inside a hosted checkout: a TOTP code plus one
//! EIP-712 signature for a dedicated session key scoped to this mandate
//! (per-charge cap, number of charges, expiry). After that the merchant charges
//! with its API key and no TOTP. Each charge still passes the minimum interval,
//! the worker's allowance check, quote validation and the execution gates.

use crate::grant::key_of;
use crate::store::{ChargeRefusal, Mandate, MandateStatus};
use crate::{checkout, fail, idempotency_key, require_merchant, ApiResult, AppState, ErrorBody};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::TokenRef;
use pcc_payments::domain::PaymentAttempt;
use pcc_payments::traits::{AdapterError, AllowanceKey, CheckoutPorts, GrantRequest, PaymentAuthorizer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

pub const DEFAULT_CHARGES: u32 = 12;
pub const MAX_CHARGES: u32 = 20;
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;
pub const MAX_INTERVAL_HOURS: u32 = 31 * 24;
pub const DEFAULT_VALID_DAYS: u32 = 30;
pub const MAX_VALID_DAYS: u32 = 30;

/// Optional fields default to this checkout's max debit, 12 charges, 24 h apart, 30 days.
#[derive(Deserialize, ToSchema)]
pub struct MandateSetupRequest {
    /// TOTP code; setting up autopay is the payer-present step.
    pub code: String,
    /// Per-charge cap in source-token base units.
    pub max_amount: Option<String>,
    /// Total charges allowed (1..=20).
    pub charges: Option<u32>,
    /// Minimum hours between charges (0..=744).
    pub interval_hours: Option<u32>,
    /// Days until the mandate expires (1..=30).
    pub valid_days: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct MandateSetupView {
    pub mandate_id: String,
    pub grant_id: String,
    /// Pass unchanged to `eth_signTypedData_v4`.
    #[schema(value_type = Object)]
    pub typed_data: serde_json::Value,
    pub chain_id: u64,
    pub token: String,
    pub max_amount: String,
    pub charges: u32,
    pub interval_secs: u64,
    /// Worst case: `max_amount * charges`.
    pub total_exposure: String,
    pub valid_until: String,
    /// The payer must sign before this.
    pub expires_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct MandateEnableRequest {
    pub grant_id: String,
    pub signature: String,
}

#[derive(Serialize, ToSchema)]
pub struct MandateView {
    pub mandate_id: String,
    pub merchant_id: String,
    pub owner: String,
    pub chain_id: u64,
    pub token: String,
    pub source_asset: String,
    pub max_amount: String,
    pub charges: u32,
    pub charges_remaining: u32,
    pub interval_secs: u64,
    pub valid_until: String,
    pub last_charge_at: Option<String>,
    /// `awaiting_signature`, `active`, `expired`, `exhausted` or `revoked`.
    pub status: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ChargeRequest {
    pub merchant_order_id: String,
    /// Payout-asset base units the merchant receives.
    pub output_amount: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChargeView {
    pub mandate_id: String,
    pub intent_id: String,
    pub attempt_id: String,
    pub quoted_input: String,
    pub deposit_address: Option<String>,
    /// Same values as checkout authorize: `gated`, `awaiting_approval`, `broadcast`, ...
    pub execution: String,
    pub simulation_ok: Option<bool>,
    pub charges_remaining: u32,
}

fn view(m: &Mandate, now: DateTime<Utc>) -> MandateView {
    let status = match &m.status {
        MandateStatus::AwaitingSignature { .. } => "awaiting_signature",
        MandateStatus::Revoked => "revoked",
        MandateStatus::Active if now >= m.valid_until => "expired",
        MandateStatus::Active if m.charges_made >= m.charges => "exhausted",
        MandateStatus::Active => "active",
    };
    let token = match m.asset.token {
        TokenRef::Erc20 { address } => address.to_string(),
        TokenRef::Native => String::new(),
    };
    MandateView {
        mandate_id: m.id.to_string(),
        merchant_id: m.merchant_id.to_string(),
        owner: m.owner.to_string(),
        chain_id: m.asset.chain_id.0,
        token,
        source_asset: m.asset.one_click_asset_id.clone(),
        max_amount: m.max_amount.to_string(),
        charges: m.charges,
        charges_remaining: m.charges.saturating_sub(m.charges_made),
        interval_secs: m.interval_secs,
        valid_until: m.valid_until.to_rfc3339(),
        last_charge_at: m.last_charge_at.map(|t| t.to_rfc3339()),
        status: status.into(),
    }
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

fn adapter_fail(e: &AdapterError) -> (StatusCode, Json<ErrorBody>) {
    match e {
        AdapterError::Rejected(m) => fail(StatusCode::UNPROCESSABLE_ENTITY, "grant_rejected", m),
        other => fail(StatusCode::BAD_GATEWAY, "grant_unavailable", &other.to_string()),
    }
}

fn mandate_key(m: &Mandate) -> ApiResult<AllowanceKey> {
    match m.asset.token {
        TokenRef::Erc20 { address } => Ok(AllowanceKey {
            owner: m.owner,
            chain_id: m.asset.chain_id,
            token: address,
            mandate: Some(m.id),
        }),
        TokenRef::Native => Err(fail(
            StatusCode::BAD_REQUEST,
            "unsupported_asset",
            "native sources are not supported",
        )),
    }
}

/// The signed-in payer's attempt in this checkout, with its merchant.
fn payer(state: &AppState, session_id: &str) -> ApiResult<(PaymentAttempt, pcc_payments::domain::MerchantId)> {
    let (session, _, _) = state
        .store
        .get_session(session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let attempt = state
        .store
        .attempt_for_session(session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    Ok((attempt, session.merchant_id))
}

fn owned_mandate(state: &AppState, session_id: &str, mandate_id: &str) -> ApiResult<Mandate> {
    let (attempt, _) = payer(state, session_id)?;
    mandate_id
        .parse::<Uuid>()
        .ok()
        .and_then(|id| state.store.mandate(id))
        .filter(|m| m.owner == attempt.source_wallet)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "no such mandate for this wallet"))
}

struct SetupParams {
    max_amount: BaseUnits,
    charges: u32,
    interval_secs: u64,
    valid_days: u32,
    total_exposure: String,
}

impl SetupParams {
    fn parse(body: &MandateSetupRequest, default_cap: BaseUnits) -> ApiResult<Self> {
        let max_amount = match body.max_amount.as_deref() {
            Some(s) => s.parse::<BaseUnits>().map_err(|_| {
                fail(
                    StatusCode::BAD_REQUEST,
                    "invalid_amount",
                    "max_amount must be base units",
                )
            })?,
            None => default_cap,
        };
        if max_amount.is_zero() {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                "invalid_amount",
                "max_amount must be > 0; pick a priced source asset or pass it",
            ));
        }
        let charges = body.charges.unwrap_or(DEFAULT_CHARGES);
        let hours = body.interval_hours.unwrap_or(DEFAULT_INTERVAL_HOURS);
        let valid_days = body.valid_days.unwrap_or(DEFAULT_VALID_DAYS);
        if !(1..=MAX_CHARGES).contains(&charges)
            || hours > MAX_INTERVAL_HOURS
            || !(1..=MAX_VALID_DAYS).contains(&valid_days)
        {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                "invalid_mandate",
                "charges must be 1..=20, interval_hours 0..=744 and valid_days 1..=30",
            ));
        }
        let total_exposure = max_amount
            .as_u256()
            .checked_mul(alloy_primitives::U256::from(charges))
            .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "invalid_amount", "cap too large"))?;
        Ok(Self {
            max_amount,
            charges,
            interval_secs: u64::from(hours) * 3600,
            valid_days,
            total_exposure: total_exposure.to_string(),
        })
    }
}

fn setup_bound(id: Uuid, max_amount: BaseUnits, charges: u32, interval_secs: u64, valid_days: u32) -> String {
    let mut h = Sha256::new();
    h.update(format!("pcc.autopay.v1|{id}|{max_amount}|{charges}|{interval_secs}|{valid_days}").as_bytes());
    format!("{:x}", h.finalize())
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/autopay", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    request_body = MandateSetupRequest,
    responses((status = 200, body = MandateSetupView), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
              (status = 409, body = ErrorBody), (status = 422, body = ErrorBody)))]
pub async fn setup(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<MandateSetupRequest>,
) -> ApiResult<Json<MandateSetupView>> {
    let (attempt, merchant_id) = payer(&state, &session_id)?;
    let ports = ports(&state)?;
    let (wallet, _) = state
        .store
        .wallet_for_session(&session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    if !state.authorizer.is_enrolled(wallet) {
        return Err(fail(
            StatusCode::CONFLICT,
            "totp_enrollment_required",
            "enroll TOTP before setting up autopay",
        ));
    }
    let key = key_of(&attempt)?;
    let p = SetupParams::parse(&body, attempt.max_debit)?;
    let (max_amount, charges, interval_secs, days) = (p.max_amount, p.charges, p.interval_secs, p.valid_days);
    let id = Uuid::new_v4();

    let bound = setup_bound(id, max_amount, charges, interval_secs, days);
    let auth = state
        .authorizer
        .verify_for_attempt(wallet, attempt.id, &bound, &body.code)
        .await
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, "totp_invalid", &e.to_string()))?;
    state
        .authorizer
        .consume_bound(auth, &bound)
        .await
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, "totp_invalid", &e.to_string()))?;

    let validity_secs = u64::from(days) * 24 * 3600;
    let prepared = ports
        .prepare_grant(&GrantRequest {
            key: AllowanceKey {
                mandate: Some(id),
                ..key
            },
            max_amount,
            uses: charges,
            validity_secs,
        })
        .await
        .map_err(|e| adapter_fail(&e))?;
    let valid_until = Utc::now() + Duration::seconds(i64::try_from(validity_secs).unwrap_or(i64::MAX));
    state.store.put_mandate(Mandate {
        id,
        merchant_id,
        owner: attempt.source_wallet,
        asset: attempt.source_asset.clone(),
        max_amount,
        charges,
        charges_made: 0,
        interval_secs,
        valid_until,
        last_charge_at: None,
        status: MandateStatus::AwaitingSignature {
            grant_id: prepared.grant_id.clone(),
        },
    });
    Ok(Json(MandateSetupView {
        mandate_id: id.to_string(),
        grant_id: prepared.grant_id,
        typed_data: prepared.typed_data,
        chain_id: key.chain_id.0,
        token: key.token.to_string(),
        max_amount: max_amount.to_string(),
        charges,
        interval_secs,
        total_exposure: p.total_exposure,
        valid_until: valid_until.to_rfc3339(),
        expires_at: prepared.expires_at.to_rfc3339(),
    }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/autopay/{mandate_id}/enable", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id"),
           ("mandate_id" = String, Path, description = "Mandate id from setup")),
    request_body = MandateEnableRequest,
    responses((status = 200, body = MandateView), (status = 404, body = ErrorBody),
              (status = 409, body = ErrorBody), (status = 422, body = ErrorBody)))]
pub async fn enable(
    State(state): State<AppState>,
    Path((session_id, mandate_id)): Path<(String, String)>,
    Json(body): Json<MandateEnableRequest>,
) -> ApiResult<Json<MandateView>> {
    let mandate = owned_mandate(&state, &session_id, &mandate_id)?;
    let ports = ports(&state)?;
    if mandate.status
        != (MandateStatus::AwaitingSignature {
            grant_id: body.grant_id.clone(),
        })
    {
        return Err(fail(
            StatusCode::CONFLICT,
            "grant_mismatch",
            "mandate is not awaiting that grant",
        ));
    }
    let enabled = ports
        .enable_grant(&body.grant_id, &body.signature)
        .await
        .map_err(|e| adapter_fail(&e))?;
    let key = mandate_key(&mandate)?;
    if enabled.key != key || enabled.max_amount != mandate.max_amount || enabled.uses_remaining != mandate.charges {
        return Err(fail(
            StatusCode::CONFLICT,
            "grant_mismatch",
            "enabled allowance does not match the mandate",
        ));
    }
    let active = state
        .store
        .set_mandate_status(mandate.id, MandateStatus::Active)
        .unwrap_or(mandate);
    tracing::info!(mandate = %active.id, owner = %active.owner, merchant = %active.merchant_id, "autopay mandate active");
    Ok(Json(view(&active, Utc::now())))
}

#[utoipa::path(get, path = "/v1/checkout/{session_id}/autopay", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    responses((status = 200, body = [MandateView]), (status = 401, body = ErrorBody)))]
pub async fn list_for_payer(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Vec<MandateView>>> {
    let (attempt, _) = payer(&state, &session_id)?;
    let now = Utc::now();
    Ok(Json(
        state
            .store
            .mandates_where(|m| m.owner == attempt.source_wallet)
            .iter()
            .map(|m| view(m, now))
            .collect(),
    ))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/autopay/{mandate_id}/revoke", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id"),
           ("mandate_id" = String, Path, description = "Mandate id")),
    responses((status = 200, body = MandateView), (status = 404, body = ErrorBody)))]
pub async fn revoke(
    State(state): State<AppState>,
    Path((session_id, mandate_id)): Path<(String, String)>,
) -> ApiResult<Json<MandateView>> {
    let mandate = owned_mandate(&state, &session_id, &mandate_id)?;
    let ports = ports(&state)?;
    let revoked = state
        .store
        .set_mandate_status(mandate.id, MandateStatus::Revoked)
        .unwrap_or(mandate);
    ports
        .revoke_allowance(&mandate_key(&revoked)?)
        .await
        .map_err(|e| adapter_fail(&e))?;
    tracing::info!(mandate = %revoked.id, "payer revoked autopay mandate");
    Ok(Json(view(&revoked, Utc::now())))
}

#[utoipa::path(get, path = "/v1/autopay", tag = "merchant",
    responses((status = 200, body = [MandateView]), (status = 401, body = ErrorBody)))]
pub async fn list_for_merchant(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<MandateView>>> {
    let merchant = require_merchant(&state, &headers)?;
    let now = Utc::now();
    Ok(Json(
        state
            .store
            .mandates_where(|m| {
                m.merchant_id == merchant && !matches!(m.status, MandateStatus::AwaitingSignature { .. })
            })
            .iter()
            .map(|m| view(m, now))
            .collect(),
    ))
}

/// Prices the charge from the payer's wallet and checks it fits the mandate and its live allowance.
async fn price_charge(
    state: &AppState,
    ports: &dyn CheckoutPorts,
    mandate: &Mandate,
    intent: &pcc_payments::domain::PaymentIntent,
    session: &crate::store::CheckoutSession,
    now: DateTime<Utc>,
) -> ApiResult<PaymentAttempt> {
    let attempt = state
        .store
        .ensure_attempt(
            session,
            intent,
            mandate.owner,
            mandate.asset.clone(),
            checkout::buffered_max_debit(intent.output_amount_base_units),
        )
        .ok_or_else(|| {
            fail(
                StatusCode::CONFLICT,
                "attempt_conflict",
                "charge intent already has an attempt",
            )
        })?;
    checkout::refresh_preview(ports, &state.store, intent, &attempt).await;
    let attempt = state.store.get_attempt(attempt.id).unwrap_or(attempt);
    if attempt.max_debit.is_zero() {
        return Err(fail(
            StatusCode::BAD_GATEWAY,
            "preview_required",
            "no priced quote for the mandate's source asset right now",
        ));
    }
    if attempt.max_debit > mandate.max_amount {
        return Err(fail(
            StatusCode::CONFLICT,
            "over_mandate_cap",
            "this charge would debit more than the mandate's per-charge cap",
        ));
    }
    let key = mandate_key(mandate)?;
    let active = ports.allowance(&key).await.map_err(|e| adapter_fail(&e))?;
    let unix_now = u64::try_from(now.timestamp()).unwrap_or(0);
    if !active.is_some_and(|a| a.covers(&key, attempt.max_debit, unix_now)) {
        return Err(fail(
            StatusCode::CONFLICT,
            "mandate_allowance_missing",
            "the payer's signed allowance for this mandate is gone (revoked, used up, or the worker restarted)",
        ));
    }
    Ok(attempt)
}

fn refusal(r: &ChargeRefusal) -> (StatusCode, Json<ErrorBody>) {
    match r {
        ChargeRefusal::NotFound => fail(StatusCode::NOT_FOUND, "not_found", "no such mandate for this merchant"),
        ChargeRefusal::NotActive => fail(StatusCode::CONFLICT, "mandate_inactive", "mandate is not active"),
        ChargeRefusal::Expired => fail(StatusCode::CONFLICT, "mandate_expired", "mandate has expired"),
        ChargeRefusal::Exhausted => fail(StatusCode::CONFLICT, "mandate_exhausted", "no charges left"),
        ChargeRefusal::TooSoon { retry_after_secs } => fail(
            StatusCode::TOO_MANY_REQUESTS,
            "mandate_interval",
            &format!("next charge allowed in {retry_after_secs}s"),
        ),
    }
}

#[utoipa::path(post, path = "/v1/autopay/{mandate_id}/charges", tag = "merchant",
    params(("mandate_id" = String, Path, description = "Mandate id"),
           ("Idempotency-Key" = String, Header, description = "Replays return the first result")),
    request_body = ChargeRequest,
    responses((status = 200, body = ChargeView), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody),
              (status = 409, body = ErrorBody), (status = 429, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub async fn charge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(mandate_id): Path<String>,
    Json(body): Json<ChargeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let merchant = require_merchant(&state, &headers)?;
    let idem = idempotency_key(&headers)?;
    let ports = ports(&state)?;
    let id = mandate_id
        .parse::<Uuid>()
        .map_err(|_| refusal(&ChargeRefusal::NotFound))?;
    let output = BaseUnits::parse_base_units(&body.output_amount)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "invalid_amount", &e.to_string()))?;
    let now = Utc::now();
    let mandate = state
        .store
        .mandate(id)
        .filter(|m| m.merchant_id == merchant)
        .ok_or_else(|| refusal(&ChargeRefusal::NotFound))?;
    match view(&mandate, now).status.as_str() {
        "active" => {}
        "expired" => return Err(refusal(&ChargeRefusal::Expired)),
        "exhausted" => return Err(refusal(&ChargeRefusal::Exhausted)),
        _ => return Err(refusal(&ChargeRefusal::NotActive)),
    }
    let (intent, session) = state
        .store
        .create_intent(
            merchant,
            &body.merchant_order_id,
            output,
            &format!("autopay:{id}:{idem}"),
            None,
            now,
        )
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "intent_rejected", &e))?;
    if let Some(done) = state.store.charge_result(intent.id) {
        return Ok(Json(done));
    }

    let attempt = price_charge(&state, ports, &mandate, &intent, &session, now).await?;
    let (reserved, previous) = state.store.reserve_charge(id, merchant, now).map_err(|r| refusal(&r))?;
    state.store.mark_attempt_authorized(attempt.id);
    let settlement = match checkout::settle_authorized(
        ports,
        &state.store,
        &intent,
        &attempt,
        state.config.gates,
        state.config.approval_mode,
        Some(id),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            state.store.release_charge(id, previous);
            return Err(fail(StatusCode::BAD_GATEWAY, "quote_failed", &e.to_string()));
        }
    };
    if matches!(settlement.execution.as_str(), "simulation_failed" | "execute_rejected") {
        state.store.release_charge(id, previous);
    }
    if settlement.execution == "broadcast" {
        crate::spawn_deposit_hint(&state, attempt.id);
    }
    let payload = serde_json::json!({
        "type": "payment.authorized",
        "intent_id": intent.id.to_string(),
        "attempt_id": attempt.id.to_string(),
        "autopay_mandate_id": id.to_string(),
        "execution": settlement.execution,
        "quoted_input": settlement.quoted_input,
        "deposit_address": settlement.deposit_address,
    })
    .to_string();
    crate::enqueue_and_deliver(&state, &intent, "payment.authorized", &payload);
    let charges_remaining = state
        .store
        .mandate(id)
        .map_or(0, |m| m.charges.saturating_sub(m.charges_made));
    let result = serde_json::to_value(ChargeView {
        mandate_id: reserved.id.to_string(),
        intent_id: intent.id.to_string(),
        attempt_id: attempt.id.to_string(),
        quoted_input: settlement.quoted_input,
        deposit_address: settlement.deposit_address,
        execution: settlement.execution,
        simulation_ok: settlement.simulation_ok,
        charges_remaining,
    })
    .unwrap_or_default();
    state.store.put_charge_result(intent.id, result.clone());
    Ok(Json(result))
}
