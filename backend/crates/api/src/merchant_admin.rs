//! Merchant self-service (M12): invite-gated signup, API key and webhook
//! secret rotation, and payout readback. New credentials are returned exactly
//! once and only their hashes (key) or the in-memory value (secret) are kept.

use crate::{fail, parse_payout, require_merchant, secret_matches, ApiResult, AppState, ErrorBody, PayoutRequest};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use pcc_payments::asset::TokenRef;
use pcc_payments::domain::MerchantId;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub(crate) const SIGNUP_HEADER: &str = "x-signup-token";
const MAX_NAME_LEN: usize = 80;

#[derive(Deserialize, ToSchema)]
pub struct SignupRequest {
    pub name: String,
    pub payout: PayoutRequest,
}

#[derive(Serialize, ToSchema)]
pub struct SignupResponse {
    pub merchant_id: String,
    pub name: String,
    /// Shown once. Store it in your server's secret manager.
    pub api_key: String,
    /// Shown once. Verifies `X-PCC-Signature` on webhooks.
    pub webhook_secret: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeyResponse {
    /// Shown once; the previous key no longer authenticates.
    pub api_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct WebhookSecretResponse {
    /// Shown once; deliveries signed after rotation use this secret.
    pub webhook_secret: String,
}

#[derive(Serialize, ToSchema)]
pub struct PayoutView {
    pub receiving_address: String,
    pub chain_id: u64,
    pub token_address: Option<String>,
    pub decimals: u8,
    pub one_click_asset_id: String,
}

fn random_secret(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

#[utoipa::path(post, path = "/v1/merchants", tag = "merchant",
    request_body = SignupRequest,
    params(("X-Signup-Token" = String, Header, description = "Operator-issued invite token")),
    responses((status = 201, body = SignupResponse), (status = 400, body = ErrorBody),
              (status = 401, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub async fn signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SignupRequest>,
) -> ApiResult<(StatusCode, Json<SignupResponse>)> {
    let presented = headers
        .get(SIGNUP_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    // Check the invite without spending it. A bad name or payout must leave the
    // code usable, and a duplicate request must not turn that into "invalid token".
    let one_time = !presented.is_empty() && state.store.invite_pending(presented);
    let master = state
        .config
        .signup_token_hash
        .as_ref()
        .is_some_and(|expected| !presented.is_empty() && secret_matches(expected, presented));
    if !one_time && !master {
        if state.config.signup_token_hash.is_none() {
            return Err(fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "signup_disabled",
                "MERCHANT_SIGNUP_TOKEN is not set",
            ));
        }
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid signup token required",
        ));
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control) {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "name must be 1-80 printable characters",
        ));
    }
    let (asset, receiving) = parse_payout(body.payout)?;
    if one_time && !state.store.consume_invite(presented) {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid signup token required",
        ));
    }
    let id = MerchantId::new();
    let payout = pcc_merchants::new_payout_config(id, asset, receiving)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "invalid_payout", &e.to_string()))?;
    let api_key = random_secret("pcc_sk_");
    let webhook_secret = random_secret("whsec_");
    state.store.register_merchant(name, &api_key, payout, &webhook_secret);
    tracing::info!(merchant_id = %id, "merchant signed up");
    Ok((
        StatusCode::CREATED,
        Json(SignupResponse {
            merchant_id: id.to_string(),
            name: name.to_owned(),
            api_key,
            webhook_secret,
        }),
    ))
}

#[utoipa::path(post, path = "/v1/api-keys/rotate", tag = "merchant",
    responses((status = 200, body = ApiKeyResponse), (status = 401, body = ErrorBody)))]
pub async fn rotate_api_key(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<ApiKeyResponse>> {
    let actor = require_merchant(&state, &headers)?;
    let api_key = random_secret("pcc_sk_");
    if !state.store.rotate_api_key(actor, &api_key) {
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "invalid API key"));
    }
    tracing::info!(merchant_id = %actor, "api key rotated");
    Ok(Json(ApiKeyResponse { api_key }))
}

#[utoipa::path(post, path = "/v1/webhook-secret/rotate", tag = "merchant",
    responses((status = 200, body = WebhookSecretResponse), (status = 401, body = ErrorBody)))]
pub async fn rotate_webhook_secret(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<WebhookSecretResponse>> {
    let actor = require_merchant(&state, &headers)?;
    let webhook_secret = random_secret("whsec_");
    if !state.store.rotate_webhook_secret(actor, &webhook_secret) {
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "invalid API key"));
    }
    tracing::info!(merchant_id = %actor, "webhook secret rotated");
    Ok(Json(WebhookSecretResponse { webhook_secret }))
}

#[utoipa::path(get, path = "/v1/payout-config", tag = "merchant",
    responses((status = 200, body = PayoutView), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub async fn get_payout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<PayoutView>> {
    let actor = require_merchant(&state, &headers)?;
    let p = state
        .store
        .payout(actor)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "no payout configured"))?;
    Ok(Json(PayoutView {
        receiving_address: p.receiving_address.to_string(),
        chain_id: p.asset.chain_id.0,
        token_address: match p.asset.token {
            TokenRef::Erc20 { address } => Some(address.to_string()),
            TokenRef::Native => None,
        },
        decimals: p.asset.decimals,
        one_click_asset_id: p.asset.one_click_asset_id,
    }))
}
