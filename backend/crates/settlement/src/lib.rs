//! Settlement worker building blocks.
//!
//! `poll_once` reconciles one attempt against the 1Click status endpoint.
//! It is idempotent: re-running it after a crash produces the same state and
//! never re-broadcasts anything (broadcast lives in the executor, gated).

pub mod backoff;

use chrono::{DateTime, Utc};
use pcc_payments::domain::PaymentAttemptId;
use pcc_payments::status::{OneClickStatus, PaymentAttemptStatus};
use pcc_payments::traits::{AdapterError, OneClickAdapter};
use pcc_storage::{Repository, StorageError};

#[derive(Debug, thiserror::Error)]
pub enum SettlementError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("attempt has no deposit address")]
    NoDepositAddress,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Terminal state reached; stop polling.
    Done(PaymentAttemptStatus),
    /// Keep polling after `next_poll_at`.
    Continue {
        status: PaymentAttemptStatus,
        one_click: OneClickStatus,
        next_poll_at: DateTime<Utc>,
    },
    /// Transient upstream problem; retry after `next_poll_at`.
    Retry {
        reason: String,
        next_poll_at: DateTime<Utc>,
    },
}

/// Reconcile one attempt. `poll_attempt` is the 0-based count of prior polls
/// used for backoff.
pub async fn poll_once<R: Repository + ?Sized, A: OneClickAdapter + ?Sized>(
    repo: &R,
    adapter: &A,
    attempt_id: PaymentAttemptId,
    poll_attempt: u32,
    now: DateTime<Utc>,
) -> Result<PollOutcome, SettlementError> {
    let attempt = repo.get_attempt(attempt_id).await?;
    if attempt.status.is_terminal() {
        return Ok(PollOutcome::Done(attempt.status));
    }
    let deposit = attempt
        .deposit_address
        .as_deref()
        .ok_or(SettlementError::NoDepositAddress)?;

    let status = match adapter.status(deposit, attempt.deposit_memo.as_deref()).await {
        Ok(s) => s,
        Err(e @ (AdapterError::Timeout | AdapterError::Unavailable(_) | AdapterError::RateLimited)) => {
            let delay = backoff::status_poll_delay(poll_attempt);
            return Ok(PollOutcome::Retry {
                reason: e.to_string(),
                next_poll_at: now + delay,
            });
        }
        Err(e) => return Err(e.into()),
    };

    let suggested = status.status.suggested_attempt_status().unwrap_or(attempt.status);

    // Only move forward; ignore suggestions that would be no-ops or regressions.
    let new_status = if suggested == attempt.status {
        attempt.status
    } else {
        match repo.transition_attempt(attempt_id, suggested).await {
            Ok(a) => a.status,
            Err(StorageError::IllegalTransition(t)) => {
                tracing::warn!(%attempt_id, %t, "ignoring regressive 1Click status");
                attempt.status
            }
            Err(e) => return Err(e.into()),
        }
    };

    if new_status.is_terminal() {
        return Ok(PollOutcome::Done(new_status));
    }
    // SUCCESS from 1Click is not "Settled" until the destination receipt is
    // verified by the chain watcher (M2). Keep polling until then.
    Ok(PollOutcome::Continue {
        status: new_status,
        one_click: status.status,
        next_poll_at: now + backoff::status_poll_delay(poll_attempt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_payments::amount::BaseUnits;
    use pcc_payments::asset::{AssetId, ChainId, TokenRef};
    use pcc_payments::domain::{PaymentAttempt, PaymentIntentId};
    use pcc_payments::mock::{MockBehaviour, MockOneClick};
    use pcc_storage::InMemoryRepository;

    fn attempt(status: PaymentAttemptStatus) -> PaymentAttempt {
        PaymentAttempt {
            id: PaymentAttemptId::new(),
            payment_intent_id: PaymentIntentId::new(),
            source_wallet: alloy_primitives::Address::from([2u8; 20]),
            source_asset: AssetId {
                chain_id: ChainId::BASE,
                token: TokenRef::Native,
                decimals: 18,
                one_click_asset_id: "x".into(),
            },
            quoted_input: BaseUnits::from_u128(1),
            max_debit: BaseUnits::from_u128(1),
            quote_signature: "s".into(),
            quote_raw_response: "{}".into(),
            quote_expires_at: Utc::now(),
            deposit_address: Some("0xdep".into()),
            deposit_memo: None,
            source_tx_hash: Some("0xaaa".into()),
            destination_tx_hash: None,
            approval_ref: None,
            status,
        }
    }

    #[tokio::test]
    async fn progresses_through_one_click_statuses() {
        let repo = InMemoryRepository::default();
        let adapter = MockOneClick::default();
        let a = attempt(PaymentAttemptStatus::DepositBroadcast);
        repo.insert_attempt(a.clone()).await.unwrap();
        let now = Utc::now();

        adapter.set_status("0xdep", OneClickStatus::PendingDeposit);
        let out = poll_once(&repo, &adapter, a.id, 0, now).await.unwrap();
        assert!(matches!(
            out,
            PollOutcome::Continue {
                status: PaymentAttemptStatus::DepositSeen,
                ..
            }
        ));

        adapter.set_status("0xdep", OneClickStatus::Processing);
        let out = poll_once(&repo, &adapter, a.id, 1, now).await.unwrap();
        assert!(matches!(
            out,
            PollOutcome::Continue {
                status: PaymentAttemptStatus::Processing,
                ..
            }
        ));

        // SUCCESS keeps us in Processing until destination receipt verification.
        adapter.set_status("0xdep", OneClickStatus::Success);
        let out = poll_once(&repo, &adapter, a.id, 2, now).await.unwrap();
        assert!(matches!(
            out,
            PollOutcome::Continue {
                status: PaymentAttemptStatus::Processing,
                one_click: OneClickStatus::Success,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn refunded_and_failed_are_terminal() {
        for (oc, expected) in [
            (OneClickStatus::Refunded, PaymentAttemptStatus::Refunded),
            (OneClickStatus::Failed, PaymentAttemptStatus::Failed),
        ] {
            let repo = InMemoryRepository::default();
            let adapter = MockOneClick::default();
            let a = attempt(PaymentAttemptStatus::Processing);
            repo.insert_attempt(a.clone()).await.unwrap();
            adapter.set_status("0xdep", oc);
            let out = poll_once(&repo, &adapter, a.id, 0, Utc::now()).await.unwrap();
            assert_eq!(out, PollOutcome::Done(expected));
            // Re-polling a terminal attempt is a no-op.
            let again = poll_once(&repo, &adapter, a.id, 1, Utc::now()).await.unwrap();
            assert_eq!(again, PollOutcome::Done(expected));
        }
    }

    #[tokio::test]
    async fn regressive_status_is_ignored() {
        let repo = InMemoryRepository::default();
        let adapter = MockOneClick::default();
        let a = attempt(PaymentAttemptStatus::Processing);
        repo.insert_attempt(a.clone()).await.unwrap();
        adapter.set_status("0xdep", OneClickStatus::PendingDeposit);
        let out = poll_once(&repo, &adapter, a.id, 0, Utc::now()).await.unwrap();
        assert!(matches!(
            out,
            PollOutcome::Continue {
                status: PaymentAttemptStatus::Processing,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn transient_errors_schedule_retry_with_backoff() {
        let repo = InMemoryRepository::default();
        let adapter = MockOneClick::default();
        let a = attempt(PaymentAttemptStatus::DepositBroadcast);
        repo.insert_attempt(a.clone()).await.unwrap();
        let now = Utc::now();
        adapter.set_behaviour(MockBehaviour::Timeout);
        let out = poll_once(&repo, &adapter, a.id, 3, now).await.unwrap();
        match out {
            PollOutcome::Retry { next_poll_at, .. } => {
                assert_eq!(next_poll_at - now, backoff::status_poll_delay(3));
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_upstream_is_a_hard_error() {
        let repo = InMemoryRepository::default();
        let adapter = MockOneClick::default();
        let a = attempt(PaymentAttemptStatus::DepositBroadcast);
        repo.insert_attempt(a.clone()).await.unwrap();
        adapter.set_behaviour(MockBehaviour::Malformed);
        assert!(matches!(
            poll_once(&repo, &adapter, a.id, 0, Utc::now()).await,
            Err(SettlementError::Adapter(AdapterError::Malformed(_)))
        ));
    }

    #[tokio::test]
    async fn missing_deposit_address_is_rejected() {
        let repo = InMemoryRepository::default();
        let adapter = MockOneClick::default();
        let mut a = attempt(PaymentAttemptStatus::DepositBroadcast);
        a.deposit_address = None;
        repo.insert_attempt(a.clone()).await.unwrap();
        assert!(matches!(
            poll_once(&repo, &adapter, a.id, 0, Utc::now()).await,
            Err(SettlementError::NoDepositAddress)
        ));
    }
}
