//! M4 checkout flow with mocked 1Click + mocked session executor. No Anvil,
//! no live money. Asserts the deposit plan is bound to the quote address
//! and that a second execute is idempotent.

use crate::flow::{execute_approved, prepare_deposit, PrepareParams};
use chrono::{Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::{MerchantId, PaymentIntent, PaymentIntentId, PayoutConfig};
use pcc_payments::mock::{MockOneClick, MockSourceExecutor};
use pcc_payments::status::PaymentAttemptStatus;
use pcc_payments::traits::QuoteSpec;
use pcc_storage::InMemoryRepository;
use pcc_storage::Repository;

fn usdc_base() -> AssetId {
    AssetId {
        chain_id: ChainId::BASE,
        token: TokenRef::Erc20 {
            address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
        },
        decimals: 6,
        one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
    }
}

fn usdc_arb() -> AssetId {
    AssetId {
        chain_id: ChainId::ARBITRUM,
        token: TokenRef::Erc20 {
            address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
        },
        decimals: 6,
        one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
    }
}

#[tokio::test]
async fn mock_session_flow_binds_quote_deposit_and_is_idempotent() {
    let adapter = MockOneClick::default();
    let exec = MockSourceExecutor::default();
    let repo = InMemoryRepository::default();
    let merchant = MerchantId::new();
    let dest = usdc_arb();
    let source = usdc_base();
    let from = "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap();
    let intent = PaymentIntent {
        id: PaymentIntentId::new(),
        merchant_id: merchant,
        merchant_order_id: "m4".into(),
        payout_config_snapshot: PayoutConfig {
            id: pcc_payments::domain::PayoutConfigId::new(),
            merchant_id: merchant,
            asset: dest.clone(),
            receiving_address: "0x1111111111111111111111111111111111111111".parse().unwrap(),
            active: true,
        },
        output_amount_base_units: BaseUnits::from_u128(1_000_000),
        status: pcc_payments::status::PaymentIntentStatus::RequiresPayment,
        expires_at: Utc::now() + Duration::hours(1),
        idempotency_key: "m4-1".into(),
    };
    let spec = QuoteSpec {
        source_asset: source.clone(),
        destination_asset: dest,
        output_amount: intent.output_amount_base_units,
        recipient: intent.payout_config_snapshot.receiving_address,
        refund_to: from,
        confidentiality: pcc_payments::quote::Confidentiality::Basic,
        slippage_bps: 50,
        deadline: Utc::now() + Duration::hours(1),
        authorization_ref: "auth-ref-m4-flow-01".into(),
    };

    let pre = prepare_deposit(
        &adapter,
        &exec,
        &repo,
        PrepareParams {
            intent,
            source_asset: source,
            spec,
            max_debit: BaseUnits::from_u128(1_050_000),
            max_fee_wei: BaseUnits::from_u128(10u128.pow(16)),
            max_slippage_bps: 100,
            now: Utc::now(),
        },
    )
    .await
    .unwrap();
    assert!(pre.simulation.ok);
    crate::plan::assert_plan_matches_quote(&pre.plan, &pre.quote).unwrap();

    let hash = execute_approved(
        &adapter,
        &exec,
        &repo,
        pre.attempt_id,
        "owner-appr-m4",
        BaseUnits::from_u128(10u128.pow(16)),
    )
    .await
    .unwrap();
    let last = exec.last_plan.lock().unwrap().clone().expect("executed");
    let deposit = pre.quote.deposit_address.as_deref().unwrap();
    assert_eq!(last.to, deposit.parse::<alloy::primitives::Address>().unwrap());
    assert_eq!(last.amount, pre.quote.amount_in);

    let stored = repo.get_attempt(pre.attempt_id).await.unwrap();
    assert_eq!(stored.status, PaymentAttemptStatus::DepositBroadcast);

    let hash2 = execute_approved(
        &adapter,
        &exec,
        &repo,
        pre.attempt_id,
        "owner-appr-m4",
        BaseUnits::from_u128(10u128.pow(16)),
    )
    .await
    .unwrap();
    assert_eq!(hash, hash2);
}
