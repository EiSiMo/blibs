//! End-to-end tests of the voebb.de engine against saved fixtures.
//!
//! What has to be proven here, from `plan/voebb.md`:
//!
//! - the session is opened, searched and filtered **in that order**, and every request
//!   replays the form state of the answer before it;
//! - the branch facet is an upstream filter, so `at[].total` is the branch's own count;
//! - `$Toolbar=3` is what fetches a further page, and the window decides how many;
//! - a `/noaccess` page is a lost session (exit 6) and never "no hits";
//! - the record page is fetched once per **displayed** record, without a session;
//! - both engines run for one invocation and produce one document with two blocks.

mod common;

use blibs::cli::{Cli, run};
use blibs::error::{Error, ExitCode, NetworkError, Outcome};
use blibs::http::{Fetch, Request};
use blibs::render::Style;
use clap::Parser;

use common::{FixtureFetch, describe, field, has_field, read_fixture};

/// A terminal wide enough that nothing is truncated, and never coloured.
const WIDE: usize = 140;

/// Parse a command line the way the binary does.
fn cli(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("blibs").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("the test invocation must parse: {error}"))
}

/// What one invocation produced.
struct Ran {
    outcome: Result<Outcome, Error>,
    out: String,
    err: String,
}

impl Ran {
    fn exit(&self) -> ExitCode {
        match &self.outcome {
            Ok(outcome) => outcome.exit(),
            Err(error) => error.exit(),
        }
    }

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

/// The record page, which carries no session at all.
fn is_record_page(request: &Request) -> bool {
    request
        .query
        .iter()
        .any(|(key, value)| key == "sp" && value.starts_with("SAK"))
}

/// Which record a record-page request is about.
fn record_of(request: &Request) -> &str {
    request
        .query
        .iter()
        .rev()
        .find(|(key, _)| key == "sp")
        .map_or("", |(_, value)| value.as_str())
}

/// The measured session: the entry page, the search, the branch facet, a further page,
/// and one record page per record. The order of the routes is the order of the session,
/// and the two form-driven ones are told apart by the field that drives them.
fn voebb_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(
            |request| has_field(request, "$Toolbar"),
            read_fixture("voebb/results_filtered_page2.html"),
        )
        .route(
            |request| record_of(request) == "SAK16112988",
            read_fixture("voebb/detail_online.html"),
        )
        .route(
            |request| record_of(request) == "SAK00000000",
            read_fixture("voebb/detail_unknown.html"),
        )
        .route(
            |request| record_of(request) == "SAK13776205",
            read_fixture("voebb/detail_available.html"),
        )
        .route(is_record_page, read_fixture("voebb/detail_on_loan.html"))
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// The advanced-search path: the entry page, the form, the result list, the facet.
fn advanced_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(
            |request| has_field(request, "$Autosuggest$0"),
            read_fixture("voebb/results_isbn.html"),
        )
        .route(
            |request| field(request, "$Button$0") == Some("pressed"),
            read_fixture("voebb/advanced_form.html"),
        )
        .fallback("voebb/start.html")
}

/// Both catalogues behind one stub, the way one invocation sees them.
fn both_engines_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| request.base.contains("AJAX/JSON"),
            read_fixture("kobv/availability/mixed.json"),
        )
        .route(
            |request| request.base.contains("sru.kobv.de"),
            read_fixture("kobv/sru/filtered.xml"),
        )
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(is_record_page, read_fixture("voebb/detail_available.html"))
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// The order of a voebb search is the session: open, search, filter. Each request
/// replays the answer before it, which is the only reason the site accepts any of them.
#[test]
fn a_branch_search_opens_a_session_searches_and_filters() {
    let fetch = voebb_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--limit",
            "3",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let log = recorder.log();
    assert_eq!(log.len(), 3, "{log:?}");
    assert_eq!(log[0], "GET https://www.voebb.de/");
    assert!(
        log[1].starts_with("POST https://www.voebb.de/aDISWeb/"),
        "{log:?}"
    );
    assert!(
        log[1].contains("$Autosuggest=Vorleser") && log[1].contains("$Select=Bibliotheksbestand"),
        "the search carries the terms and the scope: {}",
        log[1]
    );
    assert!(
        log[2].contains("$CbTree_text=sub-PTL1_tree_1_90"),
        "the facet is ticked by the checkbox's id: {}",
        log[2]
    );
    assert!(
        log[2].contains("$Button$2=pressed")
            && log[2].contains("source=$B")
            && log[2].contains("focus=$$GFBO_11"),
        "a button needs its own name, source and focus: {}",
        log[2]
    );
}

/// Every request carries the `identity` of the answer before it, and `requestCount`
/// counts up. Sending either one twice is what produces `/noaccess`.
#[test]
fn every_request_replays_the_form_state_of_the_last_answer() {
    let fetch = voebb_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &["search", "Vorleser", "--at", "AGB", "--no-availability"],
        &fetch,
    );
    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);

    let log = recorder.log();
    // start.html carries requestCount=1 and results.html requestCount=2.
    assert!(log[1].contains("requestCount=1"), "{}", log[1]);
    assert!(log[2].contains("requestCount=2"), "{}", log[2]);
    assert!(
        log[1].contains("identity=5faUoXJ8MCxJfwi-"),
        "the identity of the page that was answered: {}",
        log[1]
    );
    assert!(
        !log[2].contains("identity=5faUoXJ8MCxJfwi-"),
        "a reused identity is a lost session: {}",
        log[2]
    );
    // The action path carries the session id and comes from the form, not from a guess.
    assert!(
        log[1].contains("/aDISWeb/_21g4ad7n7gpvxuzq2ryo9pn9w87rqtq7/app"),
        "{}",
        log[1]
    );
}

/// The facet is an upstream filter: the heading carries the branch's own count (35), not
/// the 71 of the unfiltered search, and the records come from the filtered list.
#[test]
fn the_branch_total_is_the_facets_own_count() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--limit",
            "3",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    assert_eq!(document["engines"], serde_json::json!(["voebb"]));
    let at = document["at"].as_array().expect("at[] is an array");
    assert_eq!(at.len(), 1);
    assert_eq!(at[0]["key"], "AGB");
    assert_eq!(at[0]["isil"], "DE-609");
    assert_eq!(at[0]["branch"], "SIG00036");
    assert_eq!(at[0]["engine"], "voebb");
    assert_eq!(at[0]["total"], 35);
    // Two catalogues have two totals; the document's own is the KOBV one and stays null.
    assert_eq!(document["total"], serde_json::Value::Null);
    assert_eq!(document["query"]["pqf"], serde_json::Value::Null);

    let records = document["records"].as_array().expect("records is an array");
    assert_eq!(records.len(), 3, "the window is --limit records long");
    assert_eq!(records[0]["id"], "voebb_SAK15039548");
    assert_eq!(records[0]["engine"], "voebb");
    assert_eq!(records[0]["source"], "voebb");
    assert_eq!(records[0]["local_id"], "SAK15039548");
}

/// The human output puts the branch's own heading over the block, the way `plan/cli.md`
/// has it.
#[test]
fn the_block_heading_names_the_branch_and_its_total() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--limit",
            "3",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert!(
        ran.out.contains("AGB (VÖBB) · 35 results · showing 3"),
        "{}",
        ran.out
    );
}

/// One record page per **displayed** record, and the copies come back with the branch
/// they belong to. The record page needs no session — it carries no form state at all.
#[test]
fn availability_is_one_record_page_per_displayed_record() {
    let fetch = voebb_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB", "--limit", "3",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert_eq!(
        recorder.count_matching("sp=SPROD00"),
        3,
        "one per displayed record, never more: {:?}",
        recorder.log()
    );
    // The record pages come last: nothing is asked about a record that is not displayed.
    let log = recorder.log();
    let first_record_page = log
        .iter()
        .position(|line| line.contains("sp=SPROD00"))
        .expect("a record page was fetched");
    assert!(
        log[..first_record_page]
            .iter()
            .any(|line| line.contains("$CbTree_text")),
        "the filter runs before the record pages: {log:?}"
    );

    let document = ran.json();
    assert_eq!(document["availability"], "fetched");
    let records = document["records"].as_array().expect("records is an array");
    let items: Vec<&serde_json::Value> = records
        .iter()
        .flat_map(|record| record["holdings"].as_array().into_iter().flatten())
        .flat_map(|holding| holding["items"].as_array().into_iter().flatten())
        .collect();
    assert!(!items.is_empty(), "the record pages carry the copies");
    assert!(
        items.iter().any(|item| item["branch"].is_string()),
        "a copy names the branch it stands in: {items:?}"
    );
    assert!(
        items.iter().any(|item| item["order_option"].is_string()),
        "voebb.de states an ordering option per copy: {items:?}"
    );
    // The Onleihe record among the three has no copies, and says why.
    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        notes.iter().any(|note| note["kind"] == "voebb_online_only"),
        "an empty copy list is never silent: {notes:?}"
    );
}

/// `--available` hides the Onleihe record, and hides it as a record it **judged**.
///
/// Changed in round 2 (§3.6): this test used to require the
/// `availability_filter_unstated` note, because the tool left an electronic title at
/// [`Status::Unknown`] while printing its own translation of `(Das Medium ist
/// ausgeliehen / …)` two lines below. The status is read now, so the record is hidden
/// for a *stated* reason and there is nothing left for that note to explain — a note
/// that fired here would be the contradiction, not the fix.
#[test]
fn an_electronic_title_that_is_out_is_hidden_as_a_judged_record() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--limit",
            "3",
            "--available",
        ],
        &fetch,
    );

    let document = ran.json();
    let records = document["records"].as_array().expect("records is an array");
    assert!(
        !records
            .iter()
            .any(|record| record["id"] == "voebb_SAK16112988"),
        "the Onleihe copy is out, so --available hides it: {records:?}"
    );

    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        !notes
            .iter()
            .any(|note| note["kind"] == "availability_filter_unstated"),
        "the loan state was read, so nothing about this page is unjudged: {notes:?}"
    );
    // The empty copy list of an electronic title is still never silent — that note says
    // where the status came from, and it is a different statement from the one above.
    assert!(
        notes.iter().any(|note| note["kind"] == "voebb_online_only"),
        "{notes:?}"
    );
}

/// A window that starts behind the first page is paged to with `$Toolbar=3` — one field
/// carrying the button index as its value.
#[test]
fn a_second_page_is_fetched_with_the_toolbar() {
    let fetch = voebb_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--limit",
            "22",
            "--page",
            "2",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let log = recorder.log();
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("$Toolbar=3"))
            .count(),
        1,
        "one paging request, and never `$Toolbar_3=…`: {log:?}"
    );
    assert!(
        !log.iter().any(|line| line.contains("$Toolbar_3=")),
        "the paging field is `$Toolbar` with the index as its value: {log:?}"
    );
    // The facet is not ticked again — it survives the paging.
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("$CbTree_text"))
            .count(),
        1,
        "{log:?}"
    );
    // Page 2 of the filtered list is positions 23…35, and only those are displayed.
    assert!(ran.out.contains("voebb_SAK15220475"), "{}", ran.out);
    assert!(!ran.out.contains("voebb_SAK15039548"), "{}", ran.out);
}

/// The failure page of the house: HTTP 200, no form, "please close this tab". Read as a
/// result list it looks like "no hits", which is why it is a named error instead — and
/// why nothing else is asked afterwards.
#[test]
fn a_lost_session_is_exit_six_and_stops_the_invocation() {
    let fetch = FixtureFetch::new()
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/noaccess.html"),
        )
        .fallback("voebb/start.html");
    let recorder = fetch.recorder();
    let ran = invoke(&["search", "Vorleser", "--at", "AGB"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Unexpected);
    let Err(error) = ran.outcome else {
        panic!("a lost session is an error, not an empty result");
    };
    assert_eq!(error.kind(), "voebb_session_lost");
    assert!(error.to_string().contains("the search"), "{error}");
    assert_eq!(
        recorder.total(),
        2,
        "the entry page and the search, and then nothing: {:?}",
        recorder.log()
    );
    assert!(ran.out.is_empty(), "nothing is rendered: {:?}", ran.out);
}

/// A mixed `--at` runs both catalogues in one invocation and renders one document with a
/// block per location, in the user's order. Nothing is merged across the two: the same
/// edition may legitimately appear in both blocks with two different record ids.
#[test]
fn a_mixed_at_runs_both_engines_and_renders_both_blocks() {
    let fetch = both_engines_fetch();
    let ran = invoke(
        &["search", "Vorleser", "--at", "AGB,HU", "--limit", "2"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let agb = ran
        .out
        .find("AGB (VÖBB) ·")
        .unwrap_or_else(|| panic!("no AGB block in\n{}", ran.out));
    let hu = ran
        .out
        .find("HU Berlin ·")
        .unwrap_or_else(|| panic!("no HU block in\n{}", ran.out));
    assert!(agb < hu, "the blocks follow --at, not the engines");

    let fetch = both_engines_fetch();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB,HU", "--limit", "2",
        ],
        &fetch,
    );
    let document = ran.json();
    let engines = document["engines"].as_array().expect("engines is an array");
    assert_eq!(engines.len(), 2, "{engines:?}");
    assert!(engines.iter().any(|engine| engine == "kobv"), "{engines:?}");
    assert!(
        engines.iter().any(|engine| engine == "voebb"),
        "{engines:?}"
    );

    let at = document["at"].as_array().expect("at[] is an array");
    assert_eq!(at[0]["key"], "AGB");
    assert_eq!(at[0]["engine"], "voebb");
    assert_eq!(at[0]["total"], 35);
    assert_eq!(at[1]["key"], "HU");
    assert_eq!(at[1]["engine"], "kobv");
    assert_eq!(at[1]["total"], 230);
    // The KOBV search's total stays the document's — one location, one search, one hit
    // count; the voebb count lives in at[], where every location's does.
    assert_eq!(document["total"], 230);

    let engines_of_records: Vec<&str> = document["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .filter_map(|record| record["engine"].as_str())
        .collect();
    assert!(
        engines_of_records.contains(&"voebb"),
        "{engines_of_records:?}"
    );
    assert!(
        engines_of_records.contains(&"kobv"),
        "{engines_of_records:?}"
    );
}

/// Two branches of the same network, each on its own session, each with its own facet
/// checkbox. The AGB list and the `BStB` list share eight of nine records and differ in one.
fn two_branches_fetch() -> FixtureFetch {
    let bstb = read_fixture("voebb/results_filtered.html").replace("SAK15039548", "SAK99999901");
    FixtureFetch::new()
        .route(
            |request| field(request, "$CbTree_text") == Some(AGB_CHECKBOX),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(
            |request| field(request, "$CbTree_text") == Some(BSTB_CHECKBOX),
            bstb,
        )
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// The facet checkboxes of the two aliased ZLB branches in `voebb/results.html`.
const AGB_CHECKBOX: &str = "sub-PTL1_tree_1_90";
const BSTB_CHECKBOX: &str = "sub-PTL1_tree_1_91";

/// Every voebb.de holding carries `DE-609`, the network's ISIL, and with
/// `--no-availability` no copy exists that could name a branch either — so nothing *in a
/// record* says which branch's search returned it. What the blocks are built from is
/// `at[].records`, which each location's own search states, and a record only the AGB
/// holds must never appear under the `BStB` heading.
#[test]
fn two_branches_show_only_their_own_hits() {
    let fetch = two_branches_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB,BSTB",
            "--limit",
            "22",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert_eq!(
        recorder.total(),
        6,
        "one session per location: open, search, filter — twice: {:?}",
        recorder.log()
    );

    let document = ran.json();
    let at = document["at"].as_array().expect("at[] is an array");
    let members = |index: usize| -> Vec<String> {
        at[index]["records"]
            .as_array()
            .unwrap_or_else(|| panic!("at[{index}].records is an array"))
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(at[0]["key"], "AGB");
    assert_eq!(at[1]["key"], "BSTB");
    let agb = members(0);
    let bstb = members(1);
    assert!(agb.contains(&"voebb_SAK15039548".to_owned()), "{agb:?}");
    assert!(!agb.contains(&"voebb_SAK99999901".to_owned()), "{agb:?}");
    assert!(bstb.contains(&"voebb_SAK99999901".to_owned()), "{bstb:?}");
    assert!(!bstb.contains(&"voebb_SAK15039548".to_owned()), "{bstb:?}");
    // The eight editions both branches hold are one record each, listed under both.
    assert!(agb.contains(&"voebb_SAK16112988".to_owned()), "{agb:?}");
    assert!(bstb.contains(&"voebb_SAK16112988".to_owned()), "{bstb:?}");

    // Every id in at[] is a record of the document — the grouping is reconstructable.
    let shown: Vec<String> = document["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .filter_map(|record| record["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(shown.len(), 10, "nine editions plus the one only BStB has");
    for id in agb.iter().chain(&bstb) {
        assert!(shown.contains(id), "{id} is in at[] but not in records[]");
    }
}

/// The same, in the terminal: one block per location, and the record only one of them
/// holds appears under that heading alone.
#[test]
fn a_branch_block_never_shows_the_other_branchs_record() {
    let fetch = two_branches_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB,BSTB",
            "--limit",
            "22",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let split = ran
        .out
        .find("BSTB (VÖBB)")
        .unwrap_or_else(|| panic!("no BStB block in\n{}", ran.out));
    let (agb_block, bstb_block) = ran.out.split_at(split);
    assert!(agb_block.contains("AGB (VÖBB)"), "{}", ran.out);
    assert!(agb_block.contains("voebb_SAK15039548"), "{agb_block}");
    assert!(!agb_block.contains("voebb_SAK99999901"), "{agb_block}");
    assert!(bstb_block.contains("voebb_SAK99999901"), "{bstb_block}");
    assert!(!bstb_block.contains("voebb_SAK15039548"), "{bstb_block}");
}

/// `--limit` is a promise per branch: two houses with `--limit 1` are one record under
/// each heading, not one record between them — and the record pages are fetched only for
/// what is actually shown.
#[test]
fn the_limit_applies_to_every_branch_on_its_own() {
    let fetch = two_branches_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB,BSTB",
            "--limit",
            "1",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let at = document["at"].as_array().expect("at[] is an array");
    let members = |index: usize| -> Vec<String> {
        at[index]["records"]
            .as_array()
            .unwrap_or_else(|| panic!("at[{index}].records is an array"))
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(members(0).len(), 1, "AGB keeps a record of its own");
    assert_eq!(members(1).len(), 1, "and so does BStB");
    // The two branches rank different records first, so the document carries both.
    assert_eq!(members(0), ["voebb_SAK15039548"]);
    assert_eq!(members(1), ["voebb_SAK99999901"]);
    assert_eq!(document["shown"], 2);
}

/// `--at AGB,HU` and `--at HU,AGB` ask the same question, so they must answer with the
/// same document: the engines run in their own fixed order and the records of the two
/// catalogues follow each other in it. Only `at[]` and the rendered blocks follow the
/// user, which is where the user's order belongs.
#[test]
fn the_engines_run_in_a_fixed_order_whatever_at_says() {
    let order = |at: &str| -> serde_json::Value {
        let fetch = both_engines_fetch();
        let ran = invoke(
            &["--json", "search", "Vorleser", "--at", at, "--limit", "2"],
            &fetch,
        );
        assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
        ran.json()
    };

    let first = order("AGB,HU");
    let second = order("HU,AGB");
    assert_eq!(first["engines"], serde_json::json!(["kobv", "voebb"]));
    assert_eq!(second["engines"], serde_json::json!(["kobv", "voebb"]));
    assert_eq!(
        first["records"], second["records"],
        "the records must not depend on the order of --at"
    );

    // at[] is the one thing that does follow the user.
    let keys = |document: &serde_json::Value| -> Vec<String> {
        document["at"]
            .as_array()
            .expect("at[] is an array")
            .iter()
            .filter_map(|block| block["key"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(keys(&first), ["AGB", "HU"]);
    assert_eq!(keys(&second), ["HU", "AGB"]);
}

/// `show` routes on the id's prefix, and the record page answers on its own: one request,
/// no session, no form state.
#[test]
fn show_fetches_one_record_page_without_a_session() {
    let fetch = voebb_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(&["show", "voebb_SAK13776205"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert_eq!(recorder.total(), 1, "{:?}", recorder.log());
    let log = recorder.log();
    assert_eq!(
        log[0],
        "GET https://www.voebb.de/aDISWeb/app/prod00?sp=SPROD00&sp=SAK13776205"
    );
    assert!(ran.out.contains("Der Vorleser"), "{}", ran.out);
    // The id is printed in full — it is the argument for the next invocation.
    assert!(ran.out.contains("voebb_SAK13776205"), "{}", ran.out);
}

/// A copy the item table cannot place keeps its house name and borrows no id.
///
/// `SAK13776205` stands in `ZLB: Außenmagazin`, and the ZLB runs **two** outlying stacks
/// (round 2, §2.6). A human reads the cell text either way; an agent reads
/// `items[].branch` and would have believed the AGB. The name survives, the id does not.
#[test]
fn a_copy_in_an_ambiguously_named_store_names_no_branch() {
    let fetch = voebb_fetch();
    let ran = invoke(&["--json", "show", "voebb_SAK13776205"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let items: Vec<&serde_json::Value> = document["record"]["holdings"]
        .as_array()
        .expect("holdings is an array")
        .iter()
        .flat_map(|holding| holding["items"].as_array().into_iter().flatten())
        .collect();
    let [item] = items.as_slice() else {
        panic!("this record has exactly one copy: {items:?}");
    };
    assert_eq!(item["branch_name"], "ZLB: Außenmagazin");
    assert_eq!(item["branch"], serde_json::Value::Null);
}

/// A record number voebb.de does not hold is answered with its search entry page. That
/// is exit 1 — the lookup worked and there is no such record — never a selector error.
#[test]
fn show_of_an_unknown_record_number_is_exit_one() {
    let fetch = voebb_fetch();
    let ran = invoke(&["show", "voebb_SAK00000000"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(ran.out.is_empty());
    assert!(ran.err.contains("no such record"), "{:?}", ran.err);
}

/// A field flag needs the advanced form, which is one extra request: open it, fill one
/// row per flag, submit it. The rows are `$Select$0` and `$Autosuggest$0`, joined `UND`.
#[test]
fn a_field_flag_goes_through_the_advanced_form() {
    let fetch = advanced_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "search",
            "--isbn",
            "978-3-257-22953-0",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let log = recorder.log();
    assert_eq!(
        log.len(),
        4,
        "entry page, advanced form, search, branch filter: {log:?}"
    );
    assert!(
        log[1].contains("$Button$0=pressed") && log[1].contains("focus=$$GFBO_3"),
        "the advanced form is opened by its own button: {}",
        log[1]
    );
    let submitted = &log[2];
    assert!(
        submitted.contains("$Select$0=ISBN, ISSN, ISMN"),
        "{submitted}"
    );
    assert!(
        submitted.contains("$Autosuggest$0=9783257229530"),
        "{submitted}"
    );
    assert!(submitted.contains("$Select$1=UND"), "{submitted}");
    assert!(
        submitted.contains("$Button$6=pressed"),
        "on the advanced form `Suchen` is $Button$6: {submitted}"
    );
    assert!(
        submitted.contains("$Select=Bibliotheksbestand"),
        "the scope is the network's holdings, never `search everywhere`: {submitted}"
    );
    assert!(ran.out.contains("AGB (VÖBB)"), "{}", ran.out);
}

/// Free terms next to a field flag have nowhere to go — voebb.de's advanced form has no
/// free-text index — so they are searched as a title and the document says so.
#[test]
fn free_terms_next_to_a_flag_are_stated_in_a_note() {
    let fetch = advanced_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--author",
            "Schlink",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        notes
            .iter()
            .any(|note| note["kind"] == "voebb_free_terms_as_title"),
        "{notes:?}"
    );
}

/// The measured result list with the AGB removed from its facet: 71 hits in the network,
/// none of them at the branch. Derived, and documented as derived in the fixture README.
fn branch_absent_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results_without_agb.html"),
        )
        .fallback("voebb/start.html")
}

/// A search that finds nothing anywhere in the network, at a branch.
fn nothing_anywhere_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results_empty.html"),
        )
        .fallback("voebb/start.html")
}

/// A branch the facet does not list has **no hits for this search**, which is an honest
/// answer and not a broken filter: the tree only carries branches that have hits. Total
/// zero, a note, the block rendered anyway — and no filter request, because there is no
/// checkbox to tick.
///
/// Here the network *does* have the book (71 hits), so `--at` is genuinely the cause and
/// the closing line may say so.
#[test]
fn a_branch_the_facet_does_not_list_is_zero_hits_with_a_note() {
    let fetch = branch_absent_fetch();
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    let document = ran.json();
    assert_eq!(document["at"][0]["total"], 0);
    assert_eq!(document["shown"], 0);
    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        notes
            .iter()
            .any(|note| note["kind"] == "voebb_branch_not_listed"),
        "{notes:?}"
    );
    assert_eq!(
        recorder.total(),
        2,
        "nothing is filtered when there is nothing to tick: {:?}",
        recorder.log()
    );
}

/// The same shape in the terminal — and the advice that goes with it: the branch is the
/// one without the book, so naming more libraries is the way out.
#[test]
fn a_branch_without_the_book_is_told_to_name_more_libraries() {
    let fetch = branch_absent_fetch();
    let ran = invoke(
        &["search", "Vorleser", "--at", "AGB", "--no-availability"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err.contains("no results for Vorleser at AGB"),
        "{}",
        ran.err
    );
    assert!(
        ran.err.contains("name more libraries in --at"),
        "with hits in the network that advice can work: {}",
        ran.err
    );
}

/// The same output for a book **nobody** in the network has would be a lie: `--at` is not
/// what emptied the result, and naming more libraries is guaranteed to fail again. The
/// two cases used to be identical character for character (round 2, §1.2).
#[test]
fn a_book_nobody_has_does_not_blame_the_branch() {
    let fetch = nothing_anywhere_fetch();
    let ran = invoke(
        &[
            "search",
            "Xylophonquark",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    // Reworded in round 2 (4a): the message used to say "not anywhere in the region",
    // which claimed more than voebb.de proved — its zero is the public library network's,
    // and the university libraries of the other engine were never asked.
    assert!(
        ran.err
            .contains("nowhere else in the catalogue that answers for it"),
        "the words are what came back empty: {}",
        ran.err
    );
    assert!(
        !ran.err.contains("name more libraries in --at"),
        "advice that is guaranteed to fail again must not be given: {}",
        ran.err
    );
    // And the note stops describing a facet the page never carried.
    assert!(
        !ran.out.contains("branch facet does not list"),
        "{}",
        ran.out
    );
}

/// The distinction an agent reads: two tags, never one message parsed two ways.
#[test]
fn the_two_empty_branch_answers_carry_two_note_kinds() {
    let kinds = |fetch: &FixtureFetch, terms: &str| -> Vec<String> {
        let ran = invoke(
            &[
                "--json",
                "search",
                terms,
                "--at",
                "AGB",
                "--no-availability",
            ],
            fetch,
        );
        ran.json()["notes"]
            .as_array()
            .expect("notes is an array")
            .iter()
            .filter_map(|note| note["kind"].as_str().map(str::to_owned))
            .collect()
    };

    let absent = branch_absent_fetch();
    assert!(
        kinds(&absent, "Vorleser").contains(&"voebb_branch_not_listed".to_owned()),
        "hits in the network, none at the branch"
    );
    let nowhere = nothing_anywhere_fetch();
    let nowhere = kinds(&nowhere, "Xylophonquark");
    assert!(
        nowhere.contains(&"voebb_no_hits_in_network".to_owned()),
        "{nowhere:?}"
    );
    assert!(
        !nowhere.contains(&"voebb_branch_not_listed".to_owned()),
        "a facet that was never on the page is never described: {nowhere:?}"
    );
}

/// Two branches, one query, one network-wide zero: the note is the query's and is said
/// once, not once per session.
#[test]
fn the_network_wide_zero_is_stated_once_for_two_branches() {
    let fetch = nothing_anywhere_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Xylophonquark",
            "--at",
            "AGB,BSTB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    let document = ran.json();
    let kinds: Vec<&str> = document["notes"]
        .as_array()
        .expect("notes is an array")
        .iter()
        .filter_map(|note| note["kind"].as_str())
        .collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "voebb_no_hits_in_network")
            .count(),
        1,
        "{kinds:?}"
    );
}

/// A query that needs five rows loses one, and what is announced is what was **sent**.
/// The two notes used to contradict each other: "`Sommer` was searched as a title" next
/// to "`Titel = "Sommer"` was not sent" (round 2, §1.13).
#[test]
fn a_dropped_row_is_never_also_announced_as_searched() {
    let fetch = advanced_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Sommer",
            "--title",
            "Vorleser",
            "--author",
            "Schlink",
            "--subject",
            "Roman",
            "--isbn",
            "9783257229530",
            "--at",
            "AGB",
            "--limit",
            "2",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let notes = document["notes"].as_array().expect("notes is an array");
    let kinds: Vec<&str> = notes
        .iter()
        .filter_map(|note| note["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"voebb_query_truncated"), "{notes:?}");
    assert!(
        !kinds.contains(&"voebb_free_terms_as_title"),
        "the row that was dropped is not also reported as sent: {notes:?}"
    );
    let truncated = notes
        .iter()
        .find(|note| note["kind"] == "voebb_query_truncated")
        .expect("the truncation note");
    assert!(
        truncated["message"]
            .as_str()
            .is_some_and(|message| message.contains("Sommer")),
        "{truncated:?}"
    );
}

/// A note about one record names it. Three `voebb_online_only` notes over two blocks are
/// guesswork otherwise, and `message` is prose an agent may not parse (round 2, §3.5).
#[test]
fn a_note_about_a_record_names_the_record() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB", "--limit", "3",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let note = document["notes"]
        .as_array()
        .expect("notes is an array")
        .iter()
        .find(|note| note["kind"] == "voebb_online_only")
        .expect("the Onleihe record says why it has no copies");
    assert_eq!(
        note["records"],
        serde_json::json!(["voebb_SAK16112988"]),
        "{note:?}"
    );
}

/// A window filter that matched **nothing** is a tag, not only a sentence. An agent used
/// to have to derive it from `filtered && fetched > 0 && after_filter == 0` (round 2,
/// §2.7).
#[test]
fn a_filter_that_matched_nothing_is_a_note_of_its_own() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--format",
            "map",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    let document = ran.json();
    assert_eq!(document["window"]["after_filter"], 0);
    assert!(document["window"]["fetched"].as_u64().unwrap_or_default() > 0);
    let note = document["notes"]
        .as_array()
        .expect("notes is an array")
        .iter()
        .find(|note| note["kind"] == "window_filter_empty")
        .expect("the empty filter has a tag of its own");
    assert!(
        note["message"]
            .as_str()
            .is_some_and(|message| message.contains("--format map")),
        "{note:?}"
    );
    // It is about the answer, not about records: the records it speaks of are the ones
    // that are not in the document.
    assert!(note.get("records").is_none(), "{note:?}");
}

/// `--sort` anchors the window, so its pages run out — and the message says so instead of
/// falling through to a generic "no results". The sorted sibling of the filtered case.
#[test]
fn a_page_past_the_end_of_a_sorted_window_says_which_pages_hold_records() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--sort",
            "year",
            "--limit",
            "10",
            "--page",
            "6",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err.contains("--sort year put in order"),
        "the sort is named, not a filter that never ran: {}",
        ran.err
    );
    assert!(ran.err.contains("page 6"), "{}", ran.err);
    assert!(
        ran.err.contains("pages 1 to 2"),
        "the 17 rows the slimmed fixture pages hold, at 10 a page: {}",
        ran.err
    );
}

/// The filtered wording is the one the report called the best line in the tool, and the
/// sorted case must not have changed a byte of it: a filter still names the filter.
#[test]
fn a_page_past_the_end_of_a_filtered_window_still_names_the_filter() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--format",
            "book",
            "--sort",
            "year",
            "--limit",
            "10",
            "--page",
            "6",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err.contains("matched --format book"),
        "with both set the filter is the more useful sentence: {}",
        ran.err
    );
    assert!(!ran.err.contains("--sort year"), "{}", ran.err);
}

/// What the human sees for the same case, until phase 4 removes the older prose line:
/// the new note and `footer_notes`' own sentence stand next to each other. Both are true,
/// and neither contradicts the other — this test exists so that the day the prose goes,
/// the note is provably still there.
#[test]
fn the_empty_filter_is_stated_to_humans_too() {
    let fetch = voebb_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--format",
            "map",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.out.contains("none of the") && ran.out.contains("--format map"),
        "{}",
        ran.out
    );
}

/// A request the stub was never taught about would silently answer the wrong fixture, so
/// the routes are checked to be distinguishable at all: the three form-driven ones differ
/// in exactly the field that drives them.
#[test]
fn the_session_steps_are_told_apart_by_their_driving_field() {
    let search = Request::post("https://www.voebb.de/app").form("$Autosuggest", "Vorleser");
    let filter = Request::post("https://www.voebb.de/app").form("$CbTree_text", "sub-1");
    let paging = Request::post("https://www.voebb.de/app").form("$Toolbar", "3");
    assert!(describe(&search).contains("$Autosuggest=Vorleser"));
    assert!(!has_field(&search, "$CbTree_text"));
    assert!(!has_field(&filter, "$Toolbar"));
    assert!(!has_field(&paging, "$CbTree_text"));
}

/// A search with no hits **anywhere in the network**, at a branch. The most ordinary
/// answer a search can have, and it used to be exit 6: voebb.de drops `div#R06 p.info`
/// entirely on such a page, `parse_results` reported the missing selector, and the user
/// was told the scraper had broken and to file a bug. Now the page's own words —
/// `p.wichtig` … "war erfolglos" — are read as the zero they are.
///
/// The branch block is still rendered: a location that holds nothing must not be missing
/// from the output, or it cannot be told apart from one that was forgotten.
#[test]
fn a_search_with_no_hits_at_all_is_no_results_and_not_a_broken_selector() {
    let fetch = FixtureFetch::new()
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results_empty.html"),
        )
        .fallback("voebb/start.html");
    let ran = invoke(
        &[
            "--json",
            "search",
            "Xqzzyplkwrmf",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(
        ran.exit(),
        ExitCode::NoResults,
        "an empty search is exit 1, never a scraper failure: {}",
        ran.err
    );
    assert!(
        !ran.err.contains("selector"),
        "the user must not be asked to report a bug for an empty result: {}",
        ran.err
    );
    let document = ran.json();
    assert_eq!(document["at"][0]["total"], 0);
    assert_eq!(document["shown"], 0);
}

/// The same page without the sentence that makes it an empty result is an unknown page,
/// and unknown still has to fail loudly. This is the direction `CLAUDE.md` fixes: a
/// missing structure may never be rendered as an empty result.
#[test]
fn a_result_page_that_states_neither_hits_nor_emptiness_still_fails() {
    let unknown = read_fixture("voebb/results_empty.html").replace("war erfolglos", "ist unklar");
    let fetch = FixtureFetch::new()
        .route(|request| has_field(request, "$Autosuggest"), unknown)
        .fallback("voebb/start.html");
    let ran = invoke(
        &["search", "Xqzzyplkwrmf", "--at", "AGB", "--no-availability"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Unexpected);
    let Err(error) = ran.outcome else {
        panic!("an unknown page must not come back as a result");
    };
    assert_eq!(error.kind(), "missing_selector");
    assert!(
        error.to_string().contains("div#R06 p.info"),
        "the message names what was missing: {error}"
    );
}

/// `show` of an e-lending title says why its copy list is empty.
///
/// `detail_online.html` has no item table at all — voebb.de states the loan state of an
/// Onleihe or Overdrive title only in the text of its access link — so the record arrives
/// with a holding and no copies. The engine has always said so in a note; `Voebb::show`
/// dropped every note it was given (round 2, package 4a), and `blibs show` then printed a
/// silently empty copy list, which CLAUDE.md forbids above all other failure modes. The
/// search path over the same record page printed the note all along.
#[test]
fn show_of_an_online_title_states_why_it_lists_no_copies() {
    let fetch = voebb_fetch();
    let ran = invoke(&["show", "voebb_SAK16112988"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    // The wording lost its "this … it" when the per-record notes were made to read
    // correctly for one record and for four folded into one (round 3, Fund 2); what the
    // assertion is about — that the empty copy list is never silent — is unchanged.
    assert!(
        ran.out.contains("has no copies on a shelf"),
        "the missing copy list needs its reason: {}",
        ran.out
    );

    let json = invoke(&["show", "voebb_SAK16112988", "--json"], &fetch).json();
    let kinds: Vec<&str> = json["notes"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .filter_map(|note| note["kind"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        kinds.contains(&"voebb_online_only"),
        "an agent branches on the kind, not on the wording: {json}"
    );
}

/// `show <id> --at <branch>` answers for the branch, exactly as the search does.
///
/// The heaviest finding of round 2 (§1.1): the same id and the same `--at` gave two
/// different answers, because `show` picked the whole holding and rendered every copy of
/// the network under it — a green light and `3 of 4 available` over a book the user's own
/// branch had lent out. `detail_on_loan.html` is that page: the AGB's copy is
/// `Ausgeliehen` while most of the network's are in.
#[test]
fn show_at_a_branch_reports_that_branchs_copy() {
    let fetch = voebb_fetch();
    let ran = invoke(&["show", "voebb_SAK13363539", "--at", "AGB"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    assert!(
        ran.out.contains("○ Berlin VÖBB/ZLB"),
        "the AGB has it out, so the light is red: {}",
        ran.out
    );
    assert!(
        ran.out.contains("L 248 Schlin 50 e"),
        "the AGB's own copy is what the block is about: {}",
        ran.out
    );
    assert_eq!(
        ran.out.matches("Erwachsenenbereich").count(),
        0,
        "no copy of another branch may be listed: {}",
        ran.out
    );
    assert!(
        ran.out
            .contains("more copies of this library stand at other branches"),
        "what the narrowing dropped is stated, never silently gone: {}",
        ran.out
    );

    let json = invoke(
        &["show", "voebb_SAK13363539", "--at", "AGB", "--json"],
        &fetch,
    )
    .json();
    assert_eq!(json["at"][0]["given"], "AGB");
    assert_eq!(
        json["at"][0]["status"], "unavailable",
        "the pipeline an agent writes must not read the house's light: {json}"
    );
}

// ---------------------------------------------------------------------------
// Round 3: one unreadable record page costs one record, never the answer
// ---------------------------------------------------------------------------

/// A record page that arrived and is **not** a detail page: the bibliographic tables are
/// gone, but `div#R03` is still there, so this is not voebb.de's "no such record" answer
/// either. It is the shape the parser cannot recover a record from at all.
fn page_without_bibliographic_tables() -> String {
    read_fixture("voebb/detail_available.html")
        .replace("<table class=\"gi\"", "<table class=\"gx\"")
}

/// The session of [`voebb_fetch`], with one record's page replaced by an unreadable one.
fn one_unreadable_page_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(
            |request| record_of(request) == "SAK01164255",
            page_without_bibliographic_tables(),
        )
        .route(is_record_page, read_fixture("voebb/detail_available.html"))
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// One page blibs cannot read costs **that record** its copies and nothing else.
///
/// Measured live 2026-09-08: `search --author "von Schirach" --at AGB` has 146 hits, ten
/// in the window, nine of them flawless — and printed
/// `error: voebb detail page: selector "table#resptable-1" matched nothing` and not one
/// of them. `fill_availability` fetches those ten pages through `scope_map`, whose
/// contract is all-or-nothing, so the first parse failure threw the whole result away.
#[test]
fn one_unreadable_record_page_never_empties_the_result() {
    let fetch = one_unreadable_page_fetch();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB", "--limit", "3",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let ids: Vec<&str> = document["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .filter_map(|record| record["id"].as_str())
        .collect();
    assert_eq!(ids.len(), 3, "every hit of the window arrives: {ids:?}");
    assert!(ids.contains(&"voebb_SAK01164255"), "{ids:?}");

    let notes = document["notes"].as_array().expect("notes is an array");
    let unreadable: Vec<&serde_json::Value> = notes
        .iter()
        .filter(|note| note["kind"] == "voebb_page_unreadable")
        .collect();
    assert_eq!(
        unreadable.len(),
        1,
        "exactly one record failed, so exactly one note: {notes:?}"
    );
    assert_eq!(
        unreadable[0]["records"],
        serde_json::json!(["voebb_SAK01164255"]),
        "the note names the record it cost: {:?}",
        unreadable[0]
    );
    assert!(
        unreadable[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("table.gi")
                && message.contains("github.com/EiSiMo/blibs/issues")),
        "the selector and where to report it are the whole point: {:?}",
        unreadable[0]
    );

    // And the other two records still carry the copies their pages stated.
    let items = document["records"][0]["holdings"][0]["items"]
        .as_array()
        .expect("items is an array");
    assert!(!items.is_empty(), "{document}");
}

/// The other half of the line: a **transport** failure still stops the invocation.
///
/// Degrading a timeout, a 429/503 or a lost voebb.de session into "status unknown" would
/// sell an outage as an answer, which is worse than any error message. The predicate that
/// decides this is `Error::is_unreadable_document`, on the error type where it is stated
/// once — never a string comparison at a call site.
#[test]
fn a_transport_failure_is_still_an_error_and_not_a_note() {
    let stub = one_unreadable_page_fetch();
    let failing = move |request: &Request| {
        if record_of(request) == "SAK01164255" {
            return Err(Error::Network(NetworkError::Timeout {
                host: "www.voebb.de".to_owned(),
                seconds: 10,
            }));
        }
        stub.fetch(request)
    };
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB", "--limit", "3",
        ],
        &failing,
    );

    assert_eq!(
        ran.exit(),
        ExitCode::Network,
        "a host that did not answer is not a record whose copies are unknown: {}",
        ran.out
    );
}

/// `show` of a record whose page cannot be read in full states why and still exits 0.
///
/// The rule is one function ([`Voebb::detail`]) for both paths, and the parser keeps the
/// record whenever the *item table* alone is unreadable — which is the shape both
/// measured failures had. So `show` prints the record, prints the note, and does not
/// abort. `CLAUDE.md` has a whole trap line about one rule with two spellings.
#[test]
fn show_of_a_page_with_an_unreadable_item_table_still_shows_the_record() {
    let broken =
        read_fixture("voebb/detail_available.html").replace("id=\"resptable-1\"", "id=\"resp-x\"");
    let fetch = FixtureFetch::new().route(is_record_page, broken);
    let ran = invoke(&["show", "voebb_SAK13776205"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert!(
        ran.out.contains("Bernhard Schlink, Der Vorleser"),
        "the record is right there on the page: {}",
        ran.out
    );
    assert!(
        ran.out.contains("resptable-1"),
        "a missing selector is never a silently empty copy list: {}",
        ran.out
    );

    let json = invoke(&["show", "voebb_SAK13776205", "--json"], &fetch).json();
    let kinds: Vec<&str> = json["notes"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .filter_map(|note| note["kind"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        kinds.contains(&"voebb_page_unreadable"),
        "an agent branches on the kind, not on the wording: {json}"
    );
}

/// The other half of the same rule: a page carrying **no readable record at all** is an
/// error in `show`, and specifically **not** "there is no such record".
///
/// Where the search path still holds the result row and can degrade to a note, `show` has
/// nothing left. Answering `record: null` there is not a neutral choice: `cli::run` turns
/// it into [`EmptyReason::NoSuchRecord`] — exit 1, "no such record", with the note dropped
/// on the human path — so the tool would assert that a record does not exist when all it
/// did was fail to read the page. Exit 6 says what actually happened, and names the
/// selector.
#[test]
fn show_of_a_page_with_no_readable_record_is_exit_six_and_not_exit_one() {
    let fetch = FixtureFetch::new().route(is_record_page, page_without_bibliographic_tables());
    let ran = invoke(&["show", "voebb_SAK13776205"], &fetch);

    assert_eq!(
        ran.exit(),
        ExitCode::Unexpected,
        "\"could not read\" is not \"does not exist\": {} {}",
        ran.out,
        ran.err
    );
    assert_ne!(
        ran.exit(),
        ExitCode::NoResults,
        "exit 1 would claim voebb.de has no such record"
    );
    // `run` hands the error back rather than rendering it, so the message is read off the
    // outcome — the same object the binary turns into its error document.
    let error = ran.outcome.expect_err("an unreadable page is not a result");
    assert_eq!(
        error.kind(),
        "missing_selector",
        "an agent branches on the kind, not on the wording: {error}"
    );
    assert!(
        error.to_string().contains("table.gi"),
        "the selector is the only thing that tells a maintainer what moved: {error}"
    );
}

// ---------------------------------------------------------------------------
// Round 3: the fourth state of the item table
// ---------------------------------------------------------------------------

/// Every displayed record answered by `voebb_SAK34364366`'s page — an electronic title
/// whose access is a bare `URL` row and which has no item table and no lending link.
fn url_only_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(is_record_page, read_fixture("voebb/detail_online_url.html"))
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// The fourth state end to end: a regular record, not a broken page.
///
/// `--at AGB` puts it in the AGB's block because voebb.de's own branch facet returned it
/// there; the page names no owning library, so the holding stays the network's, with no
/// copies and no traffic light. Nothing here claims the AGB holds it, and nothing claims
/// it is held nowhere — the note says what the page actually states.
#[test]
fn an_electronic_title_with_only_a_url_is_a_state_and_not_a_failure() {
    let fetch = url_only_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Kinderrechte",
            "--at",
            "AGB",
            "--limit",
            "1",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let holding = &document["records"][0]["holdings"][0];
    assert_eq!(
        holding["isil"], "DE-609",
        "the page names no owning library, so the holding stays the network's: {holding}"
    );
    assert_eq!(
        holding["summary"], "unknown",
        "voebb.de says nothing about this access, so neither do we: {holding}"
    );
    assert_eq!(holding["items"], serde_json::json!([]), "{holding}");
    assert_eq!(
        document["at"][0]["total"], 35,
        "the block is the facet's own count, untouched by any of this: {document}"
    );

    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        notes
            .iter()
            .any(|note| note["kind"] == "voebb_online_url_only"),
        "the fourth state has its own tag: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .all(|note| note["kind"] != "voebb_page_unreadable"),
        "it is a regular record and not a page blibs failed on: {notes:?}"
    );
}

// ---------------------------------------------------------------------------
// Round 3: one limitation, one note, every record it is about
// ---------------------------------------------------------------------------

/// Every displayed record is the same Onleihe title, so all three notes say the same
/// thing about three different records.
fn three_online_records_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(
            |request| has_field(request, "$CbTree_text"),
            read_fixture("voebb/results_filtered.html"),
        )
        .route(is_record_page, read_fixture("voebb/detail_online.html"))
        .route(
            |request| has_field(request, "$Autosuggest"),
            read_fixture("voebb/results.html"),
        )
        .fallback("voebb/start.html")
}

/// Notes that say the same thing are one note naming every record.
///
/// Measured 2026-09-08: `search --author Kafka --at AGB` printed the same
/// `voebb_online_state_unstated` paragraph four times, with nothing beside it to say
/// which four of the ten displayed lines were meant.
#[test]
fn one_limitation_over_three_records_is_one_note_naming_all_three() {
    let fetch = three_online_records_fetch();
    let ran = invoke(
        &[
            "--json", "search", "Vorleser", "--at", "AGB", "--limit", "3",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let online: Vec<&serde_json::Value> = document["notes"]
        .as_array()
        .expect("notes is an array")
        .iter()
        .filter(|note| note["kind"] == "voebb_online_only")
        .collect();
    assert_eq!(
        online.len(),
        1,
        "three records, one statement, one note: {online:?}"
    );
    assert_eq!(
        online[0]["records"].as_array().map(Vec::len),
        Some(3),
        "and it names all three: {:?}",
        online[0]
    );
    let message = online[0]["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("this is") && !message.contains("voebb_SAK"),
        "the message has to read correctly for one record and for three, and the records \
         are the renderer's own line: {message:?}"
    );

    // The human output prints the paragraph once, not three times.
    let human = invoke(
        &["search", "Vorleser", "--at", "AGB", "--limit", "3"],
        &fetch,
    );
    assert_eq!(
        human.out.matches("has no copies on a shelf").count()
            + human.err.matches("has no copies on a shelf").count(),
        1,
        "out:\n{}\nerr:\n{}",
        human.out,
        human.err
    );
}
