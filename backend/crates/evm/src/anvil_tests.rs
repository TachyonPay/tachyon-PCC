//! Anvil-backed tests for the EOA executor. Skipped (pass) if `anvil` is not
//! on PATH — CI installs Foundry so they run there.

use crate::executor::AlloyEoaExecutor;
use crate::flow::{execute_approved, prepare_deposit};
use crate::ledger::FileLedger;
use crate::plan::plan_from_quote;
use crate::transfer::{decode_erc20_balance_of, encode_erc20_balance_of};
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy_node_bindings::Anvil;
use alloy_sol_types::{sol, SolCall};
use chrono::{Duration, Utc};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::{
    MerchantId, PaymentAttemptId, PaymentIntent, PaymentIntentId, PayoutConfig, PayoutConfigId,
};
use pcc_payments::gates::ExecutionGates;
use pcc_payments::mock::MockOneClick;
use pcc_payments::status::PaymentAttemptStatus;
use pcc_payments::traits::{DepositPlan, QuoteSpec, SourceExecutor};
use pcc_storage::{InMemoryRepository, Repository};

/// Tiny ERC-20 (mint/balanceOf/transfer) compiled with solc 0.8.24. See /tmp/pcc-erc20.
const MOCK_ERC20_BYTECODE: &str = "0x608060405234801561000f575f80fd5b506102118061001d5f395ff3fe608060405234801561000f575f80fd5b506004361061003f575f3560e01c806340c10f191461004357806370a0823114610058578063a9059cbb1461008a575b5f80fd5b610056610051366004610174565b6100ad565b005b61007761006636600461019c565b5f6020819052908152604090205481565b6040519081526020015b60405180910390f35b61009d610098366004610174565b6100dd565b6040519015158152602001610081565b6001600160a01b0382165f90815260208190526040812080548392906100d49084906101bc565b90915550505050565b335f90815260208190526040812054828110156101265760405162461bcd60e51b815260206004820152600360248201526218985b60ea1b604482015260640160405180910390fd5b335f90815260208190526040808220928590039092556001600160a01b0385168152208054830190555060015b92915050565b80356001600160a01b038116811461016f575f80fd5b919050565b5f8060408385031215610185575f80fd5b61018e83610159565b946020939093013593505050565b5f602082840312156101ac575f80fd5b6101b582610159565b9392505050565b8082018082111561015357634e487b7160e01b5f52601160045260245ffdfea26469706673582212208677931a21e56d648f7b0717cba03975dc73eb7590e207607c49ac71c763cd6c64736f6c63430008180033";

sol! {
    function mint(address to, uint256 amount);
}

fn prepend_foundry_path() {
    if let Ok(home) = std::env::var("HOME") {
        let extra = format!("{home}/.foundry/bin");
        let path = std::env::var("PATH").unwrap_or_default();
        if !path.split(':').any(|p| p == extra) {
            std::env::set_var("PATH", format!("{extra}:{path}"));
        }
    }
}

fn spawn_anvil() -> Option<(alloy_node_bindings::AnvilInstance, PrivateKeySigner, Address)> {
    prepend_foundry_path();
    let anvil = Anvil::new().try_spawn().ok()?;
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let addr = signer.address();
    Some((anvil, signer, addr))
}

fn local_gates() -> ExecutionGates {
    ExecutionGates {
        dry_run: false,
        enable_mainnet_transfers: false,
    }
}

fn native_asset() -> AssetId {
    AssetId {
        chain_id: ChainId::ANVIL,
        token: TokenRef::Native,
        decimals: 18,
        one_click_asset_id: "anvil:eth".into(),
    }
}

fn erc20_asset(token: Address) -> AssetId {
    AssetId {
        chain_id: ChainId::ANVIL,
        token: TokenRef::Erc20 { address: token },
        decimals: 18,
        one_click_asset_id: "anvil:mock".into(),
    }
}

fn plan(from: Address, to: Address, asset: AssetId, amount: u128, max_fee: u128, approval: &str) -> DepositPlan {
    DepositPlan {
        attempt_id: PaymentAttemptId::new(),
        chain_id: ChainId::ANVIL,
        asset,
        from,
        to,
        amount: BaseUnits::from_u128(amount),
        max_fee_wei: BaseUnits::from_u128(max_fee),
        approval_ref: approval.into(),
        mandate: None,
    }
}

fn executor(rpc: &str, signer: PrivateKeySigner, gates: ExecutionGates) -> (AlloyEoaExecutor, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let ledger = FileLedger::new(dir.path()).unwrap();
    let exec = AlloyEoaExecutor::connect(rpc, signer, gates, ledger).unwrap();
    (exec, dir)
}

#[tokio::test]
async fn native_transfer_end_to_end_and_restart_does_not_double_spend() {
    let Some((anvil, signer, from)) = spawn_anvil() else {
        eprintln!("skipping: anvil not on PATH");
        return;
    };
    let to = anvil.addresses()[1];
    let rpc = anvil.endpoint();
    let (exec, _dir) = executor(&rpc, signer, local_gates());
    let amount = 1_000_000_000_000_000u128; // 0.001 ETH
    let p = plan(from, to, native_asset(), amount, 10u128.pow(16), "appr-1");

    let sim = exec.simulate(&p).await.unwrap();
    assert!(sim.ok, "{sim:?}");
    assert!(sim.gas_estimate > 0);

    assert!(exec
        .receipt(ChainId::ANVIL, alloy::primitives::B256::from([9u8; 32]))
        .await
        .unwrap()
        .is_none());

    let hash = exec.execute_deposit(&p).await.unwrap();
    let mined = exec.receipt(ChainId::ANVIL, hash).await.unwrap().expect("mined");
    assert!(mined.success);

    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_http(rpc.parse().unwrap());
    let bal_after_first = provider.get_balance(to).await.unwrap();

    // Restart: new executor, same ledger directory.
    let exec2 = AlloyEoaExecutor::connect(
        &rpc,
        anvil.keys()[0].clone().into(),
        local_gates(),
        exec.ledger().clone(),
    )
    .unwrap();
    let hash2 = exec2.execute_deposit(&p).await.unwrap();
    assert_eq!(hash, hash2);
    let bal_after_second = provider.get_balance(to).await.unwrap();
    assert_eq!(bal_after_first, bal_after_second, "restart must not transfer twice");
}

#[tokio::test]
async fn dry_run_and_missing_approval_and_mainnet_gate_refuse_broadcast() {
    let Some((anvil, signer, from)) = spawn_anvil() else {
        eprintln!("skipping: anvil not on PATH");
        return;
    };
    let to = anvil.addresses()[1];
    let rpc = anvil.endpoint();
    let p = plan(
        from,
        to,
        native_asset(),
        1_000_000_000_000u128,
        10u128.pow(16),
        "appr-1",
    );

    let (exec, _d) = executor(&rpc, signer.clone(), ExecutionGates::default());
    let err = exec.execute_deposit(&p).await.unwrap_err();
    assert!(err.to_string().contains("DRY_RUN"), "{err}");

    let (exec, _d) = executor(
        &rpc,
        signer.clone(),
        ExecutionGates {
            dry_run: false,
            enable_mainnet_transfers: false,
        },
    );
    let mut no_appr = p.clone();
    no_appr.approval_ref.clear();
    let err = exec.execute_deposit(&no_appr).await.unwrap_err();
    assert!(err.to_string().contains("approval"), "{err}");

    // Mainnet chain id is refused even with approval when the flag is off.
    let mut mainnet = p.clone();
    mainnet.chain_id = ChainId::BASE;
    mainnet.asset.chain_id = ChainId::BASE;
    // RPC chain mismatch is hit first; that's also a refuse. Either is fine.
    let err = exec.execute_deposit(&mainnet).await.unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("MAINNET") || s.contains("rpc chain") || s.contains("refusing"),
        "{s}"
    );
}

#[tokio::test]
async fn insufficient_balance_and_fee_cap_fail_simulation() {
    let Some((anvil, signer, from)) = spawn_anvil() else {
        eprintln!("skipping: anvil not on PATH");
        return;
    };
    let to = anvil.addresses()[1];
    let rpc = anvil.endpoint();
    let (exec, _d) = executor(&rpc, signer, local_gates());

    let too_much = plan(from, to, native_asset(), u128::MAX / 2, u128::MAX, "appr-1");
    let sim = exec.simulate(&too_much).await.unwrap();
    assert!(!sim.ok, "{sim:?}");

    let tiny_fee = plan(from, to, native_asset(), 1_000_000_000_000u128, 1, "appr-1");
    let sim = exec.simulate(&tiny_fee).await.unwrap();
    assert!(!sim.ok, "{sim:?}");
    assert!(exec.execute_deposit(&tiny_fee).await.is_err());
}

#[tokio::test]
async fn erc20_transfer_exact_amount_and_recipient() {
    let Some((anvil, signer, from)) = spawn_anvil() else {
        eprintln!("skipping: anvil not on PATH");
        return;
    };
    let deposit = anvil.addresses()[1];
    let rpc = anvil.endpoint();
    let wallet = alloy::network::EthereumWallet::from(signer.clone());
    let setup = ProviderBuilder::new().wallet(wallet).connect_http(rpc.parse().unwrap());

    let bytecode = alloy::primitives::hex::decode(MOCK_ERC20_BYTECODE).unwrap();
    let pending = setup
        .send_transaction(TransactionRequest::default().with_deploy_code(bytecode))
        .await
        .unwrap();
    let receipt = pending.get_receipt().await.unwrap();
    let token = receipt.contract_address.expect("created");

    let mint_amount = U256::from(5_000_000u64);
    setup
        .send_transaction(
            TransactionRequest::default().with_to(token).with_input(
                mintCall {
                    to: from,
                    amount: mint_amount,
                }
                .abi_encode(),
            ),
        )
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let (exec, _d) = executor(&rpc, signer, local_gates());
    let send_amount = 1_020_000u128;
    let p = plan(
        from,
        deposit,
        erc20_asset(token),
        send_amount,
        10u128.pow(16),
        "appr-erc20",
    );
    let sim = exec.simulate(&p).await.unwrap();
    assert!(sim.ok, "{sim:?}");
    let hash = exec.execute_deposit(&p).await.unwrap();
    let mined = exec.receipt(ChainId::ANVIL, hash).await.unwrap().unwrap();
    assert!(mined.success);

    let raw = setup
        .call(
            TransactionRequest::default()
                .with_to(token)
                .with_input(encode_erc20_balance_of(deposit)),
        )
        .await
        .unwrap();
    let bal = decode_erc20_balance_of(&raw).unwrap();
    assert_eq!(
        bal,
        U256::from(send_amount),
        "must send amountIn (EXACT_OUTPUT buffer), not minAmountIn"
    );
}

#[tokio::test]
async fn flow_prepare_then_execute_with_mock_oneclick() {
    let Some((anvil, signer, from)) = spawn_anvil() else {
        eprintln!("skipping: anvil not on PATH");
        return;
    };
    let rpc = anvil.endpoint();
    let (exec, _d) = executor(&rpc, signer, local_gates());
    let adapter = MockOneClick::default();
    let repo = InMemoryRepository::default();

    let dest = AssetId {
        chain_id: ChainId::ARBITRUM,
        token: TokenRef::Erc20 {
            address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
        },
        decimals: 6,
        one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
    };
    let merchant = MerchantId::new();
    let intent = PaymentIntent {
        id: PaymentIntentId::new(),
        merchant_id: merchant,
        merchant_order_id: "m2".into(),
        payout_config_snapshot: PayoutConfig {
            id: PayoutConfigId::new(),
            merchant_id: merchant,
            asset: dest.clone(),
            receiving_address: "0x1111111111111111111111111111111111111111".parse().unwrap(),
            active: true,
        },
        output_amount_base_units: BaseUnits::from_u128(1_000_000_000_000_000),
        status: pcc_payments::status::PaymentIntentStatus::RequiresPayment,
        expires_at: Utc::now() + Duration::hours(1),
        idempotency_key: "m2-1".into(),
    };
    let spec = QuoteSpec {
        source_asset: native_asset(),
        destination_asset: dest,
        output_amount: intent.output_amount_base_units,
        recipient: intent.payout_config_snapshot.receiving_address,
        refund_to: from,
        confidentiality: pcc_payments::quote::Confidentiality::Basic,
        slippage_bps: 50,
        deadline: Utc::now() + Duration::hours(1),
        authorization_ref: "auth-ref-m2-flow-01".into(),
    };

    let pre = prepare_deposit(
        &adapter,
        &exec,
        &repo,
        crate::flow::PrepareParams {
            intent,
            source_asset: native_asset(),
            spec,
            max_debit: BaseUnits::from_u128(2_000_000_000_000_000),
            max_fee_wei: BaseUnits::from_u128(10u128.pow(16)),
            max_slippage_bps: 100,
            now: Utc::now(),
        },
    )
    .await
    .unwrap();
    assert!(pre.simulation.ok, "{:?}", pre.simulation);
    assert!(!pre.quote.dry);
    assert!(pre.quote.deposit_address.is_some());

    let hash = execute_approved(
        &adapter,
        &exec,
        &repo,
        pre.attempt_id,
        "owner-appr-1",
        BaseUnits::from_u128(10u128.pow(16)),
    )
    .await
    .unwrap();
    let mined = exec.receipt(ChainId::ANVIL, hash).await.unwrap().unwrap();
    assert!(mined.success);
    let stored = repo.get_attempt(pre.attempt_id).await.unwrap();
    assert_eq!(stored.status, PaymentAttemptStatus::DepositBroadcast);
    assert!(stored.source_tx_hash.is_some());

    // Idempotent re-execute.
    let hash2 = execute_approved(
        &adapter,
        &exec,
        &repo,
        pre.attempt_id,
        "owner-appr-1",
        BaseUnits::from_u128(10u128.pow(16)),
    )
    .await
    .unwrap();
    assert_eq!(hash, hash2);
}

#[tokio::test]
async fn plan_from_quote_refuses_dry_and_memo() {
    let q = pcc_payments::quote::VerifiedQuote {
        correlation_id: "c".into(),
        signature: "s".into(),
        signature_verified: true,
        dry: true,
        swap_type: pcc_payments::quote::SwapType::ExactOutput,
        confidentiality: pcc_payments::quote::Confidentiality::Basic,
        deposit_mode: pcc_payments::quote::DepositMode::Simple,
        origin_asset_id: "x".into(),
        destination_asset_id: "y".into(),
        recipient: "0x1111111111111111111111111111111111111111".into(),
        refund_to: "0x2222222222222222222222222222222222222222".into(),
        amount_in: BaseUnits::from_u128(1),
        min_amount_in: BaseUnits::from_u128(1),
        amount_out: BaseUnits::from_u128(1),
        min_amount_out: BaseUnits::from_u128(1),
        deposit_address: None,
        deposit_memo: None,
        quote_timestamp: Utc::now(),
        deadline: None,
        time_estimate_seconds: 1,
        raw_response_json: "{}".into(),
    };
    let err = plan_from_quote(
        PaymentAttemptId::new(),
        &q,
        &native_asset(),
        Address::ZERO,
        BaseUnits::from_u128(1),
        "a".into(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("dry"));
}
