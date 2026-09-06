//! Where the cache lives.
//!
//! Resolved by hand rather than with a crate: the rule is one line per platform, and this
//! is the only filesystem location the tool knows about.

use std::path::PathBuf;

/// `$XDG_CACHE_HOME/blibs`, else `$HOME/.cache/blibs`.
///
/// Returns `None` when neither is set — the tool then runs without a cache rather than
/// inventing a location.
pub fn cache_dir() -> Option<PathBuf> {
    todo!("phase 2: http")
}
