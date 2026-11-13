//! Axum application: health, gates, merchant API, hosted-checkout auth.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderName, Method, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use pcc_auth::{compose_siwe, verify_siwe_for_domain, SiweMessage, TotpAuthorizer};
use pcc_integration_client::{IntegrationClient, IntegrationClientConfig};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::{amount_bound_hash, MerchantId, PaymentAttemptId, PaymentIntent, PaymentIntentId};
use pcc_payments::traits::{CheckoutPorts, PaymentAuthorizer};
use pcc_payments::ExecutionGates;
use pcc_settlement::webhook::{deliver_signed, WebhookPolicy};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tower_http::cors::{Any, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

pub mod autopay;
mod checkout;
pub mod device;
pub mod grant;
pub mod merchant_admin;
pub mod onboarding;
pub mod operator;
pub mod settle;
mod store;
mod urls;

pub use store::Store;
pub use urls::assert_return_url;

/// Public API semantic version. Bump on breaking changes.
pub const PUBLIC_API_VERSION: &str = "0.1.0";

const IDEMPOTENCY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub gates: ExecutionGates,
    pub integrations_base_url: String,
    pub checkout_base_url: String,
    pub webhook_policy: WebhookPolicy,
    /// Who releases a live deposit once gates are open (M10).
    pub approval_mode: operator::ApprovalMode,
    /// SHA-256 of `OPERATOR_APPROVAL_TOKEN`; `None` disables operator routes.
    pub operator_token_hash: Option<[u8; 32]>,
    /// EIP-4361 `domain` the hosted checkout issues and accepts (M11).
    pub siwe_domain: String,
    /// SHA-256 of `MERCHANT_SIGNUP_TOKEN`; `None` disables self-serve signup (M12).
    pub signup_token_hash: Option<[u8; 32]>,
}

impl Config {
    /// Load from environment. Binds to loopback unless `API_BIND_ADDR` is set.
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = std::env::var("API_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
            .parse()?;
        let checkout_base_url =
            std::env::var("CHECKOUT_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned());
        let siwe_domain = match std::env::var("SIWE_DOMAIN") {
            Ok(d) if !d.trim().is_empty() => d.trim().to_owned(),
            _ => siwe_domain_of(&checkout_base_url)?,
        };
        Ok(Self {
            bind_addr,
            gates: ExecutionGates::from_env(),
            integrations_base_url: std::env::var("INTEGRATIONS_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned()),
            checkout_base_url,
            webhook_policy: WebhookPolicy {
                allow_loopback: std::env::var("ALLOW_WEBHOOK_LOOPBACK").as_deref() == Ok("true"),
            },
            approval_mode: operator::ApprovalMode::from_env_value(
                std::env::var("CHECKOUT_APPROVAL_MODE").ok().as_deref(),
            ),
            operator_token_hash: secret_hash_from_env("OPERATOR_APPROVAL_TOKEN"),
            siwe_domain,
            signup_token_hash: secret_hash_from_env("MERCHANT_SIGNUP_TOKEN"),
        })
    }
}

/// `host[:port]` of the checkout origin, the EIP-4361 domain browsers expect.
pub fn siwe_domain_of(checkout_base_url: &str) -> anyhow::Result<String> {
    let url = reqwest::Url::parse(checkout_base_url)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("CHECKOUT_BASE_URL has no host"))?;
    Ok(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// Shared bearer secrets must be long; shorter values leave the feature off.
const MIN_SECRET_LEN: usize = 24;

fn secret_hash_from_env(key: &str) -> Option<[u8; 32]> {
    let v = std::env::var(key).ok()?;
    let v = v.trim();
    (v.len() >= MIN_SECRET_LEN).then(|| secret_hash(v))
}

pub fn secret_hash(secret: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes()).into()
}

/// Compare a presented bearer secret against a stored hash without
/// short-circuiting on the first differing byte.
fn secret_matches(expected: &[u8; 32], presented: &str) -> bool {
    let got = secret_hash(presented);
    expected.iter().zip(got.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub authorizer: Arc<TotpAuthorizer>,
    pub integrations: Option<Arc<dyn CheckoutPorts>>,
    pub webhook_http: reqwest::Client,
    /// Account-code reads for the onboarding status page.
    pub code_reader: Option<Arc<dyn onboarding::CodeReader>>,
}

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct GatesView {
    pub dry_run: bool,
    pub enable_mainnet_transfers: bool,
    pub mock_authenticator_allowed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateIntentRequest {
    pub merchant_order_id: String,
    /// Decimal integer string of base units (never a float).
    pub output_amount: String,
    pub success_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PaymentIntentView {
    pub id: String,
    pub merchant_id: String,
    pub merchant_order_id: String,
    pub status: String,
    pub output_amount: String,
    pub expires_at: String,
    pub checkout_session_id: String,
    pub checkout_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct MerchantView {
    pub id: String,
    pub name: String,
    pub webhook_url: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PayoutRequest {
    pub receiving_address: String,
    pub chain_id: u64,
    pub token_address: Option<String>,
    pub decimals: u8,
    pub one_click_asset_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct WebhookUrlRequest {
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct CheckoutView {
    pub session_id: String,
    pub merchant_name: String,
    pub merchant_order_id: String,
    pub output_amount: String,
    pub destination_asset: String,
    pub source_asset: String,
    pub receiving_address: String,
    pub expires_at: String,
    pub attempt_id: Option<String>,
    pub quoted_input: Option<String>,
    pub max_debit: Option<String>,
    pub deposit_address: Option<String>,
    pub attempt_status: Option<String>,
    pub status: String,
    /// Where to send the payer's browser when they finish. Null when the merchant omitted it.
    pub success_url: Option<String>,
    pub intent_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct SiweStartRequest {
    pub address: String,
}

#[derive(Serialize, ToSchema)]
pub struct SiweStartResponse {
    pub nonce: String,
    pub message: String,
    pub expires_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct SiweVerifyRequest {
    pub message: String,
    pub signature: String,
}

#[derive(Serialize, ToSchema)]
pub struct SiweVerifyResponse {
    /// Signed-in payer wallet.
    pub address: String,
    pub wallet_credential_id: String,
    /// The wallet already confirmed TOTP in an earlier checkout; skip enrollment.
    pub totp_enrolled: bool,
    pub attempt_id: String,
    pub quoted_input: String,
    pub max_debit: String,
}

#[derive(Serialize, ToSchema)]
pub struct SourceAssetView {
    pub asset_id: String,
    pub symbol: String,
    pub chain_id: u64,
    pub decimals: u8,
    pub token_address: String,
}

#[derive(Serialize, ToSchema)]
pub struct SourceAssetsResponse {
    pub assets: Vec<SourceAssetView>,
    pub selected: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct SelectSourceAssetRequest {
    pub asset_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct SelectSourceAssetResponse {
    pub attempt_id: String,
    pub source_asset: String,
    /// Empty when the preview quote failed; authorize then refuses.
    pub quoted_input: String,
    pub max_debit: String,
}

#[derive(Serialize, ToSchema)]
pub struct EnrollResponse {
    pub wallet_credential_id: String,
    pub otpauth_uri: String,
    pub expires_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct TotpCodeRequest {
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
pub struct AuthorizeRequest {
    pub code: String,
    pub attempt_id: Option<String>,
    /// Issue a device token so later checkouts on this browser skip the wallet sign-in.
    #[serde(default)]
    pub remember_device: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AuthorizeResponse {
    /// Present only when `remember_device` was set. Shown once; store it on this device only.
    pub device_token: Option<String>,
    pub authorization_id: String,
    pub attempt_id: String,
    pub status: String,
    pub quoted_input: String,
    pub deposit_address: Option<String>,
    pub execution: String,
    pub simulation_ok: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct SignedEventView {
    pub r#type: String,
    pub payload: String,
    pub signature: String,
    pub status: String,
}

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorBody>)>;

fn fail(status: StatusCode, code: &str, error: &str) -> (StatusCode, Json<ErrorBody>) {
    (
        status,
        Json(ErrorBody {
            error: error.to_owned(),
            code: code.to_owned(),
        }),
    )
}

fn bearer_key(headers: &HeaderMap) -> ApiResult<String> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "unauthorized", "missing bearer token"))?;
    let key = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "unauthorized", "missing bearer token"))?;
    if key.is_empty() {
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "missing bearer token"));
    }
    Ok(key.to_owned())
}

fn require_merchant(state: &AppState, headers: &HeaderMap) -> ApiResult<MerchantId> {
    let key = bearer_key(headers)?;
    state
        .store
        .authenticate(&key)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "unauthorized", "invalid API key"))
}

fn idempotency_key(headers: &HeaderMap) -> ApiResult<String> {
    let value = headers
        .get(&IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            fail(
                StatusCode::BAD_REQUEST,
                "missing_idempotency_key",
                "Idempotency-Key required",
            )
        })?;
    if value.is_empty() || value.len() > 255 {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "missing_idempotency_key",
            "Idempotency-Key required",
        ));
    }
    Ok(value.to_owned())
}

fn snake_status<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

fn intent_view(config: &Config, intent: &PaymentIntent, session_id: &str) -> PaymentIntentView {
    PaymentIntentView {
        id: intent.id.to_string(),
        merchant_id: intent.merchant_id.to_string(),
        merchant_order_id: intent.merchant_order_id.clone(),
        status: snake_status(intent.status),
        output_amount: intent.output_amount_base_units.to_string(),
        expires_at: intent.expires_at.to_rfc3339(),
        checkout_session_id: session_id.to_owned(),
        checkout_url: format!(
            "{}/checkout/{session_id}",
            config.checkout_base_url.trim_end_matches('/')
        ),
    }
}

#[utoipa::path(get, path = "/health", tag = "meta",
    responses((status = 200, description = "Service is up", body = Health)))]
async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        service: "pcc-api",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[utoipa::path(get, path = "/v1/meta/gates", tag = "meta",
    responses((status = 200, description = "Current execution gates", body = GatesView)))]
async fn gates(State(state): State<AppState>) -> Json<GatesView> {
    let allow_flag = std::env::var("ALLOW_MOCK_AUTHENTICATOR").ok();
    Json(GatesView {
        dry_run: state.config.gates.dry_run,
        enable_mainnet_transfers: state.config.gates.enable_mainnet_transfers,
        mock_authenticator_allowed: pcc_auth::mock_authenticator_allowed(allow_flag.as_deref()).is_ok(),
    })
}

#[utoipa::path(get, path = "/v1/me", tag = "merchant",
    responses((status = 200, body = MerchantView), (status = 401, body = ErrorBody)))]
async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<MerchantView>> {
    let id = require_merchant(&state, &headers)?;
    let (merchant, name) = state
        .store
        .merchant(id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "unauthorized", "invalid API key"))?;
    Ok(Json(MerchantView {
        id: merchant.id.to_string(),
        name,
        webhook_url: merchant.webhook_url,
    }))
}

#[utoipa::path(put, path = "/v1/payout-config", tag = "merchant",
    request_body = PayoutRequest,
    responses((status = 200), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)))]
async fn put_payout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PayoutRequest>,
) -> ApiResult<StatusCode> {
    let actor = require_merchant(&state, &headers)?;
    let (asset, receiving) = parse_payout(body)?;
    state
        .store
        .upsert_payout(actor, asset, receiving)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "invalid_payout", &e.to_string()))?;
    Ok(StatusCode::OK)
}

fn parse_payout(body: PayoutRequest) -> ApiResult<(AssetId, alloy_primitives::Address)> {
    let receiving = body
        .receiving_address
        .parse()
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_address", "invalid receiving address"))?;
    let token = match body.token_address {
        None => TokenRef::Native,
        Some(addr) => TokenRef::Erc20 {
            address: addr
                .parse()
                .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_address", "invalid token address"))?,
        },
    };
    let asset = AssetId {
        chain_id: ChainId(body.chain_id),
        token,
        decimals: body.decimals,
        one_click_asset_id: body.one_click_asset_id,
    };
    Ok((asset, receiving))
}

#[utoipa::path(put, path = "/v1/webhook-url", tag = "merchant",
    request_body = WebhookUrlRequest,
    responses((status = 200), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)))]
async fn put_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WebhookUrlRequest>,
) -> ApiResult<StatusCode> {
    let actor = require_merchant(&state, &headers)?;
    state
        .store
        .set_webhook_url(actor, body.url, state.config.webhook_policy)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "ssrf_blocked", &e.to_string()))?;
    Ok(StatusCode::OK)
}

#[utoipa::path(post, path = "/v1/payment-intents", tag = "merchant",
    request_body = CreateIntentRequest,
    responses((status = 200, body = PaymentIntentView), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)))]
async fn create_intent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateIntentRequest>,
) -> ApiResult<Json<PaymentIntentView>> {
    let actor = require_merchant(&state, &headers)?;
    let idem = idempotency_key(&headers)?;
    if let Some(url) = body.success_url.as_deref() {
        urls::assert_return_url(url).map_err(|e| fail(StatusCode::BAD_REQUEST, "open_redirect", &e.to_string()))?;
    }
    let output = BaseUnits::parse_base_units(&body.output_amount)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "invalid_amount", &e.to_string()))?;
    let (intent, session) = state
        .store
        .create_intent(
            actor,
            &body.merchant_order_id,
            output,
            &idem,
            body.success_url,
            Utc::now(),
        )
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "intent_rejected", &e))?;
    let payload = serde_json::json!({
        "type": "payment.intent.created",
        "intent_id": intent.id.to_string(),
        "merchant_order_id": intent.merchant_order_id,
    })
    .to_string();
    state.store.enqueue_event(&intent, "payment.intent.created", &payload);
    Ok(Json(intent_view(&state.config, &intent, &session.id)))
}

#[utoipa::path(get, path = "/v1/payment-intents/{id}", tag = "merchant",
    params(("id" = String, Path, description = "Payment intent id")),
    responses((status = 200, body = PaymentIntentView), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_intent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentIntentView>> {
    let actor = require_merchant(&state, &headers)?;
    let intent = state
        .store
        .get_intent(actor, PaymentIntentId(id))
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "intent not found"))?;
    let session = state
        .store
        .session_for_intent(intent.id)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "intent not found"))?;
    Ok(Json(intent_view(&state.config, &intent, &session.id)))
}

#[utoipa::path(get, path = "/v1/payment-intents", tag = "merchant",
    responses((status = 200, body = Vec<PaymentIntentView>), (status = 401, body = ErrorBody)))]
async fn list_intents(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<PaymentIntentView>>> {
    let actor = require_merchant(&state, &headers)?;
    let mut out = Vec::new();
    for intent in state.store.list_intents(actor) {
        if let Some(session) = state.store.session_for_intent(intent.id) {
            out.push(intent_view(&state.config, &intent, &session.id));
        }
    }
    Ok(Json(out))
}

#[utoipa::path(get, path = "/v1/payment-intents/{id}/events", tag = "merchant",
    params(("id" = String, Path, description = "Payment intent id")),
    responses((status = 200, body = Vec<SignedEventView>), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn list_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<SignedEventView>>> {
    let actor = require_merchant(&state, &headers)?;
    let events = state
        .store
        .signed_events(actor, PaymentIntentId(id))
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "intent not found"))?;
    Ok(Json(
        events
            .into_iter()
            .map(|(r#type, payload, signature, status)| SignedEventView {
                r#type,
                payload,
                signature,
                status: snake_status(status),
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/v1/checkout/{session_id}", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    responses((status = 200, body = CheckoutView), (status = 404, body = ErrorBody)))]
async fn get_checkout(State(state): State<AppState>, Path(session_id): Path<String>) -> ApiResult<Json<CheckoutView>> {
    let (session, intent, name) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let attempt = state.store.attempt_for_session(&session.id);
    Ok(Json(checkout_view(&session, &intent, name, attempt.as_ref())))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/siwe/challenge", tag = "checkout",
    request_body = SiweStartRequest,
    responses((status = 200, body = SiweStartResponse), (status = 400, body = ErrorBody)))]
async fn siwe_challenge(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SiweStartRequest>,
) -> ApiResult<Json<SiweStartResponse>> {
    let (session, _, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let address = body
        .address
        .parse()
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_address", "invalid address"))?;
    let nonce = hex::encode(Uuid::new_v4().as_bytes());
    let now = Utc::now();
    let expires = now + Duration::minutes(10);
    let (domain, uri_base) = urls::signing_target(
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        &state.config.checkout_base_url,
        &state.config.siwe_domain,
    );
    let message = compose_siwe(&SiweMessage {
        domain,
        address,
        uri: format!("{uri_base}/checkout/{}", session.id),
        version: "1".into(),
        chain_id: 8453,
        nonce: nonce.clone(),
        issued_at: now,
        expiration_time: expires,
    });
    state.store.put_siwe(
        &session.id,
        store::SiweChallenge {
            nonce: nonce.clone(),
            address,
            expires_at: expires,
            message: message.clone(),
        },
    );
    Ok(Json(SiweStartResponse {
        nonce,
        message,
        expires_at: expires.to_rfc3339(),
    }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/siwe/verify", tag = "checkout",
    request_body = SiweVerifyRequest,
    responses((status = 200, body = SiweVerifyResponse), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn siwe_verify(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<SiweVerifyRequest>,
) -> ApiResult<Json<SiweVerifyResponse>> {
    let (session, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let challenge = state
        .store
        .siwe(&session.id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "no SIWE challenge"))?;
    if Utc::now() > challenge.expires_at {
        return Err(fail(StatusCode::UNAUTHORIZED, "siwe_expired", "SIWE challenge expired"));
    }
    // Only the exact text we issued is acceptable; this pins domain, URI and chain.
    if body.message != challenge.message {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "siwe_invalid",
            "message differs from the issued challenge",
        ));
    }
    let issued_domain = challenge.message.split(" wants you to sign in").next().unwrap_or("");
    let msg = verify_siwe_for_domain(
        &body.message,
        &body.signature,
        &challenge.nonce,
        Utc::now(),
        issued_domain,
    )
    .map_err(|e| fail(StatusCode::UNAUTHORIZED, "siwe_invalid", &e.to_string()))?;
    if msg.address != challenge.address {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "siwe_invalid",
            "signer does not match challenge",
        ));
    }
    let _ = state.store.take_siwe(&session.id);
    Ok(Json(sign_in(&state, &session, &intent, msg.address).await?))
}

/// Binds `owner` to this checkout and prices its attempt. Callers must have
/// proven control of `owner` (SIWE) or hold its remembered-device token.
async fn sign_in(
    state: &AppState,
    session: &store::CheckoutSession,
    intent: &PaymentIntent,
    owner: alloy_primitives::Address,
) -> ApiResult<SiweVerifyResponse> {
    let max_debit = checkout::buffered_max_debit(intent.output_amount_base_units);
    let attempt = state
        .store
        .ensure_attempt(session, intent, owner, AssetId::usdc_base(), max_debit)
        .ok_or_else(|| {
            fail(
                StatusCode::CONFLICT,
                "session_wallet_locked",
                "another wallet already authorized this checkout",
            )
        })?;
    let wallet = state.store.credential_for_owner(owner);
    state.authorizer.bind_owner(wallet, &owner.to_string());
    state.store.bind_wallet(&session.id, wallet, owner);
    if let Some(ports) = state.integrations.as_deref() {
        checkout::refresh_preview(ports, &state.store, intent, &attempt).await;
    }
    let attempt = state.store.attempt_for_session(&session.id).unwrap_or(attempt);
    Ok(SiweVerifyResponse {
        address: owner.to_string(),
        totp_enrolled: state.authorizer.is_enrolled(wallet),
        wallet_credential_id: wallet.to_string(),
        attempt_id: attempt.id.to_string(),
        quoted_input: attempt.quoted_input.to_string(),
        max_debit: attempt.max_debit.to_string(),
    })
}

/// 1Click cannot swap an asset into itself, so the payout asset is never a source.
async fn selectable_assets(
    state: &AppState,
    intent: &PaymentIntent,
) -> ApiResult<Vec<pcc_payments::traits::SupportedAsset>> {
    let payout = &intent.payout_config_snapshot.asset.one_click_asset_id;
    Ok(all_source_assets(state)
        .await?
        .into_iter()
        .filter(|a| &a.asset.one_click_asset_id != payout)
        .collect())
}

async fn all_source_assets(state: &AppState) -> ApiResult<Vec<pcc_payments::traits::SupportedAsset>> {
    let Some(ports) = state.integrations.as_deref() else {
        return Ok(vec![pcc_payments::traits::SupportedAsset {
            asset: AssetId::usdc_base(),
            symbol: "USDC".into(),
        }]);
    };
    let assets = ports
        .assets()
        .await
        .map_err(|e| fail(StatusCode::BAD_GATEWAY, "assets_unavailable", &e.to_string()))?;
    Ok(assets
        .into_iter()
        .filter(|a| checkout::selectable_source(&a.asset))
        .collect())
}

#[utoipa::path(get, path = "/v1/checkout/{session_id}/source-assets", tag = "checkout",
    responses((status = 200, body = SourceAssetsResponse), (status = 404, body = ErrorBody)))]
async fn list_source_assets(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<SourceAssetsResponse>> {
    let (_, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let mut assets = selectable_assets(&state, &intent).await?;
    let rank = |symbol: &str| match symbol {
        "USDC" => 0,
        "USDT" | "USDT0" => 1,
        "WETH" => 2,
        _ => 3,
    };
    assets.sort_by(|a, b| {
        (rank(&a.symbol), a.asset.chain_id, a.symbol.to_lowercase()).cmp(&(
            rank(&b.symbol),
            b.asset.chain_id,
            b.symbol.to_lowercase(),
        ))
    });
    let assets = assets
        .into_iter()
        .map(|a| SourceAssetView {
            token_address: match a.asset.token {
                TokenRef::Erc20 { address } => address.to_string(),
                TokenRef::Native => String::new(),
            },
            asset_id: a.asset.one_click_asset_id,
            symbol: a.symbol,
            chain_id: a.asset.chain_id.0,
            decimals: a.asset.decimals,
        })
        .collect();
    let selected = state
        .store
        .attempt_for_session(&session_id)
        .map(|a| a.source_asset.one_click_asset_id);
    Ok(Json(SourceAssetsResponse { assets, selected }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/source-asset", tag = "checkout",
    request_body = SelectSourceAssetRequest,
    responses((status = 200, body = SelectSourceAssetResponse), (status = 400, body = ErrorBody),
              (status = 401, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn select_source_asset(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<SelectSourceAssetRequest>,
) -> ApiResult<Json<SelectSourceAssetResponse>> {
    let (session, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let attempt = state
        .store
        .attempt_for_session(&session.id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    let asset = selectable_assets(&state, &intent)
        .await?
        .into_iter()
        .find(|a| a.asset.one_click_asset_id == body.asset_id)
        .map(|a| a.asset)
        .ok_or_else(|| {
            fail(
                StatusCode::BAD_REQUEST,
                "unsupported_asset",
                "asset is not a selectable source",
            )
        })?;
    let attempt = state.store.set_source_asset(attempt.id, asset).ok_or_else(|| {
        fail(
            StatusCode::CONFLICT,
            "attempt_locked",
            "source asset can only change before authorization",
        )
    })?;
    if let Some(ports) = state.integrations.as_deref() {
        checkout::refresh_preview(ports, &state.store, &intent, &attempt).await;
    }
    let attempt = state.store.get_attempt(attempt.id).unwrap_or(attempt);
    let priced = !attempt.max_debit.is_zero();
    Ok(Json(SelectSourceAssetResponse {
        attempt_id: attempt.id.to_string(),
        source_asset: attempt.source_asset.one_click_asset_id,
        quoted_input: if priced {
            attempt.quoted_input.to_string()
        } else {
            String::new()
        },
        max_debit: if priced {
            attempt.max_debit.to_string()
        } else {
            String::new()
        },
    }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/totp/enroll", tag = "checkout",
    responses((status = 200, body = EnrollResponse), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn totp_enroll(State(state): State<AppState>, Path(session_id): Path<String>) -> ApiResult<Json<EnrollResponse>> {
    let (wallet, _) = state
        .store
        .wallet_for_session(&session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    if state.authorizer.is_enrolled(wallet) {
        return Err(fail(
            StatusCode::CONFLICT,
            "already_enrolled",
            "this wallet already has TOTP; enter a code from your authenticator",
        ));
    }
    let challenge = state
        .authorizer
        .enroll(wallet)
        .await
        .map_err(|e| fail(StatusCode::BAD_REQUEST, "enroll_failed", &e.to_string()))?;
    Ok(Json(EnrollResponse {
        wallet_credential_id: wallet.to_string(),
        otpauth_uri: challenge.provisioning_secret_display,
        expires_at: challenge.expires_at.to_rfc3339(),
    }))
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/totp/confirm", tag = "checkout",
    request_body = TotpCodeRequest,
    responses((status = 200), (status = 401, body = ErrorBody)))]
async fn totp_confirm(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<TotpCodeRequest>,
) -> ApiResult<StatusCode> {
    let (wallet, owner) = state
        .store
        .wallet_for_session(&session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    state
        .authorizer
        .confirm_enrollment(wallet, &owner.to_string(), &body.code)
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, "totp_invalid", &e.to_string()))?;
    Ok(StatusCode::OK)
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/totp/reset", tag = "checkout",
    responses((status = 200), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn totp_reset(State(state): State<AppState>, Path(session_id): Path<String>) -> ApiResult<StatusCode> {
    let (wallet, _) = state
        .store
        .wallet_for_session(&session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    if !state.authorizer.reset_enrollment(wallet) {
        return Err(fail(
            StatusCode::CONFLICT,
            "not_enrolled",
            "this wallet has no authenticator to replace",
        ));
    }
    Ok(StatusCode::OK)
}

#[utoipa::path(post, path = "/v1/checkout/{session_id}/authorize", tag = "checkout",
    request_body = AuthorizeRequest,
    responses((status = 200, body = AuthorizeResponse), (status = 401, body = ErrorBody)))]
async fn authorize(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<AuthorizeRequest>,
) -> ApiResult<Json<AuthorizeResponse>> {
    let (session, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let (wallet, _) = state
        .store
        .wallet_for_session(&session.id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    let attempt = state
        .store
        .attempt_for_session(&session.id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    if let Some(requested) = body.attempt_id.as_deref() {
        let parsed = requested
            .parse::<Uuid>()
            .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_attempt", "invalid attempt id"))?;
        if PaymentAttemptId(parsed) != attempt.id {
            return Err(fail(
                StatusCode::FORBIDDEN,
                "attempt_mismatch",
                "attempt does not belong to this checkout",
            ));
        }
    }
    if attempt.max_debit.is_zero() {
        return Err(fail(
            StatusCode::CONFLICT,
            "preview_required",
            "no priced quote for the selected source asset; reselect it to retry",
        ));
    }
    if let Some(ports) = state.integrations.as_deref() {
        if !grant::attempt_covered(ports, &attempt).await? {
            return Err(fail(
                StatusCode::CONFLICT,
                "session_grant_required",
                "no active allowance covers this payment; sign one for this source asset",
            ));
        }
    }
    let bound = amount_bound_hash(&attempt, &intent);
    let auth_id = state
        .authorizer
        .verify_for_attempt(wallet, attempt.id, &bound, &body.code)
        .await
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, "totp_invalid", &e.to_string()))?;
    state
        .authorizer
        .consume_bound(auth_id, &bound)
        .await
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, "totp_invalid", &e.to_string()))?;
    state.store.mark_attempt_authorized(attempt.id);
    let settlement = match state.integrations.as_deref() {
        Some(ports) => checkout::settle_authorized(
            ports,
            &state.store,
            &intent,
            &attempt,
            state.config.gates,
            state.config.approval_mode,
            None,
        )
        .await
        .map_err(|e| fail(StatusCode::BAD_GATEWAY, "quote_failed", &e.to_string()))?,
        None => checkout::AuthorizeSettlement::unconfigured(&attempt),
    };
    if settlement.execution == "broadcast" {
        spawn_deposit_hint(&state, attempt.id);
    }
    let payload = serde_json::json!({
        "type": "payment.authorized",
        "intent_id": intent.id.to_string(),
        "attempt_id": attempt.id.to_string(),
        "execution": settlement.execution,
        "quoted_input": settlement.quoted_input,
        "deposit_address": settlement.deposit_address,
    })
    .to_string();
    enqueue_and_deliver(&state, &intent, "payment.authorized", &payload);
    let device_token = body
        .remember_device
        .then(|| state.store.issue_device(wallet, attempt.source_wallet, Utc::now()));
    Ok(Json(AuthorizeResponse {
        device_token,
        authorization_id: auth_id.to_string(),
        attempt_id: attempt.id.to_string(),
        status: "authorized".into(),
        quoted_input: settlement.quoted_input,
        deposit_address: settlement.deposit_address,
        execution: settlement.execution,
        simulation_ok: settlement.simulation_ok,
    }))
}

/// Rust-owned public OpenAPI document (merchant + checkout surface).
#[derive(OpenApi)]
#[openapi(
    info(title = "Private Crypto Checkout — public API", version = PUBLIC_API_VERSION,
         description = "Merchant and hosted-checkout API. Amounts are decimal integer strings."),
    paths(
        health, gates, me, put_payout, put_webhook, create_intent, get_intent, list_intents, list_events,
        merchant_admin::signup, merchant_admin::rotate_api_key, merchant_admin::rotate_webhook_secret,
        merchant_admin::get_payout,
        get_checkout, siwe_challenge, siwe_verify, list_source_assets, select_source_asset,
        onboarding::delegation_status, grant::prepare_grant, grant::enable_grant, grant::allowance_status,
        grant::revoke_allowance, totp_enroll, totp_reset, totp_confirm,
        authorize, device::resume, device::forget, autopay::setup, autopay::enable, autopay::list_for_payer, autopay::revoke,
        autopay::list_for_merchant, autopay::charge
    ),
    components(schemas(
        Health, GatesView, ErrorBody, CreateIntentRequest, PaymentIntentView, MerchantView, PayoutRequest,
        merchant_admin::SignupRequest, merchant_admin::SignupResponse, merchant_admin::ApiKeyResponse,
        merchant_admin::WebhookSecretResponse, merchant_admin::PayoutView,
        WebhookUrlRequest, CheckoutView, SiweStartRequest, SiweStartResponse, SiweVerifyRequest,
        SiweVerifyResponse, SourceAssetView, SourceAssetsResponse, SelectSourceAssetRequest,
        SelectSourceAssetResponse, onboarding::DelegationStatusResponse, onboarding::ChainDelegationView,
        grant::PrepareGrantRequest, grant::SessionGrantView, grant::EnableGrantRequest,
        grant::AllowanceView, grant::RevokeResponse,
        device::DeviceTokenRequest, device::ForgetResponse,
        autopay::MandateSetupRequest, autopay::MandateSetupView, autopay::MandateEnableRequest,
        autopay::MandateView, autopay::ChargeRequest, autopay::ChargeView,
        EnrollResponse, TotpCodeRequest, AuthorizeRequest, AuthorizeResponse, SignedEventView
    )),
    tags(
        (name = "meta", description = "Liveness and execution gates"),
        (name = "merchant", description = "API-key authenticated merchant surface"),
        (name = "checkout", description = "Public hosted checkout; no merchant keys")
    )
)]
pub struct PublicApiDoc;

impl PublicApiDoc {
    pub fn to_pretty_json() -> String {
        Self::openapi().to_pretty_json().expect("openapi serialises")
    }
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            IDEMPOTENCY_HEADER,
            HeaderName::from_static(merchant_admin::SIGNUP_HEADER),
        ])
}

fn checkout_view(
    session: &store::CheckoutSession,
    intent: &PaymentIntent,
    name: String,
    attempt: Option<&pcc_payments::domain::PaymentAttempt>,
) -> CheckoutView {
    CheckoutView {
        session_id: session.id.clone(),
        merchant_name: name,
        merchant_order_id: intent.merchant_order_id.clone(),
        output_amount: intent.output_amount_base_units.to_string(),
        destination_asset: intent.payout_config_snapshot.asset.one_click_asset_id.clone(),
        source_asset: attempt.map_or_else(
            || AssetId::usdc_base().one_click_asset_id,
            |a| a.source_asset.one_click_asset_id.clone(),
        ),
        receiving_address: intent.payout_config_snapshot.receiving_address.to_string(),
        expires_at: session.expires_at.to_rfc3339(),
        attempt_id: attempt.map(|a| a.id.to_string()),
        quoted_input: attempt.map(|a| a.quoted_input.to_string()),
        max_debit: attempt.map(|a| a.max_debit.to_string()),
        deposit_address: attempt.and_then(|a| a.deposit_address.clone()),
        attempt_status: attempt.map(|a| snake_status(a.status)),
        status: snake_status(intent.status),
        success_url: session.success_url.clone(),
        intent_id: intent.id.to_string(),
    }
}

fn webhook_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(StdDuration::from_secs(5))
        .connect_timeout(StdDuration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn integrations_from_env(base_url: &str) -> Option<Arc<dyn CheckoutPorts>> {
    let token = std::env::var("INTEGRATIONS_INTERNAL_TOKEN").ok()?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let cfg = IntegrationClientConfig {
        base_url: base_url.to_owned(),
        bearer_token: token,
        timeout: StdDuration::from_secs(25),
    };
    match IntegrationClient::new(cfg) {
        Ok(client) => Some(Arc::new(client)),
        Err(e) => {
            tracing::warn!(error = %e, "integration client not configured; checkout quotes stay local");
            None
        }
    }
}

fn enqueue_and_deliver(state: &AppState, intent: &PaymentIntent, event: &str, payload: &str) {
    state.store.enqueue_event(intent, event, payload);
    spawn_webhook_delivery(state.clone());
}

fn spawn_deposit_hint(state: &AppState, attempt_id: PaymentAttemptId) {
    let Some(ports) = state.integrations.clone() else {
        return;
    };
    let Some(a) = state.store.get_attempt(attempt_id) else {
        return;
    };
    if let (Some(deposit), Some(tx)) = (a.deposit_address, a.source_tx_hash) {
        tokio::spawn(async move {
            let delays = [5, 20, 60].map(StdDuration::from_secs);
            checkout::submit_deposit_hint(ports.as_ref(), &deposit, &tx, &delays).await;
        });
    }
}

/// Deliver every due webhook once. Failures are rescheduled by
/// `mark_delivered` on the bounded backoff until `WEBHOOK_MAX_ATTEMPTS`.
async fn deliver_due_webhooks(state: &AppState) {
    for (id, url, body, signature) in state.store.claim_due_deliveries(Utc::now(), Duration::seconds(30)) {
        let result = deliver_signed(
            &state.webhook_http,
            &url,
            &body,
            &signature,
            state.config.webhook_policy,
        )
        .await;
        if let Err(e) = &result {
            tracing::info!(delivery_id = %id, error = %e, "webhook delivery failed; will retry");
        }
        state.store.mark_delivered(id, result.is_ok());
    }
}

/// Send the first attempt immediately instead of waiting for the next tick.
fn spawn_webhook_delivery(state: AppState) {
    tokio::spawn(async move { deliver_due_webhooks(&state).await });
}

fn spawn_webhook_retrier(state: AppState, every: StdDuration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            deliver_due_webhooks(&state).await;
        }
    });
}

fn demo_usdc_arb() -> AssetId {
    AssetId::usdc_arbitrum()
}

/// Well-known local demo keys, loaded only when `PCC_DEMO_MERCHANTS=true`.
pub const DEMO_MERCHANT_A_KEY: &str = "pcc_sk_alice_demo";
pub const DEMO_MERCHANT_B_KEY: &str = "pcc_sk_bob_demo";

fn seed_demo_merchants(store: &Store) {
    use pcc_merchants::new_payout_config;
    let alice = MerchantId::new();
    let bob = MerchantId::new();
    store.register_merchant(
        "Alice Demo",
        DEMO_MERCHANT_A_KEY,
        new_payout_config(
            alice,
            demo_usdc_arb(),
            "0x1111111111111111111111111111111111111111"
                .parse()
                .expect("alice recv"),
        )
        .expect("alice payout"),
        "whsec_alice_demo",
    );
    store.register_merchant(
        "Bob Demo",
        DEMO_MERCHANT_B_KEY,
        new_payout_config(
            bob,
            demo_usdc_arb(),
            "0x2222222222222222222222222222222222222222".parse().expect("bob recv"),
        )
        .expect("bob payout"),
        "whsec_bob_demo",
    );
}

pub fn router(config: Config) -> Router {
    let master = totp_master_from_env();
    let store = Store::default();
    if std::env::var("PCC_DEMO_MERCHANTS").as_deref() == Ok("true") {
        seed_demo_merchants(&store);
        tracing::info!("demo merchants seeded (PCC_DEMO_MERCHANTS=true); keys are in the M6 runbook, not in logs");
    }
    let integrations = integrations_from_env(&config.integrations_base_url);
    let mut state = app_state(config, store, TotpAuthorizer::new(master), integrations);
    state.code_reader = Some(Arc::new(settle::RpcDestinationVerifier::from_env()));
    spawn_webhook_retrier(state.clone(), StdDuration::from_secs(2));
    if let Some(ports) = state.integrations.clone() {
        let secs = std::env::var("SETTLEMENT_POLL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10u64)
            .max(2);
        settle::spawn_poller(state.clone(), ports, StdDuration::from_secs(secs));
    }
    app(state)
}

pub fn router_with(config: Config, store: Store, authorizer: TotpAuthorizer) -> Router {
    router_with_ports(config, store, authorizer, None)
}

pub fn router_with_ports(
    config: Config,
    store: Store,
    authorizer: TotpAuthorizer,
    integrations: Option<Arc<dyn CheckoutPorts>>,
) -> Router {
    app(app_state(config, store, authorizer, integrations))
}

pub fn app_state(
    config: Config,
    store: Store,
    authorizer: TotpAuthorizer,
    integrations: Option<Arc<dyn CheckoutPorts>>,
) -> AppState {
    AppState {
        config: Arc::new(config),
        store: Arc::new(store),
        authorizer: Arc::new(authorizer),
        integrations,
        webhook_http: webhook_http_client(),
        code_reader: None,
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/meta/gates", get(gates))
        .route("/v1/me", get(me))
        .route("/v1/payout-config", put(put_payout).get(merchant_admin::get_payout))
        .route("/v1/merchants", post(merchant_admin::signup))
        .route("/v1/api-keys/rotate", post(merchant_admin::rotate_api_key))
        .route("/v1/webhook-secret/rotate", post(merchant_admin::rotate_webhook_secret))
        .route("/v1/webhook-url", put(put_webhook))
        .route("/v1/payment-intents", post(create_intent).get(list_intents))
        .route("/v1/payment-intents/{id}", get(get_intent))
        .route("/v1/payment-intents/{id}/events", get(list_events))
        .route("/v1/checkout/{session_id}", get(get_checkout))
        .route("/v1/checkout/{session_id}/siwe/challenge", post(siwe_challenge))
        .route("/v1/checkout/{session_id}/siwe/verify", post(siwe_verify))
        .route("/v1/checkout/{session_id}/source-assets", get(list_source_assets))
        .route("/v1/checkout/{session_id}/source-asset", post(select_source_asset))
        .route("/v1/checkout/{session_id}/totp/enroll", post(totp_enroll))
        .route("/v1/checkout/{session_id}/totp/reset", post(totp_reset))
        .route("/v1/checkout/{session_id}/totp/confirm", post(totp_confirm))
        .route(
            "/v1/checkout/{session_id}/delegation",
            get(onboarding::delegation_status),
        )
        .route(
            "/v1/checkout/{session_id}/session-grant",
            get(grant::allowance_status).post(grant::prepare_grant),
        )
        .route(
            "/v1/checkout/{session_id}/session-grant/enable",
            post(grant::enable_grant),
        )
        .route(
            "/v1/checkout/{session_id}/session-grant/revoke",
            post(grant::revoke_allowance),
        )
        .route(
            "/v1/checkout/{session_id}/autopay",
            get(autopay::list_for_payer).post(autopay::setup),
        )
        .route(
            "/v1/checkout/{session_id}/autopay/{mandate_id}/enable",
            post(autopay::enable),
        )
        .route(
            "/v1/checkout/{session_id}/autopay/{mandate_id}/revoke",
            post(autopay::revoke),
        )
        .route("/v1/checkout/{session_id}/device/resume", post(device::resume))
        .route("/v1/devices/forget", post(device::forget))
        .route("/v1/autopay", get(autopay::list_for_merchant))
        .route("/v1/autopay/{mandate_id}/charges", post(autopay::charge))
        .route("/v1/checkout/{session_id}/authorize", post(authorize))
        .route("/v1/operator/invites", post(operator::issue_invite))
        .route("/v1/operator/approvals", get(operator::list_pending))
        .route("/v1/operator/approvals/{attempt_id}/approve", post(operator::approve))
        .route("/v1/operator/approvals/{attempt_id}/reject", post(operator::reject))
        .layer(cors_layer())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

fn totp_master_from_env() -> Vec<u8> {
    match std::env::var("TOTP_WRAP_KEY") {
        Ok(v) if v.trim().len() >= 32 => return v.trim().as_bytes().to_vec(),
        Ok(_) => tracing::warn!("TOTP_WRAP_KEY shorter than 32 chars; using an ephemeral key"),
        Err(_) => tracing::warn!("TOTP_WRAP_KEY unset; TOTP enrollments will not survive a restart"),
    }
    let mut bytes = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use pcc_merchants::new_payout_config;
    use pcc_settlement::webhook::sign_body;
    use pcc_settlement::webhook::verify_signature;
    use tower::ServiceExt;

    const KEY_A: &str = "pcc_sk_alice";
    const KEY_B: &str = "pcc_sk_bob";
    const WH_A: &str = "whsec_alice";
    const WH_B: &str = "whsec_bob";

    fn cfg() -> Config {
        Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            gates: ExecutionGates::default(),
            integrations_base_url: "http://127.0.0.1:8787".into(),
            checkout_base_url: "http://127.0.0.1:3000".into(),
            webhook_policy: WebhookPolicy::default(),
            approval_mode: operator::ApprovalMode::Payer,
            operator_token_hash: Some(secret_hash(OPERATOR_TOKEN)),
            siwe_domain: "127.0.0.1:3000".into(),
            signup_token_hash: Some(secret_hash(SIGNUP_TOKEN)),
        }
    }

    const OPERATOR_TOKEN: &str = "op-test-token-0123456789abcdef";
    const SIGNUP_TOKEN: &str = "signup-test-token-0123456789ab";

    fn open_gates() -> ExecutionGates {
        ExecutionGates {
            dry_run: false,
            enable_mainnet_transfers: true,
        }
    }

    fn usdc_arb() -> AssetId {
        AssetId::usdc_arbitrum()
    }

    fn seeded() -> Router {
        let store = Store::default();
        let alice = MerchantId::new();
        let bob = MerchantId::new();
        store.register_merchant(
            "Alice",
            KEY_A,
            new_payout_config(
                alice,
                usdc_arb(),
                "0x1111111111111111111111111111111111111111".parse().unwrap(),
            )
            .unwrap(),
            WH_A,
        );
        store.register_merchant(
            "Bob",
            KEY_B,
            new_payout_config(
                bob,
                usdc_arb(),
                "0x2222222222222222222222222222222222222222".parse().unwrap(),
            )
            .unwrap(),
            WH_B,
        );
        router_with(cfg(), store, TotpAuthorizer::new(b"test-master".to_vec()))
    }

    async fn call(app: Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        if body.is_empty() {
            return (status, serde_json::Value::Null);
        }
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn json_req(
        method: &str,
        uri: &str,
        key: Option<&str>,
        idem: Option<&str>,
        body: serde_json::Value,
    ) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(k) = key {
            b = b.header(header::AUTHORIZATION, format!("Bearer {k}"));
        }
        if let Some(i) = idem {
            b = b.header("Idempotency-Key", i);
        }
        b.header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let (status, v) = call(seeded(), Request::builder().uri("/health").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn gates_default_to_safe_values() {
        let (_, v) = call(
            seeded(),
            Request::builder().uri("/v1/meta/gates").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["enable_mainnet_transfers"], false);
    }

    #[test]
    fn committed_public_openapi_matches_rust_types() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi/public-v1.json");
        let committed = std::fs::read_to_string(path).expect("openapi/public-v1.json is committed");
        let committed: serde_json::Value = serde_json::from_str(&committed).unwrap();
        let generated: serde_json::Value = serde_json::from_str(&PublicApiDoc::to_pretty_json()).unwrap();
        assert_eq!(
            committed, generated,
            "public OpenAPI drifted; re-run export-public-openapi"
        );
    }

    #[tokio::test]
    async fn merchant_tenancy_and_idempotency() {
        let app = seeded();
        let body = serde_json::json!({"merchant_order_id":"o1","output_amount":"1000000"});
        let (st, a1) = call(
            app.clone(),
            json_req("POST", "/v1/payment-intents", Some(KEY_A), Some("idem-1"), body.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, a2) = call(
            app.clone(),
            json_req("POST", "/v1/payment-intents", Some(KEY_A), Some("idem-1"), body.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(a1["id"], a2["id"]);
        let id = a1["id"].as_str().unwrap();
        let (st, _) = call(
            app.clone(),
            Request::builder()
                .uri(format!("/v1/payment-intents/{id}"))
                .header(header::AUTHORIZATION, format!("Bearer {KEY_B}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, me_a) = call(
            app.clone(),
            Request::builder()
                .uri("/v1/me")
                .header(header::AUTHORIZATION, format!("Bearer {KEY_A}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(me_a["name"], "Alice");
        let (st, _) = call(
            app,
            Request::builder()
                .uri("/v1/me")
                .header(header::AUTHORIZATION, "Bearer pcc_sk_unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_ssrf_and_hmac() {
        let app = seeded();
        let (st, err) = call(
            app.clone(),
            json_req(
                "PUT",
                "/v1/webhook-url",
                Some(KEY_A),
                None,
                serde_json::json!({"url":"https://127.0.0.1/x"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(err["code"], "ssrf_blocked");
        let (st, _) = call(
            app.clone(),
            json_req(
                "PUT",
                "/v1/webhook-url",
                Some(KEY_A),
                None,
                serde_json::json!({"url":"https://hooks.example.com/pcc"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, created) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/payment-intents",
                Some(KEY_A),
                Some("idem-wh"),
                serde_json::json!({"merchant_order_id":"o2","output_amount":"1000000"}),
            ),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let (st, events) = call(
            app,
            Request::builder()
                .uri(format!("/v1/payment-intents/{id}/events"))
                .header(header::AUTHORIZATION, format!("Bearer {KEY_A}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let payload = events[0]["payload"].as_str().unwrap();
        let signature = events[0]["signature"].as_str().unwrap();
        assert!(verify_signature(WH_A.as_bytes(), payload.as_bytes(), signature));
        assert!(!verify_signature(WH_B.as_bytes(), payload.as_bytes(), signature));
        assert_eq!(sign_body(WH_A.as_bytes(), payload.as_bytes()), signature);
    }

    #[tokio::test]
    async fn stolen_session_and_open_redirect() {
        let app = seeded();
        let (st, _) = call(
            app.clone(),
            Request::builder()
                .uri("/v1/checkout/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, err) = call(
            app,
            json_req(
                "POST",
                "/v1/payment-intents",
                Some(KEY_A),
                Some("idem-redir"),
                serde_json::json!({
                    "merchant_order_id":"o3",
                    "output_amount":"1",
                    "success_url":"javascript:alert(1)"
                }),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(err["code"], "open_redirect");
    }

    async fn start_intent(app: Router) -> (Router, String) {
        let (_, created) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/payment-intents",
                Some(KEY_A),
                Some(&Uuid::new_v4().to_string()),
                serde_json::json!({"merchant_order_id":"pay","output_amount":"500000"}),
            ),
        )
        .await;
        let sid = created["checkout_session_id"].as_str().unwrap().to_owned();
        (app, sid)
    }

    async fn siwe_as(app: Router, sid: &str, signer: &PrivateKeySigner) -> serde_json::Value {
        let addr = signer.address().to_string();
        let (_, ch) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/challenge"),
                None,
                None,
                serde_json::json!({"address": addr}),
            ),
        )
        .await;
        let message = ch["message"].as_str().unwrap();
        let signature = signer.sign_message_sync(message.as_bytes()).unwrap();
        let (_, v) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/verify"),
                None,
                None,
                serde_json::json!({"message": message, "signature": format!("{signature}")}),
            ),
        )
        .await;
        v
    }

    #[tokio::test]
    async fn siwe_uses_configured_domain_and_exact_message() {
        let mut config = cfg();
        config.siwe_domain = "pay.example.com".into();
        let app = router_with(config, alice_store(), TotpAuthorizer::new(b"test-master".to_vec()));
        let (app, sid) = start_intent(app).await;
        let signer = PrivateKeySigner::random();
        let (_, ch) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/challenge"),
                None,
                None,
                serde_json::json!({"address": signer.address().to_string()}),
            ),
        )
        .await;
        let issued = ch["message"].as_str().unwrap();
        assert!(issued.starts_with("pay.example.com wants you to sign in"), "{issued}");

        let verify = |message: String| {
            let signature = signer.sign_message_sync(message.as_bytes()).unwrap();
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/verify"),
                None,
                None,
                serde_json::json!({"message": message, "signature": format!("{signature}")}),
            )
        };
        for altered in [
            issued.replacen("pay.example.com", "localhost:3000", 1),
            issued.replacen("/checkout/", "/elsewhere/", 1),
        ] {
            let (st, err) = call(app.clone(), verify(altered)).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
            assert_eq!(err["code"], "siwe_invalid");
        }
        let (st, ok) = call(app, verify(issued.to_owned())).await;
        assert_eq!(st, StatusCode::OK, "{ok}");
    }

    #[tokio::test]
    async fn siwe_domain_follows_the_checkout_page_origin() {
        let app = seeded();
        let (app, sid) = start_intent(app).await;
        let signer = PrivateKeySigner::random();
        let req = Request::builder()
            .method("POST")
            .uri(format!("/v1/checkout/{sid}/siwe/challenge"))
            .header(header::ORIGIN, "https://pay.example.trycloudflare.com")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"address": signer.address().to_string()}).to_string(),
            ))
            .unwrap();
        let (st, ch) = call(app, req).await;
        assert_eq!(st, StatusCode::OK, "{ch}");
        let message = ch["message"].as_str().unwrap();
        assert!(
            message.starts_with("pay.example.trycloudflare.com wants you to sign in"),
            "{message}"
        );
        assert!(
            message.contains("https://pay.example.trycloudflare.com/checkout/"),
            "{message}"
        );
    }

    #[test]
    fn siwe_domain_defaults_to_checkout_origin() {
        assert_eq!(siwe_domain_of("http://127.0.0.1:3000").unwrap(), "127.0.0.1:3000");
        assert_eq!(siwe_domain_of("https://pay.example.com/").unwrap(), "pay.example.com");
        assert!(siwe_domain_of("not a url").is_err());
    }

    #[tokio::test]
    async fn unauthenticated_signer_is_rejected() {
        let (app, sid) = start_intent(seeded()).await;
        let owner = PrivateKeySigner::random();
        let thief = PrivateKeySigner::random();
        let (_, ch) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/challenge"),
                None,
                None,
                serde_json::json!({"address": owner.address().to_string()}),
            ),
        )
        .await;
        let message = ch["message"].as_str().unwrap();
        let signature = thief.sign_message_sync(message.as_bytes()).unwrap();
        let (st, err) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/siwe/verify"),
                None,
                None,
                serde_json::json!({"message": message, "signature": format!("{signature}")}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(err["code"], "siwe_invalid");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn totp_bind_replay_and_foreign_attempt() {
        let store = Store::default();
        let alice = MerchantId::new();
        store.register_merchant(
            "Alice",
            KEY_A,
            new_payout_config(
                alice,
                usdc_arb(),
                "0x1111111111111111111111111111111111111111".parse().unwrap(),
            )
            .unwrap(),
            WH_A,
        );
        let authorizer = TotpAuthorizer::new(b"test-master".to_vec());
        let app = router_with(cfg(), store, authorizer);
        // We need the same authorizer instance to mint codes. Re-seed via enroll API
        // then generate_current_code_for_tests requires a handle — use debug enroll URI secret.
        let (app, sid) = start_intent(app).await;
        let signer = PrivateKeySigner::random();
        let verified = siwe_as(app.clone(), &sid, &signer).await;
        let attempt_id = verified["attempt_id"].as_str().unwrap().to_owned();
        let (st, enroll) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/enroll"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let uri = enroll["otpauth_uri"].as_str().unwrap();
        let secret = uri.split("secret=").nth(1).unwrap().split('&').next().unwrap();
        let bytes = pcc_auth::totp::decode_secret(secret).unwrap();
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        let code = format!("{:06}", pcc_auth::totp::code_at(&bytes, now));
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/confirm"),
                None,
                None,
                serde_json::json!({"code": code}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/authorize"),
                None,
                None,
                serde_json::json!({"code": "000000"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, auth) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/authorize"),
                None,
                None,
                serde_json::json!({"code": code}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(auth["status"], "authorized");
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/authorize"),
                None,
                None,
                serde_json::json!({"code": code}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (app2, sid2) = start_intent(seeded()).await;
        let signer2 = PrivateKeySigner::random();
        let _ = siwe_as(app2.clone(), &sid2, &signer2).await;
        let (st, err) = call(
            app2,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid2}/authorize"),
                None,
                None,
                serde_json::json!({"code": code, "attempt_id": attempt_id}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(err["code"], "attempt_mismatch");
    }

    fn alice_store() -> Store {
        let store = Store::default();
        store.register_merchant(
            "Alice",
            KEY_A,
            new_payout_config(
                MerchantId::new(),
                usdc_arb(),
                "0x1111111111111111111111111111111111111111".parse().unwrap(),
            )
            .unwrap(),
            WH_A,
        );
        store
    }

    #[tokio::test]
    async fn authorize_quotes_and_stays_gated() {
        let authorizer = TotpAuthorizer::new(b"test-master".to_vec());
        let ports: Arc<dyn CheckoutPorts> = Arc::new(pcc_payments::mock::MockCheckout::default());
        let app = router_with_ports(cfg(), alice_store(), authorizer, Some(ports));
        let (app, sid, auth) = gated_checkout(app).await;
        assert_eq!(auth["status"], "authorized");
        assert_eq!(auth["execution"], "gated");
        assert_eq!(auth["simulation_ok"], true);
        assert!(auth["deposit_address"].as_str().unwrap().starts_with("0x"));
        let (st, view) = call(
            app,
            Request::builder()
                .uri(format!("/v1/checkout/{sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(view["source_asset"], AssetId::usdc_base().one_click_asset_id);
        assert_eq!(view["deposit_address"], auth["deposit_address"]);
    }

    #[tokio::test]
    async fn source_asset_selection_reprices_and_locks_after_authorize() {
        let ports: Arc<dyn CheckoutPorts> = Arc::new(pcc_payments::mock::MockCheckout::default());
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(ports),
        );
        let (app, sid) = start_intent(app).await;
        let select = |asset: &str| {
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/source-asset"),
                None,
                None,
                serde_json::json!({"asset_id": asset}),
            )
        };
        let arb = AssetId::usdc_arbitrum().one_click_asset_id;
        let base = AssetId::usdc_base().one_click_asset_id;

        let (st, err) = call(app.clone(), select(&base)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{err}");

        siwe_as(app.clone(), &sid, &PrivateKeySigner::random()).await;
        let (st, list) = call(
            app.clone(),
            Request::builder()
                .uri(format!("/v1/checkout/{sid}/source-assets"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let ids: Vec<&str> = list["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["asset_id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&base.as_str()));
        assert!(!ids.contains(&arb.as_str()), "payout asset is never a source");
        assert_eq!(list["selected"], base);

        let (st, picked) = call(app.clone(), select(&base)).await;
        assert_eq!(st, StatusCode::OK, "{picked}");
        assert_eq!(picked["source_asset"], base);
        assert_ne!(picked["max_debit"], "");

        for bad in [arb.as_str(), "nep141:sol.omft.near"] {
            let (st, err) = call(app.clone(), select(bad)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert_eq!(err["code"], "unsupported_asset");
        }
    }

    #[tokio::test]
    async fn source_asset_cannot_change_after_authorize() {
        let ports: Arc<dyn CheckoutPorts> = Arc::new(pcc_payments::mock::MockCheckout::default());
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(ports),
        );
        let (app, sid, _) = gated_checkout(app).await;
        let (st, err) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/source-asset"),
                None,
                None,
                serde_json::json!({"asset_id": AssetId::usdc_base().one_click_asset_id}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "attempt_locked");
    }

    #[tokio::test]
    async fn webhook_retries_after_failure_without_double_send() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let mut config = cfg();
        config.webhook_policy = WebhookPolicy { allow_loopback: true };
        let store = alice_store();
        let alice = store.authenticate(KEY_A).unwrap();
        store
            .set_webhook_url(alice, format!("{}/hook", server.uri()), config.webhook_policy)
            .unwrap();
        let state = app_state(config, store, TotpAuthorizer::new(b"test-master".to_vec()), None);
        let _ = start_intent(app(state.clone())).await;
        let sent = || async { server.received_requests().await.unwrap().len() };

        deliver_due_webhooks(&state).await;
        deliver_due_webhooks(&state).await;
        assert_eq!(sent().await, 1, "failed delivery waits for backoff");

        tokio::time::sleep(StdDuration::from_millis(2_100)).await;
        deliver_due_webhooks(&state).await;
        deliver_due_webhooks(&state).await;
        assert_eq!(sent().await, 2, "retried once, then delivered and not re-sent");
    }

    fn live_state(mode: operator::ApprovalMode) -> (AppState, Arc<pcc_payments::mock::MockCheckout>) {
        let mock = Arc::new(pcc_payments::mock::MockCheckout::default());
        let ports: Arc<dyn CheckoutPorts> = mock.clone();
        let mut config = cfg();
        config.gates = open_gates();
        config.approval_mode = mode;
        let state = app_state(
            config,
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(ports),
        );
        (state, mock)
    }

    fn operator_get(token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().uri("/v1/operator/approvals");
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(Body::empty()).unwrap()
    }

    fn approve_req(attempt: &str, token: &str, echo: &serde_json::Value) -> Request<Body> {
        json_req(
            "POST",
            &format!("/v1/operator/approvals/{attempt}/approve"),
            Some(token),
            None,
            echo.clone(),
        )
    }

    #[tokio::test]
    async fn owner_mode_parks_deposit_until_operator_echoes_it() {
        let (state, mock) = live_state(operator::ApprovalMode::Owner);
        let (app, _, auth) = gated_checkout(app(state.clone())).await;
        assert_eq!(auth["execution"], "awaiting_approval");
        assert!(
            mock.executor.last_plan.lock().unwrap().is_none(),
            "nothing sent at authorize"
        );
        let attempt = auth["attempt_id"].as_str().unwrap().to_owned();

        let (st, _) = call(app.clone(), operator_get(None)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = call(app.clone(), operator_get(Some("wrong-token-0123456789abcdef"))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, list) = call(app.clone(), operator_get(Some(OPERATOR_TOKEN))).await;
        assert_eq!(st, StatusCode::OK);
        let p = &list[0];
        assert_eq!(p["attempt_id"], attempt);
        assert_eq!(
            p["deposit_address"].as_str().unwrap().to_lowercase(),
            auth["deposit_address"].as_str().unwrap().to_lowercase()
        );
        let echo = serde_json::json!({
            "chain_id": p["chain_id"],
            "deposit_address": p["deposit_address"],
            "amount": p["amount"],
            "max_fee_wei": p["max_fee_wei"],
        });

        let (st, _) = call(
            app.clone(),
            approve_req(&attempt, "wrong-token-0123456789abcdef", &echo),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        for (field, bad) in [
            ("amount", serde_json::json!("1")),
            ("chain_id", serde_json::json!(42161)),
            ("max_fee_wei", serde_json::json!("1")),
            (
                "deposit_address",
                serde_json::json!("0x3333333333333333333333333333333333333333"),
            ),
        ] {
            let mut wrong = echo.clone();
            wrong[field] = bad;
            let (st, err) = call(app.clone(), approve_req(&attempt, OPERATOR_TOKEN, &wrong)).await;
            assert_eq!(st, StatusCode::CONFLICT, "{field}");
            assert_eq!(err["code"], "approval_mismatch");
        }
        assert!(
            mock.executor.last_plan.lock().unwrap().is_none(),
            "mismatches never send"
        );

        let (st, ok) = call(app.clone(), approve_req(&attempt, OPERATOR_TOKEN, &echo)).await;
        assert_eq!(st, StatusCode::OK, "{ok}");
        assert_eq!(ok["execution"], "broadcast");
        assert_eq!(ok["attempt_status"], "deposit_broadcast");
        let sent = mock.executor.last_plan.lock().unwrap().clone().unwrap();
        assert_eq!(sent.approval_ref, format!("operator:{attempt}"));

        let (st, _) = call(app, approve_req(&attempt, OPERATOR_TOKEN, &echo)).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "approvals are single use");
    }

    #[tokio::test]
    async fn expired_or_rejected_approvals_never_send() {
        let (state, mock) = live_state(operator::ApprovalMode::Owner);
        let (app, _, auth) = gated_checkout(app(state.clone())).await;
        let id = PaymentAttemptId(auth["attempt_id"].as_str().unwrap().parse().unwrap());
        let mut parked = state.store.pending_approval(id).unwrap();
        let echo = serde_json::json!({
            "chain_id": parked.plan.chain_id.0,
            "deposit_address": parked.plan.to.to_string(),
            "amount": parked.plan.amount.to_string(),
            "max_fee_wei": parked.plan.max_fee_wei.to_string(),
        });

        parked.expires_at = Utc::now() - Duration::seconds(1);
        state.store.put_pending(parked.clone());
        let (st, err) = call(app.clone(), approve_req(&id.to_string(), OPERATOR_TOKEN, &echo)).await;
        assert_eq!(st, StatusCode::GONE);
        assert_eq!(err["code"], "approval_expired");

        parked.expires_at = Utc::now() + Duration::minutes(5);
        state.store.put_pending(parked);
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/operator/approvals/{id}/reject"),
                Some(OPERATOR_TOKEN),
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(app, approve_req(&id.to_string(), OPERATOR_TOKEN, &echo)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(mock.executor.last_plan.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn payer_mode_sends_at_authorize() {
        let (state, mock) = live_state(operator::ApprovalMode::Payer);
        let (_, _, auth) = gated_checkout(app(state)).await;
        assert_eq!(auth["execution"], "broadcast");
        assert!(mock.executor.last_plan.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn operator_routes_are_off_without_a_token() {
        let mut config = cfg();
        config.operator_token_hash = None;
        let app = router_with(config, alice_store(), TotpAuthorizer::new(b"test-master".to_vec()));
        let (st, err) = call(app, operator_get(Some(OPERATOR_TOKEN))).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err["code"], "operator_unconfigured");
    }

    #[test]
    fn approval_mode_defaults_to_payer() {
        use operator::ApprovalMode as M;
        assert_eq!(M::from_env_value(None), M::Payer);
        assert_eq!(M::from_env_value(Some("payer")), M::Payer);
        assert_eq!(M::from_env_value(Some("yes")), M::Payer);
        assert_eq!(M::from_env_value(Some(" owner ")), M::Owner);
        assert_eq!(M::from_env_value(Some("Owner")), M::Payer);
    }

    struct FixedVerifier(bool);

    #[async_trait::async_trait]
    impl settle::DestinationVerifier for FixedVerifier {
        async fn verify(
            &self,
            _: &AssetId,
            _: alloy_primitives::Address,
            _: BaseUnits,
            _: &str,
        ) -> Result<bool, String> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn poller_settles_only_after_destination_verified() {
        use pcc_payments::status::{OneClickStatus, PaymentAttemptStatus, PaymentIntentStatus};
        let mock = Arc::new(pcc_payments::mock::MockCheckout::default());
        let ports: Arc<dyn CheckoutPorts> = mock.clone();
        let state = app_state(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(ports.clone()),
        );
        let (_, _, auth) = gated_checkout(app(state.clone())).await;
        let attempt_id = PaymentAttemptId(auth["attempt_id"].as_str().unwrap().parse().unwrap());
        let deposit = auth["deposit_address"].as_str().unwrap().to_owned();

        settle::poll_once(&state, ports.as_ref(), &FixedVerifier(true)).await;
        assert_eq!(
            state.store.get_attempt(attempt_id).unwrap().status,
            PaymentAttemptStatus::Authorized,
            "nothing is polled before broadcast"
        );

        state.store.mark_broadcast(attempt_id, "0xsource", "test-approval");
        mock.quotes.set_status(&deposit, OneClickStatus::Processing);
        settle::poll_once(&state, ports.as_ref(), &FixedVerifier(true)).await;
        assert_eq!(
            state.store.get_attempt(attempt_id).unwrap().status,
            PaymentAttemptStatus::Processing
        );

        mock.quotes.set_status(&deposit, OneClickStatus::Success);
        settle::poll_once(&state, ports.as_ref(), &FixedVerifier(false)).await;
        assert_eq!(
            state.store.get_attempt(attempt_id).unwrap().status,
            PaymentAttemptStatus::Processing,
            "1Click SUCCESS alone is not settlement"
        );

        settle::poll_once(&state, ports.as_ref(), &FixedVerifier(true)).await;
        let attempt = state.store.get_attempt(attempt_id).unwrap();
        assert_eq!(attempt.status, PaymentAttemptStatus::Settled);
        assert!(attempt.destination_tx_hash.is_some());
        let intent = state.store.intent_by_id(attempt.payment_intent_id).unwrap();
        assert_eq!(intent.status, PaymentIntentStatus::Succeeded);
        assert!(state.store.in_flight_attempts().is_empty());
    }

    async fn gated_checkout(app: Router) -> (Router, String, serde_json::Value) {
        let (app, sid, code) = totp_ready(app).await;
        enable_grant(app.clone(), &sid).await;
        let (st, auth) = call(app.clone(), authorize_req(&sid, &code)).await;
        assert_eq!(st, StatusCode::OK);
        (app, sid, auth)
    }

    fn authorize_req(sid: &str, code: &str) -> Request<Body> {
        json_req(
            "POST",
            &format!("/v1/checkout/{sid}/authorize"),
            None,
            None,
            serde_json::json!({"code": code}),
        )
    }

    /// SIWE + TOTP enrollment done; returns a currently valid code.
    async fn totp_ready(app: Router) -> (Router, String, String) {
        let (app, sid) = start_intent(app).await;
        let signer = PrivateKeySigner::random();
        let verified = siwe_as(app.clone(), &sid, &signer).await;
        assert_ne!(verified["quoted_input"], "");
        let (st, enroll) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/enroll"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let uri = enroll["otpauth_uri"].as_str().unwrap();
        let secret = uri.split("secret=").nth(1).unwrap().split('&').next().unwrap();
        let bytes = pcc_auth::totp::decode_secret(secret).unwrap();
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        let code = format!("{:06}", pcc_auth::totp::code_at(&bytes, now));
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/confirm"),
                None,
                None,
                serde_json::json!({"code": code}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        (app, sid, code)
    }

    #[tokio::test]
    async fn dashboard_preflight_allows_signup_and_operator_headers() {
        let app = app(app_state(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"m".to_vec()),
            None,
        ));
        let preflight = |path: &str, origin: &str, headers: &str| {
            Request::builder()
                .method("OPTIONS")
                .uri(path)
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", headers)
                .body(Body::empty())
                .unwrap()
        };
        for (path, headers) in [
            ("/v1/merchants", "content-type,x-signup-token"),
            ("/v1/operator/approvals/x/approve", "authorization,content-type"),
        ] {
            let res = app
                .clone()
                .oneshot(preflight(path, "http://127.0.0.1:3001", headers))
                .await
                .unwrap();
            let allowed = res
                .headers()
                .get("access-control-allow-headers")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            for h in headers.split(',') {
                assert!(allowed.contains(h), "{path}: {h} not allowed in {allowed:?}");
            }
            assert_eq!(res.headers().get("access-control-allow-origin").unwrap(), "*");
        }
        let res = app
            .oneshot(preflight("/v1/merchants", "https://shop.example", "x-signup-token"))
            .await
            .unwrap();
        assert_eq!(res.headers().get("access-control-allow-origin").unwrap(), "*");
    }

    #[tokio::test]
    async fn totp_enrolls_once_per_wallet_and_serves_later_payments() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let signer = PrivateKeySigner::random();
        let post = |path: String| json_req("POST", &path, None, None, serde_json::json!({}));

        let (app, first) = start_intent(app).await;
        let verified = siwe_as(app.clone(), &first, &signer).await;
        assert_eq!(verified["totp_enrolled"], false);
        let (st, enroll) = call(app.clone(), post(format!("/v1/checkout/{first}/totp/enroll"))).await;
        assert_eq!(st, StatusCode::OK);
        let uri = enroll["otpauth_uri"].as_str().unwrap();
        let secret = uri.split("secret=").nth(1).unwrap().split('&').next().unwrap();
        let bytes = pcc_auth::totp::decode_secret(secret).unwrap();
        let code_at = |offset: u64| {
            let now = u64::try_from(Utc::now().timestamp()).unwrap();
            format!("{:06}", pcc_auth::totp::code_at(&bytes, now + offset))
        };
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{first}/totp/confirm"),
                None,
                None,
                serde_json::json!({"code": code_at(0)}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        enable_grant(app.clone(), &first).await;
        let (st, _) = call(app.clone(), authorize_req(&first, &code_at(0))).await;
        assert_eq!(st, StatusCode::OK);

        let (app, second) = start_intent(app).await;
        let again = siwe_as(app.clone(), &second, &signer).await;
        assert_eq!(again["totp_enrolled"], true);
        assert_eq!(again["wallet_credential_id"], verified["wallet_credential_id"]);
        let (st, err) = call(app.clone(), post(format!("/v1/checkout/{second}/totp/enroll"))).await;
        assert_eq!(st, StatusCode::CONFLICT, "a confirmed secret is never replaced");
        assert_eq!(err["code"], "already_enrolled");
        let (st, status) = call(app.clone(), get_req(&format!("/v1/checkout/{second}/session-grant"))).await;
        assert_eq!(st, StatusCode::OK, "{status}");
        assert_eq!(
            status["covers_this_payment"], true,
            "one signature serves later payments"
        );
        let (st, auth) = call(app.clone(), authorize_req(&second, &code_at(30))).await;
        assert_eq!(st, StatusCode::OK, "{auth}");

        let other = PrivateKeySigner::random();
        let (app, third) = start_intent(app).await;
        let fresh = siwe_as(app.clone(), &third, &other).await;
        assert_eq!(fresh["totp_enrolled"], false, "enrollment is per wallet, not global");
    }

    #[tokio::test]
    async fn lost_authenticator_can_be_replaced_after_siwe() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let signer = PrivateKeySigner::random();
        let (app, sid) = start_intent(app).await;
        siwe_as(app.clone(), &sid, &signer).await;
        let first = enroll_wallet(app.clone(), &sid).await;
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/reset"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let second = enroll_wallet(app.clone(), &sid).await;
        assert_ne!(first, second);
        enable_grant(app.clone(), &sid).await;
        let (st, auth) = call(app, authorize_req(&sid, &totp_at(&second, 30))).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
    }

    #[tokio::test]
    async fn operator_invite_is_single_use() {
        let app = router_with(cfg(), Store::default(), TotpAuthorizer::new(b"test-master".to_vec()));
        let (st, issued) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/operator/invites",
                Some(OPERATOR_TOKEN),
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{issued}");
        let token = issued["invite_token"].as_str().unwrap();
        assert!(token.starts_with("pcc_inv_"));
        let body = serde_json::json!({"name": "Dana Shop", "payout": payout_body("0x5555555555555555555555555555555555555555")});
        let (st, created) = call(app.clone(), signup_req(Some(token), &body)).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let (st, err) = call(app, signup_req(Some(token), &body)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(err["code"], "unauthorized");
    }

    #[tokio::test]
    async fn authorize_requires_an_allowance_for_the_source_asset() {
        let mock = Arc::new(pcc_payments::mock::MockCheckout::default());
        let ports: Arc<dyn CheckoutPorts> = mock.clone();
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(ports),
        );
        let (app, sid, code) = totp_ready(app).await;

        let (st, err) = call(app.clone(), authorize_req(&sid, &code)).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "session_grant_required");

        let enable = |grant_id: &str, signature: &str| {
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/session-grant/enable"),
                None,
                None,
                serde_json::json!({"grant_id": grant_id, "signature": signature}),
            )
        };
        let (st, err) = call(app.clone(), enable("grant-1", MOCK_SIG)).await;
        assert_eq!(st, StatusCode::CONFLICT, "nothing prepared yet");
        assert_eq!(err["code"], "grant_mismatch");

        let (st, prepared) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/session-grant"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(prepared["typed_data"]["primaryType"], "Enable");
        assert_eq!(prepared["chain_id"], 8453);
        let grant_id = prepared["grant_id"].as_str().unwrap();
        let (st, err) = call(app.clone(), enable("grant-999", MOCK_SIG)).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "grant_mismatch");
        let (st, err) = call(app.clone(), enable(grant_id, "0xdeadbeef")).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(err["code"], "grant_rejected");
        let (st, view) = call(app.clone(), enable(grant_id, MOCK_SIG)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(view["uses_remaining"], grant::DEFAULT_USES);
        assert_eq!(view["covers_this_payment"], true);
        let (st, err) = call(app.clone(), enable(grant_id, MOCK_SIG)).await;
        assert_eq!(st, StatusCode::CONFLICT, "a prepared allowance enables once");
        assert_eq!(err["code"], "grant_mismatch");
        let enabled = mock.grants.enabled.lock().unwrap().clone();
        assert_eq!(enabled.len(), 1);
        assert_eq!(
            enabled[0].max_amount.to_string(),
            prepared["max_amount"].as_str().unwrap()
        );

        // Re-pricing the same source keeps the allowance: no second signature.
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/source-asset"),
                None,
                None,
                serde_json::json!({"asset_id": AssetId::usdc_base().one_click_asset_id}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, auth) = call(app, authorize_req(&sid, &code)).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        assert_eq!(auth["execution"], "gated");
    }

    fn payout_body(receiving: &str) -> serde_json::Value {
        serde_json::json!({
            "receiving_address": receiving,
            "chain_id": 42161,
            "token_address": "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
            "decimals": 6,
            "one_click_asset_id": "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near"
        })
    }

    fn signup_req(token: Option<&str>, body: &serde_json::Value) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri("/v1/merchants")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = token {
            b = b.header("x-signup-token", t);
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    fn get_with_key(uri: &str, key: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn signed_up(app: Router) -> serde_json::Value {
        let body = serde_json::json!({"name": "Carol Shop", "payout": payout_body("0x4444444444444444444444444444444444444444")});
        let (st, v) = call(app, signup_req(Some(SIGNUP_TOKEN), &body)).await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        v
    }

    #[tokio::test]
    async fn invite_gated_signup_issues_working_credentials() {
        let app = router_with(cfg(), Store::default(), TotpAuthorizer::new(b"test-master".to_vec()));
        let good = serde_json::json!({"name": "Carol Shop", "payout": payout_body("0x4444444444444444444444444444444444444444")});
        for token in [None, Some("wrong-signup-token-0123456789")] {
            let (st, _) = call(app.clone(), signup_req(token, &good)).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }
        for (body, code) in [
            (
                serde_json::json!({"name": "  ", "payout": payout_body("0x4444444444444444444444444444444444444444")}),
                "invalid_name",
            ),
            (
                serde_json::json!({"name": "x".repeat(81), "payout": payout_body("0x4444444444444444444444444444444444444444")}),
                "invalid_name",
            ),
            (
                serde_json::json!({"name": "Carol", "payout": payout_body("0x0000000000000000000000000000000000000000")}),
                "invalid_payout",
            ),
            (
                serde_json::json!({"name": "Carol", "payout": payout_body("nope")}),
                "invalid_address",
            ),
        ] {
            let (st, err) = call(app.clone(), signup_req(Some(SIGNUP_TOKEN), &body)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(err["code"], code);
        }

        let v = signed_up(app.clone()).await;
        let key = v["api_key"].as_str().unwrap();
        assert!(key.starts_with("pcc_sk_") && key.len() == 7 + 64);
        assert!(v["webhook_secret"].as_str().unwrap().starts_with("whsec_"));
        let (st, me) = call(app.clone(), get_with_key("/v1/me", key)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(me["name"], "Carol Shop");
        assert_eq!(me["id"], v["merchant_id"]);
        let (st, payout) = call(app.clone(), get_with_key("/v1/payout-config", key)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(payout["chain_id"], 42161);
        assert_eq!(
            payout["receiving_address"].as_str().unwrap().to_lowercase(),
            "0x4444444444444444444444444444444444444444"
        );

        let other = signed_up(app).await;
        assert_ne!(other["api_key"], v["api_key"], "keys are random per signup");
    }

    #[tokio::test]
    async fn signup_is_off_without_a_token() {
        let mut config = cfg();
        config.signup_token_hash = None;
        let app = router_with(config, Store::default(), TotpAuthorizer::new(b"test-master".to_vec()));
        let body =
            serde_json::json!({"name": "Carol", "payout": payout_body("0x4444444444444444444444444444444444444444")});
        let (st, err) = call(app, signup_req(Some(SIGNUP_TOKEN), &body)).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err["code"], "signup_disabled");
    }

    #[tokio::test]
    async fn rotating_the_api_key_revokes_the_old_one() {
        let app = router_with(cfg(), Store::default(), TotpAuthorizer::new(b"test-master".to_vec()));
        let v = signed_up(app.clone()).await;
        let old = v["api_key"].as_str().unwrap().to_owned();
        let (st, created) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/payment-intents",
                Some(&old),
                Some("idem-rot"),
                serde_json::json!({"merchant_order_id":"rot-1","output_amount":"1000000"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{created}");
        let (st, rotated) = call(
            app.clone(),
            json_req("POST", "/v1/api-keys/rotate", Some(&old), None, serde_json::json!({})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let new = rotated["api_key"].as_str().unwrap();
        assert_ne!(new, old);
        let (st, _) = call(app.clone(), get_with_key("/v1/me", &old)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "old key is revoked");
        let (st, list) = call(app.clone(), get_with_key("/v1/payment-intents", new)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list[0]["merchant_order_id"], "rot-1", "data stays with the merchant");
        let (st, _) = call(
            app,
            json_req("POST", "/v1/api-keys/rotate", Some(&old), None, serde_json::json!({})),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rotating_the_webhook_secret_changes_signatures() {
        let app = router_with(cfg(), alice_store(), TotpAuthorizer::new(b"test-master".to_vec()));
        let (st, _) = call(
            app.clone(),
            json_req(
                "PUT",
                "/v1/webhook-url",
                Some(KEY_A),
                None,
                serde_json::json!({"url":"https://hooks.example.com/pcc"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, created) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/payment-intents",
                Some(KEY_A),
                Some("idem-whrot"),
                serde_json::json!({"merchant_order_id":"o-rot","output_amount":"1000000"}),
            ),
        )
        .await;
        let events_uri = format!("/v1/payment-intents/{}/events", created["id"].as_str().unwrap());
        let (_, rotated) = call(
            app.clone(),
            json_req(
                "POST",
                "/v1/webhook-secret/rotate",
                Some(KEY_A),
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        let secret = rotated["webhook_secret"].as_str().unwrap();
        assert!(secret.starts_with("whsec_"));
        let (_, events) = call(app, get_with_key(&events_uri, KEY_A)).await;
        let payload = events[0]["payload"].as_str().unwrap();
        let signature = events[0]["signature"].as_str().unwrap();
        assert!(verify_signature(secret.as_bytes(), payload.as_bytes(), signature));
        assert!(!verify_signature(WH_A.as_bytes(), payload.as_bytes(), signature));
    }

    struct FakeCode;

    #[async_trait::async_trait]
    impl onboarding::CodeReader for FakeCode {
        async fn code(&self, chain: ChainId, _account: alloy_primitives::Address) -> Result<Vec<u8>, String> {
            if chain == ChainId::BASE {
                let mut c = vec![0xef, 0x01, 0x00];
                c.extend_from_slice(onboarding::KERNEL_7702_IMPLEMENTATION.as_slice());
                Ok(c)
            } else {
                Err("rpc down".into())
            }
        }
    }

    #[tokio::test]
    async fn delegation_status_reports_each_source_chain() {
        let get = |sid: &str| {
            Request::builder()
                .uri(format!("/v1/checkout/{sid}/delegation"))
                .body(Body::empty())
                .unwrap()
        };
        let mut state = app_state(cfg(), alice_store(), TotpAuthorizer::new(b"test-master".to_vec()), None);
        let (app0, sid0) = start_intent(app(state.clone())).await;
        siwe_as(app0.clone(), &sid0, &PrivateKeySigner::random()).await;
        let (st, err) = call(app0, get(&sid0)).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{err}");

        state.code_reader = Some(Arc::new(FakeCode));
        let (app, sid) = start_intent(app(state)).await;
        let (st, _) = call(app.clone(), get(&sid)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "SIWE first");
        let signer = PrivateKeySigner::random();
        siwe_as(app.clone(), &sid, &signer).await;
        let (st, v) = call(app, get(&sid)).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["address"], signer.address().to_string());
        assert_eq!(v["chains"][0]["chain_id"], 8453);
        assert_eq!(v["chains"][0]["ready"], true);
        assert_eq!(v["chains"][1]["chain_id"], 42161);
        assert_eq!(v["chains"][1]["state"], "unknown");
        assert_eq!(v["chains"][1]["ready"], false);
    }

    fn prepare_req(sid: &str, body: serde_json::Value) -> Request<Body> {
        json_req("POST", &format!("/v1/checkout/{sid}/session-grant"), None, None, body)
    }

    #[tokio::test]
    async fn prepare_bounds_cap_uses_and_days() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let (app, sid, _) = totp_ready(app).await;
        let (st, defaults) = call(app.clone(), prepare_req(&sid, serde_json::json!({}))).await;
        assert_eq!(st, StatusCode::OK, "{defaults}");
        assert_eq!(defaults["uses"], grant::DEFAULT_USES);
        let max_debit: u128 = defaults["max_amount"].as_str().unwrap().parse().unwrap();
        let week = u64::from(grant::DEFAULT_VALID_DAYS) * 24 * 3600;
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        let valid_until = defaults["valid_until"].as_u64().unwrap();
        assert!((now + week - 5..=now + week + 5).contains(&valid_until));

        for (body, code) in [
            (
                serde_json::json!({"max_amount": (max_debit - 1).to_string()}),
                "cap_below_payment",
            ),
            (serde_json::json!({"max_amount": "1e6"}), "invalid_amount"),
            (serde_json::json!({"uses": 0}), "invalid_allowance"),
            (serde_json::json!({"uses": 21}), "invalid_allowance"),
            (serde_json::json!({"valid_days": 0}), "invalid_allowance"),
            (serde_json::json!({"valid_days": 31}), "invalid_allowance"),
        ] {
            let (st, err) = call(app.clone(), prepare_req(&sid, body.clone())).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(err["code"], code, "{body}");
        }

        let cap = max_debit * 2;
        let (st, custom) = call(
            app,
            prepare_req(
                &sid,
                serde_json::json!({"max_amount": cap.to_string(), "uses": 3, "valid_days": 2}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{custom}");
        assert_eq!(custom["uses"], 3);
        assert_eq!(custom["max_amount"], cap.to_string());
        assert_eq!(custom["total_exposure"], (cap * 3).to_string());
    }

    #[tokio::test]
    async fn payer_can_see_and_revoke_the_allowance() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let (app, sid, code) = totp_ready(app).await;
        let status = || get_req(&format!("/v1/checkout/{sid}/session-grant"));
        let revoke = || {
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/session-grant/revoke"),
                None,
                None,
                serde_json::json!({}),
            )
        };

        let (st, err) = call(app.clone(), status()).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(err["code"], "no_allowance");
        enable_grant(app.clone(), &sid).await;
        let (st, view) = call(app.clone(), status()).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(view["covers_this_payment"], true);
        assert_eq!(view["chain_id"], 8453);

        let (st, out) = call(app.clone(), revoke()).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(out["revoked"], true);
        let (st, _) = call(app.clone(), status()).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (_, out) = call(app.clone(), revoke()).await;
        assert_eq!(out["revoked"], false);
        let (st, err) = call(app, authorize_req(&sid, &code)).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "session_grant_required");
    }

    #[tokio::test]
    async fn each_sent_payment_spends_one_use() {
        let (state, mock) = live_state(operator::ApprovalMode::Payer);
        let app = app(state);
        let signer = PrivateKeySigner::random();

        let (app, first) = start_intent(app).await;
        siwe_as(app.clone(), &first, &signer).await;
        let (st, enroll) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{first}/totp/enroll"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let uri = enroll["otpauth_uri"].as_str().unwrap();
        let secret = uri.split("secret=").nth(1).unwrap().split('&').next().unwrap();
        let bytes = pcc_auth::totp::decode_secret(secret).unwrap();
        let code_at = |offset: u64| {
            let now = u64::try_from(Utc::now().timestamp()).unwrap();
            format!("{:06}", pcc_auth::totp::code_at(&bytes, now + offset))
        };
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{first}/totp/confirm"),
                None,
                None,
                serde_json::json!({"code": code_at(0)}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, prepared) = call(app.clone(), prepare_req(&first, serde_json::json!({"uses": 2}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{first}/session-grant/enable"),
                None,
                None,
                serde_json::json!({"grant_id": prepared["grant_id"], "signature": MOCK_SIG}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        let (st, auth) = call(app.clone(), authorize_req(&first, &code_at(0))).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        assert_eq!(auth["execution"], "broadcast");
        let (_, left) = call(app.clone(), get_req(&format!("/v1/checkout/{first}/session-grant"))).await;
        assert_eq!(left["uses_remaining"], 1);

        let (app, second) = start_intent(app).await;
        siwe_as(app.clone(), &second, &signer).await;
        let (st, auth) = call(app.clone(), authorize_req(&second, &code_at(30))).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        assert_eq!(auth["execution"], "broadcast");
        let (st, _) = call(app.clone(), get_req(&format!("/v1/checkout/{second}/session-grant"))).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "exhausted");

        let (app, third) = start_intent(app).await;
        siwe_as(app.clone(), &third, &signer).await;
        let (st, err) = call(app, authorize_req(&third, &code_at(30))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "session_grant_required");
        assert_eq!(mock.grants.enabled.lock().unwrap().len(), 1, "signed once");
    }

    /// Mock wallets sign with any well-formed 65-byte signature.
    const MOCK_SIG: &str = "0x1111111111111111111111111111111111111111111111111111111111111111\
                            2222222222222222222222222222222222222222222222222222222222222222\
                            1b";

    async fn enable_grant(app: Router, sid: &str) -> serde_json::Value {
        let (st, prepared) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/session-grant"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{prepared}");
        let (st, enabled) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/session-grant/enable"),
                None,
                None,
                serde_json::json!({"grant_id": prepared["grant_id"], "signature": MOCK_SIG}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{enabled}");
        prepared
    }

    #[tokio::test]
    async fn authorize_without_siwe_is_rejected() {
        let (app, sid) = start_intent(seeded()).await;
        let (st, err) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/authorize"),
                None,
                None,
                serde_json::json!({"code": "123456"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(err["code"], "siwe_required");
    }

    /// Enrolls the signed-in wallet; returns its TOTP secret.
    async fn enroll_wallet(app: Router, sid: &str) -> Vec<u8> {
        let (st, enroll) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/enroll"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{enroll}");
        let uri = enroll["otpauth_uri"].as_str().unwrap();
        let secret = uri.split("secret=").nth(1).unwrap().split('&').next().unwrap();
        let bytes = pcc_auth::totp::decode_secret(secret).unwrap();
        let (st, _) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/totp/confirm"),
                None,
                None,
                serde_json::json!({"code": totp_at(&bytes, 0)}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        bytes
    }

    fn totp_at(secret: &[u8], offset: u64) -> String {
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        format!("{:06}", pcc_auth::totp::code_at(secret, now + offset))
    }

    fn charge_req(mandate: &str, idem: &str, output: &str) -> Request<Body> {
        json_req(
            "POST",
            &format!("/v1/autopay/{mandate}/charges"),
            Some(KEY_A),
            Some(idem),
            serde_json::json!({"merchant_order_id": "sub", "output_amount": output}),
        )
    }

    /// Signed-in, enrolled payer with an active mandate; returns the checkout session and mandate id.
    async fn active_mandate(app: Router, signer: &PrivateKeySigner, terms: serde_json::Value) -> (String, String) {
        let (app, sid) = start_intent(app).await;
        siwe_as(app.clone(), &sid, signer).await;
        let secret = enroll_wallet(app.clone(), &sid).await;
        let mut body = terms;
        body["code"] = serde_json::json!(totp_at(&secret, 30));
        let (st, setup) = call(
            app.clone(),
            json_req("POST", &format!("/v1/checkout/{sid}/autopay"), None, None, body),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{setup}");
        let mandate = setup["mandate_id"].as_str().unwrap().to_owned();
        let (st, enabled) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/autopay/{mandate}/enable"),
                None,
                None,
                serde_json::json!({"grant_id": setup["grant_id"], "signature": MOCK_SIG}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{enabled}");
        assert_eq!(enabled["status"], "active");
        (sid, mandate)
    }

    #[tokio::test]
    async fn autopay_setup_needs_enrollment_a_fresh_code_and_one_signature() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let signer = PrivateKeySigner::random();
        let (app, sid) = start_intent(app).await;
        siwe_as(app.clone(), &sid, &signer).await;
        let setup =
            |body: serde_json::Value| json_req("POST", &format!("/v1/checkout/{sid}/autopay"), None, None, body);

        let (st, err) = call(app.clone(), setup(serde_json::json!({"code": "123456"}))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "totp_enrollment_required");
        let secret = enroll_wallet(app.clone(), &sid).await;
        let (st, err) = call(app.clone(), setup(serde_json::json!({"code": "000000"}))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(err["code"], "totp_invalid");
        for bad in [
            serde_json::json!({"charges": 0}),
            serde_json::json!({"charges": 21}),
            serde_json::json!({"interval_hours": 745}),
            serde_json::json!({"valid_days": 31}),
        ] {
            let mut body = bad.clone();
            body["code"] = serde_json::json!("123456");
            let (st, err) = call(app.clone(), setup(body)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(err["code"], "invalid_mandate", "{bad}");
        }

        let (st, made) = call(
            app.clone(),
            setup(serde_json::json!({"code": totp_at(&secret, 30), "charges": 3, "interval_hours": 0})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{made}");
        let cap: u128 = made["max_amount"].as_str().unwrap().parse().unwrap();
        assert_eq!(made["total_exposure"], (cap * 3).to_string());
        let mandate = made["mandate_id"].as_str().unwrap();

        let (_, merchant_list) = call(
            app.clone(),
            json_req("GET", "/v1/autopay", Some(KEY_A), None, serde_json::json!({})),
        )
        .await;
        assert_eq!(
            merchant_list,
            serde_json::json!([]),
            "unsigned mandates are not chargeable or listed"
        );
        let (st, err) = call(app.clone(), charge_req(mandate, "c-0", "500000")).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "mandate_inactive");

        let enable = |grant: &serde_json::Value| {
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/autopay/{mandate}/enable"),
                None,
                None,
                serde_json::json!({"grant_id": grant, "signature": MOCK_SIG}),
            )
        };
        let (st, err) = call(app.clone(), enable(&serde_json::json!("grant-999"))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "grant_mismatch");
        let (st, view) = call(app.clone(), enable(&made["grant_id"])).await;
        assert_eq!(st, StatusCode::OK, "{view}");
        assert_eq!(view["charges_remaining"], 3);

        // The mandate's key is separate: the checkout itself still needs its own allowance.
        let (st, err) = call(app, authorize_req(&sid, &totp_at(&secret, 60))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "session_grant_required");
    }

    #[tokio::test]
    async fn merchant_charges_a_mandate_without_the_payer() {
        let (state, mock) = live_state(operator::ApprovalMode::Payer);
        let app = app(state);
        let signer = PrivateKeySigner::random();
        let (_, mandate) = active_mandate(
            app.clone(),
            &signer,
            serde_json::json!({"charges": 2, "interval_hours": 0, "valid_days": 7}),
        )
        .await;

        let (st, _) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/autopay/{mandate}/charges"),
                None,
                Some("c-1"),
                serde_json::json!({"merchant_order_id": "sub", "output_amount": "500000"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "merchant key required");
        let (st, err) = call(app.clone(), charge_req(&mandate, "c-big", "5000000")).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "over_mandate_cap");

        let (st, first) = call(app.clone(), charge_req(&mandate, "c-1", "500000")).await;
        assert_eq!(st, StatusCode::OK, "{first}");
        assert_eq!(first["execution"], "broadcast");
        assert_eq!(first["charges_remaining"], 1);
        let sent = mock.executor.last_plan.lock().unwrap().clone().unwrap();
        assert_eq!(sent.mandate.unwrap().to_string(), mandate);
        assert_eq!(sent.from, signer.address());
        assert!(sent.approval_ref.starts_with(&format!("autopay:{mandate}:")));

        let (st, replay) = call(app.clone(), charge_req(&mandate, "c-1", "500000")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(replay, first, "an idempotent replay never charges twice");

        let (st, second) = call(app.clone(), charge_req(&mandate, "c-2", "500000")).await;
        assert_eq!(st, StatusCode::OK, "{second}");
        assert_eq!(second["charges_remaining"], 0);
        assert_ne!(second["intent_id"], first["intent_id"]);
        let (st, err) = call(app.clone(), charge_req(&mandate, "c-3", "500000")).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "mandate_exhausted");

        let (_, listed) = call(
            app,
            json_req("GET", "/v1/autopay", Some(KEY_A), None, serde_json::json!({})),
        )
        .await;
        assert_eq!(listed[0]["status"], "exhausted");
    }

    #[tokio::test]
    async fn mandate_interval_and_payer_revoke_stop_charges() {
        let (state, mock) = live_state(operator::ApprovalMode::Owner);
        let app = app(state);
        let signer = PrivateKeySigner::random();
        let (sid, mandate) = active_mandate(
            app.clone(),
            &signer,
            serde_json::json!({"charges": 5, "interval_hours": 24}),
        )
        .await;

        let (st, first) = call(app.clone(), charge_req(&mandate, "i-1", "500000")).await;
        assert_eq!(st, StatusCode::OK, "{first}");
        assert_eq!(
            first["execution"], "awaiting_approval",
            "owner mode still parks autopay sends"
        );
        assert!(mock.executor.last_plan.lock().unwrap().is_none());
        let (st, err) = call(app.clone(), charge_req(&mandate, "i-2", "500000")).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err["code"], "mandate_interval");

        let (st, listed) = call(app.clone(), get_req(&format!("/v1/checkout/{sid}/autopay"))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(listed[0]["charges_remaining"], 4);
        let (st, revoked) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{sid}/autopay/{mandate}/revoke"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(revoked["status"], "revoked");
        let key = pcc_payments::traits::AllowanceKey {
            owner: signer.address(),
            chain_id: ChainId::BASE,
            token: match AssetId::usdc_base().token {
                TokenRef::Erc20 { address } => address,
                TokenRef::Native => unreachable!(),
            },
            mandate: Some(mandate.parse().unwrap()),
        };
        assert!(
            pcc_payments::traits::SessionGrantPort::allowance(&mock.grants, &key)
                .await
                .unwrap()
                .is_none(),
            "session key destroyed"
        );
        let (st, err) = call(app.clone(), charge_req(&mandate, "i-3", "500000")).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["code"], "mandate_inactive");

        let other = PrivateKeySigner::random();
        let (app, stranger) = start_intent(app).await;
        siwe_as(app.clone(), &stranger, &other).await;
        let (st, _) = call(
            app,
            json_req(
                "POST",
                &format!("/v1/checkout/{stranger}/autopay/{mandate}/revoke"),
                None,
                None,
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "only the payer who signed can revoke");
    }

    fn resume_req(sid: &str, token: &str) -> Request<Body> {
        json_req(
            "POST",
            &format!("/v1/checkout/{sid}/device/resume"),
            None,
            None,
            serde_json::json!({"device_token": token}),
        )
    }

    #[tokio::test]
    async fn remembered_device_skips_wallet_sign_in_but_not_totp() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let signer = PrivateKeySigner::random();
        let (app, first) = start_intent(app).await;
        siwe_as(app.clone(), &first, &signer).await;
        let secret = enroll_wallet(app.clone(), &first).await;
        enable_grant(app.clone(), &first).await;
        let (st, auth) = call(
            app.clone(),
            json_req(
                "POST",
                &format!("/v1/checkout/{first}/authorize"),
                None,
                None,
                serde_json::json!({"code": totp_at(&secret, 30), "remember_device": true}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        let token = auth["device_token"].as_str().unwrap().to_owned();
        assert!(token.starts_with("pcc_dev_") && token.len() == 8 + 64);

        let (app, third) = start_intent(app).await;
        for bad in ["pcc_dev_00", "nope", &token[..token.len() - 1]] {
            let (st, err) = call(app.clone(), resume_req(&third, bad)).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
            assert_eq!(err["code"], "device_unknown");
        }
        let (st, resumed) = call(app.clone(), resume_req(&third, &token)).await;
        assert_eq!(st, StatusCode::OK, "{resumed}");
        assert_eq!(resumed["address"], signer.address().to_string());
        assert_eq!(resumed["totp_enrolled"], true);
        let (st, err) = call(app.clone(), authorize_req(&third, "000000")).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "a remembered device still needs the code");
        assert_eq!(err["code"], "totp_invalid");
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        let previous = format!("{:06}", pcc_auth::totp::code_at(&secret, now - 30));
        let (st, auth) = call(app.clone(), authorize_req(&third, &previous)).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        assert!(auth["device_token"].is_null(), "only issued on request");

        let forget = || {
            json_req(
                "POST",
                "/v1/devices/forget",
                None,
                None,
                serde_json::json!({"device_token": token}),
            )
        };
        let (_, out) = call(app.clone(), forget()).await;
        assert_eq!(out["forgotten"], true);
        let (_, out) = call(app.clone(), forget()).await;
        assert_eq!(out["forgotten"], false);
        let (app, fourth) = start_intent(app).await;
        let (st, _) = call(app, resume_req(&fourth, &token)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn device_tokens_expire_and_are_stored_hashed() {
        let store = Store::default();
        let owner = alloy_primitives::Address::repeat_byte(0xaa);
        let wallet = store.credential_for_owner(owner);
        let now = Utc::now();
        let token = store.issue_device(wallet, owner, now);
        assert_eq!(store.device(&token, now), Some((wallet, owner)));
        assert_ne!(
            store.issue_device(wallet, owner, now),
            token,
            "fresh randomness per issue"
        );
        assert!(store
            .device(&token, now + chrono::Duration::days(store::DEVICE_TTL_DAYS))
            .is_none());
        assert!(store.device(&token, now).is_none(), "expired tokens are dropped");
    }

    #[tokio::test]
    async fn a_second_wallet_never_inherits_the_first_wallets_attempt() {
        let app = router_with_ports(
            cfg(),
            alice_store(),
            TotpAuthorizer::new(b"test-master".to_vec()),
            Some(Arc::new(pcc_payments::mock::MockCheckout::default())),
        );
        let (first, second) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (app, sid) = start_intent(app).await;
        siwe_as(app.clone(), &sid, &first).await;
        let secret = enroll_wallet(app.clone(), &sid).await;
        enable_grant(app.clone(), &sid).await;
        let (_, before) = call(app.clone(), get_req(&format!("/v1/checkout/{sid}"))).await;

        let after = siwe_as(app.clone(), &sid, &second).await;
        assert_ne!(
            after["attempt_id"], before["attempt_id"],
            "fresh attempt for the new wallet"
        );
        assert_eq!(after["totp_enrolled"], false);
        let (st, _) = call(app.clone(), get_req(&format!("/v1/checkout/{sid}/session-grant"))).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "first wallet's allowance is not visible");

        siwe_as(app.clone(), &sid, &first).await;
        let (st, auth) = call(app.clone(), authorize_req(&sid, &totp_at(&secret, 30))).await;
        assert_eq!(st, StatusCode::OK, "{auth}");
        let err = siwe_as(app.clone(), &sid, &second).await;
        assert_eq!(err["code"], "session_wallet_locked");
    }
}
