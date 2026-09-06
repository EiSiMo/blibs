//! Orchestration: from a parsed command line to an outcome.
//!
//! The order is not negotiable. Search first — with `--at` already in the upstream filter
//! — then select the records that will actually be displayed, and only **then** ask
//! whether those are in. Availability is one request per displayed record; asking for
//! more than fits on the screen would cost linearly and buy nothing.
//!
//! Both engines run concurrently, one thread each, over the same `&dyn Fetch`. **If one
//! of them fails, the whole invocation fails** with that error and its exit code: a
//! half-answer that looks whole is exactly what this tool must not produce.

use std::io::Write;

use crate::cli::Cli;
use crate::error::{Error, Outcome};
use crate::http::Fetch;

/// Run one invocation and write its output.
///
/// Returns [`Outcome`] for the two success cases (found / found nothing) and [`Error`]
/// for the five failure categories. The exit code comes from whichever of the two came
/// back; `main` does not decide it.
pub fn run(_cli: &Cli, _fetch: &dyn Fetch, _out: &mut dyn Write) -> Result<Outcome, Error> {
    todo!("phase 4: cli run")
}
