//! Black-box tests of the command line. Network-free by construction: everything here
//! must be answered before a request would be built.

mod common;

use assert_cmd::Command;

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
        .stdout(predicates::str::contains("Usage: blibs"))
        .stdout(predicates::str::contains("libraries"));
}

/// `--help` is a request, not an error.
#[test]
fn help_exits_zero() {
    blibs().arg("--help").assert().success();
}

/// An unknown subcommand is a usage error, exit 2, and it points at the help.
#[test]
#[ignore = "needs render::human::error — phase 3.3"]
fn unknown_subcommand_exits_two() {
    blibs().arg("serch").assert().code(2);
}

/// An unknown flag inside a subcommand is a usage error too, and the message shows that
/// subcommand's help rather than the top-level one.
#[test]
#[ignore = "needs render::human::error — phase 3.3"]
fn unknown_flag_exits_two() {
    blibs()
        .args(["search", "Kafka", "--nope"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("blibs search"));
}
