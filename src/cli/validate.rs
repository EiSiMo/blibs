//! Everything that must be refused before a request is built.
//!
//! Each of these is an exit 2 with a message that names the limit:
//!
//! | input | why |
//! | --- | --- |
//! | `*` or `?` in a term | truncation does not exist (diagnostic 1/48) |
//! | `--year 1990-2000` | ranges do not exist, and return **zero hits silently** |
//! | `--year` not four digits | `dc.date` knows four digits |
//! | `--isbn` with a bad check digit | the index ignores it and returns the wrong book |
//! | empty query | diagnostic 1/10 |
//! | query over 1000 characters | HTTP 414, returned as diagnostic 1/2 |
//! | unknown library in `--at` | with up to three suggestions |
//! | `--limit` outside 1..=50 | SRU caps at 50 silently |
//!
//! `--at` also decides the engines, and refuses the combinations that are not measured
//! yet: paging on the voebb side is exit 2 until it has been.

use crate::cli::{Plan, SearchArgs};
use crate::error::UsageError;
use crate::model::{Engine, Location};

/// Validate a search invocation and resolve everything it names.
pub fn validate(_args: &SearchArgs) -> Result<Plan, UsageError> {
    todo!("phase 3: cli validate")
}

/// Resolve a comma-separated `--at` list, preserving order and rejecting the first
/// unknown entry.
pub fn locations(_list: &str) -> Result<Vec<Location>, UsageError> {
    todo!("phase 3: cli validate")
}

/// Split resolved locations by the engine that answers for them.
///
/// One engine per location, never merged: `--at STABI,HU,AGB` runs two searches and
/// renders three blocks, and the same edition may legitimately appear in more than one.
/// Records are never matched across catalogues.
pub fn split_by_engine(_locations: &[Location]) -> Vec<(Engine, Vec<Location>)> {
    todo!("phase 3: cli validate")
}
