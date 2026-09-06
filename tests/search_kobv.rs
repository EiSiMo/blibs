//! End-to-end tests of the KOBV engine against saved fixtures.
//!
//! The seam is `blibs::http::Fetch`: [`FixtureFetch`] answers out of `tests/fixtures/`
//! and its [`Recorder`] remembers *what* was asked, *how often* and *how much of it at
//! once*. That last number is a promise to an upstream service that serves
//! `robots.txt: Disallow: /`, so it is asserted rather than assumed.
//!
//! What is proven here, from `plan/`:
//!
//! - `--at HU,STABI` issues exactly one search request and two counting requests;
//! - the PQF that goes out is character-for-character what `plan/scraping.md` §A.4a
//!   documents, with canonical ISIL spelling;
//! - one availability request per record, a record without MARC `924` causes none, and
//!   `AvailabilityMode::Skipped` causes none at all;
//! - the recorder never sees more than six requests in flight at once;
//! - a diagnostic, an HTML error page and a throttled service stay three different
//!   errors with three different exit codes.

mod common;

use std::time::Duration;

use blibs::engine::kobv::Kobv;
use blibs::error::{Error, ExitCode, ServiceError};
use blibs::http::{Request, Response};
use blibs::libraries;
use blibs::model::{
    AvailabilityMode, Catalog, Engine, FetchWindow, Limit, Location, Page, QuerySpec, Record,
    RecordId, SearchRequest, Status, Term,
};

use common::{FixtureFetch, Recorder, read_fixture};

/// The reference query of `plan/scraping.md` §A.4a, in the shape `cli` hands over: one
/// free word, no flags.
fn query(word: &str) -> QuerySpec {
    QuerySpec {
        terms: vec![Term::from_argument(word)],
        ..QuerySpec::default()
    }
}

/// Resolve `--at` exactly as `cli` does — through the library list, never by writing an
/// ISIL into the test. That is what makes the canonical-spelling assertion mean anything.
fn at(aliases: &[&str]) -> Vec<Location> {
    aliases
        .iter()
        .map(|alias| {
            libraries::resolve(alias).unwrap_or_else(|error| panic!("{alias} resolves: {error}"))
        })
        .collect()
}

fn request(word: &str, locations: Vec<Location>) -> SearchRequest {
    SearchRequest {
        query: query(word),
        locations,
        window: FetchWindow::plan(Limit::DEFAULT, Page::FIRST, false),
        want_totals: true,
    }
}

/// Is this the counting request of a location? `maximumRecords=0` *and* the ISIL
/// attribute — the two together are what a per-location count is.
///
/// The page size is compared exactly, not by substring: `10` contains `0`, and a router
/// that overlooks that answers the *search* with a count fixture.
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

/// The PQF of every request that carried one, in the order they went out.
fn queries(recorder: &Recorder) -> Vec<String> {
    recorder
        .log()
        .iter()
        .filter_map(|line| line.split("x-pquery=").nth(1).map(str::to_owned))
        .collect()
}

/// The `availability_id` of every availability request, in the order they went out.
fn availability_ids(recorder: &Recorder) -> Vec<String> {
    recorder
        .log()
        .iter()
        .filter_map(|line| line.split("availability_id=").nth(1).map(str::to_owned))
        .collect()
}

fn search(fetch: &FixtureFetch, request: &SearchRequest) -> blibs::model::EngineSearch {
    Kobv::new(fetch)
        .search(request)
        .unwrap_or_else(|error| panic!("the fixture search succeeds: {error}"))
}

/// Two institutions in `--at` are **one** search and one counting request each — never a
/// search per library, and never a client-side sieve over an unfiltered result.
///
/// The PQF is asserted character for character against the measurement in
/// `plan/scraping.md` §A.4a, including the canonical `DE-11`: the attribute is
/// case-sensitive and `de-11` would match nothing, silently.
#[test]
fn two_locations_are_one_search_and_two_counts() {
    let fetch = FixtureFetch::new()
        .route(is_count, read_fixture("kobv/sru/count.xml"))
        .fallback("kobv/sru/filtered.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Vorleser", at(&["HU", "STABI"])));

    assert_eq!(
        recorder.total(),
        3,
        "one search and two counts: {:?}",
        recorder.log()
    );
    assert_eq!(recorder.count_matching("maximumRecords=0"), 2);
    assert_eq!(recorder.count_matching("startRecord=1"), 1);
    assert_eq!(
        queries(&recorder),
        vec![
            "@and @attr 1=1016 \"Vorleser\" @or @attr 1=1044 DE-11 @attr 1=1044 DE-1",
            "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-11",
            "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-1",
        ]
    );
    assert_eq!(
        result.query_echo.as_deref(),
        Some(queries(&recorder)[0].as_str())
    );
    assert_eq!(result.engine, Engine::Kobv);
}

/// `at[]` carries the *location's* hit count, not the size of the joint search — that is
/// the entire reason the counting requests are paid for. Both counts are answered with
/// `count.xml` (3718), the search with `filtered.xml` (230), and the two numbers must not
/// be confused.
#[test]
fn each_location_reports_its_own_total() {
    let fetch = FixtureFetch::new()
        .route(is_count, read_fixture("kobv/sru/count.xml"))
        .fallback("kobv/sru/filtered.xml");

    let result = search(&fetch, &request("Vorleser", at(&["HU", "STABI"])));

    assert_eq!(result.total, Some(230));
    let at: Vec<(&str, &str, Option<u64>)> = result
        .at
        .iter()
        .map(|block| (block.key.as_str(), block.isil.as_str(), block.total))
        .collect();
    assert_eq!(
        at,
        vec![("HU", "DE-11", Some(3718)), ("STABI", "DE-1", Some(3718))]
    );
    assert!(result.at.iter().all(|block| block.engine == Engine::Kobv));
    assert!(result.at.iter().all(|block| block.branch.is_none()));
}

/// Without `--at` there is no `at[]` and no counting request — a total per location is
/// only ever paid for when a location was named.
#[test]
fn without_locations_nothing_is_counted() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Prozess", Vec::new()));

    assert_eq!(recorder.total(), 1);
    assert!(result.at.is_empty());
    assert_eq!(queries(&recorder), vec!["@attr 1=1016 \"Prozess\""]);
}

/// The library list only ever *adds* names. `library` is the full official name and
/// `short_name` the short one — the split the JSON contract in `plan/cli.md` fixes.
#[test]
fn holdings_are_named_from_the_library_list() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let result = search(&fetch, &request("Prozess", Vec::new()));

    let holding = result.records[1]
        .holdings
        .iter()
        .find(|holding| {
            holding
                .isil
                .as_ref()
                .is_some_and(|isil| isil.as_str() == "DE-11")
        })
        .expect("almahu_BV019771323 states a DE-11 holding");
    assert_eq!(
        holding.library,
        "Humboldt-Universität zu Berlin, Universitätsbibliothek, Jacob-und-Wilhelm-Grimm-Zentrum"
    );
    assert_eq!(holding.short_name.as_deref(), Some("HU Berlin"));
    assert_eq!(holding.alias.as_deref(), Some("HU"));
    assert!(
        !holding.mine,
        "`mine` is decided in `select`, not in the engine"
    );
}

/// One call per record, never batched: the response is keyed by ISIL, so two records in
/// one call would collide. The key itself is asserted, because it is assembled from MARC
/// `924` and a wrong one answers about the wrong book.
#[test]
fn availability_is_one_call_per_record_with_the_key_from_924() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/mono_kafka.xml");
    let recorder = fetch.recorder();
    let mut records = search(&fetch, &request("Prozess", Vec::new())).records;
    assert_eq!(records.len(), 2);

    let notes = Kobv::new(&fetch)
        .fill_availability(&mut records)
        .expect("the fixture answers every record");

    assert_eq!(recorder.count_matching("AJAX/JSON"), 2);
    assert_eq!(
        availability_ids(&recorder),
        vec![
            "DE-B486;BV044513648,",
            "DE-11;BV019771323,DE-188;BV019771323,DE-521;BV019771323,\
             DE-1;48035670X,DE-517;48035670X,",
        ]
    );

    let hu = records[1]
        .holdings
        .iter()
        .find(|holding| {
            holding
                .isil
                .as_ref()
                .is_some_and(|isil| isil.as_str() == "DE-11")
        })
        .expect("the DE-11 holding survives the merge");
    assert_eq!(hu.summary, Status::Available);
    assert!(!hu.items.is_empty(), "the shelf table reached the holding");
    // `yellow` is reference, not "available" — the trap of §B.5.
    let fu = records[1]
        .holdings
        .iter()
        .find(|holding| {
            holding
                .isil
                .as_ref()
                .is_some_and(|isil| isil.as_str() == "DE-188")
        })
        .expect("DE-188 is stated in the record and in the response");
    assert_eq!(fu.summary, Status::Reference);
    assert!(
        notes.is_empty(),
        "a fully answered record needs no note: {notes:?}"
    );
}

/// 4.9 % of records carry no `924` at all. There is no key to ask with, so nothing is
/// asked — and the empty holdings list keeps meaning "not stated in this record".
#[test]
fn a_record_without_924_is_never_asked_about() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/newspaper.xml");
    let recorder = fetch.recorder();
    let mut records = search(&fetch, &request("Tageszeitung", Vec::new())).records;
    assert_eq!(records.len(), 1);
    assert!(records[0].holdings.is_empty());

    let notes = Kobv::new(&fetch)
        .fill_availability(&mut records)
        .expect("nothing to ask is not a failure");

    assert_eq!(recorder.count_matching("AJAX/JSON"), 0);
    assert!(notes.is_empty());
    assert!(records[0].holdings.is_empty());
}

/// The per-host cap is a promise, not an aspiration. With a delay on every request the
/// overlap is observable, and it must reach more than one — otherwise the test would pass
/// on a completely serial client and prove nothing.
#[test]
fn availability_never_runs_more_than_six_at_once() {
    let fetch = FixtureFetch::new()
        .with_delay(Duration::from_millis(20))
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/mono_kafka.xml");
    let recorder = fetch.recorder();
    let found = search(&fetch, &request("Prozess", Vec::new())).records;
    let mut records: Vec<Record> = std::iter::repeat_n(found[1].clone(), 14).collect();

    Kobv::new(&fetch)
        .fill_availability(&mut records)
        .expect("the fixture answers every record");

    assert_eq!(recorder.count_matching("AJAX/JSON"), 14);
    assert!(
        recorder.max_in_flight() > 1,
        "the requests never overlapped; the cap assertion would prove nothing"
    );
    assert!(
        recorder.max_in_flight() <= 6,
        "{} requests were in flight at once, the cap is 6",
        recorder.max_in_flight()
    );
}

/// `show` is a lookup by `rec.id` (attribute `1=12`) and, by default, one availability
/// call for the single record it found.
#[test]
fn show_finds_a_record_and_fetches_its_copies() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/record.xml");
    let recorder = fetch.recorder();
    let id = RecordId::parse("almafu_BV008885798").expect("a prefixed id parses");

    let record = Kobv::new(&fetch)
        .show(&id, AvailabilityMode::Fetched)
        .expect("the fixture answers")
        .expect("record.xml carries exactly this record");

    assert_eq!(record.id.as_str(), "almafu_BV008885798");
    assert_eq!(record.title, "Augenblick und Irritation");
    assert_eq!(queries(&recorder)[0], "@attr 1=12 \"almafu_BV008885798\"");
    assert_eq!(recorder.count_matching("maximumRecords=1"), 1);
    assert_eq!(recorder.count_matching("AJAX/JSON"), 1);
}

/// `--no-availability` reaches the engine as [`AvailabilityMode::Skipped`], and then not
/// a single copy-level request goes out.
#[test]
fn show_asks_nothing_when_availability_is_skipped() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/record.xml");
    let recorder = fetch.recorder();
    let id = RecordId::parse("almafu_BV008885798").expect("a prefixed id parses");

    let record = Kobv::new(&fetch)
        .show(&id, AvailabilityMode::Skipped)
        .expect("the fixture answers");

    assert!(record.is_some());
    assert_eq!(recorder.total(), 1);
}

/// A well-formed id that no catalogue holds is `Ok(None)` — exit 1, "no results", not an
/// error. `unknown_id.xml` is exactly that response: `numberOfRecords=0`, no diagnostic.
#[test]
fn show_of_an_unknown_id_is_no_result_not_an_error() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/unknown_id.xml");
    let id = RecordId::parse("almafu_BV000000000").expect("a prefixed id parses");

    let found = Kobv::new(&fetch)
        .show(&id, AvailabilityMode::Fetched)
        .expect("an empty answer is not a failure");

    assert!(found.is_none());
}

/// A record announced and delivered as a surrogate diagnostic is counted, explained in a
/// note, and does not take the other records with it. `kids.xml` carries one at position
/// 49, between two ordinary records.
#[test]
fn a_surrogate_diagnostic_is_counted_and_explained() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/kids.xml");

    let result = search(&fetch, &request("Bilderbuch", Vec::new()));

    assert_eq!(result.records.len(), 2);
    assert_eq!(result.fetched, 2);
    assert_eq!(result.undelivered, 1);
    assert_eq!(result.notes.len(), 1);
    assert_eq!(result.notes[0].kind, "record_undelivered");
}

/// A rejected query is exit 5 and nothing else. It arrives with **HTTP 200**, which is
/// the whole reason the body is checked at all.
#[test]
fn a_top_level_diagnostic_is_a_rejected_query() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/diagnostic.xml");

    let error = Kobv::new(&fetch)
        .search(&request("Prozess", Vec::new()))
        .expect_err("a diagnostic is not a result");

    assert!(matches!(error, Error::Rejected(_)), "got {error:?}");
    assert_eq!(error.exit(), ExitCode::Rejected);
    assert_eq!(error.kind(), "query_rejected");
}

/// An HTML error page with status 200 is exit 6 — an implausible response, not an empty
/// result and not a rejected query.
#[test]
fn an_html_error_page_is_an_unexpected_response() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/not_xml.html");

    let error = Kobv::new(&fetch)
        .search(&request("Prozess", Vec::new()))
        .expect_err("a proxy error page is not a result");

    assert_eq!(error.exit(), ExitCode::Unexpected);
    assert_eq!(error.kind(), "not_xml");
}

/// Whether a status is an answer is decided in `http` and nowhere else, so the engine has
/// no status check to test: what has to hold here is that a transport-level error travels
/// through the engine **unchanged**, keeping the exit code an agent backs off on.
///
/// A closure is a `Fetch`, which is why this needs no stub type of its own.
#[test]
fn a_throttled_service_keeps_its_own_exit_code() {
    let throttled = |_: &Request| {
        Err::<Response, Error>(
            ServiceError::Throttled {
                host: "sru.kobv.de".to_owned(),
                status: 503,
                attempts: 3,
            }
            .into(),
        )
    };

    let error = Kobv::new(&throttled)
        .search(&request("Prozess", Vec::new()))
        .expect_err("the transport gave up");

    assert_eq!(error.exit(), ExitCode::Unavailable);
    assert_eq!(error.kind(), "service_unavailable");
    assert!(error.to_string().contains("sru.kobv.de"));
}

/// One failed availability call fails the whole fill. Half the records answered would
/// look exactly like half the records having no copies — the worst answer this tool can
/// give.
#[test]
fn a_failed_availability_call_fails_the_whole_fill() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let mut records = search(&fetch, &request("Prozess", Vec::new())).records;

    let broken = |request: &Request| {
        if request.base.contains("AJAX/JSON") {
            Err(Error::from(ServiceError::Throttled {
                host: "portal.kobv.de".to_owned(),
                status: 429,
                attempts: 3,
            }))
        } else {
            Ok(Response {
                status: 200,
                body: read_fixture("kobv/sru/mono_kafka.xml"),
                content_type: None,
                from_cache: false,
            })
        }
    };

    let error = Kobv::new(&broken)
        .fill_availability(&mut records)
        .expect_err("an unanswered record is not an answer");

    assert_eq!(error.exit(), ExitCode::Unavailable);
}
