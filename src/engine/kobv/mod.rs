//! The KOBV union catalogue: 46.7 million records over ~88 institutions.
//!
//! Two services, one engine. The SRU endpoint answers the search — always as PQF via
//! `x-pquery`, never as CQL, because PQF is the only way to reach Bib-1 attribute `1044`
//! (ISIL) and thus to filter by holdings *upstream*. The portal's availability service
//! answers "is it in right now", one call per record.
//!
//! The traps this engine has to survive are listed in `CLAUDE.md`; the two that shape the
//! code most are that **HTTP 200 means nothing here** — diagnostics, silent truncation
//! and phantom records all arrive with status 200 — and that availability **must not be
//! batched**, because the response is keyed by ISIL and two records' keys collide.

pub mod client;
pub mod parse;
pub mod pqf;

use crate::error::{Error, UnexpectedError};
use crate::http::{Fetch, scope_map};
use crate::libraries;
use crate::model::{
    AtBlock, AvailabilityId, AvailabilityMode, Catalog, Engine, EngineSearch, EngineShow,
    FetchWindow, Holding, Isil, Location, Note, QuerySpec, Record, RecordId, SearchRequest,
    SruPageSize,
};

use client::KobvClient;
use parse::sru::{RecordPayload, SruResponse};
use parse::{availability, record, sru};
use pqf::Pqf;

/// What names the document an error message is about.
const CONTEXT: &str = "the SRU response";

/// How many records beyond the page one SRU search asks for.
///
/// The catalogue delivers the same record twice inside one window — measured against
/// `sru.kobv.de/k2` directly on 2026-09-07, so it is the union catalogue's doing and not
/// this tool's. [`crate::select::dedup`] drops the repeat, and a window of exactly
/// `--limit` records then has nothing left to put in its place: `--limit 10` returned 9
/// records in 4 of 13 measured windows.
///
/// Five is chosen against that measurement rather than as a round number: every window
/// that lost records lost exactly one, and five covers a window in which a fifth of the
/// records are repeats. It costs **no extra request** — SRU serves the whole window in
/// one response — only a handful of MARC records more to parse.
///
/// **KOBV only.** This is compensation for a KOBV defect and it is applied where the SRU
/// request is built, not in [`FetchWindow::plan`]: a voebb.de window is walked 22 rows at
/// a time over sequential requests on a session that must be replayed in order, so five
/// records more there would be a further request against a host that allows one in
/// flight. That would be an etiquette regression traded for a bug that host does not have.
///
/// The one window it cannot help is `--limit 50`: [`SruPageSize`] clamps at the service's
/// own ceiling, so an anchored window (already 50) and a full-size page get no buffer at
/// all. There a dropped duplicate still shortens the page, and the
/// `duplicate_records_dropped` note is what says so.
const OVERDRAW: u32 = 5;

/// The window to send, given the window the user asked for.
///
/// [`OVERDRAW`] records wider, clamped by [`SruPageSize`] at the largest window the
/// service serves — which is also why this never stacks on top of an anchored window:
/// that one is already at the ceiling and comes back unchanged. `start` is untouched, so
/// consecutive pages overlap by [`OVERDRAW`] records; that is harmless, because the page
/// is cut to `--limit` after the duplicates are dropped.
fn overdrawn(window: FetchWindow) -> FetchWindow {
    FetchWindow {
        start: window.start,
        size: SruPageSize::new(u32::from(window.size.get()).saturating_add(OVERDRAW)),
    }
}

/// The KOBV engine.
pub struct Kobv<'f> {
    client: KobvClient<'f>,
}

/// What one SRU search returned, before anything client-side touched it.
struct Found {
    /// The query that produced it, for the JSON echo.
    query: Pqf,
    /// `numberOfRecords` — the hit count of *this* search, which with an ISIL clause is
    /// the location's own total.
    total: u64,
    /// The delivered, converted records.
    records: Vec<Record>,
    /// Announced slots that arrived as something other than a MARC record.
    undelivered: usize,
    /// What the envelope reported about those slots.
    notes: Vec<Note>,
}

impl Found {
    /// The ids this search returned, in the order the catalogue ranked them.
    fn ids(&self) -> Vec<RecordId> {
        self.records
            .iter()
            .map(|record| record.id.clone())
            .collect()
    }
}

impl<'f> Kobv<'f> {
    /// Build the engine over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self {
            client: KobvClient::new(fetch),
        }
    }

    /// The transport this engine was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.client.fetch()
    }

    /// One SRU search: send the query, convert what came back, name the libraries.
    fn search_once(
        &self,
        spec: &QuerySpec,
        isils: &[Isil],
        window: FetchWindow,
    ) -> Result<Found, Error> {
        let query = pqf::search(spec, isils)?;
        // Widened here and nowhere else: `show` looks one record up by id and must keep
        // asking for exactly one.
        let response = self.client.search(&query, overdrawn(window))?;
        let records = records(&response)?;
        Ok(Found {
            query,
            total: response.number_of_records,
            undelivered: undelivered(&response),
            notes: sru::record_notes(&response),
            records,
        })
    }

    /// The whole catalogue, with no holdings filter: one search, one total.
    fn search_everywhere(&self, request: &SearchRequest) -> Result<EngineSearch, Error> {
        let found = self.search_once(&request.query, &[], request.window)?;
        Ok(EngineSearch {
            engine: Engine::Kobv,
            total: Some(found.total),
            fetched: found.records.len(),
            undelivered: found.undelivered,
            at: Vec::new(),
            records: found.records,
            query_echo: Some(found.query.as_str().to_owned()),
            notes: found.notes,
        })
    }

    /// One search per institution in `--at`, up to six at a time.
    ///
    /// Not one `@or` search over all of them: `--limit` is a promise **per block**
    /// (`plan/cli.md` § *Menschliche Ausgabe*), and a joint search can only deliver one
    /// window of records ranked across every location — ten hits of which nine are the
    /// first library's would leave the second block one line long while its catalogue
    /// holds hundreds. A window per location is the only shape that can fill every block,
    /// and it makes the counting requests unnecessary: `numberOfRecords` of a search
    /// restricted to one ISIL *is* that location's total.
    fn search_by_location(
        &self,
        request: &SearchRequest,
        isils: &[Isil],
    ) -> Result<EngineSearch, Error> {
        let searches = scope_map(isils.to_vec(), |isil| {
            self.search_once(&request.query, std::slice::from_ref(&isil), request.window)
        })?;

        // Stated only when exactly one search ran; otherwise there is no single query and
        // no single hit count the whole answer came from.
        let only = match searches.as_slice() {
            [found] => Some(found),
            _ => None,
        };
        let query_echo = only.map(|found| found.query.as_str().to_owned());
        let total = only.map(|found| found.total);

        let mut records: Vec<Record> = Vec::new();
        let mut undelivered = 0;
        let mut notes: Vec<Note> = Vec::new();
        let mut per_isil: Vec<(u64, Vec<RecordId>)> = Vec::new();
        for found in searches {
            undelivered += found.undelivered;
            per_isil.push((found.total, found.ids()));
            // The same limitation reported by two location searches is one limitation.
            for note in found.notes {
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
            merge_records(&mut records, found.records);
        }

        Ok(EngineSearch {
            engine: Engine::Kobv,
            total,
            fetched: records.len(),
            undelivered,
            at: at_blocks(&request.locations, isils, &per_isil),
            records,
            query_echo,
            notes,
        })
    }
}

impl Catalog for Kobv<'_> {
    fn engine(&self) -> Engine {
        Engine::Kobv
    }

    /// One search per location in `--at`, concurrently and capped at six — or a single
    /// unfiltered search when no location was named.
    ///
    /// `--at` is part of the query, not a sieve over the answer: the ISIL goes out as
    /// `@attr 1=1044`, so every location's `total` and its paging apply to the filtered
    /// result. There is no separate counting request any more; a location's search states
    /// its own `numberOfRecords`, so `N` locations cost `N` requests rather than `N + 1`.
    ///
    /// Two locations that resolve to the same ISIL share one search: the query and the
    /// answer would be identical, and paying twice for them buys nothing.
    ///
    /// `total` and the PQF echo are stated only when exactly one search ran. With several
    /// there is no joint hit count that is true of the answer as a whole, and summing the
    /// locations would count every record two of them hold twice — the honest numbers are
    /// the per-location ones in `at[]`.
    fn search(&self, request: &SearchRequest) -> Result<EngineSearch, Error> {
        let isils = filter_isils(&request.locations);
        if isils.is_empty() {
            return self.search_everywhere(request);
        }
        self.search_by_location(request, &isils)
    }

    /// One availability call per record, concurrently, capped at six. Records with no
    /// MARC `924` are skipped rather than asked about — there is no key to ask with.
    ///
    /// A failed call fails the whole fill: the copies of the remaining records would be
    /// missing from an answer that still looked complete, and "not on the shelf" and "not
    /// asked" are indistinguishable to whoever reads it.
    fn fill_availability(&self, records: &mut [Record]) -> Result<Vec<Note>, Error> {
        let asked: Vec<(usize, AvailabilityId)> = records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| Some((index, record::availability_id(&record.holdings)?)))
            .collect();
        let answers = scope_map(asked, |(index, id)| {
            Ok((index, self.client.availability(&id)?))
        })?;

        let mut notes = Vec::new();
        for (index, answer) in answers {
            // The indices were taken from this very slice a moment ago, so this cannot
            // miss; `get_mut` says so without a panicking path through network data.
            let Some(record) = records.get_mut(index) else {
                continue;
            };
            // Every note the availability answer produces is about this one record — a
            // status word nobody knows, a block of copies without an ISIL, a service that
            // holds no information. The id is attached here rather than inside `merge`,
            // which sees holdings and not records, so that a reader never has to guess
            // which of the displayed records a limitation belongs to (round 2, §3.5).
            for mut note in availability::merge(&answer, &mut record.holdings) {
                note.records.push(record.id.clone());
                notes.push(note);
            }
            name_libraries(&mut record.holdings);
        }
        Ok(notes)
    }

    /// Look one record up by its id, and — unless availability was waived — ask for its
    /// copies.
    ///
    /// A `record` of `None` means the catalogue has no such record, which is exit 1 and
    /// not an error. It is returned only when the service delivered nothing *and*
    /// announced nothing: an announced record that arrives as a surrogate diagnostic is a
    /// failure of the response, not an absent record, and saying "no such record" for it
    /// would be a wrong answer rather than a missing one.
    ///
    /// The notes of the availability call travel with the record. They used to be
    /// dropped, so a `show` whose copies the service had nothing to say about printed an
    /// unexplained empty list — the one thing this tool must never do.
    fn show(&self, id: &RecordId, mode: AvailabilityMode) -> Result<EngineShow, Error> {
        let query = pqf::record_lookup(id);
        let window = FetchWindow {
            start: 1,
            size: SruPageSize::new(1),
        };
        let response = self.client.search(&query, window)?;
        if response.records.is_empty() {
            return if response.number_of_records == 0 {
                Ok(EngineShow::default())
            } else {
                Err(missing(&format!(
                    "the record {id}, which the response counted but did not deliver"
                )))
            };
        }
        let Some(mut record) = records(&response)?.into_iter().next() else {
            return Err(missing(&format!("a readable MARC record for {id}")));
        };
        let mut notes = Vec::new();
        if mode == AvailabilityMode::Fetched {
            notes = self.fill_availability(std::slice::from_mut(&mut record))?;
        }
        Ok(EngineShow {
            record: Some(record),
            notes,
        })
    }
}

/// One `at[]` entry per location, in the order the user gave them.
///
/// The totals are the point of the entries: "HU Berlin · 230 results" has to be the number
/// of HU records, not the size of a joint search — and it is, because the location's
/// search asked about nothing else.
///
/// `records` is the block's membership, and it is what that location's own search
/// returned. That is a stronger statement than reading the ISIL back out of the records:
/// the upstream filter decided it, so a record is in the block because the catalogue
/// answered it for this library.
///
/// **A branch block states no total.** The count came back for the *house* — `1044` is
/// the only restriction the catalogue offers — and printing it under a branch's heading
/// would claim a number nobody counted. Which of those records the branch actually holds
/// is decided afterwards, from the copies
/// ([`crate::select::keep_branch_per_location`]), so the honest answer here is `null`.
///
/// Two locations with the same ISIL share the one search that was run for it; a location
/// whose ISIL somehow has no search states nothing rather than borrowing another's
/// numbers.
fn at_blocks(
    locations: &[Location],
    isils: &[Isil],
    per_isil: &[(u64, Vec<RecordId>)],
) -> Vec<AtBlock> {
    locations
        .iter()
        .map(|location| {
            let found = isils
                .iter()
                .position(|isil| *isil == location.isil)
                .and_then(|index| per_isil.get(index));
            AtBlock {
                key: location.key.clone(),
                given: location.given.clone(),
                isil: location.isil.clone(),
                branch: location.branch.as_ref().map(|branch| branch.kobvid.clone()),
                engine: Engine::Kobv,
                total: found
                    .map(|(total, _)| *total)
                    .filter(|_| location.branch.is_none()),
                records: found.map(|(_, ids)| ids.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Add one location's records to the engine's list, skipping the ones already there.
///
/// Two locations that both hold an edition answer with the same record, and the JSON is
/// record-centric: a record appears once however many blocks show it. Which blocks those
/// are is stated by `at[].records`, not by the position in this list.
fn merge_records(records: &mut Vec<Record>, found: Vec<Record>) {
    for record in found {
        if !records.iter().any(|kept| kept.id == record.id) {
            records.push(record);
        }
    }
}

/// The ISILs this engine has to search, deduplicated, in the user's order.
///
/// One search runs per entry, so deduplicating is what keeps two aliases of the same
/// institution from being asked the same question twice. The spelling is the canonical
/// one from the library list, put there by `libraries::resolve`: the attribute is
/// case-sensitive and `de-11` matches nothing, silently.
fn filter_isils(locations: &[Location]) -> Vec<Isil> {
    let mut isils: Vec<Isil> = Vec::with_capacity(locations.len());
    for location in locations {
        if !isils.contains(&location.isil) {
            isils.push(location.isil.clone());
        }
    }
    isils
}

/// The delivered MARC records, converted and named.
///
/// A record that is not MARC is not an error here — it is counted by [`undelivered`] and
/// explained by [`sru::record_notes`]. A MARC record that cannot be converted *is* one:
/// the only conversion failure is a missing identity, and a record without an id can
/// neither be shown again nor asked about.
fn records(response: &SruResponse) -> Result<Vec<Record>, Error> {
    response
        .records
        .iter()
        .filter_map(|delivered| match &delivered.payload {
            RecordPayload::Marc(marc) => Some(marc),
            RecordPayload::Diagnostic(_) | RecordPayload::Unknown { .. } => None,
        })
        .map(|marc| {
            let mut record = record::from_marc(marc)?;
            name_libraries(&mut record.holdings);
            Ok(record)
        })
        .collect()
}

/// How many slots of the result list arrived empty — a surrogate diagnostic or a schema
/// this tool cannot read. Never the shortfall against `maximumRecords`, which is normal.
fn undelivered(response: &SruResponse) -> usize {
    response
        .records
        .iter()
        .filter(|delivered| !matches!(delivered.payload, RecordPayload::Marc(_)))
        .count()
}

/// Add the library list's names to every holding that carries an ISIL.
///
/// This is the *only* thing the list contributes: nothing branches on a specific ISIL, and
/// a code the list does not know keeps the bare ISIL that `parse` put there — an
/// institution that joined the network yesterday must still have its holding shown.
///
/// `library` is the full official name, `short_name` the short one; that split is fixed by
/// the JSON contract in `plan/cli.md`, where `library` reads "Humboldt-Universität zu
/// Berlin, Universitätsbibliothek" next to a `short_name` of "HU Berlin".
fn name_libraries(holdings: &mut [Holding]) {
    for holding in holdings {
        libraries::name_holding(holding);
    }
}

/// A structure the catalogue must have delivered and did not.
fn missing(what: &str) -> Error {
    UnexpectedError::MissingElement {
        what: what.to_owned(),
        context: CONTEXT.to_owned(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Limit, Page};

    /// §1.6: without a buffer, a window of exactly `--limit` records has nothing to put in
    /// the place of a duplicate the dedup drops, and `--limit 10` came back with 9 records
    /// in 4 of 13 measured windows.
    #[test]
    fn an_sru_window_is_overdrawn_so_a_dropped_duplicate_can_be_replaced() {
        let window = FetchWindow::plan(Limit::new(10).expect("10 is in range"), Page::FIRST, false);
        let sent = overdrawn(window);
        assert_eq!(sent.size.get(), 15);
        // The page's position is untouched: consecutive windows overlap by OVERDRAW, which
        // is harmless because the page is cut to `--limit` after the dedup.
        assert_eq!(sent.start, window.start);
    }

    /// The buffer never asks for more than the service serves — and therefore never
    /// stacks on top of an anchored window, which is already at that ceiling.
    #[test]
    fn the_overdraw_is_clamped_at_what_sru_serves() {
        let limit = Limit::new(50).expect("50 is in range");
        let full = FetchWindow::plan(limit, Page::FIRST, false);
        assert_eq!(
            overdrawn(full).size.get(),
            crate::model::page::MAX_SRU_PAGE_SIZE
        );

        let anchored =
            FetchWindow::plan(Limit::new(10).expect("10 is in range"), Page::FIRST, true);
        assert_eq!(overdrawn(anchored), anchored);
    }
}
