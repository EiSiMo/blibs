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

use crate::cli::{Cli, Command, LibrariesPlan, Plan, ShowPlan, TooDeep, long_help, validate};
use crate::counts::records;
use crate::engine::kobv::Kobv;
use crate::engine::voebb::Voebb;
use crate::error::{Blast, EmptyReason, Error, Outcome, UnexpectedError};
use crate::http::{Fetch, scope_map};
use crate::libraries::{self, Branch, Library};
use crate::model::{
    AtBlock, AvailabilityMode, Catalog, Engine, EngineSearch, Location, LocationRefusal, Note,
    QueryEcho, SearchRequest, SearchResult, ShowResult, SortKey, SortScope, SortSpec, WindowInfo,
    note_kinds, past_the_last_result_note, window_too_deep_note,
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
    let engines = search_engines(plan, fetch)?;
    let unstated = unstated_hidden(&engines.outcomes);
    let result = assemble_result(plan, engines, unstated);
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
        render::human::empty(reason, err, style).map_err(output)?;
    }
    Ok(outcome)
}

/// What the engines of one invocation produced.
struct Engines {
    /// One per engine that answered.
    outcomes: Vec<EngineOutcome>,
    /// The locations of an engine whose every location's window began past its own last
    /// result — see [`search_engines`]. Their blocks are built by [`at_blocks`] like any
    /// other location the engine did not report on; what they need beyond that is the note.
    refused: Vec<String>,
}

/// Run every engine this invocation has locations for, at the same time.
///
/// The first error by engine order wins and the whole invocation fails with it — two
/// catalogues answering half a question is worse than one catalogue saying it could not.
///
/// The one exception is the failure that belongs to a location rather than to the run
/// ([`Blast::Location`]), and it is the **same rule the KOBV engine applies to its own
/// locations**, one level up: an engine every one of whose locations ran out of results
/// raises the refusal, and here it is caught again as long as another engine still has an
/// answer. `--at TU,AGB --page 3 --limit 20` is where that matters — position 41 is inside
/// what voebb.de serves and past a TU result set of 40 — and without this the AGB block
/// would be thrown away for TU's.
///
/// When **no** engine answered, there is nothing left for the refusal to be smaller than
/// and it is raised: the user who simply paged too far still gets exit 5 and the
/// catalogue's own sentence.
fn search_engines(plan: &Plan, fetch: &dyn Fetch) -> Result<Engines, Error> {
    let mut answers = scope_map(
        plan.by_engine(),
        |(engine, locations)| match search_one_engine(plan, engine, &locations, fetch) {
            Ok(outcome) => Ok(EngineAnswer::Ran(Box::new(outcome))),
            Err(error) if error.blast_radius() == Blast::Location => {
                Ok(EngineAnswer::PastTheEnd { locations, error })
            }
            Err(error) => Err(error),
        },
    )?;
    if !answers.is_empty()
        && answers
            .iter()
            .all(|answer| matches!(answer, EngineAnswer::PastTheEnd { .. }))
        && let Some(error) = answers.drain(..).find_map(EngineAnswer::into_error)
    {
        return Err(error);
    }

    let mut engines = Engines {
        outcomes: Vec::with_capacity(answers.len()),
        refused: Vec::new(),
    };
    for answer in answers {
        match answer {
            EngineAnswer::Ran(outcome) => engines.outcomes.push(*outcome),
            EngineAnswer::PastTheEnd { locations, .. } => engines
                .refused
                .extend(locations.into_iter().map(|location| location.key)),
        }
    }
    Ok(engines)
}

/// What one engine answered: a result, or the refusal that its locations own.
///
/// The sibling of `engine::kobv::Located` at the level above it, and the boxed variant is
/// clippy's doing: an [`EngineOutcome`] is several vectors wide and an [`Error`] is not.
enum EngineAnswer {
    /// The engine searched.
    Ran(Box<EngineOutcome>),
    /// Every location of this engine asked for a window past its own last result. The
    /// locations are kept so their note can name them; the error, so it can be raised
    /// again when no other engine answered either.
    PastTheEnd {
        /// The locations this engine was given.
        locations: Vec<Location>,
        /// The catalogue's refusal, whole.
        error: Error,
    },
}

impl EngineAnswer {
    /// The refusal, when this is one.
    fn into_error(self) -> Option<Error> {
        match self {
            EngineAnswer::PastTheEnd { error, .. } => Some(error),
            EngineAnswer::Ran(_) => None,
        }
    }
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
    let records = if locations.is_empty() {
        select::take_page(records, plan.cut())
    } else {
        select::take_page_per_location(records, &mut search.at, plan.cut())
    };

    let mut records = records;
    if plan.availability == AvailabilityMode::Fetched {
        search
            .notes
            .extend(catalog.fill_availability(&mut records)?);
        if plan.sort == SortKey::Availability {
            select::sort(&mut records, plan.sort, sort_at);
        }
        // Only now can a KOBV branch be answered for at all: the search restricted the
        // *house*, and which branch holds a copy is said by the copies alone. It runs
        // before `--available` so that filter judges the copies of this branch rather
        // than the house's.
        let (kept, elsewhere) =
            select::keep_branch_per_location(records, &mut search.at, locations);
        records = kept;
        if let Some(note) = branch_note(locations, elsewhere) {
            search.notes.push(note);
        }
    } else if let Some(note) = unfiltered_branch_note(locations) {
        search.notes.push(note);
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

/// What a KOBV branch location has to say about its own block.
///
/// It is never nothing. The heading has no total, the page can be shorter than `--limit`,
/// and neither is visible as anything but a small result — so the limitation is stated
/// whether or not this particular page lost a record to it.
fn branch_note(locations: &[Location], elsewhere: usize) -> Option<Note> {
    let branches = branch_names(locations)?;
    let dropped = match elsewhere {
        0 => String::new(),
        1 => " 1 record on this page is held by the house but not there".to_string(),
        many => format!(" {many} records on this page are held by the house but not there"),
    };
    Some(Note::new(
        note_kinds::BRANCH_FROM_COPIES,
        format!(
            "{branches} could only be narrowed from the copies of the records on this page — \
             the catalogue filters by institution, not by branch. The block states no total, \
             the page can be shorter than --limit, and nothing here shows that the branch \
             holds no other edition.{dropped}"
        ),
    ))
}

/// The same location with `--no-availability`: no copies, so nothing to narrow by.
fn unfiltered_branch_note(locations: &[Location]) -> Option<Note> {
    let branches = branch_names(locations)?;
    Some(Note::new(
        note_kinds::BRANCH_NEEDS_COPIES,
        format!(
            "{branches} could not be applied: the branch of a copy is only named in the \
             availability answer, and --no-availability did not ask for one. The block is \
             the whole institution's."
        ),
    ))
}

/// The KOBV branch locations of this run, named for a note, or `None` when there are none.
fn branch_names(locations: &[Location]) -> Option<String> {
    let named: Vec<&str> = locations
        .iter()
        .filter(|location| location.branch.is_some() && location.engine == Engine::Kobv)
        .map(|location| location.key.as_str())
        .collect();
    match named.as_slice() {
        [] => None,
        [only] => Some(format!("--at {only}")),
        several => Some(format!("--at {}", several.join(", --at "))),
    }
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
fn assemble_result(plan: &Plan, engines: Engines, unstated: usize) -> SearchResult {
    let Engines { outcomes, refused } = engines;
    // Which engines this invocation is *about*, taken from the plan rather than from the
    // outcomes. An engine can be missing from the outcomes and still have blocks in
    // `at[]` — every one of its locations paged past its last result, or every one of
    // them lies deeper than it can be paged and no search was sent at all — and listing
    // only the engines that came back would leave those blocks under a catalogue the
    // document says was never asked. `Plan::engines` reads all of `--at` for exactly that
    // reason, while `Plan::by_engine` is the narrower list of searches that ran.
    let engines: Vec<Engine> = plan.engines();
    let at = at_blocks(plan, &outcomes, &refused);
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

    let mut engine_notes = Vec::new();
    let mut records = Vec::new();
    for outcome in outcomes {
        engine_notes.extend(outcome.search.notes);
        records.extend(outcome.search.records);
    }
    // The engines' notes are collected first because the `--available` note reads them:
    // what voebb.de said about an electronic title decides how the footnote is worded.
    let mut notes = Vec::new();
    // First of all: it is the only note about the *question* rather than about the
    // answer, and a reader who was told about somewhere else entirely has to learn that
    // before anything the answer says about a window or a filter.
    notes.extend(crate::model::ambiguous_key_notes(&plan.locations));
    // The note for a whole engine that was refused. The engine writes this one itself when
    // only some of its locations ran out; here it is the same sentence from the same
    // function, for the case where the engine had nothing left to attach it to.
    let refused: Vec<&str> = refused.iter().map(String::as_str).collect();
    notes.extend(past_the_last_result_note(&refused));
    // And the same limitation one step earlier, for the locations `cli::validate` refused
    // before a request went out: a window deeper than voebb.de can be paged. A separate
    // sentence rather than the one above, because that one reports a result set that ran
    // out and this one reports a question nobody asked.
    if let Some(too_deep) = &plan.too_deep {
        notes.extend(window_too_deep_note(
            &too_deep_keys(plan, too_deep),
            too_deep.position,
            too_deep.max,
        ));
    }
    notes.extend(unstated_note(unstated, unstated_online(&engine_notes)));
    notes.extend(window_filter_note(plan, &window));
    notes.extend(engine_notes);

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
        // Folded here, where the two engines' notes have just met and nothing else will
        // be added: four records that hit the same limitation are one note naming four
        // records, not four paragraphs a reader cannot tell apart. The rule and the
        // reason are [`Note::merged`]'s; `ShowResult` folds through the same function.
        notes: Note::merged(notes),
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

/// The note for a window filter that matched **nothing**.
///
/// The one limitation the document could state only in prose. An agent had to derive it
/// from `filtered && fetched > 0 && after_filter == 0` — three fields and a rule nobody
/// wrote down — while the human output has been saying it in words since before there was
/// a tag (round 2, §2.7). The three fields still say it; this says it once, in the
/// vocabulary agents are told to switch on.
///
/// About the answer as a whole, so it names no records: the records it is about are
/// exactly the ones that are *not* in the document.
fn window_filter_note(plan: &Plan, window: &WindowInfo) -> Option<Note> {
    if !window.filtered || window.fetched == 0 || window.after_filter > 0 {
        return None;
    }
    let (filter, value) = active_filter(plan)?;
    Some(Note::new(
        note_kinds::WINDOW_FILTER_EMPTY,
        format!(
            "none of the {} fetched records matched {filter} {value} — the filter runs \
             over the fetched window, so this is not a statement about the whole result",
            window.fetched
        ),
    ))
}

/// How many of the displayed records were electronic titles that state **no** loan status
/// at all.
///
/// Counted from the notes rather than from the records, because the note is the only
/// place that knowledge exists: `items[]` is empty for such a record and nothing in it
/// distinguishes "no copies on a shelf" from "no copies stated".
///
/// Only [`note_kinds::VOEBB_ONLINE_STATE_UNSTATED`] counts, never its sibling: an Onleihe
/// title whose `(Das Medium ist ausgeliehen / …)` was read *has* a status, is hidden by
/// `--available` as a judged record, and saying of it that it is "not known to be on
/// loan" would contradict the note printed two lines below. Overdrive states nothing, and
/// that is the whole set this count may speak for.
///
/// Counted over the notes' **records**, not over the notes: since [`Note::merged`] folds
/// four identical notes into one naming four records, counting notes would have said
/// "1 of them" about four titles the moment the fold was introduced. A note that names no
/// record counts as one, which is what a note about the answer as a whole is worth here.
fn unstated_online(notes: &[Note]) -> usize {
    notes
        .iter()
        .filter(|note| note.kind == note_kinds::VOEBB_ONLINE_STATE_UNSTATED)
        .map(|note| note.records.len().max(1))
        .sum()
}

/// The note that keeps "nothing was said" from reading as "it is out".
///
/// Only when there were such records: a note that always fires is a note nobody reads.
/// The counts are in the message for the human; an agent branches on
/// [`note_kinds::AVAILABILITY_FILTER_UNSTATED`] and never on the wording.
///
/// `online` is why the wording is not one sentence: an electronic title has no copies to
/// say anything about, and a reader who is told "no status was stated" would look for a
/// shelf that does not exist. It counts **only** the titles whose lending platform states
/// nothing — Overdrive prints no parenthesis behind its link, while the Onleihe prints
/// `(Das Medium ist ausgeliehen / …)` and that sentence is read into the status
/// ([`unstated_online`]). Until round 2 this sentence claimed the tool did not read any of
/// them and that none was known to be on loan, and then printed the Onleihe's own
/// "ausgeliehen" two lines below (§3.6).
fn unstated_note(unstated: usize, online: usize) -> Option<Note> {
    if unstated == 0 {
        return None;
    }
    // The two counts are collected independently — one per hidden record, one per note.
    // A record whose lending link says nothing has no status and is therefore among the
    // hidden ones, so this clamp should never bind; it is here so that a future engine
    // that reports differently cannot make the sentence claim more records than exist.
    let online = online.min(unstated);
    let message = match online {
        0 => format!(
            "no status was stated for {unstated} of the records --available hid; \
             nothing was said about their copies, so they are not known to be on loan"
        ),
        _ => format!(
            "--available hid {} whose status was not read; {online} of them {} electronic \
             titles whose lending platform states no loan status at all — none of those is \
             known to be on loan",
            records(unstated),
            if online == 1 { "is an" } else { "are" }
        ),
    };
    Some(Note::new(note_kinds::AVAILABILITY_FILTER_UNSTATED, message))
}

/// The `at[]` entries, **in the order the user wrote `--at`** — not in engine order.
///
/// A location whose engine did not report a block still gets one, with `total: null`: a
/// missing entry cannot be told apart from a location that was never asked for.
///
/// `refused` are the locations of an engine that was refused whole — see [`refusal_of`],
/// which turns that and `cli::validate`'s verdict into the entry's own `refused` member.
/// Without it those entries would be indistinguishable from the ones nobody reported on,
/// in the document and in the heading above the block alike.
fn at_blocks(plan: &Plan, outcomes: &[EngineOutcome], refused: &[String]) -> Vec<AtBlock> {
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
                    given: location.given.clone(),
                    isil: location.isil.clone(),
                    branch: location.branch.as_ref().map(|branch| branch.kobvid.clone()),
                    engine: location.engine,
                    total: None,
                    // No engine reported for this location, so nothing is known about its
                    // membership either. `select` falls back to the holdings for a block
                    // whose entry states none, which is why this is not a claim of "no
                    // records here".
                    records: Vec::new(),
                    refused: refusal_of(plan, location, refused),
                })
        })
        .collect()
}

/// Why a location has no entry of its own from any engine — when that is because it was
/// refused rather than because nothing was reported for it.
///
/// The two refusals reach this function from the two places that can decide them, and
/// neither is re-derived here: `refused` are the locations of an engine whose every search
/// ran out ([`search_engines`]), and [`crate::cli::TooDeep`] is `cli::validate`'s verdict
/// from before the first request. A location refused by the KOBV engine alone never gets
/// here at all — that engine writes its own `at[]` entry and marks it there.
fn refusal_of(plan: &Plan, location: &Location, refused: &[String]) -> Option<LocationRefusal> {
    if refused.contains(&location.key) {
        return Some(LocationRefusal::PastTheLastResult);
    }
    plan.too_deep
        .as_ref()
        .filter(|too_deep| too_deep.holds(&location.key))
        .map(|_| LocationRefusal::WindowTooDeep)
}

/// The refused locations named for the note, in the order the user wrote `--at`.
///
/// Read back off the locations rather than printed from [`crate::cli::TooDeep::keys`]
/// directly, so that the note lists them the way the command line did — the same rule
/// `engine::kobv` follows for its own.
fn too_deep_keys<'a>(plan: &'a Plan, too_deep: &TooDeep) -> Vec<&'a str> {
    plan.locations
        .iter()
        .filter(|location| too_deep.holds(&location.key))
        .map(|location| location.key.as_str())
        .collect()
}

/// Why this search is empty — or that it is not.
///
/// Every empty reason is a different next step, which is why they are not one: records
/// that are all out want a larger page or another day, a filter that emptied a full
/// window is a window problem, an anchored window whose pages ran out wants an earlier
/// page, a location that holds nothing is a "look elsewhere" — and only the last two mean
/// the catalogue really has nothing, one of them with the region still to try and one
/// without.
///
/// The order is the order of the questions, and it is load-bearing twice: `--available`
/// comes first because those records exist and are merely out, and the *filtered* window
/// comes before the *sorted* one because when both anchored the window it was the filter
/// that removed records.
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
    // The same sentence for the other thing that anchors a window. `--sort` pins the
    // window to one block just as a filter does, so its pages run out the same way — and
    // until this ran, `--sort year --limit 10 --page 6` fell through to a generic "no
    // results" that explained none of it (round 2, the Phase-1 aftermath). It sits
    // *after* the filtered case on purpose: with both set, the filter is what removed
    // records and its wording is the more useful of the two.
    if plan.sort != SortKey::Relevance
        && result.window.after_filter > 0
        && plan.cut().offset >= result.window.after_filter
    {
        let limit = u32::from(plan.limit.get());
        return Outcome::Empty(EmptyReason::PastTheLastSorted {
            sorted: result.window.after_filter,
            sort: plan.sort.as_str().to_owned(),
            page: plan.page.get(),
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
        let locations: Vec<String> = plan
            .locations
            .iter()
            .map(|location| location.key.clone())
            .collect();
        if counts_hits(result) {
            return Outcome::Empty(EmptyReason::NoHoldings { locations });
        }
        // The one case where the restriction is provably *not* the cause: voebb.de states
        // the network-wide count beside its facet, and it was zero. "Name more libraries"
        // would then be advice that is guaranteed to fail again, which is the whole
        // reason this reason exists (round 2, §1.2).
        return Outcome::Empty(if nothing_in_the_network(plan, result) {
            EmptyReason::NoHitsAnywhere {
                terms: plan.terms_echo.clone(),
                locations,
            }
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

/// Whether the words found nothing **anywhere**, rather than nothing at these locations.
///
/// Two conditions, and both are needed. voebb.de has to have said so — it is the only
/// engine that sees an unrestricted count, and it reports that count as
/// [`note_kinds::VOEBB_NO_HITS_IN_NETWORK`] — and every location in `--at` has to be one
/// voebb answered for. A KOBV house beside it filters upstream and has no unrestricted
/// total of its own, so its empty block leaves open exactly what the message would deny:
/// that naming another library could help.
fn nothing_in_the_network(plan: &Plan, result: &SearchResult) -> bool {
    let stated = result
        .notes
        .iter()
        .any(|note| note.kind == note_kinds::VOEBB_NO_HITS_IN_NETWORK);
    stated
        && plan
            .locations
            .iter()
            .all(|location| location.engine == Engine::Voebb)
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
    let found = catalog.show(&plan.id, plan.availability)?;
    let empty = found.record.is_none();
    let mut result = ShowResult::new(
        found.record,
        plan.engine(),
        &plan.locations,
        plan.availability,
    );
    // The engine's notes go in front of the derived ones, and the document folds
    // duplicates as it takes them — the ordering reason and the folding reason both live
    // on `ShowResult::prepend_notes`, so that a splice here could not grow a second idea
    // of either. Dropping them left `show` with a silently empty copy list and no reason
    // for it.
    result.prepend_notes(found.notes);
    if let Some(record) = &mut result.record {
        select::mark_mine(std::slice::from_mut(record), &plan.locations);
    }
    // After `mark_mine`, and from `select` exactly as the search document's `at[]` is:
    // `holdings[].summary` is what the catalogue said about the whole house, so a record
    // on loan at one branch and in at three others needs a per-location answer of its own
    // for `--at` to have meant anything.
    result.at = select::show_at(result.record.as_ref(), &plan.locations);

    if empty {
        let reason = EmptyReason::NoSuchRecord {
            id: plan.id.clone(),
        };
        if plan.json {
            // The hull with `record: null` rather than a bare `null` or an error object:
            // the lookup succeeded and the catalogue has no such record, and an agent can
            // still read from the document whether copies were even asked for. Exit 1
            // says the same thing without parsing.
            render::json::write(&result, out)?;
        } else {
            render::human::empty(&reason, err, style).map_err(output)?;
        }
        return Ok(Outcome::Empty(reason));
    }

    if plan.json {
        render::json::write(&result, out)?;
    } else if let Some(record) = &result.record {
        render::human::show(record, &plan.locations, &result.notes, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// `libraries`: the compiled-in list, offline in every form. [`list`] decides the shape,
/// [`write_listing`] writes it.
fn run_libraries(
    plan: &LibrariesPlan,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    if let Some(entry) = plan.detail {
        return show_entry(plan, entry, out, style);
    }
    write_listing(plan, list(plan), out, err, style)
}

/// One entry in detail. Never empty and never a failure: the key was resolved in
/// `validate`, so by the time we are here it names something.
///
/// A branch is answered **as a branch** — its own address, its parent house and how it
/// can be searched — and not by silently substituting the house it belongs to, which
/// would answer a question about the Amerika-Gedenkbibliothek with a 98-branch listing of
/// the whole VÖBB.
fn show_entry(
    plan: &LibrariesPlan,
    entry: libraries::Entry,
    out: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    match entry {
        libraries::Entry::Institution(library) => show_library(plan, library, out, style),
        libraries::Entry::Branch { parent, branch } => {
            show_branch(plan, parent, branch, out, style)
        }
    }
}

/// One institution in detail, with its branches listed underneath.
fn show_library(
    plan: &LibrariesPlan,
    library: &'static Library,
    out: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    if plan.json {
        render::json::write(&render::json::LibraryView::from(library), out)?;
    } else {
        render::libraries::library_detail(library, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// One branch in detail, including how `--at` searches it.
///
/// How is not decided here: [`libraries::branch_location`] is asked, and it is the same
/// call `--at` goes through, so the two can never disagree about which engine answers for
/// a branch or how complete that answer is. Nothing on this path names an ISIL.
fn show_branch(
    plan: &LibrariesPlan,
    parent: &'static Library,
    branch: &'static Branch,
    out: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let at = libraries::branch_location(parent, branch);
    if plan.json {
        render::json::write(
            &render::json::BranchDetailView::new(parent, branch, &at),
            out,
        )?;
    } else {
        render::libraries::branch_detail(parent, branch, &at, out, style).map_err(output)?;
    }
    Ok(Outcome::Found)
}

/// What a listing invocation asks for, and therefore which shape the answer has.
///
/// Three shapes rather than one, because they answer three different questions: the plain
/// listing is the 123 houses whose columns `plan/cli.md` pins; `--find` is grouped, so that
/// a house can never be pushed off the answer by its own branches; and `--branches` and
/// `--near` are flat lists of entries in which a house and a branch stand side by side.
enum Listing {
    /// Every house, in list order — and the only shape that carries the branch footer,
    /// because it is the only one whose omissions the reader cannot see.
    Houses(Vec<&'static Library>),
    /// What `--find` matched, one group per house.
    Groups(Vec<libraries::Found>),
    /// A flat list of entries, with the distance where `--near` measured one.
    Entries(Vec<(libraries::Entry, Option<f64>)>),
}

/// `libraries`: the compiled-in list, offline in every form.
///
/// The selectors combine — "the branches closest to me, of the ones called X" is a
/// sensible question and needs no new syntax. A detail key does **not** combine with any of
/// them: it answers a different question, and `validate` refuses the pair rather than
/// letting the key swallow a flag the user typed.
fn list(plan: &LibrariesPlan) -> Listing {
    if let Some(point) = plan.near {
        let mut ranked: Vec<(libraries::Entry, Option<f64>)> = libraries::near_entries(point)
            .into_iter()
            .map(|(entry, distance)| (entry, Some(distance)))
            .collect();
        narrow(&mut ranked, plan);
        return Listing::Entries(ranked);
    }
    if plan.branches {
        let mut rows: Vec<(libraries::Entry, Option<f64>)> = libraries::branches()
            .into_iter()
            .map(|(parent, branch)| (libraries::Entry::Branch { parent, branch }, None))
            .collect();
        narrow(&mut rows, plan);
        return Listing::Entries(rows);
    }
    match &plan.find {
        Some(query) => Listing::Groups(libraries::find_entries(query)),
        None => Listing::Houses(libraries::all().iter().collect()),
    }
}

/// Narrow a flat listing by the selectors that are not what built it.
///
/// `--find` reaches this path as a filter rather than as the search itself: the grouped
/// answer has no place in a list ordered by distance, and "which of these matched" is the
/// question that does.
fn narrow(rows: &mut Vec<(libraries::Entry, Option<f64>)>, plan: &LibrariesPlan) {
    if plan.branches {
        rows.retain(|(entry, _)| matches!(entry, libraries::Entry::Branch { .. }));
    }
    let Some(query) = &plan.find else {
        return;
    };
    let matched: Vec<libraries::Entry> = libraries::find_entries(query)
        .into_iter()
        .flat_map(|group| {
            let house = group
                .matched
                .then_some(libraries::Entry::Institution(group.library));
            house
                .into_iter()
                .chain(
                    group
                        .branches
                        .into_iter()
                        .map(move |branch| libraries::Entry::Branch {
                            parent: group.library,
                            branch,
                        }),
                )
        })
        .collect();
    rows.retain(|(entry, _)| matched.iter().any(|found| same_entry(*found, *entry)));
}

/// Whether two entries are the same entry of the list.
///
/// By address, not by value: both sides come out of the one compiled-in list, so the
/// pointer is exact — and comparing two houses field by field would compare their whole
/// branch lists with them.
fn same_entry(left: libraries::Entry, right: libraries::Entry) -> bool {
    match (left, right) {
        (libraries::Entry::Institution(a), libraries::Entry::Institution(b)) => std::ptr::eq(a, b),
        (
            libraries::Entry::Branch { branch: a, .. },
            libraries::Entry::Branch { branch: b, .. },
        ) => std::ptr::eq(a, b),
        _ => false,
    }
}

/// Write one listing, or say why it is empty.
fn write_listing(
    plan: &LibrariesPlan,
    listing: Listing,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    match listing {
        Listing::Houses(rows) if rows.is_empty() => empty_libraries(plan, out, err, style),
        Listing::Groups(groups) if groups.is_empty() => empty_libraries(plan, out, err, style),
        Listing::Entries(rows) if rows.is_empty() => empty_libraries(plan, out, err, style),
        Listing::Houses(rows) => {
            if plan.json {
                render::json::write(&render::json::library_views(&rows), out)?;
            } else {
                render::libraries::houses(&rows, out, style).map_err(output)?;
                // Only here: this is the listing that shows 123 of 334 addressable
                // libraries without anything on the screen saying so.
                render::libraries::footer(out, style).map_err(output)?;
            }
            Ok(Outcome::Found)
        }
        Listing::Groups(groups) => {
            if plan.json {
                render::json::write(&render::json::found_views(&groups), out)?;
            } else {
                render::libraries::found(&groups, out, style).map_err(output)?;
            }
            Ok(Outcome::Found)
        }
        Listing::Entries(rows) => {
            if plan.json {
                render::json::write(&render::json::entry_views(&rows), out)?;
            } else {
                render::libraries::entries(&rows, out, style).map_err(output)?;
            }
            Ok(Outcome::Found)
        }
    }
}

/// Nothing in the list matched. In JSON that is an empty array — a document, not a
/// failure — and the reason goes to stderr for the human.
fn empty_libraries(
    plan: &LibrariesPlan,
    out: &mut dyn Write,
    err: &mut dyn Write,
    style: Style,
) -> Result<Outcome, Error> {
    let query = plan
        .find
        .clone()
        .or_else(|| plan.near_input.clone())
        .unwrap_or_default();
    let reason = EmptyReason::NoLibraryMatched { query };
    if plan.json {
        render::json::write(&Vec::<render::json::LibraryView<'_>>::new(), out)?;
    } else {
        render::human::empty(&reason, err, style).map_err(output)?;
    }
    Ok(Outcome::Empty(reason))
}

/// A failed write is neither the user's mistake nor the service's, so it lands in the
/// same bucket as a broken response — with the io error kept, because `main` treats a
/// closed pipe as a normal end.
fn output(source: io::Error) -> Error {
    UnexpectedError::Output { source }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--available` hides what nobody judged, and the footnote has to say which kind of
    /// silence it was. Without a voebb electronic title among them, it is the
    /// catalogue's: nothing was stated at all.
    #[test]
    fn a_hidden_record_nobody_judged_says_nothing_was_stated() {
        let note = unstated_note(3, 0).expect("three hidden records are worth a note");
        assert_eq!(note.kind, note_kinds::AVAILABILITY_FILTER_UNSTATED);
        assert!(
            note.message.contains("no status was stated for 3"),
            "{note:?}"
        );
    }

    /// An electronic title has no copies to say anything about, so "no status was
    /// stated" would send the reader looking for a shelf. The wording names the e-media
    /// instead — and, since round 2 (§3.6), it speaks only of the platforms that state
    /// nothing: the Onleihe's own "ausgeliehen" is read, and claiming it was not was the
    /// sentence that contradicted the note two lines below it.
    #[test]
    fn a_hidden_electronic_title_is_named_rather_than_called_unstated() {
        let note = unstated_note(2, 1).expect("two hidden records are worth a note");
        assert!(!note.message.contains("no status was stated"), "{note:?}");
        assert!(note.message.contains("2 records"), "{note:?}");
        assert!(
            note.message.contains("1 of them is an electronic title"),
            "{note:?}"
        );
        assert!(
            note.message.contains("states no loan status at all"),
            "{note:?}"
        );
        assert!(
            !note.message.contains("this tool does not read"),
            "the loan state is read wherever the platform states one: {note:?}"
        );
    }

    /// The two counts are collected independently, so the message never claims more
    /// electronic titles than there were hidden records.
    #[test]
    fn the_electronic_count_never_exceeds_the_hidden_count() {
        let note = unstated_note(1, 4).expect("one hidden record is worth a note");
        assert!(
            note.message.contains("1 of them is an electronic"),
            "{note:?}"
        );
        assert!(!note.message.contains('4'), "{note:?}");
    }

    /// A page nothing was hidden from carries no note at all: one that always fires is
    /// one nobody reads.
    #[test]
    fn nothing_hidden_is_no_note() {
        assert!(unstated_note(0, 0).is_none());
        assert!(unstated_note(0, 2).is_none());
    }

    /// The count comes from the notes the engines returned, because `items: []` alone
    /// cannot tell an electronic title from a record whose copies were never stated —
    /// and it counts only the platform that states **nothing**. An Onleihe title whose
    /// loan state was read is hidden as a judged record and has no business in a sentence
    /// about records nobody judged (round 2, §3.6).
    #[test]
    fn only_the_titles_without_a_stated_loan_state_are_counted() {
        let notes = vec![
            Note::new(note_kinds::VOEBB_ONLINE_ONLY, "an Onleihe title, read"),
            Note::new(note_kinds::VOEBB_MULTIVOLUME, "a multi-part work"),
            Note::new(
                note_kinds::VOEBB_ONLINE_STATE_UNSTATED,
                "an Overdrive title, silent",
            ),
        ];
        assert_eq!(unstated_online(&notes), 1);
        assert_eq!(unstated_online(&[]), 0);
    }
    /// A listing plan for the tests below: no key, no JSON, and whichever selectors the
    /// case is about.
    fn listing_plan(find: Option<&str>, branches: bool, near: Option<&str>) -> LibrariesPlan {
        LibrariesPlan {
            detail: None,
            find: find.map(str::to_owned),
            branches,
            near: near.map(
                |key| match libraries::look_up(key).expect("a key in the list") {
                    libraries::Entry::Institution(library) => {
                        library.coords().expect("coordinates")
                    }
                    libraries::Entry::Branch { branch, .. } => {
                        branch.coords().expect("coordinates")
                    }
                },
            ),
            near_input: near.map(str::to_owned),
            json: false,
        }
    }

    /// Which shape each invocation asks for. The plain listing is houses, `--find` is
    /// grouped so that no house can be pushed off the answer by its own branches, and the
    /// two flat listings are entries of both kinds.
    #[test]
    fn each_selector_asks_for_the_shape_that_answers_it() {
        assert!(matches!(
            list(&listing_plan(None, false, None)),
            Listing::Houses(rows) if rows.len() == libraries::all().len()
        ));
        assert!(matches!(
            list(&listing_plan(Some("grimm"), false, None)),
            Listing::Groups(groups) if groups.len() == 1
        ));
        assert!(matches!(
            list(&listing_plan(None, true, None)),
            Listing::Entries(rows) if rows.len() == libraries::branch_count()
        ));
        assert!(matches!(
            list(&listing_plan(None, false, Some("HU"))),
            Listing::Entries(rows)
                if rows.len() == libraries::all().len() + libraries::branch_count()
        ));
    }

    /// The selectors narrow each other rather than cancelling out: `--branches --near`
    /// ranks only branches, and `--find` keeps only what it matched — house or branch.
    #[test]
    fn the_selectors_narrow_one_another() {
        let Listing::Entries(rows) = list(&listing_plan(None, true, Some("AGB"))) else {
            panic!("a ranked listing is a list of entries");
        };
        assert_eq!(rows.len(), libraries::branch_count());
        assert!(
            rows.iter()
                .all(|(entry, _)| matches!(entry, libraries::Entry::Branch { .. })),
            "--branches must leave the houses out of the ranking"
        );

        let Listing::Entries(found) = list(&listing_plan(Some("Frohnau"), false, Some("AGB")))
        else {
            panic!("a ranked listing is a list of entries");
        };
        let matched: usize = libraries::find_entries("Frohnau")
            .iter()
            .map(libraries::Found::count)
            .sum();
        assert!(matched > 0, "Frohnau is in the list");
        assert_eq!(
            found.len(),
            matched,
            "the ranking must hold exactly what --find matched"
        );
    }
}
