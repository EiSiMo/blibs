//! The portal's availability answer: JSON with an HTML fragment inside it.
//!
//! Two traps live here, and both produce a *wrong* result rather than a failure when got
//! wrong.
//!
//! **The HTML fragment carries no ISIL.** Item groups are matched against the JSON's
//! `isilAvailability` **by key order**, which is why `serde_json` is built with
//! `preserve_order` — and why this module additionally reads that member through its own
//! ordered visitor rather than trusting a Cargo feature to stay set. The matching is
//! three-stage: equal lengths means positional; otherwise by the library's portal name;
//! and whatever is left is kept **without** an ISIL. A group is never discarded.
//!
//! **The shelf table has more columns for newspapers than for books.** Columns are mapped
//! by their `<th>` text, never by position. `tr.avail-item.extra` rows count like any
//! other, and `data-more` is read as a counter-check: a mismatch is
//! [`crate::error::UnexpectedError::CountMismatch`], not a shortened list.
//!
//! Two smaller ones, each with the fixture that proves it:
//!
//! - **Empty cells are the normal case, not a parser failure.** `no_items.json` has two
//!   copies with no shelfmark at all and the placeholder `Library` where a branch name
//!   would be; `public.json` has a location cell that is empty and another that is a bare
//!   URL. None of that may read as "this library holds nothing".
//! - **The branch is read from the `bibids=` link, never from the cell text.** The text is
//!   a placeholder (`Library`, `Bibliothek`, `'`) in almost half of all rows, and matching
//!   on it hits nothing for 59 of 67 distinct texts (`plan/scraping.md` §B.5.1).

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};
use serde::de::{MapAccess, Visitor};

use crate::error::{Error, UnexpectedError};
use crate::libraries;
use crate::model::{Holding, Isil, Item, Note, Status, note_kinds};

/// What names this document for the reader of an error message.
const DOCUMENT: &str = "availability fragment";

/// Cell texts that are placeholders rather than information. The portal writes one of
/// these wherever it has no branch to name; carrying them through would put the word
/// "Library" on a shelf line.
const PLACEHOLDERS: &[&str] = &["Library", "Bibliothek", "'"];

/// The parsed availability response for one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityResponse {
    /// The overall traffic light for the record.
    pub overall: Status,
    /// The service's own `hasAvailability` flag. `false` is a note, not an error, and
    /// not an empty item list either.
    pub has_availability: bool,
    /// Per-ISIL traffic lights **in the order the JSON listed them**. The order is the
    /// only thing that connects them to the item groups.
    pub by_isil: Vec<(Isil, Status)>,
    /// Item groups from the HTML fragment, in document order.
    pub groups: Vec<ItemGroup>,
    /// The service's free-text availability line, when it supplies one.
    pub avail_text: Option<String>,
    /// What the response itself could not state cleanly — an unrecognised traffic-light
    /// colour, a `hasAvailability` of `false`. Collected here rather than returned
    /// separately so that [`parse`] is testable on its own; [`merge`] passes them on.
    pub notes: Vec<Note>,
}

/// One library's block of copies in the HTML fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemGroup {
    /// The library name as the portal writes it — the fallback key when the positional
    /// match does not apply.
    pub portal_name: String,
    /// The copies.
    pub items: Vec<Item>,
    /// What `data-more` announced beyond the rows present, if anything.
    pub announced_extra: Option<usize>,
}

/// Parse the JSON envelope, including its embedded HTML fragment.
///
/// A body that is not JSON is [`crate::error::UnexpectedError::MalformedJson`]; a body
/// that is JSON but has lost `data`, its `html` or its `isilAvailability` is
/// [`crate::error::UnexpectedError::MissingElement`], because those three are what the
/// answer *is* — silently returning an empty status list instead would be
/// indistinguishable from "nothing is available".
///
/// An unrecognised traffic-light colour is neither: it becomes [`Status::Unknown`] plus a
/// note, so a new colour in the portal degrades one status instead of failing the search.
pub fn parse(json: &str) -> Result<AvailabilityResponse, Error> {
    let envelope: Envelope =
        serde_json::from_str(json).map_err(|error| UnexpectedError::MalformedJson {
            context: "availability response".to_string(),
            detail: error.to_string(),
        })?;
    let data = envelope.data.ok_or_else(|| missing("the `data` member"))?;
    let html = data
        .html
        .ok_or_else(|| missing("the `html` member of `data`"))?;
    let isil_availability = data
        .isil_availability
        .ok_or_else(|| missing("the `isilAvailability` member of `data`"))?;

    let mut notes = Vec::new();
    let overall = colour_status(
        data.overall_availability.as_deref(),
        "overallAvailability",
        &mut notes,
    );
    let by_isil = isil_availability
        .0
        .into_iter()
        .map(|(isil, entry)| {
            let field = format!("isilAvailability[{isil}]");
            let status = colour_status(entry.color.as_deref(), &field, &mut notes);
            (Isil::new(&isil), status)
        })
        .collect();

    let has_availability = data.has_availability.unwrap_or(false);
    if !has_availability {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds no information for this record — \
             the copies below come from the catalogue and may carry no status",
        ));
    }

    let groups = parse_fragment(&html)?;
    if groups
        .iter()
        .flat_map(|group| &group.items)
        .any(|item| item.status == Status::Unknown)
    {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_UNKNOWN_STATUS,
            "some copies came back without a traffic light blibs recognises; \
             their status is reported as unknown",
        ));
    }

    Ok(AvailabilityResponse {
        overall,
        has_availability,
        by_isil,
        groups,
        avail_text: non_empty(data.avail_text.unwrap_or_default().trim()),
        notes,
    })
}

/// Parse just the HTML fragment.
///
/// A missing table is [`crate::error::UnexpectedError::MissingSelector`] naming the
/// selector — never an empty list of shelfmarks, which would read as "this book has no
/// shelfmark". So is a header row without a `Library` or `Availability` column: those two
/// are what a row *means*, and guessing their position is how a four-column parser
/// silently mangles the five-column newspaper table.
///
/// Rows that the portal collapses behind a "+5" button (`tr.avail-item.extra`,
/// `newspaper.json`) are read like any other; `data-more` is compared against them and a
/// disagreement is [`crate::error::UnexpectedError::CountMismatch`].
pub fn parse_fragment(html: &str) -> Result<Vec<ItemGroup>, Error> {
    let selectors = selectors();
    let document = Html::parse_fragment(html);
    let table = document
        .select(&selectors.table)
        .next()
        .ok_or_else(|| missing_selector("table.result-availability-items"))?;
    let columns = Columns::from_header(table)?;

    let groups = table
        .select(&selectors.group)
        .map(|group| item_group(group, &columns))
        .collect::<Result<Vec<_>, Error>>()?;

    // Counter-check against a restructured table: every copy row must sit in a group, or
    // the groups would silently hold fewer copies than the table shows.
    let in_table = table.select(&selectors.row).count();
    let in_groups: usize = groups.iter().map(|group| group.items.len()).sum();
    if in_table != in_groups {
        return Err(UnexpectedError::CountMismatch {
            context: format!("{DOCUMENT}: copy rows outside tbody.avail-group"),
            expected: in_table,
            found: in_groups,
        }
        .into());
    }

    Ok(groups)
}

/// Merge a response into a record's holdings.
///
/// Returns notes for everything that could not be matched cleanly — an unmatched group,
/// a `hasAvailability: false`, a group left without an ISIL. Never drops a group and
/// never invents an ISIL for one.
///
/// Three stages, in the order `plan/scraping.md` §B.5.1 settles them:
///
/// 1. **Positional.** Equal numbers of ISILs and groups means the *n*-th group belongs to
///    the *n*-th ISIL — measured to hold in 43 of 43 responses. Wherever the library list
///    also knows the group's portal name, the two answers are compared; a disagreement is
///    a note, never a silent choice between them.
/// 2. **By portal name**, when the lengths differ. That path is itself a note: the
///    assumption the whole matching rests on has broken.
/// 3. **Without an ISIL.** A group whose name is in neither is still shown, under the
///    portal's own name, because a stale mapping table must never make a shelf disappear.
///
/// An ISIL that the availability service knows and the MARC `924` fields do not gets a
/// holding of its own: 4.9 % of records state no holdings at all, and dropping the answer
/// would leave those records looking as though nobody held them.
pub fn merge(response: &AvailabilityResponse, holdings: &mut Vec<Holding>) -> Vec<Note> {
    let mut notes = response.notes.clone();

    // Traffic lights first, so that a library the service knows appears even when the
    // fragment carries no copy row for it.
    for (isil, status) in &response.by_isil {
        holding_for(holdings, isil).summary = *status;
    }

    for (group, isil) in
        response
            .groups
            .iter()
            .zip(assign(&response.by_isil, &response.groups, &mut notes))
    {
        if let Some(isil) = isil {
            // The traffic light from `isilAvailability` outranks the one the copies add
            // up to; only a library the JSON never mentioned is summarised from its rows.
            let stated = response
                .by_isil
                .iter()
                .any(|(candidate, _)| *candidate == isil);
            let holding = holding_for(holdings, &isil);
            holding.items.extend(group.items.iter().cloned());
            if !stated {
                holding.summary = summary_of(&group.items);
            }
        } else {
            notes.push(Note::new(
                note_kinds::HOLDING_WITHOUT_ISIL,
                format!(
                    "the availability service listed copies under {:?}, which is not in \
                     the library list — they are shown without a library code",
                    group.portal_name
                ),
            ));
            holdings.push(Holding {
                isil: None,
                alias: None,
                library: group.portal_name.clone(),
                short_name: None,
                local_id: None,
                mine: false,
                summary: summary_of(&group.items),
                items: group.items.clone(),
            });
        }
    }

    notes
}

/// Decide which ISIL each group belongs to. See [`merge`] for the three stages.
fn assign(
    by_isil: &[(Isil, Status)],
    groups: &[ItemGroup],
    notes: &mut Vec<Note>,
) -> Vec<Option<Isil>> {
    if by_isil.len() != groups.len() {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_MATCHED_BY_NAME,
            format!(
                "the availability service listed {} libraries and {} blocks of copies; \
                 the blocks were matched by library name instead of by position",
                by_isil.len(),
                groups.len()
            ),
        ));
        return groups
            .iter()
            .map(|group| portal_isil(&group.portal_name))
            .collect();
    }

    for (position, (group, (isil, _))) in groups.iter().zip(by_isil).enumerate() {
        if let Some(by_name) = portal_isil(&group.portal_name)
            && by_name != *isil
        {
            notes.push(Note::new(
                note_kinds::AVAILABILITY_MATCH_CONFLICT,
                format!(
                    "the block of copies at position {} belongs to {isil} by order and to \
                     {by_name} by name ({:?}); it was assigned by order",
                    position + 1,
                    group.portal_name
                ),
            ));
        }
    }

    by_isil.iter().map(|(isil, _)| Some(isil.clone())).collect()
}

/// The holding for this ISIL, created from the library list if the record did not state
/// it. Never returns a holding for a *different* library: the comparison is exact,
/// because attribute `1044` is case-sensitive.
fn holding_for<'h>(holdings: &'h mut Vec<Holding>, isil: &Isil) -> &'h mut Holding {
    if let Some(index) = holdings
        .iter()
        .position(|holding| holding.isil.as_ref() == Some(isil))
    {
        return &mut holdings[index];
    }
    holdings.push(new_holding(isil));
    holdings
        .last_mut()
        .expect("a holding was just pushed, so the vector is not empty")
}

/// A holding for a library the availability service named and the record did not.
fn new_holding(isil: &Isil) -> Holding {
    let library = libraries::by_isil(isil);
    Holding {
        isil: Some(isil.clone()),
        alias: library
            .and_then(|library| library.alias())
            .map(str::to_string),
        library: libraries::display_name(isil),
        short_name: library.map(|library| library.short_name.clone()),
        local_id: None,
        mine: false,
        summary: Status::Unknown,
        items: Vec::new(),
    }
}

/// The ISIL the library list gives this portal name, if it knows it.
fn portal_isil(portal_name: &str) -> Option<Isil> {
    libraries::by_portal_name(portal_name).map(|library| Isil::new(&library.isil))
}

/// The traffic light a set of copies adds up to.
fn summary_of(items: &[Item]) -> Status {
    Status::summarize(items.iter().map(|item| item.status))
}

/// One `<tbody class="avail-group">`: its name, its copies and its `data-more` count.
fn item_group(group: ElementRef<'_>, columns: &Columns) -> Result<ItemGroup, Error> {
    let selectors = selectors();
    let portal_name = group
        .attr("data-name")
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut items = Vec::new();
    let mut collapsed = 0usize;
    for row in group.select(&selectors.row) {
        if row
            .value()
            .has_class("extra", scraper::CaseSensitivity::AsciiCaseInsensitive)
        {
            collapsed += 1;
        }
        items.push(item(row, columns));
    }

    // The button announces how many rows are collapsed. When it is there, it is the only
    // counter-check against a silently shortened list; when it is not, there is nothing
    // to compare and the rows still all got read.
    let announced_extra = group
        .select(&selectors.more_button)
        .next()
        .and_then(|button| button.attr("data-more"))
        .and_then(|value| value.trim().parse::<usize>().ok());
    if let Some(announced) = announced_extra
        && announced != collapsed
    {
        return Err(UnexpectedError::CountMismatch {
            context: format!("{DOCUMENT}: collapsed copies of {portal_name:?}"),
            expected: announced,
            found: collapsed,
        }
        .into());
    }

    Ok(ItemGroup {
        portal_name,
        items,
        announced_extra,
    })
}

/// One `<tr class="avail-item">`. Every cell may legitimately be empty, so nothing here
/// fails; what is absent becomes `None` and the row stays.
fn item(row: ElementRef<'_>, columns: &Columns) -> Item {
    let cells: Vec<ElementRef<'_>> = row.select(&selectors().cell).collect();
    let location_cell = columns.location.and_then(|index| cells.get(index)).copied();
    let (branch, branch_name) = location_cell.map_or((None, None), branch_of);

    Item {
        location: location_cell
            .map(|cell| collapse(&text_of(cell)))
            .filter(|text| !is_placeholder(text)),
        branch,
        branch_name,
        call_number: columns
            .call_number
            .and_then(|index| cells.get(index))
            .and_then(|cell| non_empty(&collapse(&text_of(*cell)))),
        volume: columns
            .volume
            .and_then(|index| cells.get(index))
            .and_then(|cell| non_empty(&collapse(&text_of(*cell)))),
        status: cells
            .get(columns.availability)
            .map_or(Status::Unknown, |cell| status_of(*cell)),
        // Only voebb.de states one; the portal's fragment never does.
        order_option: None,
    }
}

/// The branch id and name of a location cell.
///
/// The id comes from `bibids=` in the link, never from the text: the text is a
/// placeholder in 131 of 293 measured rows. A cell without such a link keeps its text and
/// loses only the branch.
fn branch_of(cell: ElementRef<'_>) -> (Option<String>, Option<String>) {
    let Some(link) = cell.select(&selectors().link).find(|link| {
        link.attr("href")
            .is_some_and(|href| href.contains("bibids="))
    }) else {
        return (None, None);
    };
    let branch = link
        .attr("href")
        .and_then(|href| href.split_once("bibids="))
        .map(|(_, rest)| {
            rest.split(['|', '&'])
                .next()
                .unwrap_or(rest)
                .trim()
                .to_string()
        })
        .filter(|id| !id.is_empty());
    let name = collapse(&text_of(link));
    (branch, (!is_placeholder(&name)).then_some(name))
}

/// The traffic light of an availability cell, from the `availability-status-<colour>`
/// class on its icon. The words next to it are the portal's own translation and are not
/// read — the colour is the vocabulary.
fn status_of(cell: ElementRef<'_>) -> Status {
    cell.select(&selectors().status_icon)
        .next()
        .and_then(|icon| icon.attr("class"))
        .and_then(|classes| {
            classes
                .split_whitespace()
                .find_map(|class| class.strip_prefix("availability-status-"))
                .and_then(Status::from_color)
        })
        .unwrap_or(Status::Unknown)
}

/// Which column holds what, read from the header row.
///
/// `Library` and `Availability` are required: without them a row has neither an owner nor
/// a status, and the four- versus five-column difference between books and newspapers
/// means their positions cannot be assumed. Everything else is optional, and an
/// unrecognised heading is ignored rather than shifting the ones after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    /// Where the copy stands.
    location: Option<usize>,
    /// Its shelfmark.
    call_number: Option<usize>,
    /// Its traffic light. Required — a row without one says nothing.
    availability: usize,
    /// `Volume/Issue/Year`, present only for serials (`newspaper.json`).
    volume: Option<usize>,
}

impl Columns {
    /// Map the header cells of the table onto column indices.
    fn from_header(table: ElementRef<'_>) -> Result<Self, Error> {
        let mut library = None;
        let mut location = None;
        let mut call_number = None;
        let mut availability = None;
        let mut volume = None;

        for (index, cell) in table.select(&selectors().header_cell).enumerate() {
            match collapse(&text_of(cell)).to_ascii_lowercase().as_str() {
                "library" | "bibliothek" => library = Some(index),
                "location" | "standort" => location = Some(index),
                "call number" | "signatur" => call_number = Some(index),
                "availability" | "verfügbarkeit" => availability = Some(index),
                "volume/issue/year" | "band/heft/jahr" => volume = Some(index),
                _ => {}
            }
        }

        // The `Library` column is required but not kept: its content is the portal's own
        // name for the house, which `data-name` on the group already carries. Its
        // *presence* is the evidence that this is still the table this parser knows, so a
        // header without it is an error rather than a table read with shifted columns.
        if library.is_none() {
            return Err(missing_selector(
                "table.result-availability-items thead th:Library",
            ));
        }
        Ok(Self {
            location,
            call_number,
            availability: availability.ok_or_else(|| {
                missing_selector("table.result-availability-items thead th:Availability")
            })?,
            volume,
        })
    }
}

/// The `data` object of the response. Every member is optional here so that a missing one
/// becomes a named error rather than a serde message about a struct field.
#[derive(serde::Deserialize)]
struct Envelope {
    data: Option<Data>,
}

/// The members measured in `plan/scraping.md` §B.5.
#[derive(serde::Deserialize)]
struct Data {
    html: Option<String>,
    #[serde(rename = "overallAvailability")]
    overall_availability: Option<String>,
    #[serde(rename = "hasAvailability")]
    has_availability: Option<bool>,
    #[serde(rename = "isilAvailability")]
    isil_availability: Option<OrderedIsils>,
    #[serde(rename = "availText")]
    avail_text: Option<String>,
}

/// One library's entry in `isilAvailability`.
#[derive(serde::Deserialize)]
struct IsilEntry {
    color: Option<String>,
}

/// `isilAvailability` as an ordered list.
///
/// The order of these keys is the only thing that links a traffic light to a block of
/// copies, so it is read through a `visit_map` that appends. A map type would leave the
/// order to `serde_json`'s `preserve_order` feature — which is set, but correctness must
/// not depend on a line in `Cargo.toml` staying there.
struct OrderedIsils(Vec<(String, IsilEntry)>);

impl<'de> serde::Deserialize<'de> for OrderedIsils {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(OrderedIsilsVisitor)
    }
}

/// Appends every key in document order. See [`OrderedIsils`].
struct OrderedIsilsVisitor;

impl<'de> Visitor<'de> for OrderedIsilsVisitor {
    type Value = OrderedIsils;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a map of ISIL to availability")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some(entry) = map.next_entry::<String, IsilEntry>()? {
            entries.push(entry);
        }
        Ok(OrderedIsils(entries))
    }
}

/// The compiled selectors. Parsed once; every one of them is a literal in this file, so a
/// parse failure would be a typo in this source and nothing a user or a service can cause.
struct Selectors {
    table: Selector,
    header_cell: Selector,
    group: Selector,
    row: Selector,
    cell: Selector,
    more_button: Selector,
    link: Selector,
    status_icon: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        table: compile("table.result-availability-items"),
        header_cell: compile("thead th"),
        group: compile("tbody.avail-group"),
        row: compile("tr.avail-item"),
        cell: compile("td"),
        more_button: compile("button[data-more]"),
        link: compile("a[href]"),
        status_icon: compile("i[class*='availability-status-']"),
    })
}

/// Compile one selector literal.
fn compile(css: &str) -> Selector {
    Selector::parse(css).expect("every selector in this module is a literal and must parse")
}

/// All text below an element, with whitespace collapsed by the caller.
fn text_of(element: ElementRef<'_>) -> String {
    element.text().collect()
}

/// Collapse runs of whitespace and trim. The portal indents its cells over several lines,
/// so the raw text of a location cell carries newlines in the middle of a sentence.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `None` for an empty string, so that "the cell was blank" and "there is no such column"
/// stay the same thing in the output.
fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether this cell text says nothing. Empty counts, and so do the portal's own
/// placeholders (see [`PLACEHOLDERS`]).
fn is_placeholder(text: &str) -> bool {
    text.is_empty() || PLACEHOLDERS.contains(&text)
}

/// A traffic-light colour, or [`Status::Unknown`] plus a note saying why.
fn colour_status(colour: Option<&str>, field: &str, notes: &mut Vec<Note>) -> Status {
    let Some(colour) = colour else {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_UNKNOWN_STATUS,
            format!("the availability service sent no colour in `{field}`"),
        ));
        return Status::Unknown;
    };
    Status::from_color(colour).unwrap_or_else(|| {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_UNKNOWN_STATUS,
            format!(
                "the availability service used the colour {colour:?} in `{field}`, \
                 which blibs does not know; the status is reported as unknown"
            ),
        ));
        Status::Unknown
    })
}

/// A missing-structure error for a JSON member.
fn missing(what: &str) -> Error {
    UnexpectedError::MissingElement {
        what: what.to_string(),
        context: "availability response".to_string(),
    }
    .into()
}

/// A missing-selector error naming what stopped matching.
fn missing_selector(selector: &str) -> Error {
    UnexpectedError::MissingSelector {
        selector: selector.to_string(),
        document: DOCUMENT.to_string(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIXED: &str = include_str!("../../../../tests/fixtures/kobv/availability/mixed.json");
    const NEWSPAPER: &str =
        include_str!("../../../../tests/fixtures/kobv/availability/newspaper.json");
    const NO_ITEMS: &str =
        include_str!("../../../../tests/fixtures/kobv/availability/no_items.json");
    const REFERENCE: &str =
        include_str!("../../../../tests/fixtures/kobv/availability/reference.json");
    const PUBLIC: &str = include_str!("../../../../tests/fixtures/kobv/availability/public.json");
    const JOURNAL: &str = include_str!("../../../../tests/fixtures/kobv/availability/journal.json");

    fn parsed(json: &str) -> AvailabilityResponse {
        match parse(json) {
            Ok(response) => response,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    /// The HTML fragment of a fixture, for the manipulated-markup tests.
    fn fragment(json: &str) -> String {
        let value: serde_json::Value =
            serde_json::from_str(json).expect("fixture must be valid JSON");
        value["data"]["html"]
            .as_str()
            .expect("fixture must carry data.html")
            .to_string()
    }

    fn isils(response: &AvailabilityResponse) -> Vec<&str> {
        response
            .by_isil
            .iter()
            .map(|(isil, _)| isil.as_str())
            .collect()
    }

    fn portal_names(response: &AvailabilityResponse) -> Vec<&str> {
        response
            .groups
            .iter()
            .map(|group| group.portal_name.as_str())
            .collect()
    }

    /// `mixed.json`: the key order of `isilAvailability` is not the order the ISILs were
    /// asked for, and it is what pairs a traffic light with a block of copies.
    #[test]
    fn the_isil_order_is_the_json_key_order() {
        let response = parsed(MIXED);
        assert_eq!(
            isils(&response),
            vec!["DE-1", "DE-11", "DE-188", "DE-83", "DE-B1533", "DE-B170"]
        );
        assert_eq!(
            portal_names(&response),
            vec![
                "Stabi Berlin",
                "HU Berlin",
                "FU Berlin",
                "TU Berlin",
                "Alice Salomon HS",
                "UdK Berlin"
            ]
        );
        assert_eq!(response.overall, Status::Available);
        assert!(response.has_availability);
        assert_eq!(response.avail_text.as_deref(), Some("Verfügbar"));
        assert!(response.notes.is_empty());
    }

    /// The ordered visitor must not depend on `serde_json`'s `preserve_order` feature:
    /// the parsed order is compared against the order the keys appear in the raw text.
    #[test]
    fn the_ordered_visitor_reproduces_the_raw_key_order() {
        let response = parsed(MIXED);
        let raw = MIXED
            .split_once("\"isilAvailability\":{")
            .map(|(_, rest)| rest)
            .expect("fixture must carry isilAvailability");
        let mut positions: Vec<(usize, &str)> = isils(&response)
            .into_iter()
            .map(|isil| {
                let needle = format!("\"{isil}\":");
                (
                    raw.find(&needle)
                        .unwrap_or_else(|| panic!("{isil} must occur in the raw JSON")),
                    isil,
                )
            })
            .collect();
        let parsed_order: Vec<&str> = positions.iter().map(|(_, isil)| *isil).collect();
        positions.sort_by_key(|(offset, _)| *offset);
        let raw_order: Vec<&str> = positions.iter().map(|(_, isil)| *isil).collect();
        assert_eq!(parsed_order, raw_order);
    }

    /// The measured claim of `plan/scraping.md` §B.5.1: matching by position and matching
    /// by portal name give the same answer. If this ever fails, the assumption behind
    /// stage 1 is gone.
    #[test]
    fn positional_and_name_based_matching_agree_on_every_fixture() {
        for json in [MIXED, NEWSPAPER, NO_ITEMS, REFERENCE, PUBLIC, JOURNAL] {
            let response = parsed(json);
            assert_eq!(
                response.by_isil.len(),
                response.groups.len(),
                "the fixtures all pair one block of copies per library"
            );
            for (group, (isil, _)) in response.groups.iter().zip(&response.by_isil) {
                if let Some(by_name) = portal_isil(&group.portal_name) {
                    assert_eq!(
                        &by_name, isil,
                        "{:?} is {isil} by position and {by_name} by name",
                        group.portal_name
                    );
                }
            }
        }
    }

    /// `mixed.json`: a full four-column row — branch id from the link, branch name from
    /// its text, the rest of the cell kept as free text after it.
    #[test]
    fn a_four_column_row_carries_branch_shelfmark_and_status() {
        let response = parsed(MIXED);
        let humboldt = &response.groups[1];
        assert_eq!(humboldt.portal_name, "HU Berlin");
        assert_eq!(humboldt.announced_extra, None);
        assert_eq!(humboldt.items.len(), 1);

        let item = &humboldt.items[0];
        assert_eq!(
            item.location.as_deref(),
            Some("ZB Grimm-Zentrum, 3. OG / Bereich B - Freihandbestand")
        );
        assert_eq!(item.branch.as_deref(), Some("HUB00028"));
        assert_eq!(item.branch_name.as_deref(), Some("ZB Grimm-Zentrum"));
        assert_eq!(item.call_number.as_deref(), Some("DX 4061 S326"));
        assert_eq!(item.volume, None);
        assert_eq!(item.status, Status::Available);
        assert_eq!(item.order_option, None);

        // Yellow is reference, not "available" — measured, and a green/yellow mix inside
        // one response is normal.
        assert_eq!(response.groups[2].items[0].status, Status::Reference);
    }

    /// `newspaper.json`: five columns instead of four, and five of the eight copies are
    /// collapsed behind a `+5` button. A parser that skips `tr.avail-item.extra` loses the
    /// majority of the copies without saying so.
    #[test]
    fn the_five_column_newspaper_table_keeps_its_collapsed_rows() {
        let response = parsed(NEWSPAPER);
        assert_eq!(response.groups.len(), 1);
        let group = &response.groups[0];
        assert_eq!(group.portal_name, "Stabi Berlin");
        assert_eq!(group.announced_extra, Some(5));
        assert_eq!(group.items.len(), 8);

        let first = &group.items[0];
        assert_eq!(first.call_number.as_deref(), Some("Fremdsignatur M 485"));
        assert_eq!(first.volume.as_deref(), Some("1914,1.Sept. - 1934,30.Apr."));
        assert_eq!(first.branch.as_deref(), Some("BIB000000001"));
        // The link text is the placeholder `'`, which is not a branch name.
        assert_eq!(first.branch_name, None);
        assert_eq!(first.location, None);

        let statuses: Vec<Status> = group.items.iter().map(|item| item.status).collect();
        assert_eq!(
            statuses,
            vec![
                Status::Available,
                Status::Available,
                Status::Available,
                Status::Reference,
                Status::Reference,
                Status::Reference,
                Status::Reference,
                Status::Reference,
            ]
        );
        // The third copy is the one with a real branch name.
        assert_eq!(
            group.items[2].branch_name.as_deref(),
            Some("Unter den Linden")
        );
    }

    /// `no_items.json`: two copies with no shelfmark and the placeholder `Library` where
    /// a branch would be. The rows exist and must be kept — the documentation's claim
    /// that such a library "has no rows" is wrong.
    #[test]
    fn copies_without_a_shelfmark_are_still_copies() {
        let response = parsed(NO_ITEMS);
        assert_eq!(response.groups.len(), 1);
        let group = &response.groups[0];
        assert_eq!(group.portal_name, "Kammergericht");
        assert_eq!(group.items.len(), 2);
        for item in &group.items {
            assert_eq!(item.call_number, None);
            assert_eq!(
                item.location, None,
                "`Library` is a placeholder, not a place"
            );
            assert_eq!(item.branch_name, None);
            assert_eq!(item.branch.as_deref(), Some("SIG00057"));
            assert_eq!(item.status, Status::Reference);
        }
        assert_eq!(response.overall, Status::Reference);
        assert!(response.notes.is_empty());
    }

    /// `reference.json`: yellow means in-library use only, never "available".
    #[test]
    fn yellow_is_reference() {
        let response = parsed(REFERENCE);
        assert_eq!(isils(&response), vec!["DE-B171"]);
        assert_eq!(response.by_isil[0].1, Status::Reference);
        assert_eq!(response.groups[0].items[0].status, Status::Reference);
    }

    /// `public.json`: black is "possibly available" — the public libraries' normal answer
    /// — and a location cell may be empty or hold a bare URL instead of a place.
    #[test]
    fn black_is_possibly_available_and_a_location_may_be_a_url() {
        let response = parsed(PUBLIC);
        assert_eq!(response.overall, Status::PossiblyAvailable);
        for (_, status) in &response.by_isil {
            assert_eq!(*status, Status::PossiblyAvailable);
        }

        let empty_cell = &response.groups[0].items[0];
        assert_eq!(empty_cell.location, None);
        assert_eq!(empty_cell.branch, None);
        assert_eq!(empty_cell.branch_name, None);

        let url_cell = &response.groups[2].items[0];
        let url_location = url_cell
            .location
            .as_deref()
            .expect("the third group's cell holds a URL");
        assert!(url_location.starts_with("https://elibrary.utb.de/"));
        assert!(url_location.ends_with("(Zugriff für angemeldete Bibliotheksnutzer)"));
        assert_eq!(url_cell.branch, None);
    }

    /// `journal.json`: a call-number cell holding a single space is empty, not a shelfmark
    /// of `" "`.
    #[test]
    fn a_whitespace_only_shelfmark_is_none() {
        let response = parsed(JOURNAL);
        assert_eq!(response.groups.len(), 3);
        assert_eq!(response.groups[2].items[0].call_number, None);
        assert_eq!(response.by_isil[1].1, Status::PossiblyAvailable);
    }

    /// A renamed table is the change that must be loudest of all: without it the parser
    /// would report every book as having no shelfmark anywhere.
    #[test]
    fn a_missing_table_names_the_selector() {
        let html = fragment(MIXED).replace("result-availability-items", "result-items");
        let error = parse_fragment(&html).expect_err("the table must be required");
        assert_eq!(error.kind(), "missing_selector");
        assert!(
            error
                .to_string()
                .contains("table.result-availability-items"),
            "the message must name the selector: {error}"
        );
    }

    /// The same for the two columns a row cannot do without.
    #[test]
    fn a_missing_availability_column_names_the_selector() {
        let html = fragment(MIXED).replace(">Availability</th>", ">Ausleihstatus</th>");
        let error = parse_fragment(&html).expect_err("the status column must be required");
        assert_eq!(error.kind(), "missing_selector");
        assert!(error.to_string().contains("Availability"));
    }

    /// An unknown heading is ignored; the columns after it keep their meaning.
    #[test]
    fn an_unknown_column_heading_does_not_shift_the_others() {
        let html = fragment(NEWSPAPER).replace(">Volume/Issue/Year</th>", ">Enumeration</th>");
        let groups = parse_fragment(&html).expect("an unknown heading is not fatal");
        assert_eq!(groups[0].items.len(), 8);
        assert_eq!(
            groups[0].items[0].call_number.as_deref(),
            Some("Fremdsignatur M 485")
        );
        assert_eq!(groups[0].items[0].volume, None);
        assert_eq!(groups[0].items[0].status, Status::Available);
    }

    /// `data-more` is the counter-check against a silently shortened copy list.
    #[test]
    fn a_wrong_data_more_is_a_count_mismatch() {
        let html = fragment(NEWSPAPER).replace("data-more=\"5\"", "data-more=\"6\"");
        let error = parse_fragment(&html).expect_err("a disagreeing counter is an error");
        assert_eq!(error.kind(), "count_mismatch");
        assert!(error.to_string().contains("expected 6"));
        assert!(error.to_string().contains("found 5"));
    }

    /// A copy row that has escaped its group would otherwise vanish from the output.
    #[test]
    fn a_row_outside_a_group_is_a_count_mismatch() {
        let html = fragment(MIXED).replace("class=\"avail-group\"", "class=\"avail-flat\"");
        let error = parse_fragment(&html).expect_err("ungrouped rows must be noticed");
        assert_eq!(error.kind(), "count_mismatch");
    }

    /// A colour nobody has seen before degrades one status and says so; it never fails
    /// the search and never silently reads as "available".
    #[test]
    fn an_unknown_colour_is_unknown_plus_a_note() {
        let json = MIXED.replace("\"color\":\"yellow\"", "\"color\":\"purple\"");
        let response = parsed(&json);
        assert_eq!(response.by_isil[2].1, Status::Unknown);
        let note = response
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::AVAILABILITY_UNKNOWN_STATUS)
            .expect("an unknown colour must be noted");
        assert!(note.message.contains("purple"));
        assert!(note.message.contains("DE-188"));
    }

    /// The same for the icon inside the fragment.
    #[test]
    fn an_unknown_icon_colour_is_unknown_plus_a_note() {
        let json = MIXED.replace("availability-status-green", "availability-status-teal");
        let response = parsed(&json);
        assert_eq!(response.groups[0].items[0].status, Status::Unknown);
        assert!(
            response
                .notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_UNKNOWN_STATUS)
        );
    }

    /// `hasAvailability: false` means the service does not know the library — not that
    /// nothing is available. It is a note, and the copies stay.
    #[test]
    fn has_availability_false_is_a_note_and_keeps_the_copies() {
        let json = MIXED.replace("\"hasAvailability\":true", "\"hasAvailability\":false");
        let response = parsed(&json);
        assert!(!response.has_availability);
        assert_eq!(response.groups.len(), 6);
        assert!(
            response
                .notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_NOT_STATED)
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_malformed_json() {
        let error = parse("<html>502</html>").expect_err("HTML is not JSON");
        assert_eq!(error.kind(), "malformed_json");
    }

    #[test]
    fn a_response_without_data_names_the_member() {
        let error = parse("{}").expect_err("`data` is required");
        assert_eq!(error.kind(), "missing_structure");
        assert!(error.to_string().contains("data"));
    }

    #[test]
    fn a_response_without_isil_availability_names_the_member() {
        let error =
            parse(r#"{"data":{"html":"<p/>"}}"#).expect_err("`isilAvailability` is required");
        assert_eq!(error.kind(), "missing_structure");
        assert!(error.to_string().contains("isilAvailability"));
    }

    fn holding(isil: &str) -> Holding {
        Holding {
            isil: Some(Isil::new(isil)),
            alias: None,
            library: isil.to_string(),
            short_name: None,
            local_id: Some("BV041830956".to_string()),
            mine: false,
            summary: Status::Unknown,
            items: Vec::new(),
        }
    }

    /// Stage 1 end to end: the copies of the *n*-th block land on the *n*-th ISIL's
    /// holding, and the traffic light comes from `isilAvailability`.
    #[test]
    fn merge_pairs_blocks_with_holdings_by_position() {
        let response = parsed(MIXED);
        let mut holdings = vec![
            holding("DE-11"),
            holding("DE-1"),
            holding("DE-188"),
            holding("DE-83"),
            holding("DE-B1533"),
            holding("DE-B170"),
        ];
        let notes = merge(&response, &mut holdings);
        assert!(notes.is_empty(), "the fixture matches cleanly: {notes:?}");
        assert_eq!(holdings.len(), 6);

        let humboldt = holdings
            .iter()
            .find(|holding| holding.isil == Some(Isil::new("DE-11")))
            .expect("DE-11 stays");
        assert_eq!(humboldt.summary, Status::Available);
        assert_eq!(humboldt.items.len(), 1);
        assert_eq!(humboldt.items[0].branch.as_deref(), Some("HUB00028"));
        assert_eq!(humboldt.local_id.as_deref(), Some("BV041830956"));

        let free = holdings
            .iter()
            .find(|holding| holding.isil == Some(Isil::new("DE-188")))
            .expect("DE-188 stays");
        assert_eq!(free.summary, Status::Reference);
        assert_eq!(free.items[0].call_number.as_deref(), Some("DX 4061 S326"));
    }

    /// A library the availability service knows and the record does not gets a holding of
    /// its own rather than being dropped.
    #[test]
    fn merge_adds_a_holding_the_record_never_stated() {
        let response = parsed(MIXED);
        let mut holdings = vec![holding("DE-11")];
        merge(&response, &mut holdings);
        assert_eq!(holdings.len(), 6);
        let stabi = holdings
            .iter()
            .find(|holding| holding.isil == Some(Isil::new("DE-1")))
            .expect("DE-1 is added");
        assert_eq!(stabi.items.len(), 1);
        assert_eq!(stabi.summary, Status::Available);
        // The display name comes from the library list, never from the portal fragment.
        assert!(!stabi.library.is_empty());
        assert_eq!(stabi.local_id, None);
    }

    /// Stage 3: a block whose name nobody knows keeps its copies under the portal's own
    /// name, and says so in a note.
    #[test]
    fn merge_keeps_a_block_it_cannot_place() {
        let mut response = parsed(MIXED);
        // Break both stages for one block: an extra block makes the lengths differ, and
        // its name is in no library list.
        response.groups.push(ItemGroup {
            portal_name: "Bibliothek von Babel".to_string(),
            items: vec![Item {
                location: Some("Hexagon 6".to_string()),
                branch: None,
                branch_name: None,
                call_number: Some("MCV".to_string()),
                volume: None,
                status: Status::Reference,
                order_option: None,
            }],
            announced_extra: None,
        });

        let mut holdings = vec![holding("DE-11")];
        let notes = merge(&response, &mut holdings);

        assert!(
            notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_MATCHED_BY_NAME)
        );
        assert!(
            notes
                .iter()
                .any(|note| note.kind == note_kinds::HOLDING_WITHOUT_ISIL)
        );
        let orphan = holdings
            .iter()
            .find(|holding| holding.isil.is_none())
            .expect("the block is kept");
        assert_eq!(orphan.library, "Bibliothek von Babel");
        assert_eq!(orphan.items.len(), 1);
        assert_eq!(orphan.summary, Status::Reference);
    }

    /// When the two ways of matching disagree, that is reported — never decided silently.
    #[test]
    fn merge_notes_a_conflict_between_the_two_matches() {
        let mut response = parsed(MIXED);
        response.groups.swap(0, 1);
        let mut holdings = vec![holding("DE-1"), holding("DE-11")];
        let notes = merge(&response, &mut holdings);
        assert!(
            notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_MATCH_CONFLICT),
            "a swapped block must be noticed: {notes:?}"
        );
    }
}
