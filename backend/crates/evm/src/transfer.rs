//! Build the exact on-chain call for a deposit: native value transfer or
//! ERC-20 `transfer(address,uint256)`. No `approve` is ever generated here.

use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{sol, SolCall};
use pcc_payments::asset::TokenRef;
use pcc_payments::traits::DepositPlan;

sol! {
    /// Minimal ERC-20 surface we are allowed to call.
    interface IERC20 {
        function transfer(address to, uint256 amount) external returns (bool);
        function balanceOf(address owner) external view returns (uint256);
    }
}

/// A fully specified call the executor will sign. `value` is in wei.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositCall {
    pub to: Address,
    pub value: U256,
    pub data: Bytes,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CallError {
    #[error("deposit amount is zero")]
    ZeroAmount,
    #[error("deposit destination is the zero address")]
    ZeroDestination,
    #[error("asset chain {asset_chain} does not match plan chain {plan_chain}")]
    ChainMismatch { asset_chain: u64, plan_chain: u64 },
}

/// Build the deposit call from a validated plan.
pub fn build_deposit_call(plan: &DepositPlan) -> Result<DepositCall, CallError> {
    if plan.amount.is_zero() {
        return Err(CallError::ZeroAmount);
    }
    if plan.to == Address::ZERO {
        return Err(CallError::ZeroDestination);
    }
    if plan.asset.chain_id != plan.chain_id {
        return Err(CallError::ChainMismatch {
            asset_chain: plan.asset.chain_id.0,
            plan_chain: plan.chain_id.0,
        });
    }
    Ok(match plan.asset.token {
        TokenRef::Native => DepositCall {
            to: plan.to,
            value: plan.amount.as_u256(),
            data: Bytes::new(),
        },
        TokenRef::Erc20 { address } => DepositCall {
            to: address,
            value: U256::ZERO,
            data: encode_erc20_transfer(plan.to, plan.amount.as_u256()),
        },
    })
}

#[must_use]
pub fn encode_erc20_transfer(to: Address, amount: U256) -> Bytes {
    IERC20::transferCall { to, amount }.abi_encode().into()
}

/// Decode ERC-20 `transfer` calldata (used by tests and by the executor's
/// self-check that what it is about to sign is exactly what was planned).
pub fn decode_erc20_transfer(data: &[u8]) -> Result<(Address, U256), alloy_sol_types::Error> {
    let call = IERC20::transferCall::abi_decode(data)?;
    Ok((call.to, call.amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_payments::amount::BaseUnits;
    use pcc_payments::asset::{AssetId, ChainId};
    use pcc_payments::domain::PaymentAttemptId;

    const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const DEPOSIT: &str = "0x76b4c56085ED136a8744D52bE956396624a730E8";

    fn plan(token: TokenRef) -> DepositPlan {
        DepositPlan {
            attempt_id: PaymentAttemptId::new(),
            chain_id: ChainId::BASE,
            asset: AssetId {
                chain_id: ChainId::BASE,
                token,
                decimals: 6,
                one_click_asset_id: "nep141:base-usdc".into(),
            },
            from: "0x2222222222222222222222222222222222222222".parse().unwrap(),
            to: DEPOSIT.parse().unwrap(),
            amount: BaseUnits::from_u128(1_020_000),
            max_fee_wei: BaseUnits::from_u128(50_000_000_000_000),
            approval_ref: "appr-1".into(),
        }
    }

    #[test]
    fn erc20_transfer_targets_token_contract_with_zero_value() {
        let p = plan(TokenRef::Erc20 {
            address: USDC_BASE.parse().unwrap(),
        });
        let call = build_deposit_call(&p).unwrap();
        assert_eq!(call.to, USDC_BASE.parse::<Address>().unwrap());
        assert_eq!(call.value, U256::ZERO);
        // selector a9059cbb = transfer(address,uint256)
        assert_eq!(&call.data[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(call.data.len(), 4 + 32 + 32);
        let (to, amount) = decode_erc20_transfer(&call.data).unwrap();
        assert_eq!(to, DEPOSIT.parse::<Address>().unwrap());
        assert_eq!(amount, U256::from(1_020_000u64));
    }

    #[test]
    fn native_transfer_targets_deposit_address_with_value() {
        let p = plan(TokenRef::Native);
        let call = build_deposit_call(&p).unwrap();
        assert_eq!(call.to, DEPOSIT.parse::<Address>().unwrap());
        assert_eq!(call.value, U256::from(1_020_000u64));
        assert!(call.data.is_empty());
    }

    #[test]
    fn rejects_zero_amount_zero_destination_and_chain_mismatch() {
        let mut p = plan(TokenRef::Native);
        p.amount = BaseUnits::ZERO;
        assert_eq!(build_deposit_call(&p), Err(CallError::ZeroAmount));

        let mut p = plan(TokenRef::Native);
        p.to = Address::ZERO;
        assert_eq!(build_deposit_call(&p), Err(CallError::ZeroDestination));

        let mut p = plan(TokenRef::Native);
        p.chain_id = ChainId::ARBITRUM;
        assert!(matches!(build_deposit_call(&p), Err(CallError::ChainMismatch { .. })));
    }

    #[test]
    fn never_emits_approve_selector() {
        let p = plan(TokenRef::Erc20 {
            address: USDC_BASE.parse().unwrap(),
        });
        let call = build_deposit_call(&p).unwrap();
        assert_ne!(
            &call.data[..4],
            &[0x09, 0x5e, 0xa7, 0xb3],
            "approve(address,uint256) must never be built"
        );
    }
}
