//! The per-host concurrency cap.
//!
//! Both services answer `robots.txt: Disallow: /`. This tool is user-initiated search
//! rather than crawling, which makes the rules stricter, not looser. Six in flight per
//! host, measured, and **not user-configurable** — a flag would only ever be turned up.
//!
//! One host asks for less than that, and says so with its latency rather than with a
//! header: voebb.de answers a single record page in 1.5–2.0 s, but requests that overlap
//! are let through in ten-second steps. Four record pages cost 7.9 s one after the other
//! and 21.7 s side by side — the parallel path is 2.7× *slower* (measured 2026-09-07,
//! `plan/client.md`). Its cap is therefore one, which is both the faster and the politer
//! setting; that is not a trade-off worth thinking about.

use std::sync::{Condvar, Mutex, PoisonError};

use crate::http::lock;

/// The cap for a host that has not asked for less. Not configurable, by design.
pub const MAX_IN_FLIGHT_PER_HOST: usize = 6;

/// Hosts that answer worse when asked concurrently, with the cap they get instead.
///
/// A host is named here only with a measurement behind it, never on suspicion, and the
/// name is matched exactly against [`crate::http::Request::host`]. Anything not in the
/// table gets [`MAX_IN_FLIGHT_PER_HOST`], so a service that moves or gains a host is
/// slower than it could be and never broken.
const HOST_CAPS: [(&str, usize); 1] = [("www.voebb.de", 1)];

/// How many requests may be in flight against `host` at once.
pub fn cap_for(host: &str) -> usize {
    HOST_CAPS
        .iter()
        .find(|(name, _)| *name == host)
        .map_or(MAX_IN_FLIGHT_PER_HOST, |(_, cap)| *cap)
}

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
        let cap = cap_for(host);
        let mut table = lock(&self.inner);
        loop {
            match table.iter_mut().find(|(name, _)| name == host) {
                Some(entry) if entry.1 < cap => {
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
        // `notify_all`, not `notify_one`: one condvar serves every host, so the single
        // woken thread may well be waiting for a different one. It would find its host
        // still full, go back to sleep, and the thread this slot was actually freed for
        // would never be woken at all. Two hosts and a cap of one make that a stall
        // rather than a theoretical race.
        self.limits.ready.notify_all();
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

    /// The host that measured worse under concurrency is capped at one, and the ones
    /// that did not keep the default. Named hosts rather than a generic rule: the table
    /// is the whole policy, and a test over it is what stops a fourth host from being
    /// added on a hunch.
    #[test]
    fn a_measured_host_is_capped_below_the_default() {
        assert_eq!(cap_for("www.voebb.de"), 1);
        assert_eq!(cap_for("sru.kobv.de"), MAX_IN_FLIGHT_PER_HOST);
        assert_eq!(cap_for("portal.kobv.de"), MAX_IN_FLIGHT_PER_HOST);
        // Not a prefix or suffix match: a host that merely looks related is not the one
        // that was measured.
        assert_eq!(cap_for("voebb.de"), MAX_IN_FLIGHT_PER_HOST);
    }

    /// The cap of one is a serialisation, proven the same way the default cap is: four
    /// threads race for voebb.de and the observed peak stays at one.
    #[test]
    fn a_host_capped_at_one_never_overlaps() {
        let limits = HostLimits::new();
        let in_flight = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let _slot = limits.acquire("www.voebb.de");
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(limits.in_flight("www.voebb.de"), 0);
    }

    /// A thread waiting on a full host is woken when *its* host frees a slot, even while
    /// another host is releasing slots of its own. With one condvar for every host and
    /// `notify_one` this deadlocks: the wrong waiter is woken, sleeps again, and the
    /// right one is never reached.
    #[test]
    fn a_waiter_is_woken_by_its_own_host_not_by_another() {
        let limits = HostLimits::new();
        let voebb = limits.acquire("www.voebb.de");
        let entered = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            // Two waiters on the capped host, so that a single notification cannot serve
            // them both by accident.
            for _ in 0..2 {
                scope.spawn(|| {
                    let _slot = limits.acquire("www.voebb.de");
                    entered.fetch_add(1, Ordering::SeqCst);
                });
            }
            std::thread::sleep(Duration::from_millis(20));
            // Traffic on the other host releases slots without freeing theirs.
            for _ in 0..3 {
                drop(limits.acquire("sru.kobv.de"));
            }
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(
                entered.load(Ordering::SeqCst),
                0,
                "another host's release let a request through on a full one"
            );
            drop(voebb);
        });

        assert_eq!(entered.load(Ordering::SeqCst), 2);
        assert_eq!(limits.in_flight("www.voebb.de"), 0);
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
