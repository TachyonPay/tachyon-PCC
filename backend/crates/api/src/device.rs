//! Remembered devices: a returning payer skips the wallet sign-in popup.
//!
//! A token is issued only when the payer opts in while authorizing a payment
//! with TOTP. It stands in for SIWE only: paying still needs a TOTP code, and
//! a new allowance or mandate still needs a wallet signature.

use crate::{fail, sign_in, ApiResult, AppState, ErrorBody, SiweVerifyResponse};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct DeviceTokenRequest {
    pub device_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct ForgetResponse {
    /// False when the token was unknown or already expired.
    pub forgotten: bool,
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/device/resume", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    request_body = DeviceTokenRequest,
    responses((status = 200, body = SiweVerifyResponse), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub async fn resume(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<DeviceTokenRequest>,
) -> ApiResult<Json<SiweVerifyResponse>> {
    let (session, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let unknown = || {
        fail(
            StatusCode::UNAUTHORIZED,
            "device_unknown",
            "device not remembered or expired; sign in with your wallet",
        )
    };
    let (wallet, owner) = state.store.device(&body.device_token, Utc::now()).ok_or_else(unknown)?;
    // A device only ever stands in for SIWE next to TOTP; without it, drop the token.
    if !state.authorizer.is_enrolled(wallet) {
        state.store.forget_device(&body.device_token);
        return Err(unknown());
    }
    Ok(Json(sign_in(&state, &session, &intent, owner).await?))
}

#[utoipa::path(post, path = "/v1/devices/forget", tag = "checkout",
    request_body = DeviceTokenRequest,
    responses((status = 200, body = ForgetResponse)))]
pub async fn forget(State(state): State<AppState>, Json(body): Json<DeviceTokenRequest>) -> Json<ForgetResponse> {
    Json(ForgetResponse {
        forgotten: state.store.forget_device(&body.device_token),
    })
}
