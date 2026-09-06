//! JSON output: one document on stdout, and nothing else.
//!
//! The schema is derived from the domain types with `serde`, so it cannot drift away from
//! what the human renderer shows. Member order is part of the contract; adding a member
//! is allowed, renaming or removing one is a break.

use std::io::Write;

use crate::error::Error;

/// Write one document.
pub fn write<T: serde::Serialize>(_out: &mut impl Write, _value: &T) -> std::io::Result<()> {
    todo!("phase 3: render json")
}

/// Write the error object — `{ "error": { code, kind, message, hint } }`.
///
/// **The only place it is built.** `code`, `kind`, `message` and `hint` come from
/// [`Error::exit`], [`Error::kind`], the error's `Display` and [`Error::hint`]
/// respectively, so a new variant cannot forget one of them.
pub fn error(_out: &mut impl Write, _error: &Error) -> std::io::Result<()> {
    todo!("phase 3: render json")
}
