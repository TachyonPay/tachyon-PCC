//! Plain-EOA [`SourceExecutor`] backed by Alloy.
//!
//! Safety properties:
//! - `simulate` never signs or broadcasts.
//! - `execute_deposit` checks [`ExecutionGates`], re-simulates, signs, writes
//!   the raw tx to the [`FileLedger`], *then* broadcasts. A restart rebroadcasts
//!   the same bytes (same nonce / hash) and cannot construct a second transfer.
//! - Logs only public hashes and ids.

use crate::ledger::{BroadcastRecord, FileLedger};
use crate::transfer::{
    build_deposit_call, decode_erc20_balance_of, decode_erc20_transfer, encode_erc20_balance_of, DepositCall,
};
use alloy::consensus::{Transaction, TxEnvelope};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;
use pcc_payments::amount::BaseUnits;
use pcc_payments::asset::{ChainId, TokenRef};
use pcc_payments::gates::ExecutionGates;
use pcc_payments::traits::{AdapterError, DepositPlan, Receipt, Simulation, SourceExecutor};
const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];
const GAS_BUFFER_NUM: u64 = 12;
const GAS_BUFFER_DEN: u64 = 10;

pub struct AlloyEoaExecutor {
    provider: RootProvider,
    signer: PrivateKeySigner,
    wallet: EthereumWallet,
    gates: ExecutionGates,
    ledger: FileLedger,
}

impl AlloyEoaExecutor {
    pub fn connect(
        rpc_url: &str,
        signer: PrivateKeySigner,
        gates: ExecutionGates,
        ledger: FileLedger,
    ) -> Result<Self, AdapterError> {
        let url = rpc_url
            .parse()
            .map_err(|e| AdapterError::Unavailable(format!("bad rpc url: {e}")))?;
        let provider = ProviderBuilder::new().disable_recommended_fillers().connect_http(url);
        let wallet = EthereumWallet::from(signer.clone());
        Ok(Self {
            provider,
            signer,
            wallet,
            gates,
            ledger,
        })
    }

    pub fn address(&self) -> Address {
        self.signer.address()
    }

    pub fn ledger(&self) -> &FileLedger {
        &self.ledger
    }

    fn unsigned_request(&self, plan: &DepositPlan, call: &DepositCall) -> Result<TransactionRequest, AdapterError> {
        if plan.from != self.signer.address() {
            return Err(AdapterError::Rejected(
                "plan.from does not match the loaded SOURCE_EXECUTOR signer".into(),
            ));
        }
        if call.data.len() >= 4 && call.data[..4] == APPROVE_SELECTOR {
            return Err(AdapterError::Rejected("refusing to sign ERC-20 approve".into()));
        }
        Self::check_call_matches_plan(plan, call)?;
        Ok(TransactionRequest::default()
            .with_from(plan.from)
            .with_to(call.to)
            .with_value(call.value)
            .with_input(call.data.clone())
            .with_chain_id(plan.chain_id.0))
    }

    fn check_call_matches_plan(plan: &DepositPlan, call: &DepositCall) -> Result<(), AdapterError> {
        match plan.asset.token {
            TokenRef::Native => {
                if call.to != plan.to || call.value != plan.amount.as_u256() || !call.data.is_empty() {
                    return Err(AdapterError::Rejected("native call does not match plan".into()));
                }
            }
            TokenRef::Erc20 { address } => {
                if call.to != address || !call.value.is_zero() {
                    return Err(AdapterError::Rejected("erc20 call target/value mismatch".into()));
                }
                let (to, amount) = decode_erc20_transfer(&call.data)
                    .map_err(|e| AdapterError::Rejected(format!("not an erc20 transfer: {e}")))?;
                if to != plan.to || amount != plan.amount.as_u256() {
                    return Err(AdapterError::Rejected("erc20 transfer args do not match plan".into()));
                }
            }
        }
        Ok(())
    }

    async fn rpc_chain_id(&self) -> Result<u64, AdapterError> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|e| AdapterError::Unavailable(format!("eth_chainId: {e}")))
    }

    async fn token_balance(&self, token: Address, owner: Address) -> Result<U256, AdapterError> {
        let tx = TransactionRequest::default()
            .with_to(token)
            .with_input(encode_erc20_balance_of(owner));
        let raw = self
            .provider
            .call(tx)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("balanceOf: {e}")))?;
        decode_erc20_balance_of(&raw).map_err(|e| AdapterError::Malformed(format!("balanceOf decode: {e}")))
    }

    #[allow(clippy::too_many_lines)]
    async fn simulate_inner(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError> {
        let mut notes = Vec::new();
        let call = build_deposit_call(plan).map_err(|e| AdapterError::Rejected(e.to_string()))?;
        let req = self.unsigned_request(plan, &call)?;

        let rpc_chain = self.rpc_chain_id().await?;
        if rpc_chain != plan.chain_id.0 {
            return Err(AdapterError::Rejected(format!(
                "rpc chain {rpc_chain} != plan chain {}",
                plan.chain_id.0
            )));
        }

        let native = self
            .provider
            .get_balance(plan.from)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("eth_getBalance: {e}")))?;
        notes.push(format!("native_balance_wei={native}"));

        match plan.asset.token {
            TokenRef::Native => {
                if native < plan.amount.as_u256() {
                    notes.push("insufficient native balance for deposit".into());
                    return Ok(Simulation {
                        ok: false,
                        gas_estimate: 0,
                        max_fee_wei: BaseUnits::ZERO,
                        sender_balance: BaseUnits::from_u256(native),
                        notes,
                    });
                }
            }
            TokenRef::Erc20 { address } => {
                let bal = self.token_balance(address, plan.from).await?;
                notes.push(format!("token_balance={bal}"));
                if bal < plan.amount.as_u256() {
                    notes.push("insufficient token balance for deposit".into());
                    return Ok(Simulation {
                        ok: false,
                        gas_estimate: 0,
                        max_fee_wei: BaseUnits::ZERO,
                        sender_balance: BaseUnits::from_u256(bal),
                        notes,
                    });
                }
            }
        }

        if let Err(e) = self.provider.call(req.clone()).await {
            notes.push(format!("eth_call reverted: {e}"));
            return Ok(Simulation {
                ok: false,
                gas_estimate: 0,
                max_fee_wei: BaseUnits::ZERO,
                sender_balance: BaseUnits::from_u256(native),
                notes,
            });
        }

        let gas_estimate = self
            .provider
            .estimate_gas(req)
            .await
            .map_err(|e| AdapterError::Rejected(format!("eth_estimateGas: {e}")))?;
        let gas_limit = gas_estimate.saturating_mul(GAS_BUFFER_NUM) / GAS_BUFFER_DEN;
        notes.push(format!("gas_estimate={gas_estimate} gas_limit={gas_limit}"));

        let fees = self
            .provider
            .estimate_eip1559_fees()
            .await
            .map_err(|e| AdapterError::Unavailable(format!("fee estimate: {e}")))?;
        let max_fee_per_gas = fees.max_fee_per_gas;
        let total_fee = U256::from(gas_limit).saturating_mul(U256::from(max_fee_per_gas));
        notes.push(format!(
            "max_fee_per_gas={max_fee_per_gas} max_priority_fee_per_gas={} total_fee_wei={total_fee}",
            fees.max_priority_fee_per_gas
        ));

        if total_fee > plan.max_fee_wei.as_u256() {
            notes.push("estimated fee exceeds plan.max_fee_wei".into());
            return Ok(Simulation {
                ok: false,
                gas_estimate,
                max_fee_wei: BaseUnits::from_u256(total_fee),
                sender_balance: BaseUnits::from_u256(native),
                notes,
            });
        }

        let native_needed = match plan.asset.token {
            TokenRef::Native => plan.amount.as_u256().saturating_add(total_fee),
            TokenRef::Erc20 { .. } => total_fee,
        };
        if native < native_needed {
            notes.push("insufficient native balance for gas (+ value)".into());
            return Ok(Simulation {
                ok: false,
                gas_estimate,
                max_fee_wei: BaseUnits::from_u256(total_fee),
                sender_balance: BaseUnits::from_u256(native),
                notes,
            });
        }

        Ok(Simulation {
            ok: true,
            gas_estimate,
            max_fee_wei: BaseUnits::from_u256(total_fee),
            sender_balance: BaseUnits::from_u256(native),
            notes,
        })
    }

    async fn sign_and_record(&self, plan: &DepositPlan, sim: &Simulation) -> Result<BroadcastRecord, AdapterError> {
        let call = build_deposit_call(plan).map_err(|e| AdapterError::Rejected(e.to_string()))?;
        let nonce = self
            .provider
            .get_transaction_count(plan.from)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("eth_getTransactionCount: {e}")))?;
        let fees = self
            .provider
            .estimate_eip1559_fees()
            .await
            .map_err(|e| AdapterError::Unavailable(format!("fee estimate: {e}")))?;
        let gas_limit = sim.gas_estimate.saturating_mul(GAS_BUFFER_NUM) / GAS_BUFFER_DEN;

        let mut tx_req = self.unsigned_request(plan, &call)?;
        tx_req.set_nonce(nonce);
        tx_req.set_gas_limit(gas_limit);
        tx_req.set_max_fee_per_gas(fees.max_fee_per_gas);
        tx_req.set_max_priority_fee_per_gas(fees.max_priority_fee_per_gas);

        let envelope: TxEnvelope = tx_req
            .build(&self.wallet)
            .await
            .map_err(|e| AdapterError::Rejected(format!("sign: {e}")))?;
        Self::check_signed_envelope(plan, &call, &envelope)?;

        let raw = Bytes::from(envelope.encoded_2718());
        let tx_hash = *envelope.tx_hash();
        let record = BroadcastRecord {
            attempt_id: plan.attempt_id.to_string(),
            chain_id: plan.chain_id.0,
            nonce,
            to: envelope.to().unwrap_or(call.to),
            tx_hash,
            raw_tx: raw,
            approval_ref: plan.approval_ref.clone(),
        };
        self.ledger
            .put(plan.attempt_id, &record)
            .map_err(|e| AdapterError::Other(format!("ledger persist failed (refusing to broadcast): {e}")))?;
        Ok(record)
    }

    fn check_signed_envelope(
        plan: &DepositPlan,
        call: &DepositCall,
        envelope: &TxEnvelope,
    ) -> Result<(), AdapterError> {
        if envelope.chain_id() != Some(plan.chain_id.0) {
            return Err(AdapterError::Rejected("signed tx chain id mismatch".into()));
        }
        if envelope.to() != Some(call.to) {
            return Err(AdapterError::Rejected("signed tx `to` mismatch".into()));
        }
        if envelope.value() != call.value {
            return Err(AdapterError::Rejected("signed tx value mismatch".into()));
        }
        if envelope.input() != &call.data {
            return Err(AdapterError::Rejected("signed tx input mismatch".into()));
        }
        Ok(())
    }

    async fn broadcast_record(&self, rec: &BroadcastRecord) -> Result<B256, AdapterError> {
        tracing::info!(
            attempt_id = %rec.attempt_id,
            tx_hash = %rec.tx_hash,
            chain_id = rec.chain_id,
            nonce = rec.nonce,
            to = %rec.to,
            "broadcasting signed deposit (public ids only)"
        );
        match self.provider.send_raw_transaction(&rec.raw_tx).await {
            Ok(pending) => Ok(*pending.tx_hash()),
            Err(e) => {
                // Already-known / nonce-used means the original broadcast landed.
                let msg = e.to_string().to_lowercase();
                if msg.contains("already known")
                    || msg.contains("known transaction")
                    || msg.contains("nonce too low")
                    || msg.contains("replacement transaction underpriced")
                {
                    tracing::info!(tx_hash = %rec.tx_hash, "raw tx already in mempool/chain; returning ledger hash");
                    return Ok(rec.tx_hash);
                }
                Err(AdapterError::Unavailable(format!("eth_sendRawTransaction: {e}")))
            }
        }
    }
}

#[async_trait]
impl SourceExecutor for AlloyEoaExecutor {
    async fn simulate(&self, plan: &DepositPlan) -> Result<Simulation, AdapterError> {
        self.simulate_inner(plan).await
    }

    async fn execute_deposit(&self, plan: &DepositPlan) -> Result<B256, AdapterError> {
        self.gates
            .check_broadcast(plan.chain_id, Some(plan.approval_ref.as_str()))
            .map_err(|e| AdapterError::Rejected(e.to_string()))?;

        if let Some(existing) = self
            .ledger
            .get(plan.attempt_id)
            .map_err(|e| AdapterError::Other(e.to_string()))?
        {
            if existing.to
                != match plan.asset.token {
                    TokenRef::Native => plan.to,
                    TokenRef::Erc20 { address } => address,
                }
            {
                return Err(AdapterError::Rejected(
                    "ledger record `to` does not match this plan; refusing to rebroadcast".into(),
                ));
            }
            return self.broadcast_record(&existing).await;
        }

        let sim = self.simulate_inner(plan).await?;
        if !sim.ok {
            return Err(AdapterError::Rejected(format!(
                "preflight failed; refusing to sign: {}",
                sim.notes.join("; ")
            )));
        }
        let rec = self.sign_and_record(plan, &sim).await?;
        self.broadcast_record(&rec).await
    }

    async fn receipt(&self, chain_id: ChainId, tx_hash: B256) -> Result<Option<Receipt>, AdapterError> {
        let rpc_chain = self.rpc_chain_id().await?;
        if rpc_chain != chain_id.0 {
            return Err(AdapterError::Rejected(format!(
                "rpc chain {rpc_chain} != requested chain {}",
                chain_id.0
            )));
        }
        let rec = self
            .provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("eth_getTransactionReceipt: {e}")))?;
        Ok(rec.map(|r| Receipt {
            tx_hash: r.transaction_hash,
            block_number: r.block_number.unwrap_or(0),
            success: r.status(),
            gas_used: r.gas_used,
        }))
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn approve_selector_is_the_canonical_one() {
        // keccak256("approve(address,uint256)")[:4] = 0x095ea7b3
        assert_eq!(APPROVE_SELECTOR, [0x09, 0x5e, 0xa7, 0xb3]);
    }
}
