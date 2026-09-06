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
    AtBlock, AvailabilityMode, Catalog, Engine, EngineSearch, Location, QueryEcho, Record,
    SearchRequest, SearchResult, SortKey, SortScope, SortSpec, WindowInfo,
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
    let result = assemble_result(plan, outcomes);
    let outcome = outcome_of(plan, &result);

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

/// One engine: search, filter, sort, page, availability — in that order and no other.
///
/// Availability is fetched for the records that survived paging, because it costs one
/// request each. That is also why [`SortKey::Availability`] sorts twice: the first pass
/// has no statuses to work with, and only the second one — over the page — can order by
/// something that exists.
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
        // A per-location total costs one counting request; without `--at` there is no
        // location to count for.
        want_totals: !locations.is_empty(),
    };

    let mut search = catalog.search(&request)?;
    let filtered = select::filter(std::mem::take(&mut search.records), &plan.filters);
    let after_filter = filtered.len();

    let sort_at = sort_location(locations);
    let mut records = filtered;
    select::sort(&mut records, plan.sort, sort_at);
    let mut records = select::take_page(records, plan.limit);

    if plan.availability == AvailabilityMode::Fetched {
        search
            .notes
            .extend(catalog.fill_availability(&mut records)?);
        if plan.sort == SortKey::Availability {
            select::sort(&mut records, plan.sort, sort_at);
        }
    }
    select::mark_mine(&mut records, &plan.locations);

    search.records = records;
    // Last, and after availability: `at[].records` is the block, and a record can still
    // gain a holding here — the availability service names libraries the record's own
    // `924` fields do not.
    select::assign_blocks(&mut search.at, &search.records, &plan.locations);
    Ok(EngineOutcome {
        search,
        after_filter,
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
fn assemble_result(plan: &Plan, outcomes: Vec<EngineOutcome>) -> SearchResult {
    let engines: Vec<Engine> = outcomes
        .iter()
        .map(|outcome| outcome.search.engine)
        .collect();
    let at = at_blocks(plan, &outcomes);
    let window = WindowInfo {
        fetched: outcomes.iter().map(|o| o.search.fetched).sum(),
        after_filter: outcomes.iter().map(|o| o.after_filter).sum(),
        undelivered: outcomes.iter().map(|o| o.search.undelivered).sum(),
    };
    // `total` and `pqf` are the KOBV search's, and `null` when only voebb ran: two
    // catalogues have two totals, and adding them up would invent a number that is true
    // of neither.
    let kobv = outcomes
        .iter()
        .find(|outcome| outcome.search.engine == Engine::Kobv);
    let total = kobv.and_then(|outcome| outcome.search.total);
    let pqf = kobv.and_then(|outcome| outcome.search.query_echo.clone());

    let mut notes = Vec::new();
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
/// The three empty reasons are three different next steps, which is why they are not one:
/// a filter that emptied a full window is a window problem, a location that holds nothing
/// is a "look elsewhere", and only the last case means the catalogue really has nothing.
fn outcome_of(plan: &Plan, result: &SearchResult) -> Outcome {
    if result.shown > 0 {
        return Outcome::Found;
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
    // filter, so a total of zero means the catalogue has nothing for this query at these
    // locations at all, and saying "hits exist" would be a statement about records that
    // do not exist.
    if !plan.locations.is_empty() && result.total.is_none_or(|total| total > 0) {
        return Outcome::Empty(EmptyReason::NoHoldings {
            locations: plan
                .locations
                .iter()
                .map(|location| location.key.clone())
                .collect(),
        });
    }
    Outcome::Empty(EmptyReason::NoHits {
        terms: plan.terms_echo.clone(),
    })
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
