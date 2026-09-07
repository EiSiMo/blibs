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
//! The columns of the examples are reproduced with **[`Column::Flex`] rather than
//! [`Column::Fixed`]** wherever the content is truncatable: the example's width is the
//! column's `ideal`, so a roomy terminal gets the example's layout, a narrow one shortens
//! the title instead of overflowing, and a value longer than the example widens its
//! column rather than being cut while the row still has room. Widths that carry meaning —
//! the marker, the year, the shelfmark, the record id — stay fixed and overflow rather
//! than lie.
//!
//! **A cell bound for a [`Column::Flex`] carries its raw text.** Shortening and padding
//! are the layout's job there and happen once, at the final width. A cell pre-shortened to
//! the example's constant capped its column at that constant however wide the terminal
//! was, and its baked-in padding was counted as content by the second shortening — which
//! put an ellipsis behind values that were complete. The one place a cell still shortens
//! itself is a [`Column::Fixed`] one, which by design never shortens anything: there the
//! cap has nowhere else to live, and the two shortenings cannot disagree because the
//! column does not move.
//!
//! The separation between columns is the layout's gap, never the padding of a cell. A
//! shrunken column spends its padding on its own text, so a title that had to give way
//! would otherwise end up glued to the author beside it.

use std::io::{self, Write};

use crate::counts::{records, results};
use crate::error::{EmptyReason, Error};
use crate::libraries::{Branch, Library};
use crate::model::{
    Engine, Format, Holding, Item, Location, Note, Record, SearchResult, SortKey, Status, UrlKind,
};
use crate::render::Style;
use crate::render::style::label;
use crate::render::table::{Cell, Column, Layout, display_width, truncate};
use crate::select::{self, Block, BlockRecord, holding_status};

/// Indent of a record line, in both list forms.
const RECORD_INDENT: usize = 2;
/// Indent of a copy line under a record in the grouped list.
const ITEM_INDENT: usize = 7;
/// Indent of a copy line in `show`.
const SHOW_ITEM_INDENT: usize = 6;
/// Indent of everything under a heading in `show`.
const SHOW_INDENT: usize = 2;

/// Title column with `--at`. An `ideal`, not a cap: a longer title widens the column
/// when the terminal has the room (see the module docs).
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

/// ISO-639-2/B codes and the English name `show` prints for them.
///
/// A display table, never a translation: the code stays in the JSON document and in
/// `--language`, and only the line a human reads is spelled out. The list covers the
/// codes that actually occur in this catalogue in any number; a code that is not here is
/// **printed as the code**, because inventing a name for it would be a claim, and a wrong
/// language is worse than an unexplained one.
///
/// ISO-639-2/**B** is the bibliographic set MARC uses, so `ger`/`fre`/`dut`/`gre`/`chi`
/// are the spellings to expect — not the terminological `deu`/`fra`/`nld`/`ell`/`zho`.
const LANGUAGE_NAMES: [(&str, &str); 30] = [
    ("ger", "German"),
    ("eng", "English"),
    ("fre", "French"),
    ("spa", "Spanish"),
    ("ita", "Italian"),
    ("rus", "Russian"),
    ("pol", "Polish"),
    ("tur", "Turkish"),
    ("ara", "Arabic"),
    ("heb", "Hebrew"),
    ("chi", "Chinese"),
    ("jpn", "Japanese"),
    ("lat", "Latin"),
    ("gre", "Greek"),
    ("dut", "Dutch"),
    ("por", "Portuguese"),
    ("cze", "Czech"),
    ("hun", "Hungarian"),
    ("swe", "Swedish"),
    ("dan", "Danish"),
    ("nor", "Norwegian"),
    ("fin", "Finnish"),
    ("ukr", "Ukrainian"),
    ("per", "Persian"),
    ("hin", "Hindi"),
    ("kor", "Korean"),
    ("vie", "Vietnamese"),
    ("cat", "Catalan"),
    ("srp", "Serbian"),
    ("hrv", "Croatian"),
];

/// Codes that name no language at all: "undetermined", "no linguistic content" and
/// "multiple languages". They are dropped rather than printed — `Language und` states
/// nothing, and a record whose only code is one of these gets no `Language` line.
///
/// The KOBV parser already drops them; a record from another engine may still carry one,
/// and this renderer is the last place that can keep it off the page.
const UNNAMED_LANGUAGES: [&str; 3] = ["und", "zxx", "mul"];

/// How many authors `show` prints before it counts the rest.
const MAX_AUTHORS: usize = 5;
/// How many subject headings `show` prints before it counts the rest.
const MAX_SUBJECTS: usize = 6;

/// What `show` says instead of a holdings list for a record without `924` fields.
///
/// 4.9 % of records have none. This is "not stated in this record", never "held nowhere",
/// and never a silently empty list.
const NO_HOLDINGS: &str = "no holdings recorded in this record";

/// What `show --at` says when the record states holdings, but none of them at the
/// libraries the user named.
///
/// The counterpart of [`NO_HOLDINGS`] for the narrowed question: without it the section
/// is simply empty and the reader has to infer the answer from an absence, one line above
/// an `also at:` line that lists everybody else.
const NO_HOLDINGS_HERE: &str = "no holdings at the libraries in --at, according to this record";

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
    let filtered = availability_filtered(result);
    let mut shown = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        if scoped {
            write_grouped_block(out, block, filtered, style, &mut shown)?;
        } else {
            write_flat_block(out, result, block, filtered, style, &mut shown)?;
        }
    }
    write_footer(out, result, &shown, scoped, style)
}

/// Whether `--available` actually judged anything, which is the only thing the headings
/// need to know about it.
///
/// The question is **not** "did the flag run": with `--available --format video` the
/// window filter can empty the page before a single status was asked for, and
/// `before_available` is then `Some(0)` — the filter ran over nothing. A heading built on
/// the mere presence of the field would say `none available now` about records nobody
/// ever looked at, and contradict the `--format` explanation printed beside it.
///
/// `window.before_available` counts the records the filter judged and is `None` when the
/// flag did not run — there is no second parameter carrying the same fact, so the two
/// cannot disagree. The condition is the one [`crate::cli::run`] uses to choose
/// `EmptyReason::NothingAvailable`, for the same reason.
fn availability_filtered(result: &SearchResult) -> bool {
    result
        .window
        .before_available
        .is_some_and(|judged| judged > 0)
}

/// One location's block: heading, then a line per record with that location's copies
/// beneath it.
fn write_grouped_block(
    out: &mut dyn Write,
    block: &Block<'_>,
    filtered: bool,
    style: Style,
    shown: &mut Vec<Status>,
) -> io::Result<()> {
    write_line(
        out,
        &block_heading(block, filtered),
        0,
        style.heading(),
        style,
    )?;
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
                .map(|item| item_row(item, style))
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
    filtered: bool,
    style: Style,
    shown: &mut Vec<Status>,
) -> io::Result<()> {
    let count = block.records.len();
    write_line(
        out,
        &flat_heading(result, count, filtered),
        0,
        style.heading(),
        style,
    )?;
    if count == 0 {
        return Ok(());
    }
    writeln!(out)?;

    // No running numbers once `--available` has sieved the page: the hidden records are
    // gone by the time this runs, so the survivors' positions are not knowable here, and
    // counting the survivors would print hit 5 as 4 the moment a hidden record precedes a
    // shown one. The number is a pure reading aid — `blibs` is stateless, `show 3` cannot
    // work, and the id is what `show` takes (`plan/cli.md` § *Für beide Formen*) — so a
    // number that no longer knows its position is dropped rather than invented. Starting
    // over at 1 would be the same false claim in a quieter voice: on `--page 2` those
    // numbers belong to the records of page 1.
    let first = (!filtered).then(|| first_number(result));
    let numbering = first.map(|first| Numbering {
        first,
        width: digits(first + count as u64 - 1),
    });
    let layout = flat_record_layout(numbering);
    let rows: Vec<Vec<Cell>> = block
        .records
        .iter()
        .enumerate()
        .map(|(offset, entry)| {
            flat_record_row(
                entry,
                numbering.map(|numbering| numbering.at(offset)),
                style,
            )
        })
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
    // Never conditional on knowing the total: with several locations there is no joint
    // hit count, and a sort that silently looked complete is exactly what `plan/cli.md`
    // forbids.
    if result.sort.by != SortKey::Relevance
        && result.total.is_none_or(|total| total > result.shown as u64)
    {
        notes.push(match result.total {
            Some(total) => format!(
                "--sort ordered the {} records shown, not all {total} results",
                result.shown
            ),
            None => format!(
                "--sort ordered the {} records shown, not the whole result",
                result.shown
            ),
        });
    }
    if let Some(note) = availability_filter_note(result) {
        notes.push(note);
    }
    notes
}

/// `--available hid 7 of the 10 records on this page`, and nothing when it hid none.
///
/// The filter thins the page out instead of refilling it, so the reader has to be told
/// how much of the page went — otherwise a page of three looks like a result of three.
/// The judged count is `window.before_available`, the survivors are `shown`, and the
/// difference is what disappeared.
fn availability_filter_note(result: &SearchResult) -> Option<String> {
    let judged = result.window.before_available?;
    let hidden = judged.saturating_sub(result.shown);
    if hidden == 0 {
        return None;
    }
    Some(format!(
        "--available hid {hidden} of the {} on this page",
        records(judged)
    ))
}

/// `HU Berlin · 6 results`, `… · showing 2` when the block shows fewer than it counted,
/// `… · no results` for a location that holds nothing.
///
/// A location whose engine could not state a total says `showing N` alone: printing the
/// number of records on this page as if it were the total would be a lie.
///
/// After `--available` an empty block is not an empty result: the hits exist, none of
/// their copies is in. It keeps its true total and says so —
/// `HU Berlin · 6 results · none available now`. Only with a known total above zero,
/// because otherwise the heading would name a number nobody reported.
fn block_heading(block: &Block<'_>, filtered: bool) -> String {
    let name = block
        .location
        .map_or("", |location| location.display.as_str());
    let shown = block.records.len();
    if shown == 0 {
        return match block.total.filter(|_| filtered) {
            Some(total) if total > 0 => {
                format!("{name} · {} · none available now", results(total))
            }
            _ => format!("{name} · no results"),
        };
    }
    match block.total {
        Some(total) if total > shown as u64 => {
            format!("{name} · {} · showing {shown}", results(total))
        }
        Some(total) => format!("{name} · {}", results(total)),
        None => format!("{name} · showing {shown}"),
    }
}

/// `774 results for Kafka Prozess · showing 1-10`.
///
/// The footer always names the true total, so that a short page does not read like a
/// short result.
///
/// The echo is printed as it was assembled and is **not** quoted again here: the quotes
/// a phrase carries (`6913 results for "Der Prozess"`) are the only place a reader can
/// see that it was searched as a phrase, and a second layer of them hid exactly that
/// signal behind `"\"Der Prozess\""`.
///
/// After `--available` the range is dropped for a count —
/// `774 results for Kafka Prozess · 3 available on this page`. The survivors are not
/// hits 1 to 3 but three of the ten on this page, and a range would suggest exactly the
/// completeness `plan/cli.md` § *Für beide Formen* forbids.
fn flat_heading(result: &SearchResult, shown: usize, filtered: bool) -> String {
    let terms = &result.query.terms;
    let total = result.total.unwrap_or(shown as u64);
    let head = format!("{} for {terms}", results(total));
    if shown == 0 {
        return head;
    }
    if filtered {
        return format!("{head} · {shown} available on this page");
    }
    // With `--format`/`--language` a range over `total` would be a lie: the window is one
    // anchored block of 50 raw records, `--page` walks the matches inside it, and there is
    // no page that reaches the rest of `total` at all. The count says what is really being
    // ranged over, and the running numbers in the list say which of them these are.
    if result.window.filtered {
        return format!(
            "{head} · {shown} of {} matching in this window",
            result.window.after_filter
        );
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

/// Decimal digits of a running number, so a page's markers stay in one column.
fn digits(number: u64) -> usize {
    number.to_string().len()
}

/// The columns of a record line under a location heading, as in `plan/cli.md`.
fn grouped_record_layout() -> Layout {
    Layout::new(vec![
        Column::Fixed(1),
        Column::Flex {
            ideal: GROUPED_TITLE,
            min: 12,
        },
        Column::Fixed(18),
        Column::Fixed(5),
        Column::Last,
    ])
}

/// The running numbers of one flat page: where the count starts and how wide the column
/// is, so that every line of the page reserves the same room.
///
/// `None` at the call site once `--available` has sieved the page — see
/// [`write_flat_block`] for why the numbers go rather than shift.
#[derive(Clone, Copy)]
struct Numbering {
    first: u64,
    width: usize,
}

impl Numbering {
    /// The number of the record at `offset` on this page, and the width it shares.
    fn at(self, offset: usize) -> (u64, usize) {
        (self.first + offset as u64, self.width)
    }
}

/// The columns of a record line in the flat list: a running number in front, a wider
/// title, and no copy lines to align with.
///
/// The first column is as wide as the page's largest running number plus its marker, so
/// that the markers of a page stand in one column whatever the page number. Without
/// numbers the marker stands alone, in the one column the grouped list gives it.
fn flat_record_layout(numbering: Option<Numbering>) -> Layout {
    Layout::new(vec![
        Column::Fixed(numbering.map_or(1, |numbering| numbering.width + 2)),
        Column::Flex {
            ideal: FLAT_TITLE,
            min: 12,
        },
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
fn item_layout(location_width: usize, shelfmark_width: usize) -> Layout {
    Layout::new(vec![
        Column::Flex {
            ideal: location_width,
            min: 12,
        },
        Column::Fixed(shelfmark_width),
        Column::Fixed(STATUS_COLUMN),
        Column::Last,
    ])
}

/// The copy layout of the grouped list.
fn search_item_layout() -> Layout {
    item_layout(SEARCH_ITEM_LOCATION, 19)
}

/// The copy layout of `show`, which indents one column less and has more room.
fn show_item_layout() -> Layout {
    item_layout(SHOW_ITEM_LOCATION, 16)
}

/// One record under a location heading. The marker is that **location's** traffic light,
/// not the record's overall one.
fn grouped_record_row(entry: &BlockRecord<'_>, style: Style) -> Vec<Cell> {
    let record = entry.record;
    vec![
        Cell::new(entry.status.symbol().to_string()).styled(style.status(entry.status)),
        Cell::new(record.title.clone()),
        // A `Fixed` column never shortens: the cap belongs to the cell here.
        Cell::new(truncate(first_author(record), 18)),
        Cell::new(year(record)),
        Cell::new(record.id.as_str()).whole(),
    ]
}

/// One record in the flat list. The number is a reading aid only — `blibs` is stateless,
/// so `show 3` cannot work and the id is what `show` takes.
///
/// `None` prints the marker alone: after `--available` the line no longer knows which hit
/// it is, and a reading aid is not worth a wrong number.
fn flat_record_row(
    entry: &BlockRecord<'_>,
    number: Option<(u64, usize)>,
    style: Style,
) -> Vec<Cell> {
    let record = entry.record;
    let symbol = entry.status.symbol();
    let marker = match number {
        Some((number, width)) => format!("{number:>width$} {symbol}"),
        None => symbol.to_string(),
    };
    vec![
        Cell::new(marker).styled(style.status(entry.status)).whole(),
        Cell::new(record.title.clone()),
        // A `Fixed` column never shortens: the cap belongs to the cell here.
        Cell::new(truncate(first_author(record), 15)),
        Cell::new(year(record)),
        Cell::new(record.id.as_str()).whole(),
    ]
}

/// One copy: where it stands, what it is called there, and whether it is in.
///
/// The location column's width is the layout's ([`item_layout`]), not the row's: a copy
/// line does not know how much room the block it lands in has.
fn item_row(item: &Item, style: Style) -> Vec<Cell> {
    let mut cells = vec![
        Cell::new(item_location(item)),
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
/// Never empty for the sake of it: with neither house nor shelf the volume alone carries
/// the line, and an empty location is never a reason to drop a copy.
fn item_location(item: &Item) -> String {
    let place = place_of(
        non_blank(item.branch_name.as_deref()),
        non_blank(item.location.as_deref()),
    );
    match (&place.is_empty(), &item.volume) {
        (true, Some(volume)) => volume.clone(),
        (false, Some(volume)) => format!("{place} · {volume}"),
        _ => place,
    }
}

/// The house and the shelf in one string — `Amerika-Gedenkbibliothek · Erwachsenenbereich`.
///
/// The branch is **prepended**, not merely substituted: a VÖBB record has one copy per
/// house and every one of them calls its shelf `Erwachsenenbereich`, so the house is the
/// half that tells the lines apart.
///
/// It is left out again when the location already carries it, which is the KOBV shape:
/// there `branch_name` is the text of the branch *link inside* the location cell
/// (`ZB Grimm-Zentrum` in `ZB Grimm-Zentrum, 3. OG / Bereich B`), and prepending it would
/// print the house twice on every line.
fn place_of(branch: Option<&str>, location: Option<&str>) -> String {
    match (branch, location) {
        (Some(branch), Some(location)) if !location.contains(branch) => {
            format!("{branch} · {location}")
        }
        (_, Some(location)) => location.to_owned(),
        (Some(branch), None) => branch.to_owned(),
        (None, None) => String::new(),
    }
}

/// A value that is present *and* says something. A cell that survived parsing as an empty
/// string would otherwise render as a house called nothing, with a separator behind it.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
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
    notes: &[Note],
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    write_line(out, &record.title, 0, style.bold(), style)?;
    if let Some(subtitle) = &record.subtitle {
        write_line(out, subtitle, 0, style.dim(), style)?;
    }
    write_author_block(out, record, style)?;
    write_field_block(out, record, style)?;
    write_holdings(out, record, locations, notes, style)?;
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
        Column::Flex {
            ideal: SHOW_AUTHOR_NAME,
            min: 12,
        },
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
                Cell::new(name),
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
/// The language is spelled out where [`LANGUAGE_NAMES`] knows the code and printed as the
/// bare code where it does not — see there for why the table is small and why nothing is
/// guessed.
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
    push_field(&mut fields, "Language", languages(record));
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

/// The languages of a record, named where they can be named.
///
/// Every code the record carries is listed, in its order: a bilingual edition is two
/// languages and saying only the first would be wrong. Codes that name no language are
/// dropped, so a record that states nothing else gets no `Language` line at all.
fn languages(record: &Record) -> String {
    record
        .languages
        .iter()
        .filter(|code| !UNNAMED_LANGUAGES.contains(&code.as_str()))
        .map(|code| language_name(code))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The English name of an ISO-639-2/B code, or the code itself when it is not in
/// [`LANGUAGE_NAMES`]. Never a guess: `Language xyz` says exactly what the record says.
fn language_name(code: &str) -> &str {
    LANGUAGE_NAMES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or(code, |(_, name)| *name)
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
    notes: &[Note],
    style: Style,
) -> io::Result<()> {
    writeln!(out)?;
    write_line(out, "Holdings", 0, style.heading(), style)?;
    writeln!(out)?;

    if record.holdings.is_empty() {
        write_line(out, NO_HOLDINGS, SHOW_INDENT, style.dim(), style)?;
        return write_show_notes(out, notes, style);
    }

    let (mine, others) = split_holdings(record, locations);
    // The record has holdings, just none of the user's: said in words rather than left as
    // a blank between the heading and the `also at:` line.
    if mine.is_empty() {
        write_line(out, NO_HOLDINGS_HERE, SHOW_INDENT, style.dim(), style)?;
    }
    // The copy columns are aligned across all of the shown holdings, not per house: one
    // ragged block per library would make the shelfmarks harder to compare than they are.
    let rows: Vec<Vec<Vec<Cell>>> = mine
        .iter()
        .map(|index| {
            record.holdings[*index]
                .items
                .iter()
                .map(|item| item_row(item, style))
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
    write_show_notes(out, notes, style)?;
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
///
/// What counts as the user's is [`select::holding_is_at`] and nothing spelled out again
/// here: it is the same rule that sets `holdings[].mine` in the JSON, and a second
/// spelling of it would let the two outputs disagree about the same record — including
/// over the branch narrowing, which an ISIL comparison cannot see.
fn split_holdings(record: &Record, locations: &[Location]) -> (Vec<usize>, Vec<usize>) {
    if locations.is_empty() {
        return ((0..record.holdings.len()).collect(), Vec::new());
    }
    let mut mine: Vec<usize> = Vec::new();
    for location in locations {
        for (index, holding) in record.holdings.iter().enumerate() {
            if select::holding_is_at(holding, location) && !mine.contains(&index) {
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

/// What `show` owes the reader about what it cannot know — the run of a serial, the due
/// date of a copy that is out, a `--at` the other catalogue answers for.
///
/// The sentences are [`crate::model::ShowResult::notes`] and are derived once, in `model`,
/// so that the terminal and the JSON state the same limitations for one record: what is
/// printed here an agent finds under a `kind` it can branch on. This renderer only decides
/// where they go.
fn write_show_notes(out: &mut dyn Write, notes: &[Note], style: Style) -> io::Result<()> {
    if notes.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for note in notes {
        write_line(out, &note.message, SHOW_INDENT, style.dim(), style)?;
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
    let city_width = column_width(1, rows.iter().map(|(library, _)| library.city.as_str()));
    let layout = Layout::new(vec![
        Column::Fixed(alias_width),
        Column::Fixed(isil_width),
        Column::Fixed(short_width),
        Column::Flex { ideal: 16, min: 16 },
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
                Cell::new(library.name.clone()),
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
        // The key, not only the name: this listing is the one place a branch without a
        // shorthand becomes addressable at all, and a name with nothing to type next to
        // it is a dead end (`plan/libraries.md` §11.9). Padded to the widest key, because
        // the ids come in two lengths and a ragged column reads as two columns.
        let keys: Vec<&str> = library
            .branches
            .iter()
            .map(|branch| branch.alias().unwrap_or(&branch.kobvid))
            .collect();
        let width = keys
            .iter()
            .map(|key| key.chars().count())
            .max()
            .unwrap_or(0);
        for (branch, key) in library.branches.iter().zip(keys) {
            fields.push((
                String::new(),
                format!("{key:width$}  {}", branch.short_name),
            ));
        }
    }
    write_fields(out, &fields, style)
}

/// Render one branch in detail: the branch itself, the house it belongs to, and how it
/// can be searched.
///
/// **Not the parent's detail view.** Someone who asks about the Amerika-Gedenkbibliothek
/// wants the house at Blücherplatz, not the 98-branch listing of the network it belongs
/// to; the parent is named in one line and the listing stays with the question it answers.
///
/// `at` is [`crate::libraries::branch_location`] verbatim — this function prints it and
/// decides nothing itself, so the view and `--at` cannot come to differ about which
/// engine searches a branch.
pub fn branch_detail(
    parent: &Library,
    branch: &Branch,
    at: &Location,
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    write_line(out, &branch.name, 0, style.bold(), style)?;
    writeln!(out)?;
    let mut fields = Vec::new();
    push_field(&mut fields, "Shorthand", branch.aliases.join(" · "));
    push_field(&mut fields, "Branch of", parent_line(parent));
    push_field(&mut fields, "ISIL", branch.isil.clone().unwrap_or_default());
    push_field(&mut fields, "KOBV id", branch.kobvid.clone());
    push_field(&mut fields, "Short name", branch.short_name.clone());
    push_field(
        &mut fields,
        "Address",
        branch.address.clone().unwrap_or_default(),
    );
    if let Some(coords) = branch.coords() {
        push_field(
            &mut fields,
            "Coordinates",
            format!("{:.5}, {:.5}", coords.lat, coords.lon),
        );
    }
    push_search_field(&mut fields, at);
    write_fields(out, &fields, style)
}

/// The house behind a branch: its shorthand and its full name, or just the name where the
/// list carries no shorthand for it.
fn parent_line(parent: &Library) -> String {
    match parent.alias() {
        Some(alias) => format!("{alias} — {}", parent.name),
        None => parent.name.clone(),
    }
}

/// The `Search` field: the `--at` value that searches this branch, and — where the answer
/// has an edge — what that edge is.
///
/// A `kobv` branch is not filtered upstream: its *house* is, and the branch is read off
/// the copies of the records that come back. Saying so here is the whole point of the
/// field, because it is the difference between "nothing there" and "nothing on this page".
fn push_search_field(fields: &mut Vec<(String, String)>, at: &Location) {
    push_field(
        fields,
        "Search",
        format!("--at {} ({} engine)", at.key, at.engine),
    );
    if at.engine == Engine::Kobv && at.branch.is_some() {
        fields.push((
            String::new(),
            "narrowed from the copies of its house's records, so a page \
             can be short and absence is never proven"
                .to_owned(),
        ));
    }
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
        QueryEcho, RecordId, ResourceUrl, ShowResult, SortScope, SortSpec, WindowInfo,
    };
    use crate::render::table::{pad_right, strip_ansi};

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
            role_code: None,
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

    /// One `at[]` entry, with the ids the engine reported for that location — which is
    /// what decides the block, exactly as it does in a real document.
    fn at(key: &str, isil: &str, total: u64, engine: Engine, records: &[&str]) -> AtBlock {
        AtBlock {
            key: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine,
            total: Some(total),
            records: records
                .iter()
                .map(|id| RecordId::parse(id).expect("the fixture ids are prefixed"))
                .collect(),
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
                filtered: false,
                undelivered: 0,
                before_available: None,
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

    fn rendered_error(error: &Error) -> String {
        let mut out = Vec::new();
        super::error(error, &mut out, Style::plain(WIDE)).expect("a vector accepts bytes");
        String::from_utf8(out).expect("the renderer writes UTF-8")
    }

    /// Nothing exercised this renderer, and so `error: error: --language takes …` went
    /// out: the prefix belongs to the renderer, and a variant whose own `Display` carries
    /// one prints it twice.
    ///
    /// `UsageError::Cli` is the single exemption, and it is exempt because it never
    /// arrives here: clap's `Display` does begin with `error:`, which is exactly why
    /// `main::refused` lets clap print it and nothing in the library raises one.
    #[test]
    fn an_error_line_carries_exactly_one_prefix() {
        for error in crate::error::every_variant_for_tests() {
            if matches!(error, Error::Usage(crate::error::UsageError::Cli(_))) {
                continue;
            }
            let rendered = rendered_error(&error);
            let first = rendered.lines().next().unwrap_or_default();
            let Some(rest) = first.strip_prefix("error: ") else {
                panic!("no prefix on {first:?}");
            };
            assert!(!rest.starts_with("error: "), "doubled prefix on {first:?}");
            assert!(!rest.trim().is_empty(), "empty message on {first:?}");
        }
    }

    /// The guard above only has teeth if it fails on the shape the bug had: a variant
    /// whose own `Display` already begins with the prefix.
    #[test]
    fn the_prefix_guard_catches_a_display_that_prefixes_itself() {
        let doubled = rendered_error(&Error::Usage(crate::error::UsageError::Cli(
            clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                "--language takes a three-letter code\n",
            ),
        )));
        let first = doubled.lines().next().unwrap_or_default();
        assert!(
            first
                .strip_prefix("error: ")
                .is_some_and(|rest| rest.starts_with("error: ")),
            "{first:?}"
        );
    }

    /// The shape of the whole thing, message and indented next step, on the variant that
    /// used to have neither a usable kind nor a hint.
    #[test]
    fn an_error_prints_its_message_and_its_hint_underneath() {
        let error: Error = crate::error::UsageError::LanguageCode {
            input: "de".to_owned(),
        }
        .into();
        assert_eq!(
            rendered_error(&error),
            "error: --language takes a three-letter ISO-639-2/B code as the records carry it \
             (ger, eng, fre — not de and not German), got \"de\"\n       the records carry \
             bibliographic codes, not the everyday ones: --language ger for German, eng for \
             English, fre for French\n"
        );
    }

    fn rendered_show(record: &Record, locations: &[Location]) -> String {
        rendered_show_at(record, locations, WIDE)
    }

    /// Renders through the document `cli` builds, not past it: the notes come from
    /// [`ShowResult::new`], so what these tests see is what `blibs show` prints.
    fn rendered_show_at(record: &Record, locations: &[Location], width: usize) -> String {
        let result = ShowResult::new(
            Some(record.clone()),
            record.id.engine(),
            locations,
            AvailabilityMode::Fetched,
        );
        let mut out = Vec::new();
        show(
            record,
            locations,
            &result.notes,
            &mut out,
            Style::plain(width),
        )
        .expect("a vector accepts bytes");
        String::from_utf8(out).expect("the renderer writes UTF-8")
    }

    /// The `at[]` of [`vorleser`]: what each location's search returned, which is what
    /// the blocks are built from.
    fn vorleser_at() -> Vec<AtBlock> {
        vec![
            at(
                "HU",
                "DE-11",
                6,
                Engine::Kobv,
                &["almahu_BV011234567", "almahu_BV019876543"],
            ),
            at("STABI", "DE-1", 3, Engine::Kobv, &["almafu_BV010111222"]),
            at(
                "AGB",
                "DE-609",
                35,
                Engine::Voebb,
                &["voebb_SAK13776205", "voebb_SAK14200311"],
            ),
        ]
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
        search_result.at = vorleser_at();
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
774 results for Kafka Prozess · showing 1-3

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
            filtered: false,
            undelivered: 0,
            before_available: None,
        };
        let output = rendered(&result, &locations);
        assert!(output.contains("note: the filters saw the 50 fetched records, not all 6 results"));
    }

    /// A location whose copies are all out keeps its hit count: the results exist, none
    /// of them is in. `· no results` would deny the hits themselves.
    #[test]
    fn an_emptied_block_keeps_its_total_after_the_availability_filter() {
        let (mut result, mut locations) = vorleser();
        locations.push(institution("TU", "DE-83", "TU Berlin"));
        result.at.push(at("TU", "DE-83", 6, Engine::Kobv, &[]));

        assert!(rendered(&result, &locations).contains("TU Berlin · no results\n"));

        result.window.before_available = Some(12);
        let output = rendered(&result, &locations);
        assert!(output.contains("TU Berlin · 6 results · none available now\n"));
        assert!(!output.contains("TU Berlin · no results"));
    }

    /// Without a total there is no number to keep, so the heading stays at `no results`
    /// rather than inventing one out of the page.
    #[test]
    fn an_emptied_block_without_a_total_still_says_no_results() {
        let (mut result, mut locations) = vorleser();
        locations.push(institution("TU", "DE-83", "TU Berlin"));
        let mut block = at("TU", "DE-83", 0, Engine::Kobv, &[]);
        block.total = None;
        result.at.push(block);
        result.window.before_available = Some(12);
        assert!(rendered(&result, &locations).contains("TU Berlin · no results\n"));
    }

    /// The flat heading counts instead of naming a range once the filter ran: the
    /// survivors are not hits 1 to 3 but three of the ten on this page, and a range is
    /// exactly the suggestion `plan/cli.md` § *Für beide Formen* forbids.
    #[test]
    fn the_flat_heading_counts_instead_of_ranging_after_the_availability_filter() {
        let ids = [
            "almafu_BV008885798",
            "b3kat_BV005550341",
            "almafu_BV035123456",
        ];
        let records = ids
            .iter()
            .map(|id| {
                let mut record = record(id, "Der Prozess", "Kafka, Franz", 1953);
                record.holdings = vec![holding(
                    "DE-11",
                    "HU Berlin",
                    "Humboldt-Universität zu Berlin",
                    Status::Available,
                    Vec::new(),
                )];
                record
            })
            .collect();
        let mut result = result("Kafka Prozess", Some(774), records);
        result.limit = 10;

        assert!(
            rendered(&result, &[]).starts_with("774 results for Kafka Prozess · showing 1-3\n")
        );

        result.window.before_available = Some(10);
        assert!(
            rendered(&result, &[])
                .starts_with("774 results for Kafka Prozess · 3 available on this page\n")
        );
    }

    /// `--available` is not the only reason a page can be empty. When `--format` emptied
    /// it before a single status was asked for, `before_available` is `Some(0)` — the
    /// filter ran over nothing — and a heading claiming `none available now` would state
    /// a verdict nobody reached, next to a `--format` explanation saying the opposite.
    #[test]
    fn a_block_the_filter_never_judged_does_not_say_none_available_now() {
        let (mut result, mut locations) = vorleser();
        locations.push(institution("TU", "DE-83", "TU Berlin"));
        result.at.push(at("TU", "DE-83", 6, Engine::Kobv, &[]));
        result.window.before_available = Some(0);

        let output = rendered(&result, &locations);
        assert!(output.contains("TU Berlin · no results\n"), "{output}");
        assert!(!output.contains("none available now"), "{output}");
    }

    /// The flat list stops numbering once the filter has sieved the page: the hidden
    /// records are gone, so a count over the survivors prints hit 5 as 4. The number is a
    /// reading aid, and a reading aid is not worth a wrong position.
    #[test]
    fn the_flat_list_drops_its_numbers_after_the_availability_filter() {
        let records = ["b3kat_BV005550341", "almafu_BV035123456"]
            .iter()
            .map(|id| {
                let mut record = record(id, "Der Prozess", "Kafka, Franz", 1953);
                record.holdings = vec![holding(
                    "DE-11",
                    "HU Berlin",
                    "Humboldt-Universität zu Berlin",
                    Status::Available,
                    Vec::new(),
                )];
                record
            })
            .collect();
        let mut result = result("Kafka Prozess", Some(774), records);
        result.limit = 3;
        result.page = Page::new(2).expect("2 is a page");

        // Unfiltered the numbers are the page's own positions, and they still are.
        let plain = rendered(&result, &[]);
        assert!(plain.contains("\n  4 ●  Der Prozess"), "{plain}");
        assert!(plain.contains("\n  5 ●  Der Prozess"), "{plain}");

        // Filtered, these two are the survivors of four judged records — hits 5 and 6 for
        // all this renderer knows, never 4 and 5.
        result.window.before_available = Some(4);
        let output = rendered(&result, &[]);
        assert!(!output.contains("  4 ●"), "{output}");
        assert!(!output.contains("  5 ●"), "{output}");
        assert!(!output.contains("  1 ●"), "{output}");
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("  ●  Der Prozess"))
                .count(),
            2,
            "{output}"
        );
    }

    /// The filter thins the page out instead of refilling it, so the footer says how
    /// much of the page went — and says nothing when it took nothing.
    #[test]
    fn the_availability_filter_says_how_much_of_the_page_it_hid() {
        let (mut result, locations) = vorleser();
        result.window.before_available = Some(12);
        let output = rendered(&result, &locations);
        assert!(output.contains("note: --available hid 7 of the 12 records on this page\n"));

        result.window.before_available = Some(result.shown);
        assert!(!rendered(&result, &locations).contains("--available hid"));

        result.window.before_available = None;
        assert!(!rendered(&result, &locations).contains("--available hid"));
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

    /// Regression: a value that fits its column is never marked as cut.
    ///
    /// The author cell used to arrive pre-shortened *and* padded to
    /// [`SHOW_AUTHOR_NAME`]. In a terminal too narrow for that width the column shrank
    /// below it, the second shortening counted the baked-in padding as content, and
    /// `Kafka, Franz (1883-1924)` came out as `Kafka, Franz (1883-1924)…` — an ellipsis
    /// behind a complete name, with nothing missing.
    #[test]
    fn a_complete_value_is_never_marked_as_cut_in_a_narrow_terminal() {
        let output = rendered_show_at(&prozess(), &[], 60);
        assert!(output.contains("Kafka, Franz (1883-1924)"), "{output:?}");
        assert!(
            !output.contains("Kafka, Franz (1883-1924)…"),
            "nothing was removed, so nothing marks a removal: {output:?}"
        );
    }

    /// Regression: a long value widens its column while the row has the room.
    ///
    /// The pre-shortened cell capped the column at [`SHOW_AUTHOR_NAME`] whatever the
    /// terminal offered, so a name longer than that stayed cut in a 200-column terminal
    /// even though the JSON carried it in full.
    #[test]
    fn a_long_value_is_not_cut_when_the_terminal_has_room_for_it() {
        let mut record = prozess();
        record.authors[0].name = "Enzensberger, Hans Magnus".to_owned();
        record.authors[0].dates = Some("1929-2022".to_owned());
        let output = rendered_show_at(&record, &[], 200);
        assert!(
            output.contains("Enzensberger, Hans Magnus (1929-2022)"),
            "{output:?}"
        );
        assert!(
            !output.contains("Enzensberger, Hans Magnus (1929-202…"),
            "the column widens instead of cutting: {output:?}"
        );
    }

    /// The author column is [`Column::Fixed`], which never shortens anything — so the
    /// cell shortens itself. Without that the name overflows and pushes the year and the
    /// record id of that one line out of their columns.
    #[test]
    fn a_long_author_is_cut_rather_than_pushing_the_id_out_of_its_column() {
        let mut result = result(
            "Kafka",
            Some(2),
            vec![
                record(
                    "gbv_777604809",
                    "Kafka en las dos orillas",
                    "Kafka, Franz",
                    2013,
                ),
                record(
                    "gbv_777604810",
                    "Kafka en las dos orillas",
                    "Martínez Salazar, Elisa",
                    2013,
                ),
            ],
        );
        result.records[1].holdings = Vec::new();
        let output = rendered(&result, &[]);
        let lines: Vec<&str> = output.lines().filter(|l| l.contains("gbv_")).collect();
        assert_eq!(lines.len(), 2, "{output:?}");
        // Compared in columns, not in bytes: `í` is one column and two bytes.
        let id_column = |line: &str| {
            let start = line.find("gbv_").expect("the line was picked for its id");
            display_width(&line[..start])
        };
        assert_eq!(
            id_column(lines[0]),
            id_column(lines[1]),
            "the id column does not move: {output:?}"
        );
        assert!(lines[1].contains("Martínez Salaz…"), "{output:?}");
    }

    /// The same for a copy line: the location column of `show` is an `ideal`, not a cap.
    #[test]
    fn a_long_location_is_not_cut_when_the_terminal_has_room_for_it() {
        let long = "Zentralbibliothek Grimm-Zentrum, 7. Obergeschoss / Bereich B";
        let mut record = prozess();
        record.holdings[0].items[0].location = Some(long.to_owned());
        let output = rendered_show_at(&record, &[], 200);
        assert!(output.contains(long), "{output:?}");
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
                role_code: Some("aut".to_owned()),
            },
            Author {
                name: "Brod, Max".to_owned(),
                kind: AuthorKind::Person,
                dates: Some("1884-1968".to_owned()),
                gnd: Some("118515012".to_owned()),
                role: Some("editor".to_owned()),
                role_code: Some("edt".to_owned()),
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
    /// Three deviations, each with a reason the data itself gives:
    ///
    /// 1. `ISBN 9783596294331`, not `978-3-596-29433-1`. Re-hyphenating needs the
    ///    registration-group ranges, which are not in this binary, and the record's own
    ///    hyphens come out on the way in (`plan/cli.md` § JSON).
    /// 2. `· 2 of 2 available` behind the HU name: copies are counted whenever a house
    ///    has more than one, and the example omits it there while demanding it in prose.
    /// 3. The sentence about the copy on loan, which the rules require and the example
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
  Language     German
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

    /// A known code is spelled out, an unknown one is printed **as the code**: the tool
    /// says what the record says rather than guessing at a name.
    #[test]
    fn a_language_code_is_named_only_when_it_is_known() {
        assert_eq!(language_name("ger"), "German");
        assert_eq!(language_name("gre"), "Greek", "the B set, not `ell`");
        assert_eq!(language_name("dut"), "Dutch", "the B set, not `nld`");
        assert_eq!(language_name("xyz"), "xyz");
        assert_eq!(language_name("deu"), "deu", "the T code is not the B code");
        assert_eq!(language_name("GER"), "GER", "codes are lowercase in MARC");
    }

    /// Every code in the table is a three-letter code and every name is spelled out
    /// once — a duplicated code would make the table's first entry unreachable.
    #[test]
    fn the_language_table_holds_no_duplicate_codes() {
        for (index, (code, name)) in LANGUAGE_NAMES.iter().enumerate() {
            assert_eq!(code.len(), 3, "{code:?} is not a three-letter code");
            assert!(!name.is_empty());
            assert!(
                !LANGUAGE_NAMES[..index].iter().any(|(seen, _)| seen == code),
                "{code:?} occurs twice"
            );
            assert!(
                !UNNAMED_LANGUAGES.contains(code),
                "{code:?} names no language and must not be given one"
            );
        }
    }

    /// A record in two languages names both, in its own order.
    #[test]
    fn a_multilingual_record_names_every_language() {
        let mut record = prozess();
        record.languages = vec!["ger".to_owned(), "lat".to_owned(), "xyz".to_owned()];
        assert!(
            rendered_show(&record, &[]).contains("Language     German, Latin, xyz"),
            "{}",
            rendered_show(&record, &[])
        );
    }

    /// `und` states nothing, so it is not printed — and a record whose only code is one
    /// of those gets no `Language` line rather than an empty one.
    #[test]
    fn a_code_that_names_no_language_is_left_out() {
        let mut record = prozess();
        record.languages = vec!["und".to_owned()];
        assert!(!rendered_show(&record, &[]).contains("Language"));

        record.languages = vec!["und".to_owned(), "eng".to_owned()];
        assert!(
            rendered_show(&record, &[]).contains("Language     English"),
            "{}",
            rendered_show(&record, &[])
        );
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

    /// `--at` at a library the record does not name is an answer, not a blank: the
    /// section says so in words, the way a record without any holdings does.
    #[test]
    fn a_location_the_record_does_not_name_says_so_instead_of_showing_nothing() {
        let mut record = prozess();
        // Keep only holdings no location below asks about.
        record.holdings.truncate(2);
        let output = rendered_show(&record, &[institution("AGB2", "DE-B1583", "Bibliothek")]);
        assert!(
            output.contains("  no holdings at the libraries in --at, according to this record\n"),
            "{output}"
        );
        // The record does hold something, and the line above must not hide it.
        assert!(output.contains("also at:"), "{output}");
    }

    /// A branch of the other catalogue on a KOBV record is stated under the holdings,
    /// with the same sentence the JSON carries — never ignored in silence.
    #[test]
    fn a_branch_of_the_other_catalogue_is_printed_as_a_note() {
        let output = rendered_show(
            &prozess(),
            &[branch(
                "AGB",
                "DE-609",
                "SIG00036",
                "Amerika-Gedenkbibliothek",
            )],
        );
        assert!(
            output.contains("--at AGB is answered by the voebb"),
            "{output}"
        );
        assert!(
            output.contains("says nothing about whether the copy stands there"),
            "{output}"
        );
    }

    /// A copy line names the house it stands in. Several copies of one VÖBB record all
    /// call their shelf `Erwachsenenbereich`, and without the house they are one line
    /// printed three times.
    #[test]
    fn a_copy_names_its_house_in_front_of_its_shelf() {
        assert_eq!(
            place_of(Some("Amerika-Gedenkbibliothek"), Some("Erwachsenenbereich")),
            "Amerika-Gedenkbibliothek · Erwachsenenbereich"
        );
    }

    /// The KOBV shape: there the branch name is the link text *inside* the location cell,
    /// so prepending it would print the house twice on one line.
    #[test]
    fn a_house_already_named_in_the_location_is_not_repeated() {
        assert_eq!(
            place_of(
                Some("ZB Grimm-Zentrum"),
                Some("ZB Grimm-Zentrum, 3. OG / Bereich B")
            ),
            "ZB Grimm-Zentrum, 3. OG / Bereich B"
        );
        assert_eq!(place_of(Some("Grimm-Zentrum"), None), "Grimm-Zentrum");
        assert_eq!(place_of(None, Some("Magazin")), "Magazin");
        assert_eq!(place_of(None, None), "");
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

    /// The two entries the list holds for one branch alias, for the branch tests below.
    fn branch_of(alias: &str) -> (&'static Library, &'static Branch) {
        match crate::libraries::look_up(alias).expect("the alias is in the list") {
            crate::libraries::Entry::Branch { parent, branch } => (parent, branch),
            other @ crate::libraries::Entry::Institution(_) => {
                panic!("{alias} must be a branch, got {other:?}")
            }
        }
    }

    fn branch_output(alias: &str) -> String {
        let (parent, branch) = branch_of(alias);
        let at = crate::libraries::branch_location(parent, branch);
        let mut out = Vec::new();
        branch_detail(parent, branch, &at, &mut out, Style::plain(WIDE)).expect("bytes");
        String::from_utf8(out).expect("UTF-8")
    }

    /// A branch of the public library network: its own address, its house named in one
    /// line, and the `--at` that searches it.
    ///
    /// The house's own branch list must **not** be here — answering "where is the
    /// Amerika-Gedenkbibliothek" with 98 branch names answers a different question.
    #[test]
    fn the_branch_view_states_the_branch_and_how_to_search_it() {
        let output = branch_output("AGB");
        let (parent, branch) = branch_of("AGB");

        assert!(output.starts_with(&branch.name), "{output:?}");
        assert!(output.contains("\n  Shorthand    AGB\n"), "{output:?}");
        assert!(output.contains("\n  KOBV id      SIG00036\n"), "{output:?}");
        assert!(
            output.contains("Blücherplatz 1, 10961 Berlin"),
            "the branch's own address, not the house's: {output:?}"
        );
        assert!(
            output.contains("\n  Branch of    VOEBB — "),
            "the house is named: {output:?}"
        );
        assert!(
            output.contains("\n  Search       --at AGB (voebb engine)\n"),
            "{output:?}"
        );
        assert!(
            !output.contains(" branches\n"),
            "the house's branch list belongs to the house: {output:?}"
        );
        for other in parent.branches.iter().filter(|b| b.kobvid != branch.kobvid) {
            assert!(
                !output.contains(&other.short_name),
                "{} must not appear: {output:?}",
                other.short_name
            );
        }
    }

    /// A branch outside the public network: the same view, the `--at` that searches it —
    /// and the edge that `--at` has there, because `kobv` can only filter its house.
    #[test]
    fn a_kobv_branch_states_the_edge_of_its_search() {
        let output = branch_output("PHILBIB");

        assert!(output.contains("\n  Branch of    FU — "), "{output:?}");
        assert!(
            output.contains("\n  Search       --at PHILBIB (kobv engine)\n"),
            "{output:?}"
        );
        assert!(output.contains("from the copies"), "{output:?}");
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
