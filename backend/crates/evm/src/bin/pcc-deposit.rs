//! M2 deposit CLI.
//!
//! `preflight` (default): dry 1Click quote. Never broadcasts.
//! `preflight --allocate-deposit`: live `dry:false` quote + source-tx simulation.
//! Still never broadcasts.
//!
//! `execute` is intentionally refused in this binary: the CLI attempt store is
//! in-memory (lost between processes). Persist-then-broadcast is covered by
//! the Anvil tests and will be wired to Postgres in M6. Even then, execute
//! must pass DRY_RUN / ENABLE_MAINNET_TRANSFERS / a non-empty approval ref.

use alloy::primitives::Address;
use anyhow::{bail, Context};
use chrono::{Duration, Utc};
use pcc_evm::envfile;
use pcc_integration_client::{IntegrationClient, IntegrationClientConfig};
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::quote::Confidentiality;
use pcc_payments::traits::{OneClickAdapter, QuoteSpec};
use std::path::PathBuf;
use std::time::Duration as StdDuration;

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
        "execute" => {
            bail!(
                "execute is refused in this CLI until attempts persist in Postgres (M6).\n\
                 Anvil tests cover persist-then-broadcast + restart idempotency.\n\
                 Default gates would also refuse: DRY_RUN={} ENABLE_MAINNET_TRANSFERS={}.",
                pcc_payments::ExecutionGates::from_env().dry_run,
                pcc_payments::ExecutionGates::from_env().enable_mainnet_transfers
            )
        }
        _ => {
            eprintln!(
                "pcc-deposit preflight [--allocate-deposit]\n\
                 pcc-deposit execute   (refused until M6 persistence)\n\n\
                 preflight talks to the local TS worker (INTEGRATIONS_BASE_URL).\n\
                 --allocate-deposit is a live 1Click dry:false quote; still no broadcast."
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
