//! HTTP client for the private TS integration worker. Implements
//! `pcc_payments::traits::OneClickAdapter` over the v1 internal contract.

use crate::contract::{
    AckResponse, AssetsResponse, DepositSubmittedDto, ErrorCode, ErrorResponse, HealthResponse, QuoteRequestDto,
    SwapStatusDto, VerifiedQuoteDto, VerifyRequestDto, BASE_PATH,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcc_payments::asset::AssetId;
use pcc_payments::quote::VerifiedQuote;
use pcc_payments::traits::{AdapterError, OneClickAdapter, QuoteSpec, SupportedAsset, SwapStatus};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct IntegrationClientConfig {
    /// e.g. `http://127.0.0.1:8787`. Must be loopback or a private address.
    pub base_url: String,
    /// Shared bearer token (reference resolved by the caller; never logged).
    pub bearer_token: String,
    pub timeout: Duration,
}

impl IntegrationClientConfig {
    /// Refuse to talk to anything that is not loopback / RFC1918 / `.internal`.
    pub fn validate(&self) -> Result<(), AdapterError> {
        let url = reqwest::Url::parse(&self.base_url).map_err(|e| AdapterError::Other(format!("bad base_url: {e}")))?;
        let host = url.host_str().unwrap_or_default();
        let private = host == "localhost"
            || host.ends_with(".internal")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback() || is_private_ip(ip));
        if !private {
            return Err(AdapterError::Other(format!(
                "integration worker host {host:?} is not loopback/private; refusing"
            )));
        }
        if self.bearer_token.trim().is_empty() {
            return Err(AdapterError::Other("empty internal bearer token".into()));
        }
        Ok(())
    }
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

#[derive(Clone, Debug)]
pub struct IntegrationClient {
    http: reqwest::Client,
    cfg: IntegrationClientConfig,
}

impl IntegrationClient {
    pub fn new(cfg: IntegrationClientConfig) -> Result<Self, AdapterError> {
        cfg.validate()?;
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .connect_timeout(cfg.timeout.min(Duration::from_secs(5)))
            .no_proxy()
            .build()
            .map_err(|e| AdapterError::Other(e.to_string()))?;
        Ok(Self { http, cfg })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.cfg.base_url.trim_end_matches('/'), BASE_PATH, path)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&impl serde::Serialize>,
    ) -> Result<T, AdapterError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut req = self
            .http
            .request(method, self.url(path))
            .bearer_auth(&self.cfg.bearer_token)
            .header("X-Request-Id", &request_id)
            .header("Accept", "application/json")
            .query(query);
        if let Some(b) = body {
            req = req.json(b);
        }
        tracing::debug!(path, request_id, "integration worker call");

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                AdapterError::Timeout
            } else if e.is_connect() {
                AdapterError::Unavailable(format!("connect: {e}"))
            } else {
                AdapterError::Unavailable(e.to_string())
            }
        })?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| AdapterError::Malformed(e.to_string()))?;

        if status.is_success() {
            return serde_json::from_str::<T>(&text).map_err(|e| AdapterError::Schema(format!("{path}: {e}")));
        }

        // Prefer the structured error body; fall back to status mapping.
        if let Ok(err) = serde_json::from_str::<ErrorResponse>(&text) {
            return Err(map_error_code(err.error.code, &err.error.message));
        }
        Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => AdapterError::Unauthorized,
            StatusCode::TOO_MANY_REQUESTS => AdapterError::RateLimited,
            StatusCode::GATEWAY_TIMEOUT => AdapterError::Timeout,
            s if s.is_server_error() => AdapterError::Unavailable(format!("worker returned {s}")),
            s => AdapterError::Malformed(format!("worker returned {s} with non-contract body")),
        })
    }

    pub async fn health(&self) -> Result<HealthResponse, AdapterError> {
        self.call(Method::GET, "/health", &[], None::<&()>).await
    }
}

fn map_error_code(code: ErrorCode, message: &str) -> AdapterError {
    match code {
        ErrorCode::Unauthorized | ErrorCode::UpstreamUnauthorized => AdapterError::Unauthorized,
        ErrorCode::InvalidRequest => AdapterError::Rejected(format!("invalid request: {message}")),
        ErrorCode::AuthRefReused => AdapterError::Rejected(format!("authorization ref reused: {message}")),
        ErrorCode::UpstreamTimeout => AdapterError::Timeout,
        ErrorCode::UpstreamRateLimited => AdapterError::RateLimited,
        ErrorCode::UpstreamRejected => AdapterError::Rejected(message.to_owned()),
        ErrorCode::UpstreamUnavailable => AdapterError::Unavailable(message.to_owned()),
        ErrorCode::MalformedUpstream | ErrorCode::SignatureInvalid => AdapterError::Malformed(message.to_owned()),
        ErrorCode::NoRoute => AdapterError::NoRoute(message.to_owned()),
        ErrorCode::Internal => AdapterError::Other(format!("worker internal error: {message}")),
    }
}

fn parse_ts(s: &str, field: &str) -> Result<DateTime<Utc>, AdapterError> {
    s.parse::<DateTime<Utc>>()
        .map_err(|e| AdapterError::Schema(format!("{field}: bad timestamp {s:?}: {e}")))
}

fn quote_from_dto(d: VerifiedQuoteDto) -> Result<VerifiedQuote, AdapterError> {
    Ok(VerifiedQuote {
        correlation_id: d.correlation_id,
        signature: d.signature,
        signature_verified: d.signature_verified,
        dry: d.dry,
        swap_type: d.swap_type,
        confidentiality: d.confidentiality,
        deposit_mode: d.deposit_mode,
        origin_asset_id: d.origin_asset_id,
        destination_asset_id: d.destination_asset_id,
        recipient: d.recipient,
        refund_to: d.refund_to,
        amount_in: d.amount_in,
        min_amount_in: d.min_amount_in,
        amount_out: d.amount_out,
        min_amount_out: d.min_amount_out,
        deposit_address: d.deposit_address,
        deposit_memo: d.deposit_memo,
        quote_timestamp: parse_ts(&d.quote_timestamp, "quoteTimestamp")?,
        deadline: d.deadline.as_deref().map(|s| parse_ts(s, "deadline")).transpose()?,
        time_estimate_seconds: d.time_estimate_seconds,
        raw_response_json: d.raw_response_json,
    })
}

fn quote_request_dto(spec: &QuoteSpec, dry: bool) -> QuoteRequestDto {
    QuoteRequestDto {
        dry,
        authorization_ref: spec.authorization_ref.clone(),
        origin_asset_id: spec.source_asset.one_click_asset_id.clone(),
        destination_asset_id: spec.destination_asset.one_click_asset_id.clone(),
        amount_out: spec.output_amount,
        recipient: spec.recipient.to_string(),
        refund_to: spec.refund_to.to_string(),
        confidentiality: spec.confidentiality,
        slippage_bps: spec.slippage_bps,
        deadline: spec.deadline.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }
}

#[async_trait]
impl OneClickAdapter for IntegrationClient {
    async fn assets(&self) -> Result<Vec<SupportedAsset>, AdapterError> {
        let resp: AssetsResponse = self.call(Method::GET, "/near/assets", &[], None::<&()>).await?;
        Ok(resp
            .tokens
            .into_iter()
            .filter_map(|t| {
                AssetId::from_one_click_token(&t.blockchain, t.contract_address.as_deref(), t.decimals, &t.asset_id)
                    .map(|asset| SupportedAsset {
                        asset,
                        symbol: t.symbol,
                    })
            })
            .collect())
    }

    async fn preview(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        let dto: VerifiedQuoteDto = self
            .call(Method::POST, "/near/quote", &[], Some(&quote_request_dto(spec, true)))
            .await?;
        quote_from_dto(dto)
    }

    async fn executable_quote(&self, spec: &QuoteSpec) -> Result<VerifiedQuote, AdapterError> {
        let dto: VerifiedQuoteDto = self
            .call(Method::POST, "/near/quote", &[], Some(&quote_request_dto(spec, false)))
            .await?;
        quote_from_dto(dto)
    }

    async fn verify(&self, raw_response_json: &str) -> Result<VerifiedQuote, AdapterError> {
        let body = VerifyRequestDto {
            raw_response_json: raw_response_json.to_owned(),
        };
        let dto: VerifiedQuoteDto = self.call(Method::POST, "/near/verify", &[], Some(&body)).await?;
        quote_from_dto(dto)
    }

    async fn status(&self, deposit_address: &str, deposit_memo: Option<&str>) -> Result<SwapStatus, AdapterError> {
        let mut q = vec![("depositAddress", deposit_address)];
        if let Some(m) = deposit_memo {
            q.push(("depositMemo", m));
        }
        let dto: SwapStatusDto = self.call(Method::GET, "/near/status", &q, None::<&()>).await?;
        Ok(SwapStatus {
            status: dto.status,
            updated_at: parse_ts(&dto.updated_at, "updatedAt")?,
            origin_tx_hashes: dto.origin_tx_hashes,
            destination_tx_hashes: dto.destination_tx_hashes,
            amount_in: dto.amount_in,
            amount_out: dto.amount_out,
            refunded_amount: dto.refunded_amount,
        })
    }

    async fn submit_deposit_tx(&self, deposit_address: &str, tx_hash: &str) -> Result<(), AdapterError> {
        let body = DepositSubmittedDto {
            deposit_address: deposit_address.to_owned(),
            tx_hash: tx_hash.to_owned(),
        };
        let ack: AckResponse = self
            .call(Method::POST, "/near/deposit-submitted", &[], Some(&body))
            .await?;
        if ack.ok {
            Ok(())
        } else {
            Err(AdapterError::Rejected("worker did not acknowledge deposit".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_payments::amount::BaseUnits;
    use pcc_payments::asset::{ChainId, TokenRef};
    use pcc_payments::quote::Confidentiality;
    use pcc_payments::status::OneClickStatus;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, header_exists, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer, timeout_ms: u64) -> IntegrationClient {
        IntegrationClient::new(IntegrationClientConfig {
            base_url: server.uri(),
            bearer_token: "test-token".into(),
            timeout: Duration::from_millis(timeout_ms),
        })
        .unwrap()
    }

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
            deadline: "2026-11-03T14:00:00Z".parse().unwrap(),
            authorization_ref: "auth-ref-1".into(),
        }
    }

    fn good_quote_json(dry: bool) -> serde_json::Value {
        let mut v = json!({
            "correlationId": "c1",
            "signature": "ed25519:abc",
            "signatureVerified": true,
            "dry": dry,
            "swapType": "EXACT_OUTPUT",
            "confidentiality": "basic",
            "depositMode": "SIMPLE",
            "originAssetId": "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
            "destinationAssetId": "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
            "recipient": "0x1111111111111111111111111111111111111111",
            "refundTo": "0x2222222222222222222222222222222222222222",
            "amountIn": "1020000",
            "minAmountIn": "1012000",
            "amountOut": "1000000",
            "minAmountOut": "1000000",
            "quoteTimestamp": "2026-11-03T12:00:00.000Z",
            "timeEstimateSeconds": 120,
            "rawResponseJson": "{}"
        });
        if !dry {
            v["depositAddress"] = json!("0x76b4c56085ED136a8744D52bE956396624a730E8");
            v["deadline"] = json!("2026-11-03T14:00:00.000Z");
        }
        v
    }

    #[test]
    fn refuses_public_hosts_and_empty_tokens() {
        let bad = IntegrationClientConfig {
            base_url: "https://example.com".into(),
            bearer_token: "t".into(),
            timeout: Duration::from_secs(1),
        };
        assert!(IntegrationClient::new(bad).is_err());
        let empty = IntegrationClientConfig {
            base_url: "http://127.0.0.1:8787".into(),
            bearer_token: " ".into(),
            timeout: Duration::from_secs(1),
        };
        assert!(IntegrationClient::new(empty).is_err());
        for ok in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://10.0.0.5:8787",
            "http://worker.internal",
        ] {
            assert!(
                IntegrationClient::new(IntegrationClientConfig {
                    base_url: ok.into(),
                    bearer_token: "t".into(),
                    timeout: Duration::from_secs(1),
                })
                .is_ok(),
                "{ok}"
            );
        }
    }

    #[tokio::test]
    async fn sends_bearer_request_id_and_contract_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .and(header("Authorization", "Bearer test-token"))
            .and(header_exists("X-Request-Id"))
            .and(body_partial_json(json!({
                "dry": true,
                "authorizationRef": "auth-ref-1",
                "amountOut": "1000000",
                "confidentiality": "basic",
                "slippageBps": 100
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(good_quote_json(true)))
            .expect(1)
            .mount(&server)
            .await;
        let q = client(&server, 2000).preview(&spec()).await.unwrap();
        assert!(q.dry);
        assert!(q.signature_verified);
        assert_eq!(q.amount_in, BaseUnits::from_u128(1_020_000));
        assert!(q.deposit_address.is_none());
    }

    #[tokio::test]
    async fn executable_quote_parses_deposit_and_deadline() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .and(body_partial_json(json!({ "dry": false })))
            .respond_with(ResponseTemplate::new(200).set_body_json(good_quote_json(false)))
            .mount(&server)
            .await;
        let q = client(&server, 2000).executable_quote(&spec()).await.unwrap();
        assert!(!q.dry);
        assert_eq!(
            q.deposit_address.as_deref(),
            Some("0x76b4c56085ED136a8744D52bE956396624a730E8")
        );
        assert!(q.deadline.is_some());
    }

    #[tokio::test]
    async fn malformed_json_is_a_schema_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"correlationId\": \"c1\", \"amountIn\": 12"))
            .mount(&server)
            .await;
        let err = client(&server, 2000).preview(&spec()).await.unwrap_err();
        assert!(matches!(err, AdapterError::Schema(_)), "{err:?}");
    }

    #[tokio::test]
    async fn numeric_amount_instead_of_string_is_rejected() {
        let server = MockServer::start().await;
        let mut body = good_quote_json(true);
        body["amountIn"] = json!(1_020_000);
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let err = client(&server, 2000).preview(&spec()).await.unwrap_err();
        assert!(matches!(err, AdapterError::Schema(_)), "{err:?}");
    }

    #[tokio::test]
    async fn missing_required_field_is_rejected() {
        let server = MockServer::start().await;
        let mut body = good_quote_json(true);
        body.as_object_mut().unwrap().remove("signatureVerified");
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let err = client(&server, 2000).preview(&spec()).await.unwrap_err();
        assert!(matches!(err, AdapterError::Schema(_)), "{err:?}");
    }

    #[tokio::test]
    async fn timeout_maps_to_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/near/quote"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(good_quote_json(true))
                    .set_delay(Duration::from_millis(800)),
            )
            .mount(&server)
            .await;
        let err = client(&server, 200).preview(&spec()).await.unwrap_err();
        assert!(matches!(err, AdapterError::Timeout), "{err:?}");
    }

    #[tokio::test]
    async fn structured_error_codes_map_to_adapter_errors() {
        let cases = [
            (401, "UNAUTHORIZED", "unauthorized"),
            (409, "AUTH_REF_REUSED", "rejected"),
            (422, "NO_ROUTE", "no_route"),
            (422, "SIGNATURE_INVALID", "malformed"),
            (502, "UPSTREAM_UNAVAILABLE", "unavailable"),
            (504, "UPSTREAM_TIMEOUT", "timeout"),
            (429, "UPSTREAM_RATE_LIMITED", "rate_limited"),
            (401, "UPSTREAM_UNAUTHORIZED", "unauthorized"),
        ];
        for (status, code, expected) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/internal/v1/near/quote"))
                .respond_with(
                    ResponseTemplate::new(status).set_body_json(json!({ "error": { "code": code, "message": "m" } })),
                )
                .mount(&server)
                .await;
            let err = client(&server, 2000).preview(&spec()).await.unwrap_err();
            let got = match err {
                AdapterError::Unauthorized => "unauthorized",
                AdapterError::Rejected(_) => "rejected",
                AdapterError::NoRoute(_) => "no_route",
                AdapterError::Malformed(_) => "malformed",
                AdapterError::Unavailable(_) => "unavailable",
                AdapterError::Timeout => "timeout",
                AdapterError::RateLimited => "rate_limited",
                other => panic!("{code}: unexpected {other:?}"),
            };
            assert_eq!(got, expected, "code {code}");
        }
    }

    #[tokio::test]
    async fn non_contract_error_body_falls_back_to_status_mapping() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/near/assets"))
            .respond_with(ResponseTemplate::new(500).set_body_string("<html>oops</html>"))
            .mount(&server)
            .await;
        let err = client(&server, 2000).assets().await.unwrap_err();
        assert!(matches!(err, AdapterError::Unavailable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn connection_refused_is_unavailable() {
        // Reserve a loopback port, then release it so nothing is listening there.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let c = IntegrationClient::new(IntegrationClientConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            bearer_token: "t".into(),
            timeout: Duration::from_millis(500),
        })
        .unwrap();
        let err = c.assets().await.unwrap_err();
        assert!(
            matches!(err, AdapterError::Unavailable(_) | AdapterError::Timeout),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn assets_filters_to_mapped_evm_chains() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/near/assets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "fetchedAt": "2026-11-03T12:00:00Z",
                "tokens": [
                    { "assetId": "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near", "blockchain": "base",
                      "symbol": "USDC", "decimals": 6, "contractAddress": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
                      "priceUsd": "1", "priceUpdatedAt": "2026-11-03T12:00:00Z" },
                    { "assetId": "nep141:sol.omft.near", "blockchain": "sol", "symbol": "SOL", "decimals": 9,
                      "priceUsd": "100", "priceUpdatedAt": "2026-11-03T12:00:00Z" },
                    { "assetId": "nep141:eth.omft.near", "blockchain": "eth", "symbol": "ETH", "decimals": 18,
                      "priceUsd": "3000", "priceUpdatedAt": "2026-11-03T12:00:00Z" }
                ]
            })))
            .mount(&server)
            .await;
        let assets = client(&server, 2000).assets().await.unwrap();
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].asset.chain_id, ChainId::BASE);
        assert_eq!(assets[1].asset.chain_id, ChainId::ETHEREUM);
        assert_eq!(assets[1].asset.token, TokenRef::Native);
    }

    #[tokio::test]
    async fn status_passes_memo_and_parses_enum() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/near/status"))
            .and(query_param("depositAddress", "0xdead"))
            .and(query_param("depositMemo", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "PROCESSING",
                "updatedAt": "2026-11-03T12:05:00Z",
                "originTxHashes": ["0xaaa"],
                "destinationTxHashes": []
            })))
            .mount(&server)
            .await;
        let s = client(&server, 2000).status("0xdead", Some("42")).await.unwrap();
        assert_eq!(s.status, OneClickStatus::Processing);
        assert_eq!(s.origin_tx_hashes, vec!["0xaaa"]);
    }

    #[tokio::test]
    async fn unknown_status_enum_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/near/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "COMPLETED_MAYBE",
                "updatedAt": "2026-11-03T12:05:00Z"
            })))
            .mount(&server)
            .await;
        let err = client(&server, 2000).status("0xdead", None).await.unwrap_err();
        assert!(matches!(err, AdapterError::Schema(_)), "{err:?}");
    }
}
