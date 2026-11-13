//! HTTP client for the private TS integration worker. Implements
//! `pcc_payments::traits::OneClickAdapter` over the v1 internal contract.

use crate::contract::{
    AckResponse, AllowanceDto, AllowanceKeyDto, AssetsResponse, DepositPlanDto, DepositSubmittedDto, ErrorCode,
    ErrorResponse, ExecuteResponseDto, GrantEnableDto, GrantPrepareDto, GrantPreparedDto, HealthResponse,
    QuoteRequestDto, ReceiptDto, RevokedDto, SimulationDto, SwapStatusDto, VerifiedQuoteDto, VerifyRequestDto,
    BASE_PATH,
};
use alloy_primitives::{Address, B256};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::quote::VerifiedQuote;
use pcc_payments::traits::{
    AdapterError, Allowance, AllowanceKey, DepositPlan, GrantRequest, OneClickAdapter, PreparedGrant, QuoteSpec,
    Receipt, SessionGrantPort, Simulation, SourceExecutor, SupportedAsset, SwapStatus,
};
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

    async fn dispatch(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&impl serde::Serialize>,
    ) -> Result<(StatusCode, String), AdapterError> {
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
        Ok((status, text))
    }

    fn decode_body<T: DeserializeOwned>(path: &str, status: StatusCode, text: &str) -> Result<T, AdapterError> {
        if status.is_success() {
            return serde_json::from_str::<T>(text).map_err(|e| AdapterError::Schema(format!("{path}: {e}")));
        }
        if let Ok(err) = serde_json::from_str::<ErrorResponse>(text) {
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

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&impl serde::Serialize>,
    ) -> Result<T, AdapterError> {
        let (status, text) = self.dispatch(method, path, query, body).await?;
        Self::decode_body(path, status, &text)
    }

    pub async fn health(&self) -> Result<HealthResponse, AdapterError> {
        self.call(Method::GET, "/health", &[], None::<&()>).await
    }

    async fn call_allow_404<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&impl serde::Serialize>,
    ) -> Result<Option<T>, AdapterError> {
        let (status, text) = self.dispatch(method, path, query, body).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::decode_body(path, status, &text).map(Some)
    }
}

fn deposit_plan_dto(plan: &DepositPlan) -> Result<DepositPlanDto, AdapterError> {
    let token = match plan.asset.token {
        TokenRef::Erc20 { address } => format!("{address}"),
        TokenRef::Native => {
            return Err(AdapterError::Rejected(
                "session-key executor refuses native-value deposits; ERC-20 only".into(),
            ));
        }
    };
    let to = format!("{}", plan.to);
    Ok(DepositPlanDto {
        attempt_id: plan.attempt_id.to_string(),
        chain_id: plan.chain_id.0,
        token,
        from: format!("{}", plan.from),
        to: to.clone(),
        quote_deposit_address: to,
        amount: plan.amount,
        max_fee_wei: plan.max_fee_wei,
        approval_ref: plan.approval_ref.clone(),
        gas_limit: None,
        mandate_id: plan.mandate.map(|m| m.to_string()),
    })
}

fn parse_b256(s: &str, field: &str) -> Result<B256, AdapterError> {
    s.parse::<B256>()
        .map_err(|_| AdapterError::Schema(format!("{field}: not a 32-byte hash")))
}

#[async_trait]
impl SourceExecutor for IntegrationClient {
    async fn simulate(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError> {
        let dto: SimulationDto = self
            .call(Method::POST, "/aa/simulate", &[], Some(&deposit_plan_dto(plan)?))
            .await?;
        Ok(Simulation {
            ok: dto.ok,
            gas_estimate: dto.gas_estimate,
            max_fee_wei: dto.max_fee_wei,
            sender_balance: dto.sender_balance,
            notes: dto.notes,
        })
    }

    async fn execute_deposit(&self, plan: &DepositPlan) -> Result<B256, AdapterError> {
        let dto: ExecuteResponseDto = self
            .call(Method::POST, "/aa/execute", &[], Some(&deposit_plan_dto(plan)?))
            .await?;
        parse_b256(&dto.tx_hash, "txHash")
    }

    async fn receipt(
        &self,
        chain_id: pcc_payments::asset::ChainId,
        tx_hash: B256,
    ) -> Result<Option<Receipt>, AdapterError> {
        let chain = chain_id.0.to_string();
        let hash = format!("{tx_hash:#x}");
        let dto: Option<ReceiptDto> = self
            .call_allow_404(
                Method::GET,
                "/aa/receipt",
                &[("chainId", chain.as_str()), ("txHash", hash.as_str())],
                None::<&()>,
            )
            .await?;
        dto.map(|d| {
            Ok(Receipt {
                tx_hash: parse_b256(&d.tx_hash, "txHash")?,
                block_number: d.block_number,
                success: d.success,
                gas_used: d.gas_used,
            })
        })
        .transpose()
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
        ErrorCode::SessionRejected => AdapterError::Rejected(format!("session rejected: {message}")),
        ErrorCode::Internal => AdapterError::Other(format!("worker internal error: {message}")),
    }
}

fn parse_address(s: &str, field: &str) -> Result<Address, AdapterError> {
    s.parse()
        .map_err(|_| AdapterError::Malformed(format!("{field} is not an EVM address")))
}

#[async_trait]
impl SessionGrantPort for IntegrationClient {
    async fn prepare_grant(&self, req: &GrantRequest) -> Result<PreparedGrant, AdapterError> {
        let body = GrantPrepareDto {
            owner: req.key.owner.to_string(),
            chain_id: req.key.chain_id.0,
            token: req.key.token.to_string(),
            max_amount: req.max_amount,
            uses: req.uses,
            validity_sec: req.validity_secs,
            mandate_id: req.key.mandate.map(|m| m.to_string()),
        };
        let dto: GrantPreparedDto = self.call(Method::POST, "/aa/grants/prepare", &[], Some(&body)).await?;
        Ok(PreparedGrant {
            grant_id: dto.grant_id,
            typed_data: dto.typed_data,
            session_signer: parse_address(&dto.session_signer, "sessionSigner")?,
            uses: dto.uses,
            valid_until: dto.valid_until,
            expires_at: parse_ts(&dto.expires_at, "expiresAt")?,
        })
    }

    async fn enable_grant(&self, grant_id: &str, signature: &str) -> Result<Allowance, AdapterError> {
        let body = GrantEnableDto {
            grant_id: grant_id.to_owned(),
            signature: signature.to_owned(),
        };
        let dto: AllowanceDto = self.call(Method::POST, "/aa/grants/enable", &[], Some(&body)).await?;
        allowance_from(&dto)
    }

    async fn allowance(&self, key: &AllowanceKey) -> Result<Option<Allowance>, AdapterError> {
        let dto: Option<AllowanceDto> = self
            .call_allow_404(Method::POST, "/aa/grants/status", &[], Some(&key_dto(key)))
            .await?;
        dto.as_ref().map(allowance_from).transpose()
    }

    async fn revoke_allowance(&self, key: &AllowanceKey) -> Result<bool, AdapterError> {
        let dto: RevokedDto = self
            .call(Method::POST, "/aa/grants/revoke", &[], Some(&key_dto(key)))
            .await?;
        Ok(dto.revoked)
    }
}

fn key_dto(key: &AllowanceKey) -> AllowanceKeyDto {
    AllowanceKeyDto {
        owner: key.owner.to_string(),
        chain_id: key.chain_id.0,
        token: key.token.to_string(),
        mandate_id: key.mandate.map(|m| m.to_string()),
    }
}

fn allowance_from(dto: &AllowanceDto) -> Result<Allowance, AdapterError> {
    Ok(Allowance {
        key: AllowanceKey {
            owner: parse_address(&dto.owner, "owner")?,
            chain_id: ChainId(dto.chain_id),
            token: parse_address(&dto.token, "token")?,
            mandate: dto
                .mandate_id
                .as_deref()
                .map(|m| {
                    m.parse()
                        .map_err(|_| AdapterError::Schema("mandateId: not a uuid".into()))
                })
                .transpose()?,
        },
        max_amount: dto.max_amount,
        uses_remaining: dto.uses_remaining,
        valid_until: dto.valid_until,
    })
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
    use pcc_payments::domain::PaymentAttemptId;
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
            (422, "SESSION_REJECTED", "rejected"),
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
    async fn allowance_prepare_enable_status_revoke_round_trip() {
        let server = MockServer::start().await;
        let owner = "0x049FdEf2550772B9F36F86757C27f5F982D0970D";
        let token = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
        let mandate = "33333333-3333-3333-3333-333333333333";
        let key_json = json!({"owner": owner, "chainId": 8453, "token": token, "mandateId": mandate});
        let allowance_json = json!({
            "owner": owner, "chainId": 8453, "token": token, "mandateId": mandate,
            "maxAmount": "211568", "usesRemaining": 5, "validUntil": 1_800_000_000u64
        });
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/grants/prepare"))
            .and(body_partial_json(json!({
                "owner": owner, "chainId": 8453, "maxAmount": "211568", "uses": 5, "validitySec": 604_800,
                "mandateId": mandate
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "grantId": "g-1",
                "typedData": {"primaryType": "Enable", "message": {"nonce": "3"}},
                "sessionSigner": "0x06e6d59dd649550767405A2E3B1CC13fABE550E7",
                "uses": 5,
                "validUntil": 1_800_000_000u64,
                "expiresAt": "2026-11-08T00:10:00Z"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/grants/enable"))
            .and(body_partial_json(json!({"grantId": "g-1", "signature": "0xsig"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(allowance_json.clone()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/grants/status"))
            .and(body_partial_json(key_json.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_json(allowance_json))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/grants/status"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {"code": "INVALID_REQUEST", "message": "no active allowance", "retryable": false}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/grants/revoke"))
            .and(body_partial_json(key_json))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"revoked": true})))
            .mount(&server)
            .await;

        let c = client(&server, 2_000);
        let key = AllowanceKey {
            owner: owner.parse().unwrap(),
            chain_id: ChainId::BASE,
            token: token.parse().unwrap(),
            mandate: Some(mandate.parse().unwrap()),
        };
        let prepared = c
            .prepare_grant(&GrantRequest {
                key,
                max_amount: BaseUnits::from_u128(211_568),
                uses: 5,
                validity_secs: 604_800,
            })
            .await
            .unwrap();
        assert_eq!(prepared.grant_id, "g-1");
        assert_eq!(prepared.uses, 5);
        let enabled = c.enable_grant("g-1", "0xsig").await.unwrap();
        assert_eq!(enabled.key, key);
        assert_eq!(enabled.uses_remaining, 5);
        assert_eq!(c.allowance(&key).await.unwrap(), Some(enabled));
        assert_eq!(c.allowance(&key).await.unwrap(), None, "404 means nothing active");
        assert!(c.revoke_allowance(&key).await.unwrap());
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

    fn erc20_plan() -> DepositPlan {
        DepositPlan {
            attempt_id: PaymentAttemptId(uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()),
            chain_id: ChainId::SEPOLIA,
            asset: AssetId {
                chain_id: ChainId::SEPOLIA,
                token: TokenRef::Erc20 {
                    address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
            },
            from: "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap(),
            to: "0x76b4c56085ED136a8744D52bE956396624a730E8".parse().unwrap(),
            amount: BaseUnits::from_u128(1_020_000),
            max_fee_wei: BaseUnits::from_u128(10u128.pow(16)),
            approval_ref: "appr-aa-1".into(),
            mandate: None,
        }
    }

    const USER_OP: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn session_simulate_and_execute_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/simulate"))
            .and(body_partial_json(json!({
                "quoteDepositAddress": "0x76b4c56085ED136a8744D52bE956396624a730E8",
                "amount": "1020000",
                "to": "0x76b4c56085ED136a8744D52bE956396624a730E8"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "gasEstimate": 120_000,
                "maxFeeWei": "10000000000000000",
                "senderBalance": "0",
                "notes": ["ok"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/execute"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "userOpHash": USER_OP,
                "txHash": USER_OP
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/aa/receipt"))
            .and(query_param("txHash", USER_OP))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "txHash": USER_OP,
                "blockNumber": 1,
                "success": true,
                "gasUsed": 120_000
            })))
            .mount(&server)
            .await;

        let c = client(&server, 2000);
        let plan = erc20_plan();
        let sim = c.simulate(&plan).await.unwrap();
        assert!(sim.ok);
        assert_eq!(sim.gas_estimate, 120_000);
        let hash = c.execute_deposit(&plan).await.unwrap();
        assert_eq!(format!("{hash:#x}"), USER_OP);
        let rec = c.receipt(ChainId::SEPOLIA, hash).await.unwrap().unwrap();
        assert!(rec.success);
    }

    #[tokio::test]
    async fn session_execute_maps_policy_reject() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/v1/aa/execute"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "error": { "code": "SESSION_REJECTED", "message": "quote_address_mismatch" }
            })))
            .mount(&server)
            .await;
        let err = client(&server, 2000).execute_deposit(&erc20_plan()).await.unwrap_err();
        assert!(matches!(err, AdapterError::Rejected(_)), "{err:?}");
    }

    #[tokio::test]
    async fn session_receipt_404_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/internal/v1/aa/receipt"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "INVALID_REQUEST", "message": "receipt not found" }
            })))
            .mount(&server)
            .await;
        let hash = USER_OP.parse().unwrap();
        let rec = client(&server, 2000).receipt(ChainId::SEPOLIA, hash).await.unwrap();
        assert!(rec.is_none());
    }

    #[test]
    fn session_dto_refuses_native() {
        let mut plan = erc20_plan();
        plan.asset.token = TokenRef::Native;
        let err = deposit_plan_dto(&plan).unwrap_err();
        assert!(matches!(err, AdapterError::Rejected(_)), "{err:?}");
    }
}
