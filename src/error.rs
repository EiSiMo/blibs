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

use clap::error::ContextKind;

use crate::counts;
use crate::model::{Engine, RecordId};

/// Where a bug in this tool is reported. Named once so that every "this should not
/// happen" hint points at the same place — the notes that stand in for an error one
/// included, which is why it is not private to this module.
pub(crate) const REPORT_URL: &str = "https://github.com/EiSiMo/blibs/issues";

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

/// Why a run produced no records. Never collapsed into a bare "no results": every reason
/// here calls for a different next step, and only `NoHits` and `NoHitsAnywhere` mean the
/// catalogue really has nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmptyReason {
    /// The catalogue returned zero records for this query.
    NoHits {
        /// The query as the user typed it, echoed back.
        terms: String,
    },
    /// The same, but with `--at` in play: the search that came back empty was a
    /// *restricted* one, so widening the words is the wrong advice — widening the
    /// locations is the right one.
    ///
    /// It says nothing about whether the title exists elsewhere, and must not: `--at`
    /// filters upstream (`@attr 1=1044`), so there is no unfiltered count to compare
    /// against and asking for one would be a second request nobody wanted.
    NoHitsAtLocations {
        /// The query as the user typed it, echoed back.
        terms: String,
        /// The `--at` locations, as the user wrote them.
        locations: Vec<String>,
    },
    /// `--at` was in play, the restricted search came back empty — and so did the
    /// unrestricted one. The sibling of [`EmptyReason::NoHitsAtLocations`], and the
    /// distinction is not cosmetic: there, `--at` *is* the cause and naming a sibling
    /// branch is the way out; here it is not, and that advice is guaranteed to fail
    /// again.
    ///
    /// **What was proven is the catalogue that answered, not the region.** The
    /// unrestricted count comes from one engine — voebb.de's, over the public library
    /// network — and it says nothing about the university and research libraries the
    /// other engine answers for, so the message may not claim the region and may not tell
    /// the user that `--at` cannot help: `--at HU` still can (round 2).
    ///
    /// Only ever built where an **unfiltered** total is actually known. The `kobv` engine
    /// filters upstream and has no such total, so it must keep saying
    /// [`EmptyReason::NoHitsAtLocations`]; voebb.de answers with the network-wide count
    /// beside the branch facet, which is where the difference comes from.
    NoHitsAnywhere {
        /// The query as the user typed it, echoed back.
        terms: String,
        /// The `--at` locations, as the user wrote them. Named so the message can say
        /// which restriction was *not* to blame.
        locations: Vec<String>,
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
    /// `--available` was given and none of the displayed records is in right now. Its
    /// own reason, not `FilteredOut`: this filter runs *after* the page was cut, so
    /// narrowing the search — the advice `FilteredOut` gives — changes nothing here,
    /// while a larger page or another page can.
    NothingAvailable {
        /// How many records the catalogue has for the query, when it said so. The
        /// filter knows nothing about the records it never saw, and saying the total is
        /// what keeps the message from reading as "the catalogue has nothing".
        total: Option<u64>,
        /// How many displayed records the filter judged.
        judged: usize,
        /// How many of those stated no status at all. Printed, because an empty result
        /// renders no notes and this is the one place a human learns that the tool did
        /// not hear "on loan" — it heard nothing.
        unstated: usize,
    },
    /// A filtered window's pages ran out. Its own reason, not `FilteredOut`: records
    /// **did** match, so "none of the fetched records matched" would be false and the
    /// advice to narrow the search would send the user after a problem they do not have.
    /// With a filter the window is one anchored block and `--page` walks the matches
    /// inside it, so there is a last page and this is past it.
    PastTheLastMatch {
        /// How many records in the window matched the filter.
        matched: usize,
        /// Which flag filtered, e.g. `--format`.
        filter: String,
        /// The value that flag was given.
        value: String,
        /// The page that was asked for.
        page: u32,
        /// The last page that holds anything.
        last: u32,
    },
    /// The pages of a **sorted** window ran out. The sibling of
    /// [`EmptyReason::PastTheLastMatch`] for the other reason a window is anchored:
    /// ordering needs one set to order, so `--sort` pins the window to a single block and
    /// `--page` walks the records inside it.
    ///
    /// Its own variant rather than a field on `PastTheLastMatch`, because a sort has
    /// neither a filter nor a value to name, and the sentence it needs is a different
    /// one: nothing was *dropped* here — the records simply ran out.
    PastTheLastSorted {
        /// How many records the anchored window holds, after dedup.
        sorted: usize,
        /// The sort key as the user asked for it, e.g. `year`.
        sort: String,
        /// The page that was asked for.
        page: u32,
        /// The last page that holds anything.
        last: u32,
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
                "no results for {terms}\n\
                 try fewer or more general words — the catalogue matches whole words, not fragments"
            ),
            EmptyReason::NoHitsAtLocations { terms, locations } => {
                let locations = locations.join(", ");
                format!(
                    "no results for {terms} at {locations}\n\
                     the search was restricted to {locations} — name more libraries in --at, \
                     or drop it to ask the whole region"
                )
            }
            EmptyReason::NoHitsAnywhere { terms, locations } => {
                let locations = locations.join(", ");
                format!(
                    "no results for {terms} — not at {locations}, and nowhere else in the \
                     catalogue that answers for it\n\
                     the words are what came back empty there, not the restriction: try fewer or \
                     more general words, or ask the libraries the other catalogue answers for \
                     (--at HU, or no --at at all) — naming a sibling branch cannot help"
                )
            }
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
            EmptyReason::NothingAvailable {
                total,
                judged,
                unstated,
            } => nothing_available_message(*total, *judged, *unstated),
            EmptyReason::PastTheLastMatch {
                matched,
                filter,
                value,
                page,
                last,
            } => format!(
                "{} in the fetched window matched {filter} {value}, and page {page} begins \
                 after the last of them\n\
                 pages 1 to {last} hold them — a client-side filter only ever sees the \
                 window the catalogue delivered, so there is no page beyond it",
                counts::records(*matched)
            ),
            EmptyReason::PastTheLastSorted {
                sorted,
                sort,
                page,
                last,
            } => format!(
                "{} in the fetched window are what --sort {sort} put in order, and page {page} \
                 begins after the last of them\n\
                 pages 1 to {last} hold them — sorting needs one set to put in order, so the \
                 window is anchored and there is no page beyond it",
                counts::records(*sorted)
            ),
            EmptyReason::NoHoldings { locations } => {
                let locations = locations.join(", ");
                format!(
                    "hits exist, but none of them is held at {locations}\n\
                     run the same search without --at to see who else holds it"
                )
            }
            // The prefixes are named because an id whose prefix is none of them is
            // almost certainly not an id at all — and blibs cannot say so itself: an
            // unknown prefix is routed to KOBV on purpose (`model::id`), because sources
            // come and go and a fixed list of known ones would go stale. So the list is
            // offered as examples, never as the complete set.
            EmptyReason::NoSuchRecord { id } => format!(
                "no such record: {id}\n\
                 record ids come from the search output and carry the prefix of the source \
                 they came from — almafu_, almahu_, kobvindex_, gbv_, b3kat_ and voebb_ are \
                 among them; they also change when a record is merged upstream"
            ),
            // "the full list" was a dead end for the group most likely to read this:
            // `blibs libraries` names houses, and not one of the 211 branches, so the
            // advice has to name where a branch *is* listed as well — and since
            // `--branches` lists every branch of every house, it is the shorter answer
            // than picking the one house whose branches to list.
            EmptyReason::NoLibraryMatched { query } => format!(
                "no library matched {query:?}\n\
                 run `blibs libraries` for the houses, or `blibs libraries --branches` for \
                 every branch of every one of them"
            ),
        }
    }
}

/// The two counted cases of `--available`, phrased rather than interpolated: a single
/// page otherwise reads *"none of the 1 records"*, and one record without a statement
/// *"1 of them state no status at all"*.
///
/// A free function only because [`EmptyReason::message`] has an arm per reason and this is
/// the longest of them; `counts` holds the wording so that `render::human` says the same
/// sentence about the same numbers without `error` depending on `render`.
fn nothing_available_message(total: Option<u64>, judged: usize, unstated: usize) -> String {
    let page = counts::records(judged);
    let head = match total {
        Some(total) => format!(
            "{}, but none of the {page} on this page is in right now",
            counts::results(total)
        ),
        None => format!("none of the {page} on this page is in right now"),
    };
    let silent = if unstated > 0 {
        let state = if unstated == 1 { "states" } else { "state" };
        format!(", and {unstated} of them {state} no status at all")
    } else {
        String::new()
    };
    format!(
        "{head}{silent}\n\
         try a larger --limit, another --page, or drop --available"
    )
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

    /// Whether the document **arrived** and only its *shape* was wrong.
    ///
    /// The line a caller draws with this is the line between "one page of many was not
    /// the page this parser knows" and "the service is not talking to us". Only the
    /// former may ever be degraded into a [`crate::model::Note`] beside a result: a
    /// record whose detail page changed shape can lose its copies and keep its place in
    /// the list, while a timeout, a 429, a lost voebb.de session or a 500 must keep
    /// stopping the invocation. Degrading those would sell an outage as "status
    /// unknown", which is the one answer worse than an error.
    ///
    /// **True** for the three variants that say a delivered document lacked a structure
    /// this crate reads: [`UnexpectedError::MissingSelector`],
    /// [`UnexpectedError::MissingElement`] and [`UnexpectedError::CountMismatch`]. Each
    /// of them names what stopped matching, which is exactly what a note has to carry to
    /// be worth anything in a bug report.
    ///
    /// **False** for everything else, including two that look close and are not:
    /// [`UnexpectedError::NotXml`] and [`UnexpectedError::MalformedJson`] arrive when a
    /// proxy error page or an unreadable service answer comes back with status 200 —
    /// their own hints say "try again in a minute", so they are a disturbance in
    /// disguise, not a site that was redesigned. [`UnexpectedError::HttpStatus`],
    /// [`UnexpectedError::VoebbNoAccess`] and [`UnexpectedError::Output`] are transport
    /// and process failures outright.
    pub fn is_unreadable_document(&self) -> bool {
        matches!(
            self,
            Error::Unexpected(
                UnexpectedError::MissingSelector { .. }
                    | UnexpectedError::MissingElement { .. }
                    | UnexpectedError::CountMismatch { .. }
            )
        )
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
    /// A `<house>/<branch>` path fits more than one branch of that house.
    ///
    /// Never resolved by picking one: two branches of the same university are two
    /// different shelves, and guessing which was meant would answer a question nobody
    /// asked. The candidates carry their KOBV id, which is what makes the message
    /// actionable — every branch has one and it can be typed straight back.
    #[error("{input:?} fits {} branches of {house}", candidates.len())]
    AmbiguousBranch {
        /// What the user typed, path and all.
        input: String,
        /// How the house was named, echoed so the message reads like the input.
        house: String,
        /// Every branch the path fits, as `<kobvid> (<short name>)`.
        candidates: Vec<String>,
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
    ///
    /// The leading word is not always "ISBN": an 8-digit input is an ISSN, and a length
    /// that fits neither scheme cannot claim one — [`IsbnProblem::scheme_word`] picks it.
    #[error("invalid {} {input:?}: {problem}", .problem.scheme_word())]
    Isbn {
        /// What the user typed.
        input: String,
        /// Which part of the number is wrong, and for which scheme.
        problem: IsbnProblem,
    },
    /// `--language` was given something that is not a bibliographic language code.
    #[error(
        "--language takes a three-letter ISO-639-2/B code as the records carry it \
         (ger, eng, fre — not de and not German), got {input:?}"
    )]
    LanguageCode {
        /// What the user typed.
        input: String,
    },
    /// `--language` was given the ISO-639-2/**T** code of a language whose records carry
    /// the **B** variant — `deu` for `ger`, `fra` for `fre`, `zho` for `chi`.
    ///
    /// Its own variant, and its own `kind`, because the remedy is a different one:
    /// [`UsageError::LanguageCode`] says "that is not a code" and can only describe the
    /// shape of one, while this says "that is the wrong one of two codes" and knows
    /// which one was meant. An agent can substitute it without reading prose.
    ///
    /// It is a usage error rather than an empty result on purpose: the shape check passes
    /// for `deu`, so the filter used to run, match nothing, and exit 1 — "searched, does
    /// not exist" for what is really a typo, with advice that sends the reader off to
    /// narrow a search that was never the problem.
    #[error("--language {input:?} is the terminology code; the records carry {bibliographic:?}")]
    LanguageCodeVariant {
        /// The ISO-639-2/T code the user typed.
        input: String,
        /// The ISO-639-2/B code the records actually carry.
        bibliographic: String,
    },
    /// A flag, or a search term, was given a value with no text in it.
    ///
    /// Never dropped silently: an empty value almost always comes from a shell variable
    /// that did not expand, and the search that runs without it looks plausible while
    /// answering a different question than the one asked.
    #[error("{flag} was given an empty value")]
    EmptyValue {
        /// The flag as spelled on the command line — or `a search term` for a positional
        /// argument, which has no flag to name.
        flag: String,
    },
    /// A search word with not one letter or digit in it.
    ///
    /// Refused rather than sent, although it parses: the catalogue reads a term of pure
    /// punctuation as *no* term and answers with its whole index — `search '@and'`
    /// returned 8.55 million records, measured 2026-09-07 — which looks like a successful
    /// search and answers nothing.
    #[error("{term:?} contains no letters or digits")]
    TermWithoutText {
        /// The term as the user typed it.
        term: String,
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
    /// A flag that counts was given a negative number.
    ///
    /// Never left to clap: a leading `-` makes clap read the value as another flag, and
    /// the tip it prints then — *"to pass '-1' as a value, use '-- -1'"* — would turn the
    /// number into a **search term**, quietly answering a different question. The two
    /// counting flags already have first-class messages for `0` and `51`; this is the
    /// same mistake with a sign in front of it.
    #[error("{flag} must be a positive number, got {value}")]
    NegativeNumber {
        /// The flag as spelled on the command line.
        flag: String,
        /// The value as the user wrote it, sign and all.
        value: String,
    },
    /// `--page` must be 1-based.
    #[error("--page must be 1 or greater, got {value}")]
    PageOutOfRange {
        /// The value given.
        value: u32,
    },
    /// `--page` and `--limit` together name a window past where a catalogue can be paged.
    ///
    /// **Only the `voebb` engine reaches this, and that is a limit of what can be known
    /// locally rather than an inconsistency.** voebb.de stops at a fixed position, so the
    /// window can be measured against a constant before anything is sent — exit 2, in
    /// 0.01 s. The KOBV catalogue publishes no such ceiling: how deep a result set can be
    /// paged depends on the result set, so the same mistake can only be found out by
    /// asking, and comes back as [`RejectedError::Diagnostic`] `1/61` — exit 5. One user
    /// mistake, two exit codes, because one of them is a refusal and the other an answer.
    #[error(
        "--page {page} with --limit {limit} reaches result {position}, past where the \
         {engine} catalogue can be paged"
    )]
    WindowTooDeep {
        /// The catalogue that stops there.
        engine: Engine,
        /// `--page` as given.
        page: u32,
        /// `--limit` as given, or its default.
        limit: u32,
        /// The last result the window would need.
        position: u32,
        /// The last result the catalogue serves.
        max: u32,
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
    /// Two flags that cancel each other out, so that honouring both is impossible
    /// rather than merely odd. The pair is data: the next such conflict is one call
    /// site, not a second variant.
    #[error("{flag} cannot be combined with {with}")]
    ConflictingFlags {
        /// The flag the user asked for, spelled as on the command line.
        flag: String,
        /// The flag that contradicts it.
        with: String,
    },
    /// `--near` was given free text; the tool geocodes nothing.
    #[error("cannot locate {input:?}")]
    NearNeedsCoordinates {
        /// What the user typed.
        input: String,
    },
    /// `--near` was given two numbers that are not a place on the earth.
    ///
    /// Its own variant because "give coordinates" is absurd advice for someone who gave
    /// coordinates: what they need is the range they left.
    #[error("--near {input:?} is not a point on the earth")]
    NearCoordinatesOutOfRange {
        /// What the user typed.
        input: String,
    },
    /// `--near` was given one number where a pair is needed — `52.52`, or `52.52,`.
    #[error("--near needs a latitude and a longitude, got {input:?}")]
    NearNeedsTwoValues {
        /// What the user typed.
        input: String,
    },
    /// A record id that does not carry a source prefix.
    #[error("invalid record id {input:?}")]
    RecordId {
        /// What the user typed.
        input: String,
    },
    /// A value the advertised list allows and the tables in `cli::validate` do not.
    ///
    /// Reachable only if the two drift apart, which the tests there prevent — so it reads
    /// as a bug report, not as advice. It is still an error rather than a panic: nothing
    /// on a path reachable from user input may panic.
    #[error("{flag} does not accept {got:?}; expected one of {expected}")]
    UnsupportedValue {
        /// The flag, spelled as on the command line.
        flag: String,
        /// The value that arrived.
        got: String,
        /// The values the tables know, comma-separated.
        expected: String,
    },
    /// Anything clap itself rejected — unknown flag, unknown subcommand, missing value.
    /// The three commonest mistakes an agent makes, so this is the variant it reads most.
    ///
    /// Only ever built in `main`, from a real parse failure; nothing inside the library
    /// raises it. In **human** form it is never rendered at all: clap prints its own
    /// text, because only clap knows which help to point at. In **JSON** form clap prints
    /// nothing, so this object is the entire answer — and it therefore may not be clap's
    /// terminal rendering poured into a data field. `Display` is clap's first line, its
    /// summary, without the `error: ` prefix that belongs to the terminal; what was
    /// actionable in the rest of that rendering comes back as a hint.
    #[error("{}", clap_message(.0))]
    Cli(#[from] clap::Error),
}

impl UsageError {
    /// Machine-readable tag, unique across the whole [`Error`] enum.
    pub fn kind(&self) -> &'static str {
        match self {
            UsageError::UnknownLibrary { .. } => "unknown_library",
            UsageError::AmbiguousBranch { .. } => "ambiguous_branch",
            UsageError::Wildcard { .. } => "wildcard_unsupported",
            UsageError::Range { .. } => "range_unsupported",
            UsageError::YearFormat { .. } => "invalid_year",
            UsageError::Isbn { .. } => "invalid_isbn",
            UsageError::LanguageCode { .. } => "invalid_language",
            UsageError::LanguageCodeVariant { .. } => "language_code_variant",
            UsageError::TermWithoutText { .. } => "term_without_text",
            UsageError::EmptyValue { .. } => "empty_value",
            UsageError::EmptyQuery => "empty_query",
            UsageError::QueryTooLong { .. } => "query_too_long",
            UsageError::LimitOutOfRange { .. } => "limit_out_of_range",
            UsageError::NegativeNumber { .. } => "negative_number",
            UsageError::PageOutOfRange { .. } => "page_out_of_range",
            UsageError::WindowTooDeep { .. } => "window_too_deep",
            UsageError::FlagUnsupportedByEngine { .. } => "unsupported_by_engine",
            UsageError::ConflictingFlags { .. } => "conflicting_flags",
            UsageError::NearNeedsCoordinates { .. } => "near_needs_coordinates",
            UsageError::NearCoordinatesOutOfRange { .. } => "near_coordinates_out_of_range",
            UsageError::NearNeedsTwoValues { .. } => "near_needs_two_values",
            UsageError::RecordId { .. } => "invalid_record_id",
            UsageError::UnsupportedValue { .. } => "unsupported_value",
            UsageError::Cli(_) => "usage",
        }
    }

    /// The way out of this mistake. Never `None`: every one of these is something the
    /// user typed, so there is always a next step to name — including for
    /// [`UsageError::Cli`], which used to have none on the grounds that clap had printed
    /// a usage line already. Under `--json` clap prints nothing at all, and that left the
    /// three commonest agent mistakes with no advice whatsoever.
    ///
    /// The signature stays `Option` because [`Error::hint`] is one function over five
    /// categories and the JSON object's `hint` member is nullable by contract.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            UsageError::UnknownLibrary { suggestions, .. } => unknown_library_hint(suggestions),
            UsageError::AmbiguousBranch { candidates, .. } => {
                format!("name one of them: {}", candidates.join(", "))
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
            UsageError::LanguageCode { .. } => {
                "the records carry bibliographic codes, not the everyday ones: \
                 --language ger for German, eng for English, fre for French"
                    .to_string()
            }
            UsageError::LanguageCodeVariant { bibliographic, .. } => format!(
                "use --language {bibliographic} — MARC carries the bibliographic code, \
                 which differs from the terminology one for about twenty languages"
            ),
            UsageError::TermWithoutText { .. } => {
                "the catalogue reads a term without letters or digits as no term at all and \
                 answers with millions of unrelated records — give at least one word, \
                 e.g. `blibs search Kafka`"
                    .to_string()
            }
            UsageError::EmptyValue { flag } => format!(
                "an empty value is almost always a shell variable that did not expand — \
                 check what {flag} was given, or leave it out"
            ),
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
            UsageError::NegativeNumber { flag, value } => format!(
                "{flag} counts upward from 1, e.g. {flag} 10 — and never as `-- {value}`, \
                 which would search for {value} instead"
            ),
            UsageError::PageOutOfRange { .. } => {
                "pages are 1-based: the first page is --page 1".to_string()
            }
            UsageError::WindowTooDeep { engine, max, .. } => format!(
                "{engine} paging stops at position {max}; narrow the search, or leave the \
                 {engine} locations out of --at so the other catalogue answers"
            ),
            UsageError::FlagUnsupportedByEngine { flag, engine, .. } => format!(
                "drop {flag}, or leave the {engine} locations out of --at so the other \
                 catalogue answers"
            ),
            UsageError::ConflictingFlags { flag, with } => {
                format!("drop one of the two: {flag} asks for an answer that {with} switches off")
            }
            UsageError::NearNeedsCoordinates { .. } => {
                "blibs looks up no addresses — give coordinates (--near 52.52,13.41) \
                 or a library shortcode (--near HU)"
                    .to_string()
            }
            UsageError::NearCoordinatesOutOfRange { .. } => {
                "latitude runs from -90 to 90 and longitude from -180 to 180, \
                 in that order — Berlin is around --near 52.52,13.41"
                    .to_string()
            }
            UsageError::NearNeedsTwoValues { .. } => {
                "separate the two with a comma, as in --near 52.52,13.41 — \
                 or name a library instead, as in --near HU"
                    .to_string()
            }
            UsageError::RecordId { .. } => {
                "a record id carries its catalogue prefix, e.g. almafu_BV008885798 or \
                 voebb_SAK13776205 — copy it from the search output"
                    .to_string()
            }
            UsageError::UnsupportedValue { .. } => format!(
                "blibs advertises a value its own tables do not accept — that is a bug in \
                 blibs, please report it at {REPORT_URL}"
            ),
            UsageError::Cli(error) => clap_hint(error),
        };
        Some(hint)
    }
}

/// The way out of an unresolvable `--at`, avoiding two dead ends that were both measured.
///
/// `blibs libraries` prints the 123 houses and not one of the 211 branches, so an advice
/// line that calls it "the full list" strands exactly the reader who typed a branch name.
/// `--branches` is the listing that has them all, which is why it is named here rather
/// than one house's own branches.
/// And `--at` splits on commas, so the 63 branch short names that carry one cannot be
/// written out at all: what arrives here is *half* a name, and accusing that half without
/// naming the comma sends the user hunting for a typo that is not there. The escape has to
/// be named with it, because no spelling of the name itself can work.
///
/// The comma line is held back when the list produced a near miss: there the suggestion is
/// the answer, and a second paragraph would bury it.
fn unknown_library_hint(suggestions: &[String]) -> String {
    let lookup = "run `blibs libraries --find <name>` to look one up, or \
                  `blibs libraries --branches` to list every branch";
    if suggestions.is_empty() {
        format!(
            "{lookup}\n--at splits on commas, so a branch whose name carries one has to be \
             given by its KOBV id instead, e.g. BIB000000240"
        )
    } else {
        format!("did you mean {}? {lookup}", suggestions.join(", "))
    }
}

/// clap's own summary of what went wrong: the first line of its rendering, without the
/// `error: ` prefix that belongs to a terminal rather than to a data field.
///
/// The rest of that rendering is dropped on purpose. The usage line reads as a list of
/// *required* arguments — `Usage: blibs search --at <LIST> <TERMS>...` announces `--at`
/// as mandatory, which it is not — and "For more information, try '--help'" is advice no
/// agent can act on. What is genuinely useful in it comes back through [`clap_hint`].
fn clap_message(error: &clap::Error) -> String {
    let rendered = error.render().to_string();
    let first = rendered.lines().next().unwrap_or_default().trim();
    let message = first.strip_prefix("error: ").unwrap_or(first).trim();
    if message.is_empty() {
        // clap always renders a summary; an empty `message` would leave the JSON object
        // saying nothing at all, which is worse than saying something generic.
        return "the command line could not be parsed".to_string();
    }
    message.to_string()
}

/// Which help to run, plus clap's own suggestion when it made one.
///
/// clap's `SuggestedArg`/`SuggestedSubcommand` are kept, because *"a similar argument
/// exists: '--at'"* is the most useful sentence in the whole rendering and it names the
/// fix outright. `ContextKind::Suggested` is deliberately **not**: for a negative number
/// it says *"to pass '-1' as a value, use '-- -1'"*, which would turn a mistyped `--limit`
/// into a search term — see [`UsageError::NegativeNumber`], which catches those before
/// clap sees them.
fn clap_hint(error: &clap::Error) -> String {
    let help = format!(
        "run `{} --help` to see what it accepts",
        clap_command_path(error)
    );
    match clap_suggestion(error) {
        Some(suggestion) => format!("did you mean {suggestion}? {help}"),
        None => help,
    }
}

/// The command the failure happened in — `blibs search` for a flag inside `search`,
/// `blibs` for an unknown subcommand — read off clap's usage line, the only place that
/// knows it. Everything from the first `-`, `<` or `[` on describes the *shape* of the
/// call rather than its name, so the name ends there. A clap error without a usage
/// context (a hand-built one, or a missing value) falls back to the top-level help
/// instead of guessing.
fn clap_command_path(error: &clap::Error) -> String {
    let Some(usage) = error.get(ContextKind::Usage).map(ToString::to_string) else {
        return "blibs".to_string();
    };
    let line = usage.lines().next().unwrap_or_default();
    let line = line.trim().strip_prefix("Usage:").unwrap_or(line).trim();
    let path: Vec<&str> = line
        .split_whitespace()
        .take_while(|word| !word.starts_with(['-', '<', '[']))
        .collect();
    if path.is_empty() {
        "blibs".to_string()
    } else {
        path.join(" ")
    }
}

/// The near miss clap found, if it found one — a subcommand first, because an unknown
/// subcommand is the coarser mistake of the two.
fn clap_suggestion(error: &clap::Error) -> Option<String> {
    [ContextKind::SuggestedSubcommand, ContextKind::SuggestedArg]
        .into_iter()
        .filter_map(|kind| error.get(kind))
        .map(ToString::to_string)
        .find(|text| !text.trim().is_empty())
        .map(|text| text.trim().to_string())
}

/// Which numbering scheme an [`IsbnProblem`] was found against.
///
/// The length decides it: 8 digits is an ISSN, 10 or 13 is an ISBN. A length that fits
/// neither carries no scheme at all — that is why [`IsbnProblem::Characters`] holds an
/// `Option`, and a message about it must not pick one anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsbnScheme {
    /// 8 digits, modulo 11.
    Issn,
    /// 10 digits (modulo 11) or 13 digits (modulo 10).
    Isbn,
}

impl IsbnScheme {
    /// The scheme a number of this length belongs to, or `None` if it belongs to neither.
    pub fn from_len(len: usize) -> Option<Self> {
        match len {
            8 => Some(IsbnScheme::Issn),
            10 | 13 => Some(IsbnScheme::Isbn),
            _ => None,
        }
    }

    /// The name to put in front of "invalid ... " and in a scheme-specific hint.
    fn name(self) -> &'static str {
        match self {
            IsbnScheme::Issn => "ISSN",
            IsbnScheme::Isbn => "ISBN",
        }
    }

    /// What the scheme identifies, for the check-digit hint's "wrong ___" wording — an
    /// ISSN is checked out under the same catalogue as a serial, not "a book".
    fn identifies(self) -> &'static str {
        match self {
            IsbnScheme::Issn => "serial",
            IsbnScheme::Isbn => "book",
        }
    }
}

/// Which part of an ISBN or ISSN failed validation. Structured so that the hint can name
/// it — and, for the dangerous case, name the digit that was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsbnProblem {
    /// Not 8, 10 or 13 digits after stripping hyphens and spaces — too short or too long
    /// to be any scheme, so no scheme is named.
    Length {
        /// How many digits were left after stripping.
        digits: usize,
    },
    /// A character that is neither a digit nor a trailing `X`.
    Characters {
        /// The first offending character.
        found: char,
        /// The scheme implied by the input's length, if it matches one.
        scheme: Option<IsbnScheme>,
    },
    /// The check digit does not match the rest — the dangerous case, because the index
    /// ignores it and would return a different record.
    CheckDigit {
        /// The digit the rest of the number implies.
        expected: char,
        /// The digit the user typed.
        found: char,
        /// The scheme the check was performed under; always known here, because the
        /// check only runs once the length has matched one.
        scheme: IsbnScheme,
    },
}

impl IsbnProblem {
    /// The word to put in front of "invalid ... " — the scheme this problem was found
    /// under, or "identifier" where the length fits neither.
    fn scheme_word(&self) -> &'static str {
        match self {
            IsbnProblem::Length { .. } => "identifier",
            IsbnProblem::Characters { scheme, .. } => scheme.map_or("identifier", IsbnScheme::name),
            IsbnProblem::CheckDigit { scheme, .. } => scheme.name(),
        }
    }

    /// What to do about it. The check-digit case explains *why* this is refused instead
    /// of sent: `dc.identifier` drops the check digit, so a typo finds the wrong record
    /// rather than none.
    pub fn hint(self) -> String {
        match self {
            IsbnProblem::Length { .. } => {
                "an ISSN has 8 digits, an ISBN has 10 or 13; hyphens and spaces are fine"
                    .to_string()
            }
            IsbnProblem::Characters { scheme, .. } => match scheme {
                Some(IsbnScheme::Issn) => {
                    "an ISSN is digits only, except for a trailing X".to_string()
                }
                Some(IsbnScheme::Isbn) => {
                    "an ISBN is digits only, except for a trailing X in an ISBN-10".to_string()
                }
                None => "an ISBN or ISSN is digits only, except for a trailing X in an \
                          ISBN-10 or ISSN"
                    .to_string(),
            },
            IsbnProblem::CheckDigit {
                expected, scheme, ..
            } => {
                let what = scheme.identifies();
                format!(
                    "the catalogue ignores the check digit, so a typo would quietly \
                     return the wrong {what} — check the digits; as typed, the last one \
                     would have to be {expected}"
                )
            }
        }
    }
}

impl fmt::Display for IsbnProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IsbnProblem::Length { digits } => {
                write!(f, "must be 8, 10 or 13 digits, got {digits}")
            }
            IsbnProblem::Characters { found, .. } => {
                write!(f, "contains {found:?}, which is not a digit")
            }
            IsbnProblem::CheckDigit {
                expected, found, ..
            } => {
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
    ///
    /// Diagnostic `1/61` is the KOBV half of the boundary documented on
    /// [`UsageError::WindowTooDeep`]: a `--page` past the end of the result set is exit 2
    /// for voebb, whose ceiling is a constant blibs can check locally, and exit 5 here,
    /// where only the catalogue knows where its result set ends. Nothing is wrong with
    /// the words in that case — the window is — and the hint says so.
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
                // Nothing about the *words* is wrong here — the window is. Sending the
                // user after simpler search words would have them rewrite a query that
                // was fine.
                "1/61" => "the window begins past the last result — lower --page, \
                           or widen the search so there is more to page through"
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

/// One constructed example of every variant in the enum, split by category only so that
/// no part outgrows a readable function. The list is the test: a new variant that is
/// not added here fails the uniqueness and hint checks in this module.
///
/// It sits outside `mod tests` so that `render` can hold its renderers to the same list.
/// The doubled `error:` prefix on `--language de` survived a release because no test ever
/// rendered an error at all, and a list of variants only the wording tests can see would
/// have left that gap open.
#[cfg(test)]
pub fn every_variant_for_tests() -> Vec<Error> {
    let mut all = tests::usage_variants();
    all.extend(tests::query_variants());
    all.extend(tests::remote_variants());
    all
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn all_variants() -> Vec<Error> {
        every_variant_for_tests()
    }

    /// What the user can get wrong about *where* to search and *how much* of it to
    /// fetch — the flags, the window, the library list.
    pub(super) fn usage_variants() -> Vec<Error> {
        vec![
            UsageError::UnknownLibrary {
                input: "STABI2".to_string(),
                suggestions: vec!["STABI".to_string(), "SBB".to_string()],
            }
            .into(),
            UsageError::AmbiguousBranch {
                input: "HU/Zweigbibliothek".to_string(),
                house: "HU".to_string(),
                candidates: vec![
                    "HUB00043 (Zweigbibliothek Germanistik/Skandinavistik)".to_string(),
                ],
            }
            .into(),
            UsageError::LimitOutOfRange { value: 200 }.into(),
            UsageError::PageOutOfRange { value: 0 }.into(),
            UsageError::WindowTooDeep {
                engine: Engine::Voebb,
                page: 30,
                limit: 10,
                position: 300,
                max: 220,
            }
            .into(),
            UsageError::FlagUnsupportedByEngine {
                flag: "--publisher".to_string(),
                engine: Engine::Voebb,
            }
            .into(),
            UsageError::ConflictingFlags {
                flag: "--available".to_string(),
                with: "--no-availability".to_string(),
            }
            .into(),
            UsageError::NearNeedsCoordinates {
                input: "Alexanderplatz 1".to_string(),
            }
            .into(),
            UsageError::NegativeNumber {
                flag: "--limit".to_string(),
                value: "-1".to_string(),
            }
            .into(),
            UsageError::NearCoordinatesOutOfRange {
                input: "91,181".to_string(),
            }
            .into(),
            UsageError::NearNeedsTwoValues {
                input: "52.52".to_string(),
            }
            .into(),
            UsageError::UnsupportedValue {
                flag: "--sort".to_string(),
                got: "nonsense".to_string(),
                expected: "relevance, year, title".to_string(),
            }
            .into(),
            UsageError::Cli(clap::Error::raw(
                clap::error::ErrorKind::UnknownArgument,
                "unexpected argument '--nope' found",
            ))
            .into(),
        ]
    }

    /// What the user can get wrong about *what* to search for — the words, the codes,
    /// the identifiers.
    pub(super) fn query_variants() -> Vec<Error> {
        vec![
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
                    scheme: IsbnScheme::Isbn,
                },
            }
            .into(),
            UsageError::EmptyQuery.into(),
            UsageError::QueryTooLong { chars: 2400 }.into(),
            UsageError::RecordId {
                input: "BV008885798".to_string(),
            }
            .into(),
            UsageError::LanguageCode {
                input: "de".to_string(),
            }
            .into(),
            UsageError::LanguageCodeVariant {
                input: "deu".to_string(),
                bibliographic: "ger".to_string(),
            }
            .into(),
            UsageError::TermWithoutText {
                term: "@and".to_string(),
            }
            .into(),
            UsageError::EmptyValue {
                flag: "--at".to_string(),
            }
            .into(),
        ]
    }

    /// Everything that can go wrong once a request is on the wire.
    pub(super) fn remote_variants() -> Vec<Error> {
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

    /// Was `every_variant_but_clap_says_what_to_do`, which asserted that
    /// [`UsageError::Cli`] has **no** hint — true only while one believed clap had
    /// printed a usage line already. Under `--json` clap prints nothing, so that
    /// exemption left the three commonest agent mistakes without a next step.
    #[test]
    fn every_variant_says_what_to_do() {
        for error in all_variants() {
            let hint = error.hint().unwrap_or_default();
            assert!(!hint.trim().is_empty(), "no hint for {error:?}");
        }
    }

    /// The line [`Error::is_unreadable_document`] draws, checked over **every** variant:
    /// exactly three of them may ever become a note beside a result, and every other one
    /// — a timeout, a 429, a lost voebb.de session — has to keep stopping the
    /// invocation. A new variant lands in the `false` half by default, which is the safe
    /// half.
    #[test]
    fn only_a_document_that_changed_shape_may_degrade_to_a_note() {
        let degrades: BTreeSet<&str> = all_variants()
            .iter()
            .filter(|error| error.is_unreadable_document())
            .map(Error::kind)
            .collect();
        assert_eq!(
            degrades,
            BTreeSet::from(["missing_selector", "missing_structure", "count_mismatch"]),
            "a transport failure must never be sold as \"status unknown\""
        );
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
    ///
    /// The `hint` moved with §1.11/§3.7: it used to say "run `blibs libraries --find
    /// <name>` to look up a library", a pointer into a list that contains no branch, and
    /// it never mentioned that `--at` splits on commas. It moved once more in phase 6:
    /// `blibs libraries VOEBB` listed the branches of *one* house and left the reader to
    /// guess which, while `--branches` — which did not exist when the first wording was
    /// written — lists every branch of every house. `plan/cli.md` carries the old wording
    /// and is corrected in the same phase.
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
            r#"{"error":{"code":2,"kind":"unknown_library","message":"unknown library \"STABI2\"","hint":"run `blibs libraries --find <name>` to look one up, or `blibs libraries --branches` to list every branch\n--at splits on commas, so a branch whose name carries one has to be given by its KOBV id instead, e.g. BIB000000240"}}"#
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
                "did you mean STABI, SBB? run `blibs libraries --find <name>` to look one up, \
                 or `blibs libraries --branches` to list every branch"
            )
        );
    }

    /// §3.7: 63 of the 211 branch short names carry a comma, and `--at` splits on commas,
    /// so what reaches this error is half a name. Blaming that half without naming the
    /// comma sends the user after a typo that is not there — and the escape has to be
    /// named too, because no spelling of the name itself can work.
    ///
    /// A near miss outranks it: there the suggestion is the answer, and a second
    /// paragraph would bury it.
    #[test]
    fn an_unknown_library_names_the_comma_when_nothing_came_close() {
        let stranded = UsageError::UnknownLibrary {
            input: "Bibliothek am Schäfersee".to_string(),
            suggestions: Vec::new(),
        }
        .hint()
        .unwrap_or_default();
        assert!(stranded.contains("splits on commas"), "{stranded}");
        assert!(stranded.contains("KOBV id"), "{stranded}");

        let near_miss = UsageError::UnknownLibrary {
            input: "STABI2".to_string(),
            suggestions: vec!["STABI".to_string()],
        }
        .hint()
        .unwrap_or_default();
        assert!(near_miss.starts_with("did you mean STABI?"), "{near_miss}");
        assert!(!near_miss.contains("splits on commas"), "{near_miss}");
    }

    /// §1.11: neither advice line may claim that `blibs libraries` shows everything —
    /// it shows the 123 houses and none of the 211 branches.
    #[test]
    fn no_advice_line_calls_the_house_list_complete() {
        let unknown = UsageError::UnknownLibrary {
            input: "Frohnau".to_string(),
            suggestions: Vec::new(),
        }
        .hint()
        .unwrap_or_default();
        let unmatched = EmptyReason::NoLibraryMatched {
            query: "Frohnau".to_string(),
        }
        .message();
        for text in [&unknown, &unmatched] {
            assert!(!text.contains("full list"), "{text}");
            assert!(text.contains("branches"), "{text}");
        }
    }

    /// Was `a_missing_hint_serialises_as_null`, built from a [`UsageError::Cli`] back
    /// when that was the one variant without a hint. It has one now, and no variant is
    /// left that has none — but the member stays nullable by contract, so the shape is
    /// pinned directly instead of through a variant that no longer produces it.
    #[test]
    fn an_absent_hint_serialises_as_null_rather_than_disappearing() {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: 2,
                kind: "usage",
                message: "unexpected argument '--quatsch' found".to_string(),
                hint: None,
            },
        };
        let value = serde_json::to_value(&envelope)
            .expect("the error envelope contains only strings and a number");
        assert_eq!(value["error"]["hint"], serde_json::Value::Null);
        assert!(
            value["error"]
                .as_object()
                .is_some_and(|o| o.contains_key("hint"))
        );
    }

    /// §2.4: under `--json` clap prints nothing, so this object is the whole answer. It
    /// used to carry clap's entire terminal rendering — prefix, blank line, a usage line
    /// announcing `--at` as mandatory, and "try '--help'" — inside `message`.
    #[test]
    fn a_clap_refusal_is_one_line_with_the_pointer_in_the_hint() {
        let refusal = clap::Error::raw(
            clap::error::ErrorKind::UnknownArgument,
            "unexpected argument '--quatsch' found\n\n  tip: a similar argument exists: \
             '--at'\n\nUsage: blibs search --at <LIST> <TERMS>...\n\nFor more information, \
             try '--help'.\n",
        );
        let error: Error = UsageError::Cli(refusal).into();
        assert_eq!(error.kind(), "usage");
        assert_eq!(error.exit(), ExitCode::Usage);
        let message = error.to_string();
        assert_eq!(message, "unexpected argument '--quatsch' found");
        assert!(!message.contains("error:"), "{message}");
        assert!(!message.contains('\n'), "{message}");
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("--help"), "{hint}");
    }

    /// The usage line is the only place that knows *which* help to point at, and it is
    /// also the line that must not reach `message`.
    #[test]
    fn the_hint_points_at_the_help_of_the_command_that_failed() {
        assert_eq!(clap_command_path(&raw_with_usage("")), "blibs");
        assert_eq!(
            clap_command_path(&raw_with_usage(
                "Usage: blibs search --at <LIST> <TERMS>..."
            )),
            "blibs search"
        );
        assert_eq!(
            clap_command_path(&raw_with_usage("Usage: blibs [OPTIONS] [COMMAND]")),
            "blibs"
        );
        assert_eq!(
            clap_command_path(&raw_with_usage("Usage: blibs libraries [OPTIONS] [NAME]")),
            "blibs libraries"
        );
    }

    /// A hand-built clap error carries no context at all; the hint must still name a
    /// help that exists rather than an empty command.
    #[test]
    fn a_clap_error_without_context_still_points_somewhere() {
        let error: Error = UsageError::Cli(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            "a value is required for '--at <LIST>' but none was supplied",
        ))
        .into();
        assert_eq!(
            error.hint().as_deref(),
            Some("run `blibs --help` to see what it accepts")
        );
    }

    fn raw_with_usage(usage: &str) -> clap::Error {
        let mut error = clap::Error::raw(clap::error::ErrorKind::UnknownArgument, "boom");
        if !usage.is_empty() {
            error.insert(
                ContextKind::Usage,
                clap::error::ContextValue::String(usage.to_string()),
            );
        }
        error
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
            ("info:srw/diagnostic/1/61", "lower --page"),
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
                scheme: IsbnScheme::Isbn,
            },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid ISBN \"978-3-596-29433-4\": check digit is 4, expected 1"
        );
        assert!(error.hint().unwrap_or_default().contains("be 1"));
    }

    /// An ISSN's check digit failure must say ISSN, not ISBN — the earlier bug named the
    /// wrong scheme even though the digit itself was computed by the right rule.
    #[test]
    fn an_issn_check_digit_message_says_issn_not_isbn() {
        let error: Error = UsageError::Isbn {
            input: "0002-9549".to_string(),
            problem: IsbnProblem::CheckDigit {
                expected: '8',
                found: '9',
                scheme: IsbnScheme::Issn,
            },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid ISSN \"0002-9549\": check digit is 9, expected 8"
        );
        assert!(error.hint().unwrap_or_default().contains("be 8"));
    }

    /// 9 digits is too long for an ISSN and too short for an ISBN: the message must not
    /// pick a scheme, and must state every length the tool accepts, not just two of them.
    #[test]
    fn a_length_that_fits_no_scheme_names_none_and_lists_all_three_lengths() {
        let error: Error = UsageError::Isbn {
            input: "123456789".to_string(),
            problem: IsbnProblem::Length { digits: 9 },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid identifier \"123456789\": must be 8, 10 or 13 digits, got 9"
        );
        let hint = error.hint().unwrap_or_default();
        assert!(
            hint.contains('8') && hint.contains("10") && hint.contains("13"),
            "{hint}"
        );
    }

    /// A bad character in an 8-digit input is still recognisable as an ISSN attempt, and
    /// the message says so.
    #[test]
    fn a_bad_character_in_an_eight_digit_input_says_issn() {
        let error: Error = UsageError::Isbn {
            input: "0002-95X9".to_string(),
            problem: IsbnProblem::Characters {
                found: 'X',
                scheme: Some(IsbnScheme::Issn),
            },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid ISSN \"0002-95X9\": contains 'X', which is not a digit"
        );
    }

    /// A bad character in an input whose length fits no scheme cannot claim one either.
    #[test]
    fn a_bad_character_with_no_matching_length_names_no_scheme() {
        let error: Error = UsageError::Isbn {
            input: "Kafka".to_string(),
            problem: IsbnProblem::Characters {
                found: 'K',
                scheme: None,
            },
        }
        .into();
        assert_eq!(
            error.to_string(),
            "invalid identifier \"Kafka\": contains 'K', which is not a digit"
        );
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

    /// The two counted cases of `--available` are the point of the variant: "on loan"
    /// and "nothing was said" must stay apart, and an empty result renders no notes, so
    /// the second one has to survive in the message itself.
    #[test]
    fn nothing_available_names_the_records_that_stated_no_status() {
        assert_eq!(
            EmptyReason::NothingAvailable {
                total: Some(774),
                judged: 10,
                unstated: 3,
            }
            .message(),
            "774 results, but none of the 10 records on this page is in right now, \
             and 3 of them state no status at all\n\
             try a larger --limit, another --page, or drop --available"
        );
        assert_eq!(
            EmptyReason::NothingAvailable {
                total: None,
                judged: 4,
                unstated: 0,
            }
            .message(),
            "none of the 4 records on this page is in right now\n\
             try a larger --limit, another --page, or drop --available"
        );
    }

    /// One is one. A `--limit 1` page otherwise reads "none of the 1 records on this
    /// page", and a single record without a statement "1 of them state no status at
    /// all" — two grammatical errors in the sentence a user only ever sees when the
    /// answer disappointed them.
    #[test]
    fn a_single_record_is_not_counted_in_the_plural() {
        assert_eq!(
            EmptyReason::NothingAvailable {
                total: Some(1),
                judged: 1,
                unstated: 1,
            }
            .message(),
            "1 result, but none of the 1 record on this page is in right now, \
             and 1 of them states no status at all\n\
             try a larger --limit, another --page, or drop --available"
        );
        // The plural of the documented example is untouched.
        assert!(
            EmptyReason::NothingAvailable {
                total: Some(774),
                judged: 10,
                unstated: 3,
            }
            .message()
            .starts_with(
                "774 results, but none of the 10 records on this page is in right now, \
                 and 3 of them state no status at all"
            )
        );
    }

    /// The advice is the whole reason this is not `FilteredOut`: `--available` runs
    /// after the page was cut, so a narrower search would not bring a copy back in.
    #[test]
    fn nothing_available_advises_a_larger_page_not_a_narrower_search() {
        let message = EmptyReason::NothingAvailable {
            total: Some(6),
            judged: 6,
            unstated: 1,
        }
        .message();
        assert!(message.contains("try a larger --limit"));
        assert!(!message.contains("narrow the search"));
    }

    #[test]
    fn every_empty_reason_has_a_first_line_of_its_own() {
        let reasons = [
            EmptyReason::NoHits {
                terms: "Kafka Prozess".to_string(),
            },
            EmptyReason::NoHitsAtLocations {
                terms: "Kafka Prozess".to_string(),
                locations: vec!["ASH".to_string()],
            },
            EmptyReason::NoHitsAnywhere {
                terms: "Xylophonquark".to_string(),
                locations: vec!["AGB".to_string()],
            },
            EmptyReason::FilteredOut {
                total: None,
                fetched: 10,
                filter: "--language".to_string(),
                value: "fre".to_string(),
            },
            EmptyReason::NothingAvailable {
                total: Some(774),
                judged: 10,
                unstated: 0,
            },
            EmptyReason::PastTheLastMatch {
                matched: 26,
                filter: "--format".to_string(),
                value: "book".to_string(),
                page: 7,
                last: 6,
            },
            EmptyReason::PastTheLastSorted {
                sorted: 49,
                sort: "year".to_string(),
                page: 6,
                last: 5,
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
            Some("no results for Kafka Prozess")
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

    /// `--at ASH` finding nothing is not the catalogue having nothing, and "try fewer or
    /// more general words" would have the user weaken a search that may be exactly right.
    /// What it must **not** do is claim the title exists elsewhere: `--at` filters
    /// upstream, so nothing here ever counted the unrestricted search.
    #[test]
    fn an_empty_at_search_blames_the_restriction_and_claims_nothing_more() {
        let message = EmptyReason::NoHitsAtLocations {
            terms: "Versandhandelsmanagement".to_string(),
            locations: vec!["ASH".to_string()],
        }
        .message();
        assert!(message.starts_with("no results for Versandhandelsmanagement at ASH"));
        assert!(message.contains("--at"), "{message}");
        assert!(!message.contains("more general words"), "{message}");
        for claim in ["elsewhere", "exists", "held", "other libraries"] {
            assert!(!message.contains(claim), "{claim:?} claimed in: {message}");
        }
    }

    /// §1.2: when the unrestricted search found nothing either, `--at` was not the cause,
    /// and "name more libraries in --at" is advice that is guaranteed to fail again. The
    /// two reasons must therefore read differently, and only the one that knows a
    /// network-wide zero may say so.
    ///
    /// Changed in round 2 (4a): the message used to claim "not anywhere in the region" and
    /// that naming more libraries could not help. What the engine proved is one
    /// catalogue's zero — the VÖBB network's — and the region also holds the university
    /// and research libraries the other engine answers for, where `--at HU` can still
    /// find the book. The variant says less now, and the test pins that it says less.
    #[test]
    fn a_network_wide_zero_does_not_blame_the_location() {
        let anywhere = EmptyReason::NoHitsAnywhere {
            terms: "Xylophonquark Zwitscherbold".to_string(),
            locations: vec!["AGB".to_string()],
        }
        .message();
        assert!(
            anywhere.starts_with(
                "no results for Xylophonquark Zwitscherbold — not at AGB, and nowhere else in \
                 the catalogue that answers for it"
            ),
            "{anywhere}"
        );
        assert!(anywhere.contains("more general words"), "{anywhere}");
        assert!(!anywhere.contains("name more libraries"), "{anywhere}");
        assert!(
            !anywhere.contains("anywhere in the region"),
            "one catalogue answered, not the region: {anywhere}"
        );
        assert!(
            anywhere.contains("--at HU"),
            "the other catalogue is still worth asking: {anywhere}"
        );

        // The restricted sibling keeps saying the opposite, because there it is true.
        let at = EmptyReason::NoHitsAtLocations {
            terms: "Xylophonquark".to_string(),
            locations: vec!["AGB".to_string()],
        }
        .message();
        assert!(at.contains("name more libraries in --at"), "{at}");
    }

    /// The filtered wording is quoted approvingly in the round-2 report, so it is pinned
    /// byte for byte here. Its sorted sibling says the equivalent for the other reason a
    /// window is anchored, and must not borrow the words `filter` or `matched`: nothing
    /// was dropped, the records simply ran out.
    #[test]
    fn a_page_past_an_anchored_window_says_which_kind_of_anchor_it_was() {
        assert_eq!(
            EmptyReason::PastTheLastMatch {
                matched: 26,
                filter: "--format".to_string(),
                value: "book".to_string(),
                page: 7,
                last: 6,
            }
            .message(),
            "26 records in the fetched window matched --format book, and page 7 begins \
             after the last of them\n\
             pages 1 to 6 hold them — a client-side filter only ever sees the window the \
             catalogue delivered, so there is no page beyond it"
        );
        assert_eq!(
            EmptyReason::PastTheLastSorted {
                sorted: 49,
                sort: "year".to_string(),
                page: 6,
                last: 5,
            }
            .message(),
            "49 records in the fetched window are what --sort year put in order, and page 6 \
             begins after the last of them\n\
             pages 1 to 5 hold them — sorting needs one set to put in order, so the window \
             is anchored and there is no page beyond it"
        );
    }

    /// §1.7: `deu` is not a random string, it is the terminology code for the language
    /// whose records carry `ger`. The two language errors must not share a `kind`,
    /// because only one of them knows the answer.
    #[test]
    fn the_language_variant_names_the_code_that_was_meant() {
        let redirect: Error = UsageError::LanguageCodeVariant {
            input: "deu".to_string(),
            bibliographic: "ger".to_string(),
        }
        .into();
        assert_eq!(redirect.kind(), "language_code_variant");
        assert_eq!(redirect.exit(), ExitCode::Usage);
        assert!(redirect.to_string().contains("\"ger\""), "{redirect}");
        assert!(
            redirect
                .hint()
                .unwrap_or_default()
                .contains("--language ger"),
            "{redirect:?}"
        );
        let shape: Error = UsageError::LanguageCode {
            input: "de".to_string(),
        }
        .into();
        assert_ne!(redirect.kind(), shape.kind());
    }

    /// §3.7: clap's own advice for `--limit -1` is "to pass '-1' as a value, use '-- -1'",
    /// which would search for `-1`. The variant that replaces it must not repeat it.
    #[test]
    fn a_negative_count_is_never_offered_as_a_search_term() {
        let error: Error = UsageError::NegativeNumber {
            flag: "--limit".to_string(),
            value: "-1".to_string(),
        }
        .into();
        assert_eq!(error.kind(), "negative_number");
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("--limit 10"), "{hint}");
        assert!(hint.contains("never"), "{hint}");
    }

    /// A prefix that is none of these is almost certainly not an id at all — but the list
    /// cannot be closed: `model::id` routes an unknown prefix to KOBV on purpose, because
    /// sources come and go.
    #[test]
    fn no_such_record_names_the_prefixes_without_closing_the_list() {
        let message = EmptyReason::NoSuchRecord {
            id: RecordId::parse("foo_12345").expect("a prefixed id"),
        }
        .message();
        for prefix in [
            "almafu_",
            "almahu_",
            "kobvindex_",
            "gbv_",
            "b3kat_",
            "voebb_",
        ] {
            assert!(message.contains(prefix), "{prefix} missing from: {message}");
        }
        assert!(message.contains("among them"), "{message}");
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
