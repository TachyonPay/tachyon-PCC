//! Storage layer.
//!
//! - `migrations/` holds the PostgreSQL schema (applied with `sqlx::migrate!`).
//! - [`Repository`] is the persistence trait the API and settlement worker use.
//! - [`InMemoryRepository`] implements the same semantics (including the
//!   `(merchant_id, idempotency_key)` uniqueness rule) for unit tests.
//!
//! The PostgreSQL implementation lands in M6 together with the durable
//! event log and webhook delivery tables.

use async_trait::async_trait;
use pcc_payments::domain::{MerchantId, PaymentAttempt, PaymentAttemptId, PaymentIntent, PaymentIntentId};
use pcc_payments::status::{IllegalTransition, PaymentAttemptStatus, PaymentIntentStatus};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Embedded migrations; run with `migrator().run(&pool).await`.
#[must_use]
pub fn migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("./migrations")
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,
    #[error("duplicate (merchant_id, idempotency_key)")]
    DuplicateIdempotencyKey,
    #[error(transparent)]
    IllegalTransition(#[from] IllegalTransition),
    #[error("database error: {0}")]
    Db(String),
}

#[async_trait]
pub trait Repository: Send + Sync {
    /// Insert an intent, or return the existing one if the same
    /// `(merchant_id, idempotency_key)` was already used (idempotent create).
    async fn create_intent_idempotent(&self, intent: PaymentIntent) -> Result<PaymentIntent, StorageError>;
    async fn get_intent(&self, merchant: MerchantId, id: PaymentIntentId) -> Result<PaymentIntent, StorageError>;
    async fn transition_intent(
        &self,
        id: PaymentIntentId,
        to: PaymentIntentStatus,
    ) -> Result<PaymentIntent, StorageError>;

    async fn insert_attempt(&self, attempt: PaymentAttempt) -> Result<(), StorageError>;
    async fn get_attempt(&self, id: PaymentAttemptId) -> Result<PaymentAttempt, StorageError>;
    async fn transition_attempt(
        &self,
        id: PaymentAttemptId,
        to: PaymentAttemptStatus,
    ) -> Result<PaymentAttempt, StorageError>;
    /// Record the source tx hash exactly once; a second distinct hash is an error
    /// (protects against double-broadcast after restart).
    async fn record_source_tx(&self, id: PaymentAttemptId, tx_hash: &str) -> Result<(), StorageError>;
}

#[derive(Default)]
pub struct InMemoryRepository {
    intents: Mutex<HashMap<PaymentIntentId, PaymentIntent>>,
    attempts: Mutex<HashMap<PaymentAttemptId, PaymentAttempt>>,
}

#[async_trait]
impl Repository for InMemoryRepository {
    async fn create_intent_idempotent(&self, intent: PaymentIntent) -> Result<PaymentIntent, StorageError> {
        let mut m = self.intents.lock().await;
        if let Some(existing) = m
            .values()
            .find(|i| i.merchant_id == intent.merchant_id && i.idempotency_key == intent.idempotency_key)
        {
            return Ok(existing.clone());
        }
        m.insert(intent.id, intent.clone());
        Ok(intent)
    }

    async fn get_intent(&self, merchant: MerchantId, id: PaymentIntentId) -> Result<PaymentIntent, StorageError> {
        let m = self.intents.lock().await;
        // Tenant isolation: a foreign id is indistinguishable from a missing one.
        m.get(&id)
            .filter(|i| i.merchant_id == merchant)
            .cloned()
            .ok_or(StorageError::NotFound)
    }

    async fn transition_intent(
        &self,
        id: PaymentIntentId,
        to: PaymentIntentStatus,
    ) -> Result<PaymentIntent, StorageError> {
        let mut m = self.intents.lock().await;
        let i = m.get_mut(&id).ok_or(StorageError::NotFound)?;
        i.status = i.status.transition(to)?;
        Ok(i.clone())
    }

    async fn insert_attempt(&self, attempt: PaymentAttempt) -> Result<(), StorageError> {
        self.attempts.lock().await.insert(attempt.id, attempt);
        Ok(())
    }

    async fn get_attempt(&self, id: PaymentAttemptId) -> Result<PaymentAttempt, StorageError> {
        self.attempts
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(StorageError::NotFound)
    }

    async fn transition_attempt(
        &self,
        id: PaymentAttemptId,
        to: PaymentAttemptStatus,
    ) -> Result<PaymentAttempt, StorageError> {
        let mut m = self.attempts.lock().await;
        let a = m.get_mut(&id).ok_or(StorageError::NotFound)?;
        a.status = a.status.transition(to)?;
        Ok(a.clone())
    }

    async fn record_source_tx(&self, id: PaymentAttemptId, tx_hash: &str) -> Result<(), StorageError> {
        let mut m = self.attempts.lock().await;
        let a = m.get_mut(&id).ok_or(StorageError::NotFound)?;
        match &a.source_tx_hash {
            None => {
                a.source_tx_hash = Some(tx_hash.to_owned());
                Ok(())
            }
            Some(existing) if existing == tx_hash => Ok(()),
            Some(existing) => Err(StorageError::Db(format!(
                "attempt {id} already has source tx {existing}; refusing to record {tx_hash}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_payments::amount::BaseUnits;
    use pcc_payments::asset::{AssetId, ChainId, TokenRef};
    use pcc_payments::domain::{PayoutConfig, PayoutConfigId};

    fn intent(merchant: MerchantId, key: &str) -> PaymentIntent {
        PaymentIntent {
            id: PaymentIntentId::new(),
            merchant_id: merchant,
            merchant_order_id: "o".into(),
            payout_config_snapshot: PayoutConfig {
                id: PayoutConfigId::new(),
                merchant_id: merchant,
                asset: AssetId {
                    chain_id: ChainId::ARBITRUM,
                    token: TokenRef::Native,
                    decimals: 18,
                    one_click_asset_id: "x".into(),
                },
                receiving_address: "0x1111111111111111111111111111111111111111".parse().unwrap(),
                active: true,
            },
            output_amount_base_units: BaseUnits::from_u128(1),
            status: PaymentIntentStatus::RequiresPayment,
            expires_at: chrono_now(),
            idempotency_key: key.into(),
        }
    }

    fn chrono_now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    fn attempt(intent_id: PaymentIntentId) -> PaymentAttempt {
        PaymentAttempt {
            id: PaymentAttemptId::new(),
            payment_intent_id: intent_id,
            source_wallet: "0x2222222222222222222222222222222222222222".parse().unwrap(),
            source_asset: AssetId {
                chain_id: ChainId::BASE,
                token: TokenRef::Native,
                decimals: 18,
                one_click_asset_id: "y".into(),
            },
            quoted_input: BaseUnits::from_u128(2),
            max_debit: BaseUnits::from_u128(2),
            quote_signature: "s".into(),
            quote_expires_at: chrono_now(),
            deposit_address: None,
            deposit_memo: None,
            source_tx_hash: None,
            destination_tx_hash: None,
            status: PaymentAttemptStatus::Quoted,
        }
    }

    #[tokio::test]
    async fn duplicate_idempotency_key_returns_same_record() {
        let repo = InMemoryRepository::default();
        let m = MerchantId::new();
        let first = repo.create_intent_idempotent(intent(m, "k1")).await.unwrap();
        let second = repo.create_intent_idempotent(intent(m, "k1")).await.unwrap();
        assert_eq!(first.id, second.id);
        // Same key for a different merchant is a different record.
        let other = repo
            .create_intent_idempotent(intent(MerchantId::new(), "k1"))
            .await
            .unwrap();
        assert_ne!(other.id, first.id);
    }

    #[tokio::test]
    async fn merchant_cannot_read_other_merchants_intent() {
        let repo = InMemoryRepository::default();
        let a = MerchantId::new();
        let b = MerchantId::new();
        let i = repo.create_intent_idempotent(intent(a, "k")).await.unwrap();
        assert!(repo.get_intent(a, i.id).await.is_ok());
        assert!(matches!(repo.get_intent(b, i.id).await, Err(StorageError::NotFound)));
    }

    #[tokio::test]
    async fn illegal_transitions_are_rejected_by_storage() {
        let repo = InMemoryRepository::default();
        let i = repo
            .create_intent_idempotent(intent(MerchantId::new(), "k"))
            .await
            .unwrap();
        assert!(matches!(
            repo.transition_intent(i.id, PaymentIntentStatus::Succeeded).await,
            Err(StorageError::IllegalTransition(_))
        ));
        repo.transition_intent(i.id, PaymentIntentStatus::Processing)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn source_tx_recorded_once() {
        let repo = InMemoryRepository::default();
        let i = repo
            .create_intent_idempotent(intent(MerchantId::new(), "k"))
            .await
            .unwrap();
        let a = attempt(i.id);
        repo.insert_attempt(a.clone()).await.unwrap();
        repo.record_source_tx(a.id, "0xaaa").await.unwrap();
        repo.record_source_tx(a.id, "0xaaa").await.unwrap(); // idempotent replay ok
        assert!(repo.record_source_tx(a.id, "0xbbb").await.is_err());
    }

    #[test]
    fn migrations_embed() {
        let m = migrator();
        assert!(!m.iter().collect::<Vec<_>>().is_empty());
    }
}
