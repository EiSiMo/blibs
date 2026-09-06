//! Where the cache lives.
//!
//! Resolved by hand rather than with a crate: the rule is one line per platform, and this
//! is the only filesystem location the tool knows about.

use std::ffi::OsString;
use std::path::PathBuf;

/// The directory name appended to the cache root.
const DIR_NAME: &str = "blibs";

/// `$XDG_CACHE_HOME/blibs`, else `$HOME/.cache/blibs`.
///
/// Returns `None` when neither is set — the tool then runs without a cache rather than
/// inventing a location. An empty or relative `$XDG_CACHE_HOME` is ignored the way the
/// XDG specification asks: the variable counts only when it names an absolute path,
/// because a relative one would scatter cache files wherever the shell happened to stand.
///
/// These two variables are the only ones the crate reads, and neither configures
/// behaviour: the cache changes latency and never a result.
pub fn cache_dir() -> Option<PathBuf> {
    resolve(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

/// The decision itself, taken on values rather than on the process environment so that it
/// can be tested without a global mutation.
fn resolve(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(base) = absolute(xdg_cache_home) {
        return Some(base.join(DIR_NAME));
    }
    Some(absolute(home)?.join(".cache").join(DIR_NAME))
}

/// `value` as an absolute path, or `None` when it is missing, empty or relative.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    if path.is_absolute() { Some(path) } else { None }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// An environment value, shaped the way `resolve` receives it.
    fn env(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn xdg_cache_home_wins() {
        let dir = resolve(Some(env("/xdg")), Some(env("/home/someone")));
        assert_eq!(dir, Some(PathBuf::from("/xdg/blibs")));
    }

    #[test]
    fn home_is_the_fallback() {
        let dir = resolve(None, Some(env("/home/someone")));
        assert_eq!(dir, Some(PathBuf::from("/home/someone/.cache/blibs")));
    }

    #[test]
    fn a_relative_or_empty_xdg_value_falls_through_to_home() {
        let home = PathBuf::from("/home/someone/.cache/blibs");
        let relative = resolve(Some(env("relative/path")), Some(env("/home/someone")));
        assert_eq!(relative, Some(home.clone()));
        let empty = resolve(Some(env("")), Some(env("/home/someone")));
        assert_eq!(empty, Some(home));
    }

    #[test]
    fn nothing_usable_means_no_cache_rather_than_a_guess() {
        assert_eq!(resolve(None, None), None);
        let both_relative = resolve(Some(env("also/relative")), Some(env("relative/home")));
        assert_eq!(both_relative, None);
    }
}
