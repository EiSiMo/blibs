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
//! `Display` text and [`Error::hint`]. [`ErrorEnvelope`] is the serialisable shape it
//! writes; assembling it is a `From` conversion so a new variant cannot forget a member.

use std::fmt;

use crate::model::{Engine, RecordId};

/// Where a bug in this tool is reported. Named once so that every "this should not
/// happen" hint points at the same place.
const REPORT_URL: &str = "https://github.com/EiSiMo/blibs/issues";

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

/// Why a run produced no records. Never collapsed into a bare "no results": the five
/// reasons call for five different next steps, and only `NoHits` means the catalogue
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
        /// How many records the catalogue has for the query, when it said so. Without
        /// it the message cannot make the point that the hits exist and the window was
        /// simply too small.
        total: Option<u64>,
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

impl EmptyReason {
    /// The human explanation, ready to print. One or two lines: what happened, and — when
    /// there is one — what to try instead.
    ///
    /// This is the only place the wording lives; the JSON output carries the same facts
    /// as structured members (`total`, `window.fetched`, `window.after_filter`), so an
    /// agent never has to parse this text.
    pub fn message(&self) -> String {
        match self {
            EmptyReason::NoHits { terms } => format!(
                "no results for {terms:?}\n\
                 try fewer or more general words — the catalogue matches whole words, not fragments"
            ),
            EmptyReason::FilteredOut {
                total,
                fetched,
                filter,
                value,
            } => {
                let head = match total {
                    Some(total) => format!(
                        "{total} results, but none of the {fetched} fetched records matched {filter} {value}"
                    ),
                    None => {
                        format!("none of the {fetched} fetched records matched {filter} {value}")
                    }
                };
                format!(
                    "{head}\n\
                     narrow the search itself (--title, --author) so the filter has more to work on"
                )
            }
            EmptyReason::NoHoldings { locations } => {
                let locations = locations.join(", ");
                format!(
                    "hits exist, but none of them is held at {locations}\n\
                     run the same search without --at to see who else holds it"
                )
            }
            EmptyReason::NoSuchRecord { id } => format!(
                "no such record: {id}\n\
                 record ids come from the search output and change when a record is merged upstream"
            ),
            EmptyReason::NoLibraryMatched { query } => format!(
                "no library matched {query:?}\n\
                 run `blibs libraries` to see the full list"
            ),
        }
    }
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
    /// Exit code for this error. The category *is* the code — a variant cannot pick one.
    pub fn exit(&self) -> ExitCode {
        match self {
            Error::Usage(_) => ExitCode::Usage,
            Error::Network(_) => ExitCode::Network,
            Error::Service(_) => ExitCode::Unavailable,
            Error::Rejected(_) => ExitCode::Rejected,
            Error::Unexpected(_) => ExitCode::Unexpected,
        }
    }

    /// Stable machine-readable tag for the JSON error object, e.g. `unknown_library`.
    /// Unique across all variants — an agent switches on this, never on the message.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Usage(e) => e.kind(),
            Error::Network(e) => e.kind(),
            Error::Service(e) => e.kind(),
            Error::Rejected(e) => e.kind(),
            Error::Unexpected(e) => e.kind(),
        }
    }

    /// What the user should do next, if there is a useful next step. Never a restatement
    /// of the message: the message says what failed, the hint says what to try.
    pub fn hint(&self) -> Option<String> {
        match self {
            Error::Usage(e) => e.hint(),
            Error::Network(e) => e.hint(),
            Error::Service(e) => e.hint(),
            Error::Rejected(e) => e.hint(),
            Error::Unexpected(e) => e.hint(),
        }
    }
}

impl From<clap::Error> for Error {
    fn from(error: clap::Error) -> Self {
        Error::Usage(UsageError::Cli(error))
    }
}

/// Writing the output failed — a closed pipe, a full disk. Exit 6, because it is neither
/// the user's mistake nor the service's.
impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Unexpected(UnexpectedError::Output { source })
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
    /// A flag that one engine honours and the other cannot — there is no index for it in
    /// that catalogue, and guessing would answer the wrong question.
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

impl UsageError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            UsageError::UnknownLibrary { .. } => "unknown_library",
            UsageError::BranchNotSearchable { .. } => "branch_not_searchable",
            UsageError::Wildcard { .. } => "wildcard_unsupported",
            UsageError::Range { .. } => "range_unsupported",
            UsageError::YearFormat { .. } => "invalid_year",
            UsageError::Isbn { .. } => "invalid_isbn",
            UsageError::EmptyQuery => "empty_query",
            UsageError::QueryTooLong { .. } => "query_too_long",
            UsageError::LimitOutOfRange { .. } => "limit_out_of_range",
            UsageError::PageOutOfRange { .. } => "page_out_of_range",
            UsageError::FlagUnsupportedByEngine { .. } => "unsupported_by_engine",
            UsageError::NearNeedsCoordinates { .. } => "near_needs_coordinates",
            UsageError::RecordId { .. } => "invalid_record_id",
            UsageError::Cli(_) => "usage",
        }
    }

    /// The way out of this mistake. `None` only for [`UsageError::Cli`], where clap has
    /// already printed the usage line and a second pointer would just be noise.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            UsageError::UnknownLibrary { suggestions, .. } => {
                let lookup = "run `blibs libraries --find <name>` to look up a library";
                if suggestions.is_empty() {
                    lookup.to_string()
                } else {
                    format!("did you mean {}? {lookup}", suggestions.join(", "))
                }
            }
            UsageError::BranchNotSearchable { fallback, .. } => {
                format!("search the institution instead: --at {fallback}")
            }
            UsageError::Wildcard { .. } => {
                "this catalogue cannot truncate — search for the whole word, \
                 or use fewer words to widen the search"
                    .to_string()
            }
            UsageError::Range { .. } => {
                "ranges do not exist here and would silently find nothing — \
                 search one year at a time, e.g. --year 1994"
                    .to_string()
            }
            UsageError::YearFormat { .. } => {
                "give a four-digit year, e.g. --year 1994; for a serial it means \
                 \"was running in that year\""
                    .to_string()
            }
            UsageError::Isbn { problem, .. } => problem.hint(),
            UsageError::EmptyQuery => {
                "give at least one search word, e.g. `blibs search Kafka Prozess`".to_string()
            }
            UsageError::QueryTooLong { .. } => {
                "keep the query under 1000 characters — the service answers a longer one \
                 with an error, not with results"
                    .to_string()
            }
            UsageError::LimitOutOfRange { .. } => {
                "use --limit 1..50; the catalogue caps at 50 without saying so, \
                 and --page 2 fetches the next window"
                    .to_string()
            }
            UsageError::PageOutOfRange { .. } => {
                "pages are 1-based: the first page is --page 1".to_string()
            }
            UsageError::FlagUnsupportedByEngine { flag, engine, .. } => format!(
                "drop {flag}, or leave the {engine} locations out of --at so the other \
                 catalogue answers"
            ),
            UsageError::NearNeedsCoordinates { .. } => {
                "blibs looks up no addresses — give coordinates (--near 52.52,13.41) \
                 or a library shortcode (--near HU)"
                    .to_string()
            }
            UsageError::RecordId { .. } => {
                "a record id carries its catalogue prefix, e.g. almafu_BV008885798 or \
                 voebb_SAK13776205 — copy it from the search output"
                    .to_string()
            }
            UsageError::Cli(_) => return None,
        };
        Some(hint)
    }
}

/// Which part of an ISBN failed validation. Structured so that the hint can name it —
/// and, for the dangerous case, name the digit that was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsbnProblem {
    /// Not 10 or 13 digits after stripping hyphens and spaces.
    Length {
        /// How many digits were left after stripping.
        digits: usize,
    },
    /// A character that is neither a digit nor a trailing `X`.
    Characters {
        /// The first offending character.
        found: char,
    },
    /// The check digit does not match the rest — the dangerous case, because the index
    /// ignores it and would return a different book.
    CheckDigit {
        /// The digit the rest of the number implies.
        expected: char,
        /// The digit the user typed.
        found: char,
    },
}

impl IsbnProblem {
    /// What to do about it. The check-digit case explains *why* this is refused instead
    /// of sent: `dc.identifier` drops the check digit, so a typo finds the wrong book
    /// rather than none.
    pub fn hint(self) -> String {
        match self {
            IsbnProblem::Length { .. } => {
                "an ISBN has 10 or 13 digits; hyphens and spaces are fine".to_string()
            }
            IsbnProblem::Characters { .. } => {
                "an ISBN is digits only, except for a trailing X in an ISBN-10".to_string()
            }
            IsbnProblem::CheckDigit { expected, .. } => format!(
                "the catalogue ignores the check digit, so a typo would quietly return \
                 the wrong book — check the digits; as typed, the last one would have to \
                 be {expected}"
            ),
        }
    }
}

impl fmt::Display for IsbnProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IsbnProblem::Length { digits } => {
                write!(f, "must be 10 or 13 digits, got {digits}")
            }
            IsbnProblem::Characters { found } => {
                write!(f, "contains {found:?}, which is not a digit")
            }
            IsbnProblem::CheckDigit { expected, found } => {
                write!(f, "check digit is {found}, expected {expected}")
            }
        }
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

impl NetworkError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            NetworkError::Transport { .. } => "network",
            NetworkError::Timeout { .. } => "timeout",
        }
    }

    /// Both cases are worth retrying; the hint says what else to check first.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            NetworkError::Transport { host, .. } => {
                format!("check your internet connection, then try again — {host} may also be down")
            }
            NetworkError::Timeout { .. } => {
                "the service was slow to answer — try again, or use a smaller --limit \
                 so less has to be fetched"
                    .to_string()
            }
        };
        Some(hint)
    }
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

impl ServiceError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            ServiceError::Throttled { .. } => "service_unavailable",
        }
    }

    /// Waiting is the only remedy — blibs already caps itself, so there is nothing to
    /// turn down on this side.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            ServiceError::Throttled { host, .. } => format!(
                "wait a minute and run the search again; blibs already keeps at most \
                 6 requests in flight per host, so {host} is simply busy"
            ),
        };
        Some(hint)
    }
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
    /// though it passed the client-side length check. Despite the service's wording
    /// ("System temporarily unavailable") this is permanent: it comes from an HTTP 414,
    /// so it is never retried.
    #[error("the catalogue rejected the query as too long")]
    TooLongUpstream {
        /// The diagnostic URI it came back as.
        uri: String,
    },
}

impl RejectedError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            RejectedError::Diagnostic { .. } => "query_rejected",
            RejectedError::TooLongUpstream { .. } => "query_too_long_upstream",
        }
    }

    /// The remedy depends on *which* diagnostic came back, so it hangs off the URI. The
    /// codes are the ones measured against `k2` (`plan/scraping.md` §A.5,
    /// `plan/cql-verified.md`); several of them can only mean a bug in blibs, and say so.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            RejectedError::Diagnostic { uri, .. } => match diagnostic_code(uri) {
                "1/1" => "the catalogue's backend gave up on this query — \
                          try shorter search words"
                    .to_string(),
                "1/2" => "the query was too long for the service to accept — \
                          use fewer or shorter search words"
                    .to_string(),
                "1/4" => format!(
                    "blibs only ever sends searchRetrieve, so an unsupported operation \
                     is a bug in blibs — please report it at {REPORT_URL}"
                ),
                "1/10" => "the catalogue could not parse the query — drop punctuation \
                           such as ( ) / = from the search words"
                    .to_string(),
                "1/16" => format!(
                    "blibs asked for an index this catalogue does not have — \
                     that is a bug in blibs, please report it at {REPORT_URL}"
                ),
                "1/48" => "this catalogue cannot truncate — search for whole words \
                           instead of using * or ?"
                    .to_string(),
                "1/80" => format!(
                    "sorting is done in blibs over the fetched records, so the catalogue \
                     is never asked to sort — please report this at {REPORT_URL}"
                ),
                _ => "the catalogue refused the query itself — try simpler search words, \
                      and report it if plain words fail too"
                    .to_string(),
            },
            RejectedError::TooLongUpstream { .. } => {
                "retrying will not help — this is a length limit, not an outage. \
                 Use fewer or shorter search words."
                    .to_string()
            }
        };
        Some(hint)
    }
}

/// The `1/48` tail of a diagnostic URI such as `info:srw/diagnostic/1/48`. A URI that is
/// not shaped that way is returned unchanged, so the match simply falls through to the
/// generic hint instead of guessing.
fn diagnostic_code(uri: &str) -> &str {
    match uri.rsplit_once("/diagnostic/") {
        Some((_, code)) => code,
        None => uri,
    }
}

/// Exit 6 — HTTP 200 whose content failed the plausibility check.
///
/// Every variant names the thing that was missing. None of them may ever be turned into
/// an empty result: "no such element" and "no hits" must stay distinguishable.
#[derive(Debug, thiserror::Error)]
pub enum UnexpectedError {
    /// The body did not parse as XML at all — typically an HTML error page with status
    /// 200.
    #[error("{context}: expected XML, the body begins {snippet:?}")]
    NotXml {
        /// Which request produced it.
        context: String,
        /// The first few characters of the body. Part of the message, because "not XML"
        /// on its own tells a bug report nothing about what actually arrived.
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
    /// The output could not be written — a closed pipe, a full disk. Neither the user's
    /// mistake nor the service's, so it lands in the same bucket as a broken response.
    #[error("cannot write the output: {source}")]
    Output {
        /// What the write failed with.
        #[source]
        source: std::io::Error,
    },
}

impl UnexpectedError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            UnexpectedError::NotXml { .. } => "not_xml",
            UnexpectedError::MissingElement { .. } => "missing_structure",
            UnexpectedError::MissingSelector { .. } => "missing_selector",
            UnexpectedError::HttpStatus { .. } => "unexpected_status",
            UnexpectedError::MalformedJson { .. } => "malformed_json",
            UnexpectedError::CountMismatch { .. } => "count_mismatch",
            UnexpectedError::VoebbNoAccess { .. } => "voebb_session_lost",
            UnexpectedError::Output { .. } => "output_failed",
        }
    }

    /// Most of these mean the remote service changed shape, which the user cannot fix —
    /// so the hint says whether retrying is worth it and where to report the rest.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            UnexpectedError::NotXml { .. } => format!(
                "the service answered with something that is not an SRU document, \
                 usually a proxy error page — try again in a minute, and report it at \
                 {REPORT_URL} if it keeps happening"
            ),
            UnexpectedError::MissingElement { what, .. } => format!(
                "the response no longer contains {what}; blibs needs updating — \
                 please report it at {REPORT_URL}"
            ),
            UnexpectedError::MissingSelector { selector, .. } => format!(
                "the page changed and {selector} no longer matches; blibs needs updating — \
                 please report that selector at {REPORT_URL}"
            ),
            UnexpectedError::HttpStatus { status, .. } => format!(
                "HTTP {status} is not something blibs can act on — try again in a minute, \
                 and report it at {REPORT_URL} if it persists"
            ),
            UnexpectedError::MalformedJson { .. } => format!(
                "the availability service returned JSON blibs cannot read; \
                 rerun with --no-availability to get holdings without the traffic lights, \
                 and report it at {REPORT_URL}"
            ),
            UnexpectedError::CountMismatch { .. } => format!(
                "the response announced more items than it delivered, so the copy list \
                 would have been silently short — please report it at {REPORT_URL}"
            ),
            UnexpectedError::VoebbNoAccess { .. } => {
                "voebb.de sessions are short-lived and single-use — run the search again; \
                 if it fails twice in a row, the site has changed"
                    .to_string()
            }
            UnexpectedError::Output { .. } => {
                "the output stream closed early — this is expected when piping into \
                 `head`; otherwise check the disk"
                    .to_string()
            }
        };
        Some(hint)
    }
}

/// The JSON error document: `{ "error": { code, kind, message, hint } }`.
///
/// Built only from an [`Error`], never by hand, so that `code`, `kind`, `message` and
/// `hint` can never disagree with the enum. `hint` is `null` when there is no useful next
/// step — never an empty string, and never omitted, so the shape is the same every time.
#[derive(Debug, serde::Serialize)]
pub struct ErrorEnvelope<'a> {
    /// The single member, so that the document is self-describing.
    pub error: ErrorBody<'a>,
}

/// The members of the JSON error object. Member order is part of the contract.
#[derive(Debug, serde::Serialize)]
pub struct ErrorBody<'a> {
    /// The process exit code, as a number.
    pub code: u8,
    /// The stable tag an agent switches on.
    pub kind: &'a str,
    /// What failed, in the same words the terminal shows.
    pub message: String,
    /// What to do next, or `null`.
    pub hint: Option<String>,
}

impl<'a> From<&'a Error> for ErrorEnvelope<'a> {
    fn from(error: &'a Error) -> Self {
        ErrorEnvelope {
            error: ErrorBody {
                code: error.exit().code(),
                kind: error.kind(),
                message: error.to_string(),
                hint: error.hint(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// One constructed example of every variant in the enum, split by category only so
    /// that neither half outgrows a readable function. The list is the test: a new
    /// variant that is not added here fails the uniqueness and hint checks below.
    fn all_variants() -> Vec<Error> {
        let mut all = usage_variants();
        all.extend(remote_variants());
        all
    }

    /// Everything the user can get wrong before a byte goes out.
    fn usage_variants() -> Vec<Error> {
        vec![
            UsageError::UnknownLibrary {
                input: "STABI2".to_string(),
                suggestions: vec!["STABI".to_string(), "SBB".to_string()],
            }
            .into(),
            UsageError::BranchNotSearchable {
                input: "philbib".to_string(),
                fallback: "FU".to_string(),
            }
            .into(),
            UsageError::Wildcard {
                term: "Proze*".to_string(),
            }
            .into(),
            UsageError::Range {
                input: "1990-2000".to_string(),
            }
            .into(),
            UsageError::YearFormat {
                input: "199".to_string(),
            }
            .into(),
            UsageError::Isbn {
                input: "978-3-596-29433-4".to_string(),
                problem: IsbnProblem::CheckDigit {
                    expected: '1',
                    found: '4',
                },
            }
            .into(),
            UsageError::EmptyQuery.into(),
            UsageError::QueryTooLong { chars: 2400 }.into(),
            UsageError::LimitOutOfRange { value: 200 }.into(),
            UsageError::PageOutOfRange { value: 0 }.into(),
            UsageError::FlagUnsupportedByEngine {
                flag: "--publisher".to_string(),
                engine: Engine::Voebb,
            }
            .into(),
            UsageError::NearNeedsCoordinates {
                input: "Alexanderplatz 1".to_string(),
            }
            .into(),
            UsageError::RecordId {
                input: "BV008885798".to_string(),
            }
            .into(),
            UsageError::Cli(clap::Error::raw(
                clap::error::ErrorKind::UnknownArgument,
                "unexpected argument '--nope' found",
            ))
            .into(),
        ]
    }

    /// Everything that can go wrong once a request is on the wire.
    fn remote_variants() -> Vec<Error> {
        vec![
            NetworkError::Transport {
                host: "sru.kobv.de".to_string(),
                source: Box::new(std::io::Error::other("connection refused")),
            }
            .into(),
            NetworkError::Timeout {
                host: "portal.kobv.de".to_string(),
                seconds: 10,
            }
            .into(),
            ServiceError::Throttled {
                host: "portal.kobv.de".to_string(),
                status: 503,
                attempts: 3,
            }
            .into(),
            RejectedError::Diagnostic {
                uri: "info:srw/diagnostic/1/16".to_string(),
                message: "Unsupported index".to_string(),
                details: Some("nonexistentindex".to_string()),
            }
            .into(),
            RejectedError::TooLongUpstream {
                uri: "info:srw/diagnostic/1/2".to_string(),
            }
            .into(),
            UnexpectedError::NotXml {
                context: "SRU searchRetrieve".to_string(),
                snippet: "<html><head><title>502 Proxy Error".to_string(),
            }
            .into(),
            UnexpectedError::MissingElement {
                what: "zs:numberOfRecords".to_string(),
                context: "SRU searchRetrieve".to_string(),
            }
            .into(),
            UnexpectedError::MissingSelector {
                selector: "tr.avail-item".to_string(),
                document: "availability fragment".to_string(),
            }
            .into(),
            UnexpectedError::HttpStatus {
                host: "portal.kobv.de".to_string(),
                status: 500,
            }
            .into(),
            UnexpectedError::MalformedJson {
                context: "getAvailability".to_string(),
                detail: "expected object, found array".to_string(),
            }
            .into(),
            UnexpectedError::CountMismatch {
                context: "availability fragment".to_string(),
                expected: 8,
                found: 3,
            }
            .into(),
            UnexpectedError::VoebbNoAccess {
                step: "branch facet".to_string(),
            }
            .into(),
            UnexpectedError::Output {
                source: std::io::Error::other("broken pipe"),
            }
            .into(),
        ]
    }

    #[test]
    fn every_kind_is_unique() {
        let errors = all_variants();
        let kinds: BTreeSet<&str> = errors.iter().map(Error::kind).collect();
        assert_eq!(
            kinds.len(),
            errors.len(),
            "two variants share a `kind`; an agent could not tell them apart"
        );
    }

    #[test]
    fn every_variant_says_what_failed() {
        for error in all_variants() {
            let message = error.to_string();
            assert!(!message.trim().is_empty(), "empty message for {error:?}");
            assert!(
                error
                    .kind()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "kind {:?} is not snake_case",
                error.kind()
            );
        }
    }

    #[test]
    fn every_variant_but_clap_says_what_to_do() {
        for error in all_variants() {
            // clap has already printed its own usage line; a second pointer is noise.
            if matches!(error, Error::Usage(UsageError::Cli(_))) {
                assert!(error.hint().is_none(), "clap error carries a hint");
                continue;
            }
            let hint = error.hint().unwrap_or_default();
            assert!(!hint.trim().is_empty(), "no hint for {error:?}");
        }
    }

    #[test]
    fn the_category_decides_the_exit_code() {
        for error in all_variants() {
            let expected = match error {
                Error::Usage(_) => ExitCode::Usage,
                Error::Network(_) => ExitCode::Network,
                Error::Service(_) => ExitCode::Unavailable,
                Error::Rejected(_) => ExitCode::Rejected,
                Error::Unexpected(_) => ExitCode::Unexpected,
            };
            assert_eq!(error.exit(), expected, "wrong exit code for {error:?}");
        }
        assert_eq!(ExitCode::Usage.code(), 2);
        assert_eq!(ExitCode::Unexpected.code(), 6);
    }

    /// The exact object from `plan/cli.md` § Fehler — members, order and wording.
    #[test]
    fn unknown_library_serialises_exactly_as_specified() {
        let error: Error = UsageError::UnknownLibrary {
            input: "STABI2".to_string(),
            suggestions: Vec::new(),
        }
        .into();
        let json = serde_json::to_string(&ErrorEnvelope::from(&error))
            .expect("the error envelope contains only strings and a number");
        assert_eq!(
            json,
            r#"{"error":{"code":2,"kind":"unknown_library","message":"unknown library \"STABI2\"","hint":"run `blibs libraries --find <name>` to look up a library"}}"#
        );
    }

    #[test]
    fn suggestions_lead_the_hint_when_there_are_any() {
        let error: Error = UsageError::UnknownLibrary {
            input: "STABI2".to_string(),
            suggestions: vec!["STABI".to_string(), "SBB".to_string()],
        }
        .into();
        assert_eq!(
            error.hint().as_deref(),
            Some(
                "did you mean STABI, SBB? run `blibs libraries --find <name>` to look up a library"
            )
        );
    }

    #[test]
    fn a_missing_hint_serialises_as_null() {
        let error: Error = UsageError::Cli(clap::Error::raw(
            clap::error::ErrorKind::UnknownArgument,
            "unexpected argument",
        ))
        .into();
        let value = serde_json::to_value(ErrorEnvelope::from(&error))
            .expect("the error envelope contains only strings and a number");
        assert_eq!(value["error"]["hint"], serde_json::Value::Null);
        assert_eq!(value["error"]["code"], 2);
    }

    /// The remedy for a rejected query hangs off the diagnostic URI: the four measured
    /// codes must each say something different, and none may fall through to the generic
    /// text.
    #[test]
    fn each_diagnostic_uri_gets_its_own_hint() {
        let cases = [
            ("info:srw/diagnostic/1/10", "punctuation"),
            ("info:srw/diagnostic/1/16", "does not have"),
            ("info:srw/diagnostic/1/48", "truncate"),
            ("info:srw/diagnostic/1/80", "sorting is done in blibs"),
            ("info:srw/diagnostic/1/4", "searchRetrieve"),
            ("info:srw/diagnostic/1/1", "backend gave up"),
            ("info:srw/diagnostic/1/2", "too long"),
        ];
        let mut seen = BTreeSet::new();
        for (uri, expected) in cases {
            let error = RejectedError::Diagnostic {
                uri: uri.to_string(),
                message: "…".to_string(),
                details: None,
            };
            let hint = error.hint().unwrap_or_default();
            assert!(
                hint.contains(expected),
                "hint for {uri} does not mention {expected:?}: {hint}"
            );
            assert!(seen.insert(hint), "two diagnostics share a hint: {uri}");
        }
    }

    #[test]
    fn an_unknown_diagnostic_still_gets_a_next_step() {
        let error = RejectedError::Diagnostic {
            uri: "info:srw/diagnostic/1/63".to_string(),
            message: "Surrogate diagnostic".to_string(),
            details: None,
        };
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("simpler search words"), "{hint}");
    }

    #[test]
    fn diagnostic_code_takes_the_tail_and_tolerates_anything_else() {
        assert_eq!(diagnostic_code("info:srw/diagnostic/1/48"), "1/48");
        assert_eq!(diagnostic_code("1/48"), "1/48");
        assert_eq!(diagnostic_code(""), "");
    }

    #[test]
    fn the_isbn_message_names_the_expected_check_digit() {
        let error: Error = UsageError::Isbn {
            input: "978-3-596-29433-4".to_string(),
            problem: IsbnProblem::CheckDigit {
                expected: '1',
                found: '4',
            },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid ISBN \"978-3-596-29433-4\": check digit is 4, expected 1"
        );
        assert!(error.hint().unwrap_or_default().contains("be 1"));
    }

    /// An empty result is exit 1, and its text must distinguish "the catalogue has
    /// nothing" from "the window had nothing" — `plan/cli.md` pins this wording.
    #[test]
    fn a_filtered_out_window_says_so_and_exits_one() {
        let outcome = Outcome::Empty(EmptyReason::FilteredOut {
            total: Some(774),
            fetched: 50,
            filter: "--format".to_string(),
            value: "video".to_string(),
        });
        assert_eq!(outcome.exit(), ExitCode::NoResults);
        let Outcome::Empty(reason) = &outcome else {
            unreachable!("just constructed as Empty")
        };
        assert_eq!(
            reason.message(),
            "774 results, but none of the 50 fetched records matched --format video\n\
             narrow the search itself (--title, --author) so the filter has more to work on"
        );
    }

    #[test]
    fn every_empty_reason_has_a_first_line_of_its_own() {
        let reasons = [
            EmptyReason::NoHits {
                terms: "Kafka Prozess".to_string(),
            },
            EmptyReason::FilteredOut {
                total: None,
                fetched: 10,
                filter: "--language".to_string(),
                value: "fre".to_string(),
            },
            EmptyReason::NoHoldings {
                locations: vec!["STABI".to_string(), "HU".to_string()],
            },
            EmptyReason::NoSuchRecord {
                id: RecordId::voebb("SAK13776205"),
            },
            EmptyReason::NoLibraryMatched {
                query: "grimm".to_string(),
            },
        ];
        let mut seen = BTreeSet::new();
        for reason in reasons {
            let message = reason.message();
            let first = message.lines().next().unwrap_or_default().to_string();
            assert!(!first.trim().is_empty(), "empty message for {reason:?}");
            assert!(seen.insert(first), "two reasons read the same: {reason:?}");
        }
        assert_eq!(
            EmptyReason::NoHits {
                terms: "Kafka Prozess".to_string()
            }
            .message()
            .lines()
            .next(),
            Some("no results for \"Kafka Prozess\"")
        );
        assert_eq!(
            EmptyReason::NoSuchRecord {
                id: RecordId::voebb("SAK13776205")
            }
            .message()
            .lines()
            .next(),
            Some("no such record: voebb_SAK13776205")
        );
    }

    #[test]
    fn io_and_clap_errors_convert_into_the_right_category() {
        let io: Error = std::io::Error::other("broken pipe").into();
        assert_eq!(io.exit(), ExitCode::Unexpected);
        assert_eq!(io.kind(), "output_failed");

        let clap: Error = clap::Error::raw(clap::error::ErrorKind::InvalidValue, "bad").into();
        assert_eq!(clap.exit(), ExitCode::Usage);
        assert_eq!(clap.kind(), "usage");
    }
}
