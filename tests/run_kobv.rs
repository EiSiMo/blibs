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
use std::fmt::Write as _;

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

/// Like [`search_fetch`], but the third record — `almahu_BV048243266` — is answered with
/// a fixture of its own.
///
/// The availability call carries the record's `ISIL;local-id` pairs in `availability_id`
/// (`src/engine/kobv/client.rs`), so a route on that parameter answers *per record*. That
/// is the only way an end-to-end test can give one record a green light and another a
/// reference copy; the specific route has to precede the catch-all, which [`FixtureFetch`]
/// resolves first-match-wins.
fn search_fetch_answering_third(fixture: &str) -> FixtureFetch {
    FixtureFetch::new()
        .on_param("availability_id", "BV048243266", fixture)
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/filtered.xml")
}

/// The ids of the records the JSON document shows, in order.
fn record_ids(document: &serde_json::Value) -> Vec<String> {
    document["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .map(|record| record["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Whether the document carries a note of this kind. Agents switch on `kind`, never on
/// the message, so the test does too.
fn has_note(document: &serde_json::Value, kind: &str) -> bool {
    document["notes"]
        .as_array()
        .is_some_and(|notes| notes.iter().any(|note| note["kind"] == kind))
}

/// A key two entries of the library list claim, found by counting claims rather than by
/// naming a code, and restricted to the ones a KOBV house answers for — the fixtures here
/// are SRU answers, and a collision that resolved to a VÖBB branch would need the other
/// engine's.
///
/// Panics when the list carries no such key: this test is about a limitation of the data,
/// and silently passing when the data no longer has it would leave a green suite proving
/// nothing.
fn ambiguous_kobv_key() -> String {
    blibs::libraries::all()
        .iter()
        .flat_map(|library| {
            std::iter::once(library.isil.clone()).chain(
                library
                    .branches
                    .iter()
                    .filter_map(|branch| branch.isil.clone()),
            )
        })
        .find(|isil| {
            !blibs::libraries::shadowed_by_key(isil).is_empty()
                && blibs::libraries::resolve(isil)
                    .is_ok_and(|location| location.engine == blibs::model::Engine::Kobv)
        })
        .expect("the library list has to carry a key two entries claim, answered by kobv")
}

/// One code, two libraries, and the search answers for the first of them — which it now
/// says, in both outputs.
///
/// Round 2, §1.4: the user types the ISIL this tool prints in a branch's own detail view,
/// and gets a confident answer about a house they never named, with nothing anywhere
/// connecting the two. The key is derived from the list rather than written here, so the
/// test keeps meaning something if the collision moves.
#[test]
fn a_key_two_libraries_claim_says_who_answered_and_what_to_type() {
    let key = ambiguous_kobv_key();
    let missed: Vec<String> = blibs::libraries::shadowed_by_key(&key)
        .into_iter()
        .map(|entry| blibs::libraries::entry_location(entry).key)
        .collect();
    assert!(!missed.is_empty(), "a shared key shadows something");

    let json = invoke(
        &["--json", "search", "Vorleser", "--at", &key, "--limit", "2"],
        &search_fetch(),
    )
    .json();
    assert!(
        has_note(&json, "location_key_ambiguous"),
        "the shared key has to be stated: {:?}",
        json["notes"]
    );

    let ran = invoke(
        &["search", "Vorleser", "--at", &key, "--limit", "2"],
        &search_fetch(),
    );
    let said = format!("{}{}", ran.out, ran.err);
    assert!(said.contains(&format!("--at {key}")), "{said}");
    for other in &missed {
        assert!(
            said.contains(&format!("--at {other}")),
            "the note has to name what to type instead: {said}"
        );
    }
}

/// And a key that names one place says nothing extra — the note fires off the user's own
/// word, not off the library it landed on. `HU` is the same house `DE-11` names, and the
/// wholesale proof that no alias and no KOBV id can collide is in `tests/libraries.rs`.
#[test]
fn an_unambiguous_key_says_nothing_about_sharing() {
    for key in ["HU", "DE-11"] {
        let json = invoke(
            &["--json", "search", "Vorleser", "--at", key, "--limit", "2"],
            &search_fetch(),
        )
        .json();
        assert!(
            !has_note(&json, "location_key_ambiguous"),
            "--at {key} names one place: {:?}",
            json["notes"]
        );
    }
}

/// `--available` **thins** the page, it does not refill it: the records that are in stay,
/// the others go, and the count of records the filter judged is reported so that "three
/// of ten" can never read as "three hits". A reference copy is not available — taking a
/// `Präsenzbestand` home is exactly what the catalogue forbids.
#[test]
fn the_available_filter_thins_the_page_and_says_by_how_much() {
    let fetch = search_fetch_answering_third("kobv/availability/reference.json");
    let ran = invoke(&["--json", "search", "Vorleser", "--available"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    // Two of the three fetched records are borrowable; the third is reference only.
    assert_eq!(document["shown"], 2);
    assert_eq!(document["window"]["before_available"], 3);
    assert_eq!(
        record_ids(&document),
        vec!["almahu_9950092449202882", "almahu_9950168055502882"],
        "the reference-only record has to be the one that went: {document}"
    );
    // The catalogue's own hit count is untouched — the filter never saw those records.
    assert_eq!(document["total"], 230);
    // A reference copy is a statement, so nothing here was merely unanswered.
    assert!(
        !has_note(&document, "availability_filter_unstated"),
        "the service said \"reference\", which is an answer: {document}"
    );
}

/// The number in a block heading is that location's **catalogue** hit count and stays
/// put; only `at[].records` shrinks. The filter knows nothing about the records it never
/// fetched, so letting `total` follow it would claim the FU holds six books when it holds
/// 230 of which none is in.
#[test]
fn a_location_keeps_its_true_total_when_the_filter_empties_its_block() {
    let fetch = search_fetch();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Vorleser",
            "--at",
            "HU,FU",
            "--available",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    let at = document["at"].as_array().expect("at[] is an array");
    assert_eq!(at[0]["key"], "HU");
    assert_eq!(at[0]["total"], 230);
    assert_eq!(
        at[0]["records"]
            .as_array()
            .expect("at[].records is an array")
            .len(),
        3,
        "the HU has a green light for all three: {document}"
    );
    assert_eq!(at[1]["key"], "FU");
    // The FU's copies are reference only — its block loses every record and keeps its
    // total.
    assert_eq!(at[1]["total"], 230);
    assert_eq!(
        at[1]["records"]
            .as_array()
            .expect("at[].records is an array")
            .len(),
        0,
        "{document}"
    );
    // A record still shown under one heading was not hidden, so the page is not thinned.
    assert_eq!(document["shown"], 3);
    assert_eq!(document["window"]["before_available"], 3);

    let fetch = search_fetch();
    let ran = invoke(
        &["search", "Vorleser", "--at", "HU,FU", "--available"],
        &fetch,
    );
    assert!(
        ran.out
            .contains("FU Berlin · 230 results · none available now"),
        "an emptied block says how many results it really has: {}",
        ran.out
    );
}

/// A page on which nothing is available is exit 1 like every empty result, but it is
/// **not** "nothing found" and not "nobody holds it": both of those send the user off in
/// the wrong direction. The reason has its own text, and it names the next step that can
/// actually help — a larger window or another page.
#[test]
fn a_page_with_nothing_available_is_its_own_kind_of_empty() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/reference.json"),
        )
        .fallback("kobv/sru/filtered.xml");
    let ran = invoke(&["search", "Vorleser", "--available"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.out.is_empty(),
        "an empty result renders no list: {:?}",
        ran.out
    );
    assert!(
        ran.err
            .contains("230 results, but none of the 3 records on this page is in right now"),
        "stderr was {:?}",
        ran.err
    );
    assert!(
        ran.err
            .contains("try a larger --limit, another --page, or drop --available"),
        "the reason has to name a next step: {:?}",
        ran.err
    );
    assert!(
        !ran.err.contains("try fewer or more general words"),
        "hits existed — this is not NoHits: {:?}",
        ran.err
    );
    assert!(
        !ran.err.contains("none of them is held at"),
        "the records are held; they are just out — this is not NoHoldings: {:?}",
        ran.err
    );
    // Every one of the three was answered with "reference", so nothing was left unsaid.
    assert!(!ran.err.contains("state no status at all"), "{:?}", ran.err);
}

/// The difference between *"it is out"* and *"nothing was said about it"* must survive
/// the filter. A record the service made no statement about is hidden like an on-loan
/// one, and the note is the only thing that keeps that from reading as a refusal.
#[test]
fn a_record_without_a_status_statement_is_hidden_and_noted() {
    let fetch = search_fetch_answering_third("kobv/availability/public.json");
    let ran = invoke(&["--json", "search", "Vorleser", "--available"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    assert_eq!(document["shown"], 2);
    assert_eq!(document["window"]["before_available"], 3);
    assert!(
        !record_ids(&document).contains(&"almahu_BV048243266".to_owned()),
        "a black traffic light is not a green one: {document}"
    );
    assert!(
        has_note(&document, "availability_filter_unstated"),
        "nothing was stated about the hidden record, and the document has to say so: \
         {document}"
    );
}

/// Without the flag nothing about the document changes — same hits, and no
/// `before_available`, whose very absence is how a reader tells that no availability
/// filter ran. The committed schema snapshot depends on it.
#[test]
fn without_the_flag_the_document_is_the_one_it_always_was() {
    let fetch = search_fetch_answering_third("kobv/availability/reference.json");
    let ran = invoke(&["--json", "search", "Vorleser"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    assert_eq!(document["shown"], 3);
    assert_eq!(document["total"], 230);
    assert_eq!(
        document["window"]["before_available"],
        serde_json::Value::Null,
        "the member is absent unless the filter ran: {document}"
    );
    assert!(
        !has_note(&document, "availability_filter_unstated"),
        "{document}"
    );
}

/// `--available` filters records that were already fetched, so it costs the services
/// nothing: the same search and the same one availability call per displayed record,
/// flag or no flag. A filter that refilled the page would ask about records nobody sees,
/// which is what `CLAUDE.md` § *Upstream etiquette* forbids.
#[test]
fn the_available_filter_costs_no_extra_request() {
    let plain = search_fetch_answering_third("kobv/availability/reference.json");
    let plain_recorder = plain.recorder();
    let ran = invoke(&["search", "Vorleser"], &plain);
    assert_eq!(ran.exit(), ExitCode::Success);

    let filtered = search_fetch_answering_third("kobv/availability/reference.json");
    let filtered_recorder = filtered.recorder();
    let ran = invoke(&["search", "Vorleser", "--available"], &filtered);
    assert_eq!(ran.exit(), ExitCode::Success);

    assert_eq!(
        plain_recorder.count_matching("AJAX/JSON"),
        3,
        "{:?}",
        plain_recorder.log()
    );
    assert_eq!(
        filtered_recorder.count_matching("AJAX/JSON"),
        plain_recorder.count_matching("AJAX/JSON"),
        "the filter runs after the fetching, so it may not add a call: {:?}",
        filtered_recorder.log()
    );
    assert_eq!(
        filtered_recorder.total(),
        plain_recorder.total(),
        "one search either way: {:?}",
        filtered_recorder.log()
    );
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
        ran.err.contains("no results for Zzzzz at HU"),
        "{:?}",
        ran.err
    );
    assert!(
        !ran.err.contains("hits exist"),
        "nothing was found, so nothing exists to be held: {:?}",
        ran.err
    );
    // The restriction is what to loosen. Weakening the words instead is the advice the
    // unrestricted search gets, and it would send this user to spoil a good query.
    assert!(
        ran.err.contains("drop it to ask the whole region"),
        "{:?}",
        ran.err
    );
    assert!(!ran.err.contains("more general words"), "{:?}", ran.err);
    // `--at` filters upstream, so nothing here ever looked outside HU.
    for claim in ["elsewhere", "other libraries"] {
        assert!(!ran.err.contains(claim), "{claim:?} claimed: {:?}", ran.err);
    }
}

/// Nothing at all upstream is the other exit 1, and it reads differently.
#[test]
fn an_empty_catalogue_answer_is_no_hits() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/empty.xml");
    let ran = invoke(&["search", "Zzzzz"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(ran.err.contains("no results for Zzzzz"), "{:?}", ran.err);
    assert!(ran.err.contains("more general words"), "{:?}", ran.err);
    assert!(
        !ran.err.contains("--at"),
        "no --at was given: {:?}",
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

/// `show --json` is a hull around the record, not the bare record: an agent reads
/// `record`, and beside it the two things a bare record cannot state — whether copies
/// were asked for, and what could not be said.
#[test]
fn the_show_document_is_a_hull_around_the_record() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .fallback("kobv/sru/record.xml");
    let ran = invoke(&["--json", "show", "almafu_BV008885798"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    let document = ran.json();
    let members: Vec<&String> = document
        .as_object()
        .expect("the show document is an object")
        .keys()
        .collect();
    assert_eq!(members, vec!["record", "availability"]);
    assert_eq!(document["record"]["title"], "Augenblick und Irritation");
    assert_eq!(document["availability"], "fetched");
}

/// `--no-availability` is visible in the document. Without it an empty `items[]` means
/// two different things — "asked and got nothing" and "never asked" — which is exactly
/// what the mode exists to tell apart.
#[test]
fn a_show_that_asked_for_no_copies_says_so() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/record.xml");
    let recorder = fetch.recorder();
    let ran = invoke(
        &["--json", "show", "almafu_BV008885798", "--no-availability"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success, "{:?}", ran.err);
    assert_eq!(recorder.count_matching("AJAX/JSON"), 0);
    let document = ran.json();
    assert_eq!(document["availability"], "skipped");
    let items: Vec<&serde_json::Value> = document["record"]["holdings"]
        .as_array()
        .expect("holdings is an array")
        .iter()
        .flat_map(|holding| holding["items"].as_array().into_iter().flatten())
        .collect();
    assert!(items.is_empty(), "nothing was asked: {items:?}");
}

/// A VÖBB branch in `--at` against a KOBV record that the branch's network does not hold
/// is a contradiction the tool used to swallow. It is a note now — in the document and on
/// the terminal — and never a claim that the branch does not hold the book.
#[test]
fn a_branch_of_the_other_catalogue_in_at_is_stated() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let ran = invoke(
        &[
            "--json",
            "show",
            "b3kat_BV044513648",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    let document = ran.json();
    let notes = document["notes"].as_array().expect("notes is an array");
    assert!(
        notes
            .iter()
            .any(|note| note["kind"] == "location_other_catalogue"),
        "the branch cannot narrow a KOBV record, and that is said: {notes:?}"
    );

    let fetch = FixtureFetch::new().fallback("kobv/sru/mono_kafka.xml");
    let ran = invoke(
        &[
            "show",
            "b3kat_BV044513648",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );
    assert!(
        ran.out
            .contains("--at AGB is answered by the voebb catalogue"),
        "{}",
        ran.out
    );
}

/// The other half of the same rule (§2.8 of the second test round): where the location's
/// ISIL *does* occur in the record, the location applied, the holding is marked `mine`,
/// and the note must stay silent — it used to fire three lines under a `mine: true` that
/// said the opposite.
#[test]
fn a_record_the_branch_network_does_hold_gets_no_such_note() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/record.xml");
    let ran = invoke(
        &[
            "--json",
            "show",
            "almafu_BV008885798",
            "--at",
            "AGB",
            "--no-availability",
        ],
        &fetch,
    );

    let document = ran.json();
    let notes = document["notes"]
        .as_array()
        .map_or_else(Vec::new, Clone::clone);
    assert!(
        !notes
            .iter()
            .any(|note| note["kind"] == "location_other_catalogue"),
        "the record is held at DE-609, so the location applied: {notes:?}"
    );
}

/// An id no catalogue holds is exit 1, not an error: the lookup worked.
#[test]
fn show_of_an_unknown_id_is_exit_one() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/unknown_id.xml");
    let ran = invoke(&["show", "almafu_BV000000000"], &fetch);

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(ran.out.is_empty());
    assert!(ran.err.contains("no such record"), "{:?}", ran.err);

    // In JSON that is the hull with `record: null`: a well-formed answer meaning "no
    // record", never an error object — and it still says whether copies were asked for,
    // so an empty answer is as readable as a full one.
    let fetch = FixtureFetch::new().fallback("kobv/sru/unknown_id.xml");
    let ran = invoke(&["--json", "show", "almafu_BV000000000"], &fetch);
    assert_eq!(ran.exit(), ExitCode::NoResults);
    let document = ran.json();
    assert_eq!(document["record"], serde_json::Value::Null);
    assert_eq!(document["availability"], "fetched");
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

/// `--page 5000` is refused by the catalogue as `1/61`, and the only thing that helps is
/// a smaller `--page`. The generic advice — "try simpler search words" — sends the user
/// to rewrite a query that was never the problem.
#[test]
fn a_window_past_the_last_result_says_to_lower_the_page() {
    let fetch = FixtureFetch::new().fallback("kobv/sru/out_of_range.xml");
    let ran = invoke(
        &["search", "Kafka", "--limit", "5", "--page", "5000"],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Rejected);
    assert!(ran.out.is_empty());
    let Err(error) = ran.outcome else {
        panic!("a refused window is an error, not an empty result");
    };
    assert_eq!(error.kind(), "query_rejected");
    assert!(
        error
            .to_string()
            .contains("First record position out of range"),
        "{error}"
    );
    let hint = error.hint().unwrap_or_default();
    assert!(hint.contains("lower --page"), "{hint}");
    assert!(!hint.contains("simpler search words"), "{hint}");
}

/// The `filtered.xml` envelope with its first record delivered a second time — what
/// `sru.kobv.de/k2` was measured doing on 2026-09-07 for `"Der Vorleser"`, where one id
/// came back at two positions of a single five-record response.
fn body_with_a_repeated_record() -> String {
    let body = read_fixture("kobv/sru/filtered.xml");
    let start = body
        .find("<zs:record>")
        .expect("the envelope has a first record");
    let tail = "</zs:recordPosition></zs:record>";
    let end = body.find(tail).expect("the first record ends") + tail.len();
    format!("{}{}{}", &body[..end], &body[start..end], &body[end..])
}

/// How many availability requests went out.
fn availability_calls(recorder: &Recorder) -> usize {
    recorder
        .log()
        .iter()
        .filter(|line| line.contains("AJAX/JSON"))
        .count()
}

/// The catalogue repeats a record inside one window; `records[]` promises each record
/// exactly once, so the repeat is dropped — and it must not cost a second availability
/// request for a status already known.
#[test]
fn a_record_the_catalogue_repeats_is_dropped_and_asked_about_once() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .route(|_| true, body_with_a_repeated_record());
    let recorder = fetch.recorder();
    let ran = invoke(&["search", "Vorleser", "--json"], &fetch);

    assert_eq!(ran.exit(), ExitCode::Success);
    let document = ran.json();
    let ids: Vec<&str> = document["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .map(|record| record["id"].as_str().expect("every record has an id"))
        .collect();
    let mut distinct = ids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        ids.len(),
        distinct.len(),
        "a record was listed twice: {ids:?}"
    );

    assert_eq!(
        availability_calls(&recorder),
        ids.len(),
        "one availability request per displayed record, no more: {:?}",
        recorder.log()
    );

    let kinds: Vec<&str> = document["notes"]
        .as_array()
        .expect("the dropped repeat is stated")
        .iter()
        .map(|note| note["kind"].as_str().expect("every note has a kind"))
        .collect();
    assert!(
        kinds.contains(&"duplicate_records_dropped"),
        "dropping a record silently would contradict the hit count: {kinds:?}"
    );
}

/// `window.fetched` counts what the window holds. After a repeat is dropped it holds one
/// record fewer — otherwise `after_filter < fetched` would be true with no filter set and
/// the footer would blame filters that never ran.
#[test]
fn dropping_a_repeat_does_not_read_as_a_filter_having_run() {
    let fetch = FixtureFetch::new()
        .route(
            is_availability,
            read_fixture("kobv/availability/mixed.json"),
        )
        .route(|_| true, body_with_a_repeated_record());
    let ran = invoke(&["search", "Vorleser", "--json"], &fetch);
    let window = &ran.json()["window"];

    assert_eq!(
        window["fetched"], window["after_filter"],
        "no filter ran, so the window must not look thinned: {window}"
    );
    assert!(
        !ran.out.contains("the filters saw"),
        "no filter ran: {}",
        ran.out
    );
}

/// The catalogue does not order a result stably — proven against `sru.kobv.de/k2` with
/// three identical requests. Page one cannot show the effect; from page two on the user
/// has to be told, because `--page` can then overlap or skip.
#[test]
fn paging_past_the_first_page_states_that_the_order_is_unstable() {
    let kinds = |args: &[&str]| -> Vec<String> {
        let fetch = search_fetch();
        let ran = invoke(args, &fetch);
        ran.json()["notes"]
            .as_array()
            .map(|notes| {
                notes
                    .iter()
                    .filter_map(|note| note["kind"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };

    assert!(
        !kinds(&["search", "Vorleser", "--json", "--page", "1"])
            .iter()
            .any(|kind| kind == "result_order_unstable"),
        "one page cannot overlap itself"
    );
    assert!(
        kinds(&["search", "Vorleser", "--json", "--page", "2"])
            .iter()
            .any(|kind| kind == "result_order_unstable"),
        "from page two on the overlap is possible and has to be stated"
    );
}

/// Paging a client-side filter used to lose records: the window widened to 50 raw
/// records, page 1 showed `--limit` of the matches inside it, and page 2 jumped to raw
/// record 51 — every match between the two was reachable from no page at all, under a
/// heading that claimed to continue the count. Now the window is anchored and `--page`
/// walks the matches inside it, so the pages partition them.
#[test]
fn paging_a_filtered_result_neither_repeats_nor_loses_a_record() {
    let page = |number: &str| -> Vec<String> {
        let fetch = search_fetch();
        let ran = invoke(
            &[
                "search",
                "Vorleser",
                "--language",
                "eng",
                "--limit",
                "1",
                "--page",
                number,
                "--no-availability",
                "--json",
            ],
            &fetch,
        );
        ran.json()["records"]
            .as_array()
            .expect("records is an array")
            .iter()
            .map(|record| record["id"].as_str().expect("an id").to_owned())
            .collect()
    };

    let walked: Vec<String> = ["1", "2", "3"].iter().flat_map(|n| page(n)).collect();
    let mut distinct = walked.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        walked.len(),
        distinct.len(),
        "the pages overlap: {walked:?}"
    );
    assert_eq!(
        distinct.len(),
        3,
        "every match in the window has to sit on some page: {walked:?}"
    );
}

/// The window holds three matches and a page of one, so page four is past the last of
/// them. That is not "nothing matched" — records did — and the advice must not be to
/// narrow a search that is working.
#[test]
fn a_page_past_the_last_match_says_so_instead_of_blaming_the_filter() {
    let fetch = search_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--language",
            "eng",
            "--limit",
            "1",
            "--page",
            "4",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::NoResults);
    assert!(
        ran.err.contains("after the last of them"),
        "the page is past the matches, not empty of them: {}",
        ran.err
    );
    assert!(
        ran.err.contains("pages 1 to 3"),
        "the message says where the matches actually are: {}",
        ran.err
    );
    assert!(
        !ran.err.contains("narrow the search"),
        "narrowing a working search is the wrong advice here: {}",
        ran.err
    );
}

/// A range over `total` would be a lie once a filter is active: the window is one
/// anchored block, and no page reaches the rest of `total` at all. The heading counts
/// what it is really ranging over.
#[test]
fn a_filtered_heading_counts_the_window_instead_of_ranging_over_the_total() {
    let fetch = search_fetch();
    let ran = invoke(
        &[
            "search",
            "Vorleser",
            "--language",
            "eng",
            "--limit",
            "2",
            "--no-availability",
        ],
        &fetch,
    );

    let heading = ran.out.lines().next().unwrap_or_default();
    assert!(
        heading.contains("2 of 3 matching in this window"),
        "the heading has to say what it ranges over: {heading}"
    );
    assert!(
        !heading.contains("showing"),
        "a range implies a completeness the filter cannot have: {heading}"
    );
}

/// An SRU envelope of `count` records, in which the record at 0-based position `repeat`
/// is a second copy of the first one — exactly the shape the union catalogue delivers.
///
/// Hand-built rather than a fixture file, because the point of the test is the *number*
/// of records in the window, and a fixture would fix that number in a file whose name
/// says nothing about it.
fn window_with_a_duplicate(count: usize, repeat: usize) -> String {
    let mut body = String::from(
        r#"<?xml version="1.0"?><zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse"><zs:numberOfRecords>13410</zs:numberOfRecords><zs:records>"#,
    );
    for position in 0..count {
        let number = if position == repeat { 0 } else { position };
        let _ = write!(
            body,
            r#"<zs:record><zs:recordSchema>marcxml</zs:recordSchema><zs:recordXMLEscaping>xml</zs:recordXMLEscaping><zs:recordData><record xmlns="http://www.loc.gov/MARC21/slim">
<leader>00000nam a2200000 c 4500</leader>
<controlfield tag="001">almahu_{number:04}</controlfield>
<controlfield tag="008">180205t20172017gw            000 0 ger d</controlfield>
<datafield tag="245" ind1="1" ind2="0"><subfield code="a">Titel {number}</subfield></datafield>
</record></zs:recordData><zs:recordPosition>{}</zs:recordPosition></zs:record>"#,
            position + 1
        );
    }
    body.push_str("</zs:records></zs:searchRetrieveResponse>");
    body
}

/// §1.6: `--limit 10` returns ten records even when the catalogue puts the same record
/// in the window twice.
///
/// Two things have to hold together for that, and this test fails if either is dropped:
/// the SRU request asks for `--limit` **plus the overdraw**, and the page is cut to
/// `--limit` only after the duplicate has been removed. With a window of exactly ten
/// there is nothing to move up and the answer is nine — measured against the live
/// service in 4 of 13 windows.
#[test]
fn a_duplicate_in_the_window_does_not_shorten_the_page() {
    let fetch = FixtureFetch::new().route(|_| true, window_with_a_duplicate(15, 1));
    let recorder = fetch.recorder();
    let ran = invoke(
        &[
            "--json",
            "search",
            "Kafka",
            "--limit",
            "10",
            "--no-availability",
        ],
        &fetch,
    );

    assert_eq!(ran.exit(), ExitCode::Success);
    let json = ran.json();
    assert_eq!(json["shown"], 10, "{}", ran.out);
    assert_eq!(
        json["records"].as_array().map(Vec::len),
        Some(10),
        "{}",
        ran.out
    );
    // The repeat is gone rather than renamed: ten records, ten distinct ids.
    let mut ids: Vec<&str> = json["records"]
        .as_array()
        .expect("records is a list")
        .iter()
        .map(|record| record["id"].as_str().expect("an id is a string"))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 10, "{}", ran.out);
    // And it is stated, because the window did hold a duplicate.
    let kinds: Vec<&str> = json["notes"]
        .as_array()
        .expect("notes is a list")
        .iter()
        .map(|note| note["kind"].as_str().expect("a kind is a string"))
        .collect();
    assert!(kinds.contains(&"duplicate_records_dropped"), "{kinds:?}");
    // The buffer is the reason it worked: the window that went out was 15 wide.
    assert_eq!(
        recorder.count_matching("maximumRecords=15"),
        1,
        "{:?}",
        recorder.log()
    );
}
