//! Black-box tests of the command line. Network-free by construction: everything here
//! must be answered before a request would be built.
//!
//! Two properties are asserted over and over, because they are the whole agent contract:
//! **the exit code** (2 is "you asked wrongly and nothing was sent", 1 is "asked and found
//! nothing") and **which stream carried what** — a result on stdout, an explanation on
//! stderr, and under `--json` nothing on stdout that is not a document.

mod common;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;

fn blibs() -> Command {
    Command::cargo_bin("blibs").expect("the binary must be built")
}

/// `blibs` on its own prints the long help and exits 0 — an empty invocation practically
/// always means "show me the documentation", not "I made a mistake".
#[test]
fn bare_invocation_prints_long_help() {
    blibs()
        .assert()
        .success()
        .stdout(contains("Usage: blibs"))
        .stdout(contains("libraries"))
        // The exit codes are part of the public interface, and `--help` is where an agent
        // has to be able to find them.
        .stdout(contains("2  usage error"));
}

/// `--help` is a request, not an error.
#[test]
fn help_exits_zero() {
    blibs().arg("--help").assert().success();
}

/// `help` is a subcommand like `--help` is a flag — both are requests, not errors, and
/// both must work: an agent that types `blibs help` (or `blibs help search`) rather than
/// `--help` must not be met with a usage error.
#[test]
fn help_subcommand_exits_zero() {
    blibs()
        .arg("help")
        .assert()
        .success()
        .stdout(contains("Usage: blibs"))
        .stdout(contains("libraries"));

    blibs()
        .args(["help", "search"])
        .assert()
        .success()
        .stdout(contains("Usage: blibs search"));
}

/// An unknown subcommand is a usage error, exit 2, and it points at the *top-level* help.
#[test]
fn unknown_subcommand_exits_two() {
    blibs()
        .arg("serch")
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("Usage: blibs [OPTIONS] [COMMAND]"))
        // clap's suggestion is the whole point of exit 2 here: it names the next step.
        .stderr(contains("search"));
}

/// An unknown flag inside a subcommand is a usage error too, and the message shows that
/// subcommand's help rather than the top-level one.
#[test]
fn unknown_flag_exits_two() {
    blibs()
        .args(["search", "Kafka", "--nope"])
        .assert()
        .code(2)
        .stderr(contains("blibs search"));
}

/// A search with nothing to search for is caught here, not upstream: empty is diagnostic
/// 1/10 there, which would surface as exit 5 for a plain usage mistake.
#[test]
fn a_search_without_terms_is_a_usage_error() {
    blibs()
        .arg("search")
        .assert()
        .code(2)
        .stderr(contains("empty query"));

    blibs()
        .args(["--json", "search"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("\"kind\": \"empty_query\""));
}

/// Truncation does not exist in this catalogue; a wildcard is refused instead of sent.
#[test]
fn a_wildcard_is_refused_before_anything_is_sent() {
    blibs()
        .args(["search", "Proze*"])
        .assert()
        .code(2)
        .stderr(contains("wildcards are not supported"));
}

/// A year range comes back as *zero hits, silently*, which is why it never goes out.
#[test]
fn a_year_range_is_refused() {
    blibs()
        .args(["search", "Kafka", "--year", "1990-2000"])
        .assert()
        .code(2)
        .stderr(contains("ranges are not supported"));
}

/// The identifier index upstream discards the ISBN check digit, so a typo returns *the
/// wrong book*. The message names the digit that was expected — without it the user has
/// no way to tell which of thirteen digits they mistyped.
#[test]
fn a_wrong_isbn_check_digit_names_the_expected_digit() {
    blibs()
        .args(["search", "--isbn", "978-3-596-29433-4"])
        .assert()
        .code(2)
        .stderr(contains("invalid ISBN"))
        .stderr(contains("6"));
}

/// An unknown library in `--at` is a usage error — the search *must* have that library,
/// so a typo cannot be answered with "no hits". Under `--json` the envelope goes to
/// stderr and stdout stays empty, so a pipeline never parses half a document.
#[test]
fn an_unknown_library_is_a_usage_error() {
    blibs()
        .args(["search", "Kafka", "--at", "NOPE"])
        .assert()
        .code(2)
        .stderr(contains("unknown library"));

    blibs()
        .args(["--json", "search", "Kafka", "--at", "NOPE"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("\"kind\": \"unknown_library\""));
}

/// `--limit` is never clamped: the catalogue truncates at 50 without saying so, and a
/// silently shortened answer is the failure mode this tool exists to avoid.
#[test]
fn a_limit_above_fifty_is_refused_rather_than_clamped() {
    blibs()
        .args(["search", "Kafka", "--limit", "99"])
        .assert()
        .code(2)
        .stderr(contains("--limit must be between 1 and 50"));
}

/// A VÖBB branch routes to the second engine, and voebb.de's advanced search has no
/// index for a publisher. A flag the answering catalogue cannot honour is refused before
/// anything is sent — a search that silently dropped `--publisher` would answer a
/// question nobody asked.
#[test]
fn a_flag_the_voebb_engine_has_no_index_for_is_refused() {
    blibs()
        .args([
            "search",
            "Vorleser",
            "--at",
            "AGB",
            "--publisher",
            "Diogenes",
        ])
        .assert()
        .code(2)
        .stderr(contains("voebb"));

    blibs()
        .args(["search", "Vorleser", "--at", "AGB", "--year", "1997"])
        .assert()
        .code(2)
        .stderr(contains("voebb"));
}

/// A record id without its source prefix cannot pick a catalogue.
#[test]
fn a_record_id_without_a_prefix_is_refused() {
    blibs()
        .args(["show", "BV123"])
        .assert()
        .code(2)
        .stderr(contains("invalid record id"));

    blibs()
        .args(["--json", "show", "BV123"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("\"kind\": \"invalid_record_id\""));
}

/// The library list is compiled in, so this works offline and with a cold cache.
#[test]
fn the_library_table_lists_shorthand_and_isil() {
    blibs()
        .arg("libraries")
        .assert()
        .success()
        .stdout(contains("STABI"))
        .stdout(contains("DE-11"));
}

/// `--find` searches shorthand, name and city — "grimm" is one of HU's shorthands.
#[test]
fn find_matches_a_shorthand() {
    blibs()
        .args(["libraries", "--find", "grimm"])
        .assert()
        .success()
        .stdout(contains("HU"))
        .stdout(contains("DE-11"));
}

/// A detail view shows what the list knows — address included, opening hours never.
#[test]
fn the_detail_view_shows_the_address() {
    blibs()
        .args(["libraries", "HU"])
        .assert()
        .success()
        .stdout(contains("Geschwister-Scholl-Str"))
        .stdout(contains("DE-11"));
}

/// `--near HU` measures from HU, so HU itself is the first row at 0.0 km.
#[test]
fn near_a_library_starts_at_that_library() {
    let output = blibs()
        .args(["libraries", "--near", "HU"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).expect("the renderer writes UTF-8");
    let first = text.lines().next().unwrap_or_default();
    assert!(first.starts_with("HU "), "first row is {first:?}");
    assert!(first.contains("0.0 km"), "first row is {first:?}");
}

/// blibs geocodes nothing, and says so instead of quietly sending an address anywhere.
///
/// This is the *only* one of the four `--near` failures that "give coordinates or a
/// library shortcode" answers, and for a while it was the answer to all four.
#[test]
fn near_an_address_is_a_usage_error() {
    blibs()
        .args(["libraries", "--near", "Alexanderplatz"])
        .assert()
        .code(2)
        .stderr(contains("cannot locate"))
        .stderr(contains("--near 52.52"));
}

/// Two numbers outside the earth's ranges. Telling this user to give coordinates is
/// absurd — they gave coordinates; what they need is the range they left.
#[test]
fn near_coordinates_off_the_earth_names_the_ranges() {
    blibs()
        .args(["libraries", "--near", "91,181"])
        .assert()
        .code(2)
        .stderr(contains("not a point on the earth"))
        .stderr(contains("-90 to 90"))
        .stderr(contains("-180 to 180"))
        .stderr(contains("looks up no addresses").not());
}

/// Half a pair. The comma is the whole answer.
#[test]
fn near_one_value_asks_for_the_second() {
    blibs()
        .args(["libraries", "--near", "52.52"])
        .assert()
        .code(2)
        .stderr(contains("needs a latitude and a longitude"))
        .stderr(contains("separate the two with a comma"));
}

/// A typo in a shortcode is a library question, and gets the library list's own answer —
/// the same `did you mean` `--at` gives, rather than a lecture about coordinates.
#[test]
fn near_a_typo_in_a_shortcode_suggests_the_library() {
    blibs()
        .args(["libraries", "--near", "STABI2"])
        .assert()
        .code(2)
        .stderr(contains("unknown library"))
        .stderr(contains("did you mean"))
        .stderr(contains("STABI"));
}

/// An unset shell variable is the likely cause, and the message says so rather than
/// behaving as though the flag had not been given.
#[test]
fn an_empty_flag_value_is_a_usage_error() {
    blibs()
        .args(["search", "Kafka", "--title", ""])
        .assert()
        .code(2)
        .stderr(contains("--title was given an empty value"))
        .stderr(contains("shell variable"));
    blibs()
        .args(["search", "Kafka", "--at", ""])
        .assert()
        .code(2)
        .stderr(contains("--at was given an empty value"));
}

/// The kind is what an agent switches on, and `usage` told it only that *something* was
/// wrong. The hint was missing entirely, and the message carried the renderer's own
/// `error:` prefix inside the data field.
#[test]
fn a_bad_language_code_is_a_named_kind_in_json() {
    let output = blibs()
        .args(["search", "Kafka", "--language", "de", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    let text = String::from_utf8(output).expect("the renderer writes UTF-8");
    let value: serde_json::Value = serde_json::from_str(&text).expect("one JSON document");
    assert_eq!(value["error"]["kind"], "invalid_language");
    assert_eq!(value["error"]["code"], 2);
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("--language takes"), "{message}");
    assert!(!message.contains("error:"), "{message}");
    let hint = value["error"]["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("--language ger"), "{hint}");
}

/// The same in human form: one `error:` prefix, not two.
#[test]
fn a_bad_language_code_prints_one_error_prefix() {
    let output = blibs()
        .args(["search", "Kafka", "--language", "de"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    let text = String::from_utf8(output).expect("the renderer writes UTF-8");
    let first = text.lines().next().unwrap_or_default();
    assert!(first.starts_with("error: --language takes"), "{first:?}");
    assert_eq!(first.matches("error:").count(), 1, "{first:?}");
}

/// `libraries --json` is one array of objects, and it parses.
#[test]
fn libraries_json_is_an_array() {
    let output = blibs()
        .args(["--json", "libraries"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value =
        serde_json::from_slice(&output).expect("libraries --json must be valid JSON");
    let array = value.as_array().expect("libraries --json is an array");
    assert_eq!(array.len(), 123, "every institution is listed");
    assert!(
        array
            .iter()
            .any(|library| library["isil"] == "DE-11" && library["alias"] == "HU")
    );
}

/// A **lookup** that names nothing is exit 2 with the list's own suggestion, exactly as
/// `--at STABI2` is: `libraries STABI2` names one library the user believes exists, and an
/// empty list cannot be told apart from "this library really does not exist" — while the
/// suggestion the same list can produce would be thrown away.
#[test]
fn an_unknown_library_key_is_exit_two_with_a_suggestion() {
    blibs()
        .args(["libraries", "STABI2"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("unknown library \"STABI2\""))
        .stderr(contains("did you mean STABI?"));

    // In JSON it is the error envelope every other usage error uses, on stderr, with
    // nothing on stdout that could be mistaken for a result.
    blibs()
        .args(["--json", "libraries", "STABI2"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(contains("\"kind\": \"unknown_library\""))
        .stderr(contains("\"code\": 2"));
}

/// A **search** that matches nothing keeps its exit 1 and its empty array: "no library is
/// called that" is a true answer to `--find`, where it is no answer at all to a lookup.
#[test]
fn a_library_search_that_matches_nothing_stays_exit_one() {
    blibs()
        .args(["libraries", "--find", "Xqzzyplkwrmf"])
        .assert()
        .code(1)
        .stdout(predicates::str::is_empty())
        .stderr(contains("no library matched"));

    blibs()
        .args(["--json", "libraries", "--find", "Xqzzyplkwrmf"])
        .assert()
        .code(1)
        .stdout(contains("[]"));
}

/// A branch shorthand answers with the **branch**: its own address, the house it belongs
/// to in one line, and the `--at` that searches it.
///
/// The house's 98-branch listing must not appear — that answers the question about the
/// house, which is not the question that was asked.
#[test]
fn a_branch_key_answers_with_the_branch_not_its_house() {
    blibs()
        .args(["libraries", "AGB"])
        .assert()
        .success()
        .stdout(contains("Amerika-Gedenkbibliothek"))
        .stdout(contains("Blücherplatz 1, 10961 Berlin"))
        .stdout(contains("Branch of    VOEBB"))
        .stdout(contains("Search       --at AGB (voebb engine)"))
        // One branch of the network that is not this one; the whole listing is the
        // house's answer, not the branch's.
        .stdout(contains("Bibliothek am Luisenbad").not())
        .stdout(contains("branches").not());
}

/// The same for a branch no engine can search on its own: it is a perfectly ordinary
/// thing to look up, and the view says how to get at its holdings instead of pretending
/// the question was about the house.
#[test]
fn a_branch_outside_the_public_network_names_the_house_to_search() {
    blibs()
        .args(["libraries", "PHILBIB"])
        .assert()
        .success()
        .stdout(contains("Philologische Bibliothek"))
        .stdout(contains("Branch of    FU"))
        .stdout(contains("cannot be searched on its own"))
        .stdout(contains("search the institution instead: --at FU"));
}

/// In JSON the kind is a member, so an agent never has to infer from the members present
/// whether it asked about a house or about one of its branches.
#[test]
fn a_branch_document_says_it_is_a_branch_and_names_its_house() {
    let output = blibs()
        .args(["--json", "libraries", "BSTB"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document: serde_json::Value =
        serde_json::from_slice(&output).expect("libraries --json writes one document");

    assert_eq!(document["type"], "branch");
    assert_eq!(document["alias"], "BSTB");
    assert_eq!(document["parent"]["isil"], "DE-609");
    assert_eq!(document["search"]["at"], "BSTB");
    assert_eq!(document["search"]["engine"], "voebb");
    assert!(
        document.get("branches").is_none(),
        "the house's branch list is not the branch's answer: {document}"
    );
}

/// An institution keeps the document it always had, plus the kind that tells it apart
/// from a branch.
#[test]
fn an_institution_document_says_it_is_an_institution() {
    let output = blibs()
        .args(["--json", "libraries", "STABI"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document: serde_json::Value =
        serde_json::from_slice(&output).expect("libraries --json writes one document");

    assert_eq!(document["type"], "institution");
    assert_eq!(document["isil"], "DE-1");
    assert!(document["branches"].is_array());
}
