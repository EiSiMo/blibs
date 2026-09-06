//! `voebb.de` — the public library network, one entry per edition, branch-aware.
//!
//! This engine exists for one reason: the KOBV record does **not** know which branch
//! holds a VÖBB copy (verified: `924 $b DE-609` and nothing else), and a work is
//! scattered over many KOBV records, so a branch filter there could never prove absence.
//! voebb.de has one entry per edition listing every branch. Evidence in `plan/voebb.md`.
//!
//! The site is aDISWeb: session-bound, form-driven, and it answers a broken form with
//! **HTTP 200 and a `/noaccess` page** — a lost cookie, a stale `requestCount`, a missing
//! hidden field. That page is a named error ([`crate::error::UnexpectedError::VoebbNoAccess`],
//! exit 6) and never "no hits". Every step checks for it before parsing anything.
//!
//! Nothing is cached here: a cached session step is meaningless at best.

pub mod client;
pub mod parse;
pub mod session;

use crate::error::Error;
use crate::http::Fetch;
use crate::model::{
    AvailabilityMode, Catalog, Engine, EngineSearch, Note, Record, RecordId, SearchRequest,
};

/// The voebb.de engine.
pub struct Voebb<'f> {
    fetch: &'f dyn Fetch,
}

impl<'f> Voebb<'f> {
    /// Build the engine over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self { fetch }
    }

    /// The transport this engine was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.fetch
    }
}

impl Catalog for Voebb<'_> {
    fn engine(&self) -> Engine {
        Engine::Voebb
    }

    fn search(&self, _req: &SearchRequest) -> Result<EngineSearch, Error> {
        todo!("phase 5: voebb engine")
    }

    /// voebb.de delivers the item list with the record, so this is usually a no-op —
    /// but it stays part of the interface so that `cli` never has to know which engine
    /// answered.
    fn fill_availability(&self, _records: &mut [Record]) -> Result<Vec<Note>, Error> {
        todo!("phase 5: voebb engine")
    }

    fn show(&self, _id: &RecordId, _mode: AvailabilityMode) -> Result<Option<Record>, Error> {
        todo!("phase 5: voebb engine")
    }
}
