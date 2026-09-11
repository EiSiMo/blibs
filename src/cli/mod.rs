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
use clap::error::ContextKind;
use clap::{CommandFactory, Parser, Subcommand};

use crate::error::CommandPath;
use crate::libraries::{Entry, LatLon};
use crate::model::{
    AvailabilityMode, Engine, FetchWindow, Limit, Location, Page, QuerySpec, RecordId, SortKey,
};
use crate::select::{Filters, PageCut};

pub use run::run;
pub use validate::{validate, validate_libraries, validate_show};

/// Examples and the exit-code table, printed under the top-level help.
///
/// The exit codes are part of the public interface, and an agent that
/// reads only `--help` must find them there rather than in a repository somewhere.
const AFTER_HELP: &str = "\
Examples:
  blibs search Kafka Prozess
  blibs search \"Der Vorleser\" --at HU,FU
  blibs search --author Kafka --year 1953 --format book
  blibs search --isbn 978-3-596-29433-6 --json
  blibs show almafu_BV008885798 --at STABI,HU
  blibs libraries --find AGB
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
  blibs search \"Der Vorleser\"                      one phrase
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
  blibs libraries AGB                one branch in detail, and how to search it
  blibs libraries HU/Germanistik     a branch of a house that has no shorthand
  blibs libraries SIG00036           the same, by the KOBV id blibs libraries prints
  blibs libraries --branches         every branch, with the key that searches it
  blibs libraries --find grimm       search names, shorthands, ISILs and branches
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

/// The command a clap refusal happened in, for the hint on
/// [`crate::error::UsageError::Cli`].
///
/// **This is resolved here, at the point the clap error is turned into a
/// [`crate::error::UsageError`], and not later in its `hint`** — because the second of the
/// two ways to answer it needs the application's own command tree, and that tree is
/// `cli`, the top layer. Asking it from `error`, the bottom one, would run the
/// dependency backwards through the layering `crate`'s module block declares as a hard
/// rule. `error` therefore takes the answer as a [`CommandPath`] and only words the
/// sentence around it.
///
/// The two ways, in order:
///
/// 1. clap's own usage line, which most error kinds carry as `ContextKind::Usage` and
///    which names the command outright.
/// 2. the argument in `ContextKind::InvalidArg`, looked up in [`Cli::command`]. This is
///    the path for `ErrorKind::InvalidValue` — a `--format`/`--sort` given a value
///    outside its `possible values` — which is the one clap family that carries no usage
///    line at all. Asking the command tree is the same source clap consulted to raise
///    the error, so it cannot drift the way a hand-maintained flag-to-subcommand table
///    would.
///
/// A clap error with neither (a hand-built one) falls back to [`CommandPath::root`]
/// instead of guessing.
pub fn command_path(error: &clap::Error) -> CommandPath {
    if let Some(usage) = error.get(ContextKind::Usage) {
        return CommandPath::from_usage_line(&usage.to_string());
    }
    error
        .get(ContextKind::InvalidArg)
        .map(ToString::to_string)
        .and_then(|arg| subcommand_declaring(&arg))
        .unwrap_or_else(CommandPath::root)
}

/// The subcommand that declares the argument named in a clap `InvalidArg` context, e.g.
/// `"--format <TYPE>"` resolving to `Some(blibs search)`.
///
/// Matches by rendering each candidate [`clap::Arg`] the same way clap rendered the one
/// in the error — `Display for Arg` always formats in plain style regardless of the
/// command's own colour settings (`clap_builder`'s `arg.rs`), so the two strings line up
/// without reimplementing that formatting here.
fn subcommand_declaring(invalid_arg: &str) -> Option<CommandPath> {
    // `Cli::command()` hands back the tree as derived, before clap has resolved value
    // names and argument counts on it — `build()` is the same step `get_matches` runs
    // internally, and skipping it here makes `Arg::to_string()` panic below.
    let mut root = Cli::command();
    root.build();
    root.get_subcommands()
        .find(|sub| {
            sub.get_arguments()
                .any(|arg| arg.to_string() == invalid_arg)
        })
        .map(|sub| CommandPath::subcommand(sub.get_name()))
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
    /// A due date is printed where the catalogue prints one: voebb.de states it beside a
    /// copy that is out, KOBV states none at all. A hold lives behind a library account
    /// and blibs signs in nowhere, so a copy the catalogue says nothing about stays a
    /// bare "on loan" rather than carrying a date it cannot know.
    Show(ShowArgs),

    /// List, search and locate the libraries blibs knows.
    ///
    /// 123 institutions and 211 branches, compiled into the binary, so this command
    /// works offline and with a cold cache.
    ///
    /// A house may have more than one short name — STABI and SBB are the same house —
    /// and only branches that are actually spoken about carry one at all: AGB is the
    /// Amerika-Gedenkbibliothek as a branch of the VÖBB, while VOEBB and ZLB mean the
    /// whole network.
    Libraries(LibrariesArgs),
}

/// The largest value the counting flags (`--limit`, `--page`) accept from clap.
///
/// The two are declared as `i64` and not as `u32` on purpose. A `u32` cannot hold a
/// negative value, so clap refuses `--limit -1` before this crate ever sees it — as an
/// *unknown flag*, with the tip "to pass '-1' as a value, use '-- -1'", which would make
/// the number a **search term** and answer a different question entirely. Letting the
/// sign through to [`validate`] buys the same first-class message `--limit 0` and
/// `--limit 51` already have ([`crate::error::UsageError::NegativeNumber`]).
///
/// The magnitude stays with clap, so that everything reaching `validate` fits in a `u32`
/// and the domain types keep their own ranges to themselves. The accepted range is
/// symmetric on purpose: with an open lower end clap prints `i64::MIN` at the user when a
/// value is too large, which is noise about a type they never asked about.
const COUNT_MAX: i64 = u32::MAX as i64;

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
    /// Quote the value to search it as a phrase: --title "Der Vorleser" finds the phrase,
    /// --title Vorleser finds the word.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,

    /// Author, editor or translator.
    ///
    /// Always searched as a word list, never as a phrase, and the order of the name
    /// therefore does not matter. The index holds authority forms such as "Kafka, Franz",
    /// and as a phrase the natural order finds two orders of magnitude fewer records than
    /// the inverted one, so a name written the natural way would otherwise find almost
    /// nothing. Roles are never filtered — an editor or translator you search for stays
    /// findable.
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
    /// Eight digits are read as an ISSN, ten or thirteen as an ISBN, and the check digit
    /// of each is verified here, before anything is sent. The identifier index upstream
    /// discards that digit, so a mistyped number does not come back empty — it comes back
    /// as a different title.
    #[arg(long, value_name = "NUMBER")]
    pub isbn: Option<String>,

    /// My libraries: short names, ISILs or branches, comma-separated.
    ///
    /// Short names and ISILs may be mixed and case does not matter; blibs libraries
    /// lists them all. An institution such as HU or STABI is filtered upstream, so its
    /// hit count is the true one and paging over it is complete. A branch of the VÖBB
    /// such as AGB is answered by voebb.de instead, because a KOBV record never says
    /// which branch holds the copy. The output is grouped into one block per location,
    /// in the order given here.
    ///
    /// A branch without a short name is named by its KOBV id (SIG00036), by its own ISIL
    /// where it has one, or as a path inside its house (HU/Germanistik) — the fragment
    /// need not be the whole name, and a fragment that fits two branches is refused with
    /// both of them named. Commas separate entries here, so a branch name containing one
    /// has to be named by a fragment without it.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub at: Vec<String>,

    /// How many hits to show per location, 1 to 50 [default: 10].
    ///
    /// Never clamped: the catalogue truncates at 50 without saying so, so a larger value
    /// is refused instead of quietly ignored. Each shown record costs one extra request
    /// for its availability, which is why nothing beyond the shown records is asked
    /// about.
    ///
    /// A KOBV window is fetched a few records wider than this, so that a record the
    /// catalogue delivered twice can be dropped and still replaced. At --limit 50 there
    /// is no room left for that, and a repeat is then the one case where the page comes
    /// back one record short — a note under the result says when that happened.
    // Taken as an `i64` so that a negative value reaches [`validate`]: see [`COUNT_MAX`].
    #[arg(
        long,
        value_name = "N",
        allow_negative_numbers = true,
        value_parser = clap::value_parser!(i64).range(-COUNT_MAX..=COUNT_MAX)
    )]
    pub limit: Option<i64>,

    /// Which page of results, 1-based [default: 1].
    ///
    /// All location blocks page together. With --format, --language or any --sort but
    /// relevance the window is anchored: one block of 50 records starting at the first,
    /// and --page walks the matches inside it rather than stepping the result.
    ///
    /// A VÖBB branch in --at is the one limit:
    /// voebb.de has no offset and every page past the first is another request on the
    /// same session, so --page times --limit may not reach past result 220 and a deeper
    /// window is refused instead of walked.
    // An `i64` for the same reason as `--limit`: see [`COUNT_MAX`].
    #[arg(
        long,
        value_name = "N",
        allow_negative_numbers = true,
        value_parser = clap::value_parser!(i64).range(-COUNT_MAX..=COUNT_MAX)
    )]
    pub page: Option<i64>,

    /// How to order the shown records [default: relevance].
    ///
    /// Client-side, over the fetched records only — the catalogue cannot sort at all. So
    /// --limit 20 --sort year means "the 20 most relevant hits, the newest of those
    /// first", not "the 20 newest hits". title sorts by the displayed title with its
    /// leading article, folding ä/ö/ü/ß into ae/oe/ue/ss so that Öhler sorts before
    /// Zander; availability costs no extra request, because the data is already there for
    /// every shown record — and is refused together with --no-availability, which
    /// switches that data off.
    ///
    /// Anything but relevance anchors the window, exactly as --format does: one block of
    /// 50 records, with --page walking the ordered records inside it. Pages that stepped
    /// the result would not partition one ordered set. On a VÖBB branch that block costs
    /// up to three sequential requests instead of one.
    #[arg(long, value_name = "KEY", value_parser = PossibleValuesParser::new(SORT_VALUES))]
    pub sort: Option<String>,

    /// Keep only records of this material type.
    ///
    /// A client-side filter over the fetched records only: there is no index for
    /// material type, so an empty result can mean "none in this window" instead of "none
    /// at all", and the footer says which of the two it was. That window is anchored —
    /// one block of 50 records, with --page walking the matches inside it. book means printed and
    /// ebook means online; every record additionally carries an online flag, so nothing
    /// is lost for the material types that have no separate online value.
    #[arg(long, value_name = "TYPE", value_parser = PossibleValuesParser::new(FORMAT_VALUES))]
    pub format: Option<String>,

    /// Keep only records in this language, as a three-letter code.
    ///
    /// Bibliographic ISO-639-2/B codes as the records carry them: ger, eng, fre — not de
    /// and not German, and not the terminology codes deu, eng, fra: deu is refused with
    /// ger in the message rather than answered with nothing. Client-side over the fetched
    /// records only, exactly like --format, so it anchors the window too.
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
    /// Faster and quieter, and how much of the answer goes with it depends on the
    /// catalogue. On kobv it saves one request per shown record, and the copy lines under
    /// each hit go with it — this is the flag that turns those off. The libraries the
    /// record itself names are still there in the JSON, with no status and no copies. On
    /// voebb the record page *is* the holdings, so nothing beyond the result list is
    /// fetched at all: the hits come back without libraries, copies or shelfmarks, in
    /// about a fifth of the time.
    ///
    /// Either way the JSON marks availability as skipped, so an empty copy list cannot be
    /// mistaken for "we asked and got nothing back".
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
    /// Short names, ISILs or branches, comma-separated — everything --at accepts on a
    /// search. Everything else is summarised in one "also at" line. Without it, all
    /// holdings are simply listed and nothing is marked.
    ///
    /// A branch narrows the copies as well: the traffic light, the "n of m available"
    /// count and the JSON's at[].status then answer "is it in *there*", and the copies of
    /// the same library standing at other branches are named in one line instead. Note
    /// that holdings[].summary is the other question — what the catalogue says about the
    /// whole institution — so a copy on loan at one branch sits under a library the
    /// catalogue calls available, and both statements are true.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub at: Vec<String>,

    /// Do not ask whether the copies are in.
    ///
    /// On a kobv record the holdings are still listed, only without a status and without
    /// their copies. A voebb record states its copies on the page this command fetches
    /// anyway, so there is nothing cheaper to ask for and they come back either way.
    #[arg(long = "no-availability")]
    pub no_availability: bool,
}

/// `blibs libraries`.
#[derive(Debug, clap::Args)]
#[command(after_help = LIBRARIES_AFTER_HELP)]
pub struct LibrariesArgs {
    /// A short name or ISIL to show in detail, e.g. STABI, DE-11 or AGB.
    ///
    /// The detail view shows name, short names, type, address, coordinates, phone,
    /// website and catalogue. Opening hours are deliberately absent: they carry dated
    /// special notices that would be wrong within days of being compiled in, so the
    /// website is linked instead of a frozen copy being shown.
    ///
    /// A branch shorthand answers with that branch — its own address, its parent house
    /// and how to search it — not with the house it belongs to. An unknown shorthand is
    /// a usage error with the same "did you mean" `--at` gives.
    #[arg(value_name = "LIBRARY")]
    pub key: Option<String>,

    /// Search shorthands, names, cities, ISILs and KOBV ids — branches included.
    ///
    /// Case- and accent-insensitive substring matching over everything --at accepts, so a
    /// key this command prints is a key this command finds. Branches are grouped under
    /// their house: the question is still "which library do I mean", and a house can never
    /// be pushed off the answer by its own branches.
    #[arg(long, value_name = "TEXT")]
    pub find: Option<String>,

    /// List the branches instead of the houses.
    ///
    /// The houses are what `blibs libraries` shows; every one of their branches is
    /// addressable too, and this is where they are listed with the key that searches each.
    /// Combines with --find and --near, which then narrow the branches rather than the
    /// houses.
    #[arg(long)]
    pub branches: bool,

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
    /// The free terms **and** the field flags, in that order: everything that narrowed
    /// the search is named, because the count printed beside it belongs to all of it. An
    /// echo that named only the free words made `552 results for Kafka` out of a query
    /// that also carried `--title Prozess`, and turned an empty result into advice about
    /// the wrong word. An empty string would read as "you searched for nothing", so a
    /// search built from flags alone echoes its fields.
    pub terms_echo: String,
    /// The resolved locations, in the order the user gave them, without duplicates.
    /// Empty when `--at` was not given.
    pub locations: Vec<Location>,
    /// The locations that will **not** be searched because their catalogue cannot be
    /// paged as deep as this window, and the two numbers that say so. `None` when every
    /// location can be reached.
    ///
    /// They stay in [`Self::locations`] on purpose: they get their block like any other
    /// location, empty and with a note, because a missing block cannot be told apart from
    /// a forgotten one. What they are kept out of is [`Self::by_engine`], which is the
    /// list of searches that actually run.
    pub too_deep: Option<TooDeep>,
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

/// The locations one invocation refuses to search because the window lies deeper than
/// their catalogue can be paged, decided in `cli::validate` before a byte goes out.
///
/// Only voebb.de has such a ceiling: it has no offset, so position 220 is the tenth
/// sequential page of a session replayed in order. The numbers ride along because the
/// note that reports this states both, and re-deriving them beside the note would let the
/// sentence and the decision drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooDeep {
    /// The [`Location::key`]s that will not be searched, in `--at` order.
    pub keys: Vec<String>,
    /// The last result this window would need.
    pub position: u32,
    /// The last result the catalogue serves.
    pub max: u32,
}

impl TooDeep {
    /// Whether this location is one of the refused ones.
    pub fn holds(&self, key: &str) -> bool {
        self.keys.iter().any(|refused| refused == key)
    }
}

impl Plan {
    /// The locations grouped by the engine that answers for them, in the order the
    /// engines first appear in `--at`, **minus the ones this window cannot reach**.
    ///
    /// This is the list of searches, so a location refused by [`Self::too_deep`] is not
    /// in it — asking for a page voebb.de does not serve is the one thing already known
    /// to be pointless. Its block is written all the same, from [`Self::locations`].
    ///
    /// Without `--at` this is a single `kobv` entry with no locations: one search with
    /// no holdings filter. A `--at` from which every location has been refused is *not*
    /// that case and must never be turned into it — an unrestricted search over the whole
    /// region, printed under a library's heading, is the worst answer available — so it
    /// is no search at all. `cli::validate` refuses such an invocation outright (exit 2),
    /// which is why this is a guard rather than a path the tool takes.
    pub fn by_engine(&self) -> Vec<(Engine, Vec<Location>)> {
        let searched: Vec<Location> = self
            .locations
            .iter()
            .filter(|location| !self.refused(&location.key))
            .cloned()
            .collect();
        if searched.is_empty() && !self.locations.is_empty() {
            return Vec::new();
        }
        validate::split_by_engine(&searched)
    }

    /// Whether this location is refused before the search, by key.
    fn refused(&self, key: &str) -> bool {
        self.too_deep
            .as_ref()
            .is_some_and(|too_deep| too_deep.holds(key))
    }

    /// Which engines this invocation runs, in a stable order.
    ///
    /// Over **all** locations, including the ones no search was sent for: they have
    /// blocks in `at[]`, and an engine missing from this list would leave those blocks
    /// under a catalogue the document says was never asked about them.
    pub fn engines(&self) -> Vec<Engine> {
        validate::split_by_engine(&self.locations)
            .into_iter()
            .map(|(engine, _)| engine)
            .collect()
    }

    /// Whether this invocation's window is **anchored**: one block of 50 raw records
    /// starting at record 1, whatever `--page` says, with `--page` walking the matches
    /// inside it.
    ///
    /// Two things anchor it, and both for the same reason — they see only the fetched
    /// records:
    ///
    /// - `--format`/`--language`, which throw records away, so a window of ten would
    ///   report "no hits" for a book sitting on record eleven;
    /// - any `--sort` but relevance, which **reorders** them, so a window of ten sorted
    ///   by year is the ten most relevant records put in date order, and page two of a
    ///   stepped window can legitimately be newer than page one (measured: 2020 above
    ///   2018). A sort whose pages do not partition one ordered set is not a sort.
    ///
    /// Kept apart from [`crate::select::Filters::is_active`] on purpose: that one answers
    /// "did a filter run", which the JSON's `window.filtered` and every message about
    /// `--format` still need to ask.
    pub fn anchored(&self) -> bool {
        self.filters.is_active() || self.sort != SortKey::Relevance
    }

    /// The record window each engine fetches.
    ///
    /// Exactly the limit unless the window is [`anchored`](Self::anchored), in which case
    /// it is the largest block the service serves and starts at record 1.
    pub fn window(&self) -> FetchWindow {
        FetchWindow::plan(self.limit, self.page, self.anchored())
    }

    /// Which slice of the surviving records this page prints.
    ///
    /// Planned from the same three values as [`Self::window`], and it has to stay that
    /// way: the window says what was asked for, the cut says what is shown, and a page
    /// that cuts by a rule the window did not follow claims a position it never fetched.
    pub fn cut(&self) -> PageCut {
        PageCut::plan(self.limit, self.page, self.anchored())
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
    /// A single library to show in detail, already resolved.
    ///
    /// Resolved here rather than carried through as text, because a key that names
    /// nothing is a **usage error** (exit 2) exactly as it is in `--at`: both are
    /// lookups of a name the user chose, and answering one of them with an empty list
    /// would leave an agent unable to tell "there is no such library" from "you mistyped
    /// one". The searches — `--find` and `--near` — keep their empty result.
    ///
    /// [`Entry`] carries the branch case with it, so the renderers can answer a question
    /// about a branch with the branch, instead of silently substituting its parent house.
    pub detail: Option<Entry>,
    /// The `--find` query.
    pub find: Option<String>,
    /// Whether the listing is of branches rather than of houses.
    pub branches: bool,
    /// The point `--near` resolved to.
    pub near: Option<LatLon>,
    /// What `--near` was given, for the message when nothing matched.
    pub near_input: Option<String>,
    /// Whether the output is JSON.
    pub json: bool,
}

#[cfg(test)]
mod tests {
    use clap::error::{ContextKind, ContextValue, ErrorKind};

    use super::*;
    use crate::error::{Error, UsageError};

    /// Measured: `blibs --json search Kafka --format buch` pointed the hint at
    /// `blibs --help`, which does not even list `--format` —
    /// clap only attaches `--format`'s owning page to the *human* rendering, and there it
    /// does so on its own, outside anything this crate builds.
    ///
    /// The cause is that `ErrorKind::InvalidValue` — an out-of-range `--format` or
    /// `--sort` — is the one clap error family that never carries a `Usage` context
    /// (verified against `clap_builder` 4.6.6's `Error::invalid_value`), so the usage-line
    /// parser that resolves every other clap refusal had nothing to read and fell back to
    /// the top-level help. This reproduces the exact shape clap builds for that case —
    /// `InvalidArg` set, `Usage` absent — and checks the hint against the app's own
    /// command tree instead of a hard-coded flag list, so it keeps holding if `--format`
    /// or `--sort` ever move to a different subcommand.
    ///
    /// It lives here rather than in `error` because the command tree it consults is
    /// here: `error` is the bottom layer and may not reach up into `cli` to ask.
    #[test]
    fn an_invalid_value_error_points_at_the_subcommand_that_declares_the_flag() {
        assert_eq!(
            command_path(&raw_with_invalid_arg("--format <TYPE>")).to_string(),
            "blibs search"
        );
        assert_eq!(
            command_path(&raw_with_invalid_arg("--sort <KEY>")).to_string(),
            "blibs search"
        );
        let refusal = raw_with_invalid_arg("--format <TYPE>");
        let error: Error = UsageError::Cli {
            command: command_path(&refusal),
            source: refusal,
        }
        .into();
        assert_eq!(
            error.hint().as_deref(),
            Some("run `blibs search --help` to see what it accepts")
        );

        // A flag no subcommand declares still falls back to the top-level help rather
        // than panicking or guessing.
        assert_eq!(
            command_path(&raw_with_invalid_arg("--nonexistent <X>")).to_string(),
            "blibs"
        );
    }

    /// A clap error that does carry a usage line is answered from it, without the
    /// command tree — the cheap path, and the one almost every refusal takes.
    #[test]
    fn a_usage_line_answers_without_asking_the_command_tree() {
        assert_eq!(
            command_path(&raw_with_usage("Usage: blibs libraries [OPTIONS] [NAME]")).to_string(),
            "blibs libraries"
        );
    }

    /// A hand-built clap error carries no context at all; the hint must still name a
    /// help that exists rather than an empty command.
    #[test]
    fn a_clap_error_without_context_still_points_somewhere() {
        let refusal = clap::Error::raw(
            ErrorKind::InvalidValue,
            "a value is required for '--at <LIST>' but none was supplied",
        );
        let error: Error = UsageError::Cli {
            command: command_path(&refusal),
            source: refusal,
        }
        .into();
        assert_eq!(
            error.hint().as_deref(),
            Some("run `blibs --help` to see what it accepts")
        );
    }

    fn raw_with_usage(usage: &str) -> clap::Error {
        let mut error = clap::Error::raw(ErrorKind::UnknownArgument, "boom");
        error.insert(ContextKind::Usage, ContextValue::String(usage.to_string()));
        error
    }

    /// Builds the same shape clap's `Error::invalid_value` does: `InvalidArg` set, no
    /// `Usage` — see [`an_invalid_value_error_points_at_the_subcommand_that_declares_the_flag`].
    fn raw_with_invalid_arg(arg: &str) -> clap::Error {
        let mut error = clap::Error::raw(ErrorKind::InvalidValue, "boom");
        error.insert(
            ContextKind::InvalidArg,
            ContextValue::String(arg.to_string()),
        );
        error
    }
}
