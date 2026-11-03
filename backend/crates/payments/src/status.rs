//! Payment state machines. Transitions are explicit and tested; anything not
//! listed is rejected so a worker bug cannot "skip" a state.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("illegal transition {from} -> {to}")]
pub struct IllegalTransition {
    pub from: String,
    pub to: String,
}

/// Lifecycle of a merchant-facing `PaymentIntent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentIntentStatus {
    /// Created by the merchant; no attempt yet.
    RequiresPayment,
    /// An attempt is in flight (quote accepted, deposit pending or processing).
    Processing,
    /// Destination settlement verified on-chain.
    Succeeded,
    /// Intent expired before any attempt succeeded.
    Expired,
    /// Merchant cancelled before settlement.
    Canceled,
}

impl PaymentIntentStatus {
    pub fn transition(self, to: Self) -> Result<Self, IllegalTransition> {
        use PaymentIntentStatus as S;
        let ok = matches!(
            (self, to),
            (S::RequiresPayment, S::Processing | S::Expired | S::Canceled)
                | (S::Processing, S::Succeeded | S::RequiresPayment | S::Expired)
        );
        if ok {
            Ok(to)
        } else {
            Err(IllegalTransition {
                from: format!("{self:?}"),
                to: format!("{to:?}"),
            })
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Expired | Self::Canceled)
    }
}

/// Lifecycle of one `PaymentAttempt` (one quote, one deposit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentAttemptStatus {
    /// Executable quote obtained and verified; waiting for authorization.
    Quoted,
    /// Authorization (TOTP / owner approval) bound to this attempt.
    Authorized,
    /// Source deposit broadcast; tx hash recorded.
    DepositBroadcast,
    /// 1Click has observed the deposit (KNOWN_DEPOSIT_TX / PENDING_DEPOSIT).
    DepositSeen,
    /// 1Click reports PROCESSING.
    Processing,
    /// 1Click reports SUCCESS and destination receipt verified.
    Settled,
    /// Refund issued to `refundTo` (REFUNDED), or quote expired unfunded.
    Refunded,
    /// Terminal failure (FAILED, or deposit below minimum and deadline passed).
    Failed,
    /// Quote deadline passed before deposit.
    Expired,
}

impl PaymentAttemptStatus {
    pub fn transition(self, to: Self) -> Result<Self, IllegalTransition> {
        use PaymentAttemptStatus as S;
        let ok = matches!(
            (self, to),
            (S::Quoted, S::Authorized | S::Expired)
                | (S::Authorized, S::DepositBroadcast | S::Expired)
                | (
                    S::DepositBroadcast,
                    S::DepositSeen | S::Processing | S::Settled | S::Refunded | S::Failed
                )
                | (S::DepositSeen, S::Processing | S::Settled | S::Refunded | S::Failed)
                | (S::Processing, S::Settled | S::Refunded | S::Failed)
        );
        if ok {
            Ok(to)
        } else {
            Err(IllegalTransition {
                from: format!("{self:?}"),
                to: format!("{to:?}"),
            })
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Settled | Self::Refunded | Self::Failed | Self::Expired)
    }
}

/// Exact mirror of the 1Click `GET /v0/status` enum (API v0.1.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OneClickStatus {
    KnownDepositTx,
    PendingDeposit,
    IncompleteDeposit,
    Processing,
    Success,
    Refunded,
    Failed,
}

impl OneClickStatus {
    /// Map a 1Click status to the attempt status it *suggests*. `Success` maps
    /// to `Processing` here on purpose: an attempt only becomes `Settled` once
    /// the destination receipt has been independently verified by the
    /// settlement worker.
    #[must_use]
    pub fn suggested_attempt_status(self) -> Option<PaymentAttemptStatus> {
        Some(match self {
            Self::KnownDepositTx | Self::PendingDeposit | Self::IncompleteDeposit => PaymentAttemptStatus::DepositSeen,
            Self::Processing | Self::Success => PaymentAttemptStatus::Processing,
            Self::Refunded => PaymentAttemptStatus::Refunded,
            Self::Failed => PaymentAttemptStatus::Failed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_happy_path() {
        let s = PaymentIntentStatus::RequiresPayment;
        let s = s.transition(PaymentIntentStatus::Processing).unwrap();
        let s = s.transition(PaymentIntentStatus::Succeeded).unwrap();
        assert!(s.is_terminal());
        assert!(s.transition(PaymentIntentStatus::Processing).is_err());
    }

    #[test]
    fn intent_cannot_skip_processing() {
        assert!(PaymentIntentStatus::RequiresPayment
            .transition(PaymentIntentStatus::Succeeded)
            .is_err());
    }

    #[test]
    fn attempt_happy_path_and_terminals() {
        use PaymentAttemptStatus as S;
        let s = S::Quoted
            .transition(S::Authorized)
            .and_then(|s| s.transition(S::DepositBroadcast))
            .and_then(|s| s.transition(S::DepositSeen))
            .and_then(|s| s.transition(S::Processing))
            .and_then(|s| s.transition(S::Settled))
            .unwrap();
        assert!(s.is_terminal());
        for t in [S::Quoted, S::Authorized, S::DepositBroadcast, S::Refunded] {
            assert!(s.transition(t).is_err(), "settled -> {t:?} must be illegal");
        }
    }

    #[test]
    fn attempt_cannot_broadcast_without_authorization() {
        assert!(PaymentAttemptStatus::Quoted
            .transition(PaymentAttemptStatus::DepositBroadcast)
            .is_err());
    }

    #[test]
    fn one_click_status_json_matches_api_enum() {
        let names = [
            "KNOWN_DEPOSIT_TX",
            "PENDING_DEPOSIT",
            "INCOMPLETE_DEPOSIT",
            "PROCESSING",
            "SUCCESS",
            "REFUNDED",
            "FAILED",
        ];
        for n in names {
            let v: OneClickStatus = serde_json::from_str(&format!("\"{n}\"")).unwrap();
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{n}\""));
        }
        assert!(serde_json::from_str::<OneClickStatus>("\"DONE\"").is_err());
    }

    #[test]
    fn success_does_not_imply_settled() {
        assert_eq!(
            OneClickStatus::Success.suggested_attempt_status(),
            Some(PaymentAttemptStatus::Processing)
        );
    }
}
