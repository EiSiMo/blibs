//! Black-box tests of the command line. Network-free by construction: everything here
//! must be answered before a request would be built.
//!
//! Two properties are asserted over and over, because they are the whole agent contract:
//! **the exit code** (2 is "you asked wrongly and nothing was sent", 1 is "asked and found
//! nothing") and **which stream carried what** — a result on stdout, an explanation on
//! stderr, and under `--json` nothing on stdout that is not a document.

mod common;

use assert_cmd::Command;
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
#[test]
fn near_an_address_is_a_usage_error() {
    blibs()
        .args(["libraries", "--near", "Alexanderplatz"])
        .assert()
        .code(2)
        .stderr(contains("--near 52.52"));
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

/// A lookup that finds nothing is exit 1, not exit 2: `libraries FOO` asks a question,
/// unlike `--at FOO`, which names a library the search must have.
#[test]
fn an_unknown_library_key_is_exit_one() {
    blibs()
        .args(["libraries", "ZZZZ"])
        .assert()
        .code(1)
        .stdout(predicates::str::is_empty())
        .stderr(contains("no library matched"));

    // In JSON that is an empty array — a document, not a failure — and still exit 1.
    blibs()
        .args(["--json", "libraries", "ZZZZ"])
        .assert()
        .code(1)
        .stdout(contains("[]"));
}
