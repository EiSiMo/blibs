//! Backoff for the two failures that are worth retrying.
//!
//! Only 429, 503 and transport errors are retried. A 4xx is an answer, not a hiccup, and
//! retrying it would be noise the service did not ask for. When the attempts run out, the
//! result is [`crate::error::ServiceError::Throttled`] — a distinct exit code, not a
//! generic failure, so that an agent can back off instead of hammering.

use std::time::Duration;

/// How many times a request is attempted in total.
pub const MAX_ATTEMPTS: u32 = 3;

/// Per-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Base delays before attempts 2 and 3: 1 s and 2 s, then 4 s if the count ever grows.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Upper bound of the random jitter added to each delay.
pub const MAX_JITTER: Duration = Duration::from_millis(500);

/// Whether a status is worth another attempt.
pub fn is_retryable(_status: u16) -> bool {
    todo!("phase 2: http")
}

/// How long to wait before attempt `attempt` (1-based, so attempt 1 waits not at all).
///
/// Jitter is derived from the low bits of the system clock rather than from a random
/// number generator: a whole dependency for half a millisecond of spread is not a trade
/// worth making.
pub fn delay_for(_attempt: u32) -> Duration {
    todo!("phase 2: http")
}
