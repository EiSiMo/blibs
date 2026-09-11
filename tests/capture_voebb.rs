//! Capture a voebb.de response as a fixture.
//!
//! Every test in this file is `#[ignore]`d and hits the live service. They exist because
//! voebb.de cannot be captured the way the KOBV side can: a search there is session-bound
//! — a cookie, a hidden `identity` that changes with every response, and a `requestCount`
//! that has to match — so the plain `curl` recipe that refreshes a KOBV fixture has no
//! voebb equivalent. Replaying that flow by hand is what a captured fixture is supposed to
//! save, so the flow itself does the capturing.
//!
//! Run one deliberately, never the whole file at once:
//!
//! ```text
//! cargo test --test capture_voebb capture_a_zero_hit_result_page -- --ignored --nocapture
//! ```
//!
//! Each run is a handful of requests against a real library service. Capture what is
//! missing, look at what came back, and commit it — do not loop.

use std::sync::Mutex;

use blibs::error::Error;
use blibs::http::{Fetch, Http, Request, Response};
use blibs::model::{QuerySpec, Term};

/// A transport that keeps a copy of every body that passes through it.
///
/// The point of capturing at this level is that it works even when the parser is what
/// fails: a page blibs cannot read yet is exactly the page worth having as a fixture.
struct Tap<'f> {
    inner: &'f dyn Fetch,
    bodies: Mutex<Vec<String>>,
}

impl<'f> Tap<'f> {
    fn around(inner: &'f dyn Fetch) -> Self {
        Self {
            inner,
            bodies: Mutex::new(Vec::new()),
        }
    }

    /// Every body seen so far, in order.
    fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Fetch for Tap<'_> {
    fn fetch(&self, request: &Request) -> Result<Response, Error> {
        let response = self.inner.fetch(request)?;
        self.bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(response.body.clone());
        Ok(response)
    }
}

/// Write a captured body under `tests/fixtures/` and say where it went.
fn save(relative: &str, body: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixture directory must be writable");
    }
    std::fs::write(&path, body).expect("the fixture must be writable");
    println!("wrote {} ({} bytes)", path.display(), body.len());
}

/// The result page for a search with **no hits anywhere in the network**.
///
/// This is the page `parse_results` could not read: it reported `selector "div#R06
/// p.info" matched nothing` and exit 6 for what is an ordinary empty result. Without the
/// page there is no positive marker to recognise it by, and this crate forbids the
/// alternative — a missing selector must never be turned into an empty result.
#[test]
#[ignore = "hits the live voebb.de service; run with --ignored"]
fn capture_a_zero_hit_result_page() {
    let http = Http::new(false);
    let tap = Tap::around(&http);
    let client = blibs::engine::voebb::client::VoebbClient::new(&tap);

    let mut session = client.open().expect("voebb.de opened a session");
    let query = QuerySpec {
        terms: vec![Term::from_argument("Xqzzyplkwrmf")],
        ..QuerySpec::default()
    };
    let outcome = client.search(&mut session, &query);

    let bodies = tap.bodies();
    let last = bodies
        .last()
        .expect("the session made at least one request")
        .clone();
    save("voebb/results_empty.html", &last);

    println!("requests: {}", bodies.len());
    match outcome {
        Ok(_) => println!("the parser already reads this page"),
        Err(error) => println!("the parser still fails, as expected: {error}"),
    }
}

/// The same empty answer, but through the **advanced** form — the path a field flag
/// takes. The reproducible crash was first reported against this one
/// (`--title "Prometheus LernAtlas" Skelett --at AGB`), and it stayed broken after the
/// simple search's empty page was understood, so aDISWeb evidently phrases or places the
/// notice differently here.
#[test]
#[ignore = "hits the live voebb.de service; run with --ignored"]
fn capture_a_zero_hit_advanced_result_page() {
    let http = Http::new(false);
    let tap = Tap::around(&http);
    let client = blibs::engine::voebb::client::VoebbClient::new(&tap);

    let mut session = client.open().expect("voebb.de opened a session");
    let query = QuerySpec {
        terms: vec![Term::from_argument("Skelett")],
        title: Some(Term::from_argument("Prometheus LernAtlas")),
        ..QuerySpec::default()
    };
    let outcome = client.search(&mut session, &query);

    let bodies = tap.bodies();
    let last = bodies
        .last()
        .expect("the session made at least one request")
        .clone();
    save("voebb/results_empty_advanced.html", &last);

    println!("requests: {}", bodies.len());
    match outcome {
        Ok(_) => println!("the parser already reads this page"),
        Err(error) => println!("the parser still fails, as expected: {error}"),
    }
}
