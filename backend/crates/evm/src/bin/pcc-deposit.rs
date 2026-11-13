//! M2 deposit CLI.
//!
//! `preflight` (default): dry 1Click quote. Never broadcasts.
//! `preflight --allocate-deposit`: live `dry:false` quote + source-tx simulation.
//! Still never broadcasts.
//!
//! `execute --from --to --amount --approval-ref`: owner-approved broadcast of
//! one ERC-20 transfer. Attempts are not in Postgres yet (M6); the FileLedger
//! under `.pcc-ledger/` is the restart-safety for this path. Still requires
//! DRY_RUN=false, ENABLE_MAINNET_TRANSFERS=true, and a non-empty approval ref.

use alloy::primitives::Address;
use anyhow::{bail, Context};
use chrono::{Duration, Utc};
use pcc_evm::envfile;
use pcc_evm::keys::load_signer;
use pcc_evm::{AlloyEoaExecutor, FileLedger};
use pcc_integration_client::{IntegrationClient, IntegrationClientConfig};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::PaymentAttemptId;
use pcc_payments::gates::ExecutionGates;
use pcc_payments::quote::Confidentiality;
use pcc_payments::traits::{DepositPlan, OneClickAdapter, QuoteSpec, SourceExecutor};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use uuid::Uuid;

const ORIGIN_DEFAULT: &str = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const DEST_DEFAULT: &str = "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";

fn load_env() {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(p) = envfile::find_dotenv(&cwd) {
        let _ = envfile::load_dotenv(&p);
    }
}

fn req_env(name: &str) -> anyhow::Result<String> {
    let v = std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is not set"))?;
    if v.trim().is_empty() {
        bail!("{name} is empty");
    }
    Ok(v)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    load_env();

    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "preflight" => {
            let allocate = args.any(|a| a == "--allocate-deposit");
            preflight(allocate).await
        }
        "execute" => execute(args.collect()).await,
        _ => {
            eprintln!(
                "pcc-deposit preflight [--allocate-deposit]\n\
                 pcc-deposit execute --from <addr> --to <deposit> --amount <base-units> \\\n\
                     --approval-ref <id> [--token <erc20>] [--chain <id>] [--max-fee-wei <n>] \\\n\
                     [--attempt-id <uuid>]\n\n\
                 preflight talks to the local TS worker (INTEGRATIONS_BASE_URL).\n\
                 --allocate-deposit is a live 1Click dry:false quote; still no broadcast.\n\
                 execute broadcasts one ERC-20 transfer after ExecutionGates pass."
            );
            Ok(())
        }
    }
}

fn usdc_base() -> AssetId {
    AssetId {
        chain_id: ChainId::BASE,
        token: TokenRef::Erc20 {
            address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
        },
        decimals: 6,
        one_click_asset_id: ORIGIN_DEFAULT.into(),
    }
}

fn usdc_arb() -> AssetId {
    AssetId {
        chain_id: ChainId::ARBITRUM,
        token: TokenRef::Erc20 {
            address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
        },
        decimals: 6,
        one_click_asset_id: DEST_DEFAULT.into(),
    }
}

fn adapter() -> anyhow::Result<IntegrationClient> {
    let token = req_env("INTEGRATIONS_INTERNAL_TOKEN")?;
    if token.len() < 16 {
        bail!("INTEGRATIONS_INTERNAL_TOKEN is too short");
    }
    let cfg = IntegrationClientConfig {
        base_url: std::env::var("INTEGRATIONS_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into()),
        bearer_token: token,
        timeout: StdDuration::from_secs(25),
    };
    Ok(IntegrationClient::new(cfg)?)
}

async fn preflight(allocate: bool) -> anyhow::Result<()> {
    if allocate {
        bail!(
            "--allocate-deposit would call 1Click with dry:false and allocate a live deposit address.\n\
             That is allowed only after you confirm WALLET_A_ADDRESS, WALLET_B_ADDRESS, max $ loss,\n\
             and that you want a live quote (still no broadcast). Re-run after those are in .env\n\
             and the owner has said so in chat. Defaulting to a dry quote instead is safer;\n\
             drop the flag to get a dry quote now."
        );
    }

    // Dry quotes never fund anything; placeholders are fine until the owner
    // supplies dedicated test wallets A/B.
    let wallet_a: Address = std::env::var("WALLET_A_ADDRESS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "0x2222222222222222222222222222222222222222".into())
        .parse()
        .context("WALLET_A_ADDRESS")?;
    let wallet_b: Address = std::env::var("WALLET_B_ADDRESS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "0x1111111111111111111111111111111111111111".into())
        .parse()
        .context("WALLET_B_ADDRESS")?;
    let amount_out = BaseUnits::parse_base_units(&std::env::var("AMOUNT_OUT").unwrap_or_else(|_| "1000000".into()))?;

    let client = adapter()?;
    let spec = QuoteSpec {
        source_asset: usdc_base(),
        destination_asset: usdc_arb(),
        output_amount: amount_out,
        recipient: wallet_b,
        refund_to: wallet_a,
        confidentiality: Confidentiality::Basic,
        slippage_bps: 100,
        deadline: Utc::now() + Duration::hours(1),
        authorization_ref: format!("preflight-dry-{}", Utc::now().timestamp()),
    };
    let q = client.preview(&spec).await?;
    println!("DRY quote via TS worker (no deposit address, nothing can be funded):");
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "signatureVerified": q.signature_verified,
            "dry": q.dry,
            "confidentiality": q.confidentiality,
            "originAssetId": q.origin_asset_id,
            "destinationAssetId": q.destination_asset_id,
            "recipient": q.recipient,
            "refundTo": q.refund_to,
            "amountIn": q.amount_in,
            "minAmountIn": q.min_amount_in,
            "amountOut": q.amount_out,
            "depositAddressPresent": q.deposit_address.is_some(),
        }))?
    );
    Ok(())
}

const USDC_BASE_ADDR: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const DEFAULT_MAX_FEE_WEI: &str = "100000000000000"; // 0.0001 ETH

struct ExecuteArgs {
    from: Address,
    to: Address,
    amount: BaseUnits,
    token: Address,
    chain_id: ChainId,
    approval_ref: String,
    max_fee_wei: BaseUnits,
    attempt_id: PaymentAttemptId,
}

fn flag_map(argv: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < argv.len() {
        let key = &argv[i];
        anyhow::ensure!(key.starts_with("--"), "unexpected argument {key}");
        let val = argv.get(i + 1).with_context(|| format!("{key} requires a value"))?;
        anyhow::ensure!(!val.starts_with("--"), "{key} requires a value");
        out.insert(key.clone(), val.clone());
        i += 2;
    }
    Ok(out)
}

fn require_flag(flags: &HashMap<String, String>, name: &str) -> anyhow::Result<String> {
    flags
        .get(name)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{name} is required"))
}

fn parse_execute_args(argv: &[String]) -> anyhow::Result<ExecuteArgs> {
    let flags = flag_map(argv)?;
    let from: Address = require_flag(&flags, "--from")?.parse().context("--from")?;
    let to: Address = require_flag(&flags, "--to")?.parse().context("--to")?;
    let amount_s = require_flag(&flags, "--amount")?;
    let amount = BaseUnits::parse_base_units(&amount_s).context("--amount")?;
    let approval_ref = require_flag(&flags, "--approval-ref")?;
    let token: Address = flags
        .get("--token")
        .map_or(USDC_BASE_ADDR, String::as_str)
        .parse()
        .context("--token")?;
    let chain_id = ChainId(
        flags
            .get("--chain")
            .map_or("8453", String::as_str)
            .parse()
            .context("--chain")?,
    );
    let max_fee_wei =
        BaseUnits::parse_base_units(flags.get("--max-fee-wei").map_or(DEFAULT_MAX_FEE_WEI, String::as_str))
            .context("--max-fee-wei")?;
    if chain_id != ChainId::BASE {
        bail!("this CLI execute only sends on Base (8453) until M6");
    }
    let expected_token: Address = USDC_BASE_ADDR.parse().expect("static USDC address");
    if token != expected_token {
        bail!("this CLI execute only sends Base USDC until M6");
    }
    let attempt_id = if let Some(s) = flags.get("--attempt-id") {
        PaymentAttemptId(Uuid::parse_str(s).context("--attempt-id")?)
    } else {
        attempt_id_from_plan(from, to, &amount, token, chain_id.0)
    };
    Ok(ExecuteArgs {
        from,
        to,
        amount,
        token,
        chain_id,
        approval_ref,
        max_fee_wei,
        attempt_id,
    })
}

fn attempt_id_from_plan(
    from: Address,
    to: Address,
    amount: &BaseUnits,
    token: Address,
    chain: u64,
) -> PaymentAttemptId {
    let mut hasher = Sha256::new();
    hasher.update(from.as_slice());
    hasher.update(to.as_slice());
    hasher.update(amount.to_string().as_bytes());
    hasher.update(token.as_slice());
    hasher.update(chain.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    PaymentAttemptId(Uuid::from_bytes(bytes))
}

fn ledger_dir() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("PCC_LEDGER_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return Ok(PathBuf::from(t));
        }
    }
    let cwd = std::env::current_dir().context("cwd")?;
    let root = envfile::find_dotenv(&cwd)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or(cwd);
    Ok(root.join(".pcc-ledger"))
}

async fn execute(argv: Vec<String>) -> anyhow::Result<()> {
    let parsed = parse_execute_args(&argv)?;
    let key_ref = req_env("SOURCE_EXECUTOR_KEY_REF")?;
    let signer = load_signer(&key_ref).context("load SOURCE_EXECUTOR_KEY_REF")?;
    if signer.address() != parsed.from {
        bail!(
            "loaded signer {} does not match --from {}",
            signer.address(),
            parsed.from
        );
    }
    let rpc = req_env("RPC_URL_BASE")?;
    let gates = ExecutionGates::from_env();
    let ledger = FileLedger::new(ledger_dir()?).context("open FileLedger")?;
    let exec = AlloyEoaExecutor::connect(&rpc, signer, gates, ledger).context("connect executor")?;

    let plan = DepositPlan {
        attempt_id: parsed.attempt_id,
        chain_id: parsed.chain_id,
        asset: AssetId {
            chain_id: parsed.chain_id,
            token: TokenRef::Erc20 { address: parsed.token },
            decimals: 6,
            one_click_asset_id: ORIGIN_DEFAULT.into(),
        },
        from: parsed.from,
        to: parsed.to,
        amount: parsed.amount,
        max_fee_wei: parsed.max_fee_wei,
        approval_ref: parsed.approval_ref.clone(),
        mandate: None,
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "phase": "pre-broadcast",
            "attemptId": plan.attempt_id.to_string(),
            "chainId": plan.chain_id.0,
            "token": format!("{:#x}", parsed.token),
            "from": format!("{:#x}", plan.from),
            "to": format!("{:#x}", plan.to),
            "amount": plan.amount.to_string(),
            "maxFeeWei": plan.max_fee_wei.to_string(),
            "approvalRef": plan.approval_ref,
            "dryRun": gates.dry_run,
            "enableMainnetTransfers": gates.enable_mainnet_transfers,
        }))?
    );

    let sim = exec.simulate(&plan).await.context("simulate")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "phase": "simulation",
            "ok": sim.ok,
            "gasEstimate": sim.gas_estimate,
            "maxFeeWei": sim.max_fee_wei.to_string(),
            "notes": sim.notes,
        }))?
    );
    if !sim.ok {
        bail!("simulation failed; refusing to sign");
    }

    let hash = exec.execute_deposit(&plan).await.context("broadcast")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "phase": "broadcast",
            "txHash": format!("{hash:#x}"),
        }))?
    );

    for _ in 0..45 {
        if let Some(receipt) = exec.receipt(parsed.chain_id, hash).await.context("receipt")? {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "phase": "receipt",
                    "txHash": format!("{:#x}", receipt.tx_hash),
                    "blockNumber": receipt.block_number,
                    "success": receipt.success,
                    "gasUsed": receipt.gas_used,
                }))?
            );
            if !receipt.success {
                bail!("transaction mined but reverted");
            }
            return Ok(());
        }
        tokio::time::sleep(StdDuration::from_secs(2)).await;
    }
    bail!("broadcast sent ({hash:#x}) but no receipt after ~90s; check the explorer and retry execute with the same flags to rebroadcast the ledgered tx");
}
