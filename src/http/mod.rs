//! Transport. **This module interprets nothing.**
//!
//! It hands back bytes and a status; whether those bytes are plausible is decided in a
//! `parse` module, never here. That split is what makes the whole crate testable: the
//! seam between the tool and the network is the [`Fetch`] trait, and tests substitute a
//! fixture-backed implementation for it. There is no environment switch and no test
//! back door in the binary.
//!
//! Upstream etiquette is enforced here and is not configurable (`CLAUDE.md`):
//!
//! - at most [`limit::MAX_IN_FLIGHT_PER_HOST`] requests in flight per host,
//! - an honest [`USER_AGENT`] naming the tool and its repository,
//! - an on-disk cache so a repeated agent query does not hit the service again,
//! - backoff on 429/503, surfaced as a distinct error rather than a generic failure.

pub mod cache;
pub mod cachedir;
pub mod limit;
pub mod pool;
pub mod retry;

use crate::error::Error;

pub use pool::scope_map;

/// The `User-Agent` every request carries. Names the tool and where to complain about it.
pub const USER_AGENT: &str = concat!(
    "blibs/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/EiSiMo/blibs)"
);

/// HTTP methods this crate uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Everything on the KOBV side.
    Get,
    /// The voebb.de session forms.
    Post,
}

impl Method {
    /// The method name for the wire and for cache keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Whether this request may be served from, or written to, the on-disk cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachePolicy {
    /// Read and write. The default for catalogue searches.
    #[default]
    Normal,
    /// Never cached, in either direction. Availability is live data; a cached traffic
    /// light is worse than no traffic light. voebb.de is session-bound and cached
    /// responses would be nonsense.
    Never,
}

/// One request, fully described. Built in an engine's `client`, executed by [`Fetch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`.
    pub method: Method,
    /// Scheme, host and path — everything before the query string.
    pub base: String,
    /// Query parameters, unencoded.
    pub query: Vec<(String, String)>,
    /// Form fields for a `POST`, unencoded.
    pub form: Vec<(String, String)>,
    /// Whether the cache may be used.
    pub cache: CachePolicy,
    /// Statuses other than 200 that are not an error for this request. Empty means only
    /// 200 will do.
    pub accept_status: &'static [u16],
}

impl Request {
    /// A `GET` against `base`.
    pub fn get(base: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            base: base.into(),
            query: Vec::new(),
            form: Vec::new(),
            cache: CachePolicy::Normal,
            accept_status: &[],
        }
    }

    /// A `POST` against `base`.
    pub fn post(base: impl Into<String>) -> Self {
        Self {
            method: Method::Post,
            ..Self::get(base)
        }
    }

    /// Add one query parameter.
    #[must_use]
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    /// Add one form field.
    #[must_use]
    pub fn form(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.form.push((key.into(), value.into()));
        self
    }

    /// Set the cache policy.
    #[must_use]
    pub fn cache(mut self, policy: CachePolicy) -> Self {
        self.cache = policy;
        self
    }

    /// Declare extra acceptable statuses.
    #[must_use]
    pub fn accept_status(mut self, statuses: &'static [u16]) -> Self {
        self.accept_status = statuses;
        self
    }

    /// The host, for the per-host cap, the cache key and error messages.
    ///
    /// Falls back to the whole base string when it does not look like a URL — an error
    /// message with a slightly odd host is better than a panic on a path this crate
    /// controls anyway.
    pub fn host(&self) -> &str {
        todo!("phase 2: http")
    }
}

/// A response. Body is a `String` because both services answer in text (XML, JSON, HTML)
/// and every parser in this crate wants `&str`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body.
    pub body: String,
    /// The declared content type, if any. Only ever used for diagnostics — **the content
    /// type is not evidence**; a 200 with `text/xml` can still be an HTML error page.
    pub content_type: Option<String>,
    /// Whether this came off the disk cache. Never changes a result, only latency.
    pub from_cache: bool,
}

/// The seam between this tool and the network.
///
/// `Sync` because one `&dyn Fetch` is shared across the engine threads and the
/// availability worker pool.
pub trait Fetch: Sync {
    /// Execute one request.
    fn fetch(&self, request: &Request) -> Result<Response, Error>;
}

/// Any `Fn(&Request) -> Result<Response, Error>` is a [`Fetch`].
///
/// This is what lets a test express a stub in one closure instead of a struct, and it is
/// why no test helper has to live in the binary.
impl<F> Fetch for F
where
    F: Fn(&Request) -> Result<Response, Error> + Sync,
{
    fn fetch(&self, request: &Request) -> Result<Response, Error> {
        self(request)
    }
}

/// The real client: cache read → per-host cap → retry → `ureq` → cache write.
#[expect(
    dead_code,
    reason = "the three fields are wired up in phase 2 (http); the expectation fires as \
              soon as they are, which is the signal to delete this attribute"
)]
pub struct Http {
    agent: ureq::Agent,
    cache: Option<cache::Cache>,
    limits: limit::HostLimits,
}

impl Http {
    /// Build the client. `use_cache` is `false` under `--no-cache`.
    ///
    /// Cookies are enabled on the agent because voebb.de is session-bound; the KOBV side
    /// ignores them.
    pub fn new(_use_cache: bool) -> Self {
        todo!("phase 2: http")
    }
}

impl Fetch for Http {
    fn fetch(&self, _request: &Request) -> Result<Response, Error> {
        todo!("phase 2: http")
    }
}
