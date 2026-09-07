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

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::{Error, NetworkError, ServiceError, UnexpectedError};

pub use pool::scope_map;

/// Take a lock, recovering from poisoning.
///
/// Every mutex in this module tree guards either a counter table or a list of results
/// that are only read once the threads have joined. A poisoned lock therefore means a
/// worker panicked, not that the data is inconsistent — and that panic is re-raised when
/// the scope joins. Refusing the lock would turn a useful panic into a deadlock, or a
/// released slot into a leaked one.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

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
    /// controls anyway. The port is part of it: two ports are two servers as far as the
    /// cap is concerned.
    pub fn host(&self) -> &str {
        let after_scheme = self.after_scheme();
        let (authority, _) = after_scheme.split_at(end_of_authority(after_scheme));
        // Userinfo is not used anywhere in this crate, but stripping it keeps a stray
        // credential out of an error message.
        match authority.rsplit_once('@') {
            Some((_, host)) => host,
            None => authority,
        }
    }

    /// Everything after the host and before the query, e.g. `/k2`. Empty when the base
    /// names only a host.
    pub fn path(&self) -> &str {
        let after_scheme = self.after_scheme();
        let (_, path) = after_scheme.split_at(end_of_authority(after_scheme));
        path
    }

    /// The base without its scheme, which is where both the authority and the path are.
    ///
    /// A base that carries no `://` is returned whole: this crate builds every base
    /// itself, and an odd host in an error message beats a panic on a string that was
    /// never going to be a URL.
    fn after_scheme(&self) -> &str {
        match self.base.split_once("://") {
            Some((_, rest)) => rest,
            None => self.base.as_str(),
        }
    }

    /// The full URL, query included and percent-encoded.
    ///
    /// Parameters keep the order the client built them in: the cache key sorts them, the
    /// wire does not, so a recorded request reads the way the engine wrote it.
    pub fn url(&self) -> String {
        if self.query.is_empty() {
            return self.base.clone();
        }
        let mut url = self.base.clone();
        url.push('?');
        for (index, (key, value)) in self.query.iter().enumerate() {
            if index > 0 {
                url.push('&');
            }
            encode_query_component(key, &mut url);
            url.push('=');
            encode_query_component(value, &mut url);
        }
        url
    }
}

/// Where the authority ends in a string that has had its scheme removed: at the first
/// `/`, `?` or `#`, or at the end.
fn end_of_authority(after_scheme: &str) -> usize {
    after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len())
}

/// Percent-encode one query key or value into `out`.
///
/// Everything outside RFC 3986's *unreserved* set is encoded, which is stricter than it
/// has to be and never wrong. It matters: a PQF query carries spaces, `@`, `=` and
/// quotation marks, and leaving any of those raw would either be rejected or, worse,
/// silently change what was searched for.
fn encode_query_component(value: &str, out: &mut String) {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                out.push('%');
                out.push(HEX[usize::from(byte >> 4)]);
                out.push(HEX[usize::from(byte & 0x0f)]);
            }
        }
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
pub struct Http {
    agent: ureq::Agent,
    cache: Option<cache::Cache>,
    limits: limit::HostLimits,
}

impl Http {
    /// Build the client. `use_cache` is `false` under `--no-cache`.
    ///
    /// Cookies are enabled on the agent because voebb.de is session-bound; the KOBV side
    /// ignores them. Redirects are followed, which the voebb.de session flow requires:
    /// every form `POST` there answers 303 and the answer is behind the `Location`.
    /// `ureq` turns a `POST` into a `GET` on a 303 the way every other client does, and
    /// re-attaches the cookie jar to the redirected request.
    ///
    /// HTTP statuses are deliberately *not* raised as `ureq` errors: which statuses are
    /// acceptable is a per-request decision ([`Request::accept_status`]) and belongs to
    /// this module, not to the transport.
    ///
    /// A cache directory that cannot be created is not an error — the client then runs
    /// without one.
    pub fn new(use_cache: bool) -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            // One budget for the whole call, redirects and body included. A per-call
            // timeout would let a chain of slow redirects run for a multiple of it.
            .timeout_global(Some(retry::TIMEOUT))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.new_agent(),
            cache: if use_cache {
                cache::Cache::open()
            } else {
                None
            },
            limits: limit::HostLimits::new(),
        }
    }

    /// The cache key for this request, or `None` when it must not touch the cache.
    fn cache_key(&self, request: &Request) -> Option<String> {
        if self.cache.is_some() && cache::is_cacheable(request) {
            Some(cache::cache_key(request))
        } else {
            None
        }
    }

    /// A cached body, served as a `200`. The status is not stored, because only a
    /// successful response is ever written.
    fn cached(&self, key: &str) -> Option<Response> {
        let body = self.cache.as_ref()?.get(key, cache::ENTRY_TTL)?;
        Some(Response {
            status: 200,
            body,
            content_type: None,
            from_cache: true,
        })
    }

    /// Attempt the request up to [`retry::MAX_ATTEMPTS`] times.
    ///
    /// Retried: 429, 503 and transport failures. Not retried: everything else, including
    /// every 4xx — the request is broken and will be just as broken in a second.
    /// Exhausting the attempts on a throttling status is [`ServiceError::Throttled`],
    /// which is its own exit code so that an agent can back off instead of hammering.
    fn send_with_retry(&self, request: &Request) -> Result<Response, Error> {
        let url = request.url();
        let mut attempt = 1;
        loop {
            let delay = retry::delay_for(attempt);
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            let failure = match self.send_once(request, &url) {
                Ok(response) if is_accepted(request, response.status) => return Ok(response),
                Ok(response) if retry::is_retryable(response.status) => {
                    Failure::Throttled(response.status)
                }
                Ok(response) => {
                    return Err(UnexpectedError::HttpStatus {
                        host: request.host().to_owned(),
                        status: response.status,
                    }
                    .into());
                }
                Err(error) => Failure::Transport(error),
            };
            if attempt >= retry::MAX_ATTEMPTS {
                return Err(failure.into_error(request.host(), attempt));
            }
            attempt += 1;
        }
    }

    /// One attempt. Everything that is not an answer comes back as the transport's own
    /// error, which [`Failure::into_error`] turns into a [`NetworkError`].
    fn send_once(&self, request: &Request, url: &str) -> Result<Response, ureq::Error> {
        let mut response = match request.method {
            Method::Get => self.agent.get(url).call()?,
            Method::Post => self.agent.post(url).send_form(
                request
                    .form
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )?,
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        // The body is read even on a status this crate will reject: an SRU diagnostic and
        // an HTML error page both arrive with a body, and a discarded one cannot be
        // explained to the user.
        let body = response.body_mut().read_to_string()?;
        Ok(Response {
            status,
            body,
            content_type,
            from_cache: false,
        })
    }
}

/// Whether a status counts as an answer for this request: any 2xx, plus whatever the
/// request declared acceptable.
fn is_accepted(request: &Request, status: u16) -> bool {
    (200..300).contains(&status) || request.accept_status.contains(&status)
}

/// A failed attempt, kept until it is known whether another one follows.
enum Failure {
    /// The service asked to be left alone: 429 or 503.
    Throttled(u16),
    /// The request never produced an answer.
    Transport(ureq::Error),
}

impl Failure {
    /// The error this failure becomes once the attempts are used up.
    fn into_error(self, host: &str, attempts: u32) -> Error {
        match self {
            Failure::Throttled(status) => ServiceError::Throttled {
                host: host.to_owned(),
                status,
                attempts,
            }
            .into(),
            // A timeout gets its own variant: "did not answer in 10 s" and "could not be
            // reached" call for different next steps.
            Failure::Transport(ureq::Error::Timeout(_)) => NetworkError::Timeout {
                host: host.to_owned(),
                seconds: retry::TIMEOUT.as_secs(),
            }
            .into(),
            Failure::Transport(error) => NetworkError::Transport {
                host: host.to_owned(),
                source: Box::new(error),
            }
            .into(),
        }
    }
}

impl Fetch for Http {
    /// Cache read → per-host cap → retry → cache write.
    ///
    /// The cap is held for the network part only; a cache hit never waits for a slot. The
    /// cache is written only after a response this module accepted, so a throttling page
    /// or an error body can never be served back as if it were a catalogue answer.
    fn fetch(&self, request: &Request) -> Result<Response, Error> {
        let key = self.cache_key(request);
        if let Some(hit) = key.as_deref().and_then(|key| self.cached(key)) {
            return Ok(hit);
        }

        let response = {
            let _slot = self.limits.acquire(request.host());
            self.send_with_retry(request)?
        };

        if let (Some(key), Some(cache)) = (key.as_deref(), self.cache.as_ref()) {
            cache.put(key, &response.body);
            cache.sweep_if_due();
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_names_the_tool_and_its_repository() {
        assert!(USER_AGENT.starts_with("blibs/"));
        assert!(USER_AGENT.contains("github.com/EiSiMo/blibs"));
    }

    #[test]
    fn the_host_is_the_authority_without_the_scheme() {
        assert_eq!(Request::get("https://sru.kobv.de/k2").host(), "sru.kobv.de");
        assert_eq!(
            Request::get("https://www.voebb.de/aDISWeb/app").host(),
            "www.voebb.de"
        );
        assert_eq!(Request::get("https://host.test").host(), "host.test");
    }

    #[test]
    fn the_port_is_part_of_the_host() {
        assert_eq!(
            Request::get("http://host.test:8080/x").host(),
            "host.test:8080"
        );
    }

    #[test]
    fn a_base_that_is_not_a_url_does_not_panic() {
        assert_eq!(Request::get("nonsense").host(), "nonsense");
        assert_eq!(Request::get("").host(), "");
        assert_eq!(Request::get("").path(), "");
    }

    #[test]
    fn the_path_stops_at_the_authority() {
        assert_eq!(Request::get("https://sru.kobv.de/k2").path(), "/k2");
        assert_eq!(Request::get("https://sru.kobv.de").path(), "");
        assert_eq!(
            Request::get("https://www.voebb.de/aDISWeb/app?service=x").path(),
            "/aDISWeb/app?service=x"
        );
    }

    #[test]
    fn a_query_less_request_keeps_its_base() {
        assert_eq!(
            Request::get("https://sru.kobv.de/k2").url(),
            "https://sru.kobv.de/k2"
        );
    }

    /// A PQF query is the reason the encoder is strict: it carries spaces, `@`, `=` and
    /// quotation marks, and any of them left raw either fails or silently searches for
    /// something else.
    #[test]
    fn a_pqf_query_survives_encoding() {
        let request = Request::get("https://sru.kobv.de/k2")
            .query("operation", "searchRetrieve")
            .query("x-pquery", "@attr 1=4 \"Der Prozess\"");
        assert_eq!(
            request.url(),
            "https://sru.kobv.de/k2?operation=searchRetrieve\
             &x-pquery=%40attr%201%3D4%20%22Der%20Prozess%22"
        );
    }

    #[test]
    fn unreserved_characters_are_left_alone() {
        let request = Request::get("https://host.test/p").query("k", "aZ09-._~");
        assert_eq!(request.url(), "https://host.test/p?k=aZ09-._~");
    }

    #[test]
    fn separators_in_a_value_cannot_forge_a_parameter() {
        let request = Request::get("https://host.test/p").query("k", "a&b=c");
        assert_eq!(request.url(), "https://host.test/p?k=a%26b%3Dc");
    }

    #[test]
    fn non_ascii_is_encoded_as_utf8_bytes() {
        let request = Request::get("https://host.test/p").query("q", "Müller");
        assert_eq!(request.url(), "https://host.test/p?q=M%C3%BCller");
    }

    #[test]
    fn query_order_is_preserved_on_the_wire() {
        let request = Request::get("https://host.test/p")
            .query("b", "2")
            .query("a", "1");
        assert_eq!(request.url(), "https://host.test/p?b=2&a=1");
    }

    #[test]
    fn only_declared_statuses_are_accepted() {
        let plain = Request::get("https://host.test/p");
        assert!(is_accepted(&plain, 200));
        assert!(is_accepted(&plain, 204));
        assert!(!is_accepted(&plain, 302));
        assert!(!is_accepted(&plain, 404));

        let lenient = Request::get("https://host.test/p").accept_status(&[404]);
        assert!(is_accepted(&lenient, 404));
        assert!(!is_accepted(&lenient, 500));
    }

    #[test]
    fn throttling_becomes_its_own_error_category() {
        let error = Failure::Throttled(503).into_error("sru.kobv.de", 3);
        assert!(matches!(
            error,
            Error::Service(ServiceError::Throttled { status: 503, .. })
        ));
        assert!(error.to_string().contains("sru.kobv.de"));
    }

    /// The one test that touches the network, so it is opt-in: `cargo test http:: --
    /// --ignored`. It proves what no fixture can — that the real agent configuration
    /// (user agent, timeout, redirects, statuses not raised as errors) reaches the live
    /// SRU endpoint and gets a document back. `maximumRecords=0` asks for the count only,
    /// which is the smallest request this crate ever makes.
    #[test]
    #[ignore = "hits the live KOBV service; run with --ignored"]
    fn the_live_endpoint_answers() {
        let http = Http::new(false);
        let request = Request::get("https://sru.kobv.de/k2")
            .query("operation", "searchRetrieve")
            .query("version", "2.0")
            .query("maximumRecords", "0")
            .query("x-pquery", "@attr 1=4 \"Harry Potter\"");
        let response = http.fetch(&request).expect("the live service answered");
        assert_eq!(response.status, 200);
        assert!(!response.from_cache);
        assert!(
            response.body.contains("numberOfRecords"),
            "got {} bytes that are not an SRU envelope: {}",
            response.body.len(),
            response.body.chars().take(200).collect::<String>()
        );
    }

    /// A closure is a `Fetch`. This is the whole test seam: no environment switch, no
    /// test back door in the binary.
    #[test]
    fn a_closure_is_a_fetch() {
        fn ask(fetch: &dyn Fetch) -> Result<Response, Error> {
            fetch.fetch(&Request::get("https://host.test/p"))
        }
        let stub = |request: &Request| {
            Ok(Response {
                status: 200,
                body: request.url(),
                content_type: None,
                from_cache: false,
            })
        };
        let response = ask(&stub).expect("the stub cannot fail");
        assert_eq!(response.body, "https://host.test/p");
    }
}
