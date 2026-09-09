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
//! | an empty value for any flag | almost always a shell variable that did not expand |
//! | a term with no letter or digit | read as *no* term upstream: `'@and'` returns 8.55 M records |
//! | query over 1000 characters | HTTP 414, returned as diagnostic 1/2 |
//! | unknown library in `--at` | with up to three suggestions |
//! | `--limit` outside 1..=50 | SRU caps at 50 silently |
//! | `--limit`/`--page` negative | clap would offer `-- -1`, which searches for the number |
//! | `--language deu` | the terminology code; the records carry the bibliographic `ger` |
//! | two flags that cancel each other | `--available`/`--sort availability` with `--no-availability` |
//!
//! `--at` also decides the engines, and with them which flags can still be honoured: the
//! advanced search on voebb.de has a row index for title, person, subject and ISBN, and
//! none for publisher or year. A flag the answering catalogue has no index for is refused
//! rather than dropped — a search that silently ignored `--year` would answer a question
//! nobody asked.

use crate::cli::{
    FORMAT_VALUES, LibrariesArgs, LibrariesPlan, Plan, SORT_VALUES, SearchArgs, ShowArgs, ShowPlan,
    TooDeep, isbn,
};
use crate::engine::voebb;
use crate::error::{Error, UsageError};
use crate::libraries::{self, LatLon, NotAPoint};
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

/// The ISO-639-2 **terminology** codes, and the **bibliographic** code MARC carries in
/// their place.
///
/// These twenty languages are the whole of it: ISO-639-2 gives a second, terminology code
/// to exactly those whose bibliographic code was formed from the English name, and to no
/// others. So this is a closed table, unlike the language vocabulary itself — a record can
/// carry any code at all, and one that is in neither set keeps the old behaviour: the
/// shape check passes and the filter simply matches nothing.
///
/// `deu` is not a random string but the code someone types who knows that three-letter
/// codes exist and not that MARC picked the other set, which is why it is a usage error
/// naming the code that was meant rather than a search that finds nothing.
///
/// The right-hand column is the same set `crate::render::human`'s `LANGUAGE_NAMES` is
/// keyed by (`ger`, `fre`, `chi`, `gre`, `dut`, `cze`, `per` all appear there); no code on
/// the **left** occurs in that table, which is the check that the two agree on which of
/// the two ISO sets this tool speaks.
const TERMINOLOGY_CODES: [(&str, &str); 20] = [
    ("bod", "tib"),
    ("ces", "cze"),
    ("cym", "wel"),
    ("deu", "ger"),
    ("ell", "gre"),
    ("eus", "baq"),
    ("fas", "per"),
    ("fra", "fre"),
    ("hye", "arm"),
    ("isl", "ice"),
    ("kat", "geo"),
    ("mkd", "mac"),
    ("mri", "mao"),
    ("msa", "may"),
    ("mya", "bur"),
    ("nld", "dut"),
    ("ron", "rum"),
    ("slk", "slo"),
    ("sqi", "alb"),
    ("zho", "chi"),
];

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
    if let Some(term) = first_without_text(args) {
        return Err(UsageError::TermWithoutText { term });
    }
    let chars = query_chars(args);
    if chars >= MAX_QUERY_CHARS {
        return Err(UsageError::QueryTooLong { chars });
    }

    let limit = count(args.limit, "--limit")?.map_or(Ok(Limit::DEFAULT), Limit::new)?;
    let page = count(args.page, "--page")?.map_or(Ok(Page::FIRST), Page::new)?;
    let locations = locations(&args.at)?;
    check_engine_support(args, &locations)?;
    let filters = Filters {
        format: args.format.as_deref().map(format).transpose()?,
        language: args.language.as_deref().map(language).transpose()?,
    };
    let too_deep = check_window_depth(limit, page, &filters, &locations)?;
    let sort = sort_key(args.sort.as_deref())?;
    check_flag_conflicts(args, sort)?;

    Ok(Plan {
        too_deep,
        terms_echo: echo(args, &query),
        query,
        locations,
        limit,
        page,
        sort,
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
/// `--near` is where most of the refusing happens: it takes coordinates or a library,
/// blibs geocodes nothing, and [`point`] tells the four ways of missing that apart.
///
/// **A positional key is resolved here, and a miss is a usage error** — the same exit 2,
/// the same `did you mean STABI?` that `--at STABI2` gives. The key names one library the
/// user believes exists; answering the typo with an empty list would be indistinguishable
/// from "this library really is not in the list", and the suggestion the very same list
/// can produce would be thrown away. That it is a lookup rather than a search is exactly
/// why: a **search** — `--find` and `--near` — may legitimately match nothing and keeps
/// its exit 1, because "no library is called that" is a true answer to a search and no
/// answer at all to a lookup.
///
/// The two text arguments are checked for being empty first, so that `--find "$NAME"`
/// with `NAME` unset is refused as the empty flag it is rather than searched for.
///
/// **A key and a listing flag are refused together**, before either is looked up. The key
/// used to win in silence, which made `libraries HU --find x` the one place in the tool
/// where a flag the user typed was ignored rather than answered or refused (round 2, §3.7)
/// — and an ignored `--find` looks exactly like a `--find` that matched the house.
pub fn validate_libraries(args: &LibrariesArgs, json: bool) -> Result<LibrariesPlan, Error> {
    let key = text_argument(args.key.as_deref(), "a library name")?;
    let find = text_argument(args.find.as_deref(), "--find")?;
    if key.is_some() {
        let listing = [
            ("--find", find.is_some()),
            ("--near", args.near.is_some()),
            ("--branches", args.branches),
        ];
        for (flag, given) in listing {
            if given {
                return Err(UsageError::ConflictingFlags {
                    flag: flag.to_owned(),
                    with: "a library name".to_owned(),
                }
                .into());
            }
        }
    }
    let near = args.near.as_deref().map(str::trim).map(point).transpose()?;
    Ok(LibrariesPlan {
        detail: key.as_deref().map(libraries::look_up).transpose()?,
        find,
        branches: args.branches,
        near,
        near_input: args.near.clone(),
        json,
    })
}

/// A plain text argument, trimmed, refused when it carries no text.
///
/// `blibs libraries --find "$NAME"` with `NAME` unset used to list nothing and say "no
/// library matched", which is a true statement about the wrong question.
fn text_argument(value: Option<&str>, named: &str) -> Result<Option<String>, UsageError> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(empty_value(named));
    }
    Ok(Some(value.to_owned()))
}

/// Resolve a `--at` list, preserving order and rejecting the first unknown entry.
///
/// Duplicates are dropped rather than refused: `--at hu,STABI,HU` is a shell alias plus a
/// habit, not a mistake, and rendering the HU block twice would be worse than quietly
/// showing it once. Two entries are the same location when they resolve to the same
/// canonical key and ISIL — **not** when they were spelled the same way, or `--at DE-11,HU`
/// would render the Humboldt block twice. The first spelling is the one that survives in
/// [`Location::given`]: it is the word that reached the tool first.
///
/// An **empty** entry is not the same kind of harmless: `--at "$LIBS"` with `LIBS` unset
/// searched the whole region, and `--at "HU,,FU"` quietly searched two libraries where
/// the user believed they had named three. Both look like an ordinary answer, so both
/// are refused here — including the trailing comma of `--at HU,`, which comes down the
/// same path and cannot be told apart from a variable that expanded to nothing.
///
/// Everything that survives carries the **canonical** ISIL from the list. The upstream
/// holdings filter is case-sensitive, so forwarding the user's spelling would silently
/// find nothing.
pub fn locations(entries: &[String]) -> Result<Vec<Location>, UsageError> {
    let mut resolved: Vec<Location> = Vec::new();
    for entry in entries {
        let typed = entry.trim();
        if typed.is_empty() {
            return Err(empty_value("--at"));
        }
        let location = libraries::resolve(typed)?;
        let known = resolved
            .iter()
            .any(|seen| seen.key == location.key && seen.isil == location.isil);
        if !known {
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
///
/// Every value is refused when it carries no text: an empty one used to be dropped on
/// the way through, and the search that ran without it was a *different* search wearing
/// a plausible answer. See [`empty_value`].
fn query(args: &SearchArgs) -> Result<QuerySpec, UsageError> {
    Ok(QuerySpec {
        terms: args
            .terms
            .iter()
            .map(|argument| term(argument, "a search term"))
            .collect::<Result<Vec<_>, _>>()?,
        title: field(args.title.as_deref(), "--title")?,
        subject: field(args.subject.as_deref(), "--subject")?,
        publisher: field(args.publisher.as_deref(), "--publisher")?,
        // Never a `Term`: `--author` is always a word list, never a phrase.
        author: author(args.author.as_deref())?,
        year: args.year.as_deref().map(year).transpose()?,
        identifier: args.isbn.as_deref().map(identifier).transpose()?,
    })
}

/// One search word or phrase, refused when the argument holds no text.
fn term(argument: &str, named: &str) -> Result<Term, UsageError> {
    let term = Term::from_argument(argument);
    if term.is_empty() {
        return Err(empty_value(named));
    }
    Ok(term)
}

/// The value of a field flag that is searched as a word or a phrase.
fn field(value: Option<&str>, flag: &str) -> Result<Option<Term>, UsageError> {
    value.map(|value| term(value, flag)).transpose()
}

/// `--author`, kept as a raw string because it is always searched as a word list.
fn author(value: Option<&str>) -> Result<Option<String>, UsageError> {
    let Some(name) = value.map(str::trim) else {
        return Ok(None);
    };
    if name.is_empty() {
        return Err(empty_value("--author"));
    }
    Ok(Some(name.to_owned()))
}

/// One of the counting flags, as far as clap could take it.
///
/// clap hands these over as `i64` (see `cli::COUNT_MAX`), so the only value that cannot
/// be a count is a negative one — the upper bound is already the parser's. Refusing it
/// here rather than leaving it to clap is the whole point: clap reads `--limit -1` as an
/// unknown flag and suggests `-- -1`, which would search for the number instead of
/// counting with it, while [`Limit::new`] and [`Page::new`] have first-class messages for
/// every other way of getting these wrong.
fn count(value: Option<i64>, flag: &str) -> Result<Option<u32>, UsageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    u32::try_from(value)
        .map(Some)
        .map_err(|_| UsageError::NegativeNumber {
            flag: flag.to_owned(),
            value: value.to_string(),
        })
}

/// A value the user gave that carries no text at all.
///
/// Almost always `--at "$LIBS"` or `--title "$Q"` with the variable unset. Dropping it
/// answered a question the user did not ask, with an answer that looks right — so it is
/// an exit 2 here, before anything goes out, and the hint names the likely cause.
fn empty_value(flag: &str) -> UsageError {
    UsageError::EmptyValue {
        flag: flag.to_owned(),
    }
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

/// The first piece of query text without a single letter or digit in it.
///
/// `search '@and'` came back with 8.55 million records: the catalogue reads a term of
/// pure punctuation as *no* term and answers with its whole index, which looks like a
/// successful search and states nothing. Refused here for the same reason a wildcard is,
/// and across the field flags too — `--title '@and'` is the same non-question.
///
/// The test is [`char::is_alphanumeric`] and deliberately not an ASCII range: a CJK
/// title, a Cyrillic or a Hebrew word and a year such as `1984` are all ordinary search
/// terms, and every one of them would be refused by an ASCII letter test.
///
/// Values with no text *at all* never reach this — [`query`] has already refused them by
/// name as [`empty_value`], which is the sharper message of the two.
fn first_without_text(args: &SearchArgs) -> Option<String> {
    query_text(args)
        .into_iter()
        .map(|(_, value)| value.trim())
        .find(|value| !value.is_empty() && !value.chars().any(char::is_alphanumeric))
        .map(str::to_owned)
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
/// Its own [`UsageError`] variant rather than a `clap::Error`: the clap wrapper carries
/// the `usage` kind and no hint at all, so an agent reading the JSON got neither a tag it
/// could switch on nor a next step — and the terminal printed `error:` twice, once from
/// clap's own `Display` and once from the renderer.
fn language(value: &str) -> Result<String, UsageError> {
    let code = value.trim().to_ascii_lowercase();
    if code.is_empty() {
        return Err(empty_value("--language"));
    }
    let well_formed =
        code.len() == LANGUAGE_CODE_LEN && code.chars().all(|c| c.is_ascii_lowercase());
    if !well_formed {
        return Err(UsageError::LanguageCode {
            input: value.to_owned(),
        });
    }
    if let Some(bibliographic) = bibliographic_variant(&code) {
        return Err(UsageError::LanguageCodeVariant {
            input: value.trim().to_owned(),
            bibliographic: bibliographic.to_owned(),
        });
    }
    Ok(code)
}

/// The bibliographic code for a terminology one, if that is what was given.
///
/// The shape check above cannot catch this: `deu` is three lowercase letters and passes
/// it, so the filter used to run, match nothing and exit 1 — "searched, does not exist"
/// for what is a mistyped flag, with advice to narrow a search that was never the
/// problem. It is the one input mistake that escaped as an exit 1.
///
/// A code in neither set is left alone on purpose: MARC data carries codes no list has,
/// and inventing a closed vocabulary here would refuse legitimate searches.
fn bibliographic_variant(code: &str) -> Option<&'static str> {
    TERMINOLOGY_CODES
        .iter()
        .find(|(terminology, _)| *terminology == code)
        .map(|(_, bibliographic)| *bibliographic)
}

/// Refuse a window a VÖBB branch cannot be paged to — **that branch's block, not the
/// whole run.**
///
/// voebb.de has no offset: reaching result 200 means walking ten result pages, one
/// sequential request each, on a session that must be replayed in order. So the depth is
/// capped at [`voebb::MAX_POSITION`] and a deeper window is refused **before** the session
/// is opened, rather than a minute of requests ending in a short answer.
///
/// **Whose refusal it is, is the whole point.** Until 2026-09-09 one VÖBB branch anywhere
/// in `--at` made this the invocation's error, so `--at HU,AGB --page 20 --limit 20` was
/// exit 2 with no block at all although HU reaches record 400 without effort — one
/// location's ceiling deciding for every other, which is the unevenness
/// `plan/feedback_round_3.md` §1.2 reported for the KOBV side one step later. The
/// returned keys are the locations that are *not* searched; they get an empty block and
/// [`crate::model::note_kinds::LOCATION_WINDOW_TOO_DEEP`], and every other location
/// answers.
///
/// **Exit 2 survives for the run that is nothing but those locations**, and it stays exit
/// 2 rather than becoming the KOBV side's exit 5 (`plan/cli.md` § *Exit-Codes*): here the
/// ceiling is known before a byte goes out, so the refusal can be a usage error, and no
/// note can help a user whose every block would be empty.
///
/// The window that is measured is the one the user is asking the service for, which is
/// why this passes [`Filters::is_active`] and **not** `Plan::anchored`: `--format` and
/// `--language` widen the window to 50 records anchored at record 1, which moves its end
/// and has to be measured, while `--sort` anchors that same block without asking for a
/// deeper one. Measuring the anchored window for `--sort` too would turn
/// `--at AGB --sort year --limit 50 --page 2` into a usage error although it reaches no
/// further into the result than page 1 does — an anchored window is one block of 50 by
/// construction and can never be too deep.
///
/// The KOBV side is not checked here — SRU takes `startRecord` directly, and how far a
/// result set reaches is knowable only to the service. It answers a window past the end
/// with diagnostic `1/61`, which one location owns in exactly the same way
/// ([`crate::error::Blast::Location`]).
fn check_window_depth(
    limit: Limit,
    page: Page,
    filters: &Filters,
    locations: &[Location],
) -> Result<Option<TooDeep>, UsageError> {
    let capped: Vec<&Location> = locations
        .iter()
        .filter(|at| at.engine == Engine::Voebb)
        .collect();
    if capped.is_empty() {
        return Ok(None);
    }
    let window = FetchWindow::plan(limit, page, filters.is_active());
    let position = window
        .start
        .saturating_add(u32::from(window.size.get()).saturating_sub(1));
    if position <= voebb::MAX_POSITION {
        return Ok(None);
    }
    if capped.len() == locations.len() {
        return Err(UsageError::WindowTooDeep {
            engine: Engine::Voebb,
            page: page.get(),
            limit: u32::from(limit.get()),
            position,
            max: voebb::MAX_POSITION,
        });
    }
    Ok(Some(TooDeep {
        keys: capped.into_iter().map(|at| at.key.clone()).collect(),
        position,
        max: voebb::MAX_POSITION,
    }))
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
/// `--sort availability` is the same sentence with one word changed: without a status
/// there is nothing to *order* by either, and the run said `"sort": {"by":"availability"}`
/// next to `"availability": "skipped"` — an order that was a no-op, asserted as though it
/// had happened. It is therefore one more row of this table and not a note: a note would
/// still leave the two claims standing side by side.
///
/// Deliberately not clap's `conflicts_with`: that produces [`UsageError::Cli`], whose
/// `kind` is the catch-all `usage` and which carries no hint, while every refusal here
/// owes the user the way out.
///
/// The pairs are a table, so the next conflict is one line rather than a second function.
fn check_flag_conflicts(args: &SearchArgs, sort: SortKey) -> Result<(), UsageError> {
    let conflicts = [
        (
            ("--available", args.available),
            ("--no-availability", args.no_availability),
        ),
        (
            ("--sort availability", sort == SortKey::Availability),
            ("--no-availability", args.no_availability),
        ),
    ];
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

/// The invocation echoed back in one string, for headings and for `query.terms`.
///
/// **Everything that narrowed the search, in the order it was assembled**: the free terms
/// first, unlabelled and with the quotes that made a phrase one, then the field flags as
/// `field: value`. The quotes are the only place a reader can see that `"Der Prozess"`
/// was searched as a phrase and not as two and-ed words, so they stay.
///
/// It used to stop at the free terms as soon as there were any, which broke exactly the
/// form `--help` advertises — free words and field flags combined with AND. The count
/// beside the echo belongs to the **whole** query, so `blibs search --title Prozess
/// Kafka` read as `552 results for Kafka` where `Kafka` alone has 13410, and an empty
/// result sent the reader off to vary a word that was never the cause. Either half alone
/// still reads exactly as it did: this is the shape both pure forms already had, with the
/// other half missing.
///
/// A label, not something to paste back — the field flags reproduce themselves, and the
/// paste-able half is [`QuerySpec::echo`], which stays free terms only for that reason.
///
/// The parts are joined with a comma and not with the ` · ` the rest of the output uses,
/// because the heading already puts that separator around the echo: `… · showing 1-1`
/// would read as one more field.
fn echo(args: &SearchArgs, query: &QuerySpec) -> String {
    let free = query.echo();
    let fields = query_text(args)
        .into_iter()
        // A free term carries no flag — the empty name is what distinguishes the two in
        // `query_text`, and the free terms are already in `free`, quoted as they were
        // searched.
        .filter_map(|(flag, value)| Some((flag.strip_prefix("--")?, value)))
        .map(|(field, value)| format!("{field}: {value}"));
    std::iter::once(free)
        .filter(|free| !free.is_empty())
        .chain(fields)
        .collect::<Vec<_>>()
        .join(", ")
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
/// Four ways to get this wrong, four answers — they used to share one, which told a user
/// who had typed `91,181` to give coordinates. What blibs cannot do is geocode, and only
/// the last of the four is actually that:
///
/// - two numbers outside the earth's ranges: name the ranges,
/// - one number where a pair is needed: name the comma,
/// - a shortcode with a typo: the library list's own `did you mean`,
/// - free text: the one case where "blibs looks up no addresses" is the answer.
fn point(input: &str) -> Result<LatLon, UsageError> {
    if input.trim().is_empty() {
        return Err(empty_value("--near"));
    }
    match LatLon::parse(input) {
        Ok(point) => Ok(point),
        Err(NotAPoint::OutOfRange) => Err(UsageError::NearCoordinatesOutOfRange {
            input: input.to_owned(),
        }),
        Err(NotAPoint::OnlyOneValue) => Err(UsageError::NearNeedsTwoValues {
            input: input.to_owned(),
        }),
        Err(NotAPoint::NotCoordinates) => locate(input),
    }
}

/// Where a library the user named sits — or why the name was not one.
///
/// A near-miss on a shortcode is answered with the list's own suggestions, the same
/// `did you mean STABI?` that `--at` gives: someone who typed `STABI2` was reaching for a
/// library, and telling them about coordinates sends them somewhere else entirely. Only
/// when nothing in the list is close does the geocoding answer apply.
///
/// A library that resolves but has no usable coordinates falls through to the same
/// answer, because from the user's side the effect is identical: this name cannot place
/// them on the map.
///
/// A **lookup**, not a `--at` resolution: "where is this" is a fair question about every
/// branch in the list, including the ones no engine can search on its own. Asking
/// [`libraries::resolve`] here refused `--near PHILBIB` with "cannot locate" although the
/// list holds its coordinates.
///
/// A `<house>/<branch>` path never falls through to the geocoding answer either, whether
/// it was ambiguous or simply wrong: nobody writes a slash meaning coordinates, so
/// "give coordinates or a library shortcode" would answer a question that was not asked.
fn locate(input: &str) -> Result<LatLon, UsageError> {
    match libraries::look_up(input) {
        Ok(entry) => entry
            .coords()
            .ok_or_else(|| UsageError::NearNeedsCoordinates {
                input: input.to_owned(),
            }),
        Err(UsageError::UnknownLibrary { input, suggestions })
            if !suggestions.is_empty() || input.contains(libraries::PATH_SEPARATOR) =>
        {
            Err(UsageError::UnknownLibrary { input, suggestions })
        }
        Err(ambiguous @ UsageError::AmbiguousBranch { .. }) => Err(ambiguous),
        Err(_) => Err(UsageError::NearNeedsCoordinates {
            input: input.to_owned(),
        }),
    }
}

/// A value clap's own parser should already have rejected.
///
/// Reachable only if [`SORT_VALUES`]/[`FORMAT_VALUES`] and the tables above drift apart,
/// which the tests below prevent. It is still an error rather than a panic: nothing on a
/// path reachable from user input may panic, and a wrong value is a usage error whichever
/// side put it there.
///
/// It carries its own variant rather than a fabricated `clap::Error`. Nothing in the
/// library raises one of those: clap's `Display` already begins with `error:`, so a
/// wrapper printed through the ordinary renderer said it twice, and its `kind` is the
/// catch-all `usage` with no hint at all.
fn unreachable_value(flag: &str, got: &str, expected: &[&str]) -> UsageError {
    UsageError::UnsupportedValue {
        flag: flag.to_owned(),
        got: got.to_owned(),
        expected: expected.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use clap::{Args as _, Parser};

    use super::*;
    use crate::cli::{Cli, Command, Plan};
    use crate::error::ExitCode;
    use crate::libraries::Entry;

    /// Parse a command line exactly as the binary does, then validate it. Going through
    /// clap rather than constructing `SearchArgs` by hand means these tests also cover
    /// the flag wiring — `--at a,b` splitting, `--json` after the subcommand, and so on.
    fn parse(args: &[&str]) -> Result<Cli, Error> {
        let command_line = std::iter::once("blibs").chain(args.iter().copied());
        // The same construction `main::refused` performs: a clap refusal carries the
        // command whose help answers it, resolved here because only `cli` knows it.
        Cli::try_parse_from(command_line).map_err(|source| {
            UsageError::Cli {
                command: crate::cli::command_path(&source),
                source,
            }
            .into()
        })
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
        assert_eq!(plan.terms_echo, "isbn: 978-3-596-29433-6");
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

    /// A search made of field flags alone is *summarised*, not rebuilt as a command
    /// line: `13 results for "--title Prozess --author Kafka --year 1953"` read as a
    /// complaint about the flags rather than as an echo of the search.
    #[test]
    fn a_flags_only_search_echoes_as_a_summary_of_its_fields() {
        let plan = search(&[
            "search", "--title", "Prozess", "--author", "Kafka", "--year", "1953",
        ])
        .expect("a query made of flags");
        assert_eq!(plan.terms_echo, "title: Prozess, author: Kafka, year: 1953");
    }

    /// The quotes a phrase carries are the only place a reader sees that it was searched
    /// as a phrase, so the echo keeps them — and carries them exactly once. Formatting
    /// this string again with `{:?}` produced `"\"Der Prozess\""`, which hid the very
    /// signal the quotes are there to give.
    #[test]
    fn the_echo_quotes_a_phrase_once_and_a_word_not_at_all() {
        let plan = search(&["search", "Der Prozess"]).expect("a phrase");
        assert_eq!(plan.terms_echo, "\"Der Prozess\"");

        let plan = search(&["search", "Kafka", "Prozess"]).expect("two words");
        assert_eq!(plan.terms_echo, "Kafka Prozess");
    }

    /// §1.5: free words and field flags combine with AND — the form `--help` advertises —
    /// and the count printed beside the echo is the count of that whole query. Echoing
    /// only the free words made `552 results for Kafka` out of a search that also carried
    /// `--title Prozess`, where `Kafka` alone has 13410, and turned an empty result into
    /// advice about a word that was not the cause.
    #[test]
    fn a_mixed_search_echoes_the_free_words_and_the_fields() {
        let plan = search(&["search", "Kafka", "--title", "Prozess"]).expect("the mixed form");
        assert_eq!(plan.terms_echo, "Kafka, title: Prozess");

        let plan = search(&["search", "Prozess", "--year", "1953", "--author", "Kafka"])
            .expect("two flags beside a free word");
        assert_eq!(plan.terms_echo, "Prozess, author: Kafka, year: 1953");

        // A phrase keeps its quotes here too: they are the only place a reader sees that
        // it was not searched as two and-ed words.
        let plan = search(&["search", "Der Prozess", "--author", "Kafka"]).expect("a phrase");
        assert_eq!(plan.terms_echo, "\"Der Prozess\", author: Kafka");
    }

    /// An empty query is diagnostic 1/10 upstream, which would surface as exit 5 for
    /// what is plainly a usage error. "Empty" here means nothing was given at all — an
    /// argument that *was* given and holds no text is the sharper `empty_value`.
    #[test]
    fn an_empty_query_is_refused() {
        assert_eq!(usage_kind(&["search"]), "empty_query");
        assert_eq!(usage_kind(&["search", "--at", "HU"]), "empty_query");
    }

    /// Every value the user can give is refused when it carries no text, whatever else
    /// the invocation holds: dropping it left a search that ran, looked ordinary, and
    /// answered a different question. `--year` and `--isbn` are covered by their own
    /// stricter checks and keep those.
    #[test]
    fn an_empty_value_is_refused_for_every_flag_that_takes_text() {
        let cases: [&[&str]; 7] = [
            &["search", "   "],
            &["search", "Kafka", ""],
            &["search", "Kafka", "--title", ""],
            &["search", "Kafka", "--author", " "],
            &["search", "Kafka", "--subject", ""],
            &["search", "Kafka", "--publisher", ""],
            &["search", "Kafka", "--language", ""],
        ];
        for args in cases {
            assert_eq!(usage_kind(args), "empty_value", "{args:?}");
        }
    }

    /// The message has to name the flag, or a user with three of them expanded from
    /// variables cannot tell which one was empty.
    #[test]
    fn an_empty_value_names_the_flag_and_the_likely_cause() {
        let Err(error) = search(&["search", "Kafka", "--title", ""]) else {
            panic!("an empty --title must be refused");
        };
        assert_eq!(error.to_string(), "--title was given an empty value");
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("--title"), "{hint}");
        assert!(hint.contains("shell variable"), "{hint}");
    }

    /// `libraries` takes text too, and an unset variable there produced "no library
    /// matched" — a true answer to a question nobody asked.
    #[test]
    fn the_libraries_command_refuses_empty_text_too() {
        for args in [
            vec!["libraries", "--find", ""],
            vec!["libraries", "  "],
            vec!["libraries", "--near", " "],
        ] {
            let Err(error) = libraries_plan(&args) else {
                panic!("{args:?} must be refused");
            };
            assert_eq!(error.kind(), "empty_value", "{args:?}");
        }
    }

    /// §3.7: the catalogue reads a term of pure punctuation as *no* term and answers with
    /// its whole index, which looks like a successful search and states nothing. Refused
    /// wherever it is typed, exactly as a wildcard is.
    #[test]
    fn a_term_without_letters_or_digits_is_refused() {
        // A term starting with `-` never reaches this module: clap reads it as a flag,
        // which is its business and unchanged here.
        for term in ["+++", "...", "@@@", "!?", "«»"] {
            // `?` and `*` are wildcards and keep their own, more specific message.
            let expected = if term.contains(WILDCARDS) {
                "wildcard_unsupported"
            } else {
                "term_without_text"
            };
            assert_eq!(usage_kind(&["search", term]), expected, "{term}");
        }
        assert_eq!(
            usage_kind(&["search", "Kafka", "--title", "###"]),
            "term_without_text"
        );
        let Err(error) = search(&["search", "###"]) else {
            panic!("a term of pure punctuation must be refused");
        };
        assert!(error.to_string().contains("###"), "{error}");
        assert_eq!(error.exit(), ExitCode::Usage, "{error}");
    }

    /// "Alphanumeric" is Unicode's, not ASCII's: a year, a CJK title, a Cyrillic, Greek
    /// or Hebrew word are all ordinary search terms in this catalogue — there are
    /// fixtures for each — and an ASCII letter test would refuse every one of them.
    #[test]
    fn a_term_in_any_script_is_still_a_term() {
        for term in ["1984", "日本", "Достоевский", "Ἰλιάς", "תלמוד", "Œuvres"]
        {
            assert!(
                search(&["search", term]).is_ok(),
                "{term} is a search term like any other"
            );
        }
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

    /// §3.7: a negative count used to fall through to clap, which read `-1` as a flag and
    /// offered `-- -1` — the one tip that would turn the number into a **search term**.
    /// The two counting flags have first-class messages for every other way of getting
    /// them wrong, and this is the same mistake with a sign in front of it.
    #[test]
    fn a_negative_count_is_refused_by_blibs_and_not_by_clap() {
        for (flag, value) in [("--limit", "-1"), ("--page", "-3"), ("--limit", "-50")] {
            let Err(error) = search(&["search", "Kafka", flag, value]) else {
                panic!("{flag} {value} must be refused");
            };
            assert_eq!(error.kind(), "negative_number", "{flag} {value}");
            assert_eq!(error.exit(), ExitCode::Usage, "{error}");
            assert!(error.to_string().contains(flag), "{error}");
            assert!(error.to_string().contains(value), "{error}");
            let hint = error.hint().unwrap_or_default();
            assert!(hint.contains(flag), "{hint}");
        }
        // The in-range failures keep their own, sharper messages.
        assert_eq!(
            usage_kind(&["search", "Kafka", "--limit", "0"]),
            "limit_out_of_range"
        );
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

    /// `--at "$LIBS"` with the variable unset used to search the whole region, and
    /// `--at HU,,FU` used to search two of the three libraries the user named. Both
    /// answers look ordinary, which is what makes them worth refusing.
    #[test]
    fn an_empty_at_entry_is_refused() {
        for value in ["", "HU,,FU", "HU,", " "] {
            assert_eq!(
                usage_kind(&["search", "Kafka", "--at", value]),
                "empty_value",
                "{value:?}"
            );
        }
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

    /// A branch of a university is a location like any other, answered by `kobv`: its
    /// house is filtered upstream and the branch is read off the copies.
    #[test]
    fn a_non_voebb_branch_is_a_kobv_location() {
        let plan = search(&["search", "Kafka", "--at", "PHILBIB"]).expect("a branch of the FU");
        let [location] = plan.locations.as_slice() else {
            panic!("one location");
        };
        assert_eq!(location.engine, Engine::Kobv);
        assert_eq!(location.isil.as_str(), "DE-188");
        let branch = location
            .branch
            .as_ref()
            .expect("a branch location names it");
        assert_eq!(branch.kobvid, "FUB00019");
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

    /// A client-side filter anchors the window at position 1 whatever `--page` says, so it
    /// can never reach past voebb.de's paging depth — the depth check has nothing left to
    /// refuse. Without the filter the same page number still steps and can still be too
    /// deep.
    #[test]
    fn a_filtered_window_cannot_be_too_deep() {
        let deep = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "50", "--page", "6",
        ];
        assert_eq!(usage_kind(deep), "window_too_deep");
        assert!(
            search(&[deep.as_slice(), &["--format", "book"]].concat()).is_ok(),
            "the filtered window is one anchored block and stays inside the depth"
        );
    }

    /// One catalogue's ceiling may not answer for the other. `--at HU,AGB --page 20
    /// --limit 20` asks for result 400: AGB cannot be paged there and HU reaches it
    /// without effort, and until 2026-09-09 the whole invocation was exit 2 with no block
    /// at all — the same unevenness §1.2 reported for the KOBV side, one step earlier.
    ///
    /// Only the VÖBB location is dropped from the searches; it keeps its place in
    /// `locations`, because that is what gives it its (empty) block and its note. And
    /// both engines stay in `engines()`: a block under a catalogue the document says was
    /// never asked would be worse than the ceiling itself.
    #[test]
    fn one_locations_paging_ceiling_does_not_refuse_the_others() {
        let deep = &[
            "search", "Vorleser", "--at", "HU,AGB", "--limit", "20", "--page", "20",
        ];
        let plan = search(deep).expect("HU reaches result 400, so the run is not refused");
        let too_deep = plan
            .too_deep
            .as_ref()
            .expect("AGB cannot be paged to result 400");
        assert_eq!(too_deep.keys, vec!["AGB".to_owned()]);
        assert_eq!(too_deep.max, voebb::MAX_POSITION);
        assert_eq!(too_deep.position, 400);

        let searched = plan.by_engine();
        assert_eq!(searched.len(), 1, "{searched:?}");
        assert_eq!(searched[0].0, Engine::Kobv);
        assert_eq!(searched[0].1.len(), 1, "AGB is not searched: {searched:?}");
        assert_eq!(
            plan.locations.len(),
            2,
            "the refused location keeps its block"
        );
        assert_eq!(plan.engines(), vec![Engine::Kobv, Engine::Voebb]);
    }

    /// The other half of the same rule: with **nothing but** VÖBB locations there is no
    /// block left for an empty one to stand beside, so the invocation stays exit 2 — and
    /// stays exit 2 rather than turning into the KOBV side's exit 5, because this ceiling
    /// is known before a byte goes out (`plan/cli.md` § *Exit-Codes*).
    #[test]
    fn a_run_of_nothing_but_voebb_locations_is_still_a_usage_error() {
        let deep = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "20", "--page", "20",
        ];
        let Err(error) = search(deep) else {
            panic!("a window past the tenth page is refused");
        };
        assert_eq!(error.exit(), ExitCode::Usage, "{error}");
        assert_eq!(error.kind(), "window_too_deep");
        assert!(error.to_string().contains("--page 20"), "{error}");

        // Two of them, still nothing else: the same refusal.
        assert_eq!(
            usage_kind(&[
                "search", "Vorleser", "--at", "AGB,BSTB", "--limit", "20", "--page", "20",
            ]),
            "window_too_deep"
        );
    }

    /// `--sort` anchors the window too, but it must **not** move the depth boundary.
    ///
    /// The check measures what the user is asking voebb.de for, which is why it reads
    /// `Filters::is_active()` and not `Plan::anchored()`. `--at AGB --limit 50 --page 6`
    /// is too deep with and without `--sort`; a `--sort` that made it legal would be a
    /// usage error appearing and disappearing for a reason no finding ever asked for, and
    /// one that made an otherwise legal invocation illegal would be worse.
    #[test]
    fn sorting_does_not_move_the_voebb_depth_boundary() {
        let inside = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "50", "--page", "2",
        ];
        assert!(search(inside).is_ok(), "page 2 of 50 ends at 100");
        assert!(
            search(&[inside.as_slice(), &["--sort", "year"]].concat()).is_ok(),
            "--sort anchors the window, it does not deepen it"
        );

        let outside = &[
            "search", "Vorleser", "--at", "AGB", "--limit", "50", "--page", "6",
        ];
        assert_eq!(usage_kind(outside), "window_too_deep");
        assert_eq!(
            usage_kind(&[outside.as_slice(), &["--sort", "year"]].concat()),
            "window_too_deep"
        );
    }

    /// §2.9: a client-side sort sees only the fetched records, so its pages have to
    /// partition **one** ordered block — page 2 of a stepped window can be newer than
    /// page 1, which is not a sort. The window and the cut have to agree on that, or the
    /// page claims a position that was never fetched.
    #[test]
    fn a_sort_anchors_the_window_and_the_cut_together() {
        let plan = search(&["search", "Kafka", "--sort", "title", "--page", "2"])
            .expect("a plain sorted search");
        assert!(plan.anchored());
        assert_eq!(plan.window().start, 1, "the block stays at record 1");
        assert_eq!(
            plan.window().size.get(),
            crate::model::page::MAX_SRU_PAGE_SIZE,
            "the block is the widest the service serves"
        );
        assert_eq!(
            plan.cut().offset,
            usize::from(plan.limit.get()),
            "page 2 walks the matches inside the block"
        );

        // `--sort relevance` is the upstream order and touches nothing, so it anchors
        // nothing either: the window steps as it always did.
        let stepped = search(&["search", "Kafka", "--sort", "relevance", "--page", "2"])
            .expect("a relevance search");
        assert!(!stepped.anchored());
        assert_eq!(stepped.window().start, 11);
        assert_eq!(stepped.cut().offset, 0);
    }

    /// §2.3: `key` is canonical and `given` is what stood in `--at`, so a block can be
    /// found again under the name it was asked for. `--at ZLB` is the case that made an
    /// agent conclude "ZLB has no hits".
    #[test]
    fn a_location_remembers_the_word_the_user_typed() {
        let plan = search(&["search", "Kafka", "--at", "ZLB"]).expect("ZLB is a known library");
        assert_eq!(plan.locations[0].key, "VOEBB");
        assert_eq!(plan.locations[0].given, "ZLB");

        // Deduplication is by identity, not by spelling: `--at DE-11,HU` is one library
        // and keeps the first word the user wrote for it.
        let plan = search(&["search", "Kafka", "--at", "DE-11,HU"]).expect("known libraries");
        assert_eq!(plan.locations.len(), 1);
        assert_eq!(plan.locations[0].given, "DE-11");
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
        // §1.5: the echo names the field flags beside the free words now — the count
        // printed with it is the count of the whole query, and this invocation is the
        // mixed form the old echo silently dropped half of.
        assert_eq!(
            plan.terms_echo,
            "\"Der Prozess\" Kafka, author: Franz Kafka, year: 1953"
        );
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

    /// §2.10: the reasoning behind the pair above, word for word — with no status there
    /// is nothing to *order* by either. The run reported `"sort": {"by":"availability"}`
    /// beside `"availability": "skipped"`, an order that was a no-op stated as though it
    /// had happened.
    #[test]
    fn sorting_by_availability_conflicts_with_no_availability() {
        let Err(error) = search(&[
            "search",
            "Kafka",
            "--sort",
            "availability",
            "--no-availability",
        ]) else {
            panic!("there is nothing to sort by when nothing is asked");
        };
        assert_eq!(error.exit(), ExitCode::Usage, "{error}");
        assert_eq!(error.kind(), "conflicting_flags");
        for flag in ["--sort availability", "--no-availability"] {
            assert!(error.to_string().contains(flag), "{error}");
        }
        assert!(error.hint().is_some(), "a usage error names the way out");

        // Every other sort key orders what the records themselves say, so it is none of
        // availability's business.
        for key in ["relevance", "year", "title", "author"] {
            assert!(
                search(&["search", "Kafka", "--sort", key, "--no-availability"]).is_ok(),
                "--sort {key} needs no status"
            );
        }
        // And the sort on its own is exactly what it always was.
        assert!(search(&["search", "Kafka", "--sort", "availability"]).is_ok());
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

    /// Its own kind, not clap's catch-all `usage`: an agent switches on the kind, and
    /// `usage` told it only that *something* about the command line was wrong. The hint
    /// is the other half — the clap wrapper had none at all.
    #[test]
    fn a_language_must_be_a_three_letter_code() {
        for value in ["de", "german", "g3r", "GER1"] {
            assert_eq!(
                usage_kind(&["search", "Kafka", "--language", value]),
                "invalid_language",
                "{value}"
            );
        }
        let Err(error) = search(&["search", "Kafka", "--language", "de"]) else {
            panic!("a two-letter code must be refused");
        };
        assert!(error.to_string().contains("three-letter"), "{error}");
        assert!(!error.to_string().contains("error:"), "{error}");
        assert!(
            error.hint().unwrap_or_default().contains("--language ger"),
            "{error}"
        );
    }

    /// §1.7: `deu` passes the shape check, so the filter used to run, match nothing and
    /// exit 1 — "searched, does not exist" for a mistyped flag, with advice to narrow a
    /// search that was never the problem. It was the only input mistake in the whole test
    /// run that escaped as an exit 1.
    #[test]
    fn a_terminology_code_names_the_bibliographic_one() {
        for (terminology, bibliographic) in TERMINOLOGY_CODES {
            let Err(error) = search(&["search", "Kafka", "--language", terminology]) else {
                panic!("--language {terminology} must be redirected");
            };
            assert_eq!(error.kind(), "language_code_variant", "{terminology}");
            assert_eq!(error.exit(), ExitCode::Usage, "{error}");
            assert!(error.to_string().contains(terminology), "{error}");
            assert!(
                error.hint().unwrap_or_default().contains(bibliographic),
                "the hint must name the code that was meant: {error}"
            );
        }
        // Case is the user's business, and the message shows what they typed.
        let Err(error) = search(&["search", "Kafka", "--language", "DEU"]) else {
            panic!("case does not make it a different code");
        };
        assert_eq!(error.kind(), "language_code_variant");
        assert!(error.to_string().contains("DEU"), "{error}");
    }

    /// A code in neither set keeps today's behaviour: MARC data carries codes no list
    /// has, so the shape check passes and the filter simply matches nothing. Inventing a
    /// closed vocabulary here would refuse legitimate searches.
    #[test]
    fn a_code_that_is_neither_variant_is_still_accepted() {
        for value in ["xyz", "ger", "eng", "grc", "gsw"] {
            let plan = search(&["search", "Kafka", "--language", value])
                .unwrap_or_else(|error| panic!("--language {value} must pass: {error}"));
            assert_eq!(plan.filters.language.as_deref(), Some(value));
        }
    }

    /// The table is a redirect, so it must not contain a cycle: no bibliographic code may
    /// appear on the left, or `--language ger` would be sent somewhere else again. The
    /// shape is checked here too, because a mistyped entry would silently never match.
    #[test]
    fn the_terminology_table_redirects_once_and_only_once() {
        for (terminology, bibliographic) in TERMINOLOGY_CODES {
            for code in [terminology, bibliographic] {
                assert_eq!(code.len(), LANGUAGE_CODE_LEN, "{code}");
                assert!(code.chars().all(|c| c.is_ascii_lowercase()), "{code}");
            }
            assert_ne!(terminology, bibliographic);
            assert!(
                bibliographic_variant(bibliographic).is_none(),
                "{bibliographic} is a target and must not also be redirected"
            );
        }
        let mut terminology: Vec<&str> = TERMINOLOGY_CODES.iter().map(|(t, _)| *t).collect();
        terminology.sort_unstable();
        terminology.dedup();
        assert_eq!(terminology.len(), TERMINOLOGY_CODES.len());
    }

    /// The codes the records actually carry, in the case the index wants.
    #[test]
    fn a_three_letter_code_is_normalised_to_lowercase() {
        let plan = search(&["search", "Kafka", "--language", "GER"]).expect("a valid code");
        assert_eq!(plan.filters.language.as_deref(), Some("ger"));
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

    /// A detail key is resolved here, so that everything downstream is about a library
    /// that exists — and a branch key stays a branch instead of becoming its house.
    #[test]
    fn a_detail_key_is_resolved_to_what_it_names() {
        let plan = libraries_plan(&["libraries", "stabi"]).expect("STABI is a library");
        assert!(
            matches!(plan.detail, Some(Entry::Institution(library)) if library.isil == "DE-1"),
            "{:?}",
            plan.detail
        );
        assert!(plan.find.is_none());

        let plan = libraries_plan(&["libraries", "agb"]).expect("AGB is a branch");
        assert!(
            matches!(
                plan.detail,
                Some(Entry::Branch { branch, .. }) if branch.kobvid == "SIG00036"
            ),
            "{:?}",
            plan.detail
        );
    }

    /// A typo in a detail key is exit 2 with the list's own suggestion, exactly as in
    /// `--at`: a lookup of a name the user chose, answered with an empty list, is
    /// indistinguishable from "no such library exists".
    #[test]
    fn an_unknown_detail_key_is_a_usage_error() {
        let Err(error) = libraries_plan(&["libraries", "STABI2"]) else {
            panic!("an unknown key must be refused");
        };
        assert_eq!(error.kind(), "unknown_library");
        assert_eq!(error.exit(), ExitCode::Usage);
        assert!(
            error.hint().unwrap_or_default().contains("STABI"),
            "the hint must offer the near miss"
        );
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

    /// The one thing `unreachable_value` must do is stay an error rather than a panic —
    /// and, since it is printed through the ordinary renderer, carry a kind and a hint of
    /// its own rather than clap's catch-all.
    #[test]
    fn a_drifted_value_is_an_error_not_a_panic() {
        let error = sort_key(Some("nonsense")).expect_err("not an advertised value");
        assert_eq!(error.kind(), "unsupported_value");
        assert!(error.to_string().contains("--sort"), "{error}");
        assert!(
            error.hint().unwrap_or_default().contains("bug in blibs"),
            "{error}"
        );
    }
}
