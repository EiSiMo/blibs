//! Library identifiers (ISIL).

use std::fmt;

/// An ISIL such as `DE-11`, in the canonical spelling from the library list.
///
/// Compared **exactly**. The upstream holdings filter (Bib-1 attribute `1044`) is
/// case-sensitive: `de-11` matches nothing and does so silently. Case-insensitive
/// comparison exists in exactly one place — resolving what the user typed in
/// [`fn@crate::libraries::resolve`] — and its result is always the canonical form from the
/// list, never the user's spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct Isil(Box<str>);

impl Isil {
    /// Wrap an ISIL. The caller is responsible for it being the canonical spelling; ISILs
    /// coming out of catalogue data are wrapped as-is and never rejected.
    pub fn new(value: &str) -> Self {
        Self(Box::from(value))
    }

    /// The code as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-insensitive comparison. Only for resolving user input against the list —
    /// never for deciding whether two holdings are the same library.
    pub fn eq_ignore_case(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl fmt::Display for Isil {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
