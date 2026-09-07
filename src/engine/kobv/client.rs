//! KOBV I/O. Builds requests, hands bodies to `parse`, and interprets nothing itself.
//!
//! The request builders are free functions and deliberately pure: a test can assert on
//! the exact query string that would go out — parameter for parameter, PQF included —
//! without a transport and without a fixture. [`KobvClient`] is the only place where the
//! two halves meet, and all it does is call [`Fetch`] and hand the body to `parse`.

use crate::error::Error;
use crate::http::{CachePolicy, Fetch, Request};
use crate::model::{AvailabilityId, FetchWindow};

use super::parse::availability::{self, AvailabilityResponse};
use super::parse::sru::{self, SruResponse};
use super::pqf::Pqf;

/// The SRU endpoint. Database `k2` — the same index the portal searches.
pub const SRU_BASE: &str = "https://sru.kobv.de/k2";

/// The portal's JSON endpoint, of which this tool uses exactly one method.
pub const PORTAL_AJAX: &str = "https://portal.kobv.de/AJAX/JSON";

/// The only SRU operation this tool performs.
const OPERATION: &str = "searchRetrieve";

/// The SRU version. 2.0 is what the endpoint answers and what every measurement in
/// `plan/scraping.md` was taken against.
const VERSION: &str = "2.0";

/// MARCXML, never `dc`: the Dublin Core mapping loses the holdings in `924`, which are
/// the whole point of this tool.
const RECORD_SCHEMA: &str = "marcxml";

/// The portal method that answers "is it in right now".
const AVAILABILITY_METHOD: &str = "getAvailability";

/// Build a search request. Pure: takes a query and a window, returns a [`Request`].
///
/// The query travels as `x-pquery`, never as `query`: same indexes and same counts, plus
/// Bib-1 attribute `1044`, which is the only way to filter by holdings upstream
/// (`plan/scraping.md` §A.4a). `maximumRecords` comes from a
/// [`SruPageSize`](crate::model::SruPageSize) and can therefore never exceed the 50 above
/// which the service truncates silently.
pub fn search_request(query: &Pqf, window: FetchWindow) -> Request {
    Request::get(SRU_BASE)
        .query("operation", OPERATION)
        .query("version", VERSION)
        .query("recordSchema", RECORD_SCHEMA)
        .query("maximumRecords", window.size.get().to_string())
        .query("startRecord", window.start.to_string())
        .query("x-pquery", query.as_str())
        .cache(CachePolicy::Normal)
}

/// Build an availability request.
///
/// Never cached: a stale traffic light is worse than none. One record per request — the
/// response is keyed by ISIL, so two records in one call would collide.
pub fn availability_request(id: &AvailabilityId) -> Request {
    Request::get(PORTAL_AJAX)
        .query("method", AVAILABILITY_METHOD)
        .query("availability_id", id.as_str())
        .cache(CachePolicy::Never)
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
    pub fn search(&self, query: &Pqf, window: FetchWindow) -> Result<SruResponse, Error> {
        self.envelope(&search_request(query, window))
    }

    /// Fetch availability for one record.
    ///
    /// The status of the response is not looked at: which statuses count as an answer is
    /// a decision of [`crate::http`], and here a 200 proves nothing anyway — the
    /// plausibility check is the parse.
    pub fn availability(&self, id: &AvailabilityId) -> Result<AvailabilityResponse, Error> {
        let response = self.fetch.fetch(&availability_request(id))?;
        availability::parse(&response.body)
    }

    /// Fetch one SRU request and turn its body into a checked envelope.
    ///
    /// [`sru::check`] runs here rather than in the caller because a top-level diagnostic
    /// invalidates the whole envelope: `numberOfRecords` may be present *and* wrong next
    /// to one, and a caller that read it would report a plausible number for a query the
    /// service refused.
    fn envelope(&self, request: &Request) -> Result<SruResponse, Error> {
        let response = self.fetch.fetch(request)?;
        let envelope = sru::parse(&response.body)?;
        sru::check(&envelope)?;
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Isil, Limit, Page, QuerySpec, Term};

    fn query() -> Pqf {
        let spec = QuerySpec {
            terms: vec![Term::from_argument("Vorleser")],
            ..QuerySpec::default()
        };
        super::super::pqf::search(&spec, &[Isil::new("DE-11")]).expect("the spec has a term")
    }

    /// The parameter set of `plan/scraping.md` §A.3/§A.4a, in the order it was measured
    /// in. The PQF reaches the wire percent-encoded and unaltered — that encoding is what
    /// [`Request::url`] is tested for; here the raw pairs are what matters.
    #[test]
    fn a_search_request_carries_the_measured_parameters() {
        let window = FetchWindow::plan(Limit::DEFAULT, Page::FIRST, false);
        let request = search_request(&query(), window);
        assert_eq!(request.base, SRU_BASE);
        assert_eq!(
            request.query,
            vec![
                ("operation".to_owned(), "searchRetrieve".to_owned()),
                ("version".to_owned(), "2.0".to_owned()),
                ("recordSchema".to_owned(), "marcxml".to_owned()),
                ("maximumRecords".to_owned(), "10".to_owned()),
                ("startRecord".to_owned(), "1".to_owned()),
                (
                    "x-pquery".to_owned(),
                    "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-11".to_owned()
                ),
            ]
        );
        assert_eq!(request.cache, CachePolicy::Normal);
    }

    /// The window is the only thing a later page changes.
    #[test]
    fn the_window_reaches_the_request() {
        let limit = Limit::new(25).expect("25 is in range");
        let page = Page::new(3).expect("3 is a page");
        let request = search_request(&query(), FetchWindow::plan(limit, page, false));
        assert!(
            request
                .query
                .contains(&("startRecord".to_owned(), "51".to_owned()))
        );
        assert!(
            request
                .query
                .contains(&("maximumRecords".to_owned(), "25".to_owned()))
        );
    }

    /// Availability is live data and session-free: never cached, never batched.
    #[test]
    fn an_availability_request_is_never_cached() {
        let id = AvailabilityId::new("DE-11;BV008885798,DE-1;275177939,");
        let request = availability_request(&id);
        assert_eq!(request.base, PORTAL_AJAX);
        assert_eq!(
            request.query,
            vec![
                ("method".to_owned(), "getAvailability".to_owned()),
                (
                    "availability_id".to_owned(),
                    "DE-11;BV008885798,DE-1;275177939,".to_owned()
                ),
            ]
        );
        assert_eq!(request.cache, CachePolicy::Never);
    }
}
