//! Client-side selection: filter, sort, page, mark, group.
//!
//! Everything here sees **only the fetched records**, and that limitation is part of the
//! contract rather than something to paper over. SRU cannot sort at all and has no index
//! for material type or language, so `--sort`, `--format` and `--language` act on the
//! window — and the output must never imply otherwise. `--at` is the exception: it is an
//! upstream filter, so location results are complete and pageable.
//!
//! [`blocks`] derives the human grouping from a [`SearchResult`]. It is a *view*, not a
//! second schema: the JSON stays record-centric and a record appears once, no matter how
//! many blocks show it.

use crate::model::{Format, Item, Location, Record, SearchResult, SortKey, Status};

/// The client-side filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// `--format`.
    pub format: Option<Format>,
    /// `--language`, an ISO-639-2/B code.
    pub language: Option<String>,
}

impl Filters {
    /// Whether any filter is set. Decides whether [`crate::model::FetchWindow::plan`]
    /// widens the window.
    pub fn is_active(&self) -> bool {
        self.format.is_some() || self.language.is_some()
    }
}

/// Keep the records that match every active filter.
pub fn filter(_records: Vec<Record>, _filters: &Filters) -> Vec<Record> {
    todo!("phase 2: select")
}

/// Sort in place.
///
/// [`SortKey::Availability`] is only meaningful after availability has been fetched, and
/// [`SortKey::Availability`] with a location sorts by that location's traffic light
/// rather than the record's best one — "is it in *there*" is the question being asked.
pub fn sort(_records: &mut [Record], _by: SortKey, _at: Option<&Location>) {
    todo!("phase 2: select")
}

/// Take the requested page out of the filtered, sorted records.
pub fn take_page(_records: Vec<Record>, _limit: usize, _page: u32) -> Vec<Record> {
    todo!("phase 2: select")
}

/// Set `holdings[].mine` for the `--at` locations.
pub fn mark_mine(_records: &mut [Record], _locations: &[Location]) {
    todo!("phase 2: select")
}

/// One location's block of the human output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The location this block is about. `None` for the flat list shown without `--at`.
    pub location: Option<Location>,
    /// The location's true hit count, when the engine could state one.
    pub total: Option<u64>,
    /// The records shown under it.
    pub records: Vec<BlockRecord>,
}

/// One record inside a block, reduced to what that location holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord {
    /// The record itself.
    pub record: Record,
    /// The traffic light **for this location**, not for the record overall.
    pub status: Status,
    /// The copies at this location only.
    pub items: Vec<Item>,
}

/// Group a result by location for the human renderer.
///
/// With `--at`, one block per location in the order the user gave them. Without it, a
/// single block with `location: None` and one line per record.
pub fn blocks(_result: &SearchResult) -> Vec<Block> {
    todo!("phase 2: select")
}
