//! Operator routes: merchant invites, plus an optional parked-deposit echo.
//!
//! Default is payer mode: once gates are open, the payer's TOTP (or an autopay
//! mandate) is enough to send. `CHECKOUT_APPROVAL_MODE=owner` parks the deposit
//! until an operator echoes chain, deposit address, amount and max fee.

use crate::{enqueue_and_deliver, fail, secret_matches, snake_status, spawn_deposit_hint, ApiResult, AppState};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use chrono::Utc;
use pcc_payments::amount::BaseUnits;
use pcc_payments::domain::PaymentAttemptId;
use pcc_payments::traits::DepositPlan;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    /// An operator approves each live deposit.
    Owner,
    /// The payer's TOTP authorization is enough once gates are open (default).
    Payer,
}

impl ApprovalMode {
    /// Only the exact value `"owner"` parks deposits. Anything else, including
    /// unset, is payer-approved.
    #[must_use]
    pub fn from_env_value(v: Option<&str>) -> Self {
        if v.map(str::trim) == Some("owner") {
            Self::Owner
        } else {
            Self::Payer
        }
    }
}

#[derive(Serialize)]
pub struct PendingApprovalView {
    pub attempt_id: String,
    pub intent_id: String,
    pub chain_id: u64,
    pub token: String,
    pub from: String,
    pub deposit_address: String,
    pub amount: String,
    pub max_fee_wei: String,
    pub expires_at: String,
}

#[derive(Deserialize)]
pub struct ApproveRequest {
    pub chain_id: u64,
    pub deposit_address: String,
    pub amount: String,
    pub max_fee_wei: String,
}

#[derive(Serialize)]
pub struct ApproveResponse {
    pub attempt_id: String,
    pub execution: String,
    pub source_tx_hash: Option<String>,
    pub attempt_status: String,
}

#[derive(Serialize)]
pub struct InviteResponse {
    /// Shown once. A merchant can sign up with it a single time.
    pub invite_token: String,
}

pub async fn issue_invite(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<InviteResponse>> {
    require_operator(&state, &headers)?;
    Ok(Json(InviteResponse {
        invite_token: state.store.issue_invite(),
    }))
}

fn require_operator(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let Some(expected) = state.config.operator_token_hash.as_ref() else {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "operator_unconfigured",
            "OPERATOR_APPROVAL_TOKEN is not set",
        ));
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if presented.is_empty() || !secret_matches(expected, presented) {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "operator token required",
        ));
    }
    Ok(())
}

fn view(p: &crate::store::PendingApproval) -> PendingApprovalView {
    let token = match p.plan.asset.token {
        pcc_payments::asset::TokenRef::Erc20 { address } => address.to_string(),
        pcc_payments::asset::TokenRef::Native => "native".into(),
    };
    PendingApprovalView {
        attempt_id: p.plan.attempt_id.to_string(),
        intent_id: p.intent_id.to_string(),
        chain_id: p.plan.chain_id.0,
        token,
        from: p.plan.from.to_string(),
        deposit_address: p.plan.to.to_string(),
        amount: p.plan.amount.to_string(),
        max_fee_wei: p.plan.max_fee_wei.to_string(),
        expires_at: p.expires_at.to_rfc3339(),
    }
}

fn parse_attempt(id: &str) -> ApiResult<PaymentAttemptId> {
    id.parse::<Uuid>()
        .map(PaymentAttemptId)
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_attempt", "invalid attempt id"))
}

/// The echo must name exactly the parked transaction.
fn echo_matches(plan: &DepositPlan, req: &ApproveRequest) -> bool {
    let deposit_ok = req
        .deposit_address
        .parse::<alloy_primitives::Address>()
        .is_ok_and(|a| a == plan.to);
    let amount_ok = BaseUnits::parse_base_units(&req.amount).is_ok_and(|a| a == plan.amount);
    let fee_ok = BaseUnits::parse_base_units(&req.max_fee_wei).is_ok_and(|f| f == plan.max_fee_wei);
    req.chain_id == plan.chain_id.0 && deposit_ok && amount_ok && fee_ok
}

pub async fn list_pending(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<PendingApprovalView>>> {
    require_operator(&state, &headers)?;
    let now = Utc::now();
    Ok(Json(
        state
            .store
            .pending_approvals()
            .iter()
            .filter(|p| p.expires_at > now)
            .map(view)
            .collect(),
    ))
}

pub async fn reject(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(attempt_id): Path<String>,
) -> ApiResult<StatusCode> {
    require_operator(&state, &headers)?;
    let id = parse_attempt(&attempt_id)?;
    state.store.take_pending(id).ok_or_else(|| {
        fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "no pending approval for this attempt",
        )
    })?;
    tracing::info!(attempt_id = %id, "operator rejected parked deposit");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(attempt_id): Path<String>,
    Json(body): Json<ApproveRequest>,
) -> ApiResult<Json<ApproveResponse>> {
    require_operator(&state, &headers)?;
    let id = parse_attempt(&attempt_id)?;
    let pending = state.store.pending_approval(id).ok_or_else(|| {
        fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "no pending approval for this attempt",
        )
    })?;
    if pending.expires_at <= Utc::now() {
        state.store.take_pending(id);
        return Err(fail(
            StatusCode::GONE,
            "approval_expired",
            "approval window passed; payer must re-authorize",
        ));
    }
    if !echo_matches(&pending.plan, &body) {
        return Err(fail(
            StatusCode::CONFLICT,
            "approval_mismatch",
            "chain, deposit address, amount or fee cap differs from the parked deposit",
        ));
    }
    let Some(ports) = state.integrations.clone() else {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "unconfigured",
            "integrations not configured",
        ));
    };
    let mut plan = pending.plan.clone();
    plan.approval_ref = format!("operator:{id}");
    if let Err(e) = state
        .config
        .gates
        .check_broadcast(plan.chain_id, Some(&plan.approval_ref))
    {
        return Err(fail(StatusCode::CONFLICT, "gates_closed", &e.to_string()));
    }
    // Claim after every check so a refused request leaves the approval in place.
    let Some(_) = state.store.take_pending(id) else {
        return Err(fail(StatusCode::NOT_FOUND, "not_found", "approval already used"));
    };
    let hash = ports.execute_deposit(&plan).await.map_err(|e| {
        tracing::warn!(attempt_id = %id, error = %e, "approved deposit failed to execute");
        fail(StatusCode::BAD_GATEWAY, "execute_rejected", &e.to_string())
    })?;
    let tx = format!("{hash:#x}");
    state.store.mark_broadcast(id, &tx, &plan.approval_ref);
    spawn_deposit_hint(&state, id);
    let attempt = state.store.get_attempt(id);
    if let Some(intent) = state.store.intent_by_id(pending.intent_id) {
        let payload = serde_json::json!({
            "type": "payment.broadcast",
            "intent_id": intent.id.to_string(),
            "attempt_id": id.to_string(),
            "source_tx_hash": tx,
        })
        .to_string();
        enqueue_and_deliver(&state, &intent, "payment.broadcast", &payload);
    }
    tracing::info!(attempt_id = %id, "operator-approved deposit broadcast");
    Ok(Json(ApproveResponse {
        attempt_id: id.to_string(),
        execution: "broadcast".into(),
        source_tx_hash: Some(tx),
        attempt_status: attempt.map_or_else(|| "unknown".into(), |a| snake_status(a.status)),
    }))
}
