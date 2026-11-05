//! Persist-then-broadcast ledger.
//!
//! A signed deposit is written to disk *before* `eth_sendRawTransaction`. On
//! restart we rebroadcast the same bytes (same nonce, same hash) instead of
//! constructing a second transfer. Records are keyed by payment-attempt id.

use alloy::primitives::{Address, Bytes, B256};
use pcc_payments::domain::PaymentAttemptId;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BroadcastRecord {
    pub attempt_id: String,
    pub chain_id: u64,
    pub nonce: u64,
    pub to: Address,
    pub tx_hash: B256,
    pub raw_tx: Bytes,
    pub approval_ref: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("broadcast ledger i/o error")]
    Io,
    #[error("broadcast ledger record is corrupt")]
    Corrupt,
}

#[derive(Clone, Debug)]
pub struct FileLedger {
    dir: PathBuf,
}

impl FileLedger {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, LedgerError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|_| LedgerError::Io)?;
        Ok(Self { dir })
    }

    fn path(&self, id: PaymentAttemptId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn get(&self, id: PaymentAttemptId) -> Result<Option<BroadcastRecord>, LedgerError> {
        let p = self.path(id);
        if !p.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&p).map_err(|_| LedgerError::Io)?;
        let rec = serde_json::from_slice(&bytes).map_err(|_| LedgerError::Corrupt)?;
        Ok(Some(rec))
    }

    /// Write the record. If a record already exists for this attempt it must
    /// be byte-identical (same hash); a different hash is a programming error
    /// and is refused.
    pub fn put(&self, id: PaymentAttemptId, rec: &BroadcastRecord) -> Result<(), LedgerError> {
        if let Some(existing) = self.get(id)? {
            if existing.tx_hash != rec.tx_hash || existing.raw_tx != rec.raw_tx {
                return Err(LedgerError::Corrupt);
            }
            return Ok(());
        }
        let tmp = self.path(id).with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(rec).map_err(|_| LedgerError::Corrupt)?;
        fs::write(&tmp, json).map_err(|_| LedgerError::Io)?;
        fs::rename(&tmp, self.path(id)).map_err(|_| LedgerError::Io)?;
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::bytes;

    #[test]
    fn put_get_and_identical_replay() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = FileLedger::new(dir.path()).unwrap();
        let id = PaymentAttemptId::new();
        let rec = BroadcastRecord {
            attempt_id: id.to_string(),
            chain_id: 31337,
            nonce: 0,
            to: Address::ZERO,
            tx_hash: B256::from([1u8; 32]),
            raw_tx: bytes!("f86c"),
            approval_ref: "appr-1".into(),
        };
        ledger.put(id, &rec).unwrap();
        ledger.put(id, &rec).unwrap();
        assert_eq!(ledger.get(id).unwrap().unwrap().tx_hash, rec.tx_hash);

        let mut other = rec.clone();
        other.tx_hash = B256::from([2u8; 32]);
        assert!(ledger.put(id, &other).is_err());
    }
}
