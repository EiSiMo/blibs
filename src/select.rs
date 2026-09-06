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

use std::cmp::Reverse;

use crate::model::{Format, Holding, Item, Limit, Location, Record, SearchResult, SortKey, Status};

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
        SortKey::Title => records.sort_by_cached_key(|record| record.title.trim().to_lowercase()),
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
        .map(|author| author.name.trim().to_lowercase())
        .filter(|name| !name.is_empty());
    (name.is_none(), name.unwrap_or_default())
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
/// Only the first `limit` records — never an offset. The window that was fetched is
/// already the right page: [`crate::model::FetchWindow::plan`] positions `startRecord`
/// from `--page`, and it widens to 50 exactly when a client-side filter will thin the
/// window out. Skipping here as well would page twice and drop records silently.
pub fn take_page(mut records: Vec<Record>, limit: Limit) -> Vec<Record> {
    records.truncate(usize::from(limit.get()));
    records
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
/// ISIL equality, and deliberately nothing more. A VÖBB branch shares `DE-609` with the
/// whole network, but a branch location is only ever answered by the `voebb` engine,
/// which has already applied the branch facet upstream — so every holding that comes
/// back is the branch's. The copy-level narrowing that [`items_at`] does on top of this
/// is a refinement of the *display*, never a reason to drop a holding.
fn holding_is_at(holding: &Holding, location: &Location) -> bool {
    holding.isil.as_ref() == Some(&location.isil)
}

/// The copies of a holding that belong to a location.
///
/// For an institution that is all of them. For a branch, the items whose `branch` id is
/// the branch's — but only when at least one item actually carries it: `items[].branch`
/// comes from a link that need not be there, and reporting *no copies* because a link
/// was missing would be the worst possible answer. When nothing carries the id the
/// holding degrades to the plain ISIL match, which is what the branch facet already
/// guaranteed.
fn items_at<'a>(holding: &'a Holding, location: &Location) -> Vec<&'a Item> {
    let Some(branch) = location.branch.as_ref() else {
        return holding.items.iter().collect();
    };
    let of_branch: Vec<&Item> = holding
        .items
        .iter()
        .filter(|item| item.branch.as_deref() == Some(branch.kobvid.as_str()))
        .collect();
    if of_branch.is_empty() {
        holding.items.iter().collect()
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
    let records = result
        .records
        .iter()
        .filter(|record| {
            record
                .holdings
                .iter()
                .any(|holding| holding_is_at(holding, location))
        })
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
        records,
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
            order_option: None,
        }
    }

    fn institution(key: &str, isil: &str) -> Location {
        Location {
            key: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine: Engine::Kobv,
            display: format!("{key} display"),
        }
    }

    fn branch(key: &str, kobvid: &str) -> Location {
        Location {
            key: key.to_owned(),
            isil: Isil::new("DE-609"),
            branch: Some(BranchRef {
                kobvid: kobvid.to_owned(),
                name: key.to_owned(),
            }),
            engine: Engine::Voebb,
            display: format!("{key} (VÖBB)"),
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
                undelivered: 0,
            },
            engines: vec![Engine::Kobv],
            at: vec![
                AtBlock {
                    key: "HU".to_owned(),
                    isil: Isil::new("DE-11"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: Some(6),
                },
                AtBlock {
                    key: "STABI".to_owned(),
                    isil: Isil::new("DE-1"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: None,
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

    #[test]
    fn relevance_leaves_the_upstream_order_alone() {
        let (result, _) = result();
        let mut records = result.records;
        sort(&mut records, SortKey::Relevance, None);
        let ids: Vec<String> = records.iter().map(|r| r.id.as_str()).collect();
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

    #[test]
    fn title_sorts_by_the_displayed_title_casefolded() {
        let mut records = vec![
            record("almahu_1", "zeta", None),
            record("almahu_2", "Ähre", None),
            record("almahu_3", "Alpha", None),
        ];
        sort(&mut records, SortKey::Title, None);
        let titles: Vec<&str> = records.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Alpha", "zeta", "Ähre"]);
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
            let ids: Vec<String> = shuffled.iter().map(|r| r.id.as_str()).collect();
            assert_eq!(ids, ["almahu_1", "almahu_2", "almahu_3"], "{key:?}");
        }
        sort(&mut records, SortKey::Availability, None);
        let ids: Vec<String> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2", "almahu_3"]);
    }

    #[test]
    fn availability_sorts_the_best_traffic_light_first() {
        let (result, _) = result();
        let mut records = result.records;
        sort(&mut records, SortKey::Availability, None);
        let ids: Vec<String> = records.iter().map(|r| r.id.as_str()).collect();
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
        let ids: Vec<String> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["almahu_1", "almafu_2"],
            "unscoped: the best light wins"
        );

        let mut records = unsorted;
        sort(&mut records, SortKey::Availability, Some(&stabi));
        let ids: Vec<String> = records.iter().map(|r| r.id.as_str()).collect();
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

    #[test]
    fn take_page_keeps_the_first_records_of_the_window() {
        let (result, _) = result();
        let limit = Limit::new(2).expect("2 is in range");
        let taken = take_page(result.records.clone(), limit);
        let ids: Vec<String> = taken.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["almahu_1", "almahu_2"]);
    }

    /// The window is already the right page — `take_page` never skips as well.
    #[test]
    fn take_page_never_skips_and_never_pads() {
        let (result, _) = result();
        let limit = Limit::new(50).expect("50 is in range");
        assert_eq!(take_page(result.records, limit).len(), 3);
        assert!(take_page(Vec::new(), limit).is_empty());
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
        let hu: Vec<String> = blocks[0]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        let stabi: Vec<String> = blocks[1]
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

    /// The order inside a block is the order of `records`, which is the order the sort
    /// left them in — a block never re-ranks.
    #[test]
    fn a_block_keeps_the_order_of_the_result() {
        let (mut result, locations) = result();
        result.records.reverse();
        let blocks = blocks(&result, &locations);
        let hu: Vec<String> = blocks[0]
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        assert_eq!(hu, ["almahu_2", "almahu_1"]);
    }
}
