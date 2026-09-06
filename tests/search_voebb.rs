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
use blibs::error::{Error, ExitCode, Outcome};
use blibs::http::Request;
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

/// The KOBV side, so that a mixed `--at` can be run in one invocation.
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

/// Both catalogues behind one stub, the way one invocation sees them.
fn both_engines_fetch() -> FixtureFetch {
    FixtureFetch::new()
        .route(is_count, read_fixture("kobv/sru/count.xml"))
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
    assert_eq!(at[1]["total"], 3718);
    // The KOBV search's total stays the document's; the voebb count lives in at[].
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

/// A branch the facet does not list has **no hits for this search**, which is an honest
/// answer and not a broken filter: the tree only carries branches that have hits. Total
/// zero, a note, the block rendered anyway — and no filter request, because there is no
/// checkbox to tick.
#[test]
fn a_branch_the_facet_does_not_list_is_zero_hits_with_a_note() {
    // The measured tree with the AGB taken out of it: the same page a search would
    // produce for a work the AGB does not hold.
    let without_agb =
        read_fixture("voebb/results.html").replace("ZLB: Amerika-Gedenkbibliothek (AGB)", "ZLB");
    let fetch = FixtureFetch::new()
        .route(|request| has_field(request, "$Autosuggest"), without_agb)
        .fallback("voebb/start.html");
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
