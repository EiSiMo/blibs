//! The portal's availability answer: JSON with an HTML fragment inside it.
//!
//! Two traps live here, and both produce a *wrong* result rather than a failure when got
//! wrong.
//!
//! **The HTML fragment carries no ISIL.** Item groups are matched against the JSON's
//! `isilAvailability` **by key order**, which is why `serde_json` is built with
//! `preserve_order` — and why this module additionally reads that member through its own
//! ordered visitor rather than trusting a Cargo feature to stay set. The matching is
//! three-stage: equal lengths means positional; otherwise by the library's portal name;
//! and whatever is left is kept **without** an ISIL. A group is never discarded.
//!
//! **The shelf table has more columns for newspapers than for books.** Columns are mapped
//! by their `<th>` text, never by position. `tr.avail-item.extra` rows count like any
//! other, and `data-more` is read as a counter-check: a mismatch is
//! [`crate::error::UnexpectedError::CountMismatch`], not a shortened list.

use crate::error::Error;
use crate::model::{Holding, Isil, Item, Note, Status};

/// The parsed availability response for one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityResponse {
    /// The overall traffic light for the record.
    pub overall: Status,
    /// The service's own `hasAvailability` flag. `false` is a note, not an error, and
    /// not an empty item list either.
    pub has_availability: bool,
    /// Per-ISIL traffic lights **in the order the JSON listed them**. The order is the
    /// only thing that connects them to the item groups.
    pub by_isil: Vec<(Isil, Status)>,
    /// Item groups from the HTML fragment, in document order.
    pub groups: Vec<ItemGroup>,
    /// The service's free-text availability line, when it supplies one.
    pub avail_text: Option<String>,
}

/// One library's block of copies in the HTML fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemGroup {
    /// The library name as the portal writes it — the fallback key when the positional
    /// match does not apply.
    pub portal_name: String,
    /// The copies.
    pub items: Vec<Item>,
    /// What `data-more` announced beyond the rows present, if anything.
    pub announced_extra: Option<usize>,
}

/// Parse the JSON envelope, including its embedded HTML fragment.
pub fn parse(_json: &str) -> Result<AvailabilityResponse, Error> {
    todo!("phase 2: availability parse")
}

/// Parse just the HTML fragment.
///
/// A missing table is [`crate::error::UnexpectedError::MissingSelector`] naming the
/// selector — never an empty list of shelfmarks, which would read as "this book has no
/// shelfmark".
pub fn parse_fragment(_html: &str) -> Result<Vec<ItemGroup>, Error> {
    todo!("phase 2: availability parse")
}

/// Merge a response into a record's holdings.
///
/// Returns notes for everything that could not be matched cleanly — an unmatched group,
/// a `hasAvailability: false`, a group left without an ISIL. Never drops a group and
/// never invents an ISIL for one.
pub fn merge(_response: &AvailabilityResponse, _holdings: &mut Vec<Holding>) -> Vec<Note> {
    todo!("phase 2: availability parse")
}
