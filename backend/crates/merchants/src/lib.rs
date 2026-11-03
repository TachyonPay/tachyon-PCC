//! Merchant payout configuration and tenancy rules.

use alloy_primitives::Address;
use chrono::{DateTime, Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::AssetId;
use pcc_payments::domain::{MerchantId, PaymentIntent, PaymentIntentId, PayoutConfig, PayoutConfigId};
use pcc_payments::status::PaymentIntentStatus;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MerchantError {
    #[error("merchant {actor} does not own resource belonging to {owner}")]
    Forbidden { actor: MerchantId, owner: MerchantId },
    #[error("receiving address must not be the zero address")]
    ZeroReceivingAddress,
    #[error("payout asset chain is not an accepted mainnet candidate")]
    UnsupportedPayoutChain,
    #[error("payout config is inactive")]
    InactivePayout,
    #[error("output amount must be greater than zero")]
    ZeroOutput,
    #[error("idempotency key must be 1..=255 chars")]
    BadIdempotencyKey,
}

/// Tenant check used by every merchant-scoped handler.
pub fn assert_owner(actor: MerchantId, owner: MerchantId) -> Result<(), MerchantError> {
    if actor == owner {
        Ok(())
    } else {
        Err(MerchantError::Forbidden { actor, owner })
    }
}

/// Validate and construct a payout configuration.
pub fn new_payout_config(
    merchant_id: MerchantId,
    asset: AssetId,
    receiving_address: Address,
) -> Result<PayoutConfig, MerchantError> {
    if receiving_address == Address::ZERO {
        return Err(MerchantError::ZeroReceivingAddress);
    }
    if !asset.chain_id.is_mainnet_candidate() {
        return Err(MerchantError::UnsupportedPayoutChain);
    }
    Ok(PayoutConfig {
        id: PayoutConfigId::new(),
        merchant_id,
        asset,
        receiving_address,
        active: true,
    })
}

/// Create a payment intent with an immutable snapshot of the payout config.
/// Later edits to the merchant's payout settings never affect this intent.
pub fn new_payment_intent(
    actor: MerchantId,
    payout: &PayoutConfig,
    merchant_order_id: &str,
    output_amount: BaseUnits,
    idempotency_key: &str,
    now: DateTime<Utc>,
    ttl: Duration,
) -> Result<PaymentIntent, MerchantError> {
    assert_owner(actor, payout.merchant_id)?;
    if !payout.active {
        return Err(MerchantError::InactivePayout);
    }
    if output_amount.is_zero() {
        return Err(MerchantError::ZeroOutput);
    }
    if idempotency_key.is_empty() || idempotency_key.len() > 255 {
        return Err(MerchantError::BadIdempotencyKey);
    }
    Ok(PaymentIntent {
        id: PaymentIntentId::new(),
        merchant_id: payout.merchant_id,
        merchant_order_id: merchant_order_id.to_owned(),
        payout_config_snapshot: payout.clone(),
        output_amount_base_units: output_amount,
        status: PaymentIntentStatus::RequiresPayment,
        expires_at: now + ttl,
        idempotency_key: idempotency_key.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_payments::asset::{ChainId, TokenRef};

    fn usdc_arb() -> AssetId {
        AssetId {
            chain_id: ChainId::ARBITRUM,
            token: TokenRef::Erc20 {
                address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
            },
            decimals: 6,
            one_click_asset_id: "nep141:arb-usdc".into(),
        }
    }

    #[test]
    fn tenant_isolation() {
        let a = MerchantId::new();
        let b = MerchantId::new();
        assert!(assert_owner(a, a).is_ok());
        assert!(matches!(assert_owner(a, b), Err(MerchantError::Forbidden { .. })));
    }

    #[test]
    fn payout_config_validation() {
        let m = MerchantId::new();
        assert_eq!(
            new_payout_config(m, usdc_arb(), Address::ZERO).unwrap_err(),
            MerchantError::ZeroReceivingAddress
        );
        let mut sepolia = usdc_arb();
        sepolia.chain_id = ChainId::SEPOLIA;
        assert_eq!(
            new_payout_config(m, sepolia, Address::from([1u8; 20])).unwrap_err(),
            MerchantError::UnsupportedPayoutChain
        );
        assert!(new_payout_config(m, usdc_arb(), Address::from([1u8; 20])).is_ok());
    }

    #[test]
    fn intent_snapshot_is_immune_to_later_payout_edits() {
        let m = MerchantId::new();
        let mut payout = new_payout_config(m, usdc_arb(), Address::from([1u8; 20])).unwrap();
        let intent = new_payment_intent(
            m,
            &payout,
            "order-1",
            BaseUnits::from_u128(1_000_000),
            "idem-1",
            Utc::now(),
            Duration::minutes(30),
        )
        .unwrap();
        payout.receiving_address = Address::from([9u8; 20]);
        payout.active = false;
        assert_eq!(
            intent.payout_config_snapshot.receiving_address,
            Address::from([1u8; 20])
        );
        assert!(intent.payout_config_snapshot.active);
    }

    #[test]
    fn intent_rejects_other_tenant_inactive_zero_and_bad_idempotency() {
        let m = MerchantId::new();
        let other = MerchantId::new();
        let payout = new_payout_config(m, usdc_arb(), Address::from([1u8; 20])).unwrap();
        let now = Utc::now();
        let ttl = Duration::minutes(30);
        assert!(matches!(
            new_payment_intent(other, &payout, "o", BaseUnits::from_u128(1), "k", now, ttl),
            Err(MerchantError::Forbidden { .. })
        ));
        let mut inactive = payout.clone();
        inactive.active = false;
        assert_eq!(
            new_payment_intent(m, &inactive, "o", BaseUnits::from_u128(1), "k", now, ttl).unwrap_err(),
            MerchantError::InactivePayout
        );
        assert_eq!(
            new_payment_intent(m, &payout, "o", BaseUnits::ZERO, "k", now, ttl).unwrap_err(),
            MerchantError::ZeroOutput
        );
        assert_eq!(
            new_payment_intent(m, &payout, "o", BaseUnits::from_u128(1), "", now, ttl).unwrap_err(),
            MerchantError::BadIdempotencyKey
        );
    }
}
