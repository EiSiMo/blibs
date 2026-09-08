//! `voebb.de` — the public library network, one entry per edition, branch-aware.
//!
//! This engine exists for one reason: the KOBV record does **not** know which branch
//! holds a VÖBB copy (verified: `924 $b DE-609` and nothing else), and a work is
//! scattered over many KOBV records, so a branch filter there could never prove absence.
//! voebb.de has one entry per edition listing every branch. Evidence in `plan/voebb.md`.
//!
//! The site is aDISWeb: session-bound, form-driven, and it answers a broken form with
//! **HTTP 200 and a `/noaccess` page** — a lost cookie, a stale `requestCount`, a missing
//! hidden field. That page is a named error ([`crate::error::UnexpectedError::VoebbNoAccess`],
//! exit 6) and never "no hits". Every step checks for it before parsing anything.
//!
//! Nothing is cached here: a cached session step is meaningless at best.
//!
//! ## What one search costs
//!
//! | step | requests |
//! | --- | --- |
//! | open the session | 1 |
//! | the search (plus 1 when a field flag needs the advanced form) | 1–2 |
//! | the branch facet | 1 |
//! | every further result page the window needs | 1 |
//! | the item list of every **displayed** record | 1 |
//!
//! **One session per location.** Ticking a second branch on an already filtered page
//! would combine the two filters rather than replace them, and there is no measured way
//! back to an unfiltered list; so each location in `--at` gets its own session, run one
//! after the other. The session is a state and concurrency on it is undefined — and the
//! host would not reward parallel sessions anyway: voebb.de is capped at one request in
//! flight, measured (`crate::http::limit`).

pub mod client;
pub mod parse;
pub mod session;

use crate::error::{Error, UnexpectedError};
use crate::http::{Fetch, scope_map};
use crate::libraries::{self, Branch, VOEBB_NETWORK};
use crate::model::{
    AtBlock, AvailabilityMode, Catalog, Engine, EngineSearch, FetchWindow, Format, Holding, Isil,
    Location, Note, QuerySpec, Record, RecordId, SearchRequest, Status, note_kinds,
};

use client::{ResultView, VoebbClient};
use parse::detail::format_of;
use parse::results::Hit;
use session::Session;

/// What names this engine in an error message.
const CONTEXT: &str = "the voebb.de result list";

/// How many rows one result page carries. An observation, not a promise — nothing pages
/// with it, and [`Voebb::collect_window`] reads every row's own absolute position.
const ROWS_PER_PAGE: u32 = 22;

/// How many result pages this tool is willing to walk for one window.
///
/// Every page past the first is a further request on a session that must be replayed in
/// order, so a deep window is both slow and a load on the house. Ten pages is where
/// `plan/voebb.md` draws the line.
const MAX_PAGES: u32 = 10;

/// The last result position a VÖBB window may reach.
///
/// `cli` refuses `--page`/`--limit` past this **before** a session is opened
/// ([`crate::error::UsageError::WindowTooDeep`]): walking there would be 10 sequential
/// requests, and a window that ends beyond it could not be filled at all.
pub const MAX_POSITION: u32 = ROWS_PER_PAGE * MAX_PAGES;

/// The voebb.de engine.
pub struct Voebb<'f> {
    client: VoebbClient<'f>,
}

/// What one location's search produced.
struct Located {
    /// The hit count the site states for this location. `Some(0)` when the facet does not
    /// list the branch at all, which is the site's way of saying "nothing here".
    total: Option<u64>,
    /// The rows inside the requested window.
    hits: Vec<Hit>,
    /// What had to be compromised on the way.
    notes: Vec<Note>,
}

impl Located {
    /// The ids this location's search returned, in the order the site ranked them.
    ///
    /// This is the block's membership and the only honest source of it: every voebb.de
    /// holding carries `DE-609`, so nothing in the record itself says which *branch* the
    /// row came back for, and two branches in `--at` would otherwise show each other's
    /// hits.
    fn ids(&self) -> Vec<RecordId> {
        self.hits.iter().map(|hit| hit.id.clone()).collect()
    }
}

impl<'f> Voebb<'f> {
    /// Build the engine over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self {
            client: VoebbClient::new(fetch),
        }
    }

    /// The transport this engine was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.client.fetch()
    }

    /// One search on its own session: open, search, and — for a branch — filter.
    fn search_at(
        &self,
        query: &QuerySpec,
        branch: Option<&Branch>,
        window: FetchWindow,
    ) -> Result<Located, Error> {
        let mut session = self.client.open()?;
        let (view, mut notes) = self.client.search(&mut session, query)?;

        let view = match branch {
            None => view,
            Some(branch) => {
                let filtered = self.client.filter_branch(&mut session, &view, branch)?;
                let Some(filtered) = filtered else {
                    notes.push(Note::new(
                        note_kinds::VOEBB_BRANCH_NOT_LISTED,
                        format!(
                            "voebb.de's branch facet does not list {:?} for this search, \
                             which means it holds nothing matching",
                            branch.short_name
                        ),
                    ));
                    return Ok(Located {
                        total: Some(0),
                        hits: Vec::new(),
                        notes,
                    });
                };
                filtered
            }
        };

        let total = view.page.total;
        let hits = self.collect_window(&mut session, view, window)?;
        Ok(Located {
            total: Some(total),
            hits,
            notes,
        })
    }

    /// Page forward until the requested window is covered.
    ///
    /// voebb.de serves 22 rows a page and states each row's **absolute** position, so a
    /// window is a range over those positions rather than an offset the site could be
    /// asked for. Paging stops as soon as the window is full, the last requested position
    /// is past, or the toolbar says there is no further page — whichever comes first.
    ///
    /// The row count per page is an observation and not a promise, so nothing here
    /// computes with 22. What it does insist on is **progress**: a further page whose
    /// first row does not sit behind the previous page's last one would loop forever, so
    /// it is a failure instead.
    fn collect_window(
        &self,
        session: &mut Session,
        mut view: ResultView,
        window: FetchWindow,
    ) -> Result<Vec<Hit>, Error> {
        let size = u32::from(window.size.get());
        if size == 0 {
            return Ok(Vec::new());
        }
        let first = window.start;
        let last = first.saturating_add(size - 1);

        let mut hits: Vec<Hit> = Vec::new();
        // Only used for pages whose rows state no position of their own.
        let mut consumed: u32 = 0;
        loop {
            let mut reached = consumed;
            for hit in &view.page.hits {
                consumed = consumed.saturating_add(1);
                let position = hit.position.unwrap_or(consumed);
                reached = position;
                if (first..=last).contains(&position) {
                    hits.push(hit.clone());
                }
            }
            let enough = u32::try_from(hits.len()).is_ok_and(|found| found >= size);
            if enough || reached >= last || !view.page.has_next {
                return Ok(hits);
            }

            view = self.client.next_page(session, &view)?;
            let starts_at = match view.page.hits.first() {
                Some(hit) => hit.position.unwrap_or(consumed.saturating_add(1)),
                None => return Ok(hits),
            };
            if starts_at <= reached {
                return Err(UnexpectedError::CountMismatch {
                    context: format!("{CONTEXT} (paging did not advance)"),
                    expected: usize::try_from(reached).unwrap_or(usize::MAX) + 1,
                    found: usize::try_from(starts_at).unwrap_or(usize::MAX),
                }
                .into());
            }
        }
    }
}

impl Catalog for Voebb<'_> {
    fn engine(&self) -> Engine {
        Engine::Voebb
    }

    /// One search per location in `--at`, each on its own session, one after the other.
    ///
    /// The branch facet is a real upstream filter (71 hits → 35, exactly the number the
    /// facet states), so `at[].total` is the branch's own count and the window pages
    /// through the *filtered* list. The window problem of `plan/cli.md` does not arise
    /// here.
    ///
    /// The records carry only what a result row states — id, title, responsibility, year,
    /// material type. Copies come from the record pages in [`Self::fill_availability`],
    /// one per **displayed** record, which is the same cost rule the KOBV side follows.
    fn search(&self, request: &SearchRequest) -> Result<EngineSearch, Error> {
        let mut records: Vec<Record> = Vec::new();
        let mut at: Vec<AtBlock> = Vec::new();
        let mut notes: Vec<Note> = Vec::new();

        if request.locations.is_empty() {
            let found = self.search_at(&request.query, None, request.window)?;
            extend_records(&mut records, &found.hits);
            notes.extend(found.notes);
        }
        for location in &request.locations {
            let branch = branch_of(location)?;
            let found = self.search_at(&request.query, Some(branch), request.window)?;
            extend_records(&mut records, &found.hits);
            at.push(AtBlock {
                key: location.key.clone(),
                given: location.given.clone(),
                isil: location.isil.clone(),
                branch: Some(branch.kobvid.clone()),
                engine: Engine::Voebb,
                total: found.total,
                records: found.ids(),
            });
            notes.extend(found.notes);
        }

        Ok(EngineSearch {
            engine: Engine::Voebb,
            // Two catalogues have two totals and adding them up would invent a number
            // that is true of neither, so the document's `total` stays the KOBV one.
            // Every voebb count that means something sits in `at[]`.
            total: None,
            fetched: records.len(),
            undelivered: 0,
            at,
            records,
            // PQF is the other engine's language; nothing here has one.
            query_echo: None,
            notes,
        })
    }

    /// Fetch one record page per displayed record and take its copies.
    ///
    /// The record page is stateless, so nothing here depends on the session — but
    /// voebb.de is capped at one request in flight ([`crate::http::limit`]), so these are
    /// serialised by the transport rather than run side by side. Measured: four record
    /// pages cost 7.9 s one after the other and 21.7 s in parallel. The pool stays because
    /// the cap, not this method, is the right place to decide it.
    ///
    /// The record page also carries the properly separated bibliographic fields the result
    /// row lacks, which are used to fill in what the row left empty; nothing the row
    /// stated is overwritten.
    fn fill_availability(&self, records: &mut [Record]) -> Result<Vec<Note>, Error> {
        let wanted: Vec<(usize, RecordId)> = records
            .iter()
            .enumerate()
            .map(|(index, record)| (index, record.id.clone()))
            .collect();
        let answers = scope_map(wanted, |(index, id)| Ok((index, self.client.detail(&id)?)))?;

        let mut notes = Vec::new();
        for (index, answer) in answers {
            // The indices came out of this very slice a moment ago, so this cannot miss;
            // `get_mut` says so without a panicking path through network data.
            let Some(record) = records.get_mut(index) else {
                continue;
            };
            match answer {
                Some(page) => {
                    merge_detail(record, page.record);
                    notes.extend(page.notes);
                }
                // The row named a record the record page no longer knows. Saying nothing
                // would leave an empty copy list that reads as "held nowhere".
                None => notes.push(Note::new(
                    note_kinds::AVAILABILITY_NOT_STATED,
                    format!(
                        "voebb.de listed {} in its results but has no record page for it",
                        record.id
                    ),
                )),
            }
        }
        Ok(notes)
    }

    /// One record by id — a single request, without a session.
    ///
    /// `--no-availability` saves nothing here and is not honoured: voebb.de states the
    /// copies **on the record page itself**, so there is no cheaper page to ask for and
    /// throwing the copies away afterwards would cost the same and answer less.
    fn show(&self, id: &RecordId, _mode: AvailabilityMode) -> Result<Option<Record>, Error> {
        Ok(self.client.detail(id)?.map(|page| page.record))
    }
}

/// The library list's branch behind a resolved location.
///
/// The facet has no key of any kind — no ISIL, no KOBV id, only a display name — so
/// matching it needs the branch's names and its `match` strings, which live in the list.
/// A location that reached this engine came out of `libraries::resolve`, so the lookup
/// failing means the list changed under a running process, not that a user typed
/// something odd.
fn branch_of(location: &Location) -> Result<&'static Branch, Error> {
    let kobvid = location
        .branch
        .as_ref()
        .map(|branch| branch.kobvid.as_str())
        .ok_or_else(|| missing(&format!("a branch for the location {:?}", location.key)))?;
    libraries::by_kobvid(kobvid)
        .and_then(|(_, branch)| branch)
        .ok_or_else(|| missing(&format!("the library list's branch {kobvid:?}")))
}

/// Add one location's rows to the record list, skipping the ones already there.
///
/// The same edition is held by several branches, so two locations return the same record
/// id. The JSON is record-centric — a record appears once however many blocks would show
/// it — and the human blocks are derived from `at[]`, so deduplicating here is what keeps
/// the two consistent.
fn extend_records(records: &mut Vec<Record>, hits: &[Hit]) {
    for hit in hits {
        if !records.iter().any(|record| record.id == hit.id) {
            records.push(record_of(hit));
        }
    }
}

/// A record as far as one result row states it.
///
/// Deliberately thin. The row's traffic light is a summary over **every** branch, so it
/// is not carried into the holding: under a branch heading it would promise something
/// about that branch which the row never said. The status arrives with the copies.
fn record_of(hit: &Hit) -> Record {
    let (format, online) = format_of(hit.media_kind.as_deref().unwrap_or_default(), false);
    Record {
        id: hit.id.clone(),
        title: hit.title.clone(),
        subtitle: None,
        authors: Vec::new(),
        year: hit.year,
        publisher: None,
        place: None,
        edition: None,
        extent: None,
        languages: Vec::new(),
        format,
        online,
        isbns: Vec::new(),
        subjects: Vec::new(),
        urls: Vec::new(),
        holdings: vec![network_holding(&hit.id)],
    }
}

/// The one holding every voebb.de record has: the network, with its copies still unknown.
fn network_holding(id: &RecordId) -> Holding {
    let mut holding = Holding {
        isil: Some(Isil::new(VOEBB_NETWORK)),
        alias: None,
        library: String::new(),
        short_name: None,
        local_id: Some(id.local_id().to_string()),
        mine: false,
        // Not the row's traffic light: that one summarises every branch in the network.
        summary: Status::Unknown,
        items: Vec::new(),
    };
    libraries::name_holding(&mut holding);
    holding
}

/// Take the record page's copies, and everything else it states that the row did not.
///
/// The row's own values win where it had one: its title is the one the user saw in the
/// list, and replacing it with a slightly different transcription would make the two
/// outputs disagree about the same record.
fn merge_detail(record: &mut Record, detailed: Record) {
    record.holdings = detailed.holdings;
    record.authors = detailed.authors;
    record.subjects = detailed.subjects;
    record.isbns = detailed.isbns;
    record.urls = detailed.urls;
    record.languages = detailed.languages;
    record.subtitle = record.subtitle.take().or(detailed.subtitle);
    record.publisher = record.publisher.take().or(detailed.publisher);
    record.place = record.place.take().or(detailed.place);
    record.edition = record.edition.take().or(detailed.edition);
    record.extent = record.extent.take().or(detailed.extent);
    record.year = record.year.or(detailed.year);
    if record.format == Format::Unknown {
        record.format = detailed.format;
    }
    record.online |= detailed.online;
}

/// A structure the site must have delivered and did not.
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
    use crate::model::{Author, AuthorKind};

    fn hit(local: &str, title: &str, media: Option<&str>) -> Hit {
        Hit {
            id: RecordId::voebb(local),
            position: Some(1),
            title: title.to_owned(),
            statement: None,
            year: Some(1997),
            media_kind: media.map(str::to_owned),
            availability: Some("Verfügbar".to_owned()),
            shelfmark: None,
        }
    }

    /// A row states no copies, so the holding says `unknown` — never the row's traffic
    /// light, which is a statement about the whole network and not about a branch.
    #[test]
    fn a_row_never_claims_a_branchs_availability() {
        let record = record_of(&hit("SAK1", "Der Vorleser", Some("Band")));
        let holding = record.holdings.first().expect("one holding");
        assert_eq!(holding.summary, Status::Unknown);
        assert!(holding.items.is_empty());
        assert_eq!(holding.isil.as_ref().map(Isil::as_str), Some(VOEBB_NETWORK));
        assert!(!holding.library.is_empty(), "the list names the network");
    }

    /// The material type of the row is mapped with the same table the record page uses.
    #[test]
    fn the_rows_material_type_becomes_a_format() {
        assert_eq!(
            record_of(&hit("SAK1", "x", Some("Band"))).format,
            Format::Book
        );
        let online = record_of(&hit("SAK2", "x", Some("E-Ressource")));
        assert_eq!(online.format, Format::Ebook);
        assert!(online.online);
        assert_eq!(record_of(&hit("SAK3", "x", None)).format, Format::Unknown);
    }

    /// Two locations that both hold an edition produce **one** record.
    #[test]
    fn the_same_edition_from_two_locations_is_one_record() {
        let mut records = Vec::new();
        extend_records(&mut records, &[hit("SAK1", "Der Vorleser", Some("Band"))]);
        extend_records(&mut records, &[hit("SAK1", "Der Vorleser", Some("Band"))]);
        assert_eq!(records.len(), 1);
    }

    /// The record page fills in what the row could not state, and leaves the title alone.
    #[test]
    fn the_record_page_fills_the_gaps_the_row_left() {
        let mut record = record_of(&hit("SAK1", "Der Vorleser", Some("Band")));
        let mut detailed = record_of(&hit("SAK1", "Der Vorleser : Roman", Some("Band")));
        detailed.authors = vec![Author {
            name: "Schlink, Bernhard".to_owned(),
            kind: AuthorKind::Person,
            dates: None,
            gnd: None,
            role: None,
            role_code: None,
        }];
        detailed.publisher = Some("Diogenes".to_owned());
        detailed.subtitle = Some("Roman".to_owned());

        merge_detail(&mut record, detailed);
        assert_eq!(record.title, "Der Vorleser", "the listed title stands");
        assert_eq!(record.subtitle.as_deref(), Some("Roman"));
        assert_eq!(record.publisher.as_deref(), Some("Diogenes"));
        assert_eq!(record.authors.len(), 1);
    }
}
