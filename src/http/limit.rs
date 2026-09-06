//! The per-host concurrency cap.
//!
//! Both services answer `robots.txt: Disallow: /`. This tool is user-initiated search
//! rather than crawling, which makes the rules stricter, not looser. Six in flight per
//! host, measured, and **not user-configurable** — a flag would only ever be turned up.

/// The cap. Not configurable, by design.
pub const MAX_IN_FLIGHT_PER_HOST: usize = 6;

/// A counting semaphore per host.
///
/// A mutex and a condvar rather than a channel: the wait is short, uncontended in the
/// common case, and the whole thing has to be `Sync` for a shared `&dyn Fetch`.
#[derive(Debug, Default)]
pub struct HostLimits {
    inner: std::sync::Mutex<Vec<(String, usize)>>,
    ready: std::sync::Condvar,
}

impl HostLimits {
    /// A fresh, empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Block until this host has a free slot, then take it. The returned guard releases
    /// it on drop, so a `?` on the request path cannot leak a slot.
    pub fn acquire(&self, _host: &str) -> HostSlot<'_> {
        todo!("phase 2: http")
    }
}

/// A held slot. Releases on drop.
pub struct HostSlot<'a> {
    limits: &'a HostLimits,
    host: String,
}

impl Drop for HostSlot<'_> {
    fn drop(&mut self) {
        let mut table = match self.limits.inner.lock() {
            Ok(table) => table,
            // A poisoned lock means another thread panicked while holding it. Releasing
            // the slot is still the right thing; there is nothing to recover.
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = table.iter_mut().find(|(host, _)| *host == self.host) {
            entry.1 = entry.1.saturating_sub(1);
        }
        drop(table);
        self.limits.ready.notify_one();
    }
}
