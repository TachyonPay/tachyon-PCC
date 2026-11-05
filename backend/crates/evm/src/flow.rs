//! Deposit orchestration: executable quote → validate → persist → simulate →
//! (stop) → owner-approved broadcast → optional `/deposit/submit` → status poll.
//!
//! Broadcast still goes through [`crate::executor::AlloyEoaExecutor`], which
//! enforces the execution gates independently of this layer.

use crate::plan::plan_from_quote;
use chrono::{DateTime, Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::AssetId;
use pcc_payments::domain::{PaymentAttempt, PaymentAttemptId, PaymentIntent};
use pcc_payments::quote::{validate_quote, Confidentiality, QuoteExpectation, QuoteViolation, VerifiedQuote};
use pcc_payments::status::PaymentAttemptStatus;
use pcc_payments::traits::{AdapterError, DepositPlan, OneClickAdapter, Simulation, SourceExecutor, SwapStatus};
use pcc_settlement::{poll_once, PollOutcome, SettlementError};
use pcc_storage::{Repository, StorageError};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Settlement(#[from] SettlementError),
    #[error("quote failed validation: {0}")]
    Quote(String),
    #[error("attempt {0} is in state {1:?}; refusing")]
    WrongState(PaymentAttemptId, PaymentAttemptStatus),
}

impl From<Vec<QuoteViolation>> for FlowError {
    fn from(v: Vec<QuoteViolation>) -> Self {
        Self::Quote(v.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    pub attempt_id: PaymentAttemptId,
    pub quote: VerifiedQuote,
    pub plan: DepositPlan,
    pub simulation: Simulation,
}

/// Inputs for [`prepare_deposit`]. Packed to keep the function signature short.
pub struct PrepareParams {
    pub intent: PaymentIntent,
    pub source_asset: AssetId,
    pub spec: pcc_payments::traits::QuoteSpec,
    pub max_debit: BaseUnits,
    pub max_fee_wei: BaseUnits,
    pub max_slippage_bps: u32,
    pub now: DateTime<Utc>,
}

/// Request an executable quote, re-validate it in Rust, persist the attempt,
/// simulate the source deposit. Never broadcasts.
pub async fn prepare_deposit<A, E, R>(
    adapter: &A,
    executor: &E,
    repo: &R,
    params: PrepareParams,
) -> Result<PreflightReport, FlowError>
where
    A: OneClickAdapter + Sync,
    E: SourceExecutor + Sync,
    R: Repository + Sync,
{
    let PrepareParams {
        intent,
        source_asset,
        spec,
        max_debit,
        max_fee_wei,
        max_slippage_bps,
        now,
    } = params;
    let quote = adapter.executable_quote(&spec).await?;
    let expectation = QuoteExpectation {
        source_asset: source_asset.clone(),
        destination_asset: intent.payout_config_snapshot.asset.clone(),
        recipient: intent.payout_config_snapshot.receiving_address.to_string(),
        refund_to: spec.refund_to.to_string(),
        output_amount: intent.output_amount_base_units,
        max_debit,
        max_slippage_bps,
        require_confidentiality: Confidentiality::Basic,
        require_executable: true,
        min_remaining_validity: Duration::minutes(10),
        max_quote_age: Duration::minutes(5),
    };
    validate_quote(&quote, &expectation, now)?;

    let attempt_id = PaymentAttemptId::new();
    let plan = plan_from_quote(
        attempt_id,
        &quote,
        &source_asset,
        spec.refund_to,
        max_fee_wei,
        String::new(),
    )?;
    let simulation = executor.simulate(&plan).await?;

    let attempt = PaymentAttempt {
        id: attempt_id,
        payment_intent_id: intent.id,
        source_wallet: spec.refund_to,
        source_asset,
        quoted_input: quote.amount_in,
        max_debit,
        quote_signature: quote.signature.clone(),
        quote_raw_response: quote.raw_response_json.clone(),
        quote_expires_at: quote.deadline.unwrap_or(now),
        deposit_address: quote.deposit_address.clone(),
        deposit_memo: quote.deposit_memo.clone(),
        source_tx_hash: None,
        destination_tx_hash: None,
        approval_ref: None,
        status: PaymentAttemptStatus::Quoted,
    };
    repo.insert_attempt(attempt).await?;

    Ok(PreflightReport {
        attempt_id,
        quote,
        plan,
        simulation,
    })
}

/// Owner-approved broadcast. Transitions Quoted → Authorized → DepositBroadcast.
/// Re-running after a crash with the same approval_ref is idempotent.
pub async fn execute_approved<A, E, R>(
    adapter: &A,
    executor: &E,
    repo: &R,
    attempt_id: PaymentAttemptId,
    approval_ref: &str,
    max_fee_wei: BaseUnits,
) -> Result<alloy::primitives::B256, FlowError>
where
    A: OneClickAdapter + Sync,
    E: SourceExecutor + Sync,
    R: Repository + Sync,
{
    if approval_ref.trim().is_empty() {
        return Err(AdapterError::Rejected("empty approval_ref".into()).into());
    }
    let attempt = repo.get_attempt(attempt_id).await?;
    match attempt.status {
        PaymentAttemptStatus::Quoted => {
            repo.record_approval(attempt_id, approval_ref).await?;
            repo.transition_attempt(attempt_id, PaymentAttemptStatus::Authorized)
                .await?;
        }
        PaymentAttemptStatus::Authorized | PaymentAttemptStatus::DepositBroadcast => {
            repo.record_approval(attempt_id, approval_ref).await?;
        }
        other => return Err(FlowError::WrongState(attempt_id, other)),
    }

    let attempt = repo.get_attempt(attempt_id).await?;
    if let Some(hash) = attempt.source_tx_hash.as_deref() {
        // Already recorded; do not construct a new transaction.
        return hash
            .parse()
            .map_err(|_| AdapterError::Malformed("stored source_tx_hash is not 32 bytes".into()).into());
    }

    let deposit = attempt
        .deposit_address
        .as_deref()
        .ok_or_else(|| AdapterError::Rejected("attempt has no deposit address".into()))?;
    let to = deposit
        .parse()
        .map_err(|_| AdapterError::Rejected("deposit address is not an EVM address".into()))?;
    let plan = DepositPlan {
        attempt_id,
        chain_id: attempt.source_asset.chain_id,
        asset: attempt.source_asset.clone(),
        from: attempt.source_wallet,
        to,
        amount: attempt.quoted_input,
        max_fee_wei,
        approval_ref: approval_ref.to_owned(),
    };

    let hash = executor.execute_deposit(&plan).await?;
    repo.record_source_tx(attempt_id, &format!("{hash:#x}")).await?;
    if attempt.status == PaymentAttemptStatus::Authorized || attempt.status == PaymentAttemptStatus::Quoted {
        let latest = repo.get_attempt(attempt_id).await?;
        if latest.status == PaymentAttemptStatus::Authorized {
            repo.transition_attempt(attempt_id, PaymentAttemptStatus::DepositBroadcast)
                .await?;
        }
    }

    if let Err(e) = adapter.submit_deposit_tx(deposit, &format!("{hash:#x}")).await {
        tracing::warn!(attempt_id = %attempt_id, error = %e, "optional /deposit/submit failed; status polling will still observe the tx");
    }
    Ok(hash)
}

/// Bounded status poll. Never broadcasts.
pub async fn watch_until<A, R>(
    adapter: &A,
    repo: &R,
    attempt_id: PaymentAttemptId,
    max_polls: u32,
    mut now: DateTime<Utc>,
) -> Result<(PollOutcome, Option<SwapStatus>), FlowError>
where
    A: OneClickAdapter + Sync,
    R: Repository + Sync,
{
    let mut last_status = None;
    let mut outcome = PollOutcome::Retry {
        reason: "not started".into(),
        next_poll_at: now,
    };
    for i in 0..max_polls {
        outcome = poll_once(repo, adapter, attempt_id, i, now).await?;
        let attempt = repo.get_attempt(attempt_id).await?;
        if let Some(dep) = attempt.deposit_address.as_deref() {
            if let Ok(s) = adapter.status(dep, attempt.deposit_memo.as_deref()).await {
                last_status = Some(s);
            }
        }
        match &outcome {
            PollOutcome::Done(_) => break,
            PollOutcome::Continue { next_poll_at, .. } | PollOutcome::Retry { next_poll_at, .. } => {
                now = *next_poll_at;
            }
        }
    }
    Ok((outcome, last_status))
}

/// Redacted preflight suitable for printing / owner approval. Never includes
/// raw quote JSON (which is large) or anything secret.
#[must_use]
pub fn redacted_preflight(p: &PreflightReport) -> serde_json::Value {
    serde_json::json!({
        "attemptId": p.attempt_id.to_string(),
        "dry": p.quote.dry,
        "swapType": p.quote.swap_type,
        "confidentiality": p.quote.confidentiality,
        "originAssetId": p.quote.origin_asset_id,
        "destinationAssetId": p.quote.destination_asset_id,
        "recipient": p.quote.recipient,
        "refundTo": p.quote.refund_to,
        "amountIn": p.quote.amount_in,
        "minAmountIn": p.quote.min_amount_in,
        "amountOut": p.quote.amount_out,
        "depositAddress": p.quote.deposit_address,
        "deadline": p.quote.deadline,
        "simulationOk": p.simulation.ok,
        "gasEstimate": p.simulation.gas_estimate,
        "maxFeeWei": p.simulation.max_fee_wei,
        "simulationNotes": p.simulation.notes,
        "plan": {
            "chainId": p.plan.chain_id.0,
            "from": p.plan.from.to_string(),
            "to": p.plan.to.to_string(),
            "amount": p.plan.amount,
        }
    })
}
