//! Transport. **This module interprets nothing.**
//!
//! It hands back bytes and a status; whether those bytes are plausible is decided in a
//! `parse` module, never here. That split is what makes the whole crate testable: the
//! seam between the tool and the network is the [`Fetch`] trait, and tests substitute a
//! fixture-backed implementation for it. There is no environment switch and no test
//! back door in the binary.
//!
//! Upstream etiquette is enforced here and is not configurable:
//!
//! - at most [`limit::cap_for`] requests in flight per host — six by default, one for
//!   a host that measured worse under concurrency,
//! - at most [`limit::timeout_for`] per request — ten seconds by default, more for a host
//!   that was measured answering more slowly than that,
//! - an honest [`USER_AGENT`] naming the tool and its repository,
//! - an on-disk cache so a repeated agent query does not hit the service again,
//! - backoff on 429/503, surfaced as a distinct error rather than a generic failure,
//! - **never a second send of a request whose first send consumed state upstream**
//!   ([`Replay`]).

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

/// Whether sending this request a second time is a repetition or a different request.
///
/// The distinction is not about the HTTP method — a `GET` that carries a one-shot token
/// would be [`Replay::SingleUse`] too — but about whether **sending** it consumes state at
/// the far side. It exists because a transport failure says nothing about what the server
/// did: the request may never have arrived, or it may have been answered in full and the
/// answer lost on the way back. Retrying is the right guess only when both of those
/// possibilities lead to the same place.
///
/// This is the fix for the worst measured failure of the tool: a voebb.de form submission
/// that took 11 s against a 10 s deadline was retried, the replay spent the session's
/// `requestCount` a second time, the site answered `/noaccess`, and a search the server had
/// answered correctly was reported to the user as "the site has changed".
///
/// **429 and 503 are unaffected**, and deliberately so: there the server answered, and its
/// answer is that it did not do the work. Nothing was consumed, so the backoff in
/// [`retry`] still applies to every request regardless of this policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Replay {
    /// Sending it again asks the same question again. Every `GET` in this crate, and the
    /// default: a request has to *say* that it is single-use, because the failure mode of
    /// guessing wrong in this direction is one wasted request, and in the other direction
    /// it is a destroyed session reported as a broken site.
    #[default]
    Repeatable,
    /// Sending it consumes state at the far side, so a second send is not a repetition but
    /// a different — and, at voebb.de, guaranteed broken — request. After a transport
    /// failure it is given up on rather than replayed.
    SingleUse,
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
    /// Whether a transport failure may be answered with a second send.
    pub replay: Replay,
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
            replay: Replay::Repeatable,
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

    /// Set the replay policy.
    ///
    /// Only the client that builds the request knows whether sending it consumes anything
    /// upstream, so only that client can say so — this module cannot infer it, and a
    /// method is not evidence.
    #[must_use]
    pub fn replay(mut self, policy: Replay) -> Self {
        self.replay = policy;
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

    /// Drop this request's cached answer, if there is one.
    ///
    /// Called when a body that came from the cache turned out not to parse: the entry is
    /// intact on disk but useless, and it would otherwise be replayed for the rest of its
    /// TTL. Defaults to doing nothing, so an implementation without a cache — every test
    /// stub — needs no change.
    fn forget(&self, request: &Request) {
        let _ = request;
    }
}

/// Fetch a request and parse it, retrying once against the network if a **cached** body
/// is what failed to parse.
///
/// A cache may only ever change latency, never a result. A corrupted entry
/// breaks that rule twice over: it fails, and it keeps failing, because nothing evicts it.
/// So a parse failure on a cached body drops the entry ([`Fetch::forget`]) and asks again
/// — with the request **unchanged**, cache policy included. That matters: this arm is
/// only ever reached for a request whose cache was actually read (see below), which means
/// its policy was already [`CachePolicy::Normal`]; forcing `Never` on the retry, as an
/// earlier version of this function did, would suppress the write as well as the read
/// (`cache::is_cacheable`) and evict the entry with nothing to replace it — one corrupted
/// body would then cost a fresh upstream request forever, in every later run. Retrying
/// with the same, still-`Normal` request instead lets the retry miss the now-empty cache,
/// go to the network, and — if it parses — repopulate the entry, so the cost of the
/// corruption is exactly one extra request, not one per invocation from here on.
///
/// This cannot loop. `response.from_cache` is `true` only when the response was served
/// from a cache entry that existed at the time of that particular `fetch` call — and by
/// the time this arm's retry fires, [`Fetch::forget`] has just deleted the only entry that
/// could have produced it. The retry therefore cannot itself be served from that cache, so
/// its `Response::from_cache` comes back `false`, and the guard `if response.from_cache`
/// cannot match a second time: a second parse failure falls through to
/// `Err(error) => Err(error)` below and reaches the caller as the service's own answer, not
/// as an infinite retry.
///
/// A request built with [`CachePolicy::Never`] (availability, every voebb.de call) never
/// takes this branch in the first place: such a request is never cacheable
/// (`cache::is_cacheable`), so its response is never served `from_cache: true`, so the
/// guard above never matches for it. A parse failure on it always falls straight to
/// `Err(error) => Err(error)` on the first attempt — this function changes nothing about
/// that path, and never writes such a request to the cache either.
///
/// A body that came from the network on the first attempt is never retried here — a
/// service that answers with nonsense would otherwise be asked twice for it.
///
/// Free function rather than a [`Fetch`] method because it is generic over the parsed
/// type, and `Fetch` has to stay object-safe: `cli::run` passes `&dyn Fetch` around.
pub fn fetch_parsed<T>(
    fetch: &dyn Fetch,
    request: &Request,
    parse: impl Fn(&str) -> Result<T, Error>,
) -> Result<T, Error> {
    let response = fetch.fetch(request)?;
    match parse(&response.body) {
        Ok(parsed) => Ok(parsed),
        Err(stale) if response.from_cache => {
            let _ = stale;
            fetch.forget(request);
            let fresh = fetch.fetch(request)?;
            parse(&fresh.body)
        }
        Err(error) => Err(error),
    }
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
    /// **The agent carries no deadline of its own.** How long to wait depends on the host
    /// ([`limit::timeout_for`]) and one agent serves all three of them, so the budget is
    /// set on every request instead, in `Http::send_once` — the single place a request
    /// leaves this process. A second number here would be one that has to agree with that
    /// one, and two numbers that must agree are one number too many.
    ///
    /// A cache directory that cannot be created is not an error — the client then runs
    /// without one.
    pub fn new(use_cache: bool) -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
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

    /// Attempt the request up to [`retry::MAX_ATTEMPTS`] times, through [`with_retry`].
    fn send_with_retry(&self, request: &Request) -> Result<Response, Error> {
        let url = request.url();
        with_retry(request, |request| {
            self.send_once(request, &url).map_err(Failure::from)
        })
    }

    /// One attempt. Everything that is not an answer comes back as the transport's own
    /// error, which [`Failure::from`] classifies and [`Failure::into_error`] turns into a
    /// [`NetworkError`].
    ///
    /// The deadline is set here, per request, from [`limit::timeout_for`] — one budget for
    /// the whole call, redirects and body included, because a per-call timeout would let a
    /// chain of slow redirects run for a multiple of it. It is set on the request rather
    /// than on the agent because it is a property of the host, and this crate talks to
    /// three of them with one agent.
    fn send_once(&self, request: &Request, url: &str) -> Result<Response, ureq::Error> {
        let deadline = Some(limit::timeout_for(request.host()));
        let mut response = match request.method {
            Method::Get => self
                .agent
                .get(url)
                .config()
                .timeout_global(deadline)
                .build()
                .call()?,
            Method::Post => self
                .agent
                .post(url)
                .config()
                .timeout_global(deadline)
                .build()
                .send_form(
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

/// Attempt one request up to [`retry::MAX_ATTEMPTS`] times, `send` performing one attempt.
///
/// Retried: 429 and 503 for every request, and a transport failure for a request that may
/// be sent again ([`Replay`]). Not retried: everything else, including every 4xx — the
/// request is broken and will be just as broken in a second. Exhausting the attempts on a
/// throttling status is [`ServiceError::Throttled`], which is its own exit code so that an
/// agent can back off instead of hammering; giving up on a transport failure is a
/// [`NetworkError`], which is a different code again, because "the service is busy" and
/// "nothing came back" call for different next steps.
///
/// Free function over a closure rather than a method on [`Http`] so that the schedule can
/// be proven without a socket: the count of attempts a policy produces is exactly what a
/// regression here has to be caught by, and it is not observable through [`Fetch`], which
/// sits *above* this loop.
fn with_retry(
    request: &Request,
    send: impl Fn(&Request) -> Result<Response, Failure>,
) -> Result<Response, Error> {
    let mut attempt = 1;
    loop {
        let delay = retry::delay_for(attempt);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let failure = match send(request) {
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
            Err(failure) => failure,
        };
        if attempt >= retry::MAX_ATTEMPTS || !failure.may_repeat(request) {
            return Err(failure.into_error(request.host(), attempt));
        }
        attempt += 1;
    }
}

/// A failed attempt, kept until it is known whether another one follows.
enum Failure {
    /// The service asked to be left alone: 429 or 503.
    Throttled(u16),
    /// The deadline expired with the request still open.
    TimedOut,
    /// The request never produced an answer: DNS, TLS, a refused or dropped connection.
    Transport(Box<dyn std::error::Error + Send + Sync>),
}

/// Classify the transport's own error once, here, so that the retry decision and the
/// user-facing error read the same failure the same way.
impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Timeout(_) => Failure::TimedOut,
            other => Failure::Transport(Box::new(other)),
        }
    }
}

impl Failure {
    /// Whether the request that produced this failure may be sent a second time.
    ///
    /// The two arms are different situations, not degrees of the same one:
    ///
    /// - **429/503 is an answer.** The service replied, and its reply is that it did not do
    ///   the work. Nothing upstream was consumed, so backing off and asking again is
    ///   correct even for a [`Replay::SingleUse`] request — that is why the throttling path
    ///   is deliberately left untouched by the replay policy.
    /// - **A transport failure is silence.** Nothing here knows whether the request
    ///   arrived, whether it was executed, or whether only the answer was lost. Sending it
    ///   again is a guess, and for a request that consumes state upstream it is a guess
    ///   that is *wrong by construction*: the second send cannot be the same request.
    fn may_repeat(&self, request: &Request) -> bool {
        match self {
            Failure::Throttled(_) => true,
            Failure::TimedOut | Failure::Transport(_) => request.replay == Replay::Repeatable,
        }
    }

    /// The error this failure becomes once no further attempt will be made.
    fn into_error(self, host: &str, attempts: u32) -> Error {
        match self {
            Failure::Throttled(status) => ServiceError::Throttled {
                host: host.to_owned(),
                status,
                attempts,
            }
            .into(),
            // A timeout gets its own variant: "did not answer in 30 s" and "could not be
            // reached" call for different next steps. The number is the host's own
            // deadline, so the message states the budget that actually expired.
            Failure::TimedOut => NetworkError::Timeout {
                host: host.to_owned(),
                seconds: limit::timeout_for(host).as_secs(),
            }
            .into(),
            Failure::Transport(source) => NetworkError::Transport {
                host: host.to_owned(),
                source,
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

    /// Drop the entry this request would be served from. A request that never touches the
    /// cache has no key and nothing to drop.
    fn forget(&self, request: &Request) {
        if let (Some(key), Some(cache)) = (self.cache_key(request), self.cache.as_ref()) {
            cache.forget(&key);
        }
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

    /// A `200 OK` for whatever the loop is handed, so that only the failures under test
    /// decide how often `send` is called.
    fn answered() -> Response {
        Response {
            status: 200,
            body: "ok".to_owned(),
            content_type: None,
            from_cache: false,
        }
    }

    /// A voebb.de form submission, as `engine/voebb/client` builds it.
    fn single_use_post() -> Request {
        Request::post("https://www.voebb.de/aDISWeb/app").replay(Replay::SingleUse)
    }

    /// Run the retry loop over a scripted sequence of attempt outcomes and report how many
    /// of them were actually consumed.
    fn attempts(request: &Request, mut script: Vec<Result<Response, Failure>>) -> usize {
        script.reverse();
        let script = Mutex::new(script);
        let used = Mutex::new(0);
        let _ = with_retry(request, |_| {
            *lock(&used) += 1;
            lock(&script).pop().unwrap_or_else(|| Ok(answered()))
        });
        *lock(&used)
    }

    /// The worst measured failure of the tool, in one assertion: a voebb.de form
    /// submission that times out is **not** sent again. The replay would spend the
    /// session's `requestCount` a second time, and voebb.de answers that with `/noaccess`
    /// — so the retry could never have helped and was guaranteed to destroy the session it
    /// was trying to rescue.
    #[test]
    fn a_single_use_request_is_never_sent_again_after_a_transport_failure() {
        assert_eq!(
            attempts(&single_use_post(), vec![Err(Failure::TimedOut)]),
            1,
            "a single-use request was sent a second time"
        );
        assert_eq!(
            attempts(
                &single_use_post(),
                vec![Err(Failure::Transport(Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "connection reset",
                ))))],
            ),
            1,
            "any silence, not just a timeout, ends a single-use request"
        );
    }

    /// The other half of the rule: a request that consumes nothing upstream is still
    /// retried, so the fix costs no resilience where resilience was correct.
    #[test]
    fn a_repeatable_request_is_still_sent_again_after_a_transport_failure() {
        assert_eq!(
            attempts(
                &Request::get("https://sru.kobv.de/k2"),
                vec![Err(Failure::TimedOut), Ok(answered())],
            ),
            2
        );
    }

    /// 429 and 503 are untouched by the replay policy, and that is deliberate: there the
    /// service **answered**, and its answer is that it did not do the work. Nothing was
    /// consumed, so backing off and asking again is right even for a single-use request.
    #[test]
    fn throttling_is_retried_for_a_single_use_request_too() {
        for status in [429, 503] {
            let throttled = Response {
                status,
                ..answered()
            };
            assert_eq!(
                attempts(&single_use_post(), vec![Ok(throttled), Ok(answered())],),
                2,
                "HTTP {status} must still be retried"
            );
        }
    }

    /// A status that is neither accepted nor throttling is never retried, whatever the
    /// replay policy says — the request is broken and will be just as broken in a second.
    #[test]
    fn an_unexpected_status_is_not_retried() {
        let refused = Response {
            status: 404,
            ..answered()
        };
        assert_eq!(
            attempts(&Request::get("https://sru.kobv.de/k2"), vec![Ok(refused)]),
            1
        );
    }

    /// What a voebb.de timeout **is**, now that it is no longer converted into a lost
    /// session: exit 3, `timeout`, and not a document failure — a slow host must never
    /// again be reported as a site that changed shape.
    #[test]
    fn a_single_use_timeout_is_a_network_error_and_never_a_document_error() {
        let error = with_retry(&single_use_post(), |_| Err(Failure::TimedOut))
            .expect_err("the attempt never answers");
        assert_eq!(error.kind(), "timeout");
        assert_eq!(error.exit(), crate::error::ExitCode::Network);
        assert!(
            !error.is_unreadable_document(),
            "a timeout is silence, not a document that arrived and changed shape"
        );
        assert!(error.to_string().contains("www.voebb.de"));
    }

    /// The message states the budget that actually expired, which is the host's own — not
    /// one number standing in for three services.
    #[test]
    fn a_timeout_names_the_deadline_of_the_host_that_missed_it() {
        let voebb = Failure::TimedOut.into_error("www.voebb.de", 1);
        let kobv = Failure::TimedOut.into_error("sru.kobv.de", 1);
        assert!(
            voebb
                .to_string()
                .contains(&limit::timeout_for("www.voebb.de").as_secs().to_string()),
            "{voebb}"
        );
        assert!(
            kobv.to_string()
                .contains(&limit::DEFAULT_TIMEOUT.as_secs().to_string()),
            "{kobv}"
        );
        assert_ne!(voebb.to_string(), kobv.to_string());
    }

    /// A request has to *say* it is single-use. Getting the default wrong in this
    /// direction costs one wasted request; getting it wrong in the other costs a session
    /// and reports it as a broken site.
    #[test]
    fn a_request_is_repeatable_unless_it_says_otherwise() {
        assert_eq!(
            Request::get("https://host.test/p").replay,
            Replay::Repeatable
        );
        assert_eq!(
            Request::post("https://host.test/p").replay,
            Replay::Repeatable
        );
        assert_eq!(single_use_post().replay, Replay::SingleUse);
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

    /// A transport whose cache holds one unusable body until it is told to forget it.
    struct Poisoned {
        fetches: Mutex<u32>,
        forgotten: Mutex<bool>,
    }

    impl Poisoned {
        fn new() -> Self {
            Self {
                fetches: Mutex::new(0),
                forgotten: Mutex::new(false),
            }
        }
    }

    impl Fetch for Poisoned {
        fn fetch(&self, _request: &Request) -> Result<Response, Error> {
            *lock(&self.fetches) += 1;
            let served_from_cache = !*lock(&self.forgotten);
            Ok(Response {
                status: 200,
                body: if served_from_cache {
                    "garbage"
                } else {
                    "<sru/>"
                }
                .to_owned(),
                content_type: None,
                from_cache: served_from_cache,
            })
        }

        fn forget(&self, _request: &Request) {
            *lock(&self.forgotten) = true;
        }
    }

    /// Only `<sru/>` parses; anything else is the failure a corrupt entry produces.
    fn parse_marker(body: &str) -> Result<String, Error> {
        if body == "<sru/>" {
            return Ok(body.to_owned());
        }
        Err(UnexpectedError::NotXml {
            context: "test".to_owned(),
            snippet: body.to_owned(),
        }
        .into())
    }

    /// The whole point of [`fetch_parsed`]: a cached body that does not parse is dropped
    /// and asked for again, and the caller never sees the failure.
    #[test]
    fn a_cached_body_that_does_not_parse_is_forgotten_and_fetched_again() {
        let transport = Poisoned::new();
        let parsed = fetch_parsed(
            &transport,
            &Request::get("https://sru.kobv.de/k2"),
            parse_marker,
        )
        .expect("the second attempt answers a parsable body");
        assert_eq!(parsed, "<sru/>");
        assert!(*lock(&transport.forgotten), "the entry has to be dropped");
        assert_eq!(*lock(&transport.fetches), 2, "exactly one retry");
    }

    /// A body from the network is never retried — a service answering nonsense would
    /// otherwise be asked for it twice.
    #[test]
    fn a_network_body_that_does_not_parse_is_not_retried() {
        let transport = |_: &Request| {
            Ok(Response {
                status: 200,
                body: "garbage".to_owned(),
                content_type: None,
                from_cache: false,
            })
        };
        let attempts = std::cell::Cell::new(0);
        let error = fetch_parsed(
            &transport,
            &Request::get("https://sru.kobv.de/k2"),
            |body| {
                attempts.set(attempts.get() + 1);
                parse_marker(body)
            },
        )
        .expect_err("the body never parses");
        assert_eq!(attempts.get(), 1, "no second parse, so no second request");
        assert_eq!(error.kind(), "not_xml");
    }

    /// A `Fetch` built on the *real* cache module (`cache::is_cacheable`, `Cache::get`,
    /// `Cache::put`, `Cache::forget`), so these tests exercise the actual read → network →
    /// write cycle `Http::fetch` implements, not a hand-rolled approximation of it.
    struct RealisticUpstream {
        cache: cache::Cache,
        /// What the network answers with, every time it is actually asked.
        network_body: &'static str,
        network_calls: Mutex<u32>,
    }

    impl RealisticUpstream {
        fn new(cache: cache::Cache, network_body: &'static str) -> Self {
            Self {
                cache,
                network_body,
                network_calls: Mutex::new(0),
            }
        }
    }

    impl Fetch for RealisticUpstream {
        fn fetch(&self, request: &Request) -> Result<Response, Error> {
            let cacheable = cache::is_cacheable(request);
            let key = cache::cache_key(request);
            if cacheable && let Some(body) = self.cache.get(&key, cache::ENTRY_TTL) {
                return Ok(Response {
                    status: 200,
                    body,
                    content_type: None,
                    from_cache: true,
                });
            }
            *lock(&self.network_calls) += 1;
            if cacheable {
                self.cache.put(&key, self.network_body);
            }
            Ok(Response {
                status: 200,
                body: self.network_body.to_owned(),
                content_type: None,
                from_cache: false,
            })
        }

        fn forget(&self, request: &Request) {
            self.cache.forget(&cache::cache_key(request));
        }
    }

    /// A discarded cache entry has to be replaced in the *same* run, not just made to
    /// fail more gracefully. This proves the whole cycle: a
    /// poisoned entry costs exactly one extra upstream call, and the entry it leaves
    /// behind is the fresh body — a second `fetch_parsed` for the same request reads it
    /// straight from the cache and never touches the network again.
    #[test]
    fn a_forgotten_entry_is_replaced_so_the_next_call_never_hits_the_network() {
        let dir = tempfile::tempdir().expect("the test needs a writable temp directory");
        let cache = cache::Cache::at(dir.path().to_path_buf());
        let request = Request::get("https://sru.kobv.de/k2");
        // The corruption: a body already on disk that will never parse.
        cache.put(&cache::cache_key(&request), "garbage");

        let upstream = RealisticUpstream::new(cache, "<sru/>");

        let first = fetch_parsed(&upstream, &request, parse_marker)
            .expect("the retry answers a parsable body");
        assert_eq!(first, "<sru/>");
        assert_eq!(
            *lock(&upstream.network_calls),
            1,
            "the poisoned entry costs exactly one upstream call"
        );

        let second =
            fetch_parsed(&upstream, &request, parse_marker).expect("the repopulated entry parses");
        assert_eq!(second, "<sru/>");
        assert_eq!(
            *lock(&upstream.network_calls),
            1,
            "the second call must be served from the entry the retry just wrote, \
             not from a second upstream request"
        );
    }

    /// The guarantee this fix must not weaken: an availability-style request
    /// (`CachePolicy::Never`) is still never written to the cache, even when its body
    /// fails to parse — the retry path in `fetch_parsed` only ever concerns a request that
    /// was cacheable to begin with (see the doc comment on `fetch_parsed`).
    #[test]
    fn a_never_cached_request_is_still_never_written_even_on_a_parse_failure() {
        let dir = tempfile::tempdir().expect("the test needs a writable temp directory");
        let cache = cache::Cache::at(dir.path().to_path_buf());
        let request = Request::get("https://sru.kobv.de/k2").cache(CachePolicy::Never);
        let key = cache::cache_key(&request);

        let upstream = RealisticUpstream::new(cache, "garbage");

        let error =
            fetch_parsed(&upstream, &request, parse_marker).expect_err("the body never parses");
        assert_eq!(error.kind(), "not_xml");
        assert_eq!(
            *lock(&upstream.network_calls),
            1,
            "no retry: a Never response is never from_cache, so the guard cannot match"
        );
        assert_eq!(
            upstream.cache.get(&key, cache::ENTRY_TTL),
            None,
            "a Never request must never leave an entry behind"
        );
    }
}
