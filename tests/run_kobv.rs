//! End-to-end tests of one whole invocation: command line in, rendered output and an
//! outcome out, with `tests/fixtures/` standing in for the network.
//!
//! These go through `blibs::cli::run`, so they cover exactly what `main` does apart from
//! locking the streams — including the order that `plan/cli.md` fixes and that nothing
//! else can prove: **search, then select, then availability**, never availability for
//! records that will not be displayed.

mod common;

use blibs::cli::{Cli, run};
use blibs::error::{Error, ExitCode, NetworkError, Outcome};
use blibs::http::{Fetch, Request, Response};
use blibs::render::Style;
use clap::Parser;

use common::{FixtureFetch, Recorder, read_fixture};

/// A terminal wide enough that nothing is truncated, and never coloured: the layout is
/// identical either way, and a colour code in an assertion proves nothing.
const WIDE: usize = 120;

/// Parse a command line the way the binary does. A test that hand-builds `Cli` would
/// stop covering the flags themselves.
fn cli(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("blibs").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("the test invocation must parse: {error}"))
}

/// What one invocation produced: the outcome, stdout and stderr.
struct Ran {
    outcome: Result<Outcome, Error>,
    out: String,
    err: String,
}

impl Ran {
    /// The exit code the process would return.
    fn exit(&self) -> ExitCode {
        match &self.outcome {
            Ok(outcome) => outcome.exit(),
            Err(error) => error.exit(),
        }
    }

    /// stdout as JSON.
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.out)
            .unwrap_or_else(|error| panic!("--json must emit one document: {error}\n{}", self.out))
    }
}

fn invoke(args: &[&str], fetch: &dyn Fetch) -> Ran {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let outcome = run(&cli(args), fetch, &mut out, &mut err, Style::plain(WIDE));
    Ran {
        outcome,
        out: String::from_utf8(out).expect("the renderer writes UTF-8"),
        err: String::from_utf8(err).expect("the renderer writes UTF-8"),
    }
}

fn is_availability(request: &Request) -> bool {
    request.base.contains("AJAX/JSON")
}

/// Search fixture: three records, all held by DE-11, one of them also by DE-1. Every
/// location's search is answered with the same list — `--at` filters upstream, so what
/// comes back for one ISIL is by definition that library's own result.
fn search_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/filtered.xml")
}

/// The position of the first log line containing a needle.
fn first_index(recorder: &Recorder, needle: &str) -> usize {
    recorder
        .log()
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no request matching {needle:?} in {:?}", recorder.log()))
}

/// The position of the last log line containing a needle.
fn last_index(recorder: &Recorder, needle: &str) -> usize {
    recorder
        .log()
        .iter()
        .rposition(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no request matching {needle:?} in {:?}", recorder.log()))
}

/// With `--at` the human output is grouped: one block per location **in the user's
/// order**, with that location's copies under each hit, and a legend that names only the
/// symbols that occurred.
#[test]
fn two_locations_render_two_blocks_and_a_legend() {
    let fetch = search_fetch();
    let ran = invoke(&["search", "Vorleser", "--at", "HU,STABI"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    assert!(ran.err.is_empty(), "a found result says nothing on stderr");

    let hu = ran
        .out
        .find("HU Berlin ·")
        .unwrap_or_else(|| panic!("no HU block in\n{}", ran.out));
    let stabi = ran
        .out
        .find("Stabi Berlin ·")
        .unwrap_or_else(|| panic!("no Stabi block in\n{}", ran.out));
    assert!(hu < stabi, "the blocks follow --at, not the catalogue");
    // The heading carries the location's own total — the hit count of the search that
    // was restricted to that library, never the size of a joint one.
    assert!(ran.out.contains("HU Berlin · 230 results"), "{}", ran.out);
    // The legend is the last thing on the page and explains what the markers meant.
    let legend = ran
        .out
        .lines()
        .filter(|line| line.contains("available") || line.contains("status not confirmed"))
        .count();
    assert!(legend > 0, "no legend in\n{}", ran.out);
}

/// The JSON document is record-centric with per-location totals in `at[]`, in the user's
/// order, and it states that availability was actually fetched.
#[test]
fn the_json_document_carries_every_location() {
    let fetch = search_fetch();
    let ran = invoke(
        &["--json", "search", "Vorleser", "--at", "HU,STABI"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    let at = document["at"].as_array().expect("at[] is an array");
    assert_eq!(at.len(), 2);
    assert_eq!(at[0]["key"], "HU");
    assert_eq!(at[0]["isil"], "DE-11");
    assert_eq!(at[0]["engine"], "kobv");
    assert_eq!(at[0]["total"], 230);
    assert_eq!(at[1]["key"], "STABI");
    assert_eq!(at[1]["total"], 230);
    assert_eq!(document["engines"], serde_json::json!(["kobv"]));
    assert_eq!(document["availability"], "fetched");
    // Two locations are two searches, and no single hit count is true of both — the
    // honest numbers are the per-location ones above.
    assert_eq!(document["total"], serde_json::Value::Null);
    assert_eq!(document["page"], 1);
    assert_eq!(document["limit"], 10);
    assert!(
        !document["records"]
            .as_array()
            .expect("records is an array")
            .is_empty()
    );
    // A record appears once, whichever blocks would show it.
    assert_eq!(document["shown"], 3);
}

/// `--no-availability` asks nothing about copies, and says so in the document — without
/// that member an empty `items` list and "we never asked" are the same thing.
#[test]
fn no_availability_makes_no_portal_request() {
    let fetch = search_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "HU",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success);
    assert_eq!(ran.json()["availability"], "skipped");
    assert_eq!(recorder.count_matching("AJAX/JSON"), 0);
    // One location, one search, and nothing else at all.
    assert_eq!(recorder.total(), 1);
}

/// The order is the whole design: the searches first — with `--at` already upstream, one
/// per location — and only then availability, for the records that survived paging.
/// Asking earlier would cost one request per record that is never displayed.
#[test]
fn availability_comes_after_every_location_search() {
    let fetch = search_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(&["search", "Vorleser", "--at", "HU,STABI"], &fetch);
    assert_eq!(ran.exit(), ExitCode::Success);

    let log = recorder.log();
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("sru.kobv.de"))
            .count(),
        2,
        "one search per location, and no counting request: {log:?}"
    );
    assert!(
        last_index(&recorder, "sru.kobv.de") < first_index(&recorder, "AJAX/JSON"),
        "availability was asked before both searches were in: {log:?}"
    );
    // Three displayed records, three availability calls — never batched, never more than
    // are shown.
    assert_eq!(recorder.count_matching("AJAX/JSON"), 3);
}

/// `--limit` is a promise **per block**: `--at HU,STABI --limit 1` is one record under
/// each heading, not one record shared between them. A cut over the merged list would let
/// the library that ranks better take the whole page.
#[test]
fn the_limit_applies_to_every_block_on_its_own() {
    let fetch = search_fetch();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "HU,STABI", "--limit", "1",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    let at = document["at"].as_array().expect("at[] is an array");
    for block in at {
        let records = block["records"]
            .as_array()
            .expect("at[].records is an array");
        assert_eq!(records.len(), 1, "{block}");
    }

    let fetch = search_fetch();
    let ran = invoke(
        &["search", "Vorleser", "--at", "HU,STABI", "--limit", "1"],
        &fetch,
    );
    let split = ran
        .out
        .find("Stabi Berlin ·")
        .unwrap_or_else(|| panic!("no Stabi block in\n{}", ran.out));
    let (hu, stabi) = ran.out.split_at(split);
    let records = |block: &str| -> usize {
        block
            .lines()
            .filter(|line| line.contains("almahu_") || line.contains("almafu_"))
            .count()
    };
    assert_eq!(records(hu), 1, "{}", ran.out);
    assert_eq!(records(stabi), 1, "{}", ran.out);
}

/// A filter that empties the window is **not** "nothing found". Exit 1 either way, but
/// the output has to distinguish them: the window numbers are the only way an agent can.
#[test]
fn a_filter_that_matches_nothing_says_how_large_the_window_was() {
    let fetch = search_fetch();
    let ran = invoke(&["search", "Vorleser", "--format", "video"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err
            .contains("none of the 3 fetched records matched --format video"),
        "stderr was {:?}",
        ran.err
    );
    assert!(
        ran.err.contains("narrow the search"),
        "the reason has to name a next step: {:?}",
        ran.err
    );
    assert!(
        ran.out.is_empty(),
        "an empty result renders no list: {:?}",
        ran.out
    );

    let fetch = search_fetch();
    let ran = invoke(
        &["--json", "search", "Vorleser", "--format", "video"],
        &fetch,
    );
    assert_eq!(ran.exit(), ExitCode::NoResults);
    let document = ran.json();
    assert_eq!(document["window"]["fetched"], 3);
    assert_eq!(document["window"]["after_filter"], 0);
    assert_eq!(document["shown"], 0);
    assert_eq!(document["total"], 230);
}

/// `--at` is an upstream filter, so a total of zero means the catalogue has nothing for
/// this query *at these locations* — not that hits exist elsewhere. The block is still
/// rendered (a missing block cannot be told apart from a forgotten one), and the reason
/// says "no results", never "hits exist".
#[test]
fn an_empty_location_search_does_not_claim_hits_exist() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/empty.xml");
    let ran = invoke(&["search", "Zzzzz", "--at", "HU"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.out.contains("HU Berlin \u{b7} no results"),
        "the block is rendered anyway: {:?}",
        ran.out
    );
    assert!(
        ran.err.contains("no results for \"Zzzzz\""),
        "{:?}",
        ran.err
    );
    assert!(
        !ran.err.contains("hits exist"),
        "nothing was found, so nothing exists to be held: {:?}",
        ran.err
    );
}

/// Nothing at all upstream is the other exit 1, and it reads differently.
#[test]
fn an_empty_catalogue_answer_is_no_hits() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/empty.xml");
    let ran = invoke(&["search", "Zzzzz"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err.contains("no results for \"Zzzzz\""),
        "{:?}",
        ran.err
    );
    assert!(ran.out.is_empty());
}

/// `show` routes on the id's prefix and renders the record in full.
#[test]
fn show_renders_one_record() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/record.xml");
    let ran = invoke(&["show", "almafu_BV008885798"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    assert!(ran.out.contains("Augenblick und Irritation"), "{}", ran.out);
    // The id is printed in full — it is the argument for the next invocation.
    assert!(ran.out.contains("almafu_BV008885798"), "{}", ran.out);
}

/// An id no catalogue holds is exit 1, not an error: the lookup worked.
#[test]
fn show_of_an_unknown_id_is_exit_one() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/unknown_id.xml");
    let ran = invoke(&["show", "almafu_BV000000000"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(ran.out.is_empty());
    assert!(ran.err.contains("no such record"), "{:?}", ran.err);

    // In JSON that is the document `null`: a well-formed answer meaning "no record",
    // never an error object.
    let fetch = FixtureFetch::new().fallback("kobv/sru/unknown_id.xml");
    let ran = invoke(&["--json", "show", "almafu_BV000000000"], &fetch);
    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert_eq!(ran.json(), serde_json::Value::Null);
}

/// A branch in `--at` goes to the other engine, and a flag that engine has no index for
/// is refused **before** a request goes out — from either catalogue. Answering it from
/// the KOBV side would be a statement about the whole network, which is the one wrong
/// answer this split exists to prevent. The two engines' searches live in
/// `tests/search_voebb.rs`.
#[test]
fn a_flag_the_other_engine_cannot_honour_is_refused_without_sending_anything() {
    let fetch = search_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &["search", "Vorleser", "--at", "HU,AGB", "--year", "1997"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Usage);
    assert_eq!(
        recorder.total(),
        0,
        "nothing may be sent: {:?}",
        recorder.log()
    );
    let Err(error) = ran.outcome else {
        panic!("an unsupported flag is an error, not a result");
    };
    assert_eq!(error.kind(), "unsupported_by_engine");
    assert!(error.to_string().contains("voebb"), "{error}");
}

/// A [`Fetch`] whose requests never arrive.
///
/// Backoff and retry live *inside* the real `Http`, below this seam, so an error handed
/// back here is the final one — which is exactly the situation the exit code describes.
struct FailingFetch {
    error: fn() -> Error,
}

impl Fetch for FailingFetch {
    fn fetch(&self, _request: &Request) -> Result<Response, Error> {
        Err((self.error)())
    }
}

/// DNS, TLS, connection refused — the request never left.
fn transport_failure() -> Error {
    Error::Network(NetworkError::Transport {
        host: "sru.kobv.de".to_owned(),
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )),
    })
}

/// The request left and nothing came back within the timeout.
fn timeout() -> Error {
    Error::Network(NetworkError::Timeout {
        host: "sru.kobv.de".to_owned(),
        seconds: 10,
    })
}

/// A request that never reaches the service is exit 3, and the failure keeps its own
/// `kind` all the way out of `run` — `network` and `timeout` are different situations and
/// an agent retries them differently.
#[test]
fn an_unreachable_service_is_exit_three() {
    for (make, kind) in [
        (transport_failure as fn() -> Error, "network"),
        (timeout as fn() -> Error, "timeout"),
    ] {
        let fetch = FailingFetch { error: make };
        let ran = invoke(&["search", "Vorleser"], &fetch);

        assert_eq!(ran.exit(), ExitCode::Network, "{kind}");
        assert!(
            ran.out.is_empty(),
            "a failed search renders nothing: {:?}",
            ran.out
        );
        let Err(error) = ran.outcome else {
            panic!("an unreachable service is an error, not an empty result");
        };
        assert_eq!(error.kind(), kind);
        assert_eq!(error.exit().code(), 3);
        assert!(
            error.hint().is_some_and(|hint| hint.contains("try again")),
            "a network failure is worth retrying and has to say so: {:?}",
            error.hint()
        );
    }
}

/// The same failure as the JSON error object: `code` is the process exit code, so an
/// agent that reads the document never has to consult `$?` as well.
#[test]
fn the_json_error_object_of_a_network_failure_carries_code_three() {
    let error = transport_failure();
    let mut out = Vec::new();
    blibs::render::json::error(&error, &mut out).expect("a vector accepts bytes");
    let document: serde_json::Value =
        serde_json::from_slice(&out).expect("the error envelope is one JSON document");
    let body = &document["error"];

    assert_eq!(body["code"], 3);
    assert_eq!(body["kind"], "network");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("sru.kobv.de")),
        "the message names the host that could not be reached: {body}"
    );
    assert!(
        body["hint"].as_str().is_some_and(|hint| !hint.is_empty()),
        "exit 3 always has a next step: {body}"
    );
}

/// A rejected query keeps its own exit code all the way out of `run`.
#[test]
fn a_rejected_query_keeps_exit_five() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/diagnostic.xml");
    let ran = invoke(&["search", "Vorleser"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Rejected);
    assert!(ran.out.is_empty());
}
