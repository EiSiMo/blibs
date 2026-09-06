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

pub mod isbn;
pub mod run;
pub mod validate;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};

use crate::model::{Limit, Location, Page, QuerySpec, SortKey};
use crate::select::Filters;

pub use run::run;
pub use validate::validate;

/// Search the libraries of Berlin and Brandenburg.
#[derive(Debug, Parser)]
#[command(
    name = "blibs",
    version,
    about = "Search the libraries of Berlin and Brandenburg.",
    long_about = "Search the libraries of Berlin and Brandenburg: holdings, live \
                  availability and shelfmarks, from the KOBV union catalogue and the \
                  VÖBB public library network.",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// The command to run. `None` means the user typed `blibs` on its own, which prints
    /// the long help and exits 0 — an empty invocation practically always means "show me
    /// the documentation".
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Print one JSON document on stdout and nothing else. Errors go to stderr as JSON
    /// too.
    #[arg(long, global = true)]
    pub json: bool,

    /// Neither read from nor write to the on-disk response cache.
    #[arg(long = "no-cache", global = true)]
    pub no_cache: bool,
}

/// The three commands.
#[expect(
    clippy::large_enum_variant,
    reason = "exactly one of these exists per process; boxing it would only cost clap's \
              derive the `Args` impl it needs"
)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search the catalogue.
    Search(SearchArgs),
    /// Show one title in full.
    Show(ShowArgs),
    /// List, search or locate libraries.
    Libraries(LibrariesArgs),
}

/// `blibs search`.
#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// Search terms. An argument that contains a space is searched as a phrase — that is
    /// the whole quoting rule.
    pub terms: Vec<String>,

    /// Title.
    #[arg(long)]
    pub title: Option<String>,

    /// Author. Always searched as a word list, never as a phrase, so that "Franz Kafka"
    /// finds the same records as "Kafka, Franz".
    #[arg(long)]
    pub author: Option<String>,

    /// Subject heading.
    #[arg(long)]
    pub subject: Option<String>,

    /// Publisher.
    #[arg(long)]
    pub publisher: Option<String>,

    /// Year. Means "was running in this year": a serial published 1923-1991 matches
    /// --year 1960. Ranges do not exist in this catalogue.
    #[arg(long)]
    pub year: Option<String>,

    /// ISBN or ISSN. An ISBN is check-digit validated before it is sent, because the
    /// index ignores the check digit and a typo would return a different book.
    #[arg(long)]
    pub isbn: Option<String>,

    /// My libraries: ISILs or short names, comma-separated, mixed freely.
    #[arg(long, value_name = "LIST")]
    pub at: Option<String>,

    /// How many hits to show, 1-50.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,

    /// Page, 1-based.
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,

    /// Sort key. Client-side, over the fetched records only.
    #[arg(long, value_parser = PossibleValuesParser::new(SORT_VALUES))]
    pub sort: Option<String>,

    /// Material type. A client-side filter over the fetched records only.
    #[arg(long, value_parser = PossibleValuesParser::new(FORMAT_VALUES))]
    pub format: Option<String>,

    /// Language as an ISO-639-2/B code (ger, eng, fre). Client-side, like --format.
    #[arg(long)]
    pub language: Option<String>,

    /// Do not ask whether the copies are in.
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
pub struct ShowArgs {
    /// The full, source-prefixed record id, e.g. `almafu_BV008885798` or
    /// `voebb_SAK13776205`.
    ///
    /// The help text is given separately so that the terminal does not show the
    /// backticks that rustdoc needs.
    #[arg(
        help = "The full, source-prefixed record id, e.g. almafu_BV008885798 or voebb_SAK13776205"
    )]
    pub id: String,

    /// Mark these libraries as mine and list them first.
    #[arg(long, value_name = "LIST")]
    pub at: Option<String>,

    /// Do not ask whether the copies are in.
    #[arg(long = "no-availability")]
    pub no_availability: bool,
}

/// `blibs libraries`.
#[derive(Debug, clap::Args)]
pub struct LibrariesArgs {
    /// A short name or ISIL to show in detail.
    pub name: Option<String>,

    /// Search short names, names and cities.
    #[arg(long)]
    pub find: Option<String>,

    /// Sort by distance from "lat,lon" or from a library's short name. Offline: an
    /// address is not geocoded.
    #[arg(long, value_name = "POINT")]
    pub near: Option<String>,
}

/// A validated search, ready to run. Nothing in here can still fail validation, which is
/// what lets `run` be about orchestration only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The query.
    pub query: QuerySpec,
    /// The query as the user typed it, for the JSON echo and the "no hits" message.
    pub terms_echo: String,
    /// The resolved locations, in the order the user gave them.
    pub locations: Vec<Location>,
    /// How many hits to show.
    pub limit: Limit,
    /// Which page.
    pub page: Page,
    /// How to sort.
    pub sort: SortKey,
    /// The client-side filters.
    pub filters: Filters,
    /// Whether to fetch availability.
    pub availability: bool,
}
