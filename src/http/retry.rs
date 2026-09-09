//! Backoff for the two failures that are worth retrying.
//!
//! Only 429, 503 and transport errors are retried. A 4xx is an answer, not a hiccup, and
//! retrying it would be noise the service did not ask for. When the attempts run out, the
//! result is [`crate::error::ServiceError::Throttled`] — a distinct exit code, not a
//! generic failure, so that an agent can back off instead of hammering.
//!
//! **How often** is here; **whether at all** is a property of the request
//! ([`crate::http::Replay`]): a transport failure leaves it unknown whether the far side
//! acted, and a request whose sending consumes state there is therefore never sent twice.
//! **How long** to wait for one answer is a property of the host
//! ([`crate::http::limit::timeout_for`]) — this module holds no deadline, because a single
//! one for every service was what turned a slow answer into a destroyed session
//! (`plan/feedback_round_3.md` §1.1).
//!
//! Neither service announces throttling: no `Retry-After`, no `RateLimit-*`. The schedule
//! below is therefore fixed rather than negotiated, and nothing in this module reads a
//! response header.
//!
//! Everything here is a pure function returning a [`Duration`]; the caller does the
//! sleeping. That is what makes the schedule testable without a clock and without a
//! network.

use std::time::{Duration, SystemTime};

/// How many times a request is attempted in total.
pub const MAX_ATTEMPTS: u32 = 3;

/// Base delays before attempts 2 and 3: 1 s and 2 s, then 4 s if the count ever grows.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Upper bound of the random jitter added to each delay, in milliseconds.
const MAX_JITTER_MS: u64 = 500;

/// Upper bound of the random jitter added to each delay.
pub const MAX_JITTER: Duration = Duration::from_millis(MAX_JITTER_MS);

/// Whether a status is worth another attempt.
///
/// 429 and 503 mean "later"; everything else means "no". A 4xx in particular is never
/// retried — the request is broken and will be just as broken in a second.
pub fn is_retryable(status: u16) -> bool {
    matches!(status, 429 | 503)
}

/// How long to wait before attempt `attempt` (1-based, so attempt 1 waits not at all).
///
/// Jitter is derived from the low bits of the system clock rather than from a random
/// number generator: a whole dependency for half a second of spread is not a trade worth
/// making. Attempts beyond the table reuse its last entry, so the schedule stays bounded
/// however the attempt count is changed.
pub fn delay_for(attempt: u32) -> Duration {
    base_delay(attempt).map_or(Duration::ZERO, |base| base + jitter())
}

/// The delay without jitter, or `None` for the first attempt, which waits not at all.
fn base_delay(attempt: u32) -> Option<Duration> {
    let step = usize::try_from(attempt.checked_sub(2)?).unwrap_or(usize::MAX);
    let last = BACKOFF.len().saturating_sub(1);
    BACKOFF.get(step.min(last)).copied()
}

/// A spread of at most [`MAX_JITTER`], taken from the sub-second part of the wall clock.
///
/// It only has to differ between two clients that were throttled at the same moment, and
/// two processes never reach this line in the same nanosecond.
fn jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| u64::from(since.subsec_nanos()));
    Duration::from_millis(nanos % (MAX_JITTER_MS + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_throttling_statuses_are_retried() {
        assert!(is_retryable(429));
        assert!(is_retryable(503));
        for status in [200, 301, 400, 403, 404, 414, 500, 502, 504] {
            assert!(!is_retryable(status), "{status} must not be retried");
        }
    }

    #[test]
    fn the_first_attempt_does_not_wait() {
        assert_eq!(delay_for(0), Duration::ZERO);
        assert_eq!(delay_for(1), Duration::ZERO);
    }

    #[test]
    fn the_schedule_is_one_two_four_seconds_plus_jitter() {
        for (attempt, base) in [(2, 1), (3, 2), (4, 4)] {
            let delay = delay_for(attempt);
            assert!(
                delay >= Duration::from_secs(base)
                    && delay <= Duration::from_secs(base) + MAX_JITTER,
                "attempt {attempt} waited {delay:?}"
            );
        }
    }

    #[test]
    fn attempts_beyond_the_table_stay_bounded() {
        let delay = delay_for(u32::MAX);
        assert!(delay <= Duration::from_secs(4) + MAX_JITTER);
    }

    #[test]
    fn jitter_never_exceeds_its_bound() {
        for _ in 0..1000 {
            assert!(jitter() <= MAX_JITTER);
        }
    }

    /// Two consecutive delays differ, or the jitter would not be doing its job. The clock
    /// advances between the calls, so this is a statement about the derivation, not luck.
    #[test]
    fn jitter_actually_varies() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            seen.insert(jitter());
        }
        assert!(seen.len() > 1, "jitter was constant across 50 calls");
    }
}
