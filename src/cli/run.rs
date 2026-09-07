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
//!
//! ## Which stream gets what
//!
//! `out` carries the answer and nothing else — the result blocks, the JSON document, the
//! library table. `err` carries the explanation of an empty answer, the same way an error
//! goes there. So `blibs search zzz` writes nothing to stdout, exits 1, and says why on
//! stderr; a pipeline never has to tell a message apart from a result.

use std::io::{self, Write};

use crate::cli::{Cli, Command, LibrariesPlan, Plan, ShowPlan, long_help, validate};
use crate::engine::kobv::Kobv;
use crate::engine::voebb::Voebb;
use crate::error::{EmptyReason, Error, Outcome, UnexpectedError};
use crate::http::{Fetch, scope_map};
use crate::libraries::{self, Library};
use crate::model::{
    AtBlock, AvailabilityMode, Catalog, Engine, EngineSearch, Location, Note, QueryEcho, Record,
    SearchRequest, SearchResult, SortKey, SortScope, SortSpec, WindowInfo, note_kinds,
};
use crate::render::{self, Style};
use crate::select;

/// Run one invocation and write its output.
///
/// Returns [`Outcome`] for the two success cases (found / found nothing) and [`Error`]
/// for the five failure categories. The exit code comes from whichever of the two came
/// back; `main` does not decide it.
///
/// `out` is stdout, `err` is stderr, and `style` is what [`Style::detect`] made of the
/// terminal — all three are parameters so that the whole tool is testable without a
/// process.
pub fn run(
    cli: &Cli,
    fetch: &dyn Fetch,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    match &cli.command {
        Some(Command::Search(args)) => run_search(
            &validate(args, cli.json, cli.cache())?,
            fetch,
            out,
            err,
            style,
        ),
        Some(Command::Show(args)) => run_show(
            &validate::validate_show(args, cli.json, cli.cache())?,
            fetch,
            out,
            err,
            style,
        ),
        Some(Command::Libraries(args)) => run_libraries(
            &validate::validate_libraries(args, cli.json)?,
            out,
            err,
            style,
        ),
        // `blibs` on its own is a request for the documentation, not a mistake.
        None => {
            writeln!(out, "{}", long_help()).map_err(output)?;
            Ok(Outcome::Found)
        }
    }
}

/// What one engine contributed, after selection and availability.
///
/// `search.records` has been filtered, sorted and cut to the page; `search.fetched` still
/// counts what came back, which is what makes "nothing matched" distinguishable from
/// "nothing exists".
struct EngineOutcome {
    search: EngineSearch,
    /// How many records survived the client-side filters, before paging.
    after_filter: usize,
    /// How many displayed records `--available` judged, or `None` when it did not run.
    /// The survivors are already counted by `search.records`; this is the number the
    /// difference is taken from.
    before_available: Option<usize>,
    /// What `--available` removed from this engine's page, split into "said no" and
    /// "said nothing".
    hidden: select::Hidden,
}

/// `search`: one thread per engine, then one document out of all of them.
fn run_search(
    plan: &Plan,
    fetch: &dyn Fetch,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let outcomes = search_engines(plan, fetch)?;
    let unstated = unstated_hidden(&outcomes);
    let result = assemble_result(plan, outcomes, unstated);
    let outcome = outcome_of(plan, &result, unstated);

    if plan.json {
        render::json::write(&result, out)?;
        return Ok(outcome);
    }
    // With locations every block is rendered, including the ones that hold nothing: a
    // missing block cannot be told apart from a forgotten one. Without them an empty
    // result has nothing to render, and only the explanation is left.
    if !plan.locations.is_empty() || result.shown > 0 {
        render::human::search(&result, &plan.locations, out, style).map_err(output)?;
    }
    if let Outcome::Empty(reason) = &outcome {
        render::human::empty(reason, err).map_err(output)?;
    }
    Ok(outcome)
}

/// Run every engine this invocation has locations for, at the same time.
///
/// The first error by engine order wins and the whole invocation fails with it — two
/// catalogues answering half a question is worse than one catalogue saying it could not.
fn search_engines(plan: &Plan, fetch: &dyn Fetch) -> Result<Vec<EngineOutcome>, Error> {
    scope_map(plan.by_engine(), |(engine, locations)| {
        search_one_engine(plan, engine, &locations, fetch)
    })
}

/// One engine: search, filter, sort, page, availability, keep-available — in that order
/// and no other.
///
/// Availability is fetched for the records that survived paging, because it costs one
/// request each. That is also why [`SortKey::Availability`] sorts twice: the first pass
/// has no statuses to work with, and only the second one — over the page — can order by
/// something that exists.
///
/// Paging is where `--at` changes the shape: `--limit` is a promise **per block**, so
/// with locations every block is cut to it on its own and a record no block still shows
/// is dropped before availability is asked for.
///
/// `--available` is the one filter that runs *last*, for the same reason the second sort
/// does: before availability was fetched there is no status to judge. It therefore
/// **thins the page out rather than filling it up** — ten hits become four, and asking
/// for status on more records than are displayed to refill it is exactly the bulk traffic
/// this tool does not generate. A larger `--limit` is the way to see more.
fn search_one_engine(
    plan: &Plan,
    engine: Engine,
    locations: &[Location],
    fetch: &dyn Fetch,
) -> Result<EngineOutcome, Error> {
    let catalog = catalog_for(engine, fetch);
    let request = SearchRequest {
        query: plan.query.clone(),
        locations: locations.to_vec(),
        window: plan.window(),
    };

    let mut search = catalog.search(&request)?;

    // Before every count, so that `fetched`, `after_filter` and the block totals are all
    // over distinct records — and before `fill_availability`, so a repeat never costs a
    // second request for a status already known.
    let (deduplicated, dropped) = select::dedup(std::mem::take(&mut search.records));
    if dropped > 0 {
        // `fetched` counts what the window holds, and after this it holds distinct
        // records. Leaving the raw number would make `after_filter < fetched` true with no
        // filter set at all, and the footer would blame filters that never ran.
        search.fetched = search.fetched.saturating_sub(dropped);
        search.notes.push(Note::new(
            note_kinds::DUPLICATE_RECORDS_DROPPED,
            format!(
                "the catalogue delivered {} {} twice in this window; the repeats were dropped",
                dropped,
                if dropped == 1 { "record" } else { "records" }
            ),
        ));
    }
    if engine == Engine::Kobv && plan.page.get() > 1 {
        search.notes.push(Note::new(
            note_kinds::RESULT_ORDER_UNSTABLE,
            "the KOBV catalogue does not order a result stably — two identical requests \
             return different records, so this page may overlap the previous one or leave \
             records out",
        ));
    }

    let filtered = select::filter(deduplicated, &plan.filters);
    let after_filter = filtered.len();

    let sort_at = sort_location(locations);
    let mut records = filtered;
    select::sort(&mut records, plan.sort, sort_at);
    let mut records = if locations.is_empty() {
        select::take_page(records, plan.cut())
    } else {
        select::take_page_per_location(records, &mut search.at, plan.cut())
    };

    if plan.availability == AvailabilityMode::Fetched {
        search
            .notes
            .extend(catalog.fill_availability(&mut records)?);
        if plan.sort == SortKey::Availability {
            select::sort(&mut records, plan.sort, sort_at);
        }
    }

    // Outside the availability block, not inside it: `--available` without a status is
    // an empty page, and that is a property of the data rather than of the rule in
    // `validate` that forbids the flag next to `--no-availability`. Reading top to
    // bottom, the page is fetched, then sieved by status, then marked, then cut into
    // blocks.
    //
    // The filter judges by *this engine's* locations, never by `plan.locations`: with
    // `--at HU,AGB` the kobv run would otherwise look for an AGB block it never had and
    // find no statement about it. `mark_mine` below stays on `plan.locations` on
    // purpose — "one of mine" is a question about the whole invocation.
    let mut before_available = None;
    let mut hidden = select::Hidden::default();
    if plan.only_available {
        before_available = Some(records.len());
        let (kept, removed) = if locations.is_empty() {
            select::keep_available(records)
        } else {
            select::keep_available_per_location(records, &mut search.at, locations)
        };
        records = kept;
        hidden = removed;
    }

    select::mark_mine(&mut records, &plan.locations);

    search.records = records;
    // Last, and after the second sort: `at[].records` is the block, and it has to name
    // the displayed records in the order they are printed in.
    select::assign_blocks(&mut search.at, &search.records);
    Ok(EngineOutcome {
        search,
        after_filter,
        before_available,
        hidden,
    })
}

/// Which location `--sort availability` should judge by.
///
/// Exactly one location means "is it in *there*", and that is the light to sort on. With
/// several, no single location speaks for the record, so the record's overall light is
/// the honest key.
fn sort_location(locations: &[Location]) -> Option<&Location> {
    match locations {
        [only] => Some(only),
        _ => None,
    }
}

/// The engine that answers for these locations.
///
/// Boxed because the two engines are different types and `run` must not branch on which
/// one it got — everything downstream sees a [`Catalog`]. The choice is made **once**,
/// from `--at` or from a record id's prefix, and nothing after this line knows which
/// catalogue answered.
fn catalog_for<'f>(engine: Engine, fetch: &'f dyn Fetch) -> Box<dyn Catalog + 'f> {
    match engine {
        Engine::Kobv => Box::new(Kobv::new(fetch)),
        Engine::Voebb => Box::new(Voebb::new(fetch)),
    }
}

/// Build the one document both renderers consume.
///
/// Record-centric: the records of both engines follow each other, and the grouping humans
/// see is derived from `at[]` — never a second list of records.
///
/// `unstated` comes from [`unstated_hidden`] over the same outcomes; it is passed in
/// rather than recomputed here so that the note and [`outcome_of`] cannot end up naming
/// two different numbers for one thing.
fn assemble_result(plan: &Plan, outcomes: Vec<EngineOutcome>, unstated: usize) -> SearchResult {
    let engines: Vec<Engine> = outcomes
        .iter()
        .map(|outcome| outcome.search.engine)
        .collect();
    let at = at_blocks(plan, &outcomes);
    let window = WindowInfo {
        fetched: outcomes.iter().map(|o| o.search.fetched).sum(),
        after_filter: outcomes.iter().map(|o| o.after_filter).sum(),
        filtered: plan.filters.is_active(),
        undelivered: outcomes.iter().map(|o| o.search.undelivered).sum(),
        // `None` unless some engine actually ran the filter — the renderers read the
        // field as "did `--available` run", and a zero would answer that with "yes".
        before_available: outcomes
            .iter()
            .filter_map(|outcome| outcome.before_available)
            .reduce(|left, right| left + right),
    };
    // `total` and `pqf` are the KOBV search's, and `null` when only voebb ran, or when
    // several KOBV searches did: two catalogues have two totals, and so do two locations,
    // and adding them up would invent a number that is true of neither. The per-location
    // numbers are in `at[]`, where they are always true.
    let kobv = outcomes
        .iter()
        .find(|outcome| outcome.search.engine == Engine::Kobv);
    let total = kobv.and_then(|outcome| outcome.search.total);
    let pqf = kobv.and_then(|outcome| outcome.search.query_echo.clone());

    let mut notes = Vec::new();
    if let Some(note) = unstated_note(unstated) {
        notes.push(note);
    }
    let mut records = Vec::new();
    for outcome in outcomes {
        notes.extend(outcome.search.notes);
        records.extend(outcome.search.records);
    }

    SearchResult {
        query: QueryEcho {
            terms: plan.terms_echo.clone(),
            pqf,
        },
        total,
        shown: records.len(),
        page: plan.page,
        limit: usize::from(plan.limit.get()),
        sort: SortSpec {
            by: plan.sort,
            scope: SortScope::Fetched,
        },
        window,
        engines,
        at,
        availability: plan.availability,
        notes,
        records,
    }
}

/// How many records `--available` removed without ever having been told a status, over
/// all engines.
///
/// Only this half of [`select::Hidden`] travels any further. The other half — how many
/// records were hidden in total — is `window.before_available` minus `shown`, which the
/// document already carries, and a second copy of a number is a number that can
/// contradict the first.
fn unstated_hidden(outcomes: &[EngineOutcome]) -> usize {
    outcomes.iter().map(|outcome| outcome.hidden.unstated).sum()
}

/// The note that keeps "nothing was said" from reading as "it is out".
///
/// Only when there were such records: a note that always fires is a note nobody reads.
/// The count is in the message for the human; an agent branches on
/// [`note_kinds::AVAILABILITY_FILTER_UNSTATED`] and never on the wording.
fn unstated_note(unstated: usize) -> Option<Note> {
    (unstated > 0).then(|| {
        Note::new(
            note_kinds::AVAILABILITY_FILTER_UNSTATED,
            format!(
                "no status was stated for {unstated} of the records --available hid; \
                 nothing was said about their copies, so they are not known to be on loan"
            ),
        )
    })
}

/// The `at[]` entries, **in the order the user wrote `--at`** — not in engine order.
///
/// A location whose engine did not report a block still gets one, with `total: null`: a
/// missing entry cannot be told apart from a location that was never asked for.
fn at_blocks(plan: &Plan, outcomes: &[EngineOutcome]) -> Vec<AtBlock> {
    plan.locations
        .iter()
        .map(|location| {
            outcomes
                .iter()
                .flat_map(|outcome| &outcome.search.at)
                .find(|block| block.key == location.key && block.isil == location.isil)
                .cloned()
                .unwrap_or_else(|| AtBlock {
                    key: location.key.clone(),
                    isil: location.isil.clone(),
                    branch: location.branch.as_ref().map(|branch| branch.kobvid.clone()),
                    engine: location.engine,
                    total: None,
                    // No engine reported for this location, so nothing is known about its
                    // membership either. `select` falls back to the holdings for a block
                    // whose entry states none, which is why this is not a claim of "no
                    // records here".
                    records: Vec::new(),
                })
        })
        .collect()
}

/// Why this search is empty — or that it is not.
///
/// The four empty reasons are four different next steps, which is why they are not one:
/// records that are all out want a larger page or another day, a filter that emptied a
/// full window is a window problem, a location that holds nothing is a "look elsewhere",
/// and only the last case means the catalogue really has nothing.
fn outcome_of(plan: &Plan, result: &SearchResult, unstated: usize) -> Outcome {
    if result.shown > 0 {
        return Outcome::Found;
    }
    // First, and only when the filter had something to judge: these records *are* there
    // and they *are* held here, they are merely not in. Falling through would offer
    // "try fewer or more general words" or "none of them is held at HU", and both would
    // send the user after a problem they do not have.
    if let Some(judged) = result.window.before_available
        && judged > 0
    {
        return Outcome::Empty(EmptyReason::NothingAvailable {
            total: result.total,
            judged,
            unstated,
        });
    }
    // Before `FilteredOut`, because the two are told apart by whether anything matched:
    // records that matched but sit on an earlier page are not records that failed a
    // filter, and the advice differs — narrowing the search would only make it worse.
    if let Some((filter, value)) = active_filter(plan)
        && result.window.after_filter > 0
        && plan.cut().offset >= result.window.after_filter
    {
        let limit = u32::from(plan.limit.get());
        return Outcome::Empty(EmptyReason::PastTheLastMatch {
            matched: result.window.after_filter,
            filter,
            value,
            page: plan.page.get(),
            // The window is at most 50 records, so this always fits; saturating rather
            // than casting keeps that a fact of the code and not of the caller.
            last: u32::try_from(result.window.after_filter)
                .unwrap_or(u32::MAX)
                .div_ceil(limit)
                .max(1),
        });
    }
    if let Some((filter, value)) = active_filter(plan)
        && result.window.fetched > 0
    {
        return Outcome::Empty(EmptyReason::FilteredOut {
            total: result.total,
            fetched: result.window.fetched,
            filter,
            value,
        });
    }
    // "Held nowhere in your libraries" needs hits to be held: `--at` is an *upstream*
    // filter, so a location that counts zero has nothing for this query at all, and
    // saying "hits exist" would be a statement about records that do not exist. The
    // per-location counts are the ones to read — with several locations there is no joint
    // total, and with only `voebb` there never was one.
    //
    // A location that counts zero is nonetheless not the same as an empty catalogue: the
    // words were never tried anywhere else, so "try fewer or more general words" would
    // send the user to weaken a search that may be exactly right. What that case gets is
    // its own reason, which names the restriction and stops there — there is no
    // unfiltered count to compare against, and asking for one would be a second request
    // made purely to word a message.
    if !plan.locations.is_empty() {
        let locations = plan
            .locations
            .iter()
            .map(|location| location.key.clone())
            .collect();
        return Outcome::Empty(if counts_hits(result) {
            EmptyReason::NoHoldings { locations }
        } else {
            EmptyReason::NoHitsAtLocations {
                terms: plan.terms_echo.clone(),
                locations,
            }
        });
    }
    Outcome::Empty(EmptyReason::NoHits {
        terms: plan.terms_echo.clone(),
    })
}

/// Whether any of the named locations reports hits at all.
///
/// A location whose engine could not state a count (`total: null`) is read as "might
/// have": claiming the catalogue holds nothing on the strength of a number nobody gave
/// is the one answer that would be worse than vague.
fn counts_hits(result: &SearchResult) -> bool {
    result
        .at
        .iter()
        .any(|block| block.total.is_none_or(|total| total > 0))
}

/// The client-side filter that thinned the window, as flag and value.
///
/// `--format` first: when both are set it is the one that removes most, and naming two
/// flags in one message helps nobody decide what to change.
fn active_filter(plan: &Plan) -> Option<(String, String)> {
    if let Some(format) = plan.filters.format {
        return Some(("--format".to_owned(), format.as_str().to_owned()));
    }
    plan.filters
        .language
        .as_ref()
        .map(|code| ("--language".to_owned(), code.clone()))
}

/// `show`: one record, chosen by the id's prefix and by nothing else.
fn run_show(
    plan: &ShowPlan,
    fetch: &dyn Fetch,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let catalog = catalog_for(plan.engine(), fetch);
    let Some(mut record) = catalog.show(&plan.id, plan.availability)? else {
        let reason = EmptyReason::NoSuchRecord {
            id: plan.id.clone(),
        };
        if plan.json {
            // `null` rather than an error object: the lookup succeeded and the catalogue
            // has no such record. Exit 1 says the same thing without parsing.
            render::json::write(&Option::<Record>::None, out)?;
        } else {
            render::human::empty(&reason, err).map_err(output)?;
        }
        return Ok(Outcome::Empty(reason));
    };

    select::mark_mine(std::slice::from_mut(&mut record), &plan.locations);
    if plan.json {
        render::json::write(&record, out)?;
    } else {
        render::human::show(&record, &plan.locations, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// `libraries`: the compiled-in list, offline in every form.
///
/// `--find` and `--near` combine — "the closest of these" is a sensible question — and a
/// detail key answers on its own.
fn run_libraries(
    plan: &LibrariesPlan,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    if let Some(key) = &plan.detail {
        return show_library(plan, key, out, err, style);
    }
    match plan.near {
        Some(point) => list_near(plan, point, out, err, style),
        None => list_libraries(plan, out, err, style),
    }
}

/// One library in detail, or exit 1 with the reason.
///
/// An unknown key here is a *result*, not a usage error: `--at NOPE` names a library the
/// search must have, but `libraries NOPE` is a lookup, and a lookup that finds nothing
/// found nothing.
fn show_library(
    plan: &LibrariesPlan,
    key: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let Some(library) = library_by_key(key) else {
        return empty_libraries(plan, key.to_owned(), out, err);
    };
    if plan.json {
        render::json::write(&render::json::LibraryView::from(library), out)?;
    } else {
        render::human::library_detail(library, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// The institution a detail key names: an alias, an ISIL, or the branch alias of one.
///
/// A branch alias answers with its parent house — `libraries AGB` is a question about the
/// Amerika-Gedenkbibliothek, and the list keeps its address under the ZLB entry. Nothing
/// here branches on a specific ISIL; the three lookups are the same three the resolver
/// uses for `--at`.
fn library_by_key(key: &str) -> Option<&'static Library> {
    let typed = key.trim();
    let folded = libraries::text::fold(typed);
    if folded.is_empty() {
        return None;
    }
    let by_alias = libraries::all()
        .iter()
        .find(|library| has_alias(&library.aliases, &folded));
    by_alias
        .or_else(|| {
            libraries::all()
                .iter()
                .find(|library| library.isil.eq_ignore_ascii_case(typed))
        })
        .or_else(|| {
            libraries::all().iter().find(|library| {
                library
                    .branches
                    .iter()
                    .any(|branch| has_alias(&branch.aliases, &folded))
            })
        })
}

/// Whether a folded string is one of these aliases.
fn has_alias(aliases: &[String], folded: &str) -> bool {
    aliases
        .iter()
        .any(|alias| libraries::text::fold(alias) == folded)
}

/// The list ordered by distance, optionally narrowed by `--find`.
fn list_near(
    plan: &LibrariesPlan,
    point: crate::libraries::LatLon,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let mut ranked = libraries::near(point);
    if let Some(query) = &plan.find {
        let matching = libraries::find(query);
        ranked.retain(|(library, _)| matching.iter().any(|found| std::ptr::eq(*found, *library)));
    }
    if ranked.is_empty() {
        let query = plan
            .find
            .clone()
            .or_else(|| plan.near_input.clone())
            .unwrap_or_default();
        return empty_libraries(plan, query, out, err);
    }
    if plan.json {
        render::json::write(&render::json::library_views_near(&ranked), out)?;
    } else {
        render::human::libraries_near(&ranked, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// The whole list, or what `--find` matched of it.
fn list_libraries(
    plan: &LibrariesPlan,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let rows: Vec<&'static Library> = match &plan.find {
        Some(query) => libraries::find(query),
        None => libraries::all().iter().collect(),
    };
    if rows.is_empty() {
        let query = plan.find.clone().unwrap_or_default();
        return empty_libraries(plan, query, out, err);
    }
    if plan.json {
        render::json::write(&render::json::library_views(&rows), out)?;
    } else {
        render::human::libraries_table(&rows, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// Nothing in the list matched. In JSON that is an empty array — a document, not a
/// failure — and the reason goes to stderr for the human.
fn empty_libraries(
    plan: &LibrariesPlan,
    query: String,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Outcome, Error> {
    let reason = EmptyReason::NoLibraryMatched { query };
    if plan.json {
        render::json::write(&Vec::<render::json::LibraryView<'_>>::new(), out)?;
    } else {
        render::human::empty(&reason, err).map_err(output)?;
    }
    Ok(Outcome::Empty(reason))
}

/// A failed write is neither the user's mistake nor the service's, so it lands in the
/// same bucket as a broken response — with the io error kept, because `main` treats a
/// closed pipe as a normal end.
fn output(source: io::Error) -> Error {
    UnexpectedError::Output { source }.into()
}
