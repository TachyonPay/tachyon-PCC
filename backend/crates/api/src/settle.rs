//! Hosted-checkout settlement poller: 1Click status drives forward-only
//! attempt transitions and merchant webhooks. An attempt only becomes
//! `settled` after the destination receipt is verified on-chain.

use crate::{enqueue_and_deliver, snake_status, AppState};
use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{AssetId, ChainId, TokenRef};
use pcc_payments::domain::PaymentAttempt;
use pcc_payments::status::{OneClickStatus, PaymentAttemptStatus};
use pcc_payments::traits::CheckoutPorts;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// keccak256("Transfer(address,address,uint256)")
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[async_trait]
pub trait DestinationVerifier: Send + Sync {
    /// `Ok(true)` only when `tx_hash` succeeded and transferred at least
    /// `min_amount` of `asset` to `recipient`. `Ok(false)` when not yet mined
    /// or not matching.
    async fn verify(
        &self,
        asset: &AssetId,
        recipient: Address,
        min_amount: BaseUnits,
        tx_hash: &str,
    ) -> Result<bool, String>;
}

pub struct RpcDestinationVerifier {
    http: reqwest::Client,
    rpcs: HashMap<ChainId, String>,
}

impl RpcDestinationVerifier {
    pub fn from_env() -> Self {
        let rpc = |key: &str, fallback: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        let mut rpcs = HashMap::new();
        rpcs.insert(ChainId::BASE, rpc("RPC_URL_BASE", "https://mainnet.base.org"));
        rpcs.insert(
            ChainId::ARBITRUM,
            rpc("RPC_URL_ARBITRUM", "https://arb1.arbitrum.io/rpc"),
        );
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            rpcs,
        }
    }

    /// One JSON-RPC call; returns `result`. Errors never include the URL,
    /// which may embed an API key.
    pub(crate) async fn call(&self, chain: ChainId, method: &str, params: Value) -> Result<Value, String> {
        let url = self
            .rpcs
            .get(&chain)
            .ok_or_else(|| format!("no RPC configured for chain {}", chain.0))?;
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut resp: Value = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| format!("{method} request failed"))?
            .json()
            .await
            .map_err(|_| format!("{method} returned invalid JSON"))?;
        if !resp["error"].is_null() {
            return Err(format!("{method} returned an RPC error"));
        }
        Ok(resp["result"].take())
    }
}

#[async_trait]
impl DestinationVerifier for RpcDestinationVerifier {
    async fn verify(
        &self,
        asset: &AssetId,
        recipient: Address,
        min_amount: BaseUnits,
        tx_hash: &str,
    ) -> Result<bool, String> {
        let TokenRef::Erc20 { address: token } = asset.token else {
            return Err("native destination assets are not verifiable yet".into());
        };
        let receipt = self
            .call(
                asset.chain_id,
                "eth_getTransactionReceipt",
                serde_json::json!([tx_hash]),
            )
            .await?;
        Ok(receipt_pays(&receipt, token, recipient, min_amount))
    }
}

fn receipt_pays(receipt: &Value, token: Address, recipient: Address, min_amount: BaseUnits) -> bool {
    if receipt.is_null() || receipt["status"].as_str() != Some("0x1") {
        return false;
    }
    let Some(logs) = receipt["logs"].as_array() else {
        return false;
    };
    logs.iter().any(|log| {
        let topics: Vec<&str> = log["topics"]
            .as_array()
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let from_token = log["address"]
            .as_str()
            .and_then(|a| a.parse::<Address>().ok())
            .is_some_and(|a| a == token);
        let to_recipient = topics.get(2).is_some_and(|t| topic_address(t) == Some(recipient));
        let amount = log["data"]
            .as_str()
            .and_then(|d| U256::from_str_radix(d.trim_start_matches("0x"), 16).ok())
            .and_then(|v| BaseUnits::parse_base_units(&v.to_string()).ok());
        from_token
            && topics.len() == 3
            && topics[0].eq_ignore_ascii_case(TRANSFER_TOPIC)
            && to_recipient
            && amount.is_some_and(|a| a >= min_amount)
    })
}

fn topic_address(topic: &str) -> Option<Address> {
    let hex = topic.trim_start_matches("0x");
    if hex.len() != 64 {
        return None;
    }
    format!("0x{}", &hex[24..]).parse().ok()
}

/// One pass over every in-flight attempt. Safe to re-run; never broadcasts.
pub async fn poll_once(state: &AppState, ports: &dyn CheckoutPorts, verifier: &dyn DestinationVerifier) {
    for attempt in state.store.in_flight_attempts() {
        if let Err(e) = poll_attempt(state, ports, verifier, &attempt).await {
            tracing::warn!(attempt_id = %attempt.id, error = %e, "settlement poll failed; will retry");
        }
    }
}

async fn poll_attempt(
    state: &AppState,
    ports: &dyn CheckoutPorts,
    verifier: &dyn DestinationVerifier,
    attempt: &PaymentAttempt,
) -> Result<(), String> {
    let Some(deposit) = attempt.deposit_address.as_deref() else {
        return Ok(());
    };
    let status = ports
        .status(deposit, attempt.deposit_memo.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    let Some(mut target) = status.status.suggested_attempt_status() else {
        return Ok(());
    };
    let mut destination_tx = None;
    if status.status == OneClickStatus::Success {
        let intent = state
            .store
            .intent_by_id(attempt.payment_intent_id)
            .ok_or("intent missing")?;
        for tx in &status.destination_tx_hashes {
            let paid = verifier
                .verify(
                    &intent.payout_config_snapshot.asset,
                    intent.payout_config_snapshot.receiving_address,
                    intent.output_amount_base_units,
                    tx,
                )
                .await?;
            if paid {
                target = PaymentAttemptStatus::Settled;
                destination_tx = Some(tx.clone());
                break;
            }
        }
    }
    let Some((attempt, intent)) = state
        .store
        .advance_attempt(attempt.id, target, destination_tx.as_deref())
    else {
        return Ok(());
    };
    let event = match attempt.status {
        PaymentAttemptStatus::Settled => "payment.succeeded",
        PaymentAttemptStatus::Refunded => "payment.refunded",
        PaymentAttemptStatus::Failed => "payment.failed",
        _ => "payment.processing",
    };
    let payload = serde_json::json!({
        "type": event,
        "intent_id": intent.id.to_string(),
        "attempt_id": attempt.id.to_string(),
        "attempt_status": snake_status(attempt.status),
        "intent_status": snake_status(intent.status),
        "one_click_status": status.status,
        "source_tx_hash": attempt.source_tx_hash,
        "destination_tx_hash": attempt.destination_tx_hash,
    })
    .to_string();
    tracing::info!(attempt_id = %attempt.id, status = %snake_status(attempt.status), "settlement advanced");
    enqueue_and_deliver(state, &intent, event, &payload);
    Ok(())
}

pub fn spawn_poller(state: AppState, ports: Arc<dyn CheckoutPorts>, every: Duration) {
    let verifier = RpcDestinationVerifier::from_env();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            poll_once(&state, ports.as_ref(), &verifier).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC_ARB: &str = "0xaf88d065e77c8cc2239327c5edb3a432268e5831";
    const MERCHANT: &str = "0x1111111111111111111111111111111111111111";

    fn receipt(status: &str, token: &str, to: &str, amount: u128) -> Value {
        serde_json::json!({
            "status": status,
            "logs": [{
                "address": token,
                "topics": [
                    TRANSFER_TOPIC,
                    "0x0000000000000000000000002222222222222222222222222222222222222222",
                    format!("0x000000000000000000000000{}", to.trim_start_matches("0x")),
                ],
                "data": format!("0x{amount:064x}"),
            }],
        })
    }

    fn check(r: &Value) -> bool {
        receipt_pays(
            r,
            USDC_ARB.parse().unwrap(),
            MERCHANT.parse().unwrap(),
            BaseUnits::from_u128(200_000),
        )
    }

    #[test]
    fn verifies_exact_transfer() {
        assert!(check(&receipt("0x1", USDC_ARB, MERCHANT, 200_000)));
    }

    #[test]
    fn rejects_short_wrong_recipient_wrong_token_reverted_or_pending() {
        assert!(!check(&receipt("0x1", USDC_ARB, MERCHANT, 199_999)));
        assert!(!check(&receipt(
            "0x1",
            USDC_ARB,
            "0x3333333333333333333333333333333333333333",
            200_000
        )));
        assert!(!check(&receipt(
            "0x1",
            "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            MERCHANT,
            200_000
        )));
        assert!(!check(&receipt("0x0", USDC_ARB, MERCHANT, 200_000)));
        assert!(!check(&Value::Null));
    }

    /// Read-only check against a real 1Click delivery on Arbitrum.
    /// `cargo test -p pcc-api live_arbitrum_receipt -- --ignored`
    #[tokio::test]
    #[ignore = "network"]
    async fn live_arbitrum_receipt() {
        let v = RpcDestinationVerifier::from_env();
        let tx = "0x708bedc43f90736095825b3b774d00641ff58709d99e9be549e75850eeee9989";
        let wallet: Address = "0x049FdEf2550772B9F36F86757C27f5F982D0970D".parse().unwrap();
        let asset = AssetId::usdc_arbitrum();
        assert!(v
            .verify(&asset, wallet, BaseUnits::from_u128(1_000_000), tx)
            .await
            .unwrap());
        assert!(!v
            .verify(&asset, wallet, BaseUnits::from_u128(1_000_001), tx)
            .await
            .unwrap());
        assert!(!v
            .verify(&asset, MERCHANT.parse().unwrap(), BaseUnits::from_u128(1), tx)
            .await
            .unwrap());
    }
}
