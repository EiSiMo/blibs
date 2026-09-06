//! What was asked, where it was asked, and what came back.
//!
//! [`SearchResult`] is the JSON document. Its member order is the document's member order
//! and part of the contract in `plan/cli.md`.

use crate::model::{Engine, FetchWindow, Isil, Page, Record};

/// One search term.
///
/// The distinction comes from the shell and nowhere else: an argument that contains a
/// space was quoted by the user and is searched as a phrase; everything else is a word.
/// User input is never quoted on the user's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// A single word.
    Word(String),
    /// A quoted phrase.
    Phrase(String),
}

/// A standard number the user searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// A validated ISBN, normalised to digits. Validated **before** sending: the
    /// identifier index discards the check digit, so a typo returns the wrong book
    /// instead of nothing.
    Isbn(String),
    /// An ISSN, passed through — there the check digit is significant upstream.
    Issn(String),
}

/// The query, decomposed. Assembled once in `cli` and never mutated afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuerySpec {
    /// Free terms, `@and`-ed together.
    pub terms: Vec<Term>,
    /// `--title`.
    pub title: Option<Term>,
    /// `--subject`.
    pub subject: Option<Term>,
    /// `--publisher`.
    pub publisher: Option<Term>,
    /// `--author`, always kept as a raw string because it is **always** searched as a
    /// word list, never as a phrase: `"Kafka, Franz"` finds 2911 records, `"Franz
    /// Kafka"` as a phrase finds 40.
    pub author: Option<String>,
    /// `--year`. Means "was running in this year" for serials, "published in" for
    /// monographs.
    pub year: Option<u16>,
    /// `--isbn`/`--issn`.
    pub identifier: Option<Identifier>,
}

/// One resolved entry of `--at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// What the user typed, normalised to the canonical alias.
    pub key: String,
    /// The canonical ISIL. Never the user's spelling — the upstream filter is
    /// case-sensitive.
    pub isil: Isil,
    /// The branch, when the location names one.
    pub branch: Option<BranchRef>,
    /// Which engine answers for this location.
    pub engine: Engine,
    /// Full display name for the block heading.
    pub display: String,
}

/// A branch of a library network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// The branch's KOBV id, e.g. `SIG00036`.
    pub kobvid: String,
    /// The branch's name.
    pub name: String,
}

/// Everything one engine needs to run one search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The query.
    pub query: QuerySpec,
    /// The locations this engine is responsible for. Empty means "no `--at`".
    pub locations: Vec<Location>,
    /// The record window to fetch.
    pub window: FetchWindow,
    /// Whether per-location totals should be fetched. They cost one extra request each.
    pub want_totals: bool,
}

/// What one engine returned, before selection and before availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSearch {
    /// Which engine produced this.
    pub engine: Engine,
    /// Total hits upstream, when the service states one.
    pub total: Option<u64>,
    /// How many records were delivered.
    pub fetched: usize,
    /// How many records the envelope announced but did not deliver.
    pub undelivered: usize,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// The records.
    pub records: Vec<Record>,
    /// The query as it was sent, for the JSON echo.
    pub query_echo: Option<String>,
    /// Limitations that are not errors.
    pub notes: Vec<Note>,
}

/// One entry of `at[]`: a location, and the true number of hits at it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AtBlock {
    /// The alias as it appears in `--at`.
    pub key: String,
    /// The canonical ISIL.
    pub isil: Isil,
    /// The branch id, when the location is a branch.
    pub branch: Option<String>,
    /// The engine that answered for it.
    pub engine: Engine,
    /// Hits at this location. `None` when the engine cannot state one.
    pub total: Option<u64>,
}

/// Whether availability was fetched.
///
/// This is what makes `items: []` unambiguous. Without it, "we did not ask" and "we asked
/// and got nothing" look identical — exactly the kind of ambiguity an agent cannot
/// resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityMode {
    /// Availability was requested for every displayed record.
    Fetched,
    /// `--no-availability`: nothing was asked.
    Skipped,
}

/// How large the fetched window was and what survived the client-side filters.
///
/// Both numbers are printed. Without them, "no hits" and "no hits *in the first 50*" are
/// indistinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    /// How many records came back.
    pub fetched: usize,
    /// How many were left after `--format`/`--language`.
    pub after_filter: usize,
    /// How many the envelope announced but did not deliver.
    #[serde(skip_serializing_if = "is_zero")]
    pub undelivered: usize,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature is dictated by serde's skip_serializing_if"
)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// How the displayed records were ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SortSpec {
    /// The key.
    pub by: SortKey,
    /// What the sort could see. Always [`SortScope::Fetched`] — SRU cannot sort at all,
    /// so every sort in this tool is client-side over the fetched window. The output
    /// must never imply otherwise.
    pub scope: SortScope,
}

/// What to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Upstream order, which is relevance.
    #[default]
    Relevance,
    /// Publication year, newest first.
    Year,
    /// Displayed title, article included.
    Title,
    /// First author.
    Author,
    /// Availability, best first. Costs no extra request: the data is already there for
    /// every displayed record.
    Availability,
}

/// The scope a sort had access to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortScope {
    /// Only the records that were fetched.
    #[default]
    Fetched,
}

/// A limitation that is not an error and must not be silently swallowed: a per-record
/// diagnostic, `hasAvailability: false`, an item group that carries no ISIL, a branch
/// facet that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Note {
    /// Stable machine-readable tag.
    pub kind: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

impl Note {
    /// Build a note.
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The query, echoed back so a result can be reproduced without the shell history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueryEcho {
    /// The terms as the user typed them.
    pub terms: String,
    /// The assembled PQF query, when a PQF engine ran.
    pub pqf: Option<String>,
}

/// The complete result of one invocation — and the JSON document.
///
/// Record-centric, not location-centric: a record appears **once** in `records` even when
/// it is displayed under three location blocks. The grouping humans see is derived from
/// `at[]` and `holdings[].isil`, so there is one schema, not two.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    /// What was searched.
    pub query: QueryEcho,
    /// Total hits of the KOBV search. `None` when only `voebb` ran.
    pub total: Option<u64>,
    /// How many records are displayed.
    pub shown: usize,
    /// The page that was displayed.
    pub page: Page,
    /// How the displayed records were ordered.
    pub sort: SortSpec,
    /// What the client-side filters could see.
    pub window: WindowInfo,
    /// Which engines ran.
    pub engines: Vec<Engine>,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// Whether availability was fetched.
    pub availability: AvailabilityMode,
    /// Limitations that are not errors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The records.
    pub records: Vec<Record>,
}
