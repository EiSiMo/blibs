//! The `blibs` binary. Thin on purpose: everything testable lives in the library, and the
//! seam between the tool and the network is [`blibs::http::Fetch`], not a flag.

use std::io::Write as _;

use clap::Parser;

use blibs::cli::Cli;
use blibs::error::{Error, ExitCode, UnexpectedError, UsageError};
use blibs::http::Http;
use blibs::render::{self, Style};

fn main() {
    // Scanned before clap runs: a parse error must still be reported as JSON when the
    // user asked for JSON, and clap cannot tell us that if it refuses to parse.
    let json = std::env::args_os().any(|arg| arg == "--json");
    let code = dispatch(json);
    let _ = std::io::stdout().flush();
    std::process::exit(i32::from(code.code()));
}

fn dispatch(json: bool) -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(clap_error) if clap_error.use_stderr() => return refused(clap_error, json),
        // Help and version are requests, not mistakes: clap has already rendered them.
        Err(clap_error) => {
            let _ = clap_error.print();
            return ExitCode::Success;
        }
    };

    let http = Http::new(cli.cache());
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let style = Style::detect();
    match blibs::cli::run(&cli, &http, &mut stdout, &mut stderr, style) {
        Ok(outcome) => outcome.exit(),
        Err(error) => report(&error, cli.json),
    }
}

/// Report what clap refused: an unknown subcommand, an unknown flag, a missing value —
/// the three commonest mistakes an agent makes.
///
/// In human form clap prints it itself, because only clap knows *which* help to point at
/// — the top-level one for an unknown subcommand, the subcommand's for a flag inside it.
/// **That output stays clap's, byte for byte**; nothing here improves on a usage block
/// written for a terminal.
///
/// In JSON form clap prints nothing at all, so the envelope is the entire answer. It used
/// to carry clap's whole rendering inside `message` — its own `error: ` prefix, blank
/// lines, a usage line that reads as though `--at` were mandatory, and a pointer to
/// `--help` that no agent can follow. [`UsageError::Cli`] now reduces that to clap's first
/// line and turns what was actionable in the rest into a hint.
fn refused(clap_error: clap::Error, json: bool) -> ExitCode {
    if !json {
        let _ = clap_error.print();
        return ExitCode::Usage;
    }
    let error = Error::from(UsageError::Cli(clap_error));
    let mut stderr = std::io::stderr().lock();
    let _ = render::json::error(&error, &mut stderr);
    error.exit()
}

/// Write an error to stderr in the requested shape and return its exit code.
///
/// A closed pipe is not a failure: `blibs search … | head` closes stdout on purpose, and
/// complaining about it would turn a normal shell idiom into exit 6.
fn report(error: &Error, json: bool) -> ExitCode {
    if is_broken_pipe(error) {
        return ExitCode::Success;
    }
    let mut stderr = std::io::stderr().lock();
    if json {
        let _ = render::json::error(error, &mut stderr);
    } else {
        let _ = render::human::error(error, &mut stderr, Style::detect());
    }
    error.exit()
}

/// Whether this error is "the reader went away".
fn is_broken_pipe(error: &Error) -> bool {
    matches!(
        error,
        Error::Unexpected(UnexpectedError::Output { source })
            if source.kind() == std::io::ErrorKind::BrokenPipe
    )
}
