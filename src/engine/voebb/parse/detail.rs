//! A record's detail page, including its complete item table.
//!
//! The item table's columns are mapped by their header text, never by position — the same
//! rule as on the KOBV side, for the same reason.

use crate::error::Error;
use crate::model::{Item, Record};

/// One record with every copy the network holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    /// The bibliographic record.
    pub record: Record,
    /// Every copy, across all branches.
    pub items: Vec<Item>,
}

/// Parse a detail page. `Ok(None)` when the page says the record does not exist.
pub fn parse(_html: &str) -> Result<Option<Detail>, Error> {
    todo!("phase 5: voebb parse")
}
