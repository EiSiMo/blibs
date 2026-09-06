//! The on-disk response cache.
//!
//! It changes latency and never a result — which is also why **a cache error is never
//! fatal**. An unwritable directory, a truncated entry or a race with another process
//! degrades to "not cached", never to a failed search.

use std::path::PathBuf;

use crate::http::{Request, Response};

/// The cache key: `sha256(method + host + path + sorted query)`.
///
/// The query is sorted so that two invocations that differ only in parameter order share
/// an entry. Form fields are not part of the key: the only `POST`s in this crate are
/// voebb.de session steps, which are never cached.
pub fn cache_key(_request: &Request) -> String {
    todo!("phase 2: http")
}

/// A directory of cached responses, one file per key.
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Open (and create) the cache directory. Returns `None` when no usable directory
    /// exists — the caller then runs without a cache instead of failing.
    pub fn open() -> Option<Self> {
        todo!("phase 2: http")
    }

    /// Use a specific directory. For tests.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Read an entry, if it exists and is younger than `max_age_secs`. Age is the file's
    /// mtime; there is no metadata sidecar to keep in sync.
    pub fn get(&self, _key: &str, _max_age_secs: u64) -> Option<Response> {
        todo!("phase 2: http")
    }

    /// Write an entry. Writes to a temporary file and renames, so a reader never sees a
    /// half-written body. Failures are swallowed on purpose.
    pub fn put(&self, _key: &str, _response: &Response) {
        todo!("phase 2: http")
    }

    /// Delete entries older than `max_age_secs`. Runs at most once a day, gated by the
    /// mtime of a marker file, so that a cold `blibs libraries` never pays for it.
    pub fn sweep(&self, _max_age_secs: u64) {
        todo!("phase 2: http")
    }

    /// The directory being used.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }
}
