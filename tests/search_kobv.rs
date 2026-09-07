//! End-to-end tests of the KOBV engine against saved fixtures.
//!
//! The seam is `blibs::http::Fetch`: [`FixtureFetch`] answers out of `tests/fixtures/`
//! and its [`Recorder`] remembers *what* was asked, *how often* and *how much of it at
//! once*. That last number is a promise to an upstream service that serves
//! `robots.txt: Disallow: /`, so it is asserted rather than assumed.
//!
//! What is proven here, from `plan/`:
//!
//! - `--at HU,STABI` issues exactly one search request **per location** and no counting
//!   request at all — `--limit` is a promise per block, so every block needs a window of
//!   its own, and a location's own search already states its total;
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
    }
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
/// The availability keys that were asked for, **sorted**.
///
/// Sorted because `fill_availability` fires one request per record concurrently, so the
/// order they reach the log in is a race. Which keys were built is the contract; which of
/// two concurrent requests won is not, and asserting on it made this test fail about once
/// in a full suite run.
fn availability_ids(recorder: &Recorder) -> Vec<String> {
    let mut ids: Vec<String> = recorder
        .log()
        .iter()
        .filter_map(|line| line.split("availability_id=").nth(1).map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

fn search(fetch: &FixtureFetch, request: &SearchRequest) -> blibs::model::EngineSearch {
    Kobv::new(fetch)
        .search(request)
        .unwrap_or_else(|error| panic!("the fixture search succeeds: {error}"))
}

/// Two institutions in `--at` are **two** searches, one per library, and nothing else —
/// no joint `@or` search, no counting request, no client-side sieve over an unfiltered
/// result.
///
/// One search per location is what `--limit` per block requires: a joint search delivers
/// one window ranked across both libraries, and the block whose records rank lower would
/// come out short of its limit while its catalogue holds hundreds. The total each block
/// prints comes free with it — `numberOfRecords` of a search restricted to one ISIL is
/// that location's own count.
///
/// The PQF is asserted character for character against the measurement in
/// `plan/scraping.md` §A.4a, including the canonical `DE-11`: the attribute is
/// case-sensitive and `de-11` would match nothing, silently.
#[test]
fn two_locations_are_two_searches_and_nothing_else() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/filtered.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Vorleser", at(&["HU", "STABI"])));

    assert_eq!(
        recorder.total(),
        2,
        "one search per location: {:?}",
        recorder.log()
    );
    assert_eq!(
        recorder.count_matching("maximumRecords=0"),
        0,
        "the counting request is gone: {:?}",
        recorder.log()
    );
    assert_eq!(recorder.count_matching("startRecord=1"), 2);
    assert_eq!(
        queries(&recorder),
        vec![
            "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-11",
            "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-1",
        ]
    );
    // Neither of the two searches is *the* query, and neither hit count is true of the
    // whole answer — the honest numbers are the per-location ones.
    assert_eq!(result.query_echo, None);
    assert_eq!(result.total, None);
    assert_eq!(result.engine, Engine::Kobv);
}

/// One location is one search, and then there *is* a single query and a single hit count
/// to state.
#[test]
fn one_location_states_its_query_and_its_total() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/filtered.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Vorleser", at(&["HU"])));

    assert_eq!(recorder.total(), 1);
    assert_eq!(
        result.query_echo.as_deref(),
        Some("@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-11")
    );
    assert_eq!(result.total, Some(230));
    assert_eq!(result.at[0].total, Some(230));
}

/// Two aliases of the same institution are one search: the query and the answer would be
/// identical, and both blocks are filled from it.
#[test]
fn two_aliases_of_one_isil_share_a_single_search() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/filtered.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Vorleser", at(&["HU", "DE-11"])));

    assert_eq!(recorder.total(), 1, "{:?}", recorder.log());
    assert_eq!(result.at.len(), 2);
    assert_eq!(result.at[0].total, result.at[1].total);
    assert_eq!(result.at[0].records, result.at[1].records);
}

/// `at[]` carries the *location's* hit count and the records that location's search
/// returned — the membership of the block, stated by the catalogue rather than read back
/// out of the records.
#[test]
fn each_location_reports_its_own_total() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/filtered.xml");

    let result = search(&fetch, &request("Vorleser", at(&["HU", "STABI"])));

    let at: Vec<(&str, &str, Option<u64>)> = result
        .at
        .iter()
        .map(|block| (block.key.as_str(), block.isil.as_str(), block.total))
        .collect();
    assert_eq!(
        at,
        vec![("HU", "DE-11", Some(230)), ("STABI", "DE-1", Some(230))]
    );
    assert!(result.at.iter().all(|block| block.engine == Engine::Kobv));
    assert!(result.at.iter().all(|block| block.branch.is_none()));
    assert_eq!(
        result.at[0].records.len(),
        result.records.len(),
        "the block is what its own search returned"
    );
}

/// Without `--at` there is no `at[]` and only one search — with no holdings filter, and
/// with the whole catalogue's hit count.
#[test]
fn without_locations_one_unfiltered_search_runs() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let recorder = fetch.recorder();

    let result = search(&fetch, &request("Prozess", Vec::new()));

    assert_eq!(recorder.total(), 1);
    assert!(result.at.is_empty());
    assert!(result.total.is_some());
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
            "DE-11;BV019771323,DE-188;BV019771323,DE-521;BV019771323,\
             DE-1;48035670X,DE-517;48035670X,",
            "DE-B486;BV044513648,",
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
