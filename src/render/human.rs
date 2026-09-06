//! Terminal output: one readable line per hit.
//!
//! Rules that the example output in `plan/cli.md` pins down character for character:
//!
//! - **With `--at`, output is grouped by location** — one block per location, one line
//!   per record, that location's copies beneath it. No flag turns this off. Without
//!   `--at` it is one flat line per hit.
//! - **The legend lists only symbols that actually occur** in this output.
//! - **A record id is never shortened**; titles are.
//! - `no holdings recorded in this record` for a record with no `924` — not an empty
//!   list, and not "held nowhere".
//! - For a serial, a line saying that holdings runs cannot be determined: the service
//!   gives one traffic light for the *title*, with no volume, and a green light would
//!   otherwise read as a statement about the year the user wants.
//! - When a client-side filter was in play, a line saying how large the window was — so
//!   that "nothing found" is never mistaken for "nothing exists".
//!
//! The columns of the examples are reproduced with **padded [`Column::Flex`] cells rather
//! than [`Column::Fixed`] ones** wherever the content is truncatable: a cell padded to the
//! example's width has exactly that natural width, so a roomy terminal gets the example's
//! layout, and a narrow one shortens the title instead of overflowing. Widths that carry
//! meaning — the marker, the year, the shelfmark, the record id — stay fixed and overflow
//! rather than lie.
//!
//! The separation between columns is the layout's gap, never the padding of a cell. A
//! shrunken column spends its padding on its own text, so a title that had to give way
//! would otherwise end up glued to the author beside it.

use std::io::{self, Write};

use crate::error::{EmptyReason, Error};
use crate::libraries::{Branch, Library};
use crate::model::{
    Format, Holding, Item, Location, Record, SearchResult, SortKey, Status, UrlKind,
};
use crate::render::Style;
use crate::render::style::label;
use crate::render::table::{Cell, Column, Layout, display_width, pad_right, truncate};
use crate::select::{self, Block, BlockRecord, holding_status};

/// Indent of a record line, in both list forms.
const RECORD_INDENT: usize = 2;
/// Indent of a copy line under a record in the grouped list.
const ITEM_INDENT: usize = 7;
/// Indent of a copy line in `show`.
const SHOW_ITEM_INDENT: usize = 6;
/// Indent of everything under a heading in `show`.
const SHOW_INDENT: usize = 2;

/// Title column with `--at`.
const GROUPED_TITLE: usize = 34;
/// Title column without `--at`, which has no copy lines to make room for.
const FLAT_TITLE: usize = 38;
/// Location column of a copy line in the grouped list.
const SEARCH_ITEM_LOCATION: usize = 33;
/// Location column of a copy line in `show`, which is indented one column less.
const SHOW_ITEM_LOCATION: usize = 39;
/// Author column of the author block in `show`.
const SHOW_AUTHOR_NAME: usize = 32;
/// Label column of every `Label  value` block. Exactly as wide as the longest label.
const FIELD_LABEL: usize = 11;
/// Status column. Wide enough for the longest wording in [`label`], so that a following
/// `order_option` never collides with it.
const STATUS_COLUMN: usize = 20;

/// How many authors `show` prints before it counts the rest.
const MAX_AUTHORS: usize = 5;
/// How many subject headings `show` prints before it counts the rest.
const MAX_SUBJECTS: usize = 6;

/// What `show` says instead of a holdings list for a record without `924` fields.
///
/// 4.9 % of records have none. This is "not stated in this record", never "held nowhere",
/// and never a silently empty list.
const NO_HOLDINGS: &str = "no holdings recorded in this record";

/// What `show` says for a serial. The availability service answers one traffic light for
/// the *title*, with no volume and no shelfmark, so a green light would otherwise read as
/// a statement about the year the user is after.
const JOURNAL_NOTE: &str = "which volumes are held cannot be determined here — the service reports one status \
     for the title, without years; check the library's own catalogue or the ZDB";

/// What `show` says when a copy is out. Due dates and holds live behind a patron login,
/// and this tool never signs in — so it says that once instead of suggesting a date.
const LOAN_NOTE: &str = "a copy on loan carries no due date here — return dates and holds \
     are only in the library's own catalogue, behind a patron login";

/// Render a search result.
///
/// With locations the output is grouped: one block per location **in the user's order**,
/// including a location that holds nothing — a missing block cannot be told apart from a
/// forgotten one. Without locations it is one flat line per hit, without copy lines.
///
/// The legend at the end lists only the symbols that actually occurred, and its wording
/// depends on the form: under a location heading `●` promises "you can borrow it there",
/// in the flat list only "somebody in the region lends it".
pub fn search(
    result: &SearchResult,
    locations: &[Location],
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    let blocks = select::blocks(result, locations);
    let scoped = !locations.is_empty();
    let mut shown = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        if scoped {
            write_grouped_block(out, block, style, &mut shown)?;
        } else {
            write_flat_block(out, result, block, style, &mut shown)?;
        }
    }
    write_footer(out, result, &shown, scoped, style)
}

/// One location's block: heading, then a line per record with that location's copies
/// beneath it.
fn write_grouped_block(
    out: &mut dyn Write,
    block: &Block<'_>,
    style: Style,
    shown: &mut Vec<Status>,
) -> io::Result<()> {
    write_line(out, &block_heading(block), 0, style.heading(), style)?;
    if block.records.is_empty() {
        return Ok(());
    }
    writeln!(out)?;

    let record_layout = grouped_record_layout();
    let item_layout = search_item_layout();
    let record_rows: Vec<Vec<Cell>> = block
        .records
        .iter()
        .map(|entry| grouped_record_row(entry, style))
        .collect();
    let item_rows: Vec<Vec<Vec<Cell>>> = block
        .records
        .iter()
        .map(|entry| {
            entry
                .items
                .iter()
                .map(|item| item_row(item, SEARCH_ITEM_LOCATION, style))
                .collect()
        })
        .collect();

    let record_widths = record_layout.widths(&record_rows, available(style, RECORD_INDENT));
    let all_items: Vec<Vec<Cell>> = item_rows.iter().flatten().cloned().collect();
    let item_widths = item_layout.widths(&all_items, available(style, ITEM_INDENT));

    for ((entry, record_row), items) in block.records.iter().zip(&record_rows).zip(&item_rows) {
        write_row(
            out,
            &record_layout,
            record_row,
            &record_widths,
            RECORD_INDENT,
            style,
        )?;
        for item in items {
            write_row(out, &item_layout, item, &item_widths, ITEM_INDENT, style)?;
        }
        shown.push(entry.status);
    }
    Ok(())
}

/// The flat list: a heading with the true total, then one numbered line per hit.
fn write_flat_block(
    out: &mut dyn Write,
    result: &SearchResult,
    block: &Block<'_>,
    style: Style,
    shown: &mut Vec<Status>,
) -> io::Result<()> {
    let count = block.records.len();
    write_line(out, &flat_heading(result, count), 0, style.heading(), style)?;
    if count == 0 {
        return Ok(());
    }
    writeln!(out)?;

    let first = first_number(result);
    let number_width = digits(first + count as u64 - 1);
    let layout = flat_record_layout(number_width);
    let rows: Vec<Vec<Cell>> = block
        .records
        .iter()
        .enumerate()
        .map(|(offset, entry)| flat_record_row(entry, first + offset as u64, number_width, style))
        .collect();
    let widths = layout.widths(&rows, available(style, RECORD_INDENT));
    for (entry, row) in block.records.iter().zip(&rows) {
        write_row(out, &layout, row, &widths, RECORD_INDENT, style)?;
        shown.push(entry.status);
    }
    Ok(())
}

/// The legend and the footnotes, each preceded by a blank line and each omitted when it
/// would be empty.
fn write_footer(
    out: &mut dyn Write,
    result: &SearchResult,
    shown: &[Status],
    scoped: bool,
    style: Style,
) -> io::Result<()> {
    if let Some(legend) = style.legend(shown, scoped) {
        writeln!(out)?;
        writeln!(out, "{legend}")?;
    }
    let notes = footer_notes(result);
    if !notes.is_empty() {
        writeln!(out)?;
        for note in notes {
            write_line(out, &format!("note: {note}"), 0, style.dim(), style)?;
        }
    }
    Ok(())
}

/// The limitations worth a footnote: what the engines reported, plus the two that follow
/// from the window itself.
///
/// The window ones exist so that a short answer is never mistaken for a complete one:
/// a client-side filter and a client-side sort both see only the fetched records, and
/// `plan/cli.md` forbids output that suggests otherwise.
fn footer_notes(result: &SearchResult) -> Vec<String> {
    let mut notes: Vec<String> = result
        .notes
        .iter()
        .map(|note| note.message.clone())
        .collect();
    let window = result.window;
    if window.after_filter < window.fetched {
        notes.push(match result.total {
            Some(total) => format!(
                "the filters saw the {} fetched records, not all {total} results",
                window.fetched
            ),
            None => format!(
                "the filters saw the {} fetched records only",
                window.fetched
            ),
        });
    }
    if let Some(total) = result.total
        && result.sort.by != SortKey::Relevance
        && total > result.shown as u64
    {
        notes.push(format!(
            "--sort ordered the {} records shown, not all {total} results",
            result.shown
        ));
    }
    notes
}

/// `HU Berlin · 6 results`, `… · showing 2` when the block shows fewer than it counted,
/// `… · no results` for a location that holds nothing.
///
/// A location whose engine could not state a total says `showing N` alone: printing the
/// number of records on this page as if it were the total would be a lie.
fn block_heading(block: &Block<'_>) -> String {
    let name = block
        .location
        .map_or("", |location| location.display.as_str());
    let shown = block.records.len();
    if shown == 0 {
        return format!("{name} · no results");
    }
    match block.total {
        Some(total) if total > shown as u64 => {
            format!("{name} · {} · showing {shown}", results(total))
        }
        Some(total) => format!("{name} · {}", results(total)),
        None => format!("{name} · showing {shown}"),
    }
}

/// `774 results for "Kafka Prozess" · showing 1-10`.
///
/// The footer always names the true total, so that a short page does not read like a
/// short result.
fn flat_heading(result: &SearchResult, shown: usize) -> String {
    let terms = &result.query.terms;
    let total = result.total.unwrap_or(shown as u64);
    let head = format!("{} for {terms:?}", results(total));
    if shown == 0 {
        return head;
    }
    let first = first_number(result);
    format!("{head} · showing {first}-{}", first + shown as u64 - 1)
}

/// The running number of the first record on this page.
///
/// `(page - 1) * limit + 1`, from the page size the user asked for rather than from the
/// number of records that happened to arrive: the reading aid has to continue across
/// pages, and a short last page would otherwise restart the count from a smaller stride.
fn first_number(result: &SearchResult) -> u64 {
    u64::from(result.page.get() - 1) * result.limit as u64 + 1
}

/// `1 result` / `774 results`.
fn results(total: u64) -> String {
    if total == 1 {
        "1 result".to_owned()
    } else {
        format!("{total} results")
    }
}

/// Decimal digits of a running number, so a page's markers stay in one column.
fn digits(number: u64) -> usize {
    number.to_string().len()
}

/// The columns of a record line under a location heading, as in `plan/cli.md`.
fn grouped_record_layout() -> Layout {
    Layout::new(vec![
        Column::Fixed(1),
        Column::Flex { min: 12 },
        Column::Fixed(18),
        Column::Fixed(5),
        Column::Last,
    ])
}

/// The columns of a record line in the flat list: a running number in front, a wider
/// title, and no copy lines to align with.
///
/// The first column is as wide as the page's largest running number plus its marker, so
/// that the markers of a page stand in one column whatever the page number.
fn flat_record_layout(number_width: usize) -> Layout {
    Layout::new(vec![
        Column::Fixed(number_width + 2),
        Column::Flex { min: 12 },
        Column::Fixed(15),
        Column::Fixed(4),
        Column::Last,
    ])
}

/// The columns of a copy line: location, shelfmark, status, and whatever `voebb` says
/// about ordering it.
///
/// The shelfmark column is [`Column::Fixed`] and its cell is never shortened: half a
/// shelfmark does not find a book. It overflows into the status column instead, which is
/// visible and honest.
fn item_layout(shelfmark_width: usize) -> Layout {
    Layout::new(vec![
        Column::Flex { min: 12 },
        Column::Fixed(shelfmark_width),
        Column::Fixed(STATUS_COLUMN),
        Column::Last,
    ])
}

/// The copy layout of the grouped list.
fn search_item_layout() -> Layout {
    item_layout(19)
}

/// The copy layout of `show`, which indents one column less and has more room.
fn show_item_layout() -> Layout {
    item_layout(16)
}

/// One record under a location heading. The marker is that **location's** traffic light,
/// not the record's overall one.
fn grouped_record_row(entry: &BlockRecord<'_>, style: Style) -> Vec<Cell> {
    let record = entry.record;
    vec![
        Cell::new(entry.status.symbol().to_string()).styled(style.status(entry.status)),
        title_cell(&record.title, GROUPED_TITLE),
        Cell::new(pad_right(&truncate(first_author(record), 18), 18)),
        Cell::new(year(record)),
        Cell::new(record.id.as_str()).whole(),
    ]
}

/// One record in the flat list. The number is a reading aid only — `blibs` is stateless,
/// so `show 3` cannot work and the id is what `show` takes.
fn flat_record_row(
    entry: &BlockRecord<'_>,
    number: u64,
    number_width: usize,
    style: Style,
) -> Vec<Cell> {
    let record = entry.record;
    let marker = format!("{number:>number_width$} {}", entry.status.symbol());
    vec![
        Cell::new(marker).styled(style.status(entry.status)).whole(),
        title_cell(&record.title, FLAT_TITLE),
        Cell::new(pad_right(&truncate(first_author(record), 15), 15)),
        Cell::new(year(record)),
        Cell::new(record.id.as_str()).whole(),
    ]
}

/// One copy: where it stands, what it is called there, and whether it is in.
fn item_row(item: &Item, location_width: usize, style: Style) -> Vec<Cell> {
    let mut cells = vec![
        Cell::new(pad_right(
            &truncate(&item_location(item), location_width),
            location_width,
        )),
        Cell::new(item.call_number.clone().unwrap_or_default())
            .styled(style.dim())
            .whole(),
        Cell::new(label(item.status, true).to_owned()).styled(style.status(item.status)),
    ];
    if let Some(order) = &item.order_option {
        cells.push(Cell::new(order.clone()).styled(style.dim()));
    }
    cells
}

/// Where a copy stands, with its volume behind it when the record is a serial.
///
/// Falls back to the branch name when the catalogue named no location, and to the volume
/// alone when it named neither — an empty location is never a reason to drop the line.
fn item_location(item: &Item) -> String {
    let place = item
        .location
        .clone()
        .or_else(|| item.branch_name.clone())
        .unwrap_or_default();
    match (&place.is_empty(), &item.volume) {
        (true, Some(volume)) => volume.clone(),
        (false, Some(volume)) => format!("{place} · {volume}"),
        _ => place,
    }
}

/// A title cell: shortened to leave two columns of air, then padded so that the column
/// has the example's width whatever is in it.
fn title_cell(title: &str, width: usize) -> Cell {
    Cell::new(pad_right(&truncate(title, width), width))
}

/// The first author's name, or nothing. Never the role and never a placeholder.
fn first_author(record: &Record) -> &str {
    record
        .authors
        .first()
        .map_or("", |author| author.name.as_str())
}

/// The year as text, empty when the record states none.
fn year(record: &Record) -> String {
    record.year.map(|year| year.to_string()).unwrap_or_default()
}

/// Render one record in full.
///
/// With locations the user's own libraries come first, in the order of `--at`, and the
/// rest are named on one `also at:` line. Without them every holding is simply listed.
pub fn show(
    record: &Record,
    locations: &[Location],
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    write_line(out, &record.title, 0, style.bold(), style)?;
    if let Some(subtitle) = &record.subtitle {
        write_line(out, subtitle, 0, style.dim(), style)?;
    }
    write_author_block(out, record, style)?;
    write_field_block(out, record, style)?;
    write_holdings(out, record, locations, style)?;
    writeln!(out)?;
    writeln!(out, "{}", record.id)
}

/// The authors, with role and GND. Capped at [`MAX_AUTHORS`] with the rest counted —
/// the JSON carries them all, and no role is ever filtered out.
fn write_author_block(out: &mut dyn Write, record: &Record, style: Style) -> io::Result<()> {
    if record.authors.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    let layout = Layout::new(vec![
        Column::Flex { min: 12 },
        Column::Fixed(12),
        Column::Last,
    ]);
    let rows: Vec<Vec<Cell>> = record
        .authors
        .iter()
        .take(MAX_AUTHORS)
        .map(|author| {
            let name = match &author.dates {
                Some(dates) => format!("{} ({dates})", author.name),
                None => author.name.clone(),
            };
            vec![
                Cell::new(pad_right(
                    &truncate(&name, SHOW_AUTHOR_NAME),
                    SHOW_AUTHOR_NAME,
                )),
                Cell::new(author.role.clone().unwrap_or_default()),
                Cell::new(
                    author
                        .gnd
                        .as_ref()
                        .map(|gnd| format!("GND {gnd}"))
                        .unwrap_or_default(),
                )
                .styled(style.dim()),
            ]
        })
        .collect();
    let widths = layout.widths(&rows, available(style, SHOW_INDENT));
    for row in &rows {
        write_row(out, &layout, row, &widths, SHOW_INDENT, style)?;
    }
    write_overflow(out, record.authors.len(), MAX_AUTHORS, style)
}

/// `and N more`, for a list the terminal cut short. Nothing when nothing was cut.
fn write_overflow(out: &mut dyn Write, total: usize, shown: usize, style: Style) -> io::Result<()> {
    if total <= shown {
        return Ok(());
    }
    let text = format!("and {} more", total - shown);
    write_line(out, &text, SHOW_INDENT, style.dim(), style)
}

/// The `Label   value` block: what the record says about itself.
fn write_field_block(out: &mut dyn Write, record: &Record, style: Style) -> io::Result<()> {
    let fields = record_fields(record);
    if fields.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    write_fields(out, &fields, style)
}

/// Write a `Label   value` block, aligning every value in one column.
fn write_fields(out: &mut dyn Write, fields: &[(String, String)], style: Style) -> io::Result<()> {
    let layout = Layout::new(vec![Column::Fixed(FIELD_LABEL), Column::Last]);
    let rows: Vec<Vec<Cell>> = fields
        .iter()
        .map(|(label, value)| {
            vec![
                Cell::new(label.clone()).styled(style.dim()).whole(),
                Cell::new(value.clone()),
            ]
        })
        .collect();
    let widths = layout.widths(&rows, available(style, SHOW_INDENT));
    for row in &rows {
        write_row(out, &layout, row, &widths, SHOW_INDENT, style)?;
    }
    Ok(())
}

/// The fields `show` prints, in order, leaving out what the record does not state.
///
/// The language is the code the record carries (`ger`), not a translated name: a mapping
/// invented here would be a claim the catalogue never made.
fn record_fields(record: &Record) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let published = join_present(
        &[
            record.publisher.clone(),
            record.place.clone(),
            record.year.map(|year| year.to_string()),
        ],
        ", ",
    );
    push_field(&mut fields, "Published", published);
    push_field(
        &mut fields,
        "Edition",
        record.edition.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "Extent",
        record.extent.clone().unwrap_or_default(),
    );
    push_field(&mut fields, "Language", record.languages.join(", "));
    push_field(&mut fields, "Format", format_label(record));
    push_field(&mut fields, "ISBN", record.isbns.join(" · "));
    push_field(&mut fields, "Subjects", subjects(record));
    for (index, url) in record.urls.iter().enumerate() {
        let label = if index == 0 { "Online" } else { "" };
        fields.push((
            label.to_owned(),
            format!("{} ({})", url.url, url_kind(url.kind)),
        ));
    }
    fields
}

/// Append a field unless its value is empty. Nothing is ever printed as an empty value.
fn push_field(fields: &mut Vec<(String, String)>, label: &str, value: String) {
    if !value.is_empty() {
        fields.push((label.to_owned(), value));
    }
}

/// The subject headings, capped at [`MAX_SUBJECTS`] with the rest counted.
fn subjects(record: &Record) -> String {
    let shown = record
        .subjects
        .iter()
        .take(MAX_SUBJECTS)
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if record.subjects.len() > MAX_SUBJECTS {
        format!(
            "{shown} · and {} more",
            record.subjects.len() - MAX_SUBJECTS
        )
    } else {
        shown
    }
}

/// Join the values that are actually there, so a missing publisher does not leave a
/// dangling comma.
fn join_present(values: &[Option<String>], separator: &str) -> String {
    values
        .iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(separator)
}

/// The material type as a word, with `online` added where the vocabulary has no
/// `e`-variant for it — `format` alone would otherwise lose that half of the record.
fn format_label(record: &Record) -> String {
    let name = match record.format {
        Format::Book => "Book",
        Format::Ebook => "E-book",
        Format::Journal => "Journal",
        Format::Ejournal => "E-journal",
        Format::Article => "Article",
        Format::Database => "Database",
        Format::Map => "Map",
        Format::Score => "Score",
        Format::Audio => "Audio",
        Format::Video => "Video",
        Format::Image => "Image",
        Format::Electronic => "Electronic",
        Format::Manuscript => "Manuscript",
        Format::Object => "Object",
        Format::Mixed => "Mixed",
        Format::Unknown => "Unknown",
    };
    let already_online = matches!(record.format, Format::Ebook | Format::Ejournal);
    if record.online && !already_online {
        format!("{name} (online)")
    } else {
        name.to_owned()
    }
}

/// What a link is, in words. Three quarters of the links are covers and tables of
/// contents, so an unlabelled URL invites exactly the wrong guess.
fn url_kind(kind: UrlKind) -> &'static str {
    match kind {
        UrlKind::Fulltext => "full text",
        UrlKind::Toc => "table of contents",
        UrlKind::Cover => "cover",
        UrlKind::Publisher => "publisher",
        UrlKind::Other => "link",
    }
}

/// The `Holdings` section: the user's libraries in the order of `--at`, then the copies
/// of each, then what could not be said, then everybody else on one line.
fn write_holdings(
    out: &mut dyn Write,
    record: &Record,
    locations: &[Location],
    style: Style,
) -> io::Result<()> {
    writeln!(out)?;
    write_line(out, "Holdings", 0, style.heading(), style)?;
    writeln!(out)?;

    if record.holdings.is_empty() {
        write_line(out, NO_HOLDINGS, SHOW_INDENT, style.dim(), style)?;
        return write_show_notes(out, record, style);
    }

    let (mine, others) = split_holdings(record, locations);
    // The copy columns are aligned across all of the shown holdings, not per house: one
    // ragged block per library would make the shelfmarks harder to compare than they are.
    let rows: Vec<Vec<Vec<Cell>>> = mine
        .iter()
        .map(|index| {
            record.holdings[*index]
                .items
                .iter()
                .map(|item| item_row(item, SHOW_ITEM_LOCATION, style))
                .collect()
        })
        .collect();
    let layout = show_item_layout();
    let all: Vec<Vec<Cell>> = rows.iter().flatten().cloned().collect();
    let widths = layout.widths(&all, available(style, SHOW_ITEM_INDENT));

    for (index, items) in mine.iter().zip(&rows) {
        write_holding_heading(out, &record.holdings[*index], style)?;
        for row in items {
            write_row(out, &layout, row, &widths, SHOW_ITEM_INDENT, style)?;
        }
    }
    write_show_notes(out, record, style)?;
    write_also_at(out, record, &others, style)
}

/// `● HU Berlin — Humboldt-Universität …`, with `2 of 3 available` behind the name when
/// the library holds more than one copy.
///
/// The count is the point: one traffic light per institution summarises several houses
/// and answers "can I go there" only by accident (`plan/usecases.md` UC-1).
fn write_holding_heading(out: &mut dyn Write, holding: &Holding, style: Style) -> io::Result<()> {
    let status = holding_status(holding);
    let mut line = format!("{} {}", style.symbol(status), library_name(holding));
    if holding.items.len() > 1 {
        let available = holding
            .items
            .iter()
            .filter(|item| item.status == Status::Available)
            .count();
        let count = format!(" · {available} of {} available", holding.items.len());
        line.push_str(&style.paint(&count, style.dim()));
    }
    writeln!(out, "{}{line}", " ".repeat(SHOW_INDENT))
}

/// `HU Berlin — Humboldt-Universität zu Berlin, Universitätsbibliothek`, or the full name
/// alone when there is no shorter one. Never `null` and never empty: an unknown ISIL
/// renders as the bare code rather than removing the holding.
fn library_name(holding: &Holding) -> String {
    let short = holding
        .short_name
        .as_deref()
        .or(holding.alias.as_deref())
        .unwrap_or_default();
    if short.is_empty() || short == holding.library {
        holding.library.clone()
    } else {
        format!("{short} — {}", holding.library)
    }
}

/// Split the holdings into the user's own, in the order of `--at`, and the rest.
///
/// Without locations everything is "mine" in record order and nothing is marked — there
/// is no user preference to sort by.
fn split_holdings(record: &Record, locations: &[Location]) -> (Vec<usize>, Vec<usize>) {
    if locations.is_empty() {
        return ((0..record.holdings.len()).collect(), Vec::new());
    }
    let mut mine: Vec<usize> = Vec::new();
    for location in locations {
        for (index, holding) in record.holdings.iter().enumerate() {
            if holding.isil.as_ref() == Some(&location.isil) && !mine.contains(&index) {
                mine.push(index);
            }
        }
    }
    let others = (0..record.holdings.len())
        .filter(|index| !mine.contains(index))
        .collect();
    (mine, others)
}

/// `also at: Stabi Berlin · TU Berlin · …` — the libraries outside `--at`, named but not
/// listed. Nothing when there are none.
fn write_also_at(
    out: &mut dyn Write,
    record: &Record,
    others: &[usize],
    style: Style,
) -> io::Result<()> {
    if others.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = others
        .iter()
        .map(|index| {
            let holding = &record.holdings[*index];
            holding
                .short_name
                .clone()
                .or_else(|| holding.alias.clone())
                .unwrap_or_else(|| holding.library.clone())
        })
        .collect();
    writeln!(out)?;
    let line = format!("also at: {}", names.join(" · "));
    write_line(out, &line, SHOW_INDENT, style.dim(), style)
}

/// The two sentences `show` owes the reader about what it cannot know: holdings runs for
/// a serial, and the due date of a copy that is out. Each is said **once**, not per copy.
fn write_show_notes(out: &mut dyn Write, record: &Record, style: Style) -> io::Result<()> {
    let mut notes: Vec<&str> = Vec::new();
    if matches!(record.format, Format::Journal | Format::Ejournal) {
        notes.push(JOURNAL_NOTE);
    }
    if record
        .holdings
        .iter()
        .flat_map(|holding| &holding.items)
        .any(|item| item.status == Status::Unavailable)
    {
        notes.push(LOAN_NOTE);
    }
    if notes.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for note in notes {
        write_line(out, note, SHOW_INDENT, style.dim(), style)?;
    }
    Ok(())
}

/// Render the library table: shorthand, ISIL, short name, full name, city.
pub fn libraries_table(rows: &[&Library], out: &mut dyn Write, style: Style) -> io::Result<()> {
    let rows: Vec<(&Library, Option<f64>)> = rows.iter().map(|library| (*library, None)).collect();
    write_library_table(out, &rows, style)
}

/// The same table with a distance column, for `libraries --near`.
pub fn libraries_near(
    rows: &[(&Library, f64)],
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    let rows: Vec<(&Library, Option<f64>)> = rows
        .iter()
        .map(|(library, distance)| (*library, Some(*distance)))
        .collect();
    write_library_table(out, &rows, style)
}

/// The library table, with or without distances.
///
/// The first three columns are at least as wide as the example in `plan/cli.md` and
/// widen only if the data demands it: a shorthand is what the user types back into
/// `--at`, so it is never shortened. The name column absorbs a narrow terminal.
///
/// One column of separation, not the usual two: the example's shorthand column ends one
/// space before the ISIL, and the widest short name in the list fills its column exactly.
fn write_library_table(
    out: &mut dyn Write,
    rows: &[(&Library, Option<f64>)],
    style: Style,
) -> io::Result<()> {
    let alias_width = column_width(
        8,
        rows.iter()
            .map(|(library, _)| library.alias().unwrap_or("")),
    );
    let isil_width = column_width(10, rows.iter().map(|(library, _)| library.isil.as_str()));
    let short_width = column_width(
        16,
        rows.iter().map(|(library, _)| library.short_name.as_str()),
    );
    let name_width = column_width(1, rows.iter().map(|(library, _)| library.name.as_str()));
    let city_width = column_width(1, rows.iter().map(|(library, _)| library.city.as_str()));
    let layout = Layout::new(vec![
        Column::Fixed(alias_width),
        Column::Fixed(isil_width),
        Column::Fixed(short_width),
        Column::Flex { min: 16 },
        Column::Fixed(city_width),
        Column::Last,
    ])
    .with_gap(1);

    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|(library, distance)| {
            let mut row = vec![
                Cell::new(library.alias().unwrap_or("").to_owned())
                    .styled(style.bold())
                    .whole(),
                Cell::new(library.isil.clone()).styled(style.dim()).whole(),
                Cell::new(library.short_name.clone()),
                Cell::new(pad_right(&library.name, name_width)),
                Cell::new(library.city.clone()),
            ];
            if let Some(distance) = distance {
                row.push(Cell::new(distance_text(*distance)).styled(style.dim()));
            }
            row
        })
        .collect();
    let widths = layout.widths(&cells, style.width());
    for row in &cells {
        write_row(out, &layout, row, &widths, 0, style)?;
    }
    Ok(())
}

/// A column at least `minimum` wide, and as wide as its widest value when that is wider.
///
/// Nothing in these columns may be shortened — a shorthand is what the user types back
/// into `--at` — so the column grows instead, and the layout's gap keeps the separation.
fn column_width<'a>(minimum: usize, values: impl Iterator<Item = &'a str>) -> usize {
    values
        .map(display_width)
        .max()
        .map_or(minimum, |widest| minimum.max(widest))
}

/// A distance, rounded to the precision the coordinates actually support.
fn distance_text(km: f64) -> String {
    format!("{km:.1} km")
}

/// Render one library in detail: what the list knows and nothing it does not.
///
/// Opening hours are deliberately absent — the compiled list would be days out of date
/// (`plan/cli.md`, `plan/libraries.md`).
pub fn library_detail(library: &Library, out: &mut dyn Write, style: Style) -> io::Result<()> {
    write_line(out, &library.name, 0, style.bold(), style)?;
    writeln!(out)?;
    let mut fields = Vec::new();
    push_field(&mut fields, "Shorthand", library.aliases.join(" · "));
    push_field(&mut fields, "ISIL", library.isil.clone());
    push_field(&mut fields, "Short name", library.short_name.clone());
    push_field(
        &mut fields,
        "Type",
        library.kind.clone().unwrap_or_default(),
    );
    push_field(&mut fields, "Address", library.address.clone());
    push_field(&mut fields, "City", library.city.clone());
    if let Some(coords) = library.coords() {
        push_field(
            &mut fields,
            "Coordinates",
            format!("{:.5}, {:.5}", coords.lat, coords.lon),
        );
    }
    push_field(
        &mut fields,
        "Phone",
        library.phone.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "Email",
        library.email.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "Website",
        library.url.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "OPAC",
        library.opac.clone().unwrap_or_default(),
    );
    if !library.branches.is_empty() {
        fields.push(("Branches".to_owned(), branch_count(&library.branches)));
        for branch in &library.branches {
            fields.push((String::new(), branch.short_name.clone()));
        }
    }
    write_fields(out, &fields, style)
}

/// `3 branches`, or `1 branch`.
fn branch_count(branches: &[Branch]) -> String {
    if branches.len() == 1 {
        "1 branch".to_owned()
    } else {
        format!("{} branches", branches.len())
    }
}

/// Say why there is nothing to show. Never a bare "no results": the reason decides what
/// the user should try next.
pub fn empty(reason: &EmptyReason, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "{}", reason.message())
}

/// Render an error and its hint to stderr.
///
/// The hint is indented under the message so that the two read as one paragraph, and it
/// is never omitted when the variant has one — "request failed" without a next step is
/// exactly what this tool must not print.
pub fn error(error: &Error, out: &mut dyn Write, style: Style) -> io::Result<()> {
    writeln!(out, "{} {error}", style.paint("error:", style.bold()))?;
    let Some(hint) = error.hint() else {
        return Ok(());
    };
    for line in hint.lines() {
        writeln!(out, "       {}", style.paint(line, style.dim()))?;
    }
    Ok(())
}

/// The columns a row may use: the terminal, minus what the indent already spent.
fn available(style: Style, indent: usize) -> usize {
    style.width().saturating_sub(indent)
}

/// Write one laid-out row at an indent, without the trailing space padding leaves behind.
fn write_row(
    out: &mut dyn Write,
    layout: &Layout,
    cells: &[Cell],
    widths: &[usize],
    indent: usize,
    style: Style,
) -> io::Result<()> {
    let line = format!(
        "{}{}",
        " ".repeat(indent),
        layout.render_row(cells, widths, style)
    );
    writeln!(out, "{}", line.trim_end())
}

/// Write one painted line at an indent.
fn write_line(
    out: &mut dyn Write,
    text: &str,
    indent: usize,
    paint: anstyle::Style,
    style: Style,
) -> io::Result<()> {
    writeln!(out, "{}{}", " ".repeat(indent), style.paint(text, paint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AtBlock, Author, AuthorKind, AvailabilityMode, BranchRef, Engine, Isil, Note, Page,
        QueryEcho, RecordId, ResourceUrl, SortScope, SortSpec, WindowInfo,
    };
    use crate::render::table::strip_ansi;

    /// The width the examples in `plan/cli.md` were written for. Wide enough that no
    /// column has to give way, which is what makes them reproducible at all.
    const WIDE: usize = 100;

    fn record(id: &str, title: &str, author: &str, year: i32) -> Record {
        Record {
            id: RecordId::parse(id).expect("the example ids carry their source prefix"),
            title: title.to_owned(),
            subtitle: None,
            authors: if author.is_empty() {
                Vec::new()
            } else {
                vec![person(author)]
            },
            year: Some(year),
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

    fn person(name: &str) -> Author {
        Author {
            name: name.to_owned(),
            kind: AuthorKind::Person,
            dates: None,
            gnd: None,
            role: None,
        }
    }

    fn holding(
        isil: &str,
        short: &str,
        library: &str,
        summary: Status,
        items: Vec<Item>,
    ) -> Holding {
        Holding {
            isil: Some(Isil::new(isil)),
            alias: None,
            library: library.to_owned(),
            short_name: Some(short.to_owned()),
            local_id: None,
            mine: false,
            summary,
            items,
        }
    }

    fn item(location: &str, call_number: &str, status: Status) -> Item {
        Item {
            location: Some(location.to_owned()),
            branch: None,
            branch_name: None,
            call_number: Some(call_number.to_owned()),
            volume: None,
            status,
            order_option: None,
        }
    }

    fn institution(key: &str, isil: &str, display: &str) -> Location {
        Location {
            key: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine: Engine::Kobv,
            display: display.to_owned(),
        }
    }

    fn branch(key: &str, isil: &str, kobvid: &str, display: &str) -> Location {
        Location {
            key: key.to_owned(),
            isil: Isil::new(isil),
            branch: Some(BranchRef {
                kobvid: kobvid.to_owned(),
                name: display.to_owned(),
            }),
            engine: Engine::Voebb,
            display: display.to_owned(),
        }
    }

    fn at(key: &str, isil: &str, total: u64, engine: Engine) -> AtBlock {
        AtBlock {
            key: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine,
            total: Some(total),
        }
    }

    fn result(terms: &str, total: Option<u64>, records: Vec<Record>) -> SearchResult {
        let shown = records.len();
        SearchResult {
            query: QueryEcho {
                terms: terms.to_owned(),
                pqf: None,
            },
            total,
            shown,
            page: Page::FIRST,
            limit: shown.max(1),
            sort: SortSpec {
                by: SortKey::Relevance,
                scope: SortScope::Fetched,
            },
            window: WindowInfo {
                fetched: shown,
                after_filter: shown,
                undelivered: 0,
            },
            engines: vec![Engine::Kobv],
            at: Vec::new(),
            availability: AvailabilityMode::Fetched,
            notes: Vec::new(),
            records,
        }
    }

    fn rendered(result: &SearchResult, locations: &[Location]) -> String {
        let mut out = Vec::new();
        search(result, locations, &mut out, Style::plain(WIDE)).expect("a vector accepts bytes");
        String::from_utf8(out).expect("the renderer writes UTF-8")
    }

    fn rendered_show(record: &Record, locations: &[Location]) -> String {
        let mut out = Vec::new();
        show(record, locations, &mut out, Style::plain(WIDE)).expect("a vector accepts bytes");
        String::from_utf8(out).expect("the renderer writes UTF-8")
    }

    /// The three-location example, built to match `plan/cli.md` § *Mit `--at`*.
    fn vorleser() -> (SearchResult, Vec<Location>) {
        let mut hu_first = record(
            "almahu_BV011234567",
            "Der Vorleser : Roman",
            "Schlink, Bernhard",
            1997,
        );
        hu_first.holdings = vec![holding(
            "DE-11",
            "HU Berlin",
            "Humboldt-Universität zu Berlin, Universitätsbibliothek",
            Status::Available,
            vec![
                item("Grimm-Zentrum, 7. OG", "GM 5000 S345 V9", Status::Available),
                item(
                    "ZwB Germanistik, UG",
                    "GM 5000 S345 V9",
                    Status::Unavailable,
                ),
            ],
        )];
        let mut hu_second = record(
            "almahu_BV019876543",
            "Bernhard Schlink, Der Vorleser",
            "Mittelberg, E.",
            2004,
        );
        hu_second.holdings = vec![holding(
            "DE-11",
            "HU Berlin",
            "Humboldt-Universität zu Berlin, Universitätsbibliothek",
            Status::Reference,
            vec![item(
                "Grimm-Zentrum, 5. OG",
                "GM 5000 S345 V9 M6",
                Status::Reference,
            )],
        )];
        let mut stabi = record(
            "almafu_BV010111222",
            "Der Vorleser : Roman",
            "Schlink, Bernhard",
            1995,
        );
        stabi.holdings = vec![holding(
            "DE-1",
            "Stabi Berlin",
            "Staatsbibliothek zu Berlin",
            Status::Available,
            vec![item(
                "Haus Potsdamer Straße",
                "1 A 234 567",
                Status::Available,
            )],
        )];
        let mut agb_first = record(
            "voebb_SAK13776205",
            "Der Vorleser",
            "Schlink, Bernhard",
            1997,
        );
        agb_first.holdings = vec![holding(
            "DE-609",
            "Berlin VÖBB/ZLB",
            "Zentral- und Landesbibliothek Berlin",
            Status::Available,
            vec![item("Belletristik", "Schlink", Status::Available)],
        )];
        let mut agb_second = record(
            "voebb_SAK14200311",
            "Der Vorleser",
            "Schlink, Bernhard",
            2012,
        );
        agb_second.holdings = vec![holding(
            "DE-609",
            "Berlin VÖBB/ZLB",
            "Zentral- und Landesbibliothek Berlin",
            Status::Unavailable,
            vec![item("Belletristik", "Schlink", Status::Unavailable)],
        )];

        let mut search_result = result(
            "Der Vorleser",
            Some(6),
            vec![hu_first, hu_second, stabi, agb_first, agb_second],
        );
        search_result.at = vec![
            at("HU", "DE-11", 6, Engine::Kobv),
            at("STABI", "DE-1", 3, Engine::Kobv),
            at("AGB", "DE-609", 35, Engine::Voebb),
        ];
        search_result.engines = vec![Engine::Kobv, Engine::Voebb];
        let locations = vec![
            institution("HU", "DE-11", "HU Berlin"),
            institution("STABI", "DE-1", "Stabi Berlin"),
            branch("AGB", "DE-609", "SIG00036", "AGB (VÖBB)"),
        ];
        (search_result, locations)
    }

    /// The example in `plan/cli.md` § *Mit `--at`*, character for character.
    ///
    /// Two deviations, both because the example abbreviates itself:
    ///
    /// 1. The `HU` and `Stabi` headings carry `· showing N`. The example lists two of
    ///    six and one of three records but omits it, which contradicts its own rule
    ///    ("Zeigt der Block weniger, steht `showing N` dabei") — and `AGB`, with the
    ///    same shape, does carry it.
    /// 2. The second copy line of the first record is one column further right in the
    ///    example than the other four; the shelfmark column is at 42 everywhere else.
    #[test]
    fn the_grouped_example_is_reproduced_character_for_character() {
        let (result, locations) = vorleser();
        let expected = "\
HU Berlin · 6 results · showing 2

  ●  Der Vorleser : Roman                Schlink, Bernhard   1997   almahu_BV011234567
       Grimm-Zentrum, 7. OG               GM 5000 S345 V9      available
       ZwB Germanistik, UG                GM 5000 S345 V9      on loan
  ◐  Bernhard Schlink, Der Vorleser      Mittelberg, E.      2004   almahu_BV019876543
       Grimm-Zentrum, 5. OG               GM 5000 S345 V9 M6   reference only

Stabi Berlin · 3 results · showing 1

  ●  Der Vorleser : Roman                Schlink, Bernhard   1995   almafu_BV010111222
       Haus Potsdamer Straße              1 A 234 567          available

AGB (VÖBB) · 35 results · showing 2

  ●  Der Vorleser                        Schlink, Bernhard   1997   voebb_SAK13776205
       Belletristik                       Schlink              available
  ○  Der Vorleser                        Schlink, Bernhard   2012   voebb_SAK14200311
       Belletristik                       Schlink              on loan

●  available      ◐  reference only      ○  on loan
";
        assert_eq!(rendered(&result, &locations), expected);
    }

    /// The flat example in `plan/cli.md` § *Ohne `--at`*.
    ///
    /// One deviation: the range is `showing 1-3`, not `showing 1-10`. The example lists
    /// three of the ten records it claims; the range states what was actually printed.
    #[test]
    fn the_flat_example_is_reproduced_character_for_character() {
        let mut first = record("almafu_BV008885798", "Der Prozess", "Kafka, Franz", 1953);
        first.holdings = vec![holding(
            "DE-11",
            "HU Berlin",
            "Humboldt-Universität zu Berlin",
            Status::Available,
            Vec::new(),
        )];
        let mut second = record(
            "b3kat_BV005550341",
            "Der Process : Roman",
            "Kafka, Franz",
            1990,
        );
        second.holdings = vec![holding(
            "DE-11",
            "HU Berlin",
            "Humboldt-Universität zu Berlin",
            Status::Reference,
            Vec::new(),
        )];
        let mut third = record(
            "almafu_BV035123456",
            "Der Prozeß : Roman",
            "Kafka, Franz",
            2008,
        );
        third.holdings = vec![holding(
            "DE-11",
            "HU Berlin",
            "Humboldt-Universität zu Berlin",
            Status::Unavailable,
            Vec::new(),
        )];
        let result = result("Kafka Prozess", Some(774), vec![first, second, third]);
        let expected = "\
774 results for \"Kafka Prozess\" · showing 1-3

  1 ●  Der Prozess                             Kafka, Franz     1953  almafu_BV008885798
  2 ◐  Der Process : Roman                     Kafka, Franz     1990  b3kat_BV005550341
  3 ○  Der Prozeß : Roman                      Kafka, Franz     2008  almafu_BV035123456

●  available somewhere      ◐  reference only      ○  currently unavailable
";
        assert_eq!(rendered(&result, &[]), expected);
    }

    /// A location that holds nothing still gets its block: a missing block cannot be
    /// told apart from a forgotten one.
    #[test]
    fn a_location_without_hits_still_gets_its_heading() {
        let (result, mut locations) = vorleser();
        locations.push(institution("TU", "DE-83", "TU Berlin"));
        let output = rendered(&result, &locations);
        assert!(output.contains("TU Berlin · no results\n"));
        assert!(!output.contains("TU Berlin · 0 results"));
    }

    /// The legend names what occurred and nothing else.
    #[test]
    fn the_legend_lists_only_the_symbols_that_occurred() {
        let (mut result, locations) = vorleser();
        // Drop everything but the two available records, so `◐` and `○` disappear.
        result.records.retain(|record| {
            record.id.as_str() == "almahu_BV011234567" || record.id.as_str() == "almafu_BV010111222"
        });
        let output = rendered(&result, &locations);
        assert!(output.contains("●  available\n"));
        assert!(!output.contains("reference only"));
        assert!(!output.contains("currently unavailable"));
    }

    /// A note is a footnote, not a line the reader has to hunt for in the list.
    #[test]
    fn notes_are_rendered_as_footnotes() {
        let (mut result, locations) = vorleser();
        result.notes = vec![Note::new(
            crate::model::note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds nothing for 1 record",
        )];
        let output = rendered(&result, &locations);
        assert!(output.ends_with("note: the availability service holds nothing for 1 record\n"));
    }

    /// A window that a client-side filter thinned out is stated, so that a short answer
    /// is never mistaken for a complete one.
    #[test]
    fn a_filtered_window_is_stated() {
        let (mut result, locations) = vorleser();
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 5,
            undelivered: 0,
        };
        let output = rendered(&result, &locations);
        assert!(output.contains("note: the filters saw the 50 fetched records, not all 6 results"));
    }

    /// Colour is decoration: strip the escapes and the coloured output is the plain one.
    #[test]
    fn colour_does_not_change_the_layout() {
        let (result, locations) = vorleser();
        let mut plain = Vec::new();
        let mut coloured = Vec::new();
        search(&result, &locations, &mut plain, Style::plain(WIDE)).expect("bytes");
        search(&result, &locations, &mut coloured, Style::new(true, WIDE)).expect("bytes");
        let plain = String::from_utf8(plain).expect("UTF-8");
        let coloured = String::from_utf8(coloured).expect("UTF-8");
        assert_ne!(plain, coloured, "colour must actually have been written");
        assert_eq!(plain, strip_ansi(&coloured));
    }

    /// A record id is the argument for `show`, so it is never shortened — even when the
    /// terminal is far too narrow for the row.
    #[test]
    fn a_narrow_terminal_shortens_the_title_and_never_the_id() {
        let (result, locations) = vorleser();
        let mut out = Vec::new();
        search(&result, &locations, &mut out, Style::plain(60)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert!(output.contains("almahu_BV011234567"));
        assert!(output.contains('…'), "the title gives way instead");
    }

    /// The record of `plan/cli.md` § `show`.
    fn prozess() -> Record {
        let mut record = record("almafu_BV008885798", "Der Prozess", "", 1953);
        record.subtitle = Some("Roman".to_owned());
        record.authors = vec![
            Author {
                name: "Kafka, Franz".to_owned(),
                kind: AuthorKind::Person,
                dates: Some("1883-1924".to_owned()),
                gnd: Some("118559230".to_owned()),
                role: Some("author".to_owned()),
            },
            Author {
                name: "Brod, Max".to_owned(),
                kind: AuthorKind::Person,
                dates: Some("1884-1968".to_owned()),
                gnd: Some("118515012".to_owned()),
                role: Some("editor".to_owned()),
            },
        ];
        record.publisher = Some("S. Fischer".to_owned());
        record.place = Some("Frankfurt am Main".to_owned());
        record.edition = Some("3. Auflage".to_owned());
        record.extent = Some("345 Seiten".to_owned());
        record.isbns = vec!["9783596294331".to_owned()];
        record.subjects = vec![
            "Deutsche Literatur".to_owned(),
            "Roman".to_owned(),
            "Prag".to_owned(),
        ];
        record.urls = vec![ResourceUrl {
            url: "https://d-nb.info/…".to_owned(),
            kind: UrlKind::Toc,
            label: Some("Inhaltsverzeichnis".to_owned()),
        }];
        record.holdings = prozess_holdings();
        record
    }

    /// The holdings of that record: three houses with copies, four that only state that
    /// they hold it.
    fn prozess_holdings() -> Vec<Holding> {
        vec![
            holding(
                "DE-11",
                "HU Berlin",
                "Humboldt-Universität zu Berlin, Universitätsbibliothek",
                Status::Available,
                vec![
                    item(
                        "ZB Grimm-Zentrum, 7. OG / Bereich B",
                        "96 A 10064",
                        Status::Available,
                    ),
                    item(
                        "ZwB Germanistik/Skandinavistik, UG",
                        "GM 4004 K64",
                        Status::Available,
                    ),
                ],
            ),
            holding(
                "DE-188",
                "FU Berlin",
                "Freie Universität Berlin, Universitätsbibliothek",
                Status::Reference,
                vec![item(
                    "Philologische Bibliothek, Ebene 1",
                    "GM 4004 K64 A9",
                    Status::Reference,
                )],
            ),
            holding(
                "DE-609",
                "ZLB",
                "Zentral- und Landesbibliothek Berlin",
                Status::Unavailable,
                vec![item(
                    "Amerika-Gedenkbibliothek",
                    "Kaf 3",
                    Status::Unavailable,
                )],
            ),
            holding(
                "DE-1",
                "Stabi Berlin",
                "Staatsbibliothek zu Berlin",
                Status::Available,
                Vec::new(),
            ),
            holding(
                "DE-83",
                "TU Berlin",
                "Technische Universität Berlin",
                Status::Available,
                Vec::new(),
            ),
            holding(
                "DE-521",
                "EUV Frankfurt (Oder)",
                "Europa-Universität Viadrina",
                Status::Available,
                Vec::new(),
            ),
            holding(
                "DE-517",
                "UP Potsdam",
                "Universität Potsdam",
                Status::Available,
                Vec::new(),
            ),
        ]
    }

    /// The example in `plan/cli.md` § `show`.
    ///
    /// Four deviations, each with a reason the data itself gives:
    ///
    /// 1. `Language ger`, not `German`. The record carries an ISO-639-2/B code and this
    ///    tool never invents a name for it.
    /// 2. `ISBN 9783596294331`, not `978-3-596-29433-1`. Hyphenation needs the
    ///    registration-group ranges, which are not in this binary.
    /// 3. `· 2 of 2 available` behind the HU name: copies are counted whenever a house
    ///    has more than one, and the example omits it there while demanding it in prose.
    /// 4. The sentence about the copy on loan, which the rules require and the example
    ///    leaves out.
    #[test]
    fn the_show_example_is_reproduced() {
        let locations = vec![
            institution("HU", "DE-11", "HU Berlin"),
            institution("FU", "DE-188", "FU Berlin"),
            institution("ZLB", "DE-609", "ZLB"),
        ];
        let expected = "\
Der Prozess
Roman

  Kafka, Franz (1883-1924)          author        GND 118559230
  Brod, Max (1884-1968)             editor        GND 118515012

  Published    S. Fischer, Frankfurt am Main, 1953
  Edition      3. Auflage
  Extent       345 Seiten
  Language     ger
  Format       Book
  ISBN         9783596294331
  Subjects     Deutsche Literatur · Roman · Prag
  Online       https://d-nb.info/… (table of contents)

Holdings

  ● HU Berlin — Humboldt-Universität zu Berlin, Universitätsbibliothek · 2 of 2 available
      ZB Grimm-Zentrum, 7. OG / Bereich B      96 A 10064        available
      ZwB Germanistik/Skandinavistik, UG       GM 4004 K64       available
  ◐ FU Berlin — Freie Universität Berlin, Universitätsbibliothek
      Philologische Bibliothek, Ebene 1        GM 4004 K64 A9    reference only
  ○ ZLB — Zentral- und Landesbibliothek Berlin
      Amerika-Gedenkbibliothek                 Kaf 3             on loan

  a copy on loan carries no due date here — return dates and holds are only in the library's own catalogue, behind a patron login

  also at: Stabi Berlin · TU Berlin · EUV Frankfurt (Oder) · UP Potsdam

almafu_BV008885798
";
        assert_eq!(rendered_show(&prozess(), &locations), expected);
    }

    /// 4.9 % of records have no `924` at all. That is "not stated in this record", and
    /// it is said in words — never as a silently empty list.
    #[test]
    fn a_record_without_holdings_says_so() {
        let mut record = prozess();
        record.holdings = Vec::new();
        let output = rendered_show(&record, &[]);
        assert!(output.contains("  no holdings recorded in this record\n"));
        assert!(!output.contains("also at:"));
    }

    /// A serial gets the sentence that the traffic light is about the title, not about
    /// the year the reader wants.
    #[test]
    fn a_serial_says_that_volumes_cannot_be_determined() {
        let mut record = prozess();
        record.format = Format::Journal;
        let output = rendered_show(&record, &[]);
        assert!(output.contains("which volumes are held cannot be determined here"));
        assert!(output.contains("ZDB"));
    }

    /// The sentence about a copy that is out is said once, whatever the number of copies
    /// that are out.
    #[test]
    fn the_due_date_sentence_is_said_once() {
        let mut record = prozess();
        for holding in &mut record.holdings {
            for item in &mut holding.items {
                item.status = Status::Unavailable;
            }
        }
        let output = rendered_show(&record, &[]);
        assert_eq!(output.matches("carries no due date here").count(), 1);
    }

    /// A book says nothing about volumes, and a book with nothing on loan says nothing
    /// about due dates: a note that is always there is a note nobody reads.
    #[test]
    fn a_book_with_every_copy_in_gets_no_notes() {
        let mut record = prozess();
        for holding in &mut record.holdings {
            for item in &mut holding.items {
                item.status = Status::Available;
            }
        }
        let output = rendered_show(&record, &[]);
        assert!(!output.contains("due date"));
        assert!(!output.contains("volumes are held"));
    }

    /// The terminal caps the lists; the JSON never does.
    #[test]
    fn the_author_and_subject_lists_are_capped_and_counted() {
        let mut record = prozess();
        record.authors = (0..8).map(|n| person(&format!("Autor {n}"))).collect();
        record.subjects = (0..9).map(|n| format!("Thema {n}")).collect();
        let output = rendered_show(&record, &[]);
        assert_eq!(output.matches("Autor ").count(), MAX_AUTHORS);
        assert!(output.contains("and 3 more"));
        assert_eq!(output.matches("Thema ").count(), MAX_SUBJECTS);
        assert!(output.contains("Thema 5 · and 3 more"));
    }

    /// Without `--at` nothing is "mine": every holding is listed and there is no
    /// `also at:` line to put anybody on.
    #[test]
    fn show_without_locations_lists_every_holding() {
        let output = rendered_show(&prozess(), &[]);
        assert!(!output.contains("also at:"));
        assert!(output.contains("Stabi Berlin — Staatsbibliothek zu Berlin"));
        assert!(output.contains("UP Potsdam — Universität Potsdam"));
    }

    /// The copy line of a `voebb` record carries the ordering note behind the status,
    /// where it cannot be mistaken for one.
    #[test]
    fn an_order_option_follows_the_status() {
        let mut record = prozess();
        record.holdings.truncate(1);
        record.holdings[0].items.truncate(1);
        record.holdings[0].items[0].order_option = Some("Außenmagazin, bestellbar".to_owned());
        let output = rendered_show(&record, &[]);
        let expected = format!(
            "{}  Außenmagazin, bestellbar",
            pad_right(label(Status::Available, true), STATUS_COLUMN)
        );
        assert!(output.contains(&expected), "{output:?}");
    }

    /// The library table of `plan/cli.md`, character for character.
    #[test]
    fn the_library_table_is_reproduced() {
        let libraries = crate::libraries::all();
        let wanted = ["DE-1", "DE-11", "DE-B1533", "DE-609"];
        let rows: Vec<&Library> = wanted
            .iter()
            .filter_map(|isil| libraries.iter().find(|library| library.isil == *isil))
            .collect();
        assert_eq!(
            rows.len(),
            wanted.len(),
            "the four example houses are listed"
        );
        let mut out = Vec::new();
        libraries_table(&rows, &mut out, Style::plain(WIDE)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        for line in output.lines() {
            assert_eq!(
                &line[5..9],
                "    ",
                "the shorthand column is nine wide: {line:?}"
            );
        }
        assert!(output.starts_with("STABI    DE-1       Stabi Berlin     "));
        assert!(output.contains("\nHU       DE-11      HU Berlin        "));
        assert!(output.contains("\nASH      DE-B1533   Alice Salomon HS "));
    }

    /// `--near` adds one column and changes nothing else.
    #[test]
    fn the_near_table_carries_the_distance() {
        let library = crate::libraries::all()
            .first()
            .expect("the list is not empty");
        let mut out = Vec::new();
        libraries_near(&[(library, 3.42)], &mut out, Style::plain(WIDE)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert!(output.trim_end().ends_with(" 3.4 km"), "{output:?}");
        assert!(output.contains(&library.short_name), "{output:?}");
    }

    /// The detail view states what the list knows and leaves out what it does not.
    #[test]
    fn the_detail_view_leaves_out_what_is_not_stated() {
        let library = crate::libraries::all()
            .iter()
            .find(|library| library.isil == "DE-1")
            .expect("the Stabi is in the list");
        let mut out = Vec::new();
        library_detail(library, &mut out, Style::plain(WIDE)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert!(output.starts_with(&library.name));
        assert!(output.contains("\n  Shorthand    STABI · SBB\n"));
        assert!(output.contains("\n  ISIL         DE-1\n"));
        assert!(output.contains("\n  Coordinates  52.51755, 13.39162\n"));
        assert!(!output.contains("Opening"), "hours are never compiled in");
        assert!(
            output.contains(" branches\n"),
            "the branch count comes before the branch names"
        );
    }

    /// "Nothing found" is never a bare line: the reason decides what to try next.
    #[test]
    fn an_empty_result_says_why() {
        let mut out = Vec::new();
        empty(
            &EmptyReason::FilteredOut {
                total: Some(774),
                fetched: 50,
                filter: "--format".to_owned(),
                value: "video".to_owned(),
            },
            &mut out,
        )
        .expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            output,
            "774 results, but none of the 50 fetched records matched --format video\n\
             narrow the search itself (--title, --author) so the filter has more to work on\n"
        );
    }

    /// An error says what failed and what to do about it, and the hint is indented under
    /// the message rather than left to look like a second error.
    #[test]
    fn an_error_carries_its_hint() {
        let mut out = Vec::new();
        error(
            &Error::Usage(crate::error::UsageError::UnknownLibrary {
                input: "STABI2".to_owned(),
                suggestions: vec!["STABI".to_owned()],
            }),
            &mut out,
            Style::plain(WIDE),
        )
        .expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        let mut lines = output.lines();
        assert_eq!(lines.next(), Some("error: unknown library \"STABI2\""));
        for line in lines {
            assert!(
                line.starts_with("       "),
                "the hint is indented: {line:?}"
            );
        }
    }
}
