//! Quote business rules.
//!
//! The TS worker verifies the 1Click Ed25519 signature with the official
//! `verifyQuoteSignature()` and returns a `VerifiedQuote`. Rust then applies
//! these rules independently: nothing is executed on the basis of the TS
//! worker's word alone.

use crate::amount::BaseUnits;
use crate::asset::AssetId;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 1Click `swapType`. Only `EXACT_OUTPUT` is accepted for payments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwapType {
    ExactInput,
    ExactOutput,
    FlexInput,
    AnyInput,
}

/// 1Click `confidentiality`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidentiality {
    Public,
    Basic,
    Advanced,
}

/// 1Click `depositMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DepositMode {
    Simple,
    Memo,
}

/// A 1Click quote after the TS worker verified its signature.
///
/// All amounts are base-unit strings. `raw_response_json` is the exact bytes
/// returned by 1Click (stored for disputes; never mutated).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VerifiedQuote {
    pub correlation_id: String,
    pub signature: String,
    pub signature_verified: bool,
    pub dry: bool,
    pub swap_type: SwapType,
    pub confidentiality: Confidentiality,
    pub deposit_mode: DepositMode,
    pub origin_asset_id: String,
    pub destination_asset_id: String,
    pub recipient: String,
    pub refund_to: String,
    pub amount_in: BaseUnits,
    pub min_amount_in: BaseUnits,
    pub amount_out: BaseUnits,
    pub min_amount_out: BaseUnits,
    pub deposit_address: Option<String>,
    pub deposit_memo: Option<String>,
    pub quote_timestamp: DateTime<Utc>,
    /// `quote.deadline` (absent for dry quotes).
    pub deadline: Option<DateTime<Utc>>,
    pub time_estimate_seconds: u64,
    pub raw_response_json: String,
}

/// What the Rust business layer expects the quote to be for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteExpectation {
    pub source_asset: AssetId,
    pub destination_asset: AssetId,
    /// Merchant payout address from the immutable snapshot.
    pub recipient: String,
    /// Payer's refund address (wallet A on the origin chain).
    pub refund_to: String,
    /// Exact merchant output in destination base units.
    pub output_amount: BaseUnits,
    /// Maximum the payer authorized to be debited (origin base units).
    pub max_debit: BaseUnits,
    /// Reject if `amount_in` exceeds `min_amount_in` by more than this (bps).
    pub max_slippage_bps: u32,
    pub require_confidentiality: Confidentiality,
    pub require_executable: bool,
    /// Minimum remaining validity required at execution time.
    pub min_remaining_validity: Duration,
    /// Maximum age of the quote timestamp (replay protection).
    pub max_quote_age: Duration,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QuoteViolation {
    #[error("quote signature was not verified")]
    SignatureNotVerified,
    #[error("quote is dry but an executable quote is required")]
    DryQuote,
    #[error("swapType must be EXACT_OUTPUT, got {0:?}")]
    WrongSwapType(SwapType),
    #[error("confidentiality {got:?} does not satisfy required {required:?}")]
    Confidentiality {
        got: Confidentiality,
        required: Confidentiality,
    },
    #[error("origin asset mismatch: expected {expected}, got {got}")]
    OriginAsset { expected: String, got: String },
    #[error("destination asset mismatch: expected {expected}, got {got}")]
    DestinationAsset { expected: String, got: String },
    #[error("recipient mismatch")]
    Recipient,
    #[error("refund address mismatch")]
    RefundTo,
    #[error("amountOut {got} != required output {expected}")]
    OutputAmount { expected: BaseUnits, got: BaseUnits },
    #[error("minAmountOut {0} is below required output")]
    MinOutputBelowRequired(BaseUnits),
    #[error("amountIn {amount_in} exceeds authorized max debit {max_debit}")]
    ExceedsMaxDebit { amount_in: BaseUnits, max_debit: BaseUnits },
    #[error("zero amount in quote")]
    ZeroAmount,
    #[error("slippage buffer too large: amountIn {amount_in} vs minAmountIn {min_amount_in} (cap {cap_bps} bps)")]
    ExcessiveSlippage {
        amount_in: BaseUnits,
        min_amount_in: BaseUnits,
        cap_bps: u32,
    },
    #[error("executable quote is missing a deposit address")]
    MissingDepositAddress,
    #[error("deposit mode MEMO requires a deposit memo")]
    MissingDepositMemo,
    #[error("deposit address is not a valid EVM address for the origin chain")]
    InvalidDepositAddress,
    #[error("quote deadline missing on executable quote")]
    MissingDeadline,
    #[error("quote expires too soon (deadline {deadline})")]
    ExpiresTooSoon { deadline: DateTime<Utc> },
    #[error("quote timestamp {timestamp} is stale or in the future")]
    StaleTimestamp { timestamp: DateTime<Utc> },
}

/// Validate a verified quote against the business expectation at `now`.
/// Returns every violation found so callers can log all of them at once.
pub fn validate_quote(
    q: &VerifiedQuote,
    exp: &QuoteExpectation,
    now: DateTime<Utc>,
) -> Result<(), Vec<QuoteViolation>> {
    let mut v = Vec::new();

    if !q.signature_verified {
        v.push(QuoteViolation::SignatureNotVerified);
    }
    if exp.require_executable && q.dry {
        v.push(QuoteViolation::DryQuote);
    }
    if q.swap_type != SwapType::ExactOutput {
        v.push(QuoteViolation::WrongSwapType(q.swap_type));
    }
    if !confidentiality_satisfies(q.confidentiality, exp.require_confidentiality) {
        v.push(QuoteViolation::Confidentiality {
            got: q.confidentiality,
            required: exp.require_confidentiality,
        });
    }
    if q.origin_asset_id != exp.source_asset.one_click_asset_id {
        v.push(QuoteViolation::OriginAsset {
            expected: exp.source_asset.one_click_asset_id.clone(),
            got: q.origin_asset_id.clone(),
        });
    }
    if q.destination_asset_id != exp.destination_asset.one_click_asset_id {
        v.push(QuoteViolation::DestinationAsset {
            expected: exp.destination_asset.one_click_asset_id.clone(),
            got: q.destination_asset_id.clone(),
        });
    }
    if !eq_evm_address(&q.recipient, &exp.recipient) {
        v.push(QuoteViolation::Recipient);
    }
    if !eq_evm_address(&q.refund_to, &exp.refund_to) {
        v.push(QuoteViolation::RefundTo);
    }
    if q.amount_in.is_zero() || q.amount_out.is_zero() || exp.output_amount.is_zero() {
        v.push(QuoteViolation::ZeroAmount);
    }
    if q.amount_out != exp.output_amount {
        v.push(QuoteViolation::OutputAmount {
            expected: exp.output_amount,
            got: q.amount_out,
        });
    }
    if q.min_amount_out < exp.output_amount {
        v.push(QuoteViolation::MinOutputBelowRequired(q.min_amount_out));
    }
    if q.amount_in > exp.max_debit {
        v.push(QuoteViolation::ExceedsMaxDebit {
            amount_in: q.amount_in,
            max_debit: exp.max_debit,
        });
    }
    // Slippage buffer check: amount_in <= min_amount_in * (1 + cap).
    if let Ok(cap) = q.min_amount_in.bps_ceil(exp.max_slippage_bps) {
        if let Ok(limit) = q.min_amount_in.checked_add(cap) {
            if q.amount_in > limit {
                v.push(QuoteViolation::ExcessiveSlippage {
                    amount_in: q.amount_in,
                    min_amount_in: q.min_amount_in,
                    cap_bps: exp.max_slippage_bps,
                });
            }
        }
    }

    if !q.dry {
        match &q.deposit_address {
            None => v.push(QuoteViolation::MissingDepositAddress),
            Some(addr) => {
                if addr.parse::<alloy_primitives::Address>().is_err() {
                    v.push(QuoteViolation::InvalidDepositAddress);
                }
            }
        }
        if q.deposit_mode == DepositMode::Memo && q.deposit_memo.as_deref().is_none_or(str::is_empty) {
            v.push(QuoteViolation::MissingDepositMemo);
        }
        match q.deadline {
            None => v.push(QuoteViolation::MissingDeadline),
            Some(deadline) => {
                if deadline - now < exp.min_remaining_validity {
                    v.push(QuoteViolation::ExpiresTooSoon { deadline });
                }
            }
        }
    }

    let age = now - q.quote_timestamp;
    if age > exp.max_quote_age || age < Duration::seconds(-60) {
        v.push(QuoteViolation::StaleTimestamp {
            timestamp: q.quote_timestamp,
        });
    }

    if v.is_empty() {
        Ok(())
    } else {
        Err(v)
    }
}

fn confidentiality_satisfies(got: Confidentiality, required: Confidentiality) -> bool {
    use Confidentiality as C;
    match required {
        C::Public => true,
        C::Basic => matches!(got, C::Basic | C::Advanced),
        C::Advanced => got == C::Advanced,
    }
}

fn eq_evm_address(a: &str, b: &str) -> bool {
    match (
        a.parse::<alloy_primitives::Address>(),
        b.parse::<alloy_primitives::Address>(),
    ) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{ChainId, TokenRef};

    const USDC_BASE: &str = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
    const USDC_ARB: &str = "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";
    const RECIPIENT: &str = "0x1111111111111111111111111111111111111111";
    const REFUND: &str = "0x2222222222222222222222222222222222222222";
    const DEPOSIT: &str = "0x76b4c56085ED136a8744D52bE956396624a730E8";

    fn now() -> DateTime<Utc> {
        "2026-11-03T12:00:00Z".parse().unwrap()
    }

    fn asset(chain: ChainId, addr: &str, id: &str) -> AssetId {
        AssetId {
            chain_id: chain,
            token: TokenRef::Erc20 {
                address: addr.parse().unwrap(),
            },
            decimals: 6,
            one_click_asset_id: id.into(),
        }
    }

    fn expectation() -> QuoteExpectation {
        QuoteExpectation {
            source_asset: asset(ChainId::BASE, "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", USDC_BASE),
            destination_asset: asset(
                ChainId::ARBITRUM,
                "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
                USDC_ARB,
            ),
            recipient: RECIPIENT.into(),
            refund_to: REFUND.into(),
            output_amount: BaseUnits::from_u128(1_000_000),
            max_debit: BaseUnits::from_u128(1_050_000),
            max_slippage_bps: 100,
            require_confidentiality: Confidentiality::Basic,
            require_executable: true,
            min_remaining_validity: Duration::minutes(10),
            max_quote_age: Duration::minutes(5),
        }
    }

    fn good_quote() -> VerifiedQuote {
        VerifiedQuote {
            correlation_id: "corr".into(),
            signature: "ed25519:sig".into(),
            signature_verified: true,
            dry: false,
            swap_type: SwapType::ExactOutput,
            confidentiality: Confidentiality::Basic,
            deposit_mode: DepositMode::Simple,
            origin_asset_id: USDC_BASE.into(),
            destination_asset_id: USDC_ARB.into(),
            recipient: RECIPIENT.into(),
            refund_to: REFUND.into(),
            amount_in: BaseUnits::from_u128(1_020_000),
            min_amount_in: BaseUnits::from_u128(1_012_000),
            amount_out: BaseUnits::from_u128(1_000_000),
            min_amount_out: BaseUnits::from_u128(1_000_000),
            deposit_address: Some(DEPOSIT.into()),
            deposit_memo: None,
            quote_timestamp: now() - Duration::seconds(30),
            deadline: Some(now() + Duration::hours(2)),
            time_estimate_seconds: 120,
            raw_response_json: "{}".into(),
        }
    }

    fn only(q: &VerifiedQuote, pred: impl Fn(&QuoteViolation) -> bool) {
        let errs = validate_quote(q, &expectation(), now()).unwrap_err();
        assert_eq!(errs.len(), 1, "expected exactly one violation, got {errs:?}");
        assert!(pred(&errs[0]), "unexpected violation {errs:?}");
    }

    #[test]
    fn accepts_a_good_confidential_exact_output_quote() {
        validate_quote(&good_quote(), &expectation(), now()).unwrap();
    }

    #[test]
    fn rejects_unverified_signature() {
        let mut q = good_quote();
        q.signature_verified = false;
        only(&q, |e| *e == QuoteViolation::SignatureNotVerified);
    }

    #[test]
    fn rejects_non_confidential_quote() {
        let mut q = good_quote();
        q.confidentiality = Confidentiality::Public;
        only(&q, |e| matches!(e, QuoteViolation::Confidentiality { .. }));
        // Advanced satisfies Basic.
        q.confidentiality = Confidentiality::Advanced;
        validate_quote(&q, &expectation(), now()).unwrap();
    }

    #[test]
    fn rejects_wrong_swap_type() {
        let mut q = good_quote();
        q.swap_type = SwapType::ExactInput;
        only(&q, |e| matches!(e, QuoteViolation::WrongSwapType(SwapType::ExactInput)));
    }

    #[test]
    fn rejects_reversed_chains() {
        let mut q = good_quote();
        q.origin_asset_id = USDC_ARB.into();
        q.destination_asset_id = USDC_BASE.into();
        let errs = validate_quote(&q, &expectation(), now()).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, QuoteViolation::OriginAsset { .. })));
        assert!(errs
            .iter()
            .any(|e| matches!(e, QuoteViolation::DestinationAsset { .. })));
    }

    #[test]
    fn rejects_recipient_substitution_and_refund_mismatch() {
        let mut q = good_quote();
        q.recipient = "0x3333333333333333333333333333333333333333".into();
        only(&q, |e| *e == QuoteViolation::Recipient);
        let mut q = good_quote();
        q.refund_to = "not-an-address".into();
        only(&q, |e| *e == QuoteViolation::RefundTo);
        // Case-insensitive address equality is fine.
        let mut q = good_quote();
        q.recipient = RECIPIENT.to_uppercase().replace("0X", "0x");
        validate_quote(&q, &expectation(), now()).unwrap();
    }

    #[test]
    fn rejects_wrong_output_amount_and_decimals_mistakes() {
        let mut q = good_quote();
        q.amount_out = BaseUnits::from_u128(1_000_000_000_000); // 18-dec style mistake
        q.min_amount_out = q.amount_out;
        only(&q, |e| matches!(e, QuoteViolation::OutputAmount { .. }));
    }

    #[test]
    fn rejects_zero_amount() {
        let mut q = good_quote();
        q.amount_in = BaseUnits::ZERO;
        q.min_amount_in = BaseUnits::ZERO;
        only(&q, |e| *e == QuoteViolation::ZeroAmount);
    }

    #[test]
    fn rejects_exceeding_max_debit() {
        let mut q = good_quote();
        q.amount_in = BaseUnits::from_u128(1_050_001);
        q.min_amount_in = BaseUnits::from_u128(1_050_001);
        only(&q, |e| matches!(e, QuoteViolation::ExceedsMaxDebit { .. }));
    }

    #[test]
    fn rejects_excessive_slippage_buffer() {
        let mut q = good_quote();
        q.min_amount_in = BaseUnits::from_u128(1_000_000);
        q.amount_in = BaseUnits::from_u128(1_020_000); // 2% > 1% cap
        only(&q, |e| matches!(e, QuoteViolation::ExcessiveSlippage { .. }));
    }

    #[test]
    fn rejects_dry_quote_when_executable_required() {
        let mut q = good_quote();
        q.dry = true;
        q.deposit_address = None;
        q.deadline = None;
        only(&q, |e| *e == QuoteViolation::DryQuote);
        // ... but a dry quote is acceptable for previews.
        let mut exp = expectation();
        exp.require_executable = false;
        validate_quote(&q, &exp, now()).unwrap();
    }

    #[test]
    fn rejects_missing_or_invalid_deposit_address() {
        let mut q = good_quote();
        q.deposit_address = None;
        only(&q, |e| *e == QuoteViolation::MissingDepositAddress);
        let mut q = good_quote();
        q.deposit_address = Some("bc1qxyz".into());
        only(&q, |e| *e == QuoteViolation::InvalidDepositAddress);
    }

    #[test]
    fn memo_mode_requires_memo() {
        let mut q = good_quote();
        q.deposit_mode = DepositMode::Memo;
        only(&q, |e| *e == QuoteViolation::MissingDepositMemo);
        q.deposit_memo = Some("1234".into());
        validate_quote(&q, &expectation(), now()).unwrap();
    }

    #[test]
    fn rejects_stale_or_soon_expiring_quotes() {
        let mut q = good_quote();
        q.deadline = Some(now() + Duration::minutes(5));
        only(&q, |e| matches!(e, QuoteViolation::ExpiresTooSoon { .. }));
        let mut q = good_quote();
        q.deadline = None;
        only(&q, |e| *e == QuoteViolation::MissingDeadline);
        let mut q = good_quote();
        q.quote_timestamp = now() - Duration::minutes(6);
        only(&q, |e| matches!(e, QuoteViolation::StaleTimestamp { .. }));
        let mut q = good_quote();
        q.quote_timestamp = now() + Duration::minutes(5);
        only(&q, |e| matches!(e, QuoteViolation::StaleTimestamp { .. }));
    }

    #[test]
    fn reports_all_violations_at_once() {
        let mut q = good_quote();
        q.signature_verified = false;
        q.confidentiality = Confidentiality::Public;
        q.recipient = REFUND.into();
        let errs = validate_quote(&q, &expectation(), now()).unwrap_err();
        assert_eq!(errs.len(), 3);
    }

    #[test]
    fn enums_serialize_like_the_api() {
        assert_eq!(
            serde_json::to_string(&SwapType::ExactOutput).unwrap(),
            "\"EXACT_OUTPUT\""
        );
        assert_eq!(serde_json::to_string(&Confidentiality::Basic).unwrap(), "\"basic\"");
        assert_eq!(serde_json::to_string(&DepositMode::Memo).unwrap(), "\"MEMO\"");
    }
}
