//! The engine interface.

use crate::error::Error;
use crate::model::{AvailabilityMode, Engine, EngineSearch, Note, Record, RecordId};

/// What both catalogues can do.
///
/// Three methods rather than two, because [`crate::select`] sits between search and
/// availability: the tool only ever asks about the records it is actually going to
/// display. That way the engine never has to know about selection, and `cli` never has to
/// know about the availability call.
///
/// `Sync` because `cli` runs one engine per thread against a shared `&dyn Fetch`.
pub trait Catalog: Sync {
    /// Which catalogue this is.
    fn engine(&self) -> Engine;

    /// Run a search. Returns records **without** availability — no copy-level data is
    /// fetched here.
    fn search(&self, req: &crate::model::SearchRequest) -> Result<EngineSearch, Error>;

    /// Fetch availability for the records that will be displayed, and only those.
    ///
    /// One request per record, mapped over the worker pool and capped by the host it
    /// goes to ([`crate::http::limit::cap_for`]) — six in flight for the KOBV portal, one
    /// for voebb.de. Never batched: the response is keyed by ISIL and two records' keys
    /// collide. Records with no holdings are skipped rather than asked about.
    ///
    /// Returns the notes for anything that was limited but not wrong.
    fn fill_availability(&self, records: &mut [Record]) -> Result<Vec<crate::model::Note>, Error>;

    /// Fetch one record by id.
    ///
    /// An [`EngineShow`] whose `record` is `None` is "this catalogue has no such record"
    /// and becomes exit 1. A rejected query is exit 5 and an implausible response exit 6 —
    /// the three are never merged.
    fn show(&self, id: &RecordId, mode: AvailabilityMode) -> Result<EngineShow, Error>;
}

/// What a catalogue answers a `show` with: the record, and what it could not say about it.
///
/// The notes are why this is not a bare `Option<Record>`. voebb.de states the loan state of
/// an e-lending title nowhere but in the text of its access link, so such a record arrives
/// with an empty `items[]` and a note is the *only* thing that explains it — and the
/// engines used to drop those notes on the floor, leaving `blibs show` of an Overdrive
/// title with a silently empty copy list. That is the failure mode CLAUDE.md forbids above
/// all others: an empty list is indistinguishable from "held nowhere". `Kobv::show` lost
/// the availability notes to the same signature.
///
/// The notes join the ones [`crate::model::ShowResult::new`] derives from the record
/// itself; both are printed under the holdings and both carry a `kind` from
/// [`crate::model::note_kinds`], so an agent branches on the tag either way.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineShow {
    /// The record, or `None` when this catalogue has no such id.
    pub record: Option<Record>,
    /// Limitations that are not errors.
    pub notes: Vec<Note>,
}
