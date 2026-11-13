//! Hosted-checkout quote + gated session-deposit orchestration.
//!
//! Broadcast still requires `DRY_RUN=false`, `ENABLE_MAINNET_TRANSFERS=true`
//! on mainnet, and a non-empty approval ref. Default gates stop before send.

use crate::operator::ApprovalMode;
use crate::store::{PendingApproval, Store};
use chrono::{Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::{PaymentAttempt, PaymentIntent};
use pcc_payments::quote::{validate_quote, Confidentiality, QuoteExpectation};
use pcc_payments::traits::{AdapterError, CheckoutPorts, DepositPlan, QuoteSpec, Simulation};

const SLIPPAGE_BPS: u32 = 100;
const PREVIEW_BUFFER_BPS: u32 = 1_000;
/// Cap on input drift between the preview the payer sees and the executable quote.
const DRIFT_BUFFER_BPS: u32 = 200;
const MAX_FEE_WEI: u128 = 300_000_000_000_000; // 0.0003 ETH
/// How long an operator has to approve a parked deposit.
const APPROVAL_WINDOW: Duration = Duration::minutes(10);

pub fn buffered_max_debit(output: BaseUnits) -> BaseUnits {
    match output.bps_ceil(PREVIEW_BUFFER_BPS) {
        Ok(buf) => output.checked_add(buf).unwrap_or(output),
        Err(_) => output,
    }
}

/// Source assets the session-key executor can spend: ERC-20s on 7702 chains.
pub fn selectable_source(asset: &AssetId) -> bool {
    matches!(asset.token, TokenRef::Erc20 { .. }) && matches!(asset.chain_id, ChainId::BASE | ChainId::ARBITRUM)
}

pub fn quote_spec(intent: &PaymentIntent, attempt: &PaymentAttempt, dry_ref: bool) -> QuoteSpec {
    let suffix = if dry_ref { "preview" } else { "exec" };
    QuoteSpec {
        source_asset: attempt.source_asset.clone(),
        destination_asset: intent.payout_config_snapshot.asset.clone(),
        output_amount: intent.output_amount_base_units,
        recipient: intent.payout_config_snapshot.receiving_address,
        refund_to: attempt.source_wallet,
        confidentiality: Confidentiality::Basic,
        slippage_bps: SLIPPAGE_BPS,
        deadline: Utc::now() + Duration::hours(1),
        authorization_ref: format!("checkout-{suffix}-{}", attempt.id),
    }
}

pub async fn refresh_preview(
    ports: &dyn CheckoutPorts,
    store: &Store,
    intent: &PaymentIntent,
    attempt: &PaymentAttempt,
) {
    let spec = quote_spec(intent, attempt, true);
    match ports.preview(&spec).await {
        Ok(quote) => {
            let drift_cap = quote
                .amount_in
                .bps_ceil(DRIFT_BUFFER_BPS)
                .and_then(|buf| quote.amount_in.checked_add(buf))
                .unwrap_or(quote.amount_in);
            // A zero cap means the asset changed and has no unit-correct fallback yet.
            let max_debit = if attempt.max_debit.is_zero() {
                drift_cap
            } else {
                drift_cap.min(attempt.max_debit)
            };
            store.apply_preview(attempt.id, &quote, max_debit);
        }
        Err(e) => {
            tracing::warn!(error = %e, attempt_id = %attempt.id, "checkout preview quote failed; authorize needs a non-zero max_debit");
        }
    }
}

/// Best-effort `/v0/deposit/submit` hint. 1Click detects deposits on its own;
/// this only speeds detection, so failures are retried briefly and dropped.
pub async fn submit_deposit_hint(ports: &dyn CheckoutPorts, deposit: &str, tx: &str, delays: &[std::time::Duration]) {
    for (i, delay) in std::iter::once(&std::time::Duration::ZERO).chain(delays).enumerate() {
        tokio::time::sleep(*delay).await;
        match ports.submit_deposit_tx(deposit, tx).await {
            Ok(()) => {
                tracing::info!(deposit, try_no = i + 1, "deposit hint accepted by 1Click");
                return;
            }
            Err(AdapterError::Unavailable(_) | AdapterError::Timeout | AdapterError::RateLimited) => {}
            Err(e) => {
                tracing::info!(deposit, error = %e, "deposit hint rejected; relying on 1Click detection");
                return;
            }
        }
    }
    tracing::info!(
        deposit,
        "deposit hint unavailable after retries; relying on 1Click detection"
    );
}

pub struct AuthorizeSettlement {
    pub quoted_input: String,
    pub deposit_address: Option<String>,
    pub execution: String,
    pub simulation_ok: Option<bool>,
}

impl AuthorizeSettlement {
    pub fn unconfigured(attempt: &PaymentAttempt) -> Self {
        Self {
            quoted_input: attempt.quoted_input.to_string(),
            deposit_address: attempt.deposit_address.clone(),
            execution: "unconfigured".into(),
            simulation_ok: None,
        }
    }
}

pub async fn settle_authorized(
    ports: &dyn CheckoutPorts,
    store: &Store,
    intent: &PaymentIntent,
    attempt: &PaymentAttempt,
    gates: pcc_payments::ExecutionGates,
    approval_mode: ApprovalMode,
    mandate: Option<uuid::Uuid>,
) -> Result<AuthorizeSettlement, AdapterError> {
    let spec = quote_spec(intent, attempt, false);
    let quote = ports.executable_quote(&spec).await?;
    let now = Utc::now();
    let expectation = QuoteExpectation {
        source_asset: attempt.source_asset.clone(),
        destination_asset: intent.payout_config_snapshot.asset.clone(),
        recipient: intent.payout_config_snapshot.receiving_address.to_string(),
        refund_to: attempt.source_wallet.to_string(),
        output_amount: intent.output_amount_base_units,
        max_debit: attempt.max_debit,
        max_slippage_bps: SLIPPAGE_BPS,
        require_confidentiality: Confidentiality::Basic,
        require_executable: true,
        min_remaining_validity: Duration::minutes(10),
        max_quote_age: Duration::minutes(5),
    };
    validate_quote(&quote, &expectation, now)
        .map_err(|v| AdapterError::Rejected(v.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))?;
    store.apply_executable(attempt.id, &quote);

    let deposit = quote
        .deposit_address
        .as_deref()
        .ok_or_else(|| AdapterError::Rejected("executable quote missing deposit address".into()))?;
    let to = deposit
        .parse()
        .map_err(|_| AdapterError::Rejected("deposit address is not an EVM address".into()))?;
    let approval_ref = match mandate {
        Some(m) => format!("autopay:{m}:{}", attempt.id),
        None => format!("checkout:{}", attempt.id),
    };
    let plan = DepositPlan {
        attempt_id: attempt.id,
        chain_id: attempt.source_asset.chain_id,
        asset: attempt.source_asset.clone(),
        from: attempt.source_wallet,
        to,
        amount: quote.amount_in,
        max_fee_wei: BaseUnits::from_u128(MAX_FEE_WEI),
        approval_ref: approval_ref.clone(),
        mandate,
    };
    let simulation: Simulation = ports.simulate(&plan).await?;
    tracing::info!(
        attempt_id = %attempt.id,
        ok = simulation.ok,
        gas = simulation.gas_estimate,
        sender_balance = %simulation.sender_balance,
        amount = %plan.amount,
        to = %plan.to,
        notes = ?simulation.notes,
        "deposit preflight"
    );
    let execution = if !simulation.ok {
        "simulation_failed".to_owned()
    } else if gates.check_broadcast(plan.chain_id, Some(&approval_ref)).is_err() {
        "gated".to_owned()
    } else if approval_mode == ApprovalMode::Owner {
        let expires_at = quote.deadline.map_or(now + APPROVAL_WINDOW, |d| {
            (d - Duration::minutes(5)).min(now + APPROVAL_WINDOW)
        });
        store.put_pending(PendingApproval {
            plan: plan.clone(),
            intent_id: intent.id,
            expires_at,
        });
        "awaiting_approval".to_owned()
    } else {
        match ports.execute_deposit(&plan).await {
            Ok(hash) => {
                let tx = format!("{hash:#x}");
                store.mark_broadcast(attempt.id, &tx, &approval_ref);
                "broadcast".to_owned()
            }
            Err(e) => {
                tracing::warn!(error = %e, "gated or failed session execute");
                "execute_rejected".to_owned()
            }
        }
    };

    Ok(AuthorizeSettlement {
        quoted_input: quote.amount_in.to_string(),
        deposit_address: quote.deposit_address,
        execution,
        simulation_ok: Some(simulation.ok),
    })
}
