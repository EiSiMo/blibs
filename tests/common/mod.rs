//! Shared test helpers.
//!
//! The seam between this tool and the network is `blibs::http::Fetch`, and nothing else.
//! There is no environment switch and no test back door in the binary — a stub is simply
//! another implementation of that trait, which is why these helpers can live here instead
//! of in `src/`.
//!
//! [`FixtureFetch`] answers requests out of files under `tests/fixtures/`, and the
//! [`Recorder`] it carries records what was asked and, crucially, **how many requests
//! were in flight at once** — the per-host cap is a promise to the upstream services and
//! has to be provable, not assumed.

#![allow(
    dead_code,
    reason = "one shared helper module, several test binaries; each uses a subset"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blibs::error::Error;
use blibs::http::{Fetch, Request, Response};

/// The fixture root.
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Read a fixture by its path relative to `tests/fixtures/`.
///
/// Panics with the full path when it is missing: a silently empty fixture would make a
/// parser test pass for the wrong reason.
pub fn read_fixture(relative: &str) -> String {
    let path = fixture_dir().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read fixture {}: {err}", path.display()))
}

/// A request rendered as one comparable line: `GET https://host/path?a=1&b=2`.
///
/// Query parameters are kept in the order the client built them, so a test can assert on
/// the exact query that would go out — including the PQF string.
pub fn describe(request: &Request) -> String {
    let mut line = format!("{} {}", request.method.as_str(), request.base);
    if !request.query.is_empty() {
        line.push('?');
        let pairs: Vec<String> = request
            .query
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        line.push_str(&pairs.join("&"));
    }
    line
}

#[derive(Debug, Default)]
struct RecorderInner {
    total: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    log: Mutex<Vec<String>>,
}

/// Counts requests and tracks peak concurrency. Cheap to clone; every clone observes the
/// same counters.
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    inner: Arc<RecorderInner>,
}

impl Recorder {
    /// A fresh recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a request and hold a slot until the returned guard is dropped.
    pub fn enter(&self, request: &Request) -> InFlight {
        self.inner.total.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut log) = self.inner.log.lock() {
            log.push(describe(request));
        }
        let now = self.inner.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.max_in_flight.fetch_max(now, Ordering::SeqCst);
        InFlight {
            inner: Arc::clone(&self.inner),
        }
    }

    /// How many requests were made in total.
    pub fn total(&self) -> usize {
        self.inner.total.load(Ordering::SeqCst)
    }

    /// The highest number of requests that were ever in flight at the same time.
    pub fn max_in_flight(&self) -> usize {
        self.inner.max_in_flight.load(Ordering::SeqCst)
    }

    /// Every request, in the order they started, as produced by [`describe`].
    pub fn log(&self) -> Vec<String> {
        self.inner
            .log
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default()
    }

    /// How many requests match a substring — the usual shape of a request-count
    /// assertion ("one search plus two counting requests").
    pub fn count_matching(&self, needle: &str) -> usize {
        self.log()
            .iter()
            .filter(|line| line.contains(needle))
            .count()
    }
}

/// A held request slot. Releases on drop.
pub struct InFlight {
    inner: Arc<RecorderInner>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.inner.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

type Matcher = Box<dyn Fn(&Request) -> bool + Send + Sync>;

struct Route {
    matcher: Matcher,
    response: Response,
}

/// A [`Fetch`] that answers out of fixture files.
///
/// Routes are tried in the order they were added, first match wins. An unrouted request
/// panics naming the request — in a test, "no route" is a mistake in the test, and
/// answering it with an empty body would make the assertion pass for the wrong reason.
#[derive(Default)]
pub struct FixtureFetch {
    routes: Vec<Route>,
    recorder: Recorder,
    delay: Option<Duration>,
}

impl FixtureFetch {
    /// An empty stub.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorder. Clone it before handing the stub to the code under test.
    pub fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    /// Hold every request open for this long, so that concurrency is actually observable
    /// — without it, requests may complete faster than they are started and
    /// [`Recorder::max_in_flight`] would read 1 however parallel the caller is.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Answer requests matching `matcher` with `body`.
    #[must_use]
    pub fn route<F>(mut self, matcher: F, body: impl Into<String>) -> Self
    where
        F: Fn(&Request) -> bool + Send + Sync + 'static,
    {
        self.routes.push(Route {
            matcher: Box::new(matcher),
            response: ok(body),
        });
        self
    }

    /// Answer requests whose query contains `key=value` with the given fixture.
    #[must_use]
    pub fn on_param(self, key: &'static str, value: &'static str, fixture: &str) -> Self {
        let body = read_fixture(fixture);
        self.route(
            move |request| {
                request
                    .query
                    .iter()
                    .any(|(k, v)| k == key && v.contains(value))
            },
            body,
        )
    }

    /// Answer requests whose base contains `needle` with the given fixture.
    #[must_use]
    pub fn on_base(self, needle: &'static str, fixture: &str) -> Self {
        let body = read_fixture(fixture);
        self.route(move |request| request.base.contains(needle), body)
    }

    /// Answer everything not matched so far with the given fixture.
    #[must_use]
    pub fn fallback(self, fixture: &str) -> Self {
        let body = read_fixture(fixture);
        self.route(|_| true, body)
    }

    /// Answer requests matching `matcher` with an explicit response — for status codes
    /// and bodies that are not a fixture, such as a 503 or a truncated body.
    #[must_use]
    pub fn respond<F>(mut self, matcher: F, response: Response) -> Self
    where
        F: Fn(&Request) -> bool + Send + Sync + 'static,
    {
        self.routes.push(Route {
            matcher: Box::new(matcher),
            response,
        });
        self
    }
}

impl Fetch for FixtureFetch {
    fn fetch(&self, request: &Request) -> Result<Response, Error> {
        let _slot = self.recorder.enter(request);
        if let Some(delay) = self.delay {
            std::thread::sleep(delay);
        }
        match self.routes.iter().find(|route| (route.matcher)(request)) {
            Some(route) => Ok(route.response.clone()),
            None => panic!("no fixture route for {}", describe(request)),
        }
    }
}

/// A 200 response with this body.
pub fn ok(body: impl Into<String>) -> Response {
    Response {
        status: 200,
        body: body.into(),
        content_type: None,
        from_cache: false,
    }
}

/// A response with an explicit status — for the 429/503 backoff tests, and for the
/// "status 200 with an HTML error page" case that this crate exists to survive.
pub fn with_status(status: u16, body: impl Into<String>) -> Response {
    Response {
        status,
        body: body.into(),
        content_type: None,
        from_cache: false,
    }
}
