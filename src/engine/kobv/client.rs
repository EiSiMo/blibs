//! KOBV I/O. Builds requests, hands bodies to `parse`, and interprets nothing itself.

use crate::error::Error;
use crate::http::{Fetch, Request};
use crate::model::{AvailabilityId, FetchWindow};

use super::parse::availability::AvailabilityResponse;
use super::parse::sru::SruResponse;
use super::pqf::Pqf;

/// The SRU endpoint. Database `k2` — the same index the portal searches.
pub const SRU_BASE: &str = "https://kobv-sru.hosted.exlibrisgroup.com/k2";

/// The portal's availability service.
pub const AVAILABILITY_BASE: &str = "https://portal.kobv.de/AJAX/JSON";

/// Build a search request. Pure: takes a query and a window, returns a [`Request`].
pub fn search_request(_query: &Pqf, _window: FetchWindow) -> Request {
    todo!("phase 3: kobv client")
}

/// Build a counting request — `maximumRecords=0`, which returns `numberOfRecords` and no
/// records.
pub fn count_request(_query: &Pqf) -> Request {
    todo!("phase 3: kobv client")
}

/// Build an availability request.
///
/// Never cached: a stale traffic light is worse than none. One record per request — the
/// response is keyed by ISIL, so two records in one call would collide.
pub fn availability_request(_id: &AvailabilityId) -> Request {
    todo!("phase 3: kobv client")
}

/// Fetch and parse. The only place where the two halves meet.
pub struct KobvClient<'f> {
    fetch: &'f dyn Fetch,
}

impl<'f> KobvClient<'f> {
    /// Build the client over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self { fetch }
    }

    /// The transport this client was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.fetch
    }

    /// Run a search and parse the envelope. Top-level diagnostics become
    /// [`crate::error::RejectedError`] here, not an empty result.
    pub fn search(&self, _query: &Pqf, _window: FetchWindow) -> Result<SruResponse, Error> {
        todo!("phase 3: kobv client")
    }

    /// Ask only for the hit count.
    pub fn count(&self, _query: &Pqf) -> Result<u64, Error> {
        todo!("phase 3: kobv client")
    }

    /// Fetch availability for one record.
    pub fn availability(&self, _id: &AvailabilityId) -> Result<AvailabilityResponse, Error> {
        todo!("phase 3: kobv client")
    }
}
