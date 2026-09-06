//! voebb.de I/O: open a session, search, filter by branch, page, fetch a record.
//!
//! Every response passes [`super::parse::noaccess::check`] before any other parser sees
//! it. Cookies live in the shared `ureq` agent; nothing here is cached.

use crate::error::Error;
use crate::http::Fetch;

use super::parse::detail::Detail;
use super::parse::results::ResultList;
use super::session::Session;

/// The public entry point of the search form.
pub const VOEBB_BASE: &str = "https://www.voebb.de";

/// A session-bound client.
pub struct VoebbClient<'f> {
    fetch: &'f dyn Fetch,
}

impl<'f> VoebbClient<'f> {
    /// Build the client over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self { fetch }
    }

    /// The transport this client was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.fetch
    }

    /// Fetch the entry page and read the initial form state out of it.
    pub fn open(&self) -> Result<Session, Error> {
        todo!("phase 5: voebb client")
    }

    /// Submit the search form.
    pub fn search(&self, _session: &Session, _terms: &str) -> Result<(Session, ResultList), Error> {
        todo!("phase 5: voebb client")
    }

    /// Apply the branch facet to a result list.
    ///
    /// When the facet cannot be applied, the fallback is honest rather than clever: the
    /// facet's own count is still read and reported, the copies are sieved client-side,
    /// and a note says so.
    pub fn filter_branch(
        &self,
        _session: &Session,
        _branch: &str,
    ) -> Result<(Session, ResultList), Error> {
        todo!("phase 5: voebb client")
    }

    /// Advance to a further page of results.
    pub fn page(&self, _session: &Session, _page: u32) -> Result<(Session, ResultList), Error> {
        todo!("phase 5: voebb client")
    }

    /// Fetch one record's detail page. Stateless — the detail URL works without a
    /// session, which is what makes `show voebb_...` a single request.
    pub fn detail(&self, _local_id: &str) -> Result<Option<Detail>, Error> {
        todo!("phase 5: voebb client")
    }
}
