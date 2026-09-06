//! ISBN validation.
//!
//! This is not tidiness. `dc.identifier` throws away hyphens, the `978`/`979` prefix
//! **and the check digit** — so a mistyped ISBN does not return nothing, it returns a
//! *different book*: the example `978-3-596-29433-4` finds 17 editions of *Der
//! Zauberberg*. Validating before sending is the only way the user finds out.
//!
//! An ISSN is passed through untouched: there the check digit is significant upstream. A
//! DOI is not accepted at all — the index does not hold them.

use crate::error::UsageError;

/// Validate and normalise an ISBN to bare digits (`X` preserved for ISBN-10).
///
/// Accepts ISBN-10 and ISBN-13, with or without hyphens and spaces.
pub fn normalize(_input: &str) -> Result<String, UsageError> {
    todo!("phase 3: cli isbn")
}

/// Whether a 10-character candidate has a valid check digit. Position 10 may be `X`.
pub fn check_isbn10(_digits: &str) -> bool {
    todo!("phase 3: cli isbn")
}

/// Whether a 13-digit candidate has a valid check digit.
pub fn check_isbn13(_digits: &str) -> bool {
    todo!("phase 3: cli isbn")
}
