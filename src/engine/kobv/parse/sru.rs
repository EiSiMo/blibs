//! The SRU envelope.
//!
//! What is an error here and what is not has been settled against live responses:
//!
//! - **Errors.** A body that is not XML at all (an HTML page with status 200 is the usual
//!   form), and a missing `numberOfRecords`.
//! - **Not errors.** Fewer records than announced — SRU caps `maximumRecords` at 50
//!   silently — XML comments *inside* a `<record>` element, and per-record surrogate
//!   diagnostics. The last two are real: `sru_newspaper.xml` has comments between record
//!   children, so children must be filtered to elements; `sru_kids.xml` delivers a
//!   diagnostic at record position 49, leaving 49 valid records and one note.
//! - **Rejected upstream.** A top-level `<diagnostics>` element is
//!   [`crate::error::RejectedError`], exit 5 — the service understood the query and
//!   refused it.

use crate::error::Error;
use crate::model::Note;

/// The parsed envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SruResponse {
    /// What the service says the total is. Never assumed to equal `records.len()`.
    pub number_of_records: u64,
    /// Where the next window starts, when the service says.
    pub next_record_position: Option<u32>,
    /// The delivered records, in delivery order.
    pub records: Vec<SruRecord>,
    /// Top-level diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// One `<zs:record>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SruRecord {
    /// `recordPosition`, kept as delivered. Never renumbered — the fixtures rely on the
    /// original numbering to reproduce the surrogate-diagnostic case.
    pub position: Option<u32>,
    /// What the record actually contained. The `recordSchema` is checked per record, not
    /// once for the response.
    pub payload: RecordPayload,
}

/// What one delivered record turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordPayload {
    /// A MARCXML record, as raw XML for [`super::marc`] to take apart.
    Marc(String),
    /// A surrogate diagnostic in place of a record.
    Diagnostic(Diagnostic),
    /// A schema this tool does not know. Kept rather than dropped, so it can be counted
    /// and reported in a note.
    Unknown {
        /// The declared `recordSchema`.
        schema: String,
    },
}

/// An SRU diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The diagnostic URI, e.g. `info:srw/diagnostic/1/48`.
    pub uri: String,
    /// The service's message.
    pub message: Option<String>,
    /// Its `details` element.
    pub details: Option<String>,
}

/// Parse an SRU response body.
///
/// Fails with [`crate::error::UnexpectedError::NotXml`] for a body that does not parse,
/// and with [`crate::error::UnexpectedError::MissingElement`] when the envelope has no
/// `numberOfRecords`. Never fails because there are no records.
pub fn parse(_body: &str) -> Result<SruResponse, Error> {
    todo!("phase 2: sru parse")
}

/// Turn top-level diagnostics into an error.
///
/// Separate from [`parse`] so that a caller can look at a partially usable response
/// before deciding; `search` calls it immediately.
pub fn check(_response: &SruResponse) -> Result<(), Error> {
    todo!("phase 2: sru parse")
}

/// Notes for everything that limited the response without breaking it: per-record
/// diagnostics, unknown schemas, and records the envelope announced but did not deliver.
pub fn record_notes(_response: &SruResponse) -> Vec<Note> {
    todo!("phase 2: sru parse")
}
