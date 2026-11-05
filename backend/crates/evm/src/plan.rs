//! Build a [`DepositPlan`] from a validated executable quote.

use crate::transfer::build_deposit_call;
use alloy::primitives::Address;
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::AssetId;
use pcc_payments::domain::PaymentAttemptId;
use pcc_payments::quote::{DepositMode, VerifiedQuote};
use pcc_payments::traits::{AdapterError, DepositPlan};

/// Construct the exact on-chain deposit described by `quote`.
///
/// EXACT_OUTPUT quotes include a slippage buffer in `amount_in`; we send
/// `amount_in` (not `min_amount_in`). 1Click refunds any unused buffer to
/// `refundTo`. MEMO deposits are refused: a plain EOA ERC-20/native transfer
/// cannot attach a memo.
pub fn plan_from_quote(
    attempt_id: PaymentAttemptId,
    quote: &VerifiedQuote,
    source_asset: &AssetId,
    from: Address,
    max_fee_wei: BaseUnits,
    approval_ref: String,
) -> Result<DepositPlan, AdapterError> {
    if quote.dry {
        return Err(AdapterError::Rejected("refusing to plan a dry quote".into()));
    }
    if quote.deposit_mode == DepositMode::Memo {
        return Err(AdapterError::Rejected(
            "MEMO deposits cannot be sent as a plain EOA transfer; use a memo-capable executor".into(),
        ));
    }
    if quote.amount_in < quote.min_amount_in {
        return Err(AdapterError::Rejected("amountIn is below minAmountIn".into()));
    }
    let deposit = quote
        .deposit_address
        .as_deref()
        .ok_or_else(|| AdapterError::Rejected("executable quote has no deposit address".into()))?;
    let to: Address = deposit
        .parse()
        .map_err(|_| AdapterError::Rejected("deposit address is not a valid EVM address".into()))?;

    let plan = DepositPlan {
        attempt_id,
        chain_id: source_asset.chain_id,
        asset: source_asset.clone(),
        from,
        to,
        amount: quote.amount_in,
        max_fee_wei,
        approval_ref,
    };
    // Fail closed: if we cannot even *build* the call, we must not proceed.
    build_deposit_call(&plan).map_err(|e| AdapterError::Rejected(e.to_string()))?;
    Ok(plan)
}
