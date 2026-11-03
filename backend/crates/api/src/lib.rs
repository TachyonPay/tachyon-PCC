//! Axum application. M0/M1 exposes health and gate status only; merchant and
//! payment endpoints are added in M6 on top of the same router.
//!
//! The public merchant/checkout API contract is Rust-owned: `PublicApiDoc`
//! is exported to `openapi/public-v1.json` and the frontend `@pcc/api-types`
//! package is generated from it (same pattern as the internal contract).

use axum::{extract::State, routing::get, Json, Router};
use pcc_payments::ExecutionGates;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

/// Public API semantic version. Bump on breaking changes.
pub const PUBLIC_API_VERSION: &str = "0.1.0";

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub gates: ExecutionGates,
    pub integrations_base_url: String,
}

impl Config {
    /// Load from environment. Binds to loopback unless `API_BIND_ADDR` is set.
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = std::env::var("API_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
            .parse()?;
        Ok(Self {
            bind_addr,
            gates: ExecutionGates::from_env(),
            integrations_base_url: std::env::var("INTEGRATIONS_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned()),
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

/// Liveness response.
#[derive(Serialize, ToSchema)]
pub struct Health {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
}

/// Execution-gate status. Read-only; gates can only be changed via environment.
#[derive(Serialize, ToSchema)]
pub struct GatesView {
    pub dry_run: bool,
    pub enable_mainnet_transfers: bool,
    pub mock_authenticator_allowed: bool,
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

/// Rust-owned public OpenAPI document (merchant + checkout surface).
#[derive(OpenApi)]
#[openapi(
    info(title = "Private Crypto Checkout — public API", version = PUBLIC_API_VERSION,
         description = "Merchant and checkout API. M0/M1: meta endpoints only; payment-intent endpoints land in M6."),
    paths(health, gates),
    components(schemas(Health, GatesView)),
    tags((name = "meta", description = "Liveness and execution gates"))
)]
pub struct PublicApiDoc;

impl PublicApiDoc {
    pub fn to_pretty_json() -> String {
        Self::openapi().to_pretty_json().expect("openapi serialises")
    }
}

pub fn router(config: Config) -> Router {
    let state = AppState {
        config: Arc::new(config),
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/meta/gates", get(gates))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn cfg() -> Config {
        Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            gates: ExecutionGates::default(),
            integrations_base_url: "http://127.0.0.1:8787".into(),
        }
    }

    #[tokio::test]
    async fn health_ok() {
        let app = router(cfg());
        let res = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn gates_default_to_safe_values() {
        let app = router(cfg());
        let res = app
            .oneshot(Request::builder().uri("/v1/meta/gates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["enable_mainnet_transfers"], false);
    }

    /// Drift guard: the committed public OpenAPI must match the Rust types.
    /// Regenerate with `cargo run -p pcc-api --bin export-public-openapi`.
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
}
