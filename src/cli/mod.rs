//! The command line: what can be asked, what is refused, and which engine answers.
//!
//! Everything the services cannot do is caught **here, before the first byte goes out**,
//! and always with exit 2 and a message that names the limit. That is not politeness: a
//! wildcard comes back as a diagnostic, but a range comes back as *zero hits, silently*,
//! and a mistyped ISBN comes back as **the wrong book** — the identifier index discards
//! the check digit. A query that fails loudly upstream is the good case.
//!
//! The engine is chosen exactly once, here: from `--at` for a search, from the id prefix
//! for `show`. Nothing downstream branches on it.
//!
//! There is no configuration file and no environment variable. "My libraries" is
//! `--at STABI,HU`, and a human writes a shell alias. An invocation is fully described by
//! its command line — the only form an agent can reproduce.
//!
//! ## The help text is part of the interface
//!
//! Every flag whose behaviour is surprising says so in its own long help: `--author` is
//! split into words, `--year` means "was running in this year", `--sort`/`--format`/
//! `--language` see only the fetched window, subject headings are mixed-language, and
//! `--near` geocodes nothing. Those sentences are not decoration — they are the only
//! place a user finds out *before* getting a wrong-looking answer.

pub mod isbn;
pub mod run;
pub mod validate;

use clap::builder::PossibleValuesParser;
use clap::{CommandFactory, Parser, Subcommand};

use crate::libraries::LatLon;
use crate::model::{
    AvailabilityMode, Engine, FetchWindow, Limit, Location, Page, QuerySpec, RecordId, SortKey,
};
use crate::select::{Filters, PageCut};

pub use run::run;
pub use validate::{validate, validate_libraries, validate_show};

/// Examples and the exit-code table, printed under the top-level help.
///
/// The exit codes are part of the public interface (`plan/cli.md`), and an agent that
/// reads only `--help` must find them there rather than in a repository somewhere.
const AFTER_HELP: &str = "\
Examples:
  blibs search Kafka Prozess
  blibs search \"Der Prozess\" --at HU,FU
  blibs search --author Kafka --year 1953 --format book
  blibs search --isbn 978-3-596-29433-6 --json
  blibs show almafu_BV008885798 --at STABI,HU
  blibs libraries --find grimm
  blibs libraries --near 52.52,13.39

Quoting is the whole trick: an argument in shell quotes is searched as a phrase,
an unquoted one as a word. There is no config file — \"my libraries\" is --at, and a
shell alias makes it short:

  alias bl='blibs search --at STABI,HU,AGB'

Exit codes:
  0  found at least one hit
  1  searched and found nothing; also an unknown record id or no library matched
  2  usage error — nothing was sent
  3  network error — DNS, TLS, connection refused, timeout
  4  service unavailable — HTTP 429/503, backoff exhausted
  5  the catalogue rejected the query
  6  unexpected response — HTTP 200 whose content failed the plausibility check\
";

/// Examples under `blibs search --help`.
const SEARCH_AFTER_HELP: &str = "\
Examples:
  blibs search Kafka Prozess                       two words, both must occur
  blibs search \"Der Prozess\"                       one phrase
  blibs search \"Der Vorleser\" --at HU,STABI,AGB    one block per location
  blibs search --author Kafka --year 1953          flags combine with AND
  blibs search --isbn 978-3-596-29433-6 --json     for scripts and agents

This catalogue cannot truncate, cannot search ranges, cannot sort, and has no index
for material type or language. blibs refuses the first two instead of sending them,
and says of the last two that they only saw the fetched records.\
";

/// Examples under `blibs show --help`.
const SHOW_AFTER_HELP: &str = "\
Examples:
  blibs show almafu_BV008885798
  blibs show almafu_BV008885798 --at STABI,HU
  blibs show voebb_SAK13776205 --json

The id comes from the search output and carries its catalogue prefix; that prefix
picks the catalogue, so ids are never interchangeable between the two.\
";

/// Examples under `blibs libraries --help`.
const LIBRARIES_AFTER_HELP: &str = "\
Examples:
  blibs libraries                    all of them, with short name, ISIL and city
  blibs libraries STABI              one house in detail
  blibs libraries --find grimm       search short names, names and cities
  blibs libraries --near HU          nearest first, measured from another library
  blibs libraries --near 52.52,13.39 nearest first, measured from a point\
";

/// Search the libraries of Berlin and Brandenburg.
#[derive(Debug, Parser)]
#[command(
    name = "blibs",
    version,
    about = "Search the libraries of Berlin and Brandenburg.",
    long_about = "Search the libraries of Berlin and Brandenburg: who holds a title, \
                  whether it is in right now, and where it is on the shelf.\n\n\
                  Two catalogues answer. The KOBV union catalogue covers every \
                  institution in the region — universities, research institutes, \
                  archives, museums and the public networks. voebb.de answers for a \
                  single branch of the Berlin public library network, which is the only \
                  place that knows which branch holds a copy. Which one runs follows \
                  from --at and from nothing else.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true,
    max_term_width = 100
)]
pub struct Cli {
    /// The command to run. `None` means the user typed `blibs` on its own, which prints
    /// the long help and exits 0 — an empty invocation practically always means "show me
    /// the documentation".
    #[command(subcommand)]
    pub command: Option<Command>,

    /// One JSON document on stdout and nothing else.
    ///
    /// Errors go to stderr as JSON too, shaped as an "error" object with code, kind,
    /// message and hint. Fields may be added over time; renaming or removing one is a
    /// breaking change.
    #[arg(long, global = true, display_order = 900)]
    pub json: bool,

    /// Neither read from nor write to the on-disk response cache.
    ///
    /// The cache only ever changes latency, never a result. Turning it off is for
    /// checking a status that may have changed in the last minutes.
    #[arg(long = "no-cache", global = true, display_order = 901)]
    pub no_cache: bool,
}

impl Cli {
    /// Whether the response cache may be used. The flag is negative, the setting is not.
    pub fn cache(&self) -> bool {
        !self.no_cache
    }
}

/// The full help page as a string.
///
/// `blibs` with no arguments prints this: the tool is invoked interactively rarely
/// enough that an empty invocation practically always means "show me the documentation",
/// not "I forgot what I wanted".
pub fn long_help() -> String {
    Cli::command().render_long_help().to_string()
}

/// The three commands.
#[expect(
    clippy::large_enum_variant,
    reason = "exactly one of these exists per process; boxing it would only cost clap's \
              derive the `Args` impl it needs"
)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search the catalogue: who holds it, is it in, where is it on the shelf.
    ///
    /// Free words and the field flags combine with AND, and each shown record is listed
    /// with its holdings and — unless --no-availability is given — with the status of
    /// every copy.
    ///
    /// With --at the output is one block per location, in the order given, with the
    /// copies of that location under each hit. Without --at it is one line per hit.
    Search(SearchArgs),

    /// Show one title in full: every holding, every copy, shelfmark and status.
    ///
    /// The bibliographic record first, then every library that holds it with its copies,
    /// shelfmarks and status.
    ///
    /// A copy that is out never carries a due date. Due dates and holds live behind a
    /// library account and blibs signs in nowhere, so it says the copy is on loan and
    /// stops there rather than suggesting a date it cannot know.
    Show(ShowArgs),

    /// List, search and locate the libraries blibs knows.
    ///
    /// 123 institutions and 212 branches, compiled into the binary, so this command
    /// works offline and with a cold cache.
    ///
    /// A house may have more than one short name — STABI and SBB are the same house —
    /// and only branches that are actually spoken about carry one at all: AGB is the
    /// Amerika-Gedenkbibliothek as a branch of the VÖBB, while VOEBB and ZLB mean the
    /// whole network.
    Libraries(LibrariesArgs),
}

/// `blibs search`.
#[derive(Debug, clap::Args)]
#[command(after_help = SEARCH_AFTER_HELP)]
pub struct SearchArgs {
    /// Search terms, matched as whole words anywhere in the record.
    ///
    /// An argument containing a space was quoted in the shell and is searched as a
    /// phrase; an argument without one is a word, and several words must all occur.
    /// That is the whole quoting rule. Truncation does not exist in this catalogue, so
    /// a term containing * or ? is refused rather than sent.
    pub terms: Vec<String>,

    /// Words from the title.
    ///
    /// Quote the value to search it as a phrase: --title "Der Prozess" finds the phrase,
    /// --title Prozess finds the word.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,

    /// Author, editor or translator.
    ///
    /// Always searched as a word list, never as a phrase, and the order of the name
    /// therefore does not matter. The index holds authority forms: "Kafka, Franz" finds
    /// 2911 records while "Franz Kafka" as a phrase finds 40, so a name written the
    /// natural way would otherwise find almost nothing. Roles are never filtered — an
    /// editor or translator you search for stays findable.
    #[arg(long, value_name = "NAME")]
    pub author: Option<String>,

    /// Subject heading.
    ///
    /// The headings are mixed-language and are not translated: --subject Recht and
    /// --subject law are two different searches, and both return records. For German
    /// holdings the German term usually finds more.
    #[arg(long, value_name = "TEXT")]
    pub subject: Option<String>,

    /// Publisher.
    ///
    /// Only the KOBV catalogue has an index for this. Combined with a VÖBB branch in
    /// --at it is refused, rather than answered by a search that quietly ignored it.
    #[arg(long, value_name = "NAME")]
    pub publisher: Option<String>,

    /// Four-digit year, meaning "was running in this year".
    ///
    /// For a book that is the year of publication. For a serial it is a membership: a
    /// journal that ran from 1923 to 1991 is found by --year 1960 and not by --year
    /// 1922. Ranges do not exist here and would return nothing at all without saying so,
    /// so --year 1990-2000 is refused. Only the KOBV catalogue has this index.
    #[arg(long, value_name = "YYYY")]
    pub year: Option<String>,

    /// ISBN or ISSN, with or without hyphens.
    ///
    /// The check digit is verified here, before anything is sent. The identifier index
    /// upstream discards it, so a mistyped ISBN does not come back empty — it comes back
    /// as a different book. An ISSN is passed on, where the check digit does count.
    #[arg(long, value_name = "NUMBER")]
    pub isbn: Option<String>,

    /// My libraries: short names or ISILs, comma-separated.
    ///
    /// Short names and ISILs may be mixed and case does not matter; blibs libraries
    /// lists them all. An institution such as HU or STABI is filtered upstream, so its
    /// hit count is the true one and paging over it is complete. A branch of the VÖBB
    /// such as AGB is answered by voebb.de instead, because a KOBV record never says
    /// which branch holds the copy. The output is grouped into one block per location,
    /// in the order given here.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub at: Vec<String>,

    /// How many hits to show per location, 1 to 50 [default: 10].
    ///
    /// Never clamped: the catalogue truncates at 50 without saying so, so a larger value
    /// is refused instead of quietly ignored. Each shown record costs one extra request
    /// for its availability, which is why nothing beyond the shown records is asked
    /// about.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,

    /// Which page of results, 1-based [default: 1].
    ///
    /// All location blocks page together. A VÖBB branch in --at is the one limit:
    /// voebb.de has no offset and every page past the first is another request on the
    /// same session, so --page times --limit may not reach past result 220 and a deeper
    /// window is refused instead of walked.
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,

    /// How to order the shown records [default: relevance].
    ///
    /// Client-side, over the fetched records only — the catalogue cannot sort at all. So
    /// --limit 20 --sort year means "the 20 most relevant hits, the newest of those
    /// first", not "the 20 newest hits". title sorts by the displayed title with its
    /// leading article; availability costs no extra request, because the data is already
    /// there for every shown record.
    #[arg(long, value_name = "KEY", value_parser = PossibleValuesParser::new(SORT_VALUES))]
    pub sort: Option<String>,

    /// Keep only records of this material type.
    ///
    /// A client-side filter over the fetched records only: there is no index for
    /// material type, so an empty result can mean "none in this window" instead of "none
    /// at all", and the footer says which of the two it was. book means printed and
    /// ebook means online; every record additionally carries an online flag, so nothing
    /// is lost for the material types that have no separate online value.
    #[arg(long, value_name = "TYPE", value_parser = PossibleValuesParser::new(FORMAT_VALUES))]
    pub format: Option<String>,

    /// Keep only records in this language, as a three-letter code.
    ///
    /// Bibliographic ISO-639-2/B codes as the records carry them: ger, eng, fre — not de
    /// and not German. Client-side over the fetched records only, exactly like --format.
    #[arg(long, value_name = "CODE")]
    pub language: Option<String>,

    /// Keep only the records with a copy that is in right now.
    ///
    /// It thins the page out and does not reload: the status is only asked about for the
    /// records that are shown, so ten hits may come back as four. A larger --limit is the
    /// way to see more, and unlike --format and --language this flag deliberately does
    /// not widen the fetched window — the 40 extra records would carry no status at all.
    ///
    /// Reference stock does not count as available: it is there, but it cannot be taken
    /// home. Records the catalogue states no status for drop out as well and are counted,
    /// and the footer — notes in the JSON — says how many. Combining this with --sort
    /// availability is allowed and harmless: every surviving record then carries the same
    /// status, so that sort has nothing left to order.
    #[arg(long)]
    pub available: bool,

    /// Do not ask whether the copies are in.
    ///
    /// Faster and quieter: availability is one request per shown record. Holdings are
    /// still listed, only without a status, and the JSON marks that as skipped so an
    /// empty copy list cannot be mistaken for "we asked and got nothing back".
    #[arg(long = "no-availability")]
    pub no_availability: bool,
}

/// Accepted values for `--sort`.
pub const SORT_VALUES: [&str; 5] = ["relevance", "year", "title", "author", "availability"];

/// Accepted values for `--format`. Closed vocabulary, same as the JSON.
pub const FORMAT_VALUES: [&str; 16] = [
    "book",
    "ebook",
    "journal",
    "ejournal",
    "article",
    "database",
    "map",
    "score",
    "audio",
    "video",
    "image",
    "electronic",
    "manuscript",
    "object",
    "mixed",
    "unknown",
];

/// `blibs show`.
#[derive(Debug, clap::Args)]
#[command(after_help = SHOW_AFTER_HELP)]
pub struct ShowArgs {
    /// The full, source-prefixed record id, as printed by search.
    ///
    /// The help text is given separately so that the terminal does not show the
    /// backticks that rustdoc needs.
    #[arg(
        value_name = "ID",
        help = "The full, source-prefixed record id, e.g. almafu_BV008885798 or voebb_SAK13776205",
        long_help = "The full, source-prefixed record id, e.g. almafu_BV008885798 or \
                     voebb_SAK13776205.\n\n\
                     The prefix is part of the id and selects the catalogue, so an id is \
                     never valid in the other one. Copy it from the search output; the \
                     line number there is a reading aid and cannot be used instead, \
                     because blibs keeps no state between invocations."
    )]
    pub id: String,

    /// Mark these libraries as mine and list them first.
    ///
    /// Short names or ISILs, comma-separated. Everything else is summarised in one
    /// "also at" line. Without it, all holdings are simply listed and nothing is marked.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub at: Vec<String>,

    /// Do not ask whether the copies are in.
    ///
    /// The holdings are still listed, only without a status.
    #[arg(long = "no-availability")]
    pub no_availability: bool,
}

/// `blibs libraries`.
#[derive(Debug, clap::Args)]
#[command(after_help = LIBRARIES_AFTER_HELP)]
pub struct LibrariesArgs {
    /// A short name or ISIL to show in detail, e.g. STABI or DE-11.
    ///
    /// The detail view shows name, short names, type, address, coordinates, phone,
    /// website and catalogue. Opening hours are deliberately absent: they carry dated
    /// special notices that would be wrong within days of being compiled in, so the
    /// website is linked instead of a frozen copy being shown.
    #[arg(value_name = "LIBRARY")]
    pub key: Option<String>,

    /// Search short names, names and cities.
    ///
    /// Case- and accent-insensitive substring matching over the institutions. Branches
    /// are not searched — the question this answers is "which house do I mean", and 212
    /// branch names would bury the 123 houses.
    #[arg(long, value_name = "TEXT")]
    pub find: Option<String>,

    /// Order by distance from "lat,lon" or from another library's short name.
    ///
    /// Computed offline from the coordinates in the list. blibs geocodes nothing, so an
    /// address is refused with a pointer rather than quietly sent to a third-party
    /// service: use --near 52.52,13.39 or --near HU.
    #[arg(long, value_name = "POINT")]
    pub near: Option<String>,
}

/// A validated search, ready to run. Nothing in here can still fail validation, which is
/// what lets `run` be about orchestration only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The query.
    pub query: QuerySpec,
    /// The invocation echoed back in one string, for `query.terms` in the JSON and for
    /// the "no results for …" message.
    ///
    /// The free terms when there are any, and otherwise the field flags that made the
    /// query — a search built from flags alone must still echo back as something that
    /// reproduces it, and an empty string would read as "you searched for nothing".
    pub terms_echo: String,
    /// The resolved locations, in the order the user gave them, without duplicates.
    /// Empty when `--at` was not given.
    pub locations: Vec<Location>,
    /// How many hits to show per location.
    pub limit: Limit,
    /// Which page.
    pub page: Page,
    /// How to sort. Always over the fetched records only.
    pub sort: SortKey,
    /// The client-side filters.
    pub filters: Filters,
    /// Whether availability is fetched for the shown records.
    pub availability: AvailabilityMode,
    /// Whether only the records with a copy that is in right now are shown.
    ///
    /// Deliberately not a third field in [`Filters`]: it filters on the status, which
    /// exists only *after* availability was fetched for the shown page, so it is not a
    /// window filter. As one, `Filters::is_active()` would widen the fetched window to
    /// 50 for nothing — `take_page*` cuts the page before `fill_availability` runs, so
    /// the 40 extra records would never get a status to be judged by — and `active_filter`
    /// in `run` would sooner or later name it in a message about the fetched window,
    /// where it does not belong.
    pub only_available: bool,
    /// Whether the output is JSON.
    pub json: bool,
    /// Whether the response cache may be used.
    pub cache: bool,
}

impl Plan {
    /// The locations grouped by the engine that answers for them, in the order the
    /// engines first appear in `--at`.
    ///
    /// Without `--at` this is a single `kobv` entry with no locations: one search with
    /// no holdings filter.
    pub fn by_engine(&self) -> Vec<(Engine, Vec<Location>)> {
        validate::split_by_engine(&self.locations)
    }

    /// Which engines this invocation runs, in a stable order.
    pub fn engines(&self) -> Vec<Engine> {
        self.by_engine()
            .into_iter()
            .map(|(engine, _)| engine)
            .collect()
    }

    /// The record window each engine fetches.
    ///
    /// Exactly the limit unless a client-side filter is active, in which case it is
    /// widened — a filter that only ever sees ten records would report "no hits" for a
    /// book sitting on record eleven.
    pub fn window(&self) -> FetchWindow {
        FetchWindow::plan(self.limit, self.page, self.filters.is_active())
    }

    /// Which slice of the surviving records this page prints.
    ///
    /// Planned from the same three values as [`Self::window`], and it has to stay that
    /// way: the window says what was asked for, the cut says what is shown, and a page
    /// that cuts by a rule the window did not follow claims a position it never fetched.
    pub fn cut(&self) -> PageCut {
        PageCut::plan(self.limit, self.page, self.filters.is_active())
    }
}

/// A validated `show`, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowPlan {
    /// The record to fetch. Its prefix, and nothing else, chooses the engine.
    pub id: RecordId,
    /// The locations to mark as mine and list first. Never used to choose an engine.
    pub locations: Vec<Location>,
    /// Whether availability is fetched.
    pub availability: AvailabilityMode,
    /// Whether the output is JSON.
    pub json: bool,
    /// Whether the response cache may be used.
    pub cache: bool,
}

impl ShowPlan {
    /// The engine that answers, taken from the id's source prefix.
    pub fn engine(&self) -> Engine {
        self.id.engine()
    }
}

/// A validated `libraries`, ready to run.
///
/// The three selectors are independent fields rather than one enum so that `--find` and
/// `--near` can combine — "the closest of these" is a sensible question and needs no new
/// syntax. A detail key answers on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct LibrariesPlan {
    /// A single library to show in detail, as the user typed it.
    ///
    /// Deliberately unresolved: an unknown key here is exit 1 ("no library matched"),
    /// not exit 2. `--at` names a library the search *must* have, so a typo there is a
    /// usage error; `libraries FOO` is a lookup, and a lookup that finds nothing is a
    /// result.
    pub detail: Option<String>,
    /// The `--find` query.
    pub find: Option<String>,
    /// The point `--near` resolved to.
    pub near: Option<LatLon>,
    /// What `--near` was given, for the message when nothing matched.
    pub near_input: Option<String>,
    /// Whether the output is JSON.
    pub json: bool,
}
