//! The per-host concurrency cap.
//!
//! Both services answer `robots.txt: Disallow: /`. This tool is user-initiated search
//! rather than crawling, which makes the rules stricter, not looser. Six in flight per
//! host, measured, and **not user-configurable** — a flag would only ever be turned up.

use std::sync::{Condvar, Mutex, PoisonError};

use crate::http::lock;

/// The cap. Not configurable, by design.
pub const MAX_IN_FLIGHT_PER_HOST: usize = 6;

/// A counting semaphore per host.
///
/// A mutex and a condvar rather than a channel: the wait is short, uncontended in the
/// common case, and the whole thing has to be `Sync` for a shared `&dyn Fetch`.
///
/// The table is a `Vec` rather than a map because it holds at most two entries — the two
/// catalogue hosts — and a linear scan over two strings is cheaper than hashing them.
#[derive(Debug, Default)]
pub struct HostLimits {
    inner: Mutex<Vec<(String, usize)>>,
    ready: Condvar,
}

impl HostLimits {
    /// A fresh, empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Block until this host has a free slot, then take it. The returned guard releases
    /// it on drop, so a `?` on the request path cannot leak a slot.
    ///
    /// A poisoned lock is recovered rather than propagated: the counters are plain
    /// integers, a panicking peer cannot have left them in a shape that makes another
    /// thread's request wrong, and refusing to search because of it would be worse.
    pub fn acquire(&self, host: &str) -> HostSlot<'_> {
        let mut table = lock(&self.inner);
        loop {
            match table.iter_mut().find(|(name, _)| name == host) {
                Some(entry) if entry.1 < MAX_IN_FLIGHT_PER_HOST => {
                    entry.1 += 1;
                    break;
                }
                // The host is at the cap: wait for a slot to be released and look again.
                Some(_) => {}
                None => {
                    table.push((host.to_owned(), 1));
                    break;
                }
            }
            table = self
                .ready
                .wait(table)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(table);
        HostSlot {
            limits: self,
            host: host.to_owned(),
        }
    }

    /// How many requests are currently in flight against `host`. For tests and for
    /// nothing else — the request path only ever calls [`HostLimits::acquire`].
    #[cfg(test)]
    fn in_flight(&self, host: &str) -> usize {
        lock(&self.inner)
            .iter()
            .find(|(name, _)| name == host)
            .map_or(0, |(_, count)| *count)
    }
}

/// A held slot. Releases on drop.
pub struct HostSlot<'a> {
    limits: &'a HostLimits,
    host: String,
}

impl Drop for HostSlot<'_> {
    fn drop(&mut self) {
        // A poisoned lock means another thread panicked while holding it. Releasing the
        // slot is still the right thing, and a leaked slot would stall every later
        // request to this host.
        let mut table = lock(&self.limits.inner);
        if let Some(entry) = table.iter_mut().find(|(host, _)| *host == self.host) {
            entry.1 = entry.1.saturating_sub(1);
        }
        drop(table);
        self.limits.ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_slot_is_released_on_drop() {
        let limits = HostLimits::new();
        {
            let _slot = limits.acquire("sru.kobv.de");
            assert_eq!(limits.in_flight("sru.kobv.de"), 1);
        }
        assert_eq!(limits.in_flight("sru.kobv.de"), 0);
    }

    #[test]
    fn hosts_are_counted_separately() {
        let limits = HostLimits::new();
        let _one = limits.acquire("sru.kobv.de");
        let _other = limits.acquire("www.voebb.de");
        assert_eq!(limits.in_flight("sru.kobv.de"), 1);
        assert_eq!(limits.in_flight("www.voebb.de"), 1);
    }

    /// The promise to the upstream services, proven rather than assumed: 20 threads race
    /// for the same host and the observed peak never exceeds the cap.
    #[test]
    fn twenty_threads_never_exceed_the_cap() {
        let limits = HostLimits::new();
        let in_flight = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..20 {
                scope.spawn(|| {
                    let _slot = limits.acquire("sru.kobv.de");
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    // Long enough that the threads actually overlap; without it a thread
                    // may finish before the next one starts and the peak would read 1.
                    std::thread::sleep(Duration::from_millis(20));
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        let observed = peak.load(Ordering::SeqCst);
        assert!(
            observed <= MAX_IN_FLIGHT_PER_HOST,
            "{observed} requests were in flight at once, cap is {MAX_IN_FLIGHT_PER_HOST}"
        );
        assert!(
            observed > 1,
            "the threads never overlapped; the test proves nothing"
        );
        assert_eq!(limits.in_flight("sru.kobv.de"), 0);
    }

    /// A full host does not hold up the other one: the two engines run side by side.
    #[test]
    fn the_cap_is_per_host_not_global() {
        let limits = HostLimits::new();
        let full: Vec<HostSlot<'_>> = (0..MAX_IN_FLIGHT_PER_HOST)
            .map(|_| limits.acquire("sru.kobv.de"))
            .collect();
        assert_eq!(limits.in_flight("sru.kobv.de"), MAX_IN_FLIGHT_PER_HOST);

        // If the cap were global this would block until `full` is dropped, which never
        // happens on this thread.
        let _other = limits.acquire("www.voebb.de");
        assert_eq!(limits.in_flight("www.voebb.de"), 1);
        drop(full);
    }

    /// The seventh request against a full host waits, and is let through by the release
    /// of a slot rather than by a timeout.
    #[test]
    fn a_full_host_makes_the_next_request_wait() {
        let limits = HostLimits::new();
        let full: Vec<HostSlot<'_>> = (0..MAX_IN_FLIGHT_PER_HOST)
            .map(|_| limits.acquire("sru.kobv.de"))
            .collect();
        let entered = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let _slot = limits.acquire("sru.kobv.de");
                entered.fetch_add(1, Ordering::SeqCst);
            });
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                entered.load(Ordering::SeqCst),
                0,
                "the seventh request was let through while the host was full"
            );
            drop(full);
            waiter.join().expect("the waiting thread must not panic");
        });

        assert_eq!(entered.load(Ordering::SeqCst), 1);
    }
}
