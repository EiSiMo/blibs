//! End-to-end tests of one whole invocation: command line in, rendered output and an
//! outcome out, with `tests/fixtures/` standing in for the network.
//!
//! These go through `blibs::cli::run`, so they cover exactly what `main` does apart from
//! locking the streams — including the order that `plan/cli.md` fixes and that nothing
//! else can prove: **search, then select, then availability**, never availability for
//! records that will not be displayed.

mod common;

use blibs::cli::{Cli, run};
use blibs::error::{Error, ExitCode, Outcome};
use blibs::http::Request;
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

fn invoke(args: &[&str], fetch: &FixtureFetch) -> Ran {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let outcome = run(&cli(args), fetch, &mut out, &mut err, Style::plain(WIDE));
    Ran {
        outcome,
        out: String::from_utf8(out).expect("the renderer writes UTF-8"),
        err: String::from_utf8(err).expect("the renderer writes UTF-8"),
    }
}

/// A counting request: `maximumRecords=0` *and* the ISIL attribute. The page size is
/// compared exactly — `10` contains `0`, and a sloppy router would answer the search with
/// the count fixture.
fn is_count(request: &Request) -> bool {
    let param = |key: &str| {
        request
            .query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    };
    param("maximumRecords") == Some("0") && param("x-pquery").is_some_and(|q| q.contains("1044"))
}

fn is_availability(request: &Request) -> bool {
    request.base.contains("AJAX/JSON")
}

/// Search fixture: three records, all held by DE-11, one of them also by DE-1.
fn search_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(is_count, read_fixture("kobv/sru/count.xml"))
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
    // The heading carries the location's own total, from its counting request — not the
    // size of the joint search.
    assert!(ran.out.contains("HU Berlin · 3718 results"), "{}", ran.out);
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
    assert_eq!(at[0]["total"], 3718);
    assert_eq!(at[1]["key"], "STABI");
    assert_eq!(document["engines"], serde_json::json!(["kobv"]));
    assert_eq!(document["availability"], "fetched");
    assert_eq!(document["total"], 230);
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
    // One search and one counting request, and nothing else.
    assert_eq!(recorder.total(), 2);
}

/// The order is the whole design: search first — with `--at` already upstream — then the
/// counting requests that make the headings honest, and only then availability, for the
/// records that survived paging. Asking earlier would cost one request per record that is
/// never displayed.
#[test]
fn availability_comes_after_the_search_and_the_counts() {
    let fetch = search_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(&["search", "Vorleser", "--at", "HU,STABI"], &fetch);
    assert_eq!(ran.exit(), ExitCode::Success);

    let log = recorder.log();
    assert!(
        log[0].contains("maximumRecords=10"),
        "the search goes first: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("maximumRecords=0"))
            .count(),
        2,
        "one counting request per location: {log:?}"
    );
    assert!(
        last_index(&recorder, "maximumRecords=0") < first_index(&recorder, "AJAX/JSON"),
        "availability was asked before the counts were in: {log:?}"
    );
    // Three displayed records, three availability calls — never batched, never more than
    // are shown.
    assert_eq!(recorder.count_matching("AJAX/JSON"), 3);
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
    let fetch = FixtureFetch::new()
        .route(is_count, read_fixture("kobv/sru/count.xml"))
        .fallback("kobv/sru/empty.xml");
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

/// Until the voebb engine exists, a branch in `--at` is refused **before** a request goes
/// out. Answering it from the KOBV side would be a statement about the whole network,
/// which is the one wrong answer this split exists to prevent.
#[test]
fn a_voebb_branch_is_refused_without_sending_anything() {
    let fetch = search_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(&["search", "Vorleser", "--at", "AGB"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Usage);
    assert_eq!(
        recorder.total(),
        0,
        "nothing may be sent: {:?}",
        recorder.log()
    );
    let Err(error) = ran.outcome else {
        panic!("a missing engine is an error, not a result");
    };
    assert_eq!(error.kind(), "unsupported_by_engine");
    assert!(error.to_string().contains("voebb"), "{error}");
}

/// One failing engine fails the whole invocation. A half-answer that looks whole is worse
/// than no answer, so the KOBV half is not rendered either.
#[test]
fn one_failing_engine_fails_the_invocation() {
    let fetch = search_fetch();
    let ran = invoke(&["search", "Vorleser", "--at", "HU,AGB"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Usage);
    assert!(ran.out.is_empty(), "nothing is rendered: {:?}", ran.out);
}

/// A rejected query keeps its own exit code all the way out of `run`.
#[test]
fn a_rejected_query_keeps_exit_five() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/diagnostic.xml");
    let ran = invoke(&["search", "Vorleser"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Rejected);
    assert!(ran.out.is_empty());
}
