//! The KOBV union catalogue: 46.7 million records over ~88 institutions.
//!
//! Two services, one engine. The SRU endpoint answers the search — always as PQF via
//! `x-pquery`, never as CQL, because PQF is the only way to reach Bib-1 attribute `1044`
//! (ISIL) and thus to filter by holdings *upstream*. The portal's availability service
//! answers "is it in right now", one call per record.
//!
//! The traps this engine has to survive are listed in `CLAUDE.md`; the two that shape the
//! code most are that **HTTP 200 means nothing here** — diagnostics, silent truncation
//! and phantom records all arrive with status 200 — and that availability **must not be
//! batched**, because the response is keyed by ISIL and two records' keys collide.

pub mod client;
pub mod parse;
pub mod pqf;

use crate::error::Error;
use crate::http::Fetch;
use crate::model::{
    AvailabilityMode, Catalog, Engine, EngineSearch, Note, Record, RecordId, SearchRequest,
};

/// The KOBV engine.
pub struct Kobv<'f> {
    fetch: &'f dyn Fetch,
}

impl<'f> Kobv<'f> {
    /// Build the engine over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self { fetch }
    }

    /// The transport this engine was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.fetch
    }
}

impl Catalog for Kobv<'_> {
    fn engine(&self) -> Engine {
        Engine::Kobv
    }

    /// One search request, plus one counting request per `--at` location, run
    /// concurrently. The counting requests use `maximumRecords=0` and exist so that
    /// `at[].total` is the location's real hit count and not the size of the window.
    fn search(&self, _req: &SearchRequest) -> Result<EngineSearch, Error> {
        todo!("phase 3: kobv engine")
    }

    /// One availability call per record, concurrently, capped at six. Records with no
    /// MARC `924` are skipped rather than asked about — there is no key to ask with.
    fn fill_availability(&self, _records: &mut [Record]) -> Result<Vec<Note>, Error> {
        todo!("phase 3: kobv engine")
    }

    fn show(&self, _id: &RecordId, _mode: AvailabilityMode) -> Result<Option<Record>, Error> {
        todo!("phase 3: kobv engine")
    }
}
