//! One error enum for the whole crate, and the exit codes it maps onto.
//!
//! Two rules decide what belongs here.
//!
//! 1. **An error carries context, not just a cause.** Every variant has structured
//!    fields, its `Display` says *what* failed, and [`Error::hint`] says *what to do
//!    next*. Nothing is stringified early — the JSON renderer needs the parts.
//! 2. **"Nothing found" is not an error.** It is [`Outcome::Empty`] with an
//!    [`EmptyReason`], and it exits 1. Only the five categories of [`Error`] exit 2–6.
//!
//! The JSON error object `{ "error": { code, kind, message, hint } }` is built in exactly
//! one place, [`crate::render::json`], out of [`Error::exit`], [`Error::kind`], the
//! `Display` text and [`Error::hint`].

use std::fmt;

use crate::model::{Engine, RecordId};

/// Process exit codes. Part of the public interface: once published, a code's meaning
/// does not change.
///
/// `1` is deliberately distinct from `0` so that an agent can tell "searched, found
/// nothing" from "found something" without parsing output. `6` exists because HTTP 200
/// means nothing here — diagnostics, silent truncation and phantom records all arrive
/// with status 200.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ExitCode {
    /// At least one hit.
    Success = 0,
    /// The search ran and found nothing. Not a failure.
    NoResults = 1,
    /// The invocation itself was wrong; nothing was sent.
    Usage = 2,
    /// DNS, TLS, connection refused, timeout.
    Network = 3,
    /// The service answered 429/503 until the backoff was exhausted.
    Unavailable = 4,
    /// The query was rejected upstream by an SRU diagnostic.
    Rejected = 5,
    /// A 200 response whose content failed the plausibility check.
    Unexpected = 6,
}

impl ExitCode {
    /// The numeric code, for `std::process::exit` and for the JSON error object.
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// What a successful run produced. Kept apart from [`Error`] because exit 1 is a result,
/// not a failure, and the human renderer says something different for each reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// At least one record was shown. Exit 0.
    Found,
    /// Nothing was shown, and this is why. Exit 1.
    Empty(EmptyReason),
}

impl Outcome {
    /// Exit code of this outcome.
    pub fn exit(&self) -> ExitCode {
        match self {
            Outcome::Found => ExitCode::Success,
            Outcome::Empty(_) => ExitCode::NoResults,
        }
    }
}

/// Why a run produced no records. Never collapsed into a bare "no results": the four
/// reasons call for four different next steps, and only `NoHits` means the catalogue
/// really has nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmptyReason {
    /// The catalogue returned zero records for this query.
    NoHits {
        /// The query as the user typed it, echoed back.
        terms: String,
    },
    /// Records were fetched, but a client-side filter removed all of them. Carries the
    /// window size so the output can say that only the fetched records were seen.
    FilteredOut {
        /// How many records the filter looked at.
        fetched: usize,
        /// Which flag filtered, e.g. `--format`.
        filter: String,
        /// The value that flag was given.
        value: String,
    },
    /// Hits existed, but none of them is held at any of the requested locations.
    NoHoldings {
        /// The `--at` locations, as the user wrote them.
        locations: Vec<String>,
    },
    /// `show` was given an id the catalogue does not know.
    NoSuchRecord {
        /// The id that was looked up.
        id: RecordId,
    },
    /// `libraries --find`/`--near` matched nothing.
    NoLibraryMatched {
        /// The search string.
        query: String,
    },
}

/// The crate's only error type. Five categories, one per exit code.
///
/// Never boxed and never stringified across a module boundary: the JSON renderer needs
/// the structured fields, and `Box<dyn Error>` would discard them.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The invocation was wrong. Exit 2. Nothing was sent.
    #[error(transparent)]
    Usage(#[from] UsageError),
    /// The request never reached the service. Exit 3.
    #[error(transparent)]
    Network(#[from] NetworkError),
    /// The service is unavailable. Exit 4.
    #[error(transparent)]
    Service(#[from] ServiceError),
    /// The service understood the query and refused it. Exit 5.
    #[error(transparent)]
    Rejected(#[from] RejectedError),
    /// A 200 response that did not contain what it must contain. Exit 6.
    #[error(transparent)]
    Unexpected(#[from] UnexpectedError),
}

impl Error {
    /// Exit code for this error.
    pub fn exit(&self) -> ExitCode {
        todo!("phase 1: error")
    }

    /// Stable machine-readable tag for the JSON error object, e.g. `unknown_library`.
    /// Unique across all variants — an agent switches on this, never on the message.
    pub fn kind(&self) -> &'static str {
        todo!("phase 1: error")
    }

    /// What the user should do next, if there is a useful next step. Never a restatement
    /// of the message.
    pub fn hint(&self) -> Option<String> {
        todo!("phase 1: error")
    }
}

/// Exit 2 — caught in `cli` before the first byte goes out.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// `--at` named something that is not in the library list.
    #[error("unknown library {input:?}")]
    UnknownLibrary {
        /// What the user typed.
        input: String,
        /// Up to three close matches from the library list.
        suggestions: Vec<String>,
    },
    /// `--at` named a branch that no engine can search on its own.
    #[error("{input:?} is a branch that cannot be searched on its own")]
    BranchNotSearchable {
        /// What the user typed.
        input: String,
        /// The institution to use instead.
        fallback: String,
    },
    /// Truncation does not exist in this catalogue (diagnostic 1/48).
    #[error("wildcards are not supported: {term:?}")]
    Wildcard {
        /// The term containing `*` or `?`.
        term: String,
    },
    /// Ranges do not exist; sending one returns zero hits *silently*.
    #[error("ranges are not supported: {input:?}")]
    Range {
        /// The range the user typed.
        input: String,
    },
    /// `dc.date` knows four digits and nothing else.
    #[error("invalid year {input:?}")]
    YearFormat {
        /// What the user typed.
        input: String,
    },
    /// The identifier index ignores the check digit, so a typo returns the *wrong* book.
    #[error("invalid ISBN {input:?}: {problem}")]
    Isbn {
        /// What the user typed.
        input: String,
        /// Which part of the ISBN is wrong.
        problem: IsbnProblem,
    },
    /// An empty query is diagnostic 1/10 upstream.
    #[error("empty query")]
    EmptyQuery,
    /// Over 1000 characters the service answers HTTP 414 as diagnostic 1/2.
    #[error("query too long: {chars} characters")]
    QueryTooLong {
        /// Length of the assembled query.
        chars: usize,
    },
    /// `--limit` outside 1..=50. Never clamped: SRU caps at 50 silently, so a clamp
    /// would hide the truncation.
    #[error("--limit must be between 1 and 50, got {value}")]
    LimitOutOfRange {
        /// The value given.
        value: u32,
    },
    /// `--page` must be 1-based.
    #[error("--page must be 1 or greater, got {value}")]
    PageOutOfRange {
        /// The value given.
        value: u32,
    },
    /// Paging is not measured for this engine, so it is refused rather than guessed.
    #[error("paging is not supported for the {engine} catalogue")]
    PagingUnsupported {
        /// The engine that cannot page.
        engine: Engine,
    },
    /// A flag that one engine honours and the other cannot.
    #[error("{flag} is not supported by the {engine} catalogue")]
    FlagUnsupportedByEngine {
        /// The flag, spelled as on the command line.
        flag: String,
        /// The engine that does not support it.
        engine: Engine,
    },
    /// `--near` was given free text; the tool geocodes nothing.
    #[error("cannot locate {input:?}")]
    NearNeedsCoordinates {
        /// What the user typed.
        input: String,
    },
    /// A record id that does not carry a source prefix.
    #[error("invalid record id {input:?}")]
    RecordId {
        /// What the user typed.
        input: String,
    },
    /// Anything clap itself rejected — unknown flag, unknown subcommand, missing value.
    #[error(transparent)]
    Cli(#[from] clap::Error),
}

/// Which part of an ISBN failed validation. Structured so that the hint can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsbnProblem {
    /// Not 10 or 13 digits after stripping hyphens and spaces.
    Length,
    /// A character that is neither a digit nor a trailing `X`.
    Characters,
    /// The check digit does not match the rest — the dangerous case, because the index
    /// ignores it and would return a different book.
    CheckDigit,
}

impl fmt::Display for IsbnProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            IsbnProblem::Length => "must be 10 or 13 digits",
            IsbnProblem::Characters => "contains characters that are not digits",
            IsbnProblem::CheckDigit => "check digit does not match",
        };
        f.write_str(text)
    }
}

/// Exit 3 — the request never reached the service.
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// DNS, TLS, connection refused.
    #[error("cannot reach {host}: {source}")]
    Transport {
        /// The host that was contacted.
        host: String,
        /// The underlying transport failure.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The request was still open when the timeout expired.
    #[error("{host} did not answer within {seconds}s")]
    Timeout {
        /// The host that was contacted.
        host: String,
        /// The timeout that expired.
        seconds: u64,
    },
}

/// Exit 4 — the service asked to be left alone and the backoff was exhausted.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// 429 or 503 after every retry.
    #[error("{host} is throttling requests (HTTP {status}, gave up after {attempts} attempts)")]
    Throttled {
        /// The host that answered.
        host: String,
        /// The status it kept returning.
        status: u16,
        /// How many attempts were made.
        attempts: u32,
    },
}

/// Exit 5 — the service understood the query and refused it.
#[derive(Debug, thiserror::Error)]
pub enum RejectedError {
    /// A top-level SRU diagnostic.
    #[error("the catalogue rejected the query: {message}")]
    Diagnostic {
        /// The diagnostic URI, e.g. `info:srw/diagnostic/1/48`.
        uri: String,
        /// The service's own message.
        message: String,
        /// Its `details` element, when present.
        details: Option<String>,
    },
    /// Diagnostic 1/2 — the assembled query exceeded what the service accepts, even
    /// though it passed the client-side length check.
    #[error("the catalogue rejected the query as too long")]
    TooLongUpstream {
        /// The diagnostic URI it came back as.
        uri: String,
    },
}

/// Exit 6 — HTTP 200 whose content failed the plausibility check.
///
/// Every variant names the thing that was missing. None of them may ever be turned into
/// an empty result: "no such element" and "no hits" must stay distinguishable.
#[derive(Debug, thiserror::Error)]
pub enum UnexpectedError {
    /// The body did not parse as XML at all — typically an HTML error page with status
    /// 200.
    #[error("{context}: expected XML, got something else")]
    NotXml {
        /// Which request produced it.
        context: String,
        /// The first few characters of the body, for the message.
        snippet: String,
    },
    /// A required XML element or MARC field was absent.
    #[error("{context}: missing {what}")]
    MissingElement {
        /// The element or field that should have been there.
        what: String,
        /// Which document it should have been in.
        context: String,
    },
    /// An HTML selector matched nothing. The selector is part of the message, because it
    /// is the only thing that tells a maintainer what the remote site changed.
    #[error("{document}: selector {selector:?} matched nothing")]
    MissingSelector {
        /// The selector that failed.
        selector: String,
        /// Which document it was applied to.
        document: String,
    },
    /// A status that is neither success nor a throttling signal.
    #[error("{host} answered HTTP {status}")]
    HttpStatus {
        /// The host that answered.
        host: String,
        /// The status it returned.
        status: u16,
    },
    /// The body claimed to be JSON and was not, or lacked a required member.
    #[error("{context}: malformed JSON ({detail})")]
    MalformedJson {
        /// Which request produced it.
        context: String,
        /// What the JSON parser or the shape check said.
        detail: String,
    },
    /// A self-declared count disagrees with what was delivered — the counter-check
    /// against silent truncation.
    #[error("{context}: expected {expected} items, found {found}")]
    CountMismatch {
        /// Which document was checked.
        context: String,
        /// What the document announced.
        expected: usize,
        /// What it actually contained.
        found: usize,
    },
    /// voebb.de answered a broken form with 200 and a `/noaccess` page: a lost cookie, a
    /// stale `requestCount` or a missing hidden field. Never "no hits".
    #[error("the voebb.de session was lost during {step}")]
    VoebbNoAccess {
        /// Which step of the session flow hit it.
        step: String,
    },
}
