//! The on-disk response cache.
//!
//! It changes latency and never a result — which is also why **a cache error is never
//! fatal**. An unwritable directory, a truncated entry or a race with another process
//! degrades to "not cached", never to a failed search. Every method here therefore
//! swallows its I/O errors: there is no `Result` to propagate and nothing a user could do
//! about a full disk that they would not rather have the search finish without.
//!
//! The layout is deliberately primitive: one file per request, named by its key, holding
//! the raw body. Age is the file's mtime, so there is no metadata sidecar to keep in
//! sync, and a parser change never invalidates the cache because what is stored is the
//! response, not the interpretation of it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::http::{Method, Request};

/// How long a cached catalogue response is served before it is fetched again.
///
/// Catalogue records do not change by the hour and an acquisition shows up the next day
/// at the latest. Availability is not cached at all — a stale traffic light is worse than
/// no traffic light — which is expressed by [`crate::http::CachePolicy::Never`] on those
/// requests rather than by a second duration here.
pub const ENTRY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Entries older than this are deleted by [`Cache::sweep_if_due`].
pub const ENTRY_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How often the sweep may run. Sweeping on every start would cost cold-start time on
/// every invocation, including the ones that touch no network at all.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The marker whose mtime records the last sweep. Dot-prefixed so it can never collide
/// with a key, which is always 64 hex characters.
const SWEEP_MARKER: &str = ".last-sweep";

/// The cache key: `sha256(method + host + path + sorted query)`.
///
/// The query is sorted so that two invocations that differ only in parameter order share
/// an entry. Form fields are not part of the key: the only `POST`s in this crate are
/// voebb.de session steps, which are never cached.
pub fn cache_key(request: &Request) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.method.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(request.host().as_bytes());
    hasher.update(b"\n");
    hasher.update(request.path().as_bytes());
    let mut pairs: Vec<(&str, &str)> = request
        .query
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    pairs.sort_unstable();
    for (key, value) in pairs {
        // The separators are bytes that cannot occur in a key or value here, so two
        // different query lists cannot hash to the same input string.
        hasher.update(b"\n");
        hasher.update(key.as_bytes());
        hasher.update(b"\x00");
        hasher.update(value.as_bytes());
    }
    hex(&hasher.finalize())
}

/// Lower-case hex, written by hand to avoid a dependency for eight lines.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)]);
        out.push(DIGITS[usize::from(byte & 0x0f)]);
    }
    out
}

/// A directory of cached responses, one file per key.
#[derive(Debug)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Open (and create) the cache directory. Returns `None` when no usable directory
    /// exists — the caller then runs without a cache instead of failing.
    pub fn open() -> Option<Self> {
        let root = super::cachedir::cache_dir()?;
        std::fs::create_dir_all(&root).ok()?;
        Some(Self { root })
    }

    /// Use a specific directory. For tests.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Read an entry, if it exists and is younger than `max_age`. Age is the file's
    /// mtime; there is no metadata sidecar to keep in sync.
    ///
    /// A missing file, an unreadable one, a clock that jumped backwards and a body that
    /// is not UTF-8 all mean the same thing here: not cached.
    pub fn get(&self, key: &str, max_age: Duration) -> Option<String> {
        let path = self.entry_path(key);
        if age(&path)? > max_age {
            return None;
        }
        std::fs::read_to_string(&path).ok()
    }

    /// Write an entry. Writes to a temporary file and renames, so a concurrent reader
    /// never sees a half-written body. Failures are swallowed on purpose.
    pub fn put(&self, key: &str, body: &str) {
        if std::fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let temp = self.root.join(temp_name(key));
        if std::fs::write(&temp, body).is_err() {
            // Leaving a stray temp file behind would be worse than the failed write; the
            // sweep would only reach it after 30 days.
            let _ = std::fs::remove_file(&temp);
            return;
        }
        if std::fs::rename(&temp, self.entry_path(key)).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }

    /// Delete entries older than [`ENTRY_MAX_AGE`], at most once every
    /// [`SWEEP_INTERVAL`], gated by the mtime of a marker file — so that a cold
    /// `blibs libraries` never pays for it.
    ///
    /// The marker is touched *before* the sweep, so two processes starting at the same
    /// moment do not both walk the directory, and a sweep that dies half way still counts
    /// as done. Nothing here is load-bearing: a skipped sweep only costs disk.
    pub fn sweep_if_due(&self) {
        let marker = self.root.join(SWEEP_MARKER);
        if age(&marker).is_some_and(|since_last| since_last < SWEEP_INTERVAL) {
            return;
        }
        if std::fs::write(&marker, b"").is_err() {
            return;
        }
        self.sweep(ENTRY_MAX_AGE);
    }

    /// Delete every entry older than `max_age`, unconditionally. [`Cache::sweep_if_due`]
    /// is what the request path calls; this is the part that does the work, exposed so a
    /// test can drive it without waiting a day.
    pub fn sweep(&self, max_age: Duration) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == SWEEP_MARKER) {
                continue;
            }
            if age(&path).is_some_and(|age| age > max_age) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// The directory being used.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file an entry lives in.
    fn entry_path(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }
}

/// How long ago `path` was last written, or `None` when it does not exist, carries no
/// mtime, or claims to have been written in the future.
fn age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// A temp file name that no two concurrent writers can share: process id plus the
/// nanosecond the write started.
fn temp_name(key: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!("{key}.{}.{nanos}.tmp", std::process::id())
}

/// Whether a request may use the cache at all: only `GET`, only under
/// [`crate::http::CachePolicy::Normal`].
///
/// A `POST` is a voebb.de session step whose answer depends on state that is not in the
/// key, and `Never` marks the live data — availability — where "old" means "wrong".
pub fn is_cacheable(request: &Request) -> bool {
    request.method == Method::Get && request.cache == super::CachePolicy::Normal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::CachePolicy;

    fn temp_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().expect("the test needs a writable temp directory");
        let cache = Cache::at(dir.path().to_path_buf());
        (dir, cache)
    }

    /// Backdate a file so that an age-dependent branch can be tested without sleeping.
    fn backdate(path: &Path, by: Duration) {
        let when = SystemTime::now() - by;
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the test just created this file");
        file.set_modified(when)
            .expect("the temp filesystem must support setting mtime");
    }

    #[test]
    fn the_key_is_stable_and_ignores_query_order() {
        let one = Request::get("https://sru.kobv.de/k2")
            .query("operation", "searchRetrieve")
            .query("x-pquery", "@attr 1=4 \"Prozess\"");
        let other = Request::get("https://sru.kobv.de/k2")
            .query("x-pquery", "@attr 1=4 \"Prozess\"")
            .query("operation", "searchRetrieve");
        assert_eq!(cache_key(&one), cache_key(&other));
        assert_eq!(cache_key(&one).len(), 64);
        assert!(cache_key(&one).chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_different_value_is_a_different_key() {
        let one = Request::get("https://sru.kobv.de/k2").query("x-pquery", "a");
        let other = Request::get("https://sru.kobv.de/k2").query("x-pquery", "b");
        assert_ne!(cache_key(&one), cache_key(&other));
    }

    /// `a=1&bc=2` and `ab=1&c=2` must not collide — the reason keys and values are
    /// separated by bytes that cannot occur in either.
    #[test]
    fn key_and_value_boundaries_cannot_be_shifted() {
        let one = Request::get("https://x.test/p")
            .query("a", "1")
            .query("bc", "2");
        let other = Request::get("https://x.test/p")
            .query("ab", "1")
            .query("c", "2");
        assert_ne!(cache_key(&one), cache_key(&other));
    }

    #[test]
    fn the_path_is_part_of_the_key() {
        let one = Request::get("https://x.test/one").query("q", "1");
        let other = Request::get("https://x.test/other").query("q", "1");
        assert_ne!(cache_key(&one), cache_key(&other));
    }

    #[test]
    fn a_write_can_be_read_back() {
        let (_dir, cache) = temp_cache();
        cache.put("abc", "<hello/>");
        assert_eq!(cache.get("abc", ENTRY_TTL).as_deref(), Some("<hello/>"));
    }

    #[test]
    fn a_missing_entry_is_not_an_error() {
        let (_dir, cache) = temp_cache();
        assert_eq!(cache.get("nothing-here", ENTRY_TTL), None);
    }

    #[test]
    fn an_entry_past_its_ttl_is_not_served() {
        let (_dir, cache) = temp_cache();
        cache.put("abc", "<hello/>");
        backdate(&cache.entry_path("abc"), Duration::from_secs(48 * 60 * 60));
        assert_eq!(cache.get("abc", ENTRY_TTL), None);
        // ... but the file is still there: reading is what expires, not the entry.
        assert!(cache.entry_path("abc").exists());
    }

    #[test]
    fn writing_leaves_no_temp_files_behind() {
        let (dir, cache) = temp_cache();
        cache.put("abc", "body");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("the temp directory exists")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["abc".to_string()]);
    }

    #[test]
    fn an_unwritable_directory_is_survivable() {
        // No directory, no permissions, no panic: the cache is allowed to do nothing.
        let cache = Cache::at(PathBuf::from("/proc/blibs-cannot-exist"));
        cache.put("abc", "body");
        assert_eq!(cache.get("abc", ENTRY_TTL), None);
        cache.sweep_if_due();
    }

    #[test]
    fn the_sweep_removes_only_old_entries() {
        let (_dir, cache) = temp_cache();
        cache.put("old", "gone");
        cache.put("fresh", "kept");
        backdate(
            &cache.entry_path("old"),
            Duration::from_secs(40 * 24 * 60 * 60),
        );
        cache.sweep(ENTRY_MAX_AGE);
        assert!(!cache.entry_path("old").exists());
        assert!(cache.entry_path("fresh").exists());
    }

    #[test]
    fn the_sweep_runs_at_most_once_a_day() {
        let (_dir, cache) = temp_cache();
        cache.put("old", "gone");
        backdate(
            &cache.entry_path("old"),
            Duration::from_secs(40 * 24 * 60 * 60),
        );

        cache.sweep_if_due();
        assert!(!cache.entry_path("old").exists());

        // Second call on the same day: the marker is fresh, so nothing is walked.
        cache.put("old", "back");
        backdate(
            &cache.entry_path("old"),
            Duration::from_secs(40 * 24 * 60 * 60),
        );
        cache.sweep_if_due();
        assert!(cache.entry_path("old").exists());
    }

    #[test]
    fn the_sweep_never_deletes_its_own_marker() {
        let (_dir, cache) = temp_cache();
        cache.sweep_if_due();
        let marker = cache.root().join(SWEEP_MARKER);
        backdate(&marker, Duration::from_secs(400 * 24 * 60 * 60));
        cache.sweep(ENTRY_MAX_AGE);
        assert!(marker.exists());
    }

    #[test]
    fn only_cacheable_requests_are_cacheable() {
        assert!(is_cacheable(&Request::get("https://x.test/p")));
        assert!(!is_cacheable(
            &Request::get("https://x.test/p").cache(CachePolicy::Never)
        ));
        assert!(!is_cacheable(&Request::post("https://x.test/p")));
    }
}
