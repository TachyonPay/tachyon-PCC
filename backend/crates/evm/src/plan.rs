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
        mandate: None,
    };
    // Fail closed: if we cannot even *build* the call, we must not proceed.
    build_deposit_call(&plan).map_err(|e| AdapterError::Rejected(e.to_string()))?;
    Ok(plan)
}

/// Refuse a plan whose `to` / `amount` do not match the verified quote.
/// Used by tests and by any path that still has the quote in hand.
pub fn assert_plan_matches_quote(plan: &DepositPlan, quote: &VerifiedQuote) -> Result<(), AdapterError> {
    let expected = quote
        .deposit_address
        .as_deref()
        .ok_or_else(|| AdapterError::Rejected("quote has no deposit address".into()))?;
    let expected_addr: Address = expected
        .parse()
        .map_err(|_| AdapterError::Rejected("quote deposit address is not a valid EVM address".into()))?;
    if plan.to != expected_addr {
        return Err(AdapterError::Rejected(
            "quote address substitution: plan.to is not the quote deposit address".into(),
        ));
    }
    if plan.amount != quote.amount_in {
        return Err(AdapterError::Rejected("plan amount is not quote amountIn".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pcc_payments::asset::{ChainId, TokenRef};
    use pcc_payments::quote::{Confidentiality, DepositMode, SwapType};

    fn source() -> AssetId {
        AssetId {
            chain_id: ChainId::BASE,
            token: TokenRef::Erc20 {
                address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
            },
            decimals: 6,
            one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
        }
    }

    fn quote(deposit: &str) -> VerifiedQuote {
        VerifiedQuote {
            correlation_id: "c".into(),
            signature: "s".into(),
            signature_verified: true,
            dry: false,
            swap_type: SwapType::ExactOutput,
            confidentiality: Confidentiality::Basic,
            deposit_mode: DepositMode::Simple,
            origin_asset_id: "o".into(),
            destination_asset_id: "d".into(),
            recipient: "0x1111111111111111111111111111111111111111".into(),
            refund_to: "0x2222222222222222222222222222222222222222".into(),
            amount_in: BaseUnits::from_u128(1_020_000),
            min_amount_in: BaseUnits::from_u128(1_010_000),
            amount_out: BaseUnits::from_u128(1_000_000),
            min_amount_out: BaseUnits::from_u128(1_000_000),
            deposit_address: Some(deposit.into()),
            deposit_memo: None,
            quote_timestamp: Utc::now(),
            deadline: Some(Utc::now()),
            time_estimate_seconds: 1,
            raw_response_json: "{}".into(),
        }
    }

    #[test]
    fn plan_binds_quote_deposit_address() {
        const DEPOSIT: &str = "0x76b4c56085ED136a8744D52bE956396624a730E8";
        let q = quote(DEPOSIT);
        let from: Address = "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap();
        let plan = plan_from_quote(
            PaymentAttemptId::new(),
            &q,
            &source(),
            from,
            BaseUnits::from_u128(1),
            "appr".into(),
        )
        .unwrap();
        assert_plan_matches_quote(&plan, &q).unwrap();
        assert_eq!(plan.to, DEPOSIT.parse::<Address>().unwrap());
        assert_eq!(plan.amount, q.amount_in);
    }

    #[test]
    fn substituted_recipient_is_rejected() {
        const DEPOSIT: &str = "0x76b4c56085ED136a8744D52bE956396624a730E8";
        let q = quote(DEPOSIT);
        let from: Address = "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap();
        let mut plan = plan_from_quote(
            PaymentAttemptId::new(),
            &q,
            &source(),
            from,
            BaseUnits::from_u128(1),
            "appr".into(),
        )
        .unwrap();
        plan.to = "0x3333333333333333333333333333333333333333".parse().unwrap();
        let err = assert_plan_matches_quote(&plan, &q).unwrap_err();
        assert!(err.to_string().contains("substitution"), "{err}");
    }
}
