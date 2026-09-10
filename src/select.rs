//! Client-side selection: filter, sort, page, mark, group.
//!
//! Everything here sees **only the fetched records**, and that limitation is part of the
//! contract rather than something to paper over. SRU cannot sort at all and has no index
//! for material type or language, so `--sort`, `--format` and `--language` act on the
//! window — and the output must never imply otherwise. `--at` is the exception: it is an
//! upstream filter, so location results are complete and pageable.
//!
//! `--available` ([`keep_available`], [`keep_available_per_location`]) is the one filter
//! that runs *after* the page has been cut, because the status it judges only exists once
//! availability has been fetched — and that is fetched for the page alone. So it sees the
//! records of this page and no others: it **thins the page out rather than refilling it**,
//! and the renderers have to say so.
//!
//! Grouping happens here too, so that the terminal and the JSON cannot disagree about
//! which location holds what. The engine states each location's membership;
//! [`take_page_per_location`] cuts every block to `--limit` on its own,
//! [`assign_blocks`] restates the survivors in the order they will be printed, and
//! [`blocks`] derives the human grouping from a [`SearchResult`]. The last is a *view*,
//! not a second schema: the JSON stays record-centric and a record appears once, no
//! matter how many blocks show it.

use std::cmp::Reverse;
use std::collections::HashSet;

use crate::model::{
    AtBlock, Engine, Format, Holding, Item, Limit, Location, LocationRefusal, Record, RecordId,
    SearchResult, SortKey, Status,
};

/// The client-side filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// `--format`.
    pub format: Option<Format>,
    /// `--language`, an ISO-639-2/B code.
    pub language: Option<String>,
}

impl Filters {
    /// Whether any filter is set — and nothing more.
    ///
    /// Deliberately **not** the answer to "is the window anchored": `--sort` anchors it
    /// too without being a filter, and widening this method to cover that would leave a
    /// name meaning something other than what it says. That question is
    /// [`crate::cli::Plan::anchored`], and the places that reason about *filters* —
    /// `window.filtered` in the JSON, the "none matched --format map" heading, the
    /// depth check for a VÖBB window — keep asking this one.
    pub fn is_active(&self) -> bool {
        self.format.is_some() || self.language.is_some()
    }
}

/// Which slice of the filtered records this page shows.
///
/// The mirror of [`crate::model::FetchWindow`]: that one says which records to ask for,
/// this one says which of the survivors to print. The two have to be planned from the
/// same three values or a page will claim a position it never fetched.
///
/// Without an anchored window the fetched records already *are* the page — `--page` moved
/// `startRecord` — so the offset is zero and this only truncates. When the window is
/// anchored it is one block of 50 raw records and `--page` walks the matches inside it,
/// so the offset is `(page - 1) * limit`. What anchors it is
/// [`crate::cli::Plan::anchored`]: `--format`, `--language`, or any `--sort` but
/// relevance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCut {
    /// How many of the filtered records to skip.
    pub offset: usize,
    /// How many to keep after that.
    pub limit: usize,
}

impl PageCut {
    /// Plan the cut from the same values [`crate::model::FetchWindow::plan`] uses.
    ///
    /// `anchored` has to be the **same** value the window was planned with, or the page
    /// claims a position that was never fetched: an anchored window that is cut without
    /// an offset shows page one under every page number.
    pub fn plan(limit: Limit, page: crate::model::Page, anchored: bool) -> Self {
        let limit = usize::from(limit.get());
        let offset = if anchored {
            (page.get() as usize)
                .saturating_sub(1)
                .saturating_mul(limit)
        } else {
            0
        };
        Self { offset, limit }
    }

    /// The whole of what it is given — for [`assign_blocks`], which restates rather than
    /// cuts.
    fn everything() -> Self {
        Self {
            offset: 0,
            limit: usize::MAX,
        }
    }
}

/// Drop repeated records, keeping the first of each and the order of the rest.
///
/// The union catalogue delivers the same record twice inside one response — verified
/// against `sru.kobv.de/k2` directly, so this is not a parsing artefact. Two things make
/// that worth fixing here rather than tolerating: `records[]` promises each record exactly
/// once, and a repeat would otherwise cost a second availability request for a status
/// already known, against the one-request-per-*displayed*-record rule in `CLAUDE.md`.
///
/// Runs before filtering, sorting and paging, so every later count is over distinct
/// records. Returns how many were dropped, because silence would make the tool disagree
/// with a hit count the user can see.
pub fn dedup(records: Vec<Record>) -> (Vec<Record>, usize) {
    let mut seen: HashSet<RecordId> = HashSet::with_capacity(records.len());
    let before = records.len();
    let kept: Vec<Record> = records
        .into_iter()
        .filter(|record| seen.insert(record.id.clone()))
        .collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

/// Keep the records that match every active filter.
///
/// An empty result here is **not** "nothing found" — it is "nothing in the fetched
/// window matched", and the caller must say so; `window.after_filter` in the JSON and the
/// window line in the human output exist for exactly that distinction.
pub fn filter(records: Vec<Record>, filters: &Filters) -> Vec<Record> {
    records
        .into_iter()
        .filter(|record| passes(record, filters))
        .collect()
}

/// Whether one record survives every active filter.
fn passes(record: &Record, filters: &Filters) -> bool {
    filters.format.is_none_or(|wanted| record.format == wanted)
        && filters
            .language
            .as_deref()
            .is_none_or(|wanted| speaks(record, wanted))
}

/// Whether the record carries the wanted language code.
///
/// Compared case-insensitively: the codes are ASCII (`ger`, `eng`), records spell them
/// lowercase and users type them either way. This is a *contains* test, not equality —
/// a bilingual record carries several codes and matches all of them.
fn speaks(record: &Record, wanted: &str) -> bool {
    record
        .languages
        .iter()
        .any(|code| code.eq_ignore_ascii_case(wanted))
}

/// Sort in place.
///
/// Stable throughout, so that upstream relevance survives as the tie-breaker of every
/// other key: two records of the same year stay in the order the catalogue ranked them.
///
/// [`SortKey::Availability`] is only meaningful after availability has been fetched, and
/// [`SortKey::Availability`] with a location sorts by that location's traffic light
/// rather than the record's best one — "is it in *there*" is the question being asked.
pub fn sort(records: &mut [Record], by: SortKey, at: Option<&Location>) {
    match by {
        // Upstream order *is* relevance; touching it would only destroy it.
        SortKey::Relevance => {}
        // Newest first, and a record without a year sorts after every dated one rather
        // than pretending to be from year zero.
        SortKey::Year => {
            records.sort_by_cached_key(|record| (record.year.is_none(), Reverse(record.year)));
        }
        // By the *displayed* title, article included (`plan/marc-mapping.md`, decision 9):
        // in a terminal it must be visible what the list was sorted by.
        SortKey::Title => records.sort_by_cached_key(|record| collation_key(&record.title)),
        SortKey::Author => records.sort_by_cached_key(author_key),
        SortKey::Availability => {
            // `Status::rank` rather than a table of its own: the sort and
            // `Status::summarize` must agree, and one definition cannot drift from itself.
            records.sort_by_cached_key(|record| Reverse(status_of(record, at).rank()));
        }
    }
}

/// The sort key of the first author: casefolded, with nameless records last.
///
/// The leading `bool` is what puts them last — `false < true`, so every record that has
/// an author comes first, and an author whose name is blank counts as no author at all.
fn author_key(record: &Record) -> (bool, String) {
    let name = record
        .authors
        .first()
        .map(|author| collation_key(&author.name))
        .filter(|name| !name.is_empty());
    (name.is_none(), name.unwrap_or_default())
}

/// The string a `--sort title`/`--sort author` comparison actually runs on.
///
/// Casefolded, and with the four German special letters written out — `ä`/`ö`/`ü` as
/// `ae`/`oe`/`ue`, `ß` as `ss`, in both cases. Without that the comparison is by Unicode
/// codepoint, where `ö` (U+00F6) sorts *after* `z` (U+007A), and in a tool for German
/// libraries every umlaut title lands behind Z — which reads as a broken sort rather than
/// as a collation decision.
///
/// **Only the key.** The displayed title and the displayed author name are never touched:
/// what is printed is what the catalogue holds.
///
/// This is deliberately not DIN 5007 and does not pretend to be. It ignores everything
/// that standard says about other diacritics (`é`, `å`, `č` still sort by codepoint,
/// after `z`), about DIN 5007-2's name variant (where `ä` sorts *as* `a`), and about
/// punctuation and articles. It fixes the German 99 % without a collation crate; anything
/// beyond that needs one.
fn collation_key(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let mut key = String::with_capacity(lowered.len());
    for character in lowered.chars() {
        match character {
            'ä' => key.push_str("ae"),
            'ö' => key.push_str("oe"),
            'ü' => key.push_str("ue"),
            // `to_lowercase` has already turned `ẞ` into `ß`; `ß` itself is unchanged by
            // it, so this arm sees both spellings.
            'ß' => key.push_str("ss"),
            _ => key.push(character),
        }
    }
    key
}

/// The status a sort or a block heading should show for a record: the location's traffic
/// light when there is a location, the record's overall one otherwise.
fn status_of(record: &Record, at: Option<&Location>) -> Status {
    at.map_or_else(
        || record_status(record),
        |location| location_status(record, location),
    )
}

/// Take the requested page out of the filtered, sorted records.
///
/// The offset comes from [`PageCut`] and is zero unless a client-side filter is active:
/// without one the fetched window already *is* the page, and skipping here as well would
/// page twice. With one the window is an anchored block of 50 raw records that holds
/// several pages' worth of matches, and the offset is the only thing that reaches the
/// ones past the first `limit`.
///
/// This is the form without `--at`, where there is one flat list to cut. With locations
/// the cut is per block — [`take_page_per_location`].
pub fn take_page(records: Vec<Record>, cut: PageCut) -> Vec<Record> {
    records
        .into_iter()
        .skip(cut.offset)
        .take(cut.limit)
        .collect()
}

/// Cut **every location's block** to `limit` records and keep only what some block still
/// shows.
///
/// `--limit` is a promise per block (`plan/cli.md` § *Menschliche Ausgabe*): `--at
/// HU,STABI --limit 2` is two lines under each heading, not two lines in total shared
/// out between them. Cutting the merged list instead would let the library that ranks
/// better take the whole page and leave the other block empty — which reads exactly like
/// "nothing there".
///
/// A record that is left in no block at all is dropped from the record list, so that
/// availability is never fetched for a record no heading will show it under.
pub fn take_page_per_location(
    records: Vec<Record>,
    at: &mut [AtBlock],
    cut: PageCut,
) -> Vec<Record> {
    restate(at, &records, cut);
    let shown: Vec<&RecordId> = at.iter().flat_map(|block| &block.records).collect();
    records
        .into_iter()
        .filter(|record| shown.contains(&&record.id))
        .collect()
}

/// Restate `at[].records` over the records that will actually be shown, in their order.
///
/// Called once per engine, last: the list *is* the block, so it has to name exactly the
/// records shown under the heading, in the order of `records[]`. An id `records[]` no
/// longer carries would make the grouping unreconstructable from the JSON, and the order
/// has to follow the final sort — `--sort availability` runs after availability was
/// fetched, and a block whose ids still stood in the pre-sort order would contradict the
/// list above it.
///
/// What it never does is *add* a record. Which records a location holds is the engine's
/// answer — its search for that location returned them — and adding one here from a
/// holding the availability service reported would put a record in a block whose own
/// window never contained it, past the block's `--limit` and past its paging.
pub fn assign_blocks(at: &mut [AtBlock], records: &[Record]) {
    restate(at, records, PageCut::everything());
}

/// The shared half of [`take_page_per_location`] and [`assign_blocks`]: intersect each
/// block with the records, in the records' order, and keep the [`PageCut`]'s slice.
///
/// The offset is applied **per block**, exactly like the limit: `--limit` is a promise
/// per heading, so `--page` has to be one too, or a location's second page would start
/// wherever another location's matches happened to fall.
fn restate(at: &mut [AtBlock], records: &[Record], cut: PageCut) {
    for block in at {
        let stated = std::mem::take(&mut block.records);
        block.records = records
            .iter()
            .map(|record| &record.id)
            .filter(|id| stated.contains(*id))
            .skip(cut.offset)
            .take(cut.limit)
            .cloned()
            .collect();
    }
}

/// How many records `--available` removed, and how many of those said nothing at all.
///
/// The split *is* the feature. Once a record is gone from the page, "it is on loan" and
/// "no status was stated about it" look exactly alike, and only `unstated` keeps them
/// apart — it feeds the note in the JSON and the sentence printed when the filter empties
/// the page. `unstated` counts the drops that carried [`Status::Unknown`] or
/// [`Status::PossiblyAvailable`], the two non-statements, and is therefore never larger
/// than `total`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hidden {
    /// Records the filter removed from the page.
    pub total: usize,
    /// How many of them were removed for a status nobody stated.
    pub unstated: usize,
}

impl Hidden {
    /// Count one removed record. `unstated` says whether it was dropped for a
    /// non-statement rather than for a "no".
    fn drop_one(&mut self, unstated: bool) {
        self.total += 1;
        self.unstated += usize::from(unstated);
    }
}

/// Whether a status is a statement about the copies, or the absence of one.
///
/// [`Status::PossiblyAvailable`] belongs on this side: the black KOBV traffic light is
/// the normal answer for a public library and asserts nothing, so a record dropped for it
/// was never really answered.
fn is_unstated(status: Status) -> bool {
    matches!(status, Status::Unknown | Status::PossiblyAvailable)
}

/// Keep only the records that are borrowable right now — the form without `--at`.
///
/// Judged by [`record_status`], the record's overall traffic light, which is the same
/// light the flat list prints: `--available` shows what the marker already claimed.
/// [`Status::Reference`] is *not* available — a reference copy cannot be taken home —
/// and neither non-statement is, so a record without any `924` holding falls out and is
/// counted as `unstated`: 4.9 % of records state no holdings, and that is "not stated",
/// never "held nowhere".
///
/// The failure mode this guards against is silence: the returned [`Hidden`] is what lets
/// the caller say *how many* records this page lost and how many of them were merely
/// unanswered. An empty result here is never "nothing found".
pub fn keep_available(records: Vec<Record>) -> (Vec<Record>, Hidden) {
    let mut hidden = Hidden::default();
    let kept = records
        .into_iter()
        .filter(|record| {
            let status = record_status(record);
            let keep = status == Status::Available;
            if !keep {
                hidden.drop_one(is_unstated(status));
            }
            keep
        })
        .collect();
    (kept, hidden)
}

/// Narrow each **branch** block of a KOBV search to the records that branch actually
/// holds a copy of.
///
/// The engine filtered the search by the branch's *house*, because that is the only
/// restriction the catalogue offers (`@attr 1=1044`); which branch holds a copy is said
/// by the availability answer alone, one `bibids=` link per copy. So this runs **after**
/// availability and does what the search could not: a record stays in the block when one
/// of its holdings [`holding_is_at`] the location, and leaves it otherwise.
///
/// It changes nothing for a location without a branch, and nothing for a VÖBB branch:
/// there the house facet already filtered upstream and the block is complete, and
/// re-deciding it here from copies that have not been fetched would only be a chance to
/// get it wrong.
///
/// [`holding_is_at`]'s deliberate exception carries straight through — a holding whose
/// copies name **no** branch at all is kept, because a missing link is "not stated", never
/// "not there". Dropping those would answer a broken link as an absence, which is the one
/// answer this tool must never give by accident.
///
/// Returns how many records fell out of every block, so the page can say it rather than
/// simply being short.
pub fn keep_branch_per_location(
    records: Vec<Record>,
    at: &mut [AtBlock],
    locations: &[Location],
) -> (Vec<Record>, usize) {
    let sieved =
        |location: &&Location| location.branch.is_some() && location.engine == Engine::Kobv;
    if !locations.iter().any(|location| sieved(&location)) {
        return (records, 0);
    }
    for block in at.iter_mut() {
        // Matched by alias *and* ISIL, as `run::at_blocks` does. A block whose location is
        // not among `locations` cannot happen — the engine builds `at[]` from these very
        // locations — but judging its records by nothing would empty the whole block, so
        // it is left as the engine stated it.
        let Some(location) = locations
            .iter()
            .find(|location| block.key == location.key && block.isil == location.isil)
            .filter(sieved)
        else {
            continue;
        };
        let stated = std::mem::take(&mut block.records);
        block.records = stated
            .into_iter()
            .filter(|id| {
                records
                    .iter()
                    .find(|record| &record.id == id)
                    .is_some_and(|record| {
                        record
                            .holdings
                            .iter()
                            .any(|holding| holding_is_at(holding, location))
                    })
            })
            .collect();
    }
    let shown: Vec<&RecordId> = at.iter().flat_map(|block| &block.records).collect();
    let mut dropped = 0;
    let kept = records
        .into_iter()
        .filter(|record| {
            let keep = shown.contains(&&record.id);
            dropped += usize::from(!keep);
            keep
        })
        .collect();
    (kept, dropped)
}

/// Keep only what is borrowable **at each location** — the form with `--at`.
///
/// Every block is filtered on its own, by [`location_status`], because that is the light
/// its heading prints: `--at HU,STABI --available` answers "what can I pick up at the HU"
/// and "what can I pick up at the Stabi" separately, and a record can legitimately
/// survive in one block and vanish from the other. A record no block still shows is then
/// dropped from `records`, exactly as [`take_page_per_location`] does it, so that nothing
/// is rendered under no heading.
///
/// A block is matched to its location by `key` **and** ISIL, never by position: the two
/// lists are built independently — `at[]` in user order, `locations` per engine — and
/// zipping them would silently judge one location's records by another's copies.
///
/// `Hidden` counts only the records that fall out of *every* block; one that is still
/// shown somewhere was not hidden. Such a record counts as `unstated` only when no block
/// made a statement about it — a record definitely on loan at one location has been
/// answered, even if another location said nothing.
pub fn keep_available_per_location(
    records: Vec<Record>,
    at: &mut [AtBlock],
    locations: &[Location],
) -> (Vec<Record>, Hidden) {
    let mut unanswered = vec![None; records.len()];
    for block in at.iter_mut() {
        // Matched by alias *and* ISIL, as `run::at_blocks` does. A block whose location
        // is not among `locations` cannot happen — the engine builds `at[]` from these
        // very locations — but judging its records by nothing would keep the whole block,
        // so it falls back to the record's overall light instead.
        let location = locations
            .iter()
            .find(|location| block.key == location.key && block.isil == location.isil);
        let stated = std::mem::take(&mut block.records);
        block.records = stated
            .into_iter()
            .filter(|id| {
                let Some(index) = records.iter().position(|record| &record.id == id) else {
                    // An id with no record behind it can be shown by no block anyway.
                    return false;
                };
                let status = status_of(&records[index], location);
                let slot = &mut unanswered[index];
                *slot = Some(slot.unwrap_or(true) && is_unstated(status));
                status == Status::Available
            })
            .collect();
    }

    let shown: Vec<&RecordId> = at.iter().flat_map(|block| &block.records).collect();
    let mut hidden = Hidden::default();
    let kept = records
        .into_iter()
        .zip(unanswered)
        .filter_map(|(record, unanswered)| {
            if shown.contains(&&record.id) {
                return Some(record);
            }
            // No block judged this record — it stands under no heading, so paging had
            // already dropped it. Judge it by its own light rather than leave it uncounted.
            let unstated = unanswered.unwrap_or_else(|| is_unstated(record_status(&record)));
            hidden.drop_one(unstated);
            None
        })
        .collect();
    (kept, hidden)
}

/// Set `holdings[].mine` for the `--at` locations.
///
/// Idempotent and absolute: a holding that is no longer among the locations is unmarked
/// rather than left over from an earlier call.
pub fn mark_mine(records: &mut [Record], locations: &[Location]) {
    for record in records {
        for holding in &mut record.holdings {
            holding.mine = locations
                .iter()
                .any(|location| holding_is_at(holding, location));
        }
    }
}

/// Whether a holding belongs to a location.
///
/// For an **institution** that is ISIL equality and nothing more. For a **branch** the
/// ISIL cannot decide it: every copy in the Berlin public network is catalogued under
/// `DE-609`, so `--at AGB,BSTB` would show each block the other's holdings. What decides
/// it there is the copies — the holding belongs to the branch when one of them stands in
/// it ([`Item::branch`] against [`BranchRef::kobvid`]).
///
/// The one deliberate exception is a holding whose copies name **no** branch at all,
/// which includes a holding with no copies yet: `--no-availability` was given, or the
/// copies have not been fetched, or the link the branch id is read from was missing. That
/// is "not stated", never "not there", so the holding is kept rather than dropped — the
/// same rule the copy-level match follows one level down. A holding whose copies name only
/// *other* branches is a statement, and it is not this location's.
///
/// Which records a *block* contains is a different question and is answered by
/// [`blocks`]: it uses what the engine reported in `at[].records`, because a record that
/// has no copies yet cannot say which branch's search returned it.
///
/// [`BranchRef::kobvid`]: crate::model::BranchRef::kobvid
pub fn holding_is_at(holding: &Holding, location: &Location) -> bool {
    if holding.isil.as_ref() != Some(&location.isil) {
        return false;
    }
    match location.branch.as_ref() {
        None => true,
        Some(branch) => {
            let kobvid = branch.kobvid.as_str();
            let here = |item: &Item| item.branch.as_deref() == Some(kobvid);
            let unstated = |item: &Item| item.branch.is_none();
            holding.items.iter().any(here) || holding.items.iter().all(unstated)
        }
    }
}

/// The copies of a holding that belong to a location.
///
/// For an institution that is all of them. For a branch, the items whose `branch` id is
/// the branch's — but only when at least one item actually carries it: `items[].branch`
/// comes from a link that need not be there, and reporting *no copies* because a link was
/// missing would be the worst possible answer. When nothing carries the id every copy is
/// shown, which is exactly the case [`holding_is_at`] lets through for the same reason.
fn items_at<'a>(holding: &'a Holding, location: &Location) -> Vec<&'a Item> {
    item_indices_at(holding, location)
        .into_iter()
        .filter_map(|index| holding.items.get(index))
        .collect()
}

/// [`items_at`] by position, for callers that have to merge two locations' answers.
///
/// Positions rather than references because a union of two `Vec<&Item>` can only be
/// deduplicated by pointer identity, which is a fragile thing to compare; an index into
/// `holding.items` is the copy's own name inside its holding.
fn item_indices_at(holding: &Holding, location: &Location) -> Vec<usize> {
    let all = || (0..holding.items.len()).collect::<Vec<usize>>();
    let Some(branch) = location.branch.as_ref() else {
        return all();
    };
    let of_branch: Vec<usize> = holding
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.branch.as_deref() == Some(branch.kobvid.as_str()))
        .map(|(index, _)| index)
        .collect();
    if of_branch.is_empty() {
        all()
    } else {
        of_branch
    }
}

/// The traffic light of one holding: its copies when they are known, the library-level
/// light otherwise. Empty `items` means "not asked" as often as "nothing came back", so
/// it never downgrades the light the service already gave.
pub fn holding_status(holding: &Holding) -> Status {
    if holding.items.is_empty() {
        holding.summary
    } else {
        Status::summarize(holding.items.iter().map(|item| item.status))
    }
}

/// The traffic light for one location — *not* the record's overall one.
///
/// Summarised over that location's copies, because the question under a location heading
/// is "can I go there". Without copies it falls back to the library-level light, and a
/// record the location does not hold at all is [`Status::Unknown`], never
/// [`Status::Unavailable`]: nothing was said about it.
pub fn location_status(record: &Record, location: &Location) -> Status {
    let holdings: Vec<&Holding> = record
        .holdings
        .iter()
        .filter(|holding| holding_is_at(holding, location))
        .collect();
    if holdings.is_empty() {
        return Status::Unknown;
    }
    let items: Vec<&Item> = holdings
        .iter()
        .flat_map(|holding| items_at(holding, location))
        .collect();
    if items.is_empty() {
        Status::summarize(holdings.iter().map(|holding| holding.summary))
    } else {
        Status::summarize(items.iter().map(|item| item.status))
    }
}

/// The record's overall traffic light: the best any library offers.
///
/// This is the marker of the flat list shown without `--at`, where `●` means "available
/// *somewhere*". A record with no `924` fields summarises to [`Status::Unknown`] — 4.9 %
/// of records have none, and that is "not stated", not "held nowhere".
pub fn record_status(record: &Record) -> Status {
    Status::summarize(record.holdings.iter().map(holding_status))
}

/// How many of a set of copies are in, out of how many said anything at all.
///
/// The denominator is the point. `0 of 2 available` next to two lines reading "status not
/// confirmed" is a counting statement about copies nobody counted: a human reads "both
/// gone" where the truth is "nothing was said". So a copy whose status is a
/// non-statement — [`Status::Unknown`] or the black light [`Status::PossiblyAvailable`],
/// the same two `is_unstated` recognises everywhere else — is in neither number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvailableCount {
    /// Copies that are in and borrowable.
    pub available: usize,
    /// Copies whose status is a statement at all. Never zero — see [`available_count`].
    pub known: usize,
}

/// Count the copies that are in, or `None` when there is nothing to count.
///
/// `None` means **no copy stated a status**, and it is the whole reason this returns an
/// option: there is no honest fraction to print, and printing `0 of 2` instead is the bug
/// this replaces. A renderer that gets `None` prints the traffic light and no count.
pub fn available_count<'a>(items: impl IntoIterator<Item = &'a Item>) -> Option<AvailableCount> {
    let mut count = AvailableCount {
        available: 0,
        known: 0,
    };
    for item in items {
        if is_unstated(item.status) {
            continue;
        }
        count.known += 1;
        count.available += usize::from(item.status == Status::Available);
    }
    (count.known > 0).then_some(count)
}

/// One holding of a `show`, narrowed to the locations the user named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldingAt<'a> {
    /// Where the holding sits in `record.holdings`, so a renderer can name it back.
    pub index: usize,
    /// The holding itself, unnarrowed — `library`, `summary` and the rest are the house's.
    pub holding: &'a Holding,
    /// The `--at` locations this holding belongs to, in the order the user gave them.
    ///
    /// Usually one. Two when `--at` names two branches of the same house, which the
    /// holding then belongs to twice — and dropping the second would hide its copies
    /// without a word.
    pub locations: Vec<&'a Location>,
    /// The copies of this holding that stand at one of [`Self::locations`], in the
    /// holding's own order. For an institution that is every copy; for a branch the ones
    /// whose `branch` id matches — unless *no* copy names a branch at all, in which case
    /// they are all kept, because a missing link is "not stated" and never "not there".
    pub items: Vec<&'a Item>,
    /// The traffic light over [`Self::items`], falling back to the holding's
    /// library-level light when there are no copies to summarise.
    pub status: Status,
}

/// The holdings of a `show`, split into the user's own and the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowHoldings<'a> {
    /// The holdings at some `--at` location, in the order of `--at` and then of the
    /// record. Without `--at`, every holding in record order, each with no location and
    /// all of its copies.
    pub mine: Vec<HoldingAt<'a>>,
    /// The indices of the holdings no location claimed — the `also at:` line. Empty
    /// without `--at`.
    pub others: Vec<usize>,
}

/// Narrow a record's holdings to `--at`, copies included.
///
/// **The one place that decides what `show --at …` is about**, so that the terminal and
/// the JSON cannot answer differently. The search path has always narrowed both levels —
/// [`holding_is_at`] for which holding, `items_at` for which of its copies — and `show`
/// narrowed only the first, so `show <id> --at AGB` printed all four copies of a VÖBB
/// record and a green light for a book that is on loan at the AGB. Membership and copies
/// are one question and are answered here once.
///
/// Without locations there is nothing to narrow: every holding is "mine", with every copy
/// and no location, and [`Self::others`] is empty. That is what `show` without `--at`
/// shows, and it is not a claim that the user owns the whole catalogue.
///
/// [`Self::others`]: ShowHoldings::others
pub fn show_holdings<'a>(record: &'a Record, locations: &'a [Location]) -> ShowHoldings<'a> {
    if locations.is_empty() {
        let mine = record
            .holdings
            .iter()
            .enumerate()
            .map(|(index, holding)| HoldingAt {
                index,
                holding,
                locations: Vec::new(),
                items: holding.items.iter().collect(),
                status: holding_status(holding),
            })
            .collect();
        return ShowHoldings {
            mine,
            others: Vec::new(),
        };
    }

    let mut mine: Vec<HoldingAt<'a>> = Vec::new();
    // Outer loop over the locations, so the block is ordered the way the user asked
    // rather than the way the catalogue listed the libraries.
    for location in locations {
        for (index, holding) in record.holdings.iter().enumerate() {
            if !holding_is_at(holding, location) {
                continue;
            }
            match mine.iter_mut().find(|entry| entry.index == index) {
                // Seen under an earlier location: this one only widens the set of copies.
                Some(entry) => entry.locations.push(location),
                None => mine.push(HoldingAt {
                    index,
                    holding,
                    locations: vec![location],
                    items: Vec::new(),
                    status: Status::Unknown,
                }),
            }
        }
    }
    for entry in &mut mine {
        let mut indices: Vec<usize> = entry
            .locations
            .iter()
            .flat_map(|location| item_indices_at(entry.holding, location))
            .collect();
        indices.sort_unstable();
        indices.dedup();
        entry.items = indices
            .into_iter()
            .filter_map(|index| entry.holding.items.get(index))
            .collect();
        entry.status = if entry.items.is_empty() {
            entry.holding.summary
        } else {
            Status::summarize(entry.items.iter().map(|item| item.status))
        };
    }

    let claimed: Vec<usize> = mine.iter().map(|entry| entry.index).collect();
    let others = (0..record.holdings.len())
        .filter(|index| !claimed.contains(index))
        .collect();
    ShowHoldings { mine, others }
}

/// The `at[]` of a `show` document: one entry per `--at` location, with the record's
/// status there.
///
/// Built here rather than in [`crate::model::ShowResult::new`] for the same reason
/// `cli::run` builds the search document's `at[]`: the status is
/// [`location_status`], which knows the branch narrowing, and a second copy of that
/// knowledge inside `model` is what let the two outputs disagree.
///
/// Without a record — the catalogue has no such id — every location still gets its entry,
/// with [`Status::Unknown`]: the question was asked and the answer is that nothing is
/// known, which is not the same as the location having been dropped from the document.
pub fn show_at(record: Option<&Record>, locations: &[Location]) -> Vec<crate::model::ShowAt> {
    locations
        .iter()
        .map(|location| crate::model::ShowAt {
            key: location.key.clone(),
            given: location.given.clone(),
            isil: location.isil.clone(),
            branch: location.branch.as_ref().map(|branch| branch.kobvid.clone()),
            engine: location.engine,
            status: record.map_or(Status::Unknown, |record| location_status(record, location)),
        })
        .collect()
}

/// One location's block of the human output.
///
/// Borrows the result: a record shown under three locations is one record, not three
/// copies of one. The JSON renderer never sees these — it stays record-centric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block<'a> {
    /// The location this block is about. `None` for the flat list shown without `--at`.
    pub location: Option<&'a Location>,
    /// The location's true hit count, when the engine could state one.
    pub total: Option<u64>,
    /// Why this location was not searched, when it was not — [`AtBlock::refused`]
    /// carried through.
    ///
    /// A block that was refused is empty for a reason that has nothing to do with what
    /// the library holds, and the heading has to say so instead of `no results`: the
    /// renderer cannot tell the two apart from the records, and the note that explains it
    /// sits two lines further down in prose.
    pub refused: Option<LocationRefusal>,
    /// The records shown under it.
    pub records: Vec<BlockRecord<'a>>,
}

/// One record inside a block, reduced to what that location holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord<'a> {
    /// The record itself.
    pub record: &'a Record,
    /// The traffic light **for this location**, not for the record overall.
    pub status: Status,
    /// The copies at this location only. Empty in the flat block: without `--at` there
    /// are no copy lines to show.
    pub items: Vec<&'a Item>,
}

/// Group a result by location for the human renderer.
///
/// With `--at`, one block per location in the order the user gave them — including a
/// location that holds nothing, because a missing block cannot be told apart from a
/// forgotten one. Without `--at`, a single block with `location: None` and one line per
/// record.
///
/// `locations` is a parameter rather than read off `result.at`, which carries totals but
/// not the display name a heading needs, and is per engine rather than in user order.
pub fn blocks<'a>(result: &'a SearchResult, locations: &'a [Location]) -> Vec<Block<'a>> {
    if locations.is_empty() {
        return vec![flat_block(result)];
    }
    locations
        .iter()
        .map(|location| location_block(result, location))
        .collect()
}

/// The single block of the flat list: every record, with its overall traffic light.
fn flat_block(result: &SearchResult) -> Block<'_> {
    Block {
        location: None,
        total: result.total,
        // Nothing to refuse: the flat list is the answer of a search with no `--at` at
        // all, and every refusal there is the run's.
        refused: None,
        records: result
            .records
            .iter()
            .map(|record| BlockRecord {
                record,
                status: record_status(record),
                items: Vec::new(),
            })
            .collect(),
    }
}

/// One location's block: the records held there, in the order the engine ranked them.
fn location_block<'a>(result: &'a SearchResult, location: &'a Location) -> Block<'a> {
    let stated = stated_records(result, location);
    let records = result
        .records
        .iter()
        .filter(|record| record_is_at(record, location, stated))
        .map(|record| BlockRecord {
            record,
            status: location_status(record, location),
            items: record
                .holdings
                .iter()
                .filter(|holding| holding_is_at(holding, location))
                .flat_map(|holding| items_at(holding, location))
                .collect(),
        })
        .collect();
    Block {
        location: Some(location),
        total: total_at(result, location),
        refused: refusal_at(result, location),
        records,
    }
}

/// Why this location was not searched, as its `at[]` entry states it.
///
/// Read off the entry rather than guessed from "no total and no records": those two are
/// also what a location nobody reported on looks like, and telling a user their library
/// holds nothing when nothing was ever asked is the one thing a heading may not do.
fn refusal_at(result: &SearchResult, location: &Location) -> Option<LocationRefusal> {
    result
        .at
        .iter()
        .find(|block| block.key == location.key)
        .and_then(|block| block.refused)
}

/// The membership `at[]` states for a location, if it states one.
///
/// `None` means no engine reported a block for this location — then, and only then, the
/// holdings have to answer the question themselves.
fn stated_records<'a>(result: &'a SearchResult, location: &Location) -> Option<&'a [RecordId]> {
    result
        .at
        .iter()
        .find(|block| block.key == location.key)
        .map(|block| block.records.as_slice())
}

/// Whether a record belongs under a location's heading.
///
/// The engine's own answer wins wherever there is one: it knows which search returned the
/// record, and for a VÖBB branch that is the *only* place the information exists — every
/// holding carries the network's `DE-609`, and a record whose copies have not been
/// fetched carries nothing that names a branch at all. Falling back to the holdings keeps
/// a hand-built result (and any location no engine reported on) rendering sensibly.
fn record_is_at(record: &Record, location: &Location, stated: Option<&[RecordId]>) -> bool {
    match stated {
        Some(ids) => ids.contains(&record.id),
        None => record
            .holdings
            .iter()
            .any(|holding| holding_is_at(holding, location)),
    }
}

/// The location's own hit count, matched by the alias the user typed.
///
/// `None` when the engine could not state one — `at[]` is built per engine and voebb.de
/// cannot always count a branch. A missing total is printed as a missing total, never as
/// the number of records that happen to be on this page.
fn total_at(result: &SearchResult, location: &Location) -> Option<u64> {
    result
        .at
        .iter()
        .find(|block| block.key == location.key)
        .and_then(|block| block.total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AtBlock, Author, AuthorKind, AvailabilityMode, BranchRef, Engine, Isil, Page, QueryEcho,
        RecordId, SortScope, SortSpec, WindowInfo,
    };

    fn record(id: &str, title: &str, year: Option<i32>) -> Record {
        Record {
            id: RecordId::parse(id).expect("the fixture ids are prefixed"),
            title: title.to_owned(),
            subtitle: None,
            authors: Vec::new(),
            year,
            publisher: None,
            place: None,
            edition: None,
            extent: None,
            languages: vec!["ger".to_owned()],
            format: Format::Book,
            online: false,
            isbns: Vec::new(),
            subjects: Vec::new(),
            urls: Vec::new(),
            holdings: Vec::new(),
        }
    }

    fn with_author(mut record: Record, name: &str) -> Record {
        record.authors.push(Author {
            name: name.to_owned(),
            kind: AuthorKind::Person,
            dates: None,
            gnd: None,
            role: None,
            role_code: None,
        });
        record
    }

    fn holding(isil: &str, summary: Status, items: Vec<Item>) -> Holding {
        Holding {
            isil: Some(Isil::new(isil)),
            alias: None,
            library: isil.to_owned(),
            short_name: None,
            local_id: None,
            mine: false,
            summary,
            // Prose holdings: only voebb.de states any.
            holdings_statement: None,
            online_access: None,
            items,
        }
    }

    fn item(location: &str, branch: Option<&str>, status: Status) -> Item {
        Item {
            location: Some(location.to_owned()),
            branch: branch.map(ToOwned::to_owned),
            branch_name: None,
            call_number: None,
            volume: None,
            status,
            // A return date: only voebb.de states one.
            due_date: None,
            order_option: None,
        }
    }

    fn ids(all: &[&str]) -> Vec<RecordId> {
        all.iter()
            .map(|id| RecordId::parse(id).expect("the fixture ids are prefixed"))
            .collect()
    }

    fn institution(key: &str, isil: &str) -> Location {
        Location {
            key: key.to_owned(),
            given: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine: Engine::Kobv,
            display: format!("{key} display"),
        }
    }

    fn branch(key: &str, kobvid: &str) -> Location {
        Location {
            key: key.to_owned(),
            given: key.to_owned(),
            isil: Isil::new("DE-609"),
            branch: Some(BranchRef {
                kobvid: kobvid.to_owned(),
                name: key.to_owned(),
            }),
            engine: Engine::Voebb,
            display: format!("{key} (VÖBB)"),
        }
    }

    /// A branch of a KOBV institution, the case `keep_branch_per_location` exists for.
    fn kobv_branch(key: &str, isil: &str, kobvid: &str) -> Location {
        Location {
            key: key.to_owned(),
            given: key.to_owned(),
            isil: Isil::new(isil),
            branch: Some(BranchRef {
                kobvid: kobvid.to_owned(),
                name: key.to_owned(),
            }),
            engine: Engine::Kobv,
            display: format!("{key} display"),
        }
    }

    /// Three records over two locations, the shape the human renderer is built for:
    /// record 1 is held at both, record 2 only at HU, record 3 nowhere.
    fn result() -> (SearchResult, Vec<Location>) {
        let mut first = with_author(
            record("almahu_1", "Der Vorleser", Some(1997)),
            "Schlink, B.",
        );
        first.holdings = vec![
            holding(
                "DE-11",
                Status::Available,
                vec![
                    item("Grimm-Zentrum", None, Status::Available),
                    item("ZwB Germanistik", None, Status::Unavailable),
                ],
            ),
            holding(
                "DE-1",
                Status::Reference,
                vec![item("Haus Potsdamer Straße", None, Status::Reference)],
            ),
        ];

        let mut second = with_author(record("almahu_2", "Bernhard Schlink", Some(2004)), "Mittel");
        second.holdings = vec![holding(
            "DE-11",
            Status::Unavailable,
            vec![item("Grimm-Zentrum", None, Status::Unavailable)],
        )];
        second.format = Format::Video;
        second.languages = vec!["eng".to_owned()];

        let third = record("almafu_3", "Ohne Bestand", None);

        let locations = vec![institution("HU", "DE-11"), institution("STABI", "DE-1")];
        let result = SearchResult {
            query: QueryEcho {
                terms: "Vorleser".to_owned(),
                pqf: None,
            },
            total: Some(774),
            shown: 3,
            page: Page::FIRST,
            limit: 10,
            sort: SortSpec {
                by: SortKey::Relevance,
                scope: SortScope::Fetched,
            },
            window: WindowInfo {
                fetched: 3,
                after_filter: 3,
                filtered: false,
                undelivered: 0,
                before_available: None,
            },
            engines: vec![Engine::Kobv],
            at: vec![
                AtBlock {
                    key: "HU".to_owned(),
                    given: "HU".to_owned(),
                    isil: Isil::new("DE-11"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: Some(6),
                    records: ids(&["almahu_1", "almahu_2"]),
                    refused: None,
                },
                AtBlock {
                    key: "STABI".to_owned(),
                    given: "STABI".to_owned(),
                    isil: Isil::new("DE-1"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: None,
                    records: ids(&["almahu_1"]),
                    refused: None,
                },
            ],
            availability: AvailabilityMode::Fetched,
            notes: Vec::new(),
            records: vec![first, second, third],
        };
        (result, locations)
    }

    #[test]
    fn no_filter_keeps_everything() {
        let (result, _) = result();
        let kept = filter(result.records.clone(), &Filters::default());
        assert_eq!(kept.len(), 3);
        assert!(!Filters::default().is_active());
    }

    #[test]
    fn the_format_filter_keeps_only_that_material() {
        let (result, _) = result();
        let filters = Filters {
            format: Some(Format::Video),
            language: None,
        };
        assert!(filters.is_active());
        let kept = filter(result.records.clone(), &filters);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "Bernhard Schlink");
    }

    /// The codes are ASCII and users type them either way; the record's spelling wins.
    #[test]
    fn the_language_filter_compares_case_insensitively() {
        let (result, _) = result();
        for wanted in ["ger", "GER", "Ger"] {
            let filters = Filters {
                format: None,
                language: Some(wanted.to_owned()),
            };
            let kept = filter(result.records.clone(), &filters);
            assert_eq!(
                kept.len(),
                2,
                "{wanted} should match the two German records"
            );
        }
    }

    /// A record carries every language it is in, so a multilingual record matches each.
    #[test]
    fn a_record_matches_any_of_its_languages() {
        let mut multi = record("almahu_9", "Zweisprachig", None);
        multi.languages = vec!["ger".to_owned(), "lat".to_owned()];
        for wanted in ["ger", "lat"] {
            let filters = Filters {
                format: None,
                language: Some(wanted.to_owned()),
            };
            assert_eq!(filter(vec![multi.clone()], &filters).len(), 1);
        }
        let filters = Filters {
            format: None,
            language: Some("fre".to_owned()),
        };
        assert!(filter(vec![multi], &filters).is_empty());
    }

    /// Both filters at once are an AND, not an OR.
    #[test]
    fn two_filters_must_both_match() {
        let (result, _) = result();
        let filters = Filters {
            format: Some(Format::Video),
            language: Some("ger".to_owned()),
        };
        assert!(filter(result.records, &filters).is_empty());
    }

    /// Without `--at` the record's overall light decides: only the record some library
    /// lends survives, and the two drops are told apart — one is on loan, the other
    /// states no holdings at all.
    #[test]
    fn the_availability_filter_keeps_only_what_is_in() {
        let (result, _) = result();
        let (kept, hidden) = keep_available(result.records);
        let ids: Vec<&str> = kept.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1"]);
        assert_eq!(hidden.total, 2);
        assert_eq!(hidden.unstated, 1, "only the record without holdings");
    }

    /// `Reference` and `Unavailable` are answers, `PossiblyAvailable` and `Unknown` are
    /// not — and the filter drops all four while counting only the last two.
    #[test]
    fn unstated_counts_the_non_statements_and_nothing_else() {
        let statuses = [
            Status::Available,
            Status::Reference,
            Status::Unavailable,
            Status::PossiblyAvailable,
        ];
        let records: Vec<Record> = statuses
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let mut one = record(&format!("almahu_{index}"), "Titel", None);
                one.holdings = vec![holding("DE-11", *status, Vec::new())];
                one
            })
            .collect();

        let (kept, hidden) = keep_available(records);
        assert_eq!(kept.len(), 1, "reference stock is not borrowable");
        assert_eq!(hidden.total, 3);
        assert_eq!(hidden.unstated, 1, "possibly_available says nothing");
    }

    /// The 4.9 % trap: no `924` means "not stated in this record", never "held nowhere",
    /// so the record falls out as *unstated* rather than as a "no".
    #[test]
    fn a_record_without_holdings_is_hidden_as_unstated() {
        let (_, hidden) = keep_available(vec![record("almafu_3", "Ohne Bestand", None)]);
        assert_eq!(hidden.total, 1);
        assert_eq!(hidden.unstated, 1);
    }

    /// Blockweise: the same record is lent by the HU and only readable at the Stabi, so
    /// it survives under one heading and vanishes from the other — and `records` keeps it,
    /// because a block still shows it.
    #[test]
    fn a_record_survives_in_the_block_that_lends_it() {
        let (mut result, locations) = result();
        // Paging has already dropped the record no block shows.
        result.records.pop();
        result.at[0].records = ids(&["almahu_1", "almahu_2"]);
        result.at[1].records = ids(&["almahu_1"]);

        let (kept, hidden) =
            keep_available_per_location(result.records, &mut result.at, &locations);

        let of = |index: usize| -> Vec<&str> {
            result.at[index]
                .records
                .iter()
                .map(RecordId::as_str)
                .collect()
        };
        assert_eq!(of(0), ["almahu_1"], "the HU lends it");
        assert!(of(1).is_empty(), "the Stabi copy is reference stock");
        let shown: Vec<&str> = kept.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(shown, ["almahu_1"], "one block still shows it");
        assert_eq!(hidden.total, 1, "only the record no block still shows");
        assert_eq!(hidden.unstated, 0, "the HU said it is on loan");
    }

    /// A record that falls out of every block leaves the page. It is *not* unstated: one
    /// block said it is on loan, even though the other said nothing about it at all.
    #[test]
    fn a_record_no_block_lends_leaves_the_page() {
        let (mut result, locations) = result();
        result.records.pop();
        result.at[0].records = ids(&["almahu_1", "almahu_2"]);
        result.at[1].records = ids(&["almahu_2"]);

        let (kept, hidden) =
            keep_available_per_location(result.records, &mut result.at, &locations);

        let shown: Vec<&str> = kept.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(shown, ["almahu_1"]);
        assert_eq!(hidden.total, 1);
        assert_eq!(
            hidden.unstated, 0,
            "on loan at the HU outweighs the Stabi saying nothing"
        );
    }

    /// A VÖBB branch: every holding carries the network's `DE-609`, so the copies decide,
    /// and the filter means the same thing there — what is on the shelf *of that branch*.
    #[test]
    fn a_branch_filters_by_the_copies_standing_in_it() {
        let (mut result, locations) = two_branches();
        let (kept, hidden) =
            keep_available_per_location(result.records, &mut result.at, &locations);

        let shown: Vec<&str> = kept.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(shown, ["voebb_SAK1"], "the AGB copy is in");
        let bstb: Vec<&str> = result.at[1].records.iter().map(RecordId::as_str).collect();
        assert!(bstb.is_empty(), "the BSTB copy is on loan");
        assert_eq!(hidden.total, 1);
        assert_eq!(hidden.unstated, 0);
    }

    #[test]
    fn relevance_leaves_the_upstream_order_alone() {
        let (result, _) = result();
        let mut records = result.records;
        sort(&mut records, SortKey::Relevance, None);
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2", "almafu_3"]);
    }

    /// Newest first, and the undated record last rather than in year zero.
    #[test]
    fn year_sorts_newest_first_with_undated_records_last() {
        let (result, _) = result();
        let mut records = result.records;
        sort(&mut records, SortKey::Year, None);
        let years: Vec<Option<i32>> = records.iter().map(|r| r.year).collect();
        assert_eq!(years, [Some(2004), Some(1997), None]);
    }

    /// Updated: this used to assert `["Alpha", "zeta", "Ähre"]`, which is what sorting by
    /// codepoint produces and what a user of a German library tool reads as a broken
    /// sort. `Ähre` now folds to `aehre` and lands where a German list expects it — the
    /// displayed title is untouched, only the key changed.
    #[test]
    fn title_sorts_by_the_displayed_title_casefolded() {
        let mut records = vec![
            record("almahu_1", "zeta", None),
            record("almahu_2", "Ähre", None),
            record("almahu_3", "Alpha", None),
        ];
        sort(&mut records, SortKey::Title, None);
        let titles: Vec<&str> = records.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Ähre", "Alpha", "zeta"]);
    }

    /// Decision 9 in `plan/marc-mapping.md`: the *displayed* title, article included —
    /// a list that starts with "Der" and claims to be alphabetical reads like a bug, so
    /// the sort must match what the terminal shows.
    #[test]
    fn title_sorts_with_the_article_because_that_is_what_is_shown() {
        let mut records = vec![
            record("almahu_1", "Amerika", None),
            record("almahu_2", "Der Prozess", None),
        ];
        sort(&mut records, SortKey::Title, None);
        let titles: Vec<&str> = records.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Amerika", "Der Prozess"]);
    }

    #[test]
    fn author_sorts_casefolded_with_nameless_records_last() {
        let mut records = vec![
            record("almahu_1", "Anonym", None),
            with_author(record("almahu_2", "Zweiter", None), "schlink, Bernhard"),
            with_author(record("almahu_3", "Dritter", None), "Kafka, Franz"),
        ];
        sort(&mut records, SortKey::Author, None);
        let titles: Vec<&str> = records.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Dritter", "Zweiter", "Anonym"]);
    }

    /// Every sort is stable, so upstream relevance survives as the tie-breaker.
    #[test]
    fn equal_keys_keep_the_upstream_order() {
        let mut records = vec![
            record("almahu_1", "Gleich", Some(2000)),
            record("almahu_2", "Gleich", Some(2000)),
            record("almahu_3", "Gleich", Some(2000)),
        ];
        for key in [SortKey::Year, SortKey::Title, SortKey::Author] {
            let mut shuffled = records.clone();
            sort(&mut shuffled, key, None);
            let ids: Vec<&str> = shuffled.iter().map(|r| r.id.as_str()).collect();
            assert_eq!(ids, ["almahu_1", "almahu_2", "almahu_3"], "{key:?}");
        }
        sort(&mut records, SortKey::Availability, None);
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2", "almahu_3"]);
    }

    #[test]
    fn availability_sorts_the_best_traffic_light_first() {
        let (result, _) = result();
        let mut records = result.records;
        sort(&mut records, SortKey::Availability, None);
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        // 1 is available somewhere, 2 is on loan, 3 states no holdings at all.
        assert_eq!(ids, ["almahu_1", "almahu_2", "almafu_3"]);
    }

    /// With a location the question is "is it in *there*", and the answer differs from
    /// the record's best light: the record HU lends outranks everything unscoped, but
    /// under the Stabi heading it is not held at all and the reference copy wins.
    #[test]
    fn availability_with_a_location_sorts_by_that_location() {
        let mut at_hu = record("almahu_1", "Nur an der HU", None);
        at_hu.holdings = vec![holding(
            "DE-11",
            Status::Available,
            vec![item("Grimm-Zentrum", None, Status::Available)],
        )];
        let mut at_stabi = record("almafu_2", "Nur in der Stabi", None);
        at_stabi.holdings = vec![holding(
            "DE-1",
            Status::Reference,
            vec![item("Haus Potsdamer Straße", None, Status::Reference)],
        )];
        let unsorted = vec![at_hu, at_stabi];
        let stabi = institution("STABI", "DE-1");

        let mut records = unsorted.clone();
        sort(&mut records, SortKey::Availability, None);
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["almahu_1", "almafu_2"],
            "unscoped: the best light wins"
        );

        let mut records = unsorted;
        sort(&mut records, SortKey::Availability, Some(&stabi));
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["almafu_2", "almahu_1"],
            "at Stabi the other one is not held"
        );
    }

    /// The one rank behind both the sort and [`Status::summarize`]: for every pair,
    /// summarising picks the better-ranked status. Kept as a test because the two uses
    /// live in different modules and only this pins the meaning of the number.
    #[test]
    fn the_sort_rank_agrees_with_summarize() {
        let all = [
            Status::Available,
            Status::Reference,
            Status::Unavailable,
            Status::PossiblyAvailable,
            Status::Unknown,
        ];
        for a in all {
            for b in all {
                let better = if a.rank() >= b.rank() { a } else { b };
                assert_eq!(
                    Status::summarize([a, b]),
                    better,
                    "summarize disagrees with the sort rank for {a:?} / {b:?}"
                );
            }
        }
    }

    /// The union catalogue really does deliver one record twice inside a single response.
    #[test]
    fn dedup_keeps_the_first_of_each_repeat() {
        let records = vec![
            record("almahu_1", "One", Some(2001)),
            record("almahu_1", "One", Some(2001)),
            record("almahu_2", "Two", Some(2002)),
            record("almahu_1", "One", Some(2001)),
        ];
        let (kept, dropped) = dedup(records);
        let ids: Vec<&str> = kept.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2"]);
        assert_eq!(dropped, 2);
    }

    /// Order is relevance and must survive: dedup keeps the first sighting, never the last.
    #[test]
    fn dedup_preserves_the_catalogues_order() {
        let records = vec![
            record("almahu_3", "Three", None),
            record("almahu_1", "One", None),
            record("almahu_3", "Three", None),
            record("almahu_2", "Two", None),
        ];
        let (kept, dropped) = dedup(records);
        let ids: Vec<&str> = kept.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_3", "almahu_1", "almahu_2"]);
        assert_eq!(dropped, 1);
    }

    /// A window without repeats must not be reported as if it had any.
    #[test]
    fn dedup_reports_nothing_when_there_is_nothing_to_drop() {
        let (result, _) = result();
        let before = result.records.len();
        let (kept, dropped) = dedup(result.records);
        assert_eq!(kept.len(), before);
        assert_eq!(dropped, 0);
    }

    /// With a filter the window is one anchored block holding several pages of matches,
    /// and the offset is the only thing that reaches the ones past the first `limit`.
    #[test]
    fn a_filtered_cut_walks_the_matches_inside_the_window() {
        let limit = Limit::new(1).expect("1 is in range");
        let page = |number: u32| {
            let cut = PageCut::plan(limit, Page::new(number).expect("a page"), true);
            let (result, _) = result();
            take_page(result.records, cut)
                .iter()
                .map(|record| record.id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(page(1), ["almahu_1"]);
        assert_eq!(page(2), ["almahu_2"]);
        assert_eq!(page(3), ["almafu_3"]);
        assert!(page(4).is_empty(), "past the last match, not wrapped round");
    }

    /// Without a filter the window already *is* the page — `--page` moved `startRecord` —
    /// so cutting must not skip a second time.
    #[test]
    fn an_unfiltered_cut_never_skips() {
        let limit = Limit::new(2).expect("2 is in range");
        for number in [1, 2, 7] {
            let cut = PageCut::plan(limit, Page::new(number).expect("a page"), false);
            assert_eq!(cut.offset, 0, "page {number} would page twice");
        }
    }

    #[test]
    fn take_page_keeps_the_first_records_of_the_window() {
        let (result, _) = result();
        let cut = PageCut::plan(Limit::new(2).expect("2 is in range"), Page::FIRST, false);
        let taken = take_page(result.records.clone(), cut);
        let ids: Vec<&str> = taken.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2"]);
    }

    /// The window is already the right page — `take_page` never skips as well.
    #[test]
    fn take_page_never_skips_and_never_pads() {
        let (result, _) = result();
        let cut = PageCut::plan(Limit::new(50).expect("50 is in range"), Page::FIRST, false);
        assert_eq!(take_page(result.records, cut).len(), 3);
        assert!(take_page(Vec::new(), cut).is_empty());
    }

    #[test]
    fn mark_mine_marks_exactly_the_locations() {
        let (result, locations) = result();
        let mut records = result.records;
        mark_mine(&mut records, &locations[..1]);
        assert!(records[0].holdings[0].mine, "DE-11 is --at HU");
        assert!(!records[0].holdings[1].mine, "DE-1 is not");
    }

    /// Calling it again with other locations unmarks the earlier ones.
    #[test]
    fn mark_mine_is_absolute_not_additive() {
        let (result, locations) = result();
        let mut records = result.records;
        mark_mine(&mut records, &locations);
        assert!(records[0].holdings.iter().all(|h| h.mine));
        mark_mine(&mut records, &locations[1..]);
        assert!(!records[0].holdings[0].mine);
        assert!(records[0].holdings[1].mine);
        mark_mine(&mut records, &[]);
        assert!(records[0].holdings.iter().all(|h| !h.mine));
    }

    /// A branch shares `DE-609` with the whole network, so the ISIL is what marks it.
    #[test]
    fn a_branch_marks_its_networks_holding() {
        let mut agb = record("voebb_SAK1", "Der Vorleser", Some(1997));
        agb.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![item("Belletristik", Some("SIG00036"), Status::Available)],
        )];
        let mut records = vec![agb];
        mark_mine(&mut records, &[branch("AGB", "SIG00036")]);
        assert!(records[0].holdings[0].mine);
    }

    #[test]
    fn a_location_summarises_only_its_own_copies() {
        let (result, locations) = result();
        // HU has one available and one on loan copy: available.
        assert_eq!(
            location_status(&result.records[0], &locations[0]),
            Status::Available
        );
        // Stabi has one reference copy of the same record.
        assert_eq!(
            location_status(&result.records[0], &locations[1]),
            Status::Reference
        );
    }

    /// Not held there is "nothing was said", never "on loan".
    #[test]
    fn a_location_that_holds_nothing_is_unknown_not_unavailable() {
        let (result, locations) = result();
        assert_eq!(
            location_status(&result.records[2], &locations[0]),
            Status::Unknown
        );
    }

    /// Availability may not have been fetched; then the library-level light stands.
    #[test]
    fn a_location_without_copies_falls_back_to_the_library_light() {
        let mut lonely = record("almahu_7", "Ohne Exemplare", None);
        lonely.holdings = vec![holding("DE-11", Status::Reference, Vec::new())];
        assert_eq!(
            location_status(&lonely, &institution("HU", "DE-11")),
            Status::Reference
        );
    }

    /// The branch narrows the copies — but only where the branch id is actually there.
    #[test]
    fn a_branch_narrows_to_its_own_copies_when_the_ids_are_present() {
        let mut agb = record("voebb_SAK1", "Der Vorleser", Some(1997));
        agb.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![
                item("Belletristik", Some("SIG00036"), Status::Unavailable),
                item("Pankow", Some("SIG00099"), Status::Available),
            ],
        )];
        assert_eq!(
            location_status(&agb, &branch("AGB", "SIG00036")),
            Status::Unavailable
        );
    }

    /// `items[].branch` comes from a link that need not be there. Without it the branch
    /// degrades to the plain ISIL match — never to "no copies".
    #[test]
    fn a_branch_without_item_ids_degrades_to_the_isil_match() {
        let mut agb = record("voebb_SAK1", "Der Vorleser", Some(1997));
        agb.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![item("Belletristik", None, Status::Available)],
        )];
        assert_eq!(
            location_status(&agb, &branch("AGB", "SIG00036")),
            Status::Available
        );
    }

    #[test]
    fn a_record_summarises_over_every_library() {
        let (result, _) = result();
        assert_eq!(record_status(&result.records[0]), Status::Available);
        assert_eq!(record_status(&result.records[1]), Status::Unavailable);
    }

    /// 4.9 % of records carry no `924` at all; that is "not stated", not "held nowhere".
    #[test]
    fn a_record_without_holdings_is_unknown() {
        let (result, _) = result();
        assert_eq!(record_status(&result.records[2]), Status::Unknown);
    }

    /// Two VÖBB branches, one record each, as one voebb search per location produces
    /// them: one shared `DE-609` holding, the copies naming the branch they stand in, and
    /// `at[].records` stating which search returned which record.
    fn two_branches() -> (SearchResult, Vec<Location>) {
        let mut agb = record("voebb_SAK1", "Der Vorleser", Some(1997));
        agb.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![item("Belletristik", Some("SIG00036"), Status::Available)],
        )];
        let mut bstb = record("voebb_SAK2", "Der Vorleser", Some(2012));
        bstb.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![item("Erwachsene", Some("SIG00021"), Status::Unavailable)],
        )];

        let locations = vec![branch("AGB", "SIG00036"), branch("BSTB", "SIG00021")];
        let (mut result, _) = result();
        result.at = locations
            .iter()
            .zip([&["voebb_SAK1"][..], &["voebb_SAK2"][..]])
            .map(|(location, members)| AtBlock {
                key: location.key.clone(),
                given: location.key.clone(),
                isil: location.isil.clone(),
                branch: location.branch.as_ref().map(|b| b.kobvid.clone()),
                engine: Engine::Voebb,
                total: Some(1),
                records: ids(members),
                refused: None,
            })
            .collect();
        result.records = vec![agb, bstb];
        result.engines = vec![Engine::Voebb];
        result.total = None;
        (result, locations)
    }

    #[test]
    fn without_locations_there_is_one_flat_block() {
        let (result, _) = result();
        let blocks = blocks(&result, &[]);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].location.is_none());
        assert_eq!(blocks[0].total, Some(774));
        assert_eq!(blocks[0].records.len(), 3);
        // The flat list shows no copy lines, and the marker is the record's own light.
        assert!(blocks[0].records.iter().all(|r| r.items.is_empty()));
        assert_eq!(blocks[0].records[0].status, Status::Available);
        assert_eq!(blocks[0].records[2].status, Status::Unknown);
    }

    /// One block per location, in the user's order, with that location's own total.
    #[test]
    fn blocks_follow_the_users_order_and_carry_their_own_totals() {
        let (result, locations) = result();
        let blocks = blocks(&result, &locations);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].location.map(|l| l.key.as_str()), Some("HU"));
        assert_eq!(blocks[0].total, Some(6));
        assert_eq!(blocks[1].location.map(|l| l.key.as_str()), Some("STABI"));
        assert_eq!(blocks[1].total, None, "a total the engine could not state");
    }

    /// A record held in two places is shown in both blocks — each block is complete for
    /// its location, and that is deliberate.
    #[test]
    fn a_record_appears_in_every_block_that_holds_it() {
        let (result, locations) = result();
        let blocks = blocks(&result, &locations);
        let hu: Vec<&str> = blocks[0]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        let stabi: Vec<&str> = blocks[1]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        assert_eq!(hu, ["almahu_1", "almahu_2"]);
        assert_eq!(stabi, ["almahu_1"]);
    }

    /// Each block shows the copies of *its* location and the light that summarises them.
    #[test]
    fn a_block_carries_only_its_own_copies() {
        let (result, locations) = result();
        let blocks = blocks(&result, &locations);
        let hu = &blocks[0].records[0];
        assert_eq!(hu.status, Status::Available);
        let locations_shown: Vec<&str> = hu
            .items
            .iter()
            .filter_map(|i| i.location.as_deref())
            .collect();
        assert_eq!(locations_shown, ["Grimm-Zentrum", "ZwB Germanistik"]);

        let stabi = &blocks[1].records[0];
        assert_eq!(stabi.status, Status::Reference);
        let locations_shown: Vec<&str> = stabi
            .items
            .iter()
            .filter_map(|i| i.location.as_deref())
            .collect();
        assert_eq!(locations_shown, ["Haus Potsdamer Straße"]);
    }

    /// A location without hits keeps its block: a missing block is indistinguishable
    /// from a forgotten one.
    #[test]
    fn a_location_without_hits_still_gets_a_block() {
        let (result, _) = result();
        let empty = institution("TU", "DE-83");
        let blocks = blocks(&result, std::slice::from_ref(&empty));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].location.map(|l| l.key.as_str()), Some("TU"));
        assert!(blocks[0].records.is_empty());
        assert_eq!(blocks[0].total, None);
    }

    /// Two branches of the same network, one record each: every voebb.de holding carries
    /// `DE-609`, so the ISIL cannot tell the blocks apart and `at[].records` — what the
    /// engine's search for that location returned — is what does.
    #[test]
    fn two_branches_of_one_network_do_not_show_each_others_hits() {
        let (result, locations) = two_branches();
        let blocks = blocks(&result, &locations);
        let agb: Vec<&str> = blocks[0]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        let bstb: Vec<&str> = blocks[1]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        assert_eq!(agb, ["voebb_SAK1"]);
        assert_eq!(bstb, ["voebb_SAK2"]);
    }

    /// The same, with `--no-availability`: neither record has a copy that could name a
    /// branch, and only the engine's own list keeps the two blocks apart.
    #[test]
    fn a_record_without_copies_stays_in_the_block_its_search_returned_it_for() {
        let (mut result, locations) = two_branches();
        for record in &mut result.records {
            for holding in &mut record.holdings {
                holding.items.clear();
            }
        }
        let blocks = blocks(&result, &locations);
        assert_eq!(blocks[0].records.len(), 1);
        assert_eq!(blocks[0].records[0].record.id.as_str(), "voebb_SAK1");
        assert_eq!(blocks[1].records.len(), 1);
        assert_eq!(blocks[1].records[0].record.id.as_str(), "voebb_SAK2");
        // Nothing was asked, so the library-level light stands rather than "on loan".
        assert_eq!(blocks[0].records[0].status, Status::Available);
    }

    /// A holding whose copies stand in *other* branches is not this branch's — that is a
    /// statement, not a gap, so `mine` stays false and the location holds nothing.
    #[test]
    fn a_branch_does_not_own_a_holding_whose_copies_are_elsewhere() {
        let mut elsewhere = record("voebb_SAK9", "Nur in Pankow", None);
        elsewhere.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![item("Pankow", Some("SIG00099"), Status::Available)],
        )];
        let agb = branch("AGB", "SIG00036");
        assert_eq!(location_status(&elsewhere, &agb), Status::Unknown);
        let mut records = vec![elsewhere];
        mark_mine(&mut records, std::slice::from_ref(&agb));
        assert!(!records[0].holdings[0].mine);
    }

    /// An institution is unaffected by all of it: the ISIL still decides, and a location
    /// `at[]` says nothing about still falls back to the holdings.
    #[test]
    fn an_institution_is_matched_by_its_isil_alone() {
        let (result, locations) = result();
        let stated = blocks(&result, &locations);
        assert_eq!(stated[0].records.len(), 2, "HU");
        assert_eq!(stated[1].records.len(), 1, "STABI");

        let mut without_at = result.clone();
        without_at.at.clear();
        let derived = blocks(&without_at, &locations);
        assert_eq!(derived[0].records.len(), 2, "HU, from the holdings alone");
        assert_eq!(
            derived[1].records.len(),
            1,
            "STABI, from the holdings alone"
        );
    }

    /// `at[].records` names exactly the records of the block, in the document's order:
    /// ids the page no longer shows fall out, and nothing is ever added.
    ///
    /// Adding is what the location's own search is for. A record only another location's
    /// window returned would sit past this block's `--limit` and past its paging, however
    /// plainly its holdings name the library.
    #[test]
    fn a_block_is_cut_to_the_displayed_records_and_never_grows() {
        let (mut result, _) = result();
        // As it comes back from an engine: an id that will not be displayed, and nothing
        // for the record whose Stabi holding only the availability service knows about.
        result.at[0].records = ids(&["almahu_1", "almahu_2", "almahu_gone"]);
        result.at[1].records = Vec::new();
        assign_blocks(&mut result.at, &result.records);

        let of = |index: usize| -> Vec<&str> {
            result.at[index]
                .records
                .iter()
                .map(RecordId::as_str)
                .collect()
        };
        assert_eq!(of(0), ["almahu_1", "almahu_2"], "the absent id falls out");
        assert!(
            of(1).is_empty(),
            "a holding is not a membership: the Stabi search never returned this record"
        );
    }

    /// The order follows `records[]`, not the order the engine stated the ids in — the
    /// second sort of `--sort availability` runs after the engine is done.
    #[test]
    fn a_block_is_restated_in_the_order_of_the_records() {
        let (mut result, _) = result();
        result.at[0].records = ids(&["almahu_2", "almahu_1"]);
        assign_blocks(&mut result.at, &result.records);
        let of: Vec<&str> = result.at[0].records.iter().map(RecordId::as_str).collect();
        assert_eq!(of, ["almahu_1", "almahu_2"]);
    }

    /// `--limit` is a promise per block: each location keeps its own first `limit`
    /// records, and the merged list is only what some block still shows.
    #[test]
    fn every_block_is_cut_to_the_limit_of_its_own() {
        let (mut result, _) = result();
        result.at[0].records = ids(&["almahu_1", "almahu_2"]);
        result.at[1].records = ids(&["almahu_1"]);
        let cut = PageCut::plan(Limit::new(1).expect("1 is in range"), Page::FIRST, false);

        let kept = take_page_per_location(result.records, &mut result.at, cut);

        let of = |index: usize| -> Vec<&str> {
            result.at[index]
                .records
                .iter()
                .map(RecordId::as_str)
                .collect()
        };
        assert_eq!(of(0), ["almahu_1"], "one record under HU");
        assert_eq!(of(1), ["almahu_1"], "one record under STABI, the same one");
        let shown: Vec<&str> = kept.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(
            shown,
            ["almahu_1"],
            "a record no block still shows is never asked about"
        );
    }

    /// Two locations that rank differently keep a full block each — the whole reason the
    /// cut is per block and not over the merged list.
    #[test]
    fn one_locations_hits_never_crowd_out_the_others() {
        let (mut result, _) = result();
        result.at[0].records = ids(&["almahu_1", "almahu_2"]);
        result.at[1].records = ids(&["almafu_3"]);
        let cut = PageCut::plan(Limit::new(2).expect("2 is in range"), Page::FIRST, false);

        let kept = take_page_per_location(result.records, &mut result.at, cut);

        assert_eq!(result.at[0].records.len(), 2);
        assert_eq!(result.at[1].records.len(), 1);
        assert_eq!(kept.len(), 3, "both blocks are filled, not one of them");
    }

    /// A branch is answered from the engine's list alone. The other branch's copies are
    /// on the same record page, and reading membership off them would show the record
    /// under a heading whose own window never returned it.
    #[test]
    fn a_branch_block_is_the_engines_list_and_nothing_else() {
        let (mut result, _) = two_branches();
        // Both records name both branches in their copies, as a record page does.
        for record in &mut result.records {
            record.holdings[0]
                .items
                .push(item("Erwachsene", Some("SIG00021"), Status::Available));
            record.holdings[0].items.push(item(
                "Belletristik",
                Some("SIG00036"),
                Status::Available,
            ));
        }
        assign_blocks(&mut result.at, &result.records);
        let of = |index: usize| -> Vec<&str> {
            result.at[index]
                .records
                .iter()
                .map(RecordId::as_str)
                .collect()
        };
        assert_eq!(of(0), ["voebb_SAK1"]);
        assert_eq!(of(1), ["voebb_SAK2"]);
    }

    /// The order inside a block is the order of `records`, which is the order the sort
    /// left them in — a block never re-ranks.
    #[test]
    fn a_block_keeps_the_order_of_the_result() {
        let (mut result, locations) = result();
        result.records.reverse();
        let blocks = blocks(&result, &locations);
        let hu: Vec<&str> = blocks[0]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        assert_eq!(hu, ["almahu_2", "almahu_1"]);
    }

    /// The sieve keeps a record the branch holds a copy of, drops one only its house
    /// holds, and counts the drop — the block heading has no total, so the number is the
    /// only thing that tells "not on this page" from "not held".
    #[test]
    fn the_branch_sieve_keeps_only_what_the_branch_holds() {
        let mut here = record("almahu_1", "Held at the branch", None);
        here.holdings = vec![holding(
            "DE-11",
            Status::Available,
            vec![
                item("Grimm-Zentrum", Some("HUB00028"), Status::Available),
                item("ZwB Germanistik", Some("HUB00043"), Status::Available),
            ],
        )];
        let mut elsewhere = record("almahu_2", "Held at the house only", None);
        elsewhere.holdings = vec![holding(
            "DE-11",
            Status::Available,
            vec![item("Grimm-Zentrum", Some("HUB00028"), Status::Available)],
        )];

        let locations = vec![kobv_branch("HUB00043", "DE-11", "HUB00043")];
        let mut at = vec![AtBlock {
            key: "HUB00043".to_owned(),
            given: "HUB00043".to_owned(),
            isil: Isil::new("DE-11"),
            branch: Some("HUB00043".to_owned()),
            engine: Engine::Kobv,
            total: None,
            records: ids(&["almahu_1", "almahu_2"]),
            refused: None,
        }];

        let (kept, dropped) = keep_branch_per_location(vec![here, elsewhere], &mut at, &locations);

        assert_eq!(dropped, 1);
        assert_eq!(
            kept.iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["almahu_1"]
        );
        assert_eq!(at[0].records, ids(&["almahu_1"]));
    }

    /// A holding whose copies name **no** branch is kept: a missing `bibids=` link is
    /// "not stated", and answering it as an absence is the one mistake this tool must not
    /// make. An institution block is untouched by the same call.
    #[test]
    fn the_branch_sieve_keeps_copies_that_name_no_branch() {
        let mut unstated = record("almahu_1", "Online, no branch link", None);
        unstated.holdings = vec![holding(
            "DE-11",
            Status::Available,
            vec![item("Online-Zugriff", None, Status::Available)],
        )];

        let locations = vec![
            kobv_branch("HUB00043", "DE-11", "HUB00043"),
            institution("HU", "DE-11"),
        ];
        let mut at = vec![
            AtBlock {
                key: "HUB00043".to_owned(),
                given: "HUB00043".to_owned(),
                isil: Isil::new("DE-11"),
                branch: Some("HUB00043".to_owned()),
                engine: Engine::Kobv,
                total: None,
                records: ids(&["almahu_1"]),
                refused: None,
            },
            AtBlock {
                key: "HU".to_owned(),
                given: "HU".to_owned(),
                isil: Isil::new("DE-11"),
                branch: None,
                engine: Engine::Kobv,
                total: Some(9),
                records: ids(&["almahu_1"]),
                refused: None,
            },
        ];

        let (kept, dropped) = keep_branch_per_location(vec![unstated], &mut at, &locations);

        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 1);
        assert_eq!(at[0].records, ids(&["almahu_1"]));
        assert_eq!(
            at[1].records,
            ids(&["almahu_1"]),
            "the house block is untouched"
        );
    }

    /// A VÖBB branch is not sieved here: its house facet filtered upstream, and its
    /// records carry no copies at this point, so re-deciding it would only lose them.
    #[test]
    fn the_branch_sieve_leaves_a_voebb_block_alone() {
        let record = record("voebb_1", "Der Vorleser", None);
        let locations = vec![branch("AGB", "SIG00036")];
        let mut at = vec![AtBlock {
            key: "AGB".to_owned(),
            given: "AGB".to_owned(),
            isil: Isil::new("DE-609"),
            branch: Some("SIG00036".to_owned()),
            engine: Engine::Voebb,
            total: Some(35),
            records: ids(&["voebb_1"]),
            refused: None,
        }];

        let (kept, dropped) = keep_branch_per_location(vec![record], &mut at, &locations);

        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 1);
        assert_eq!(at[0].records, ids(&["voebb_1"]));
    }

    /// §1.10: `Ö` is U+00F6 and `z` is U+007A, so a codepoint comparison puts every
    /// German name behind Z — in a tool for German libraries that reads as a broken sort.
    #[test]
    fn umlauts_sort_where_a_german_reader_looks_for_them() {
        let mut records = vec![
            with_author(record("almahu_1", "Eins", None), "Zander, Anna"),
            with_author(record("almahu_2", "Zwei", None), "Öhler, Bert"),
            with_author(record("almahu_3", "Drei", None), "Adler, Cara"),
        ];
        sort(&mut records, SortKey::Author, None);
        let authors: Vec<&str> = records
            .iter()
            .map(|record| record.authors[0].name.as_str())
            .collect();
        assert_eq!(authors, ["Adler, Cara", "Öhler, Bert", "Zander, Anna"]);
    }

    /// The folding is in the key alone: `ß` sorts as `ss` and `Über` as `ueber`, and both
    /// are still printed the way the catalogue holds them.
    #[test]
    fn the_folding_never_reaches_the_displayed_text() {
        let mut records = vec![
            record("almahu_1", "Suzuki", None),
            record("almahu_2", "Süß", None),
            record("almahu_3", "Ueberall", None),
            record("almahu_4", "Über allem", None),
        ];
        sort(&mut records, SortKey::Title, None);
        let titles: Vec<&str> = records.iter().map(|r| r.title.as_str()).collect();
        // suess < suzuki, and "ueber allem" < "ueberall" because the space sorts first.
        assert_eq!(titles, ["Süß", "Suzuki", "Über allem", "Ueberall"]);
    }

    /// An anchored window holds several pages' worth of matches, and the cut is the only
    /// thing that reaches past the first `limit` of them. The two pages have to partition
    /// the block — no record twice, none unreachable.
    #[test]
    fn an_anchored_cut_partitions_the_window() {
        let block: Vec<Record> = (0..12)
            .map(|number| {
                record(
                    &format!("almahu_{number:02}"),
                    &format!("Titel {number}"),
                    None,
                )
            })
            .collect();
        let limit = Limit::new(5).expect("5 is in range");
        let page = |number: u32| {
            take_page(
                block.clone(),
                PageCut::plan(
                    limit,
                    crate::model::Page::new(number).expect("a page"),
                    true,
                ),
            )
        };
        let ids = |records: Vec<Record>| -> Vec<String> {
            records
                .into_iter()
                .map(|r| r.id.as_str().to_owned())
                .collect()
        };
        assert_eq!(
            ids(page(1)),
            [
                "almahu_00",
                "almahu_01",
                "almahu_02",
                "almahu_03",
                "almahu_04"
            ]
        );
        assert_eq!(
            ids(page(2)),
            [
                "almahu_05",
                "almahu_06",
                "almahu_07",
                "almahu_08",
                "almahu_09"
            ]
        );
        assert_eq!(ids(page(3)), ["almahu_10", "almahu_11"]);
    }

    /// §1.8: a count is a statement, and it must not be made about copies nobody made a
    /// statement about. Two copies of unknown status are not "0 of 2 available".
    #[test]
    fn copies_nobody_judged_are_in_neither_number() {
        let unknown = [
            item("Akademiebibliothek", None, Status::Unknown),
            item("Akademiebibliothek", None, Status::Unknown),
        ];
        assert_eq!(available_count(unknown.iter()), None);

        let black = [item("Freihand", None, Status::PossiblyAvailable)];
        assert_eq!(available_count(black.iter()), None);

        let mixed = [
            item("Freihand", None, Status::Available),
            item("Magazin", None, Status::Unavailable),
            item("Lesesaal", None, Status::Reference),
            item("Nirgends", None, Status::Unknown),
        ];
        assert_eq!(
            available_count(mixed.iter()),
            Some(AvailableCount {
                available: 1,
                known: 3
            })
        );
    }

    /// §1.1, the heaviest finding of the round: `show <id> --at <branch>` used to pick
    /// whole holdings and then print every copy of the house, so the AGB's copy being out
    /// disappeared behind three copies that are in. Membership and copies are one
    /// question.
    #[test]
    fn show_narrows_a_holding_to_the_branch_the_user_named() {
        let mut record = record("voebb_SAK1", "Der Vorleser", Some(2002));
        record.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![
                item("Marzahn-Hellersdorf", Some("SIG00120"), Status::Available),
                item(
                    "Mitte: Hansabibliothek",
                    Some("SIG00044"),
                    Status::Available,
                ),
                item(
                    "ZLB: Amerika-Gedenkbibliothek",
                    Some("SIG00036"),
                    Status::Unavailable,
                ),
            ],
        )];
        let locations = vec![branch("AGB", "SIG00036")];

        let split = show_holdings(&record, &locations);
        assert_eq!(split.mine.len(), 1);
        assert!(split.others.is_empty());
        let mine = &split.mine[0];
        assert_eq!(mine.index, 0);
        assert_eq!(mine.locations, vec![&locations[0]]);
        assert_eq!(mine.items.len(), 1, "only the AGB copy");
        assert_eq!(mine.items[0].branch.as_deref(), Some("SIG00036"));
        // The light follows the narrowed copies, not the network's summary.
        assert_eq!(mine.status, Status::Unavailable);
        assert_eq!(
            available_count(mine.items.iter().copied()),
            Some(AvailableCount {
                available: 0,
                known: 1
            })
        );
        // And the same narrowing as the search path: one answer, one rule.
        assert_eq!(location_status(&record, &locations[0]), Status::Unavailable);
    }

    /// Two branches of one house claim the same holding, and both of their copies have to
    /// show: dropping the second location would hide copies without a word.
    #[test]
    fn two_branches_of_one_house_share_a_holding_and_both_sets_of_copies() {
        let mut record = record("almahu_1", "Eschweiler", Some(1990));
        record.holdings = vec![holding(
            "DE-11",
            Status::Available,
            vec![
                item("Grimm-Zentrum", Some("HUB00028"), Status::Available),
                item("Germanistik", Some("HUB00043"), Status::Available),
                item("Theologie", Some("HUB00099"), Status::Unavailable),
            ],
        )];
        let locations = vec![
            kobv_branch("DE-11-105", "DE-11", "HUB00043"),
            kobv_branch("DE-11-035", "DE-11", "HUB00028"),
        ];
        let split = show_holdings(&record, &locations);
        assert_eq!(split.mine.len(), 1, "one holding, claimed twice");
        let mine = &split.mine[0];
        assert_eq!(mine.locations.len(), 2);
        // Both branches' copies, in the holding's own order, and not the third one.
        let branches: Vec<&str> = mine
            .items
            .iter()
            .filter_map(|item| item.branch.as_deref())
            .collect();
        assert_eq!(branches, ["HUB00028", "HUB00043"]);
    }

    /// A holding no location claims is not narrowed away — it is the `also at:` line.
    #[test]
    fn holdings_outside_at_are_kept_apart_rather_than_dropped() {
        let mut record = record("almahu_1", "Der Prozess", Some(1953));
        record.holdings = vec![
            holding(
                "DE-11",
                Status::Available,
                vec![item("Grimm", None, Status::Available)],
            ),
            holding(
                "DE-1",
                Status::Reference,
                vec![item("Magazin", None, Status::Reference)],
            ),
        ];
        let locations = vec![institution("HU", "DE-11")];
        let split = show_holdings(&record, &locations);
        assert_eq!(split.mine.len(), 1);
        assert_eq!(split.mine[0].index, 0);
        assert_eq!(split.others, vec![1]);
    }

    /// Without `--at` nothing is narrowed and nothing is set aside: every holding is
    /// shown with every copy, which is what `show` alone has always printed.
    #[test]
    fn without_at_every_holding_is_shown_whole() {
        let mut record = record("almahu_1", "Der Prozess", Some(1953));
        record.holdings = vec![
            holding(
                "DE-11",
                Status::Available,
                vec![item("Grimm", None, Status::Available)],
            ),
            holding(
                "DE-1",
                Status::Reference,
                vec![item("Magazin", None, Status::Reference)],
            ),
        ];
        let split = show_holdings(&record, &[]);
        assert_eq!(split.mine.len(), 2);
        assert!(split.others.is_empty());
        assert!(split.mine.iter().all(|entry| entry.locations.is_empty()));
        assert_eq!(split.mine[0].items.len(), 1);
    }

    /// §1.1 (JSON): the `at[]` of a `show` answers per location, so the obvious pipeline
    /// reads the branch's own light instead of the whole network's summary.
    #[test]
    fn show_at_states_the_status_of_each_location() {
        let mut record = record("voebb_SAK1", "Der Vorleser", Some(2002));
        record.holdings = vec![holding(
            "DE-609",
            Status::Available,
            vec![
                item(
                    "ZLB: Amerika-Gedenkbibliothek",
                    Some("SIG00036"),
                    Status::Unavailable,
                ),
                item(
                    "Mitte: Hansabibliothek",
                    Some("SIG00044"),
                    Status::Available,
                ),
            ],
        )];
        let locations = vec![branch("AGB", "SIG00036"), branch("BSTB", "SIG00021")];
        let at = show_at(Some(&record), &locations);

        assert_eq!(at.len(), 2);
        assert_eq!(at[0].key, "AGB");
        assert_eq!(at[0].given, "AGB");
        assert_eq!(at[0].branch.as_deref(), Some("SIG00036"));
        assert_eq!(at[0].status, Status::Unavailable);
        // A branch that holds no copy says "nothing known", never "not available".
        assert_eq!(at[1].status, Status::Unknown);
    }

    /// No record at all is still an answer per location, and it is `unknown`: the
    /// question was asked and nothing is known, which is not the same as the location
    /// having been dropped from the document.
    #[test]
    fn show_at_answers_unknown_when_there_is_no_record() {
        let at = show_at(None, &[institution("HU", "DE-11")]);
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].status, Status::Unknown);
    }
}
