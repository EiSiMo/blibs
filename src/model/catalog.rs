//! The engine interface.

use crate::error::Error;
use crate::model::{AvailabilityMode, Engine, EngineSearch, Record, RecordId};

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
    /// One request per record, run concurrently and capped at six in flight. Never
    /// batched: the response is keyed by ISIL and two records' keys collide. Records with
    /// no holdings are skipped rather than asked about.
    ///
    /// Returns the notes for anything that was limited but not wrong.
    fn fill_availability(&self, records: &mut [Record]) -> Result<Vec<crate::model::Note>, Error>;

    /// Fetch one record by id.
    ///
    /// `Ok(None)` is "this catalogue has no such record" and becomes exit 1. A rejected
    /// query is exit 5 and an implausible response exit 6 — the three are never merged.
    fn show(&self, id: &RecordId, mode: AvailabilityMode) -> Result<Option<Record>, Error>;
}
