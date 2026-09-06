//! The `blibs` binary. Thin on purpose: everything testable lives in the library, and the
//! seam between the tool and the network is [`blibs::http::Fetch`], not a flag.

use std::io::Write as _;

use clap::{CommandFactory, Parser};

use blibs::cli::Cli;
use blibs::error::{Error, ExitCode, UsageError};
use blibs::http::Http;
use blibs::render;

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
        // Help and version are requests, not mistakes: clap has already written them.
        Err(err) if err.use_stderr() => return report(&UsageError::Cli(err).into(), json),
        Err(err) => {
            let _ = err.print();
            return ExitCode::Success;
        }
    };

    if cli.command.is_none() {
        let _ = Cli::command().print_long_help();
        return ExitCode::Success;
    }

    let http = Http::new(!cli.no_cache);
    let mut stdout = std::io::stdout().lock();
    match blibs::cli::run(&cli, &http, &mut stdout) {
        Ok(outcome) => outcome.exit(),
        Err(err) => report(&err, cli.json),
    }
}

/// Write an error to stderr in the requested shape and return its exit code.
fn report(error: &Error, json: bool) -> ExitCode {
    let mut stderr = std::io::stderr().lock();
    let _ = if json {
        render::json::error(&mut stderr, error)
    } else {
        render::human::error(&mut stderr, error, render::Style::detect())
    };
    error.exit()
}
