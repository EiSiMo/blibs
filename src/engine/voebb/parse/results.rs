//! The result list.

use crate::error::Error;
use crate::model::Record;

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultList {
    /// Hits the page states. `None` when the page does not state one — never guessed
    /// from the number of rows.
    pub total: Option<u64>,
    /// The records on this page, without their copies.
    pub records: Vec<Record>,
    /// Whether the page offers a next one.
    pub has_next: bool,
}

/// Parse a result page.
pub fn parse(_html: &str) -> Result<ResultList, Error> {
    todo!("phase 5: voebb parse")
}
