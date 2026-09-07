//! Everything that must be refused before a request is built.
//!
//! Each of these is an exit 2 with a message that names the limit:
//!
//! | input | why |
//! | --- | --- |
//! | `*` or `?` in a term | truncation does not exist (diagnostic 1/48) |
//! | `--year 1990-2000` | ranges do not exist, and return **zero hits silently** |
//! | `--year` not four digits | `dc.date` knows four digits |
//! | `--isbn` with a bad check digit | the index ignores it and returns the wrong book |
//! | empty query | diagnostic 1/10 |
//! | query over 1000 characters | HTTP 414, returned as diagnostic 1/2 |
//! | unknown library in `--at` | with up to three suggestions |
//! | `--limit` outside 1..=50 | SRU caps at 50 silently |
//!
//! `--at` also decides the engines, and with them which flags can still be honoured: the
//! advanced search on voebb.de has a row index for title, person, subject and ISBN, and
//! none for publisher or year. A flag the answering catalogue has no index for is refused
//! rather than dropped — a search that silently ignored `--year` would answer a question
//! nobody asked.

use crate::cli::{
    FORMAT_VALUES, LibrariesArgs, LibrariesPlan, Plan, SORT_VALUES, SearchArgs, ShowArgs, ShowPlan,
    isbn,
};
use crate::engine::voebb;
use crate::error::{Error, UsageError};
use crate::libraries::{self, Branch, LatLon, Library};
use crate::model::{
    AvailabilityMode, Engine, FetchWindow, Format, Identifier, Limit, Location, Page, QuerySpec,
    RecordId, SortKey, Term,
};
use crate::select::Filters;

/// The characters that would be truncation operators in a catalogue that had any.
const WILDCARDS: [char; 2] = ['*', '?'];

/// Written forms of a range. The ASCII hyphen is the one `plan/cli.md` names; the dashes
/// and the ellipsis are what a copy from a printed citation produces, and all of them
/// would come back as zero hits without a word of explanation.
const RANGE_MARKERS: [&str; 4] = ["-", "\u{2013}", "\u{2014}", ".."];

/// Where the service starts answering with HTTP 414, returned as diagnostic 1/2.
///
/// Measured against the raw query text rather than the assembled query, which is longer:
/// this is a guard rail, and the upstream refusal is caught separately as
/// [`crate::error::RejectedError::TooLongUpstream`].
const MAX_QUERY_CHARS: usize = 1000;

/// How long an ISO-639-2/B language code is.
const LANGUAGE_CODE_LEN: usize = 3;

/// Validate a search invocation and resolve everything it names.
///
/// The order of the checks is the order of the user's attention: what they typed as a
/// query first, then how much of it they wanted, then where. Every failure here happens
/// before a socket is opened.
pub fn validate(args: &SearchArgs, json: bool, cache: bool) -> Result<Plan, Error> {
    Ok(plan(args, json, cache)?)
}

/// The body of [`validate`], in terms of the one error category it can produce.
fn plan(args: &SearchArgs, json: bool, cache: bool) -> Result<Plan, UsageError> {
    if let Some(term) = first_wildcard(args) {
        return Err(UsageError::Wildcard { term });
    }
    let query = query(args)?;
    if query.is_empty() {
        return Err(UsageError::EmptyQuery);
    }
    let chars = query_chars(args);
    if chars >= MAX_QUERY_CHARS {
        return Err(UsageError::QueryTooLong { chars });
    }

    let limit = args.limit.map_or(Ok(Limit::DEFAULT), Limit::new)?;
    let page = args.page.map_or(Ok(Page::FIRST), Page::new)?;
    let locations = locations(&args.at)?;
    check_engine_support(args, &locations)?;
    let filters = Filters {
        format: args.format.as_deref().map(format).transpose()?,
        language: args.language.as_deref().map(language).transpose()?,
    };
    check_window_depth(limit, page, &filters, &locations)?;
    check_flag_conflicts(args)?;

    Ok(Plan {
        terms_echo: echo(args, &query),
        query,
        locations,
        limit,
        page,
        sort: sort_key(args.sort.as_deref())?,
        filters,
        availability: availability(args.no_availability),
        only_available: args.available,
        json,
        cache,
    })
}

/// Validate a `show` invocation.
///
/// The engine comes from the id's source prefix and from nothing else, so `--at` here is
/// purely a display instruction: mark these libraries as mine and list them first. A
/// VÖBB branch in `--at` alongside a KOBV id is therefore not a contradiction and not
/// refused.
pub fn validate_show(args: &ShowArgs, json: bool, cache: bool) -> Result<ShowPlan, Error> {
    Ok(ShowPlan {
        id: RecordId::parse(args.id.trim())?,
        locations: locations(&args.at)?,
        availability: availability(args.no_availability),
        json,
        cache,
    })
}

/// Validate a `libraries` invocation.
///
/// `--near` is the only thing that can fail: it takes coordinates or a library, and
/// blibs geocodes nothing. Anything else — an address, a typo — is
/// [`UsageError::NearNeedsCoordinates`], whose hint names both accepted forms. The
/// distinction between "unknown library" and "that is an address" is not one this
/// function can make reliably, and guessing it would only produce two ways of saying the
/// same thing.
pub fn validate_libraries(args: &LibrariesArgs, json: bool) -> Result<LibrariesPlan, Error> {
    let near = args.near.as_deref().map(str::trim).map(point).transpose()?;
    Ok(LibrariesPlan {
        detail: args.key.as_deref().map(str::trim).map(str::to_owned),
        find: args.find.as_deref().map(str::trim).map(str::to_owned),
        near,
        near_input: args.near.clone(),
        json,
    })
}

/// Resolve a `--at` list, preserving order and rejecting the first unknown entry.
///
/// Duplicates are dropped rather than refused: `--at hu,STABI,HU` is a shell alias plus a
/// habit, not a mistake, and rendering the HU block twice would be worse than quietly
/// showing it once. Empty entries — the trailing comma in `--at HU,` — are skipped for
/// the same reason.
///
/// Everything that survives carries the **canonical** ISIL from the list. The upstream
/// holdings filter is case-sensitive, so forwarding the user's spelling would silently
/// find nothing.
pub fn locations(entries: &[String]) -> Result<Vec<Location>, UsageError> {
    let mut resolved: Vec<Location> = Vec::new();
    for entry in entries {
        let typed = entry.trim();
        if typed.is_empty() {
            continue;
        }
        let location = libraries::resolve(typed)?;
        if !resolved.contains(&location) {
            resolved.push(location);
        }
    }
    Ok(resolved)
}

/// Split resolved locations by the engine that answers for them.
///
/// One engine per location, never merged: `--at STABI,HU,AGB` runs two searches and
/// renders three blocks, and the same edition may legitimately appear in more than one.
/// Records are never matched across catalogues.
///
/// The groups come out in [`Engine::ALL`] order, **not** in the order of `--at`, and only
/// engines that have a location at all appear. `--at AGB,HU` and `--at HU,AGB` ask the
/// same question, so they must produce the same document — the engine order decides
/// `engines[]` and the order the two catalogues' records follow each other in, while the
/// user's order survives where it belongs, in `at[]` and in the rendered blocks.
///
/// Inside a group the locations keep the user's order, which is what `at[]` is built
/// from on the KOBV side.
///
/// An empty list is one `kobv` group with no locations — no `--at` means one search
/// without a holdings filter, and encoding that here keeps the rule out of `run`.
pub fn split_by_engine(locations: &[Location]) -> Vec<(Engine, Vec<Location>)> {
    if locations.is_empty() {
        return vec![(Engine::Kobv, Vec::new())];
    }
    Engine::ALL
        .into_iter()
        .filter_map(|engine| {
            let group: Vec<Location> = locations
                .iter()
                .filter(|location| location.engine == engine)
                .cloned()
                .collect();
            (!group.is_empty()).then_some((engine, group))
        })
        .collect()
}

/// Assemble the query from the free terms and the field flags.
fn query(args: &SearchArgs) -> Result<QuerySpec, UsageError> {
    Ok(QuerySpec {
        terms: args
            .terms
            .iter()
            .map(String::as_str)
            .map(Term::from_argument)
            .filter(|term| !term.is_empty())
            .collect(),
        title: args.title.as_deref().map(Term::from_argument),
        subject: args.subject.as_deref().map(Term::from_argument),
        publisher: args.publisher.as_deref().map(Term::from_argument),
        // Never a `Term`: `--author` is always a word list, never a phrase.
        author: args
            .author
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        year: args.year.as_deref().map(year).transpose()?,
        identifier: args.isbn.as_deref().map(identifier).transpose()?,
    })
}

/// Every piece of query text the user typed, in the order the flags are declared.
///
/// One list, used by the wildcard scan and the length check alike, so that a flag added
/// to one cannot be forgotten in the other.
fn query_text(args: &SearchArgs) -> Vec<(&'static str, &str)> {
    let mut text: Vec<(&'static str, &str)> =
        args.terms.iter().map(|term| ("", term.as_str())).collect();
    let flagged = [
        ("--title", &args.title),
        ("--author", &args.author),
        ("--subject", &args.subject),
        ("--publisher", &args.publisher),
        ("--year", &args.year),
        ("--isbn", &args.isbn),
    ];
    for (flag, value) in flagged {
        if let Some(value) = value {
            text.push((flag, value.as_str()));
        }
    }
    text
}

/// The first piece of query text that contains a truncation character.
///
/// Checked across the free terms *and* every field flag: `--title Proze*` fails upstream
/// exactly as `search Proze*` does, and diagnostic 1/48 does not say which of the two it
/// meant.
fn first_wildcard(args: &SearchArgs) -> Option<String> {
    query_text(args)
        .into_iter()
        .find(|(_, value)| value.contains(WILDCARDS))
        .map(|(_, value)| value.to_owned())
}

/// How long the query text is, counted in characters rather than bytes — the limit
/// upstream is on the URL, and an umlaut is one character to the user either way.
fn query_chars(args: &SearchArgs) -> usize {
    query_text(args)
        .into_iter()
        .map(|(_, value)| value.chars().count())
        .sum()
}

/// Validate `--year`.
///
/// A range is caught before the format check, because "1990-2000" is a *different*
/// mistake from "199": the range has a plausible meaning that this catalogue answers
/// with a silent zero, and the hint has to say so instead of talking about digit counts.
fn year(input: &str) -> Result<u16, UsageError> {
    let trimmed = input.trim();
    if RANGE_MARKERS.iter().any(|marker| trimmed.contains(marker)) {
        return Err(UsageError::Range {
            input: trimmed.to_owned(),
        });
    }
    let four_digits = trimmed.len() == 4 && trimmed.chars().all(|c| c.is_ascii_digit());
    four_digits
        .then(|| trimmed.parse::<u16>().ok())
        .flatten()
        .ok_or_else(|| UsageError::YearFormat {
            input: trimmed.to_owned(),
        })
}

/// Validate `--isbn`, keeping the user's spelling in the error so they can see the typo.
fn identifier(input: &str) -> Result<Identifier, UsageError> {
    isbn::parse(input).map_err(|problem| UsageError::Isbn {
        input: input.trim().to_owned(),
        problem,
    })
}

/// `--sort` as an enum.
fn sort_key(value: Option<&str>) -> Result<SortKey, UsageError> {
    let Some(value) = value else {
        return Ok(SortKey::default());
    };
    match value {
        "relevance" => Ok(SortKey::Relevance),
        "year" => Ok(SortKey::Year),
        "title" => Ok(SortKey::Title),
        "author" => Ok(SortKey::Author),
        "availability" => Ok(SortKey::Availability),
        other => Err(unreachable_value("--sort", other, &SORT_VALUES)),
    }
}

/// `--format` as an enum. The vocabulary is closed and shared with the JSON, so this is a
/// table and not a parse.
fn format(value: &str) -> Result<Format, UsageError> {
    match value {
        "book" => Ok(Format::Book),
        "ebook" => Ok(Format::Ebook),
        "journal" => Ok(Format::Journal),
        "ejournal" => Ok(Format::Ejournal),
        "article" => Ok(Format::Article),
        "database" => Ok(Format::Database),
        "map" => Ok(Format::Map),
        "score" => Ok(Format::Score),
        "audio" => Ok(Format::Audio),
        "video" => Ok(Format::Video),
        "image" => Ok(Format::Image),
        "electronic" => Ok(Format::Electronic),
        "manuscript" => Ok(Format::Manuscript),
        "object" => Ok(Format::Object),
        "mixed" => Ok(Format::Mixed),
        "unknown" => Ok(Format::Unknown),
        other => Err(unreachable_value("--format", other, &FORMAT_VALUES)),
    }
}

/// Normalise `--language` to a lowercase three-letter code.
///
/// Bibliographic codes, not the two-letter ones: the records carry `ger`, so `de` would
/// match nothing at all and look like an empty shelf rather than a wrong flag.
fn language(value: &str) -> Result<String, UsageError> {
    let code = value.trim().to_ascii_lowercase();
    let well_formed =
        code.len() == LANGUAGE_CODE_LEN && code.chars().all(|c| c.is_ascii_lowercase());
    if well_formed {
        Ok(code)
    } else {
        Err(invalid_value(format!(
            "--language takes a three-letter ISO-639-2/B code as the records carry it \
             (ger, eng, fre — not de and not German), got {value:?}"
        )))
    }
}

/// Refuse a window a VÖBB branch cannot be paged to.
///
/// voebb.de has no offset: reaching result 200 means walking ten result pages, one
/// sequential request each, on a session that must be replayed in order. So the depth is
/// capped at [`voebb::MAX_POSITION`] and a deeper window is a usage error **before** the
/// session is opened, rather than a minute of requests ending in a short answer.
///
/// The window that is measured is the one that would actually be fetched: `--format` and
/// `--language` widen it to 50 records a page, which moves its end.
///
/// The KOBV side is not checked here — SRU takes `startRecord` directly, and a window
/// past the last hit comes back as an honest empty page.
fn check_window_depth(
    limit: Limit,
    page: Page,
    filters: &Filters,
    locations: &[Location],
) -> Result<(), UsageError> {
    if !locations.iter().any(|at| at.engine == Engine::Voebb) {
        return Ok(());
    }
    let window = FetchWindow::plan(limit, page, filters.is_active());
    let position = window
        .start
        .saturating_add(u32::from(window.size.get()).saturating_sub(1));
    if position <= voebb::MAX_POSITION {
        return Ok(());
    }
    Err(UsageError::WindowTooDeep {
        engine: Engine::Voebb,
        page: page.get(),
        limit: u32::from(limit.get()),
        position,
        max: voebb::MAX_POSITION,
    })
}

/// Refuse a flag the answering catalogue has no index for.
///
/// voebb.de's advanced search offers four rows, each with an index chosen from a fixed
/// list: title, person, subject and "ISBN, ISSN, ISMN" are the four this tool maps onto,
/// and they cover `--title`, `--author`, `--subject` and `--isbn` as well as the free
/// terms. There is no row index for publisher or year — the form does carry an unmeasured
/// publisher text field and three year fields, but sending an unmeasured field would at
/// best be ignored and at worst narrow the search in a way nobody could see. So the two
/// are refused while a VÖBB branch is in `--at`.
///
/// `--sort`, `--format` and `--language` are never refused: they are client-side for both
/// engines and cost the user nothing but a window.
fn check_engine_support(args: &SearchArgs, locations: &[Location]) -> Result<(), UsageError> {
    if !locations.iter().any(|at| at.engine == Engine::Voebb) {
        return Ok(());
    }
    let unsupported = [
        ("--publisher", args.publisher.is_some()),
        ("--year", args.year.is_some()),
    ];
    for (flag, given) in unsupported {
        if given {
            return Err(UsageError::FlagUnsupportedByEngine {
                flag: flag.to_owned(),
                engine: Engine::Voebb,
            });
        }
    }
    Ok(())
}

/// Refuse two flags that cancel each other out.
///
/// `--available` keeps the records whose copies are in, and `--no-availability` says not
/// to ask whether they are — together there would be nothing left to filter on, and
/// picking a winner would silently answer a question the user did not ask.
///
/// Deliberately not clap's `conflicts_with`: that produces [`UsageError::Cli`], whose
/// `kind` is the catch-all `usage` and which carries no hint, while every refusal here
/// owes the user the way out.
///
/// The pairs are a table, so the next conflict is one line rather than a second function.
fn check_flag_conflicts(args: &SearchArgs) -> Result<(), UsageError> {
    let conflicts = [(
        ("--available", args.available),
        ("--no-availability", args.no_availability),
    )];
    for ((flag, asked), (with, refused)) in conflicts {
        if asked && refused {
            return Err(UsageError::ConflictingFlags {
                flag: flag.to_owned(),
                with: with.to_owned(),
            });
        }
    }
    Ok(())
}

/// The invocation echoed back in one string.
///
/// The free terms when there are any. A search built from field flags alone echoes as
/// those flags instead — `query.terms` has to reproduce the search, and an empty string
/// would read as "you searched for nothing" in the message for exit 1.
fn echo(args: &SearchArgs, query: &QuerySpec) -> String {
    let free = query.echo();
    if !free.is_empty() {
        return free;
    }
    query_text(args)
        .into_iter()
        .filter(|(flag, _)| !flag.is_empty())
        .map(|(flag, value)| format!("{flag} {}", requote(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Put back the quotes the shell removed, so the echo can be pasted onto a command line.
fn requote(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

/// Whether availability is fetched, from the negative flag.
fn availability(no_availability: bool) -> AvailabilityMode {
    if no_availability {
        AvailabilityMode::Skipped
    } else {
        AvailabilityMode::Fetched
    }
}

/// The point `--near` names: coordinates, or the position of a library.
///
/// Every failure is the same one — blibs geocodes nothing — so an address and a typo'd
/// short name get the same answer, which names both accepted forms.
fn point(input: &str) -> Result<LatLon, UsageError> {
    if let Some(point) = LatLon::parse(input) {
        return Ok(point);
    }
    libraries::resolve(input)
        .ok()
        .and_then(|location| coordinates(&location))
        .ok_or_else(|| UsageError::NearNeedsCoordinates {
            input: input.to_owned(),
        })
}

/// Where a resolved location sits. A branch has its own coordinates and they are the
/// better answer — the network's are those of its head office.
fn coordinates(location: &Location) -> Option<LatLon> {
    if let Some(branch) = &location.branch
        && let Some((_, Some(branch))) = libraries::by_kobvid(&branch.kobvid)
    {
        return Branch::coords(branch);
    }
    libraries::by_isil(&location.isil).and_then(Library::coords)
}

/// A value clap's own parser should already have rejected.
///
/// Reachable only if [`SORT_VALUES`]/[`FORMAT_VALUES`] and the tables above drift apart,
/// which the tests below prevent. It is still an error rather than a panic: nothing on a
/// path reachable from user input may panic, and a wrong value is a usage error whichever
/// side put it there.
fn unreachable_value(flag: &str, got: &str, expected: &[&str]) -> UsageError {
    invalid_value(format!(
        "{flag} does not accept {got:?}; expected one of {}",
        expected.join(", ")
    ))
}

/// Wrap a message as the same kind of error clap raises for a bad value, so that the
/// terminal shows it in the shape a user already knows from every other flag.
fn invalid_value(mut message: String) -> UsageError {
    // clap prints the message verbatim; the trailing newline is what separates it from
    // the usage line clap appends underneath.
    message.push('\n');
    UsageError::Cli(clap::Error::raw(
        clap::error::ErrorKind::InvalidValue,
        message,
    ))
}

#[cfg(test)]
mod tests {
    use clap::{Args as _, Parser};

    use super::*;
    use crate::cli::{Cli, Command, Plan};
    use crate::error::ExitCode;

    /// Parse a command line exactly as the binary does, then validate it. Going through
    /// clap rather than constructing `SearchArgs` by hand means these tests also cover
    /// the flag wiring — `--at a,b` splitting, `--json` after the subcommand, and so on.
    fn parse(args: &[&str]) -> Result<Cli, Error> {
        let command_line = std::iter::once("blibs").chain(args.iter().copied());
        Ok(Cli::try_parse_from(command_line)?)
    }

    fn search(args: &[&str]) -> Result<Plan, Error> {
        let cli = parse(args)?;
        let (json, cache) = (cli.json, cli.cache());
        match cli.command {
            Some(Command::Search(search)) => validate(&search, json, cache),
            other => panic!("expected a search, got {other:?}"),
        }
    }

    /// The `kind` of whatever a search invocation failed with. Every one of these must be
    /// exit 2 — nothing here may reach the network.
    fn usage_kind(args: &[&str]) -> &'static str {
        match search(args) {
            Ok(plan) => panic!("expected a usage error, got {plan:?}"),
            Err(error) => {
                assert_eq!(error.exit(), ExitCode::Usage, "{error}");
                error.kind()
            }
        }
    }

    fn libraries_plan(args: &[&str]) -> Result<LibrariesPlan, Error> {
        let cli = parse(args)?;
        let json = cli.json;
        match cli.command {
            Some(Command::Libraries(libraries)) => validate_libraries(&libraries, json),
            other => panic!("expected libraries, got {other:?}"),
        }
    }

    // ---- the table "Was cli abfangen muss" from plan/cli.md, one test per row ----

    /// Truncation does not exist (diagnostic 1/48), and it is refused wherever it is
    /// typed — a wildcard in `--title` fails exactly as one in a free term does.
    #[test]
    fn wildcards_are_refused_in_terms_and_in_flags() {
        assert_eq!(usage_kind(&["search", "Proze*"]), "wildcard_unsupported");
        assert_eq!(usage_kind(&["search", "Proze?"]), "wildcard_unsupported");
        assert_eq!(
            usage_kind(&["search", "--title", "Proze*"]),
            "wildcard_unsupported"
        );
        let Err(error) = search(&["search", "Proze*"]) else {
            panic!("a wildcard must be refused");
        };
        assert!(error.to_string().contains("Proze*"), "{error}");
    }

    /// A range is the dangerous one: it does not fail upstream, it returns zero hits
    /// without a word. It must therefore not be reported as a format problem.
    #[test]
    fn a_year_range_is_refused_as_a_range() {
        for value in ["1990-2000", "1990–2000", "1990..2000"] {
            assert_eq!(
                usage_kind(&["search", "Kafka", "--year", value]),
                "range_unsupported",
                "{value}"
            );
        }
    }

    #[test]
    fn a_year_must_be_four_digits() {
        for value in ["199", "19533", "abcd", "19x3"] {
            assert_eq!(
                usage_kind(&["search", "Kafka", "--year", value]),
                "invalid_year",
                "{value}"
            );
        }
    }

    /// The identifier index discards the check digit, so this would return a different
    /// book rather than nothing.
    #[test]
    fn a_bad_isbn_check_digit_is_refused_before_sending() {
        let Err(error) = search(&["search", "--isbn", "978-3-596-29433-4"]) else {
            panic!("a broken check digit must be refused");
        };
        assert_eq!(error.kind(), "invalid_isbn");
        assert!(error.to_string().contains("expected 6"), "{error}");
    }

    #[test]
    fn a_valid_isbn_becomes_a_normalised_identifier() {
        let plan = search(&["search", "--isbn", "978-3-596-29433-6"])
            .expect("a valid ISBN is a valid query");
        assert_eq!(
            plan.query.identifier,
            Some(Identifier::Isbn("9783596294336".to_owned()))
        );
        assert_eq!(plan.terms_echo, "--isbn 978-3-596-29433-6");
    }

    /// An ISSN is passed on, where the check digit is significant upstream.
    #[test]
    fn an_issn_is_carried_as_an_issn() {
        let plan = search(&["search", "--isbn", "0028-0836"]).expect("a valid ISSN");
        assert_eq!(
            plan.query.identifier,
            Some(Identifier::Issn("00280836".to_owned()))
        );
    }

    /// An empty query is diagnostic 1/10 upstream, which would surface as exit 5 for
    /// what is plainly a usage error. Whitespace-only arguments do not count as content.
    #[test]
    fn an_empty_query_is_refused() {
        assert_eq!(usage_kind(&["search"]), "empty_query");
        assert_eq!(usage_kind(&["search", "   "]), "empty_query");
        assert_eq!(usage_kind(&["search", "--at", "HU"]), "empty_query");
    }

    #[test]
    fn an_over_long_query_is_refused() {
        let long = "a".repeat(MAX_QUERY_CHARS);
        assert_eq!(usage_kind(&["search", &long]), "query_too_long");
        let just_under = "a".repeat(MAX_QUERY_CHARS - 1);
        assert!(search(&["search", &just_under]).is_ok());
    }

    #[test]
    fn an_unknown_library_is_refused_with_suggestions() {
        let Err(error) = search(&["search", "Kafka", "--at", "STABI2"]) else {
            panic!("an unknown library must be refused");
        };
        assert_eq!(error.kind(), "unknown_library");
        assert!(error.to_string().contains("STABI2"), "{error}");
        assert!(
            error.hint().unwrap_or_default().contains("STABI"),
            "the hint should offer the library that was meant"
        );
    }

    /// SRU caps at 50 without saying so, so a larger limit is refused rather than
    /// clamped — a clamp would look like it worked.
    #[test]
    fn a_limit_outside_the_range_is_refused() {
        assert_eq!(
            usage_kind(&["search", "Kafka", "--limit", "0"]),
            "limit_out_of_range"
        );
        assert_eq!(
            usage_kind(&["search", "Kafka", "--limit", "51"]),
            "limit_out_of_range"
        );
        assert!(search(&["search", "Kafka", "--limit", "50"]).is_ok());
    }

    #[test]
    fn page_zero_is_refused() {
        assert_eq!(
            usage_kind(&["search", "Kafka", "--page", "0"]),
            "page_out_of_range"
        );
    }

    // ---- --at ----

    /// Case is irrelevant and a repeat is not a mistake; the order the user gave is.
    #[test]
    fn at_resolves_case_insensitively_deduplicates_and_keeps_order() {
        let plan = search(&["search", "Kafka", "--at", "hu,STABI,hu"]).expect("known libraries");
        let keys: Vec<&str> = plan.locations.iter().map(|at| at.key.as_str()).collect();
        assert_eq!(keys, ["HU", "STABI"]);
    }

    /// An ISIL and its short name are the same location and must not produce two blocks.
    #[test]
    fn an_isil_and_its_alias_are_one_location() {
        let plan = search(&["search", "Kafka", "--at", "DE-11,HU"]).expect("known libraries");
        assert_eq!(plan.locations.len(), 1);
        assert_eq!(plan.locations[0].isil.as_str(), "DE-11");
    }

    /// A trailing comma is a typo in the shell, not an unknown library.
    #[test]
    fn empty_at_entries_are_skipped() {
        let plan = search(&["search", "Kafka", "--at", "HU,"]).expect("known libraries");
        assert_eq!(plan.locations.len(), 1);
    }

    /// Every location belongs to exactly one engine, and mixing them runs both.
    #[test]
    fn at_chooses_the_engines() {
        let plan = search(&["search", "Kafka"]).expect("a query without --at");
        assert_eq!(plan.engines(), [Engine::Kobv]);

        let plan = search(&["search", "Kafka", "--at", "AGB"]).expect("a VÖBB branch");
        assert_eq!(plan.engines(), [Engine::Voebb]);

        let plan =
            search(&["search", "Kafka", "--at", "STABI,HU,AGB"]).expect("a mixed location list");
        assert_eq!(plan.engines(), [Engine::Kobv, Engine::Voebb]);
        let grouped = plan.by_engine();
        assert_eq!(grouped[0].1.len(), 2, "STABI and HU go to one search");
        assert_eq!(grouped[1].1.len(), 1);
    }

    /// The engines run in [`Engine::ALL`] order whatever `--at` says, so the same
    /// question asked with the locations swapped produces the same document. The user's
    /// order survives in `at[]` and in the rendered blocks, which is where it belongs.
    #[test]
    fn the_engine_order_does_not_follow_the_order_of_at() {
        for at in ["AGB,HU", "HU,AGB", "AGB,STABI,HU", "HU,AGB,STABI"] {
            let plan = search(&["search", "Kafka", "--at", at]).expect("known libraries");
            assert_eq!(
                plan.engines(),
                [Engine::Kobv, Engine::Voebb],
                "--at {at} must still run kobv first"
            );
            let grouped = plan.by_engine();
            assert_eq!(grouped[0].0, Engine::Kobv);
            assert_eq!(grouped[1].0, Engine::Voebb);
        }
        // Inside a group the user's order stands: `at[]` on the KOBV side is built from it.
        let plan = search(&["search", "Kafka", "--at", "HU,AGB,STABI"]).expect("known libraries");
        let grouped = plan.by_engine();
        let keys: Vec<&str> = grouped[0]
            .1
            .iter()
            .map(|location| location.key.as_str())
            .collect();
        assert_eq!(keys, ["HU", "STABI"]);
    }

    /// A branch of a university is not a location: the KOBV record carries every copy of
    /// the institution anyway, and voebb.de does not know the house.
    #[test]
    fn a_non_voebb_branch_is_refused_with_its_institution() {
        let Err(error) = search(&["search", "Kafka", "--at", "PHILBIB"]) else {
            panic!("a non-VÖBB branch cannot be searched on its own");
        };
        assert_eq!(error.kind(), "branch_not_searchable");
        assert!(error.hint().unwrap_or_default().contains("--at"), "{error}");
    }

    // ---- engine support ----

    /// voebb.de's advanced search has row indexes for title, person, subject and ISBN —
    /// those four and the free terms are honoured for a VÖBB branch.
    #[test]
    fn voebb_honours_the_four_measured_indexes() {
        let plan = search(&[
            "search",
            "--at",
            "AGB",
            "--title",
            "Der Vorleser",
            "--author",
            "Schlink",
            "--subject",
            "Roman",
            "--isbn",
            "978-3-596-29433-6",
        ])
        .expect("the four indexes voebb.de offers");
        assert_eq!(plan.engines(), [Engine::Voebb]);
    }

    /// It has no row index for publisher or year, so those are refused rather than
    /// dropped — a search that silently ignored --year answers a different question.
    #[test]
    fn voebb_refuses_publisher_and_year() {
        for flag in ["--publisher", "--year"] {
            let value = if flag == "--year" { "1953" } else { "Fischer" };
            let Err(error) = search(&["search", "Vorleser", "--at", "AGB", flag, value]) else {
                panic!("{flag} has no index on voebb.de");
            };
            assert_eq!(error.kind(), "unsupported_by_engine");
            assert!(error.to_string().contains(flag), "{error}");
            assert!(error.to_string().contains("voebb"), "{error}");
        }
    }

    // ---- paging depth ----

    /// voebb.de has no offset: result 221 is behind ten sequential pages on a session
    /// that must be replayed in order, so a window reaching past it is refused before the
    /// session is opened rather than walked to.
    #[test]
    fn a_voebb_window_past_the_tenth_page_is_refused() {
        let deep = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "10", "--page",
        ];
        assert!(
            search(&[deep.as_slice(), &["22"]].concat()).is_ok(),
            "page 22 of 10 ends exactly at 220"
        );
        let error = match search(&[deep.as_slice(), &["23"]].concat()) {
            Ok(plan) => panic!("expected a usage error, got {plan:?}"),
            Err(error) => error,
        };
        assert_eq!(error.exit(), ExitCode::Usage, "{error}");
        assert_eq!(error.kind(), "window_too_deep");
        assert!(error.to_string().contains("--page 23"), "{error}");
        assert!(
            error.hint().unwrap_or_default().contains("220"),
            "the hint names where paging stops: {error}"
        );
    }

    /// The measured window is the one that would be fetched, and a client-side filter
    /// widens it to 50 a page — so the same `--page` reaches much further with one.
    #[test]
    fn a_filter_widens_the_window_and_with_it_the_depth() {
        let with_filter = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "10", "--page", "6", "--format", "book",
        ];
        assert_eq!(usage_kind(with_filter), "window_too_deep");
        assert!(
            search(&with_filter[..8]).is_ok(),
            "without the filter the same page is well inside"
        );
    }

    /// The depth is voebb.de's, not the tool's: SRU takes `startRecord` directly and
    /// answers a window past the last hit with an honest empty page.
    #[test]
    fn a_deep_window_is_fine_without_a_voebb_location() {
        assert!(search(&["search", "Kafka", "--page", "10000"]).is_ok());
        assert!(search(&["search", "Kafka", "--at", "HU", "--page", "10000"]).is_ok());
    }

    /// Those two are fine as long as no VÖBB branch is asked to answer them.
    #[test]
    fn publisher_and_year_are_fine_for_kobv() {
        assert!(
            search(&[
                "search",
                "--at",
                "HU",
                "--publisher",
                "Fischer",
                "--year",
                "1953"
            ])
            .is_ok()
        );
    }

    /// The client-side filters and the sort belong to blibs, not to a catalogue, so they
    /// are honoured whichever engine answers.
    #[test]
    fn client_side_flags_are_never_refused_by_an_engine() {
        assert!(
            search(&[
                "search",
                "Vorleser",
                "--at",
                "AGB",
                "--sort",
                "year",
                "--format",
                "book",
                "--language",
                "ger",
            ])
            .is_ok()
        );
    }

    /// Paging works on voebb.de (the toolbar is a plain form field), so it is allowed —
    /// only its depth is capped, which the tests above pin.
    #[test]
    fn paging_is_allowed_for_voebb() {
        assert!(search(&["search", "Vorleser", "--at", "AGB", "--page", "2"]).is_ok());
    }

    // ---- the plan itself ----

    #[test]
    fn a_full_invocation_becomes_a_complete_plan() {
        let plan = search(&[
            "search",
            "Der Prozess",
            "Kafka",
            "--author",
            "Franz Kafka",
            "--year",
            "1953",
            "--at",
            "HU,STABI",
            "--limit",
            "5",
            "--page",
            "2",
            "--sort",
            "year",
            "--format",
            "book",
            "--language",
            "GER",
            "--json",
            "--no-cache",
        ])
        .expect("a valid invocation");

        assert_eq!(
            plan.query.terms,
            [
                Term::Phrase("Der Prozess".to_owned()),
                Term::Word("Kafka".to_owned())
            ]
        );
        // Never a phrase: the author index holds authority forms.
        assert_eq!(plan.query.author.as_deref(), Some("Franz Kafka"));
        assert_eq!(plan.query.year, Some(1953));
        assert_eq!(plan.terms_echo, "\"Der Prozess\" Kafka");
        assert_eq!(plan.limit.get(), 5);
        assert_eq!(plan.page.get(), 2);
        assert_eq!(plan.sort, SortKey::Year);
        assert_eq!(plan.filters.format, Some(Format::Book));
        assert_eq!(plan.filters.language.as_deref(), Some("ger"));
        assert_eq!(plan.availability, AvailabilityMode::Fetched);
        assert!(plan.json);
        assert!(!plan.cache);
    }

    /// The defaults live in `model`, not in a clap attribute, so there is one place that
    /// says what they are.
    #[test]
    fn the_defaults_come_from_the_domain_types() {
        let plan = search(&["search", "Kafka"]).expect("a bare search");
        assert_eq!(plan.limit, Limit::DEFAULT);
        assert_eq!(plan.page, Page::FIRST);
        assert_eq!(plan.sort, SortKey::Relevance);
        assert_eq!(plan.filters, Filters::default());
        assert_eq!(plan.availability, AvailabilityMode::Fetched);
        assert!(
            plan.cache,
            "the cache is on unless --no-cache says otherwise"
        );
    }

    /// A filter widens the fetched window, because a filter that only ever sees ten
    /// records reports "no hits" for a book on record eleven.
    #[test]
    fn a_filter_widens_the_window() {
        let plain = search(&["search", "Kafka", "--limit", "10"]).expect("a bare search");
        assert_eq!(plain.window().size.get(), 10);
        let filtered =
            search(&["search", "Kafka", "--limit", "10", "--format", "video"]).expect("filtered");
        assert_eq!(filtered.window().size.get(), 50);
    }

    /// The counterpart to the test above, and the reason it is not a `Filters` field:
    /// `--available` judges a status that only the *shown* records ever get, because
    /// paging happens before availability is fetched. Widening the window would buy 40
    /// records without a status to be judged by, so the window stays at the limit.
    #[test]
    fn the_availability_filter_leaves_the_window_at_the_limit() {
        let plan = search(&["search", "Kafka", "--limit", "10", "--available"])
            .expect("a search filtered by availability");
        assert_eq!(plan.window().size.get(), 10);
    }

    #[test]
    fn available_is_carried_as_a_plan_flag() {
        let filtered = search(&["search", "Kafka", "--available"]).expect("a bare search");
        assert!(filtered.only_available);
        let plain = search(&["search", "Kafka"]).expect("a bare search");
        assert!(!plain.only_available);
    }

    /// One flag asks what the other switches off, so there would be nothing left to
    /// filter on. Refused with both names, rather than one of them quietly winning.
    #[test]
    fn available_and_no_availability_are_refused_together() {
        let Err(error) = search(&["search", "Kafka", "--available", "--no-availability"]) else {
            panic!("the two availability flags contradict each other");
        };
        assert_eq!(error.exit(), ExitCode::Usage, "{error}");
        assert_eq!(error.kind(), "conflicting_flags");
        for flag in ["--available", "--no-availability"] {
            assert!(error.to_string().contains(flag), "{error}");
        }
        assert!(error.hint().is_some(), "a usage error names the way out");
    }

    #[test]
    fn no_availability_is_carried_as_skipped() {
        let plan = search(&["search", "Kafka", "--no-availability"]).expect("a bare search");
        assert_eq!(plan.availability, AvailabilityMode::Skipped);
    }

    /// The global flags work after the subcommand, which is where a user types them.
    #[test]
    fn global_flags_may_follow_the_subcommand() {
        let plan = search(&["search", "Kafka", "--json"]).expect("a bare search");
        assert!(plan.json);
    }

    #[test]
    fn a_language_must_be_a_three_letter_code() {
        for value in ["de", "german", "g3r"] {
            assert_eq!(
                usage_kind(&["search", "Kafka", "--language", value]),
                "usage",
                "{value}"
            );
        }
    }

    /// clap and the tables in this module must not drift apart, or a legal value would
    /// come back as an internal error.
    #[test]
    fn every_advertised_value_maps_to_an_enum() {
        for value in SORT_VALUES {
            assert!(sort_key(Some(value)).is_ok(), "--sort {value}");
        }
        for value in FORMAT_VALUES {
            assert!(format(value).is_ok(), "--format {value}");
        }
    }

    // ---- show ----

    #[test]
    fn show_takes_the_engine_from_the_id_prefix() {
        let cli = parse(&["show", "voebb_SAK13776205", "--at", "HU"]).expect("a valid id");
        let (json, cache) = (cli.json, cli.cache());
        let Some(Command::Show(args)) = cli.command else {
            panic!("expected show");
        };
        let plan = validate_show(&args, json, cache).expect("a valid id");
        assert_eq!(plan.engine(), Engine::Voebb);
        assert_eq!(plan.id.local_id(), "SAK13776205");
        // `--at` is a display instruction here, never an engine choice.
        assert_eq!(plan.locations.len(), 1);
        assert_eq!(plan.locations[0].engine, Engine::Kobv);
    }

    #[test]
    fn show_refuses_an_id_without_a_source_prefix() {
        let cli = parse(&["show", "BV008885798"]).expect("clap accepts any string");
        let Some(Command::Show(args)) = cli.command else {
            panic!("expected show");
        };
        let Err(error) = validate_show(&args, false, true) else {
            panic!("an id without a prefix cannot be routed");
        };
        assert_eq!(error.kind(), "invalid_record_id");
        assert_eq!(error.exit(), ExitCode::Usage);
    }

    // ---- libraries ----

    #[test]
    fn near_accepts_coordinates() {
        let plan = libraries_plan(&["libraries", "--near", "52.52,13.39"]).expect("coordinates");
        let point = plan.near.expect("a point");
        assert!((point.lat - 52.52).abs() < 1e-9);
        assert!((point.lon - 13.39).abs() < 1e-9);
    }

    #[test]
    fn near_accepts_a_library() {
        let plan = libraries_plan(&["libraries", "--near", "HU"]).expect("a known library");
        assert!(plan.near.is_some());
    }

    /// blibs geocodes nothing, and says so rather than reaching for a service the user
    /// did not ask about.
    #[test]
    fn near_refuses_an_address() {
        let Err(error) = libraries_plan(&["libraries", "--near", "Alexanderplatz, Berlin"]) else {
            panic!("an address must be refused");
        };
        assert_eq!(error.kind(), "near_needs_coordinates");
        assert!(
            error.hint().unwrap_or_default().contains("--near HU"),
            "the hint must name both accepted forms"
        );
    }

    /// A detail key is a lookup, not a promise: an unknown one is exit 1 further down,
    /// so validation carries it through untouched.
    #[test]
    fn a_detail_key_is_carried_through_unresolved() {
        let plan = libraries_plan(&["libraries", "NOSUCHLIBRARY"]).expect("a lookup never fails");
        assert_eq!(plan.detail.as_deref(), Some("NOSUCHLIBRARY"));
        assert!(plan.find.is_none());
    }

    #[test]
    fn find_and_near_may_combine() {
        let plan = libraries_plan(&["libraries", "--find", "grimm", "--near", "HU"])
            .expect("both selectors");
        assert_eq!(plan.find.as_deref(), Some("grimm"));
        assert!(plan.near.is_some());
    }

    // ---- the parser itself ----

    /// An unknown flag is clap's error, exit 2, and clap points at the right help.
    #[test]
    fn an_unknown_flag_is_a_usage_error() {
        let Err(error) = parse(&["search", "Kafka", "--nope"]) else {
            panic!("an unknown flag must be refused");
        };
        assert_eq!(error.exit(), ExitCode::Usage);
        assert_eq!(error.kind(), "usage");
    }

    #[test]
    fn a_bare_invocation_parses_to_no_command() {
        let cli = parse(&[]).expect("blibs on its own is not an error");
        assert!(cli.command.is_none());
    }

    /// The long help is what a bare invocation prints, so it has to carry the whole
    /// contract: the commands, and the exit codes an agent branches on.
    #[test]
    fn the_long_help_carries_the_commands_and_the_exit_codes() {
        let help = crate::cli::long_help();
        for expected in ["search", "show", "libraries", "Exit codes:", "--json"] {
            assert!(help.contains(expected), "the long help omits {expected:?}");
        }
    }

    /// The surprising flags explain themselves; that is the whole point of the help text
    /// in a tool whose second audience cannot ask a follow-up question.
    #[test]
    fn the_search_help_explains_what_is_surprising() {
        let help = SearchArgs::augment_args(clap::Command::new("search"))
            .render_long_help()
            .to_string();
        let expectations = [
            ("word list", "--author is split into words"),
            ("was running in this year", "--year is a membership"),
            ("fetched records only", "the window rule"),
            ("mixed-language", "subject headings are not translated"),
            ("blibs libraries", "--at points at the list"),
            ("phrase", "the quoting rule"),
            (
                "thins the page out",
                "--available does not reload to refill the page",
            ),
            (
                "Reference stock",
                "--available drops non-circulating copies",
            ),
            (
                "states no status for",
                "--available drops and counts the records without a status",
            ),
        ];
        for (needle, why) in expectations {
            assert!(help.contains(needle), "the search help omits {why}");
        }
    }

    #[test]
    fn the_libraries_help_says_that_near_is_offline() {
        let help = LibrariesArgs::augment_args(clap::Command::new("libraries"))
            .render_long_help()
            .to_string();
        assert!(help.contains("geocodes nothing"), "{help}");
    }

    /// The one thing `unreachable_value` must do is stay an error rather than a panic.
    #[test]
    fn a_drifted_value_is_an_error_not_a_panic() {
        let error = sort_key(Some("nonsense")).expect_err("not an advertised value");
        assert_eq!(error.kind(), "usage");
    }
}
