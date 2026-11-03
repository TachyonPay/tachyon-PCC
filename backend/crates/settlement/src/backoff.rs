//! Deterministic bounded backoff schedules (no randomness so tests and
//! restarts are reproducible; jitter can be added at the scheduler level).

use chrono::Duration;

/// 1Click status polling: 5s, 10s, 20s, 40s, 60s, 60s, ... (capped at 60s).
#[must_use]
pub fn status_poll_delay(attempt: u32) -> Duration {
    exponential(attempt, Duration::seconds(5), Duration::seconds(60))
}

/// Webhook delivery: 1s, 2s, 4s, ..., capped at 1h. Max attempts enforced by caller.
#[must_use]
pub fn webhook_retry_delay(attempt: u32) -> Duration {
    exponential(attempt, Duration::seconds(1), Duration::hours(1))
}

pub const WEBHOOK_MAX_ATTEMPTS: u32 = 12;

fn exponential(attempt: u32, base: Duration, cap: Duration) -> Duration {
    let shift = attempt.min(30);
    let factor = 1i64 << shift;
    let candidate_ms = base.num_milliseconds().saturating_mul(factor);
    Duration::milliseconds(candidate_ms.min(cap.num_milliseconds()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_schedule_is_bounded() {
        assert_eq!(status_poll_delay(0), Duration::seconds(5));
        assert_eq!(status_poll_delay(1), Duration::seconds(10));
        assert_eq!(status_poll_delay(2), Duration::seconds(20));
        assert_eq!(status_poll_delay(3), Duration::seconds(40));
        assert_eq!(status_poll_delay(4), Duration::seconds(60));
        assert_eq!(status_poll_delay(50), Duration::seconds(60));
        assert_eq!(status_poll_delay(u32::MAX), Duration::seconds(60));
    }

    #[test]
    fn webhook_schedule_is_bounded() {
        assert_eq!(webhook_retry_delay(0), Duration::seconds(1));
        assert_eq!(webhook_retry_delay(5), Duration::seconds(32));
        assert_eq!(webhook_retry_delay(WEBHOOK_MAX_ATTEMPTS), Duration::hours(1));
        assert_eq!(webhook_retry_delay(100), Duration::hours(1));
    }
}
