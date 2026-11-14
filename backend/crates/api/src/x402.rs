//! Private x402 facilitator surface (POC).
//!
//! Direct wallet pay-in (no EIP-7702, no TOTP). The payer signs/sends a one-shot
//! transfer to a confidential 1Click deposit; we bridge to the merchant asset
//! via NEAR Intents. Scheme id: `exact-intents`.
//!
//! While `DRY_RUN=true`, `/verify` and `/settle` accept a `dryrun:` payment
//! reference so Harborline can exercise the chooser without a mainnet send.

use crate::checkout::{self, buffered_max_debit};
use crate::settle::{DestinationVerifier, RpcDestinationVerifier};
use crate::{enqueue_and_deliver, fail, snake_status, spawn_deposit_hint, ApiResult, AppState, ErrorBody};
use alloy_primitives::Address;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, TokenRef};
use pcc_payments::domain::{PaymentAttempt, PaymentAttemptId, PaymentIntent};
use pcc_payments::quote::{validate_quote, Confidentiality, QuoteExpectation};
use pcc_payments::status::PaymentAttemptStatus;
use pcc_payments::traits::SupportedAsset;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub const SCHEME: &str = "exact-intents";
pub const X402_VERSION: u32 = 1;

const CHAIN_NAME: &[(u64, &str)] = &[
    (1, "Ethereum"),
    (10, "Optimism"),
    (8453, "Base"),
    (42161, "Arbitrum"),
];

fn caip(chain_id: u64) -> String {
    format!("eip155:{chain_id}")
}

fn chain_label(chain_id: u64) -> &'static str {
    CHAIN_NAME
        .iter()
        .find(|(id, _)| *id == chain_id)
        .map(|(_, n)| *n)
        .unwrap_or("EVM")
}

fn token_addr(asset: &AssetId) -> String {
    match asset.token {
        TokenRef::Erc20 { address } => address.to_string(),
        TokenRef::Native => String::new(),
    }
}

#[derive(Serialize, ToSchema)]
pub struct SupportedResponse {
    pub x402_version: u32,
    pub scheme: &'static str,
    pub networks: Vec<String>,
    pub description: &'static str,
    pub totp_required: bool,
    pub eip7702_required: bool,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct AcceptEntry {
    pub scheme: String,
    pub network: String,
    pub max_amount_required: String,
    pub asset: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    pub extra: AcceptExtra,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct AcceptExtra {
    pub name: String,
    pub chain_id: u64,
    pub chain_name: String,
    pub asset_id: String,
    pub decimals: u8,
    pub symbol: String,
    pub description: String,
}

#[derive(Serialize, ToSchema)]
pub struct RequirementsResponse {
    pub x402_version: u32,
    pub error: String,
    pub resource: RequirementsResource,
    pub accepts: Vec<AcceptEntry>,
    pub session_id: String,
    pub intent_id: String,
    pub merchant_order_id: String,
    pub output_amount: String,
    pub destination_asset: String,
    pub dry_run: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RequirementsResource {
    pub url: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PrepareRequest {
    pub asset_id: String,
    pub payer: String,
}

#[derive(Serialize, ToSchema)]
pub struct PrepareResponse {
    pub x402_version: u32,
    pub attempt_id: String,
    pub session_id: String,
    pub intent_id: String,
    pub payer: String,
    pub quoted_input: String,
    pub deposit_address: String,
    pub source_asset: String,
    pub chain_id: u64,
    pub chain_name: String,
    pub token_address: String,
    pub decimals: u8,
    pub symbol: String,
    pub accepts: AcceptEntry,
    pub dry_run: bool,
    pub execution: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PaymentPayloadBody {
    pub x402_version: u32,
    pub scheme: String,
    pub network: String,
    pub payload: PaymentPayloadInner,
}

#[derive(Deserialize, ToSchema)]
pub struct PaymentPayloadInner {
    pub tx_hash: String,
    pub from: String,
    pub amount: String,
    pub asset: String,
    pub pay_to: String,
    pub attempt_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct FacilitatorRequest {
    pub payment_payload: PaymentPayloadBody,
    pub payment_requirements: AcceptEntry,
}

#[derive(Serialize, ToSchema)]
pub struct VerifyResponse {
    pub is_valid: bool,
    pub invalid_reason: Option<String>,
    pub payer: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SettleResponse {
    pub success: bool,
    pub error: Option<String>,
    pub tx_hash: Option<String>,
    pub network: Option<String>,
    pub payer: Option<String>,
    pub attempt_id: Option<String>,
    pub intent_id: Option<String>,
    pub intent_status: Option<String>,
    pub execution: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/x402/supported",
    tag = "x402",
    responses((status = 200, body = SupportedResponse))
)]
pub async fn supported() -> Json<SupportedResponse> {
    Json(SupportedResponse {
        x402_version: X402_VERSION,
        scheme: SCHEME,
        networks: checkout::SOURCE_CHAIN_IDS
            .iter()
            .map(|c| caip(c.0))
            .collect(),
        description: "Private exact payment via confidential NEAR Intents. Wallet transfer only; no TOTP or EIP-7702.",
        totp_required: false,
        eip7702_required: false,
    })
}

#[utoipa::path(
    get,
    path = "/v1/x402/sessions/{session_id}/requirements",
    tag = "x402",
    params(("session_id" = String, Path, description = "Checkout session id from the payment intent")),
    responses((status = 200, body = RequirementsResponse), (status = 404, body = ErrorBody))
)]
pub async fn requirements(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<RequirementsResponse>> {
    let (session, intent, name) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let assets = selectable(&state, &intent).await?;
    let mut accepts = Vec::with_capacity(assets.len());
    for a in assets {
        let amount = preview_amount(&state, &intent, &a.asset).await;
        accepts.push(accept_entry(&a, &amount, /*pay_to*/ String::new()));
    }
    Ok(Json(RequirementsResponse {
        x402_version: X402_VERSION,
        error: "PAYMENT-SIGNATURE header is required".into(),
        resource: RequirementsResource {
            url: format!(
                "{}/x402/{}",
                state.config.checkout_base_url.trim_end_matches('/'),
                session.id
            ),
            description: format!("{name} private x402 payment"),
            mime_type: "application/json".into(),
        },
        accepts,
        session_id: session.id,
        intent_id: intent.id.to_string(),
        merchant_order_id: intent.merchant_order_id,
        output_amount: intent.output_amount_base_units.to_string(),
        destination_asset: intent.payout_config_snapshot.asset.one_click_asset_id,
        dry_run: state.config.gates.dry_run,
    }))
}

#[utoipa::path(
    post,
    path = "/v1/x402/sessions/{session_id}/prepare",
    tag = "x402",
    request_body = PrepareRequest,
    responses(
        (status = 200, body = PrepareResponse),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)
    )
)]
pub async fn prepare(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PrepareRequest>,
) -> ApiResult<Json<PrepareResponse>> {
    let (session, intent, _) = state
        .store
        .get_session(&session_id, Utc::now())
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "not_found", "checkout session not found"))?;
    let payer: Address = body
        .payer
        .parse()
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid_payer", "payer must be an EVM address"))?;
    let chosen = selectable(&state, &intent)
        .await?
        .into_iter()
        .find(|a| a.asset.one_click_asset_id == body.asset_id)
        .ok_or_else(|| {
            fail(
                StatusCode::BAD_REQUEST,
                "unsupported_asset",
                "asset is not a selectable x402 source",
            )
        })?;

    let max_debit = buffered_max_debit(intent.output_amount_base_units);
    let attempt = state
        .store
        .ensure_attempt(&session, &intent, payer, chosen.asset.clone(), max_debit)
        .ok_or_else(|| {
            fail(
                StatusCode::CONFLICT,
                "session_wallet_locked",
                "another wallet already prepared this x402 session",
            )
        })?;
    let attempt = if attempt.source_asset.one_click_asset_id != chosen.asset.one_click_asset_id {
        state
            .store
            .set_source_asset(attempt.id, chosen.asset.clone())
            .ok_or_else(|| {
                fail(
                    StatusCode::CONFLICT,
                    "attempt_locked",
                    "source asset can only change before payment",
                )
            })?
    } else {
        attempt
    };

    let (quoted, deposit, execution) = allocate_deposit(&state, &intent, &attempt).await?;
    let attempt = state.store.get_attempt(attempt.id).unwrap_or(attempt);
    let pay_to = deposit.clone();
    let accepts = accept_entry(
        &SupportedAsset {
            asset: attempt.source_asset.clone(),
            symbol: chosen.symbol.clone(),
        },
        &quoted,
        pay_to.clone(),
    );

    Ok(Json(PrepareResponse {
        x402_version: X402_VERSION,
        attempt_id: attempt.id.to_string(),
        session_id: session.id,
        intent_id: intent.id.to_string(),
        payer: payer.to_string(),
        quoted_input: quoted.clone(),
        deposit_address: deposit,
        source_asset: attempt.source_asset.one_click_asset_id.clone(),
        chain_id: attempt.source_asset.chain_id.0,
        chain_name: chain_label(attempt.source_asset.chain_id.0).into(),
        token_address: token_addr(&attempt.source_asset),
        decimals: attempt.source_asset.decimals,
        symbol: chosen.symbol,
        accepts,
        dry_run: state.config.gates.dry_run,
        execution,
    }))
}

#[utoipa::path(
    post,
    path = "/v1/x402/verify",
    tag = "x402",
    request_body = FacilitatorRequest,
    responses((status = 200, body = VerifyResponse), (status = 400, body = ErrorBody))
)]
pub async fn verify(
    State(state): State<AppState>,
    Json(body): Json<FacilitatorRequest>,
) -> ApiResult<Json<VerifyResponse>> {
    match check_payment(&state, &body).await {
        Ok(ctx) => Ok(Json(VerifyResponse {
            is_valid: true,
            invalid_reason: None,
            payer: Some(ctx.payer.to_string()),
        })),
        Err(reason) => Ok(Json(VerifyResponse {
            is_valid: false,
            invalid_reason: Some(reason),
            payer: None,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/v1/x402/settle",
    tag = "x402",
    request_body = FacilitatorRequest,
    responses((status = 200, body = SettleResponse), (status = 400, body = ErrorBody))
)]
pub async fn settle(
    State(state): State<AppState>,
    Json(body): Json<FacilitatorRequest>,
) -> ApiResult<Json<SettleResponse>> {
    let ctx = match check_payment(&state, &body).await {
        Ok(c) => c,
        Err(reason) => {
            return Ok(Json(SettleResponse {
                success: false,
                error: Some(reason),
                tx_hash: None,
                network: None,
                payer: None,
                attempt_id: None,
                intent_id: None,
                intent_status: None,
                execution: None,
            }));
        }
    };

    let approval_ref = format!("x402:{}", ctx.attempt.id);
    let already = ctx
        .attempt
        .source_tx_hash
        .as_deref()
        .is_some_and(|h| h.eq_ignore_ascii_case(&ctx.tx_hash));
    if !already {
        if matches!(
            ctx.attempt.status,
            PaymentAttemptStatus::DepositBroadcast | PaymentAttemptStatus::Settled
        ) && ctx.attempt.source_tx_hash.is_some()
        {
            return Ok(Json(SettleResponse {
                success: false,
                error: Some("attempt already has a different source payment".into()),
                tx_hash: ctx.attempt.source_tx_hash,
                network: Some(caip(ctx.attempt.source_asset.chain_id.0)),
                payer: Some(ctx.payer.to_string()),
                attempt_id: Some(ctx.attempt.id.to_string()),
                intent_id: Some(ctx.intent.id.to_string()),
                intent_status: Some(snake_status(ctx.intent.status)),
                execution: Some("conflict".into()),
            }));
        }
        state
            .store
            .mark_broadcast(ctx.attempt.id, &ctx.tx_hash, &approval_ref);
        spawn_deposit_hint(&state, ctx.attempt.id);
        let payload = serde_json::json!({
            "type": "payment.authorized",
            "intent_id": ctx.intent.id.to_string(),
            "attempt_id": ctx.attempt.id.to_string(),
            "execution": "x402_wallet",
            "quoted_input": ctx.attempt.quoted_input.to_string(),
            "deposit_address": ctx.attempt.deposit_address,
            "source_tx_hash": ctx.tx_hash,
            "scheme": SCHEME,
        })
        .to_string();
        enqueue_and_deliver(&state, &ctx.intent, "payment.authorized", &payload);
    }

    let intent = state
        .store
        .intent_by_id(ctx.intent.id)
        .unwrap_or(ctx.intent);
    let execution = if state.config.gates.dry_run && ctx.tx_hash.starts_with("dryrun:") {
        "dry_run"
    } else {
        "x402_wallet"
    };

    Ok(Json(SettleResponse {
        success: true,
        error: None,
        tx_hash: Some(ctx.tx_hash),
        network: Some(caip(ctx.attempt.source_asset.chain_id.0)),
        payer: Some(ctx.payer.to_string()),
        attempt_id: Some(ctx.attempt.id.to_string()),
        intent_id: Some(intent.id.to_string()),
        intent_status: Some(snake_status(intent.status)),
        execution: Some(execution.into()),
    }))
}

struct PaymentCtx {
    attempt: PaymentAttempt,
    intent: PaymentIntent,
    payer: Address,
    tx_hash: String,
}

async fn check_payment(state: &AppState, body: &FacilitatorRequest) -> Result<PaymentCtx, String> {
    if body.payment_payload.x402_version != X402_VERSION {
        return Err(format!("unsupported x402Version; want {X402_VERSION}"));
    }
    if body.payment_payload.scheme != SCHEME || body.payment_requirements.scheme != SCHEME {
        return Err(format!("scheme must be {SCHEME}"));
    }
    if body.payment_payload.network != body.payment_requirements.network {
        return Err("payload network does not match requirements".into());
    }

    let attempt_id = body
        .payment_payload
        .payload
        .attempt_id
        .parse::<Uuid>()
        .map_err(|_| "invalid attempt_id".to_owned())?;
    let attempt = state
        .store
        .get_attempt(PaymentAttemptId(attempt_id))
        .ok_or_else(|| "attempt not found".to_owned())?;
    let intent = state
        .store
        .intent_by_id(attempt.payment_intent_id)
        .ok_or_else(|| "intent not found".to_owned())?;

    let payer: Address = body
        .payment_payload
        .payload
        .from
        .parse()
        .map_err(|_| "invalid from address".to_owned())?;
    if payer != attempt.source_wallet {
        return Err("payer does not match prepared attempt".into());
    }

    let deposit = attempt
        .deposit_address
        .as_deref()
        .ok_or_else(|| "attempt has no deposit address; call prepare first".to_owned())?;
    if !deposit.eq_ignore_ascii_case(body.payment_payload.payload.pay_to.trim())
        || !deposit.eq_ignore_ascii_case(body.payment_requirements.pay_to.trim())
    {
        return Err("payTo does not match the prepared 1Click deposit".into());
    }

    let want_amount = attempt.quoted_input.to_string();
    if body.payment_payload.payload.amount != want_amount
        || body.payment_requirements.max_amount_required != want_amount
    {
        return Err("amount does not match the prepared quote".into());
    }

    let want_asset = token_addr(&attempt.source_asset);
    if !body
        .payment_payload
        .payload
        .asset
        .eq_ignore_ascii_case(&want_asset)
        || !body.payment_requirements.asset.eq_ignore_ascii_case(&want_asset)
    {
        return Err("asset does not match the prepared source token".into());
    }

    if caip(attempt.source_asset.chain_id.0) != body.payment_payload.network {
        return Err("network does not match the prepared source chain".into());
    }

    let tx_hash = body.payment_payload.payload.tx_hash.trim().to_owned();
    if tx_hash.is_empty() {
        return Err("tx_hash required".into());
    }

    if state.config.gates.dry_run && tx_hash.starts_with("dryrun:") {
        return Ok(PaymentCtx {
            attempt,
            intent,
            payer,
            tx_hash,
        });
    }

    if !looks_like_tx_hash(&tx_hash) {
        return Err("tx_hash must be a 0x-prefixed 32-byte hash (or dryrun:… while DRY_RUN)".into());
    }

    // Live path: confirm the ERC-20 transfer landed (or at least matches once mined).
    // While still pending, we accept the hash after structural checks so settle can
    // register it; the existing settlement poller confirms destination receipt.
    if !state.config.gates.dry_run {
        let verifier = RpcDestinationVerifier::from_env();
        let min: BaseUnits = want_amount.parse().unwrap_or(attempt.quoted_input);
        let to: Address = deposit
            .parse()
            .map_err(|_| "deposit address is not an EVM address".to_owned())?;
        match verifier.verify(&attempt.source_asset, to, min, &tx_hash).await
        {
            Ok(true) => {}
            Ok(false) => {
                // Pending or not yet indexed is ok; settle still records the hash.
            }
            Err(e) => return Err(format!("could not check source tx: {e}")),
        }
    }

    Ok(PaymentCtx {
        attempt,
        intent,
        payer,
        tx_hash,
    })
}

fn looks_like_tx_hash(h: &str) -> bool {
    let h = h.strip_prefix("0x").unwrap_or(h);
    h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn accept_entry(asset: &SupportedAsset, amount: &str, pay_to: String) -> AcceptEntry {
    AcceptEntry {
        scheme: SCHEME.into(),
        network: caip(asset.asset.chain_id.0),
        max_amount_required: amount.to_owned(),
        asset: token_addr(&asset.asset),
        pay_to,
        max_timeout_seconds: 600,
        extra: AcceptExtra {
            name: asset.symbol.clone(),
            chain_id: asset.asset.chain_id.0,
            chain_name: chain_label(asset.asset.chain_id.0).into(),
            asset_id: asset.asset.one_click_asset_id.clone(),
            decimals: asset.asset.decimals,
            symbol: asset.symbol.clone(),
            description: "Confidential NEAR Intents deposit; Harborline receives its configured payout asset."
                .into(),
        },
    }
}

async fn selectable(state: &AppState, intent: &PaymentIntent) -> ApiResult<Vec<SupportedAsset>> {
    let payout = &intent.payout_config_snapshot.asset.one_click_asset_id;
    let assets = match state.integrations.as_deref() {
        Some(ports) => ports
            .assets()
            .await
            .map_err(|e| fail(StatusCode::BAD_GATEWAY, "assets_unavailable", &e.to_string()))?,
        None => vec![
            SupportedAsset {
                asset: AssetId::usdc_base(),
                symbol: "USDC".into(),
            },
            SupportedAsset {
                asset: AssetId::from_one_click_token(
                    "op",
                    Some("0x0b2c639c533813f4aa9d7837caf62653d097ff85"),
                    6,
                    "nep141:op-0x0b2c639c533813f4aa9d7837caf62653d097ff85.omft.near",
                )
                .unwrap_or_else(AssetId::usdc_base),
                symbol: "USDC".into(),
            },
            SupportedAsset {
                asset: AssetId::from_one_click_token(
                    "eth",
                    Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
                    6,
                    "nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
                )
                .unwrap_or_else(AssetId::usdc_base),
                symbol: "USDC".into(),
            },
        ],
    };
    Ok(assets
        .into_iter()
        .filter(|a| checkout::selectable_source(&a.asset) && &a.asset.one_click_asset_id != payout)
        .collect())
}

async fn preview_amount(state: &AppState, intent: &PaymentIntent, asset: &AssetId) -> String {
    let Some(ports) = state.integrations.as_deref() else {
        return intent.output_amount_base_units.to_string();
    };
    let dummy = PaymentAttempt {
        id: PaymentAttemptId::new(),
        payment_intent_id: intent.id,
        source_wallet: Address::ZERO,
        source_asset: asset.clone(),
        quoted_input: intent.output_amount_base_units,
        max_debit: buffered_max_debit(intent.output_amount_base_units),
        quote_signature: "x402-preview".into(),
        quote_raw_response: "{}".into(),
        quote_expires_at: intent.expires_at,
        deposit_address: None,
        deposit_memo: None,
        source_tx_hash: None,
        destination_tx_hash: None,
        approval_ref: None,
        status: PaymentAttemptStatus::Quoted,
    };
    let spec = checkout::quote_spec_named(intent, &dummy, true, "x402");
    match ports.preview(&spec).await {
        Ok(q) => q.amount_in.to_string(),
        Err(_) => intent.output_amount_base_units.to_string(),
    }
}

async fn allocate_deposit(
    state: &AppState,
    intent: &PaymentIntent,
    attempt: &PaymentAttempt,
) -> ApiResult<(String, String, String)> {
    let Some(ports) = state.integrations.as_deref() else {
        // Local UI without the TS worker: deterministic placeholder deposit.
        let deposit = format!("0x{:040x}", attempt.id.0.as_u128() ^ 0x402u128);
        state.store.apply_preview(
            attempt.id,
            &synthetic_quote(intent, attempt, &deposit, true),
            buffered_max_debit(intent.output_amount_base_units),
        );
        let quoted = intent.output_amount_base_units.to_string();
        // Persist deposit on the attempt via a tiny executable-shaped quote.
        state
            .store
            .apply_executable(attempt.id, &synthetic_quote(intent, attempt, &deposit, false));
        return Ok((quoted, deposit, "unconfigured".into()));
    };

    checkout::refresh_preview(ports, &state.store, intent, attempt).await;
    let attempt = state.store.get_attempt(attempt.id).unwrap_or_else(|| attempt.clone());
    if attempt.max_debit.is_zero() {
        return Err(fail(
            StatusCode::BAD_GATEWAY,
            "preview_required",
            "no priced quote for this source asset",
        ));
    }

    let spec = checkout::quote_spec_named(intent, &attempt, false, "x402");
    let quote = ports
        .executable_quote(&spec)
        .await
        .map_err(|e| fail(StatusCode::BAD_GATEWAY, "quote_failed", &e.to_string()))?;
    let now = Utc::now();
    let expectation = QuoteExpectation {
        source_asset: attempt.source_asset.clone(),
        destination_asset: intent.payout_config_snapshot.asset.clone(),
        recipient: intent.payout_config_snapshot.receiving_address.to_string(),
        refund_to: attempt.source_wallet.to_string(),
        output_amount: intent.output_amount_base_units,
        max_debit: attempt.max_debit,
        max_slippage_bps: 100,
        require_confidentiality: Confidentiality::Basic,
        require_executable: true,
        min_remaining_validity: Duration::minutes(10),
        max_quote_age: Duration::minutes(5),
    };
    validate_quote(&quote, &expectation, now).map_err(|v| {
        fail(
            StatusCode::BAD_GATEWAY,
            "quote_invalid",
            &v.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
        )
    })?;
    let deposit = quote
        .deposit_address
        .clone()
        .ok_or_else(|| fail(StatusCode::BAD_GATEWAY, "quote_failed", "executable quote missing deposit"))?;
    state.store.apply_executable(attempt.id, &quote);
    Ok((quote.amount_in.to_string(), deposit, "awaiting_wallet".into()))
}

fn synthetic_quote(
    intent: &PaymentIntent,
    attempt: &PaymentAttempt,
    deposit: &str,
    dry: bool,
) -> pcc_payments::quote::VerifiedQuote {
    pcc_payments::quote::VerifiedQuote {
        correlation_id: format!("x402-local-{}", attempt.id),
        signature: "x402-local".into(),
        signature_verified: true,
        dry,
        swap_type: pcc_payments::quote::SwapType::ExactOutput,
        confidentiality: Confidentiality::Basic,
        deposit_mode: pcc_payments::quote::DepositMode::Simple,
        origin_asset_id: attempt.source_asset.one_click_asset_id.clone(),
        destination_asset_id: intent.payout_config_snapshot.asset.one_click_asset_id.clone(),
        recipient: intent.payout_config_snapshot.receiving_address.to_string(),
        refund_to: attempt.source_wallet.to_string(),
        amount_in: intent.output_amount_base_units,
        min_amount_in: intent.output_amount_base_units,
        amount_out: intent.output_amount_base_units,
        min_amount_out: intent.output_amount_base_units,
        deposit_address: (!dry).then(|| deposit.to_owned()),
        deposit_memo: None,
        quote_timestamp: Utc::now(),
        deadline: Some(Utc::now() + Duration::hours(1)),
        time_estimate_seconds: 60,
        raw_response_json: "{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{router_with, Config, Store};
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use pcc_auth::TotpAuthorizer;
    use pcc_merchants::new_payout_config;
    use pcc_payments::domain::MerchantId;
    use pcc_payments::ExecutionGates;
    use pcc_settlement::webhook::WebhookPolicy;
    use tower::ServiceExt;

    const KEY: &str = "pcc_sk_x402";

    fn cfg() -> Config {
        Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            gates: ExecutionGates::default(),
            integrations_base_url: "http://127.0.0.1:8787".into(),
            checkout_base_url: "http://127.0.0.1:3000".into(),
            webhook_policy: WebhookPolicy::default(),
            approval_mode: crate::operator::ApprovalMode::Payer,
            operator_token_hash: None,
            siwe_domain: "127.0.0.1:3000".into(),
            signup_token_hash: None,
        }
    }

    fn app() -> axum::Router {
        let store = Store::default();
        let mid = MerchantId::new();
        store.register_merchant(
            "Harborline X402",
            KEY,
            new_payout_config(
                mid,
                AssetId::usdc_arbitrum(),
                "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap(),
            )
            .unwrap(),
            "whsec_x402",
        );
        router_with(cfg(), store, TotpAuthorizer::new(b"x402-test-master-key!!!!!!".to_vec()))
    }

    async fn call(app: axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        if body.is_empty() {
            return (status, serde_json::Value::Null);
        }
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn supported_lists_exact_intents() {
        let (st, v) = call(app(), Request::builder().uri("/v1/x402/supported").body(Body::empty()).unwrap()).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["scheme"], SCHEME);
        assert_eq!(v["totp_required"], false);
        assert_eq!(v["eip7702_required"], false);
    }

    #[tokio::test]
    async fn prepare_verify_settle_dry_run_without_totp() {
        let app = app();
        let (st, intent_v) = call(
            app.clone(),
            Request::builder()
                .method("POST")
                .uri("/v1/payment-intents")
                .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                .header("Idempotency-Key", "x402-flow")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"merchant_order_id":"boat-x402-flow","output_amount":"200000"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{intent_v}");
        let session = intent_v["checkout_session_id"].as_str().unwrap();

        let (st, reqs) = call(
            app.clone(),
            Request::builder()
                .uri(format!("/v1/x402/sessions/{session}/requirements"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{reqs}");
        assert!(!reqs["accepts"].as_array().unwrap().is_empty());
        assert_eq!(reqs["dry_run"], true);
        let asset_id = reqs["accepts"][0]["extra"]["asset_id"].as_str().unwrap();

        let payer = "0x1111111111111111111111111111111111111111";
        let (st, prep) = call(
            app.clone(),
            Request::builder()
                .method("POST")
                .uri(format!("/v1/x402/sessions/{session}/prepare"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"asset_id":"{asset_id}","payer":"{payer}"}}"#
                )))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{prep}");
        assert!(!prep["deposit_address"].as_str().unwrap().is_empty());
        let accepts = &prep["accepts"];
        let body = serde_json::json!({
            "payment_payload": {
                "x402_version": 1,
                "scheme": SCHEME,
                "network": accepts["network"],
                "payload": {
                    "tx_hash": "dryrun:harborline-poc",
                    "from": payer,
                    "amount": prep["quoted_input"],
                    "asset": accepts["asset"],
                    "pay_to": prep["deposit_address"],
                    "attempt_id": prep["attempt_id"],
                }
            },
            "payment_requirements": accepts,
        });

        let (st, ver) = call(
            app.clone(),
            Request::builder()
                .method("POST")
                .uri("/v1/x402/verify")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{ver}");
        assert_eq!(ver["is_valid"], true);

        let (st, set) = call(
            app,
            Request::builder()
                .method("POST")
                .uri("/v1/x402/settle")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{set}");
        assert_eq!(set["success"], true);
        assert_eq!(set["execution"], "dry_run");
        assert_eq!(set["intent_status"], "processing");
    }
}
