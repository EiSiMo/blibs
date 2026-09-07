//! A worker pool over borrowed data.
//!
//! `std::thread::scope` rather than an async runtime or a thread-pool crate: the tool
//! makes at most a few dozen requests per invocation, they are all I/O-bound, and a
//! scoped pool can borrow `&dyn Fetch` without an `Arc`. The argument is in
//! `plan/client.md`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::error::Error;
use crate::http::limit::MAX_IN_FLIGHT_PER_HOST;
use crate::http::lock;

/// Map `f` over `items` on up to [`crate::http::limit::MAX_IN_FLIGHT_PER_HOST`] threads,
/// preserving input order.
///
/// That is an upper bound on the threads, not on the requests: what a host actually
/// allows in flight is [`crate::http::limit::cap_for`], enforced inside [`Fetch`] where
/// the host is known. A pool of six against a host capped at one therefore runs one
/// request at a time with five threads asleep in `acquire` — the cap holds, whichever
/// engine or mix of hosts the caller happens to be mapping over.
///
/// [`Fetch`]: crate::http::Fetch
///
/// The first error wins and the remaining work is abandoned: one failed availability
/// call means the answer is incomplete, and an incomplete answer that looks complete is
/// the failure mode this crate exists to avoid. "First" is by *input* position, not by
/// the order the failures happened in, so the same input produces the same error however
/// the threads were scheduled.
///
/// Work already in progress when the first error appears is allowed to finish; only new
/// work is refused. A cancelled request would need a mechanism the transport does not
/// have, and the results are thrown away anyway.
///
/// Panics only if `f` panics — the panic is re-raised on the calling thread, as it would
/// be without the pool.
pub fn scope_map<T, U, F>(items: Vec<T>, f: F) -> Result<Vec<U>, Error>
where
    T: Send,
    U: Send,
    F: Fn(T) -> Result<U, Error> + Sync,
{
    // One item is the common case (a single search), and spawning a thread to wait for a
    // thread is pure overhead on a tool that is measured on cold start.
    if items.len() <= 1 {
        return items.into_iter().map(f).collect();
    }

    let workers = items.len().min(MAX_IN_FLIGHT_PER_HOST);
    let queue: Mutex<VecDeque<(usize, T)>> = Mutex::new(items.into_iter().enumerate().collect());
    let done: Mutex<Vec<(usize, U)>> = Mutex::new(Vec::new());
    let failures: Mutex<Vec<(usize, Error)>> = Mutex::new(Vec::new());
    let failed = AtomicBool::new(false);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !failed.load(Ordering::Acquire) {
                    let Some((index, item)) = lock(&queue).pop_front() else {
                        break;
                    };
                    match f(item) {
                        Ok(value) => lock(&done).push((index, value)),
                        Err(error) => {
                            lock(&failures).push((index, error));
                            failed.store(true, Ordering::Release);
                        }
                    }
                }
            });
        }
    });

    let mut failures = into_inner(failures);
    if !failures.is_empty() {
        failures.sort_by_key(|(index, _)| *index);
        // `remove(0)` cannot panic: the vector is non-empty and nothing else holds it.
        return Err(failures.remove(0).1);
    }

    let mut done = into_inner(done);
    done.sort_by_key(|(index, _)| *index);
    Ok(done.into_iter().map(|(_, value)| value).collect())
}

/// Unwrap a mutex after the workers have joined, recovering from poisoning.
fn into_inner<T>(mutex: Mutex<T>) -> T {
    mutex.into_inner().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;
    use crate::error::{NetworkError, UnexpectedError};

    fn boom(host: &str) -> Error {
        UnexpectedError::HttpStatus {
            host: host.to_owned(),
            status: 500,
        }
        .into()
    }

    #[test]
    fn nothing_in_nothing_out() {
        let out: Vec<u32> = scope_map(Vec::<u32>::new(), Ok).expect("no work, no error");
        assert!(out.is_empty());
    }

    #[test]
    fn a_single_item_needs_no_thread() {
        let out = scope_map(vec![7_u32], |item| Ok(item * 2)).expect("no error");
        assert_eq!(out, vec![14]);
    }

    #[test]
    fn a_single_item_still_propagates_its_error() {
        let error = scope_map(vec![7_u32], |_| Err::<u32, _>(boom("one.test")))
            .expect_err("the closure failed");
        assert!(error.to_string().contains("one.test"));
    }

    /// Output order is input order, whatever the threads did — proven by making the early
    /// items the slow ones, so that completion order is the reverse of input order.
    #[test]
    fn results_come_back_in_input_order() {
        let items: Vec<u64> = (0..24).collect();
        let out = scope_map(items, |item| {
            std::thread::sleep(Duration::from_millis(24 - item));
            Ok(item * 10)
        })
        .expect("no error");
        assert_eq!(out, (0..24).map(|item| item * 10).collect::<Vec<u64>>());
    }

    #[test]
    fn the_first_error_in_input_order_wins() {
        // Item 1 fails last in wall-clock time and first in input order; item 5 fails
        // immediately. The reported error must be item 1's.
        let out = scope_map(vec![0_u64, 1, 2, 3, 4, 5], |item| match item {
            1 => {
                std::thread::sleep(Duration::from_millis(60));
                Err(boom("first.test"))
            }
            5 => Err(boom("later.test")),
            other => Ok(other),
        });
        let error = out.expect_err("two items failed");
        assert!(
            error.to_string().contains("first.test"),
            "got {error} instead of the failure of the earliest item"
        );
    }

    #[test]
    fn no_new_work_starts_after_a_failure() {
        let started = AtomicUsize::new(0);
        let items: Vec<u64> = (0..200).collect();
        let out = scope_map(items, |item| {
            started.fetch_add(1, Ordering::SeqCst);
            if item == 0 {
                return Err(boom("stop.test"));
            }
            std::thread::sleep(Duration::from_millis(5));
            Ok(item)
        });
        assert!(out.is_err());
        let started = started.load(Ordering::SeqCst);
        assert!(
            started < 200,
            "all {started} items ran although the first one failed"
        );
    }

    #[test]
    fn concurrency_is_capped_at_the_worker_count() {
        let in_flight = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let items: Vec<u64> = (0..40).collect();
        scope_map(items, |item| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(item)
        })
        .expect("no error");
        let observed = peak.load(Ordering::SeqCst);
        assert!(
            observed > 1,
            "the work never overlapped; the test proves nothing"
        );
        assert!(
            observed <= MAX_IN_FLIGHT_PER_HOST,
            "{observed} items ran at once, the pool allows {MAX_IN_FLIGHT_PER_HOST}"
        );
    }

    #[test]
    fn errors_keep_their_structure() {
        let error = scope_map(vec![0_u8, 1], |_| {
            Err::<u8, _>(Error::from(NetworkError::Timeout {
                host: "sru.kobv.de".to_owned(),
                seconds: 10,
            }))
        })
        .expect_err("both items failed");
        assert!(matches!(
            error,
            Error::Network(NetworkError::Timeout { .. })
        ));
    }
}
