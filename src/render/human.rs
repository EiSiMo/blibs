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
//! - **A copy that is out says when it comes back** where the catalogue states a date, in
//!   the status cell it belongs to and never in a column of its own ([`status_text`]).
//! - **A library that states its holdings in prose says so at the holding**, verbatim and
//!   wrapped, above whatever copies it also has ([`write_holdings_statement`]) — for a
//!   newspaper that sentence is the entire answer.
//! - When a client-side filter was in play, a line saying how large the window was — so
//!   that "nothing found" is never mistaken for "nothing exists".
//! - **A footnote about particular records names them**, under its own sentence: four
//!   identical paragraphs saying nothing about which of ten lines they meant is what the
//!   ids in [`Note::records`] exist to prevent ([`note_records_line`]).
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
//! put an ellipsis behind values that were complete. **No cell shortens itself** any more:
//! every column whose content may be shortened is a [`Column::Flex`], so the width is
//! decided once, by the layout, at the width the terminal actually has.
//!
//! The separation between columns is the layout's gap, never the padding of a cell. A
//! shrunken column spends its padding on its own text, so a title that had to give way
//! would otherwise end up glued to the author beside it.
//!
//! The **library views** are not here: [`super::libraries`] renders the compiled-in list,
//! this module renders search results. The two share the primitives below — a field block,
//! a wrapped line, a laid-out row — and nothing else.
//!
//! **Everything that is not a table row is word-wrapped** to the same width, with a
//! hanging indent where the line has a label (`write_paragraph`). A note, a library name
//! and a heading are sentences, not cells: they are continued rather than cut, and the
//! terminal never gets to break one wherever the character happens to fall.

use std::io::{self, Write};

use crate::counts::{records, results};
use crate::error::{EmptyReason, Error};
use crate::model::note_kinds;
use crate::model::{
    AvailabilityMode, Engine, Format, Holding, Item, Location, LocationRefusal, Note, Record,
    RecordId, SearchResult, SortKey, Status, UrlKind,
};
use crate::render::Style;
use crate::render::style::{Voice, label};
use crate::render::table::{Cell, Column, DEFAULT_GAP, Layout, display_width, wrap, wrap_hanging};
use crate::select::{self, Block, BlockRecord};

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
/// Author column with `--at`. An `ideal` for the same reason the title is one: `Einem,
/// Gottfried von` came out as `Einem, Gottfri…` in a 200-column terminal with the space
/// for it standing unused to the right (round 2, §3.4).
const GROUPED_AUTHOR: usize = 18;
/// Author column without `--at`, which spends the columns on the title instead.
const FLAT_AUTHOR: usize = 15;
/// The narrowest an author column may become. A name shortened past this says nothing:
/// `Schlink, Ber…` is still a person, `Schl…` is a prefix.
const AUTHOR_MIN: usize = 12;
/// Location column of a copy line in the grouped list.
const SEARCH_ITEM_LOCATION: usize = 33;
/// Location column of a copy line in `show`, which is indented one column less.
const SHOW_ITEM_LOCATION: usize = 39;
/// Author column of the author block in `show`.
const SHOW_AUTHOR_NAME: usize = 32;
/// Role column of the author block in `show`, wide enough for the words in [`ROLE_NAMES`].
const ROLE_COLUMN: usize = 12;
/// Label column of every `Label  value` block. Exactly as wide as the longest label.
const FIELD_LABEL: usize = 11;
/// Status column — a **floor**, not a width ([`item_layout`]): wide enough for the wordings
/// in [`label`], so that a following `order_option` never collides with it, and widened by
/// the block for a page whose copies state a return date beside the status.
const STATUS_COLUMN: usize = 20;

/// What a holding's prose statement is introduced with.
///
/// **`stated`, because it is not a copy line.** The items above it are shelves the service
/// listed one by one; this is what the library says about its run in its own words, and a
/// newspaper's only answer to "is it there" is this sentence. Without the label it reads as
/// an orphaned copy whose columns went missing.
const STATEMENT_LABEL: &str = "stated holdings: ";

/// The hanging indent of a wrapped holdings statement.
///
/// **Not the label's width**, which is what every other labelled line here uses: at 17
/// columns it would leave a 40-column terminal 16 for the text itself. Two columns are
/// enough to tell a continuation from a line of its own when the line above it opens with a
/// label.
const STATEMENT_HANG: usize = 2;

/// The English month names, for a return date a human reads.
///
/// A **display table like [`LANGUAGE_NAMES`]**: the ISO form is what the JSON carries and
/// what an agent parses, and only the terminal line is spelled out. `22 Sep 2026` rather
/// than `2026-09-22` because the reader of this line is deciding whether to wait three
/// weeks, and rather than `22/09/2026` because half the world reads that as 9 September.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

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

/// MARC relator codes and the English word `show` prints for them.
///
/// Exactly the policy of [`LANGUAGE_NAMES`], for the same reason: a **display table, never
/// a vocabulary**. The code stays in the JSON (`role_code`), and only the line a human
/// reads is spelled out. The codes here are the ones that actually occur in this catalogue
/// in any number (`plan/marc-mapping.md` § `$4`); a code that is not here is **printed as
/// the code**.
///
/// Printing the bare code rather than inventing a word is the whole point: `$4` is an
/// **open** vocabulary — `kom`, `isb`, `dgg`, `dgs` and `wac` are German extensions that no
/// `LoC` list contains, and a guessed word beside a name would be a claim about a person's
/// part in a book. `isb` unexplained is a small puzzle; `isb` glossed as "publisher"
/// because it looked like one is a falsehood.
const ROLE_NAMES: [(&str, &str); 22] = [
    ("aut", "author"),
    ("edt", "editor"),
    ("trl", "translator"),
    ("pbl", "publisher"),
    ("ill", "illustrator"),
    ("com", "compiler"),
    ("ctb", "contributor"),
    ("cmp", "composer"),
    ("act", "actor"),
    ("drt", "director"),
    ("prf", "performer"),
    ("nrt", "narrator"),
    ("lyr", "lyricist"),
    ("pro", "producer"),
    ("itr", "instrumentalist"),
    ("cng", "cinematographer"),
    ("ctg", "cartographer"),
    ("hnr", "honouree"),
    ("pht", "photographer"),
    ("prt", "printer"),
    ("art", "artist"),
    ("oth", "other"),
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
/// How many other branches the line about narrowed-away copies still names. Above it the
/// line states the count alone: it is a pointer, not a listing — the copies it speaks
/// about are the ones the user did not ask for.
const MAX_OTHER_BRANCHES: usize = 3;

/// Indent of everything under an error message: the width of `error: `, so that the hint
/// and a wrapped message stand under the sentence rather than under the label.
const ERROR_INDENT: usize = 7;

/// What a footnote is introduced with. Its width is also the hanging indent of a footnote
/// that has to be wrapped, so the two cannot drift apart.
const NOTE_LABEL: &str = "note: ";

/// How far in from its own note the list of affected records stands.
///
/// The width of [`NOTE_LABEL`], so that under `note: ` the ids start where the sentence
/// starts. In `show`, where a note carries no label, it is the same offset from the note's
/// indent — the list is subordinate to one sentence in both places, and one offset is what
/// keeps it looking that way.
const NOTE_RECORDS_INDENT: usize = NOTE_LABEL.len();

/// How many record ids a footnote names before it counts the rest.
///
/// A footnote about thirty records that lists all thirty is as unreadable as the thirty
/// separate footnotes it replaced, only in one paragraph instead of thirty. The rest is
/// **counted, never dropped**: `and 24 more` says that the note speaks about them too.
const MAX_NOTE_RECORDS: usize = 6;

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
    let sieve = Sieve::of(result);
    let mut shown = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        if scoped {
            write_grouped_block(out, block, sieve, style, &mut shown)?;
        } else {
            write_flat_block(out, result, block, sieve, style, &mut shown)?;
        }
    }
    write_footer(out, result, &shown, scoped, style)
}

/// One printed marker: the status its symbol stands for, and whether the record wearing
/// it is a thing a library lends at all.
///
/// The pair, not the status on its own. The symbol is a fact of the block — the copies
/// *there* — while the wording the legend may gloss it with is a fact of the record, and
/// keeping only the status left the legend deciding over the page instead of over the
/// records it speaks about ([`legend_voice`]).
#[derive(Debug, Clone, Copy)]
struct Marker {
    status: Status,
    lendable: bool,
}

impl Marker {
    /// The marker one printed line wears.
    ///
    /// `lendable` is [`Record::is_online_resource`] negated and not a second reading of
    /// the record: the copy line beneath asks the same question through [`item_voice`],
    /// and two spellings of it would let a legend and the lines under it disagree.
    fn of(entry: &BlockRecord<'_>) -> Self {
        Self {
            status: entry.status,
            lendable: !entry.record.is_online_resource(),
        }
    }
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

/// Whether the window this answer was cut from is **anchored**: one block of 50 raw
/// records starting at record 1, with `--page` walking the records it holds instead of
/// stepping the catalogue.
///
/// [`crate::cli::Plan::anchored`] read back off the answer, not a second rule: that
/// decision is `filters.is_active() || sort != relevance`, and this document carries both
/// halves verbatim — `window.filtered` *is* the first and `sort.by` *is* the second, so
/// nothing here is guessed.
///
/// Deliberately **not** `window.filtered` alone. `--sort` anchors the window without
/// filtering anything, so a renderer that asks the filter question gets `false` for a
/// sorted page and then prints a range, or a `no results`, that no page can deliver
/// (round 3, §1.4). `window.filtered` still answers its own question — "did a filter
/// run" — wherever a message names `--format`.
fn anchored(result: &SearchResult) -> bool {
    result.window.filtered || result.sort.by != SortKey::Relevance
}

/// The page number when this page begins after the last record an anchored window holds,
/// and `None` when it does not.
///
/// `(page − 1) × limit` is [`crate::select::PageCut`]'s offset for an anchored window and
/// `window.after_filter` is what that window holds — the same two numbers
/// [`crate::cli::run`] compares to choose `PastTheLastMatch`/`PastTheLastSorted`, so the
/// heading and the reason printed under it cannot disagree about whether this page exists.
///
/// Never true while `--available` was the one that emptied the page: a filter that judged
/// records had records to judge, which is only so on a page inside the window.
fn past_the_window(result: &SearchResult) -> Option<u32> {
    let page = result.page.get();
    let offset = (page as usize)
        .saturating_sub(1)
        .saturating_mul(result.limit);
    (anchored(result) && result.window.after_filter > 0 && offset >= result.window.after_filter)
        .then_some(page)
}

/// What ran over the answer as a whole, as far as a heading needs to know.
///
/// A heading may never say `no results` about a location whose hit count it knows to be
/// above zero: the hits exist, and a client-side sieve simply left none of them standing —
/// or the page asked for begins past the block the sieve pinned the window to. Which of
/// them it was decides the wording, all are facts about the run rather than about one
/// block, and the `--no-availability` one decides whether a branch heading may claim its
/// branch at all — so they are read off the result once and handed down.
#[derive(Debug, Clone, Copy)]
struct Sieve {
    /// `--available` judged at least one record — see [`availability_filtered`].
    availability: bool,
    /// `--format`/`--language` ran and left **nothing** of the fetched window, carrying
    /// the size of that window. `Some(0)` cannot occur: a window of nothing was never
    /// filtered.
    window_emptied: Option<usize>,
    /// The page begins after the end of an anchored window, carrying its number — see
    /// [`past_the_window`]. Exclusive with the other two by arithmetic, not by order.
    past_the_window: Option<u32>,
    /// `--no-availability` was given, which is what makes a KOBV branch unanswerable:
    /// the branch of a copy is named in the availability answer and nowhere else.
    no_availability: bool,
    /// The page that was asked for. Not a sieve itself, but the number every one of these
    /// headings names, and there is exactly one of it per run — a block that carries a
    /// refusal of its own ([`crate::model::LocationRefusal`]) needs it to say which page
    /// it was refused for.
    page: u32,
}

impl Sieve {
    /// Read the facts off the result. Nothing is derived twice: each of them has
    /// exactly one rule behind it, and the two about the window share [`anchored`].
    fn of(result: &SearchResult) -> Self {
        let window = result.window;
        Self {
            availability: availability_filtered(result),
            window_emptied: (window.filtered && window.fetched > 0 && window.after_filter == 0)
                .then_some(window.fetched),
            past_the_window: past_the_window(result),
            no_availability: result.availability == AvailabilityMode::Skipped,
            page: result.page.get(),
        }
    }
}

/// One location's block: heading, then a line per record with that location's copies
/// beneath it.
fn write_grouped_block(
    out: &mut dyn Write,
    block: &Block<'_>,
    sieve: Sieve,
    style: Style,
    shown: &mut Vec<Marker>,
) -> io::Result<()> {
    write_line(out, &block_heading(block, sieve), 0, style.heading(), style)?;
    if block.records.is_empty() {
        return Ok(());
    }
    writeln!(out)?;

    let record_layout = grouped_record_layout();
    let record_rows: Vec<Vec<Cell>> = block
        .records
        .iter()
        .map(|entry| grouped_record_row(entry, style))
        .collect();
    let item_rows: Vec<Vec<Vec<Cell>>> = block
        .records
        .iter()
        .map(|entry| item_rows(&entry.items, item_voice(entry.record), style))
        .collect();

    let record_widths = record_layout.widths(&record_rows, available(style, RECORD_INDENT));
    let all_items: Vec<Vec<Cell>> = item_rows.iter().flatten().cloned().collect();
    let item_layout = search_item_layout();
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
        for holding in holdings_at(entry, block.location) {
            write_holdings_statement(out, holding, ITEM_INDENT, style)?;
        }
        for item in items {
            write_row(out, &item_layout, item, &item_widths, ITEM_INDENT, style)?;
        }
        shown.push(Marker::of(entry));
    }
    Ok(())
}

/// The holdings of one record **at this block's location**, in the record's own order.
///
/// The same [`select::holding_is_at`] the copy lines were selected with, and not a second
/// spelling of the narrowing: a renderer that re-derived which holding belongs to a
/// location would drift from the one that answered, which has already cost a green light
/// over a book a branch had lent out (CLAUDE.md, *The traps*).
///
/// Empty without a location — the flat list has no copy lines to hang a statement under.
fn holdings_at<'a>(entry: &BlockRecord<'a>, location: Option<&Location>) -> Vec<&'a Holding> {
    let Some(location) = location else {
        return Vec::new();
    };
    entry
        .record
        .holdings
        .iter()
        .filter(|holding| select::holding_is_at(holding, location))
        .collect()
}

/// The flat list: a heading with the true total, then one numbered line per hit.
fn write_flat_block(
    out: &mut dyn Write,
    result: &SearchResult,
    block: &Block<'_>,
    sieve: Sieve,
    style: Style,
    shown: &mut Vec<Marker>,
) -> io::Result<()> {
    let count = block.records.len();
    let filtered = sieve.availability;
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
        shown.push(Marker::of(entry));
    }
    Ok(())
}

/// The legend and the footnotes, each preceded by a blank line and each omitted when it
/// would be empty.
fn write_footer(
    out: &mut dyn Write,
    result: &SearchResult,
    shown: &[Marker],
    scoped: bool,
    style: Style,
) -> io::Result<()> {
    // The legend lists the symbols that occurred and glosses them in one voice, so it is
    // handed both halves of the same markers: the statuses say which entries there are,
    // the records they belong to say what `○` may be called.
    let statuses: Vec<Status> = shown.iter().map(|marker| marker.status).collect();
    if let Some(legend) = style.legend(&statuses, legend_voice(shown, scoped)) {
        writeln!(out)?;
        writeln!(out, "{legend}")?;
    }
    let notes = footer_notes(result);
    if !notes.is_empty() {
        // Which records the answer is made of, for the footnotes that speak about some of
        // them. `SearchResult::shown` is this list's length by construction (`cli::run`),
        // so a footnote naming every one of them can say so instead of listing them.
        let universe: Vec<&RecordId> = result.records.iter().map(|record| &record.id).collect();
        writeln!(out)?;
        for note in notes {
            // The continuation lines start under the sentence, not under the label: a
            // second line flush with `note:` reads as a second note.
            write_paragraph(
                out,
                &format!("{NOTE_LABEL}{}", note.message),
                0,
                NOTE_LABEL.len(),
                style.dim(),
                style,
            )?;
            write_note_records(out, note.records, &universe, 0, style)?;
        }
    }
    Ok(())
}

/// A footnote as the terminal prints it: one sentence, and the records it is about.
///
/// The records are [`Note::records`] and are **not** re-derived here: which records a
/// limitation applies to is the engine's statement, and the JSON carries the same list
/// under the same note. The synthesised footnotes below — the window, the sort, the
/// availability filter — are about the answer as a whole and name none.
struct Footnote<'a> {
    message: String,
    records: &'a [RecordId],
}

impl Footnote<'_> {
    /// A sentence about the whole answer, which names no records because none of them is
    /// more affected than the rest.
    fn whole(message: String) -> Self {
        Self {
            message,
            records: &[],
        }
    }
}

/// Print which records a footnote is about, under the sentence itself.
///
/// The one place both `search` and `show` say this: a footnote that names records was
/// printed identically in both paths and would otherwise be the second spelling of one
/// rule (CLAUDE.md, *The traps*). `indent` is the note's own, and the list stands
/// [`NOTE_RECORDS_INDENT`] columns further in.
fn write_note_records(
    out: &mut dyn Write,
    records: &[RecordId],
    universe: &[&RecordId],
    indent: usize,
    style: Style,
) -> io::Result<()> {
    let Some(line) = note_records_line(records, universe) else {
        return Ok(());
    };
    write_line(out, &line, indent + NOTE_RECORDS_INDENT, style.dim(), style)
}

/// `record: voebb_SAK35527269` / `4 records: …, …` / `all 10 records`, or nothing.
///
/// Nothing at all for a note about the whole answer — a window that was truncated, a
/// location the other catalogue answers for — because there is no subset to point at.
///
/// **When the note names every record of the answer, the ids are the noise, not the
/// signal**: `all 10 records` is shorter than ten ids and says the same thing, and with a
/// single record — every `show`, and a search with one hit — even that says nothing the
/// reader cannot see, so the line is dropped. Anything shorter than the whole is listed,
/// because "which of these lines does this apply to" is the question the note leaves open.
///
/// The singular is not `1 record: …`: a count in front of one id is a count of nothing.
///
/// Above [`MAX_NOTE_RECORDS`] the list is cut and the remainder counted. The head keeps the
/// **true** number, so the count never states less than the note covers.
fn note_records_line(about: &[RecordId], universe: &[&RecordId]) -> Option<String> {
    if about.is_empty() {
        return None;
    }
    if covers_every_record(about, universe) {
        // The universe's count, not the note's: the two are the same set, and a note that
        // happens to name one id twice would otherwise count it twice.
        return (universe.len() > 1).then(|| format!("all {}", records(universe.len())));
    }
    let head = if about.len() == 1 {
        "record".to_owned()
    } else {
        records(about.len())
    };
    let listed: Vec<&str> = about
        .iter()
        .take(MAX_NOTE_RECORDS)
        .map(RecordId::as_str)
        .collect();
    let named = listed.join(", ");
    let rest = about.len() - listed.len();
    Some(if rest > 0 {
        format!("{head}: {named}, and {rest} more")
    } else {
        format!("{head}: {named}")
    })
}

/// Whether a note's records are exactly the records the output is about.
///
/// Set equality in both directions, and never true of an empty answer: a note that names
/// an id this output does not show has something to say that `all N records` would hide.
fn covers_every_record(about: &[RecordId], universe: &[&RecordId]) -> bool {
    !universe.is_empty()
        && universe.iter().all(|id| about.contains(id))
        && about.iter().all(|id| universe.contains(&id))
}

/// What the legend's wording may promise about this output.
///
/// Without `--at` the answer is about the region ([`Voice::Region`]). With it the symbols
/// are one library's, and the two scoped voices differ in exactly one gloss: `○` is "on
/// loan" where something can be lent and "currently unavailable" where nothing can
/// ([`item_voice`], [`crate::render::style::label`]).
///
/// **The set that decides it is the records wearing `○`, never the page.** Six hits of
/// which two were online resources kept the lending voice and glossed the symbol as "on
/// loan" over two lines that each read "currently unavailable" (round 3, §1.5) — the
/// invented fact round 2 took out of the copy line, standing in its definition instead.
///
/// A mixed page — a book that is out and an e-resource nobody can reach — takes the
/// neutral wording too, and that is the whole decision: "currently unavailable" is true
/// of both, since a copy on loan is also not to be had right now, while "on loan" over
/// the e-resource would be the invented fact again. The legend loses nothing by it,
/// because the copy lines keep the sharper voice of their own record and say which is
/// which. With no `○` on the page the choice glosses nothing, and the lending voice —
/// the one a library block speaks in — stands.
fn legend_voice(shown: &[Marker], scoped: bool) -> Voice {
    if !scoped {
        return Voice::Region;
    }
    let lendable = shown
        .iter()
        .filter(|marker| marker.status == Status::Unavailable)
        .all(|marker| marker.lendable);
    if lendable { Voice::Copy } else { Voice::Access }
}

/// The limitations worth a footnote: what the engines reported, plus the two that follow
/// from the window itself.
///
/// The window ones exist so that a short answer is never mistaken for a complete one:
/// a client-side filter and a client-side sort both see only the fetched records, and
/// `plan/cli.md` forbids output that suggests otherwise.
fn footer_notes(result: &SearchResult) -> Vec<Footnote<'_>> {
    // The page-wide count has to print before `AVAILABILITY_FILTER_UNSTATED`, which
    // refines it ("N of them said nothing at all"): a refinement printed above the fact
    // it refines names a subset before the whole, and reads backwards (round 3, §3.4).
    // Built here rather than left for its old place at the end, so it can be spliced in
    // right before the note it refines instead of merely after every other note.
    let mut available_hid = availability_filter_note(result).map(Footnote::whole);
    let mut notes: Vec<Footnote<'_>> = Vec::with_capacity(result.notes.len() + 1);
    for note in &result.notes {
        if note.kind == note_kinds::AVAILABILITY_FILTER_UNSTATED
            && let Some(hid) = available_hid.take()
        {
            notes.push(hid);
        }
        notes.push(Footnote {
            message: note.message.clone(),
            records: &note.records,
        });
    }
    // No refinement in this result — e.g. every hidden record's status was actually
    // read, just not `Available` — so there is nothing to stand in front of. Same
    // sentence, same place it always had.
    if let Some(hid) = available_hid {
        notes.push(hid);
    }
    let window = result.window;
    // Not when the filter matched nothing at all: the engine states that case as
    // `window_filter_empty`, which is already in `notes` above and says the same sentence
    // with the flag's own name in it. Two spellings of one limitation is what the note
    // vocabulary exists to prevent (round 2, §1.3).
    let stated = result
        .notes
        .iter()
        .any(|note| note.kind == note_kinds::WINDOW_FILTER_EMPTY);
    if window.after_filter < window.fetched && !stated {
        notes.push(Footnote::whole(match result.total {
            Some(total) => format!(
                "the filters saw the {} fetched records, not all {total} results",
                window.fetched
            ),
            None => format!(
                "the filters saw the {} fetched records only",
                window.fetched
            ),
        }));
    }
    if let Some(note) = sort_note(result) {
        notes.push(Footnote::whole(note));
    }
    notes
}

/// What `--sort` actually ordered, when that is less than the whole result.
///
/// **The number is the window's, not the page's.** `select::sort` runs over the deduped,
/// filtered window and `take_page` cuts the page out of the ordered set afterwards, so
/// `--sort ordered the 5 records shown` understated a sort over 32 and contradicted the
/// heading of its own output (round 2, §2.9). With `--sort` the window is anchored, which
/// is what makes that set nameable at all.
///
/// `--sort availability` is the one exception and keeps the page: statuses exist only for
/// the records that were fetched availability for, so the ordering that decides the output
/// is the second one, over the page (`cli::run`). Without availability there is no second
/// sort and the window is the honest number again.
///
/// Never conditional on knowing the total: with several locations there is no joint hit
/// count, and a sort that silently looked complete is exactly what `plan/cli.md` forbids.
fn sort_note(result: &SearchResult) -> Option<String> {
    if result.sort.by == SortKey::Relevance {
        return None;
    }
    let by_status =
        result.sort.by == SortKey::Availability && result.availability == AvailabilityMode::Fetched;
    let (ordered, what) = if by_status {
        (result.shown, "records shown")
    } else {
        (result.window.after_filter, "records in this window")
    };
    if result.total.is_some_and(|total| total <= ordered as u64) {
        return None;
    }
    Some(match result.total {
        Some(total) => format!("--sort ordered the {ordered} {what}, not all {total} results"),
        None => format!("--sort ordered the {ordered} {what}, not the whole result"),
    })
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
/// **An empty block is not an empty result once a sieve has run.** With a known total
/// above zero the heading keeps it and names what emptied the block:
///
/// - after `--available`, `HU Berlin · 6 results · none available now` — the hits exist,
///   none of their copies is in;
/// - after `--format`/`--language`,
///   `HU Berlin · 2005 results · none of the 50 fetched records matched` — the filter saw
///   one window of the result and nothing else. `· no results` there denied 2005 hits
///   that the footer named three lines further down (round 2, §1.3), and it is the one
///   arm of this function that can be a lie;
/// - past the end of an anchored window,
///   `HU Berlin · 2005 results · page 11 begins after the fetched window` — the same lie
///   for the other reason a window is anchored (round 3, §1.4b). `--sort` pins the window
///   to one block and `--page` walks what it holds, so a page past it is a page past the
///   window and not a library without hits. The wording is the reason's own, which is
///   printed underneath: two sentences about one situation must not be two situations.
///
/// Only with a known total above zero, because otherwise the heading would name a number
/// nobody reported.
///
/// **The fourth case has no total to keep and comes before all of them**: a location that
/// was never searched, because the page begins past its own last result or deeper than
/// its catalogue can be paged at all (round 3, §1.2). Its emptiness is not a sieve's
/// doing and not the library's — see [`refused_heading`] — and `· no results` there was
/// the same lie as the two above, contradicted by its own note two lines further down.
fn block_heading(block: &Block<'_>, sieve: Sieve) -> String {
    let name = block_name(block, sieve);
    let shown = block.records.len();
    if shown == 0 {
        // Before the total is even asked for: a refused location has none by
        // construction, and the answer to "why is this block empty" is one this
        // invocation decided rather than one the catalogue reported.
        if let Some(refused) = block.refused {
            return format!("{name} · {}", refused_heading(refused, sieve.page));
        }
        let Some(total) = block.total.filter(|total| *total > 0) else {
            return format!("{name} · no results");
        };
        // The window filter first: it runs before availability, and when it emptied the
        // window there was nothing left for `--available` to judge. The three are
        // mutually exclusive anyway — see [`Sieve`] — so this order reads rather than
        // decides.
        if let Some(fetched) = sieve.window_emptied {
            return format!(
                "{name} · {} · none of the {fetched} fetched records matched",
                results(total)
            );
        }
        if let Some(page) = sieve.past_the_window {
            return format!(
                "{name} · {} · page {page} begins after the fetched window",
                results(total)
            );
        }
        if sieve.availability {
            return format!("{name} · {} · none available now", results(total));
        }
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

/// What a block whose location was never searched says instead of `no results`.
///
/// Both refusals are about **this page**, not about the library, and the fourth heading
/// of a family whose first three are in [`block_heading`]. They read alike on purpose and
/// differ in the one clause that matters, because that clause is the whole difference
/// between them:
///
/// - `page 30 begins past its last result` — the catalogue answered, and *this location's*
///   result set ends before the page begins. It says how far that result set reaches;
/// - `page 20 begins past the deepest result this catalogue serves` — nothing was asked at
///   all. voebb.de is paged one sequential request at a time and blibs stops at
///   [`crate::engine::voebb::MAX_POSITION`], so this says how far the *tool* goes and
///   nothing whatsoever about the branch.
///
/// Neither names a number of hits, and not for the reason `past_the_window` has: there
/// simply is no total to name, because nobody counted.
fn refused_heading(refused: LocationRefusal, page: u32) -> String {
    match refused {
        LocationRefusal::PastTheLastResult => format!("page {page} begins past its last result"),
        LocationRefusal::WindowTooDeep => {
            format!("page {page} begins past the deepest result this catalogue serves")
        }
    }
}

/// What a block is about, which is not always what `--at` said.
///
/// With `--no-availability` a branch of a KOBV institution cannot be applied at all — the
/// branch of a copy is named in the availability answer and nowhere else — so the block
/// underneath is the whole **house's**. The note says so, but in a block format the
/// heading is what gets read, and it used to claim the branch over five records of the
/// institution (round 2, §2.1).
///
/// The house is named from the records' own holdings rather than from the location, whose
/// `display` is the branch's. An empty block has no holding to ask, and then the branch
/// name plus the qualifier is still the honest heading.
fn block_name(block: &Block<'_>, sieve: Sieve) -> String {
    let Some(location) = block.location else {
        return String::new();
    };
    let unapplied =
        sieve.no_availability && location.branch.is_some() && location.engine == Engine::Kobv;
    if !unapplied {
        return location.display.clone();
    }
    let house = block
        .records
        .iter()
        .flat_map(|entry| &entry.record.holdings)
        .find(|holding| holding.isil.as_ref() == Some(&location.isil))
        .and_then(|holding| holding.short_name.clone().or_else(|| holding.alias.clone()))
        .unwrap_or_else(|| location.display.clone());
    format!("{house} (branch not applied)")
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
///
/// An [`anchored`] window drops it for the same reason, and for both of the things that
/// anchor one: `--sort year --limit 5 --page 2` printed `301 results … showing 6-10`
/// over a footnote saying the sort had seen 50 records (round 3, §1.4a). Only the filter
/// may say `matching`, because only a filter matched anything.
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
    // With an anchored window a range over `total` would be a lie: the window is one
    // block of 50 raw records, `--page` walks what it holds, and there is no page that
    // reaches the rest of `total` at all. The count says what is really being ranged
    // over, and the running numbers in the list say which of them these are.
    if anchored(result) {
        let in_window = result.window.after_filter;
        return if result.window.filtered {
            format!("{head} · {shown} of {in_window} matching in this window")
        } else {
            format!("{head} · {shown} of {in_window} in this window")
        };
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
        Column::Least(1),
        Column::Flex {
            ideal: GROUPED_TITLE,
            min: 12,
        },
        Column::Flex {
            ideal: GROUPED_AUTHOR,
            min: AUTHOR_MIN,
        },
        // A year is not cut — `199…` is not a year — so the column follows the longest one
        // instead. Four digits is the normal case and five is not impossible.
        Column::Least(5),
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
        Column::Least(numbering.map_or(1, |numbering| numbering.width + 2)),
        Column::Flex {
            ideal: FLAT_TITLE,
            min: 12,
        },
        Column::Flex {
            ideal: FLAT_AUTHOR,
            min: AUTHOR_MIN,
        },
        Column::Least(4),
        Column::Last,
    ])
}

/// The columns of a copy line: location, shelfmark, status, and whatever `voebb` says
/// about ordering it.
///
/// **Neither the shelfmark nor the status may be cut** — half a shelfmark does not find a
/// book, and half a date is a different deadline — so both are [`Column::Least`]: the
/// stated width is a floor and the block's longest value decides the rest. They used to be
/// [`Column::Fixed`], which cuts nothing *and* grows for nothing, so a single copy with an
/// 18-character shelfmark in a 17-column column pushed its own status two columns right of
/// every other row of the block (measured on `voebb_SAK34906286`, 120 columns, 23 copies).
/// The location column gives way for them, and says so with an ellipsis.
fn item_layout(location_width: usize, shelfmark_width: usize) -> Layout {
    Layout::new(vec![
        Column::Flex {
            ideal: location_width,
            min: 12,
        },
        Column::Least(shelfmark_width),
        Column::Least(STATUS_COLUMN),
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
        Cell::new(first_author(record)),
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
        Cell::new(first_author(record)),
        Cell::new(year(record)),
        Cell::new(record.id.as_str()).whole(),
    ]
}

/// The copy lines of one record or one holding.
///
/// A copy that states neither where it stands nor what it is called there has nothing to
/// put in the two left columns, and eight such copies used to render as eight lines of
/// eighty spaces and a status word — which answers "where is it on the shelf" not at all
/// and reads like a layout crash (round 2, §3.3). Those copies are counted into **one**
/// line instead: `8 copies` where the shelf would be.
///
/// What is summarised and what is not:
///
/// - A copy that says *anything* — a house, a shelfmark, a volume — keeps its own line.
///   Suppressing an empty cell is forbidden and this does not do it: the collapsed copies
///   are exactly the ones with no cell to suppress.
/// - Copies of **different** status, or with different order options, are never counted
///   together: the summary carries one status word, and merging two would invent a claim
///   about half of them.
/// - The summary stands where the first of its copies stood, so the reading order of the
///   copies that do say something is untouched.
///
/// The count is the number of copies, and nothing else about them changes.
fn item_rows(items: &[&Item], voice: Voice, style: Style) -> Vec<Vec<Cell>> {
    // Asked once per copy: `item_location` composes a string, and the summarising below
    // looks at every copy again for every group it opens.
    let placeless: Vec<bool> = items.iter().map(|item| !states_a_place(item)).collect();
    let key = |item: &Item| (item.status, item.order_option.clone());
    let mut rows = Vec::new();
    let mut summarised = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if !placeless[index] {
            rows.push(item_row(item, voice, style));
            continue;
        }
        let group = key(item);
        if summarised.contains(&group) {
            continue;
        }
        let count = items
            .iter()
            .zip(&placeless)
            .filter(|(other, placeless)| **placeless && key(other) == group)
            .count();
        summarised.push(group);
        rows.push(placeless_row(item, count, voice, style));
    }
    rows
}

/// Whether a copy says where it is at all — a house, a shelf, a volume, a shelfmark.
///
/// The question is asked of the *rendered* cells, not of the fields: [`item_location`]
/// already decides what a location line says, and a second opinion here would collapse
/// copies whose line does carry something.
fn states_a_place(item: &Item) -> bool {
    !item_location(item).is_empty() || non_blank(item.call_number.as_deref()).is_some()
}

/// One line for `count` copies that state no place: `8 copies`, then the status they
/// share. The count stands in the location column, which is the column those copies left
/// empty — it is a count of copies, not a place, and it is dimmed like every other
/// secondary value on the line.
fn placeless_row(item: &Item, count: usize, voice: Voice, style: Style) -> Vec<Cell> {
    let mut cells = item_row(item, voice, style);
    if let Some(first) = cells.first_mut() {
        *first = Cell::new(copies(count)).styled(style.dim());
    }
    cells
}

/// `1 copy` / `8 copies`. Copies, never records: `counts::records` speaks about hits on a
/// page, and these are shelves of one and the same record.
fn copies(count: usize) -> String {
    if count == 1 {
        "1 copy".to_owned()
    } else {
        format!("{count} copies")
    }
}

/// One copy: where it stands, what it is called there, and whether it is in.
///
/// The location column's width is the layout's ([`item_layout`]), not the row's: a copy
/// line does not know how much room the block it lands in has.
///
/// `voice` is the record's, not the copy's ([`item_voice`]): whether `unavailable` may be
/// read as "on loan" is a question about the *thing*, and an [`Item`] alone cannot answer
/// it — which is why this takes a second argument rather than deciding on its own.
///
/// One row per copy — the summarising of copies that state no place at all is
/// [`item_rows`]'s, so that this one stays what its name says.
fn item_row(item: &Item, voice: Voice, style: Style) -> Vec<Cell> {
    let mut cells = vec![
        Cell::new(item_location(item)),
        Cell::new(item.call_number.clone().unwrap_or_default())
            .styled(style.dim())
            .whole(),
        Cell::new(status_text(item, voice))
            .styled(style.status(item.status))
            .whole(),
    ];
    if let Some(order) = &item.order_option {
        cells.push(Cell::new(order.clone()).styled(style.dim()));
    }
    cells
}

/// What the status column of a copy says: the traffic light in words, and the return date
/// where the catalogue stated one.
///
/// **One cell, not two.** A column of its own would stand empty on every copy that is in —
/// and most are — for the sake of the few that are out; here the date joins the sentence it
/// belongs to (`on loan, due 22 Sep 2026`), and the column widens for the page that has one
/// ([`item_layout`]).
///
/// The date is printed whatever the light says. A return date beside `available` is odd,
/// but it is what the catalogue stated, and dropping a stated field to make the line tidier
/// is the failure mode this renderer must not have.
fn status_text(item: &Item, voice: Voice) -> String {
    let word = label(item.status, voice);
    match non_blank(item.due_date.as_deref()) {
        Some(date) => format!("{word}, due {}", due_date(date)),
        None => word.to_owned(),
    }
}

/// An ISO-8601 date as a human reads it — `2026-09-22` becomes `22 Sep 2026`.
///
/// Exactly the policy of [`language_name`]: a value this table cannot name is **printed as
/// it stands**. `Item::due_date` is ISO by construction, but a renderer that reformatted
/// whatever it was handed would answer a changed upstream format with a wrong date, and a
/// wrong deadline is worse than an unpretty one.
fn due_date(date: &str) -> String {
    iso_date(date).unwrap_or_else(|| date.to_owned())
}

/// The three parts of an ISO date, spelled out — `None` for anything that is not exactly
/// `YYYY-MM-DD` with a month between 1 and 12.
fn iso_date(date: &str) -> Option<String> {
    let mut parts = date.split('-');
    let year = parts.next()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let month = MONTH_NAMES.get(month.checked_sub(1)?)?;
    Some(format!("{day} {month} {year}"))
}

/// Write what a library says about its holdings in prose, where it says anything.
///
/// The one place both `search` and `show` print it, so the two cannot come to word it
/// differently. It is **verbatim** — the statement is free text in the library's own
/// language and is never split into a location and a shelfmark ([`Holding::holdings_statement`]) —
/// and it is wrapped like every other sentence outside a table rather than laid out as a
/// row, because it runs well past a terminal and a cut one loses the end of a run.
fn write_holdings_statement(
    out: &mut dyn Write,
    holding: &Holding,
    indent: usize,
    style: Style,
) -> io::Result<()> {
    let Some(statement) = non_blank(holding.holdings_statement.as_deref()) else {
        return Ok(());
    };
    write_paragraph(
        out,
        &format!("{STATEMENT_LABEL}{statement}"),
        indent,
        STATEMENT_HANG,
        style.dim(),
        style,
    )
}

/// Which vocabulary the copy lines of a record may use.
///
/// [`Record::is_online_resource`] and nothing spelled out again here: the note about
/// missing due dates asks the same question in `model`, and two spellings of it would let
/// a copy line reading "currently unavailable" stand under a sentence about a copy on
/// loan (round 2, §1.9).
fn item_voice(record: &Record) -> Voice {
    if record.is_online_resource() {
        Voice::Access
    } else {
        Voice::Copy
    }
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
        // An `ideal`, not a cap: `$e` is free text (`Herausgebendes Organ`) and a spelled
        // out relator can be longer than the column, and both used to push the GND of that
        // one line out of its column.
        Column::Flex {
            ideal: ROLE_COLUMN,
            min: 8,
        },
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
                Cell::new(author_role(author)),
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

/// What one author's part in the book is called, in words where there are any.
///
/// `$e` first, because it is what the record itself says in words; `$4` after it, spelled
/// out through [`ROLE_NAMES`] where the code is known and printed bare where it is not.
/// Empty only when the record states neither.
///
/// The fallback is the whole point: `$4` without `$e` is the normal case in the BVB/B3Kat
/// records, and printing only `$e` left the column empty for them — three names in a row
/// with no role, one of which was the **publisher** (round 2, §1.12).
fn author_role(author: &crate::model::Author) -> String {
    if let Some(role) = non_blank(author.role.as_deref()) {
        return role.to_owned();
    }
    non_blank(author.role_code.as_deref())
        .map(role_name)
        .unwrap_or_default()
}

/// The English word for a relator code, or the code itself when [`ROLE_NAMES`] does not
/// know it. Never a guess — see there.
///
/// The comparison ignores case: `$4` is lower case throughout the sampled records, but the
/// subfield is free text and an upper-case `AUT` is not worth losing the word over.
fn role_name(code: &str) -> String {
    ROLE_NAMES
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(code))
        .map_or_else(|| code.to_owned(), |(_, name)| (*name).to_owned())
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
///
/// Shared with [`super::libraries`], which writes the same block for a house and for a
/// branch: `show` and the detail views must not come to disagree about where a value
/// starts or how a long one is continued.
///
/// A value too wide for the terminal is **wrapped into the value column**, not cut: the
/// subject headings of a record run past 80 columns routinely, and the terminal's own break
/// would put the continuation under the label, where it reads as a field of its own. The
/// continuation rows carry an empty label, which is how the second URL of a record is
/// already written.
///
/// The value column starts at a width the content cannot move — [`FIELD_LABEL`] is as wide
/// as the longest label there is — so the wrap width is known before the layout is
/// computed, and the two cannot disagree. It is a [`Column::Least`] all the same: a label
/// longer than the constant would otherwise push one row's value out of the column instead
/// of widening it, and this block is shared with the library views, whose labels this
/// module does not own.
pub(crate) fn write_fields(
    out: &mut dyn Write,
    fields: &[(String, String)],
    style: Style,
) -> io::Result<()> {
    let layout = Layout::new(vec![Column::Least(FIELD_LABEL), Column::Last]);
    let value_width = available(style, SHOW_INDENT + FIELD_LABEL + DEFAULT_GAP);
    let rows: Vec<Vec<Cell>> = fields
        .iter()
        .flat_map(|(label, value)| {
            wrap(value, value_width)
                .into_iter()
                .enumerate()
                .map(|(index, line)| {
                    let label = if index == 0 { label.as_str() } else { "" };
                    vec![
                        Cell::new(label).styled(style.dim()).whole(),
                        Cell::new(line),
                    ]
                })
                .collect::<Vec<_>>()
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
pub(crate) fn push_field(fields: &mut Vec<(String, String)>, label: &str, value: String) {
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

    // The answer is this one record, so a note that names it names everything there is
    // and prints no list — see [`note_records_line`].
    let universe = [&record.id];
    if record.holdings.is_empty() {
        write_line(out, NO_HOLDINGS, SHOW_INDENT, style.dim(), style)?;
        return write_show_notes(out, notes, &universe, style);
    }

    let split = select::show_holdings(record, locations);
    // The record has holdings, just none of the user's: said in words rather than left as
    // a blank between the heading and the `also at:` line.
    if split.mine.is_empty() {
        write_line(out, NO_HOLDINGS_HERE, SHOW_INDENT, style.dim(), style)?;
    }
    // The copy columns are aligned across all of the shown holdings, not per house: one
    // ragged block per library would make the shelfmarks harder to compare than they are.
    let voice = item_voice(record);
    let rows: Vec<Vec<Vec<Cell>>> = split
        .mine
        .iter()
        .map(|entry| item_rows(&entry.items, voice, style))
        .collect();
    let all: Vec<Vec<Cell>> = rows.iter().flatten().cloned().collect();
    let layout = show_item_layout();
    let widths = layout.widths(&all, available(style, SHOW_ITEM_INDENT));

    for (entry, items) in split.mine.iter().zip(&rows) {
        write_holding_heading(out, entry, style)?;
        // Above the copies: the statement is about the run as a whole, and the copies are
        // instances of it. A holding may carry one, copies, both or neither — a newspaper
        // is the statement alone, and this is the only line that answers for it.
        write_holdings_statement(out, entry.holding, SHOW_ITEM_INDENT, style)?;
        for row in items {
            write_row(out, &layout, row, &widths, SHOW_ITEM_INDENT, style)?;
        }
        write_narrowed_away(out, entry, style)?;
    }
    write_show_notes(out, notes, &universe, style)?;
    write_also_at(out, record, &split.others, style)
}

/// `● HU Berlin — Humboldt-Universität …`, with `2 of 3 available` behind the name when
/// the library holds more than one copy that said anything.
///
/// The count is the point: one traffic light per institution summarises several houses
/// and answers "can I go there" only by accident (`plan/usecases.md` UC-1).
///
/// Both numbers are the **narrowed** ones. The light is [`select::HoldingAt::status`] and
/// the count runs over [`select::HoldingAt::items`], so `show <id> --at AGB` says what the
/// AGB says and not what the network says: it used to paint `● … · 3 of 4 available` over
/// a book the user's own branch had lent out (round 2, §1.1).
///
/// The denominator is [`select::available_count`] — copies that stated a status at all —
/// and the count is dropped when fewer than two of them did. `0 of 2 available` beside two
/// lines reading "status not confirmed" was a counting claim next to two admissions that
/// nothing is known (§1.8), and `1 of 1` beside three copies is the same claim in a
/// quieter voice.
fn write_holding_heading(
    out: &mut dyn Write,
    entry: &select::HoldingAt<'_>,
    style: Style,
) -> io::Result<()> {
    // The name of a house runs to about a hundred characters (`Humboldt-Universität zu
    // Berlin, Universitätsbibliothek`, and longer), so the line is wrapped under itself
    // rather than under the traffic light: a continuation flush with the symbol would read
    // as a second library. The symbol keeps its own colour, and the count keeps its dim,
    // because both are appended here and never handed to the wrap as one painted string.
    let name = library_name(entry.holding);
    // The symbol and the space behind it; the name starts one column further in, and so do
    // its continuations.
    let indent = SHOW_INDENT + display_width(&entry.status.symbol().to_string()) + 1;
    let width = available(style, indent);
    let mut lines = wrap(&name, width);
    let mut painted: Vec<String> = lines.clone();
    if let Some(count) = select::available_count(entry.items.iter().copied())
        && count.known > 1
    {
        let text = format!(" · {} of {} available", count.available, count.known);
        // The count belongs behind the name; it moves to a line of its own only when the
        // last line has no room left for it. `trim_start` because a line does not begin
        // with the separator's space.
        let last = lines.len() - 1;
        if display_width(&lines[last]) + display_width(&text) <= width {
            painted[last].push_str(&style.paint(&text, style.dim()));
        } else {
            let own = text.trim_start().to_owned();
            painted.push(style.paint(&own, style.dim()));
            lines.push(own);
        }
    }
    for (index, line) in painted.iter().enumerate() {
        if index == 0 {
            writeln!(
                out,
                "{}{} {line}",
                " ".repeat(SHOW_INDENT),
                style.symbol(entry.status)
            )?;
        } else {
            writeln!(out, "{}{line}", " ".repeat(indent))?;
        }
    }
    Ok(())
}

/// `2 more copies at this library stand elsewhere: …` — what `--at <branch>` narrowed
/// away, and where it went.
///
/// The counterpart of the narrowing itself. A branch answer that simply shows fewer copies
/// than the house has is indistinguishable from a house that has that few, and CLAUDE.md
/// forbids reporting a record as having no shelfmarks it does in fact have. So the copies
/// that were dropped are counted and their houses named — never listed, because they are
/// not what was asked for.
///
/// Nothing at all when nothing was dropped, which is every holding of an institution: for
/// a location without a branch every copy is that location's.
fn write_narrowed_away(
    out: &mut dyn Write,
    entry: &select::HoldingAt<'_>,
    style: Style,
) -> io::Result<()> {
    let dropped = entry.holding.items.len() - entry.items.len();
    if dropped == 0 {
        return Ok(());
    }
    let mut elsewhere: Vec<String> = entry
        .holding
        .items
        .iter()
        // Identity, not equality: two copies of one record can be equal in every field
        // and still be two shelves. `entry.items` borrows out of this very vector, so a
        // pointer comparison is exact here and nowhere near as fragile as it would be
        // across two holdings.
        .filter(|item| !entry.items.iter().any(|kept| std::ptr::eq(*kept, *item)))
        .filter_map(|item| non_blank(item.branch_name.as_deref()).map(str::to_owned))
        .collect();
    // Not `dedup`: two copies of one branch need not be neighbours in the holding, and a
    // house named twice reads as two houses.
    let mut seen: Vec<String> = Vec::new();
    elsewhere.retain(|name| {
        let first = !seen.contains(name);
        if first {
            seen.push(name.clone());
        }
        first
    });
    // All of them or none: a partial list under a count of copies invites the reader to
    // add the two numbers up, and they are counts of different things — 44 copies stood at
    // 39 named branches and 5 houses the page did not name at all.
    let where_to = match elsewhere.len() {
        1..=MAX_OTHER_BRANCHES => format!(": {}", elsewhere.join(" · ")),
        _ => String::new(),
    };
    // Copies, never records: `counts::records` speaks about hits on a page, and these are
    // other shelves of one and the same record.
    let line = if dropped == 1 {
        format!("1 more copy of this library stands at another branch{where_to}")
    } else {
        format!("{dropped} more copies of this library stand at other branches{where_to}")
    };
    write_line(out, &line, SHOW_ITEM_INDENT, style.dim(), style)
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
fn write_show_notes(
    out: &mut dyn Write,
    notes: &[Note],
    universe: &[&RecordId],
    style: Style,
) -> io::Result<()> {
    if notes.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for note in notes {
        write_line(out, &note.message, SHOW_INDENT, style.dim(), style)?;
        write_note_records(out, &note.records, universe, SHOW_INDENT, style)?;
    }
    Ok(())
}

/// Say why there is nothing to show. Never a bare "no results": the reason decides what
/// the user should try next.
///
/// Wrapped like every other sentence this renderer writes. It was the one piece of output
/// that could not respect `COLUMNS`, because it took no [`Style`] — and it is two
/// sentences long in most of its variants, so it was also the one most likely to be broken
/// by the terminal wherever the character happened to fall.
pub fn empty(reason: &EmptyReason, out: &mut dyn Write, style: Style) -> io::Result<()> {
    for line in reason.message().lines() {
        write_line(out, line, 0, anstyle::Style::new(), style)?;
    }
    Ok(())
}

/// Render an error and its hint to stderr.
///
/// The hint is indented under the message so that the two read as one paragraph, and it
/// is never omitted when the variant has one — "request failed" without a next step is
/// exactly what this tool must not print.
///
/// Both are wrapped like every other sentence this renderer writes, at the hint's own
/// indent: an error is the one line the reader has to be able to read in full, and the
/// remediation text of some variants is three sentences long.
pub fn error(error: &Error, out: &mut dyn Write, style: Style) -> io::Result<()> {
    let label = style.paint("error:", style.bold());
    let message = error.to_string();
    let mut lines = wrap_hanging(
        &message,
        available(style, ERROR_INDENT),
        available(style, ERROR_INDENT),
    )
    .into_iter();
    // The first line carries the label; the rest stand under it, where the hint stands too.
    if let Some(first) = lines.next() {
        writeln!(out, "{label} {first}")?;
    }
    for line in lines {
        writeln!(out, "{}{line}", " ".repeat(ERROR_INDENT))?;
    }
    let Some(hint) = error.hint() else {
        return Ok(());
    };
    for line in hint.lines() {
        write_line(out, line, ERROR_INDENT, style.dim(), style)?;
    }
    Ok(())
}

/// The columns a row may use: the terminal, minus what the indent already spent.
pub(crate) fn available(style: Style, indent: usize) -> usize {
    style.width().saturating_sub(indent)
}

/// Write one laid-out row at an indent, without the trailing space padding leaves behind.
pub(crate) fn write_row(
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

/// Write one painted line at an indent, wrapped to the terminal.
///
/// Everything outside a table goes through here, which is what makes those lines respect
/// the width at all: a heading, a note and a library name used to run past the edge and be
/// broken by the terminal wherever the character happened to fall (round 2, §3.4). Nothing
/// is shortened — [`wrap`] only chooses where the line continues.
pub(crate) fn write_line(
    out: &mut dyn Write,
    text: &str,
    indent: usize,
    paint: anstyle::Style,
    style: Style,
) -> io::Result<()> {
    write_paragraph(out, text, indent, 0, paint, style)
}

/// The same, with the continuation lines indented `hang` columns further.
///
/// The hanging indent is what tells a continuation from a new line of its own: under
/// `note: ` the second line starts where the sentence does, not under the label.
fn write_paragraph(
    out: &mut dyn Write,
    text: &str,
    indent: usize,
    hang: usize,
    paint: anstyle::Style,
    style: Style,
) -> io::Result<()> {
    let lines = wrap_hanging(
        text,
        available(style, indent),
        available(style, indent + hang),
    );
    for (index, line) in lines.iter().enumerate() {
        let indent = if index == 0 { indent } else { indent + hang };
        writeln!(out, "{}{}", " ".repeat(indent), style.paint(line, paint))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AtBlock, Author, AuthorKind, AvailabilityMode, BranchRef, Engine, Isil, Note, Page,
        QueryEcho, RecordId, ResourceUrl, ShowResult, SortScope, SortSpec, WindowInfo,
    };
    use crate::render::table::{MIN_WIDTH, pad_right, strip_ansi};

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
            // Prose holdings: only voebb.de states any.
            holdings_statement: None,
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
            // A return date: only voebb.de states one.
            due_date: None,
            order_option: None,
        }
    }

    /// One `at[]` entry for a location that was never searched: no total, no records,
    /// and the reason it was refused.
    fn refused_at(key: &str, isil: &str, engine: Engine, why: LocationRefusal) -> AtBlock {
        AtBlock {
            total: None,
            refused: Some(why),
            ..at(key, isil, 0, engine, &[])
        }
    }

    fn institution(key: &str, isil: &str, display: &str) -> Location {
        Location {
            key: key.to_owned(),
            given: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine: Engine::Kobv,
            display: display.to_owned(),
        }
    }

    fn branch(key: &str, isil: &str, kobvid: &str, display: &str) -> Location {
        Location {
            key: key.to_owned(),
            given: key.to_owned(),
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
            given: key.to_owned(),
            isil: Isil::new(isil),
            branch: None,
            engine,
            total: Some(total),
            records: records
                .iter()
                .map(|id| RecordId::parse(id).expect("the fixture ids are prefixed"))
                .collect(),
            refused: None,
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

    /// The output with every run of whitespace collapsed to a single space.
    ///
    /// For an assertion about a **sentence** rather than about where it was wrapped: every
    /// line outside a table is laid out for the terminal since round 2 §3.4, so a long note
    /// is continued on the next line and a plain `contains` would be an assertion about the
    /// width, not about the words.
    fn one_line(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
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
            if matches!(error, Error::Usage(crate::error::UsageError::Cli { .. })) {
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
        // Not `UsageError::Cli` any more: since the second test round it reports clap's
        // first line with the `error: ` prefix stripped, so it no longer has the shape
        // this guard is here to catch. Any `Display` that prefixes itself will do.
        let doubled = rendered_error(&Error::Unexpected(
            crate::error::UnexpectedError::HttpStatus {
                host: "error: sru.kobv.de".to_owned(),
                status: 500,
            },
        ));
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
    ///
    /// Both are **wrapped** since round 2 §3.4 — the message here is 96 columns and
    /// [`WIDE`] is 100, so the continuation stands at the hint's indent, under the sentence
    /// rather than under the label.
    #[test]
    fn an_error_prints_its_message_and_its_hint_underneath() {
        let error: Error = crate::error::UsageError::LanguageCode {
            input: "de".to_owned(),
        }
        .into();
        assert_eq!(
            rendered_error(&error),
            "error: --language takes a three-letter ISO-639-2/B code as the records carry it \
             (ger, eng, fre — not\n       de and not German), got \"de\"\n       the records \
             carry bibliographic codes, not the everyday ones: --language ger for German, eng\n\
             \x20      for English, fre for French\n"
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

    /// A note that is about particular records, built here rather than taken from an
    /// engine: the messages are prose and are being reworded elsewhere, and a test that
    /// asserted on one of them would be a test of the wording, not of the rendering.
    fn note_about(kind: &'static str, message: &str, ids: &[&str]) -> Note {
        Note::about(
            kind,
            message,
            ids.iter()
                .map(|id| RecordId::parse(id).expect("the example ids carry their source prefix")),
        )
    }

    /// The lines a footnote's records are printed on, as they stand in the output.
    ///
    /// Looked for as lines of their own rather than with `contains` on the whole output,
    /// which would pass on an id that merely stood in the record list above. The list runs
    /// from the line opening with `prefix` to the end of its note — a blank line or the
    /// next `note:` — because it is wrapped like every other sentence here.
    fn note_lines<'a>(output: &'a str, prefix: &str) -> Vec<&'a str> {
        let mut lines = Vec::new();
        for line in output
            .lines()
            .skip_while(|line| !line.trim_start().starts_with(prefix))
        {
            let trimmed = line.trim_start();
            if !lines.is_empty() && (trimmed.is_empty() || trimmed.starts_with("note:")) {
                break;
            }
            lines.push(line);
        }
        lines
    }

    /// The same list as one sentence, so an assertion is about the words and not about
    /// where the terminal width put the break.
    fn note_records(output: &str, prefix: &str) -> String {
        one_line(&note_lines(output, prefix).join(" "))
    }

    /// The finding of round 3: four identical paragraphs and not a word about which of the
    /// ten lines above them each was talking about. The ids are in the data; the renderer
    /// used to drop them.
    #[test]
    fn a_note_names_the_records_it_is_about() {
        let (mut result, locations) = vorleser();
        result.notes = vec![note_about(
            note_kinds::VOEBB_ONLINE_ONLY,
            "an electronic title states no loan status this tool can read",
            &["voebb_SAK13776205", "voebb_SAK14200311"],
        )];
        let output = rendered(&result, &locations);
        assert_eq!(
            note_records(&output, "2 records:"),
            "2 records: voebb_SAK13776205, voebb_SAK14200311",
            "{output}"
        );
    }

    /// One record gets no count in front of it: `1 record: <id>` counts nothing.
    #[test]
    fn a_note_about_one_record_names_it_without_a_count() {
        let (mut result, locations) = vorleser();
        result.notes = vec![note_about(
            note_kinds::LOAN_WITHOUT_DUE_DATE,
            "a copy is on loan and the service states no due date",
            &["almahu_BV011234567"],
        )];
        let output = rendered(&result, &locations);
        assert_eq!(
            note_records(&output, "record:"),
            "record: almahu_BV011234567",
            "{output}"
        );
        assert!(!output.contains("1 record:"), "{output}");
    }

    /// A note that applies to everything on the page says so. Listing all five ids would
    /// be the same noise the four repeated paragraphs were, one line further down.
    #[test]
    fn a_note_about_every_record_says_all_instead_of_listing_them() {
        let (mut result, locations) = vorleser();
        let all: Vec<&str> = result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        let note = note_about(
            note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds nothing for these records",
            &all,
        );
        drop(all);
        result.notes = vec![note];
        let output = rendered(&result, &locations);
        assert_eq!(note_records(&output, "all "), "all 5 records", "{output}");
        assert!(note_lines(&output, "5 records:").is_empty(), "{output}");
    }

    /// A single-record answer states nothing at all: the one id is the line the reader is
    /// already looking at. Every `show` is this case.
    #[test]
    fn a_note_about_the_only_record_names_nothing() {
        let (mut result, locations) = vorleser();
        result.records.truncate(1);
        result.shown = 1;
        result.notes = vec![note_about(
            note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds nothing for this record",
            &["almahu_BV011234567"],
        )];
        let output = rendered(&result, &locations);
        assert!(note_lines(&output, "record:").is_empty(), "{output}");
        assert!(note_lines(&output, "all ").is_empty(), "{output}");
    }

    /// Thirty ids over half the screen are no better than thirty paragraphs. The list is
    /// cut at [`MAX_NOTE_RECORDS`] and the remainder is **counted**, never dropped: the
    /// head keeps the true number and the tail says how many are not named.
    #[test]
    fn a_long_record_list_is_cut_and_the_rest_counted() {
        let records: Vec<Record> = (0..30)
            .map(|index| {
                record(
                    &format!("almahu_BV{index:09}"),
                    "Der Prozess",
                    "Kafka, Franz",
                    1953,
                )
            })
            .collect();
        let named: Vec<String> = (0..8).map(|index| format!("almahu_BV{index:09}")).collect();
        let mut result = result("Kafka", Some(774), records);
        result.notes = vec![note_about(
            note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds nothing for these records",
            &named.iter().map(String::as_str).collect::<Vec<_>>(),
        )];
        let output = rendered(&result, &[]);
        assert_eq!(
            note_records(&output, "8 records:"),
            "8 records: almahu_BV000000000, almahu_BV000000001, almahu_BV000000002, \
             almahu_BV000000003, almahu_BV000000004, almahu_BV000000005, and 2 more",
            "{output}"
        );
    }

    /// The list is wrapped like every other sentence outside a table, at the width the
    /// terminal actually has.
    #[test]
    fn a_record_list_respects_the_terminal_width() {
        let (mut result, locations) = vorleser();
        result.notes = vec![note_about(
            note_kinds::VOEBB_ONLINE_ONLY,
            "an electronic title states no loan status this tool can read",
            &[
                "almahu_BV011234567",
                "voebb_SAK13776205",
                "voebb_SAK14200311",
            ],
        )];
        let mut out = Vec::new();
        search(&result, &locations, &mut out, Style::plain(MIN_WIDTH)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        let list = note_lines(&output, "3 records:");
        assert!(
            list.len() > 1,
            "three ids do not fit into 40 columns: {output}"
        );
        for line in &list {
            assert!(
                display_width(line) <= MIN_WIDTH,
                "{line:?} runs past {MIN_WIDTH} columns: {output}"
            );
        }
        assert_eq!(
            note_records(&output, "3 records:"),
            "3 records: almahu_BV011234567, voebb_SAK13776205, voebb_SAK14200311",
            "the wrap loses nothing: {output}"
        );
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

    /// Round 3, §1.4a: `--sort` anchors the window without filtering anything, and the
    /// heading asked the filter question. `301 results … showing 6-10` promised hits 6
    /// to 10 of 301 and a page 3, four lines above a note saying the sort had seen the
    /// 50 records of one window.
    #[test]
    fn the_flat_heading_counts_instead_of_ranging_over_an_anchored_window() {
        let records = ["b3kat_BV005550341", "almafu_BV035123456"]
            .iter()
            .map(|id| record(id, "Der Prozess", "Kafka, Franz", 2016))
            .collect();
        let mut result = result("title: Prozess, author: Kafka", Some(301), records);
        result.limit = 5;
        result.page = Page::new(2).expect("2 is a page");
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 50,
            filtered: false,
            undelivered: 0,
            before_available: None,
        };

        // Relevance steps the window, so the range is the truth and stays.
        let stepped = rendered(&result, &[]);
        assert!(stepped.contains("· showing 6-7"), "{stepped}");

        result.sort = SortSpec {
            by: SortKey::Year,
            scope: SortScope::Fetched,
        };
        let sorted = rendered(&result, &[]);
        assert!(sorted.contains("· 2 of 50 in this window"), "{sorted}");
        assert!(
            !sorted.contains("showing"),
            "a range claims pages the anchor cannot deliver: {sorted}"
        );
        assert!(
            !sorted.contains("matching"),
            "nothing matched here — the sort ordered: {sorted}"
        );

        // The filter's own wording is untouched, and keeps the word only it has earned.
        result.window.filtered = true;
        result.window.after_filter = 22;
        let filtered = rendered(&result, &[]);
        assert!(
            filtered.contains("· 2 of 22 matching in this window"),
            "{filtered}"
        );
    }

    /// Round 3, §1.2: a location that was never searched said `no results` — over a
    /// library that had simply been asked for a page nobody could deliver, and two lines
    /// above a note saying exactly that. The heading is what gets read in a block format,
    /// so it carries the reason itself.
    ///
    /// Both refusals in one output, because their difference is the point: the first
    /// reports a result set the catalogue ran out of, the second a question that was never
    /// asked. Neither may claim a number of hits, and neither may say `no results`.
    #[test]
    fn a_refused_block_names_the_page_instead_of_denying_hits() {
        let mut result = result("Kafka", None, Vec::new());
        result.at = vec![
            refused_at(
                "TU",
                "DE-83",
                Engine::Kobv,
                LocationRefusal::PastTheLastResult,
            ),
            refused_at(
                "AGB",
                "DE-609",
                Engine::Voebb,
                LocationRefusal::WindowTooDeep,
            ),
        ];
        result.limit = 20;
        result.page = Page::new(20).expect("20 is a page");
        let locations = vec![
            institution("TU", "DE-83", "TU Berlin"),
            branch("AGB", "DE-609", "SIG00036", "AGB (VÖBB)"),
        ];

        let out = rendered(&result, &locations);
        assert!(
            out.contains("TU Berlin · page 20 begins past its last result"),
            "{out}"
        );
        assert!(
            out.contains(
                "AGB (VÖBB) · page 20 begins past the deepest result this catalogue serves"
            ),
            "{out}"
        );
        assert!(
            !out.contains("no results"),
            "an empty block that was never searched is not an empty result: {out}"
        );
    }

    /// Round 3, §1.4b: `no results` over a library with 2005 hits. An anchored window is
    /// walked, not stepped, so a page past its end is a page past the window — and the
    /// footer three lines down named the hits the heading had just denied.
    #[test]
    fn a_block_a_page_ran_past_keeps_its_total() {
        let mut result = result("Kafka", Some(2005), Vec::new());
        result.at = vec![at("HU", "DE-11", 2005, Engine::Kobv, &[])];
        result.limit = 5;
        result.page = Page::new(11).expect("11 is a page");
        result.sort = SortSpec {
            by: SortKey::Year,
            scope: SortScope::Fetched,
        };
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 50,
            filtered: false,
            undelivered: 0,
            before_available: None,
        };
        let locations = vec![institution("HU", "DE-11", "HU Berlin")];

        let sorted = rendered(&result, &locations);
        assert!(
            sorted.contains("HU Berlin · 2005 results · page 11 begins after the fetched window"),
            "{sorted}"
        );
        assert!(!sorted.contains("no results"), "{sorted}");

        // The other anchor, in the case the round measured beside it: 22 records matched
        // `--format book` and page 4 begins after the last of them.
        result.sort = SortSpec {
            by: SortKey::Relevance,
            scope: SortScope::Fetched,
        };
        result.window.filtered = true;
        result.window.after_filter = 22;
        result.limit = 10;
        result.page = Page::new(4).expect("4 is a page");
        let filtered = rendered(&result, &locations);
        assert!(
            filtered.contains("HU Berlin · 2005 results · page 4 begins after the fetched window"),
            "{filtered}"
        );

        // Un-anchored, `--page` steps the catalogue and this heading knows of no end to
        // run past: it may not claim one.
        result.window.filtered = false;
        let stepped = rendered(&result, &locations);
        assert!(stepped.contains("HU Berlin · no results"), "{stepped}");
    }

    /// `--available` emptied the page inside the window, which is not a page past it:
    /// a filter that judged records had records to judge.
    #[test]
    fn a_page_emptied_by_the_availability_filter_is_not_a_page_past_the_window() {
        let mut result = result("Kafka", Some(2005), Vec::new());
        result.at = vec![at("HU", "DE-11", 2005, Engine::Kobv, &[])];
        result.limit = 5;
        result.sort = SortSpec {
            by: SortKey::Year,
            scope: SortScope::Fetched,
        };
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 50,
            filtered: false,
            undelivered: 0,
            before_available: Some(5),
        };
        let locations = vec![institution("HU", "DE-11", "HU Berlin")];

        let output = rendered(&result, &locations);
        assert!(
            output.contains("HU Berlin · 2005 results · none available now"),
            "{output}"
        );
        assert!(!output.contains("begins after"), "{output}");
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

    /// Round 3, §3.4: `AVAILABILITY_FILTER_UNSTATED` refines the page-wide count — "N of
    /// the records `--available` hid said nothing at all" only means something once the
    /// reader has been told that records were hidden, and how many. The two used to print
    /// in the opposite order: the refinement first, naming a subset (1) before the whole
    /// (2) it was drawn from.
    #[test]
    fn the_hidden_count_precedes_the_note_that_refines_it() {
        let (mut result, locations) = vorleser();
        result.window.before_available = Some(result.shown + 2);
        result.notes = vec![Note::new(
            note_kinds::AVAILABILITY_FILTER_UNSTATED,
            "no status was stated for 1 of the records --available hid; nothing was said \
             about their copies, so they are not known to be on loan",
        )];

        let output = rendered(&result, &locations);
        let hid = output
            .find("--available hid 2 of the")
            .expect("the page-wide count is printed");
        let refinement = output
            .find("no status was stated for 1")
            .expect("the refinement is printed");
        assert!(
            hid < refinement,
            "the page-wide count must precede the note that refines it:\n{output}"
        );
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

    /// A long author widens the column for the whole page rather than being cut in one
    /// line: the id column has to stay where it is, and at this width there is room.
    ///
    /// Changed in round 2 §3.4. The column used to be [`Column::Fixed`] with the cell
    /// shortening itself, which kept the id column in place but printed `Martínez Salaz…`
    /// in a terminal with sixty columns to spare. The guarantee this test was written for —
    /// the id column does not move — is unchanged and still asserted; what changed is that
    /// the column now grows to hold the name instead of the name being cut to the column.
    /// The narrow-terminal half of it is
    /// [`a_long_author_is_cut_when_the_row_has_no_room_for_it`].
    #[test]
    fn a_long_author_is_not_cut_when_the_page_has_room_for_it() {
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
        assert!(lines[1].contains("Martínez Salazar, Elisa"), "{output:?}");
        assert!(!output.contains('…'), "nothing was cut: {output:?}");
    }

    /// The other half of §3.4: the column gives way when the row genuinely has no room,
    /// and says so with an ellipsis rather than by silently losing the end of a name.
    #[test]
    fn a_long_author_is_cut_when_the_row_has_no_room_for_it() {
        let mut result = result(
            "Kafka",
            Some(1),
            vec![record(
                "gbv_777604810",
                "Kafka en las dos orillas",
                "Martínez Salazar, Elisa",
                2013,
            )],
        );
        result.records[0].holdings = Vec::new();
        let mut out = Vec::new();
        search(&result, &[], &mut out, Style::plain(60)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert!(output.contains('…'), "{output:?}");
        assert!(
            output.contains("gbv_777604810"),
            "the id is never cut: {output:?}"
        );
    }

    /// §3.3: eight copies that state neither a shelf nor a house are one line, not eight
    /// lines of eighty spaces behind a status word.
    #[test]
    fn copies_that_state_no_place_are_counted_into_one_line() {
        let (result, locations) = one_record_with(vec![placeless(Status::Available); 8]);
        let output = rendered(&result, &locations);
        assert_eq!(
            output.matches("available").count(),
            2,
            "one copy line and one legend entry: {output}"
        );
        assert!(output.contains("8 copies"), "{output}");
    }

    /// The count is a count of copies and says so in the singular too — the line exists to
    /// give the status something true to stand on, not to look plural.
    #[test]
    fn one_placeless_copy_says_one_copy() {
        let (result, locations) = one_record_with(vec![placeless(Status::Available)]);
        assert!(
            rendered(&result, &locations).contains("1 copy "),
            "{result:?}"
        );
    }

    /// A mixture is never flattened: a copy that states a shelf keeps its own line, and
    /// only the ones with nothing to put in those columns are summarised.
    #[test]
    fn a_copy_that_states_a_shelf_is_never_summarised_away() {
        let (result, locations) = one_record_with(vec![
            item("Grimm-Zentrum, 7. OG", "GM 5000 S345 V9", Status::Available),
            placeless(Status::Available),
            placeless(Status::Available),
        ]);
        let output = rendered(&result, &locations);
        assert!(output.contains("Grimm-Zentrum, 7. OG"), "{output}");
        assert!(output.contains("GM 5000 S345 V9"), "{output}");
        assert!(output.contains("2 copies"), "{output}");
    }

    /// Copies of different status are never counted together: the summary carries one
    /// status word, and merging two would state something about half of them that the
    /// service never said.
    #[test]
    fn placeless_copies_of_different_status_stay_apart() {
        let (result, locations) = one_record_with(vec![
            placeless(Status::Available),
            placeless(Status::Unavailable),
            placeless(Status::Available),
        ]);
        let output = one_line(&rendered(&result, &locations));
        assert!(output.contains("2 copies available"), "{output}");
        assert!(output.contains("1 copy on loan"), "{output}");
    }

    /// A copy that states only the volume it is — the serial case — carries its line, and
    /// is never counted away into a copy that states a different one.
    #[test]
    fn a_copy_that_states_only_its_volume_keeps_its_line() {
        let mut first = placeless(Status::Available);
        first.volume = Some("1.1953".to_owned());
        let mut second = placeless(Status::Available);
        second.volume = Some("2.1954".to_owned());
        let (result, locations) = one_record_with(vec![first, second]);
        let output = rendered(&result, &locations);
        assert!(output.contains("1.1953"), "{output}");
        assert!(output.contains("2.1954"), "{output}");
        assert!(!output.contains("copies"), "{output}");
    }

    /// A copy with nothing to say about where it stands: `location: null`,
    /// `call_number: null`, which is what 885 FU records look like.
    fn placeless(status: Status) -> Item {
        Item {
            location: None,
            branch: None,
            branch_name: None,
            call_number: None,
            volume: None,
            status,
            // A return date: only voebb.de states one.
            due_date: None,
            order_option: None,
        }
    }

    /// One record at one location, holding exactly these copies.
    fn one_record_with(items: Vec<Item>) -> (SearchResult, Vec<Location>) {
        let status = items.first().map_or(Status::Unknown, |item| item.status);
        let mut record = record(
            "almafu_9959168730302883",
            "Gewaltige Liebe",
            "Lohner, Eva Maria",
            2019,
        );
        record.holdings = vec![holding(
            "DE-188",
            "FU Berlin",
            "Freie Universität Berlin, Universitätsbibliothek",
            status,
            items,
        )];
        let mut result = result("Der Prozess", Some(885), vec![record]);
        result.at = vec![at(
            "FU",
            "DE-188",
            885,
            Engine::Kobv,
            &["almafu_9959168730302883"],
        )];
        (result, vec![institution("FU", "DE-188", "FU Berlin")])
    }

    /// §3.4, the wide direction: with two hundred columns nothing is shortened anywhere —
    /// not the title, not the author, not a copy line.
    #[test]
    fn nothing_is_cut_while_two_hundred_columns_stand_free() {
        let (mut result, locations) = vorleser();
        result.records[0].title =
            "Der Vorleser : Roman einer Kindheit und einer Schuld, mit einem Nachwort".to_owned();
        result.records[0].authors[0].name = "Einem, Gottfried von".to_owned();
        let mut out = Vec::new();
        search(&result, &locations, &mut out, Style::plain(200)).expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert!(!output.contains('…'), "{output}");
        assert!(output.contains("Einem, Gottfried von"), "{output}");
        assert!(
            output.contains(
                "Der Vorleser : Roman einer Kindheit und einer Schuld, mit einem Nachwort"
            ),
            "{output}"
        );
    }

    /// §3.4, the narrow direction: a terminal narrower than any layout must not panic, and
    /// whatever was removed says so with an ellipsis. The record id is still whole, because
    /// it is what `show` takes.
    ///
    /// [`MIN_WIDTH`] is where a real invocation bottoms out — a narrower `COLUMNS` is
    /// raised to it. The absurd widths are here anyway, because a [`Style`] can be built
    /// with any number and none of them may reach a subtraction that wraps.
    #[test]
    fn a_narrow_terminal_neither_panics_nor_cuts_silently() {
        let (result, locations) = vorleser();
        for width in [1, 20, MIN_WIDTH, 64] {
            let mut out = Vec::new();
            search(&result, &locations, &mut out, Style::plain(width)).expect("bytes");
            let output = String::from_utf8(out).expect("UTF-8");
            assert!(output.contains("almahu_BV011234567"), "{width}: {output}");
            // Every line that lost text says so; no line ends mid-word without a mark.
            assert!(
                output.contains('…'),
                "at {width} columns something had to give way: {output}"
            );
            let mut show = Vec::new();
            super::show(&prozess(), &locations, &[], &mut show, Style::plain(width))
                .expect("bytes");
            assert!(
                String::from_utf8(show)
                    .expect("UTF-8")
                    .contains("almafu_BV008885798"),
                "the id survives {width} columns"
            );
        }
    }

    /// The library line of `show` runs to about a hundred characters and used to run past
    /// the edge of every terminal. It is wrapped under itself — not under the traffic
    /// light, which would read as a second library — and the count stays behind the name.
    #[test]
    fn a_long_library_name_is_wrapped_under_itself() {
        let output = rendered_show_at(&prozess(), &[], 64);
        let lines: Vec<&str> = output.lines().collect();
        let heading = lines
            .iter()
            .position(|line| line.contains("HU Berlin"))
            .expect("the HU holding is printed");
        assert!(
            lines.iter().all(|line| display_width(line) <= 64),
            "nothing runs past the width: {output}"
        );
        assert!(
            lines[heading + 1].starts_with("    ") && !lines[heading + 1].starts_with("     "),
            "the continuation stands under the name, not under the symbol: {output}"
        );
        assert!(
            one_line(&output).contains(
                "HU Berlin — Humboldt-Universität zu Berlin, Universitätsbibliothek · 2 of 2 available"
            ),
            "{output}"
        );
    }

    /// A field value too wide for the terminal is continued in its own column, under an
    /// empty label — and never cut, and never broken on the separator of its list.
    #[test]
    fn a_long_field_value_is_wrapped_into_its_column() {
        let mut record = prozess();
        record.subjects = vec![
            "Deutsche Literatur".to_owned(),
            "Roman".to_owned(),
            "Prag".to_owned(),
            "Gerichtsverfahren".to_owned(),
            "Schuld".to_owned(),
        ];
        let output = rendered_show_at(&record, &[], 64);
        assert!(
            output.contains("  Subjects     Deutsche Literatur · Roman · Prag\n"),
            "no line ends on the separator: {output}"
        );
        assert!(
            output.contains("               · Gerichtsverfahren · Schuld\n"),
            "the continuation stands under the value: {output}"
        );
    }

    /// A URL survives every width: it is one word, so it overflows its line rather than
    /// being broken in two by a wrap that would put a space inside it.
    #[test]
    fn a_url_is_never_broken_by_the_wrap() {
        let mut record = prozess();
        let url = "https://nbn-resolving.org/urn:nbn:de:kobv:11-100123456-7890".to_owned();
        record.urls = vec![ResourceUrl {
            url: url.clone(),
            kind: UrlKind::Fulltext,
            label: None,
        }];
        assert!(rendered_show_at(&record, &[], 40).contains(&url));
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
    ///    leaves out. It is **wrapped** over two lines, because since round 2 §3.4 every
    ///    line outside a table is laid out for the width like the tables always were; at
    ///    [`WIDE`] the sentence is 127 columns long.
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

  a copy on loan carries no due date here — return dates and holds are only in the library's own
  catalogue, behind a patron login

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

    /// §1.12: the role column stays filled when the record states the role only as a MARC
    /// code, which is the normal case in the BVB/B3Kat records. Three names with an empty
    /// column read as three equal authors — one of them the publisher.
    #[test]
    fn a_role_stated_only_as_a_code_is_still_named() {
        let mut record = prozess();
        record.authors = vec![
            author_with_role("Kafka, Franz", None, Some("aut")),
            author_with_role("Stach, Reiner", None, Some("edt")),
            author_with_role("Wallstein-Verlag", None, Some("pbl")),
        ];
        let output = rendered_show(&record, &[]);
        assert!(
            output.contains("Kafka, Franz                      author"),
            "{output}"
        );
        assert!(
            output.contains("Stach, Reiner                     editor"),
            "{output}"
        );
        assert!(
            output.contains("Wallstein-Verlag                  publisher"),
            "the publisher must not read as an author: {output}"
        );
    }

    /// `$4` is an open vocabulary — `isb`, `dgg` and `wac` are German extensions no `LoC`
    /// list contains. An unknown code is printed **as the code**: a guessed word beside a
    /// name would be a claim about that person's part in the book.
    #[test]
    fn an_unknown_relator_code_is_printed_as_the_code() {
        assert_eq!(role_name("aut"), "author");
        assert_eq!(role_name("pbl"), "publisher");
        assert_eq!(role_name("isb"), "isb");
        assert_eq!(role_name("wac"), "wac");
        assert_eq!(role_name("AUT"), "author", "the case is not worth the word");
        let mut record = prozess();
        record.authors = vec![author_with_role("Beispiel, Ada", None, Some("dgg"))];
        assert!(rendered_show(&record, &[]).contains("dgg"));
    }

    /// `$e` is what the record says in words and wins over the code; a record with neither
    /// leaves the column empty rather than inventing a role.
    #[test]
    fn the_free_text_role_wins_over_the_code_and_neither_is_invented() {
        let author = author_with_role("Kafka, Franz", Some("Verfasser/in"), Some("aut"));
        assert_eq!(author_role(&author), "Verfasser/in");
        assert_eq!(
            author_role(&author_with_role("Kafka, Franz", None, None)),
            ""
        );
        assert_eq!(
            author_role(&author_with_role("Kafka, Franz", Some("  "), Some("edt"))),
            "editor",
            "an empty $e is not a role"
        );
    }

    /// Every code is a three-letter code, spelled out once.
    #[test]
    fn the_role_table_holds_no_duplicate_codes() {
        for (index, (code, name)) in ROLE_NAMES.iter().enumerate() {
            assert_eq!(code.len(), 3, "{code:?} is not a three-letter code");
            assert!(!name.is_empty());
            assert!(
                !ROLE_NAMES[..index].iter().any(|(seen, _)| seen == code),
                "{code:?} occurs twice"
            );
        }
    }

    fn author_with_role(name: &str, role: Option<&str>, code: Option<&str>) -> Author {
        Author {
            name: name.to_owned(),
            kind: AuthorKind::Person,
            dates: None,
            gnd: None,
            role: role.map(str::to_owned),
            role_code: code.map(str::to_owned),
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

    /// The verbatim `Bestand` line of `voebb_SAK13708822`, measured 2026-09-08. Long,
    /// German, and not a grammar — which is why it is printed whole and never split.
    const STATEMENT: &str = "Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.) Mikrofilm \
         Standort: BStB Signatur: A 80 ZC 181 Beil.:Mikro";

    /// One copy, with everything voebb.de states about it.
    fn copy(location: &str, call: &str, status: Status, due: Option<&str>) -> Item {
        Item {
            location: Some(location.to_owned()),
            branch: None,
            branch_name: None,
            call_number: Some(call.to_owned()),
            volume: None,
            status,
            due_date: due.map(str::to_owned),
            order_option: Some("Ausleihbar".to_owned()),
        }
    }

    /// A one-record answer at `HU`, whose single holding is `holding`. The vehicle for the
    /// copy-line tests below: one block, one record, one library.
    fn one_holding(holding: Holding) -> (SearchResult, Vec<Location>, Record) {
        let (mut result, locations) = vorleser();
        result.records.truncate(1);
        result.shown = 1;
        result.records[0].holdings = vec![holding];
        let record = result.records[0].clone();
        (result, locations[..1].to_vec(), record)
    }

    /// A holding at `HU`, with a prose statement, copies, both or neither.
    fn hu_holding(statement: Option<&str>, items: Vec<Item>) -> Holding {
        Holding {
            holdings_statement: statement.map(str::to_owned),
            ..holding(
                "DE-11",
                "HU Berlin",
                "Humboldt-Universität zu Berlin, Universitätsbibliothek",
                Status::Unavailable,
                items,
            )
        }
    }

    /// The copy lines of an output, indent and painting removed.
    fn copy_lines(output: &str) -> Vec<String> {
        output
            .lines()
            .filter(|line| line.contains("Erwachsenenbereich") || line.contains("Kinderbereich"))
            .map(|line| strip_ansi(line.trim_start()))
            .collect()
    }

    /// "When is it back" is a use case (`plan/usecases.md`), voebb.de answers it — eight
    /// times on `voebb_SAK34906286` alone — and the renderer used to drop the answer.
    ///
    /// The date joins the status rather than opening a column of its own: most copies are
    /// in and would pay for it with an empty cell.
    #[test]
    fn a_copy_on_loan_says_when_it_comes_back() {
        let (result, locations, record) = one_holding(hu_holding(
            None,
            vec![copy(
                "Erwachsenenbereich",
                "Schl",
                Status::Unavailable,
                Some("2026-09-22"),
            )],
        ));
        for output in [
            rendered(&result, &locations),
            rendered_show(&record, &locations),
        ] {
            assert!(
                strip_ansi(&output).contains("on loan, due 22 Sep 2026"),
                "{output}"
            );
        }
    }

    /// The date is written as a person reads it. `2026-09-22` is what the JSON carries and
    /// what an agent parses; `22/09/2026` would be 9 September to half the world.
    #[test]
    fn a_return_date_is_spelled_out_and_never_guessed() {
        assert_eq!(due_date("2026-09-22"), "22 Sep 2026");
        assert_eq!(due_date("2026-01-05"), "5 Jan 2026");
        assert_eq!(due_date("2026-10-01"), "1 Oct 2026");
        // Anything this table cannot name is printed as it stands — the policy of
        // `language_name` and `role_name`, for the same reason: a reformatted deadline that
        // is not the stated one is worse than an unpretty one.
        for stated in [
            "22.9.2026",
            "2026-13-01",
            "2026-09",
            "2026-09-22-01",
            "",
            "later",
        ] {
            assert_eq!(due_date(stated), stated, "{stated:?} is not an ISO date");
        }
    }

    /// The column widens for the page that has a date, and the copies that have none keep
    /// their neighbour in line — the whole reason the width is measured per block.
    #[test]
    fn the_status_column_widens_for_a_date_without_breaking_the_alignment() {
        let (result, locations, record) = one_holding(hu_holding(
            None,
            vec![
                copy(
                    "Erwachsenenbereich",
                    "Schl",
                    Status::Unavailable,
                    Some("2026-09-22"),
                ),
                copy("Kinderbereich", "Schl", Status::Available, None),
            ],
        ));
        for output in [
            rendered(&result, &locations),
            rendered_show(&record, &locations),
        ] {
            let lines = copy_lines(&output);
            assert_eq!(lines.len(), 2, "{output}");
            let order_at: Vec<Option<usize>> =
                lines.iter().map(|line| line.find("Ausleihbar")).collect();
            assert_eq!(order_at[0], order_at[1], "{lines:#?}");
            assert!(order_at[0].is_some(), "{lines:#?}");
        }
    }

    /// One row of a copy table: where it stands, what it is called there, what it says.
    fn copy_row(location: &str, call: &str, status: &str) -> Vec<Vec<Cell>> {
        vec![vec![
            Cell::new(location),
            Cell::new(call),
            Cell::new(status),
        ]]
    }

    /// A page without a single return date is laid out exactly as it was before dates
    /// existed: the width is a floor, and nothing pays for a column it does not use.
    ///
    /// Asked of the layout since the shelfmark bug, where the measuring moved out of a
    /// helper of this module and into [`Column::Least`]. The question is the same one; the
    /// two columns that may not cut their cells are now asked together, because they answer
    /// for one rule.
    #[test]
    fn a_column_that_may_not_cut_follows_its_longest_cell() {
        let layout = show_item_layout();
        let width_of = |rows: &[Vec<Cell>], column: usize| layout.widths(rows, WIDE)[column];

        let plain = copy_row("Erwachsenenbereich", "Schl", "on loan");
        assert_eq!(width_of(&plain, 2), STATUS_COLUMN);
        assert_eq!(width_of(&[], 2), STATUS_COLUMN);

        let dated = copy_row("Erwachsenenbereich", "Schl", "on loan, due 22 Sep 2026");
        assert_eq!(
            width_of(&dated, 2),
            display_width("on loan, due 22 Sep 2026")
        );

        // The bug: 18 characters in a 16-column column. The column grows; the row does not
        // move.
        let long = copy_row(
            "Mitte: Zentralbibliothek",
            "Konsolenspiel Fire",
            "available",
        );
        assert_eq!(width_of(&long, 1), display_width("Konsolenspiel Fire"));
        assert!(width_of(&plain, 1) < width_of(&long, 1));
    }

    /// A cell wider than its column widens the column and never pushes its own row out of
    /// line — measured on `voebb_SAK34906286` at 120 columns, where one 18-character
    /// shelfmark set that copy's status two columns right of the other 22.
    ///
    /// The assertion is **derivative**: every copy line of a block puts its status and its
    /// order option in the same column as every other. A test that wrote the width down as
    /// a number would be wrong again with the next record that has a longer shelfmark.
    #[test]
    fn one_over_long_shelfmark_never_moves_a_row_out_of_line() {
        let (result, locations, record) = one_holding(hu_holding(
            None,
            vec![
                copy(
                    "Mitte: Bezirkszentralbibliothek",
                    "Konsolenspiel Fire",
                    Status::Available,
                    None,
                ),
                copy(
                    "Mitte: Schiller-Bibliothek",
                    "EDV 945,7 Fire",
                    Status::Unavailable,
                    Some("2026-10-05"),
                ),
            ],
        ));
        for width in [MIN_WIDTH, 60, 80, 120] {
            let mut search_out = Vec::new();
            search(&result, &locations, &mut search_out, Style::plain(width)).expect("bytes");
            let mut show_out = Vec::new();
            show(&record, &locations, &[], &mut show_out, Style::plain(width)).expect("bytes");
            for bytes in [search_out, show_out] {
                let output = strip_ansi(&String::from_utf8(bytes).expect("UTF-8"));
                let lines: Vec<&str> = output
                    .lines()
                    .filter(|line| line.contains("Mitte:"))
                    .collect();
                assert_eq!(lines.len(), 2, "at {width}: {output}");
                assert_eq!(
                    lines[0].find("available"),
                    lines[1].find("on loan"),
                    "at {width} the status column moved: {output}"
                );
                let orders: Vec<Option<usize>> =
                    lines.iter().map(|line| line.find("Ausleihbar")).collect();
                assert_eq!(orders[0], orders[1], "at {width}: {output}");
                assert!(orders[0].is_some(), "at {width}: {output}");
            }
        }
    }

    /// A newspaper has no copies and is not "held nowhere": the page states a run, a house
    /// and a shelfmark in one prose line, and the tool used to answer that sentence with
    /// silence.
    #[test]
    fn a_holding_that_states_its_run_in_prose_prints_it() {
        let (result, locations, record) = one_holding(hu_holding(Some(STATEMENT), Vec::new()));
        for output in [
            rendered(&result, &locations),
            rendered_show(&record, &locations),
        ] {
            let said = one_line(&strip_ansi(&output));
            assert!(
                said.contains("stated holdings: Bestand in ZLB:"),
                "{output}"
            );
            assert!(
                said.contains("Signatur: A 80 ZC 181 Beil.:Mikro"),
                "{output}"
            );
        }
    }

    /// A statement and copies are not alternatives. The statement is about the run, the
    /// copies are the shelves it is on, and it stands above them.
    #[test]
    fn a_holding_prints_its_statement_above_its_copies() {
        let (result, locations, record) = one_holding(hu_holding(
            Some(STATEMENT),
            vec![copy(
                "Erwachsenenbereich",
                "A 80 ZC 181",
                Status::Unavailable,
                Some("2026-01-05"),
            )],
        ));
        for output in [
            rendered(&result, &locations),
            rendered_show(&record, &locations),
        ] {
            let plain = strip_ansi(&output);
            let statement = plain.find("stated holdings:").expect("the statement");
            let copy = plain.find("Erwachsenenbereich").expect("the copy line");
            assert!(
                statement < copy,
                "the run comes before its shelves: {output}"
            );
            assert!(plain.contains("on loan, due 5 Jan 2026"), "{output}");
        }
    }

    /// A statement belongs to the library that made it. It is picked with the same
    /// `select::holding_is_at` the copy lines are picked with, so a block never shows
    /// another library's prose.
    #[test]
    fn a_statement_stays_in_the_block_of_its_own_library() {
        let (mut result, locations) = vorleser();
        result.records.truncate(1);
        result.shown = 1;
        // Without a stated membership the blocks are built from the holdings, which is
        // what this test is about: the record is held at `DE-1` and nowhere else.
        result.at.clear();
        result.records[0].holdings = vec![Holding {
            holdings_statement: Some(STATEMENT.to_owned()),
            ..holding(
                "DE-1",
                "Stabi Berlin",
                "Staatsbibliothek zu Berlin",
                Status::Available,
                vec![copy(
                    "Haus Potsdamer Straße",
                    "1 A",
                    Status::Available,
                    None,
                )],
            )
        }];
        let output = strip_ansi(&rendered(&result, &locations));
        let hu = output.find("HU Berlin ·").expect("the HU block");
        let stabi = output.find("Stabi Berlin ·").expect("the Stabi block");
        let statement = output.find("stated holdings:").expect("the statement");
        assert!(hu < stabi && stabi < statement, "{output}");
    }

    /// Prose runs past a terminal, and a cut statement loses the end of a run. It is
    /// wrapped like every other sentence outside a table, at the width there is.
    #[test]
    fn a_long_statement_is_wrapped_and_never_cut() {
        let (result, locations, record) = one_holding(hu_holding(Some(STATEMENT), Vec::new()));
        let mut search_out = Vec::new();
        search(
            &result,
            &locations,
            &mut search_out,
            Style::plain(MIN_WIDTH),
        )
        .expect("bytes");
        let mut show_out = Vec::new();
        show(
            &record,
            &locations,
            &[],
            &mut show_out,
            Style::plain(MIN_WIDTH),
        )
        .expect("bytes");
        for bytes in [search_out, show_out] {
            let output = strip_ansi(&String::from_utf8(bytes).expect("UTF-8"));
            let said = one_line(&output);
            assert!(
                said.contains(&one_line(STATEMENT)),
                "nothing of the statement is lost at {MIN_WIDTH} columns: {output}"
            );
            for line in output.lines().filter(|line| {
                line.contains("Bestand") || line.contains("Signatur") || line.contains("Mikrofilm")
            }) {
                // The title above it is shortened at 40 columns and says so; a statement
                // never is, because the end of a run is the half a reader is after.
                assert!(!line.contains('…'), "a statement is never cut: {line:?}");
                assert!(
                    display_width(line) <= MIN_WIDTH,
                    "{line:?} runs past {MIN_WIDTH} columns"
                );
            }
        }
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
    ///
    /// The record must be one the location genuinely does not reach. `prozess()` is not:
    /// its ZLB copy stands in the Amerika-Gedenkbibliothek, and §2.8 of the second test
    /// round is precisely that the note used to fire there too, three lines under a
    /// `mine: true` that said the opposite. So this builds a ZLB record whose copy is in
    /// a different house of the network.
    #[test]
    fn a_branch_of_the_other_catalogue_is_printed_as_a_note() {
        let mut elsewhere = prozess();
        elsewhere.holdings = vec![holding(
            "DE-609",
            "ZLB",
            "Zentral- und Landesbibliothek Berlin",
            Status::Unavailable,
            vec![Item {
                location: Some("Berliner Stadtbibliothek".to_owned()),
                branch: Some("BIB000000072".to_owned()),
                branch_name: Some("Berliner Stadtbibliothek".to_owned()),
                call_number: Some("Kaf 3".to_owned()),
                volume: None,
                status: Status::Unavailable,
                // A return date: only voebb.de states one.
                due_date: None,
                order_option: None,
            }],
        )];
        let output = rendered_show(
            &elsewhere,
            &[branch(
                "AGB",
                "DE-609",
                "SIG00036",
                "Amerika-Gedenkbibliothek",
            )],
        );
        // Unwrapped: the note is longer than the terminal and is continued rather than cut
        // (round 2, §3.4), so the assertion is about the words and not about the width.
        let said = one_line(&output);
        assert!(
            said.contains("--at AGB is answered by the voebb"),
            "{output}"
        );
        assert!(
            said.contains("says nothing about whether the copy stands there"),
            "{output}"
        );
    }

    /// A copy at a named branch, for the `--at <branch>` tests below.
    fn copy_at(branch: &str, house: &str, call_number: &str, status: Status) -> Item {
        Item {
            location: Some("Erwachsenenbereich".to_owned()),
            branch: Some(branch.to_owned()),
            branch_name: Some(house.to_owned()),
            call_number: Some(call_number.to_owned()),
            volume: None,
            status,
            // A return date: only voebb.de states one.
            due_date: None,
            order_option: None,
        }
    }

    /// The VÖBB record of round 2, §1.1: one holding for the network, four copies in four
    /// houses, and the user's own house is the one that lent it out.
    fn vorleser_voebb() -> Record {
        let mut record = record(
            "voebb_SAK13363539",
            "Der Vorleser : Roman",
            "Schlink, Bernhard",
            2002,
        );
        record.holdings = vec![holding(
            "DE-609",
            "Berlin VÖBB/ZLB",
            "Verbund der Öffentlichen Bibliotheken Berlins",
            Status::Available,
            vec![
                copy_at(
                    "BIB000000010",
                    "Marzahn-Hellersdorf: Bezirkszentralbibliothek",
                    "Roman Schlin",
                    Status::Available,
                ),
                copy_at(
                    "BIB000000020",
                    "Mitte: Hansabibliothek",
                    "Roman Schlink",
                    Status::Available,
                ),
                copy_at(
                    "BIB000000030",
                    "Reinickendorf: Bibliothek Frohnau",
                    "Roman Schlin",
                    Status::Available,
                ),
                copy_at(
                    "SIG00036",
                    "ZLB: Amerika-Gedenkbibliothek",
                    "L 248 Schlin 50 p",
                    Status::Unavailable,
                ),
            ],
        )];
        record
    }

    /// §1.1, the heaviest finding of round 2: `show <id> --at AGB` used to render every
    /// copy of the network under the user's own branch, count them all, and paint the
    /// house's green light over a book the AGB had lent out — the exact opposite of what
    /// the search path said about the same id and the same `--at`.
    #[test]
    fn show_at_a_branch_answers_for_that_branch_and_not_for_its_house() {
        let agb = branch("AGB", "DE-609", "SIG00036", "Amerika-Gedenkbibliothek");
        let output = rendered_show(&vorleser_voebb(), std::slice::from_ref(&agb));

        assert!(
            output.contains("○ Berlin VÖBB/ZLB"),
            "the traffic light is the branch's: {output}"
        );
        assert!(
            !output.contains("available"),
            "no copy of the AGB is in, so nothing may say it is: {output}"
        );
        assert!(
            output.contains("L 248 Schlin 50 p"),
            "the AGB's own copy is missing: {output}"
        );
        for elsewhere in ["Hansabibliothek · Erwachsenenbereich", "Roman Schlink"] {
            assert!(
                !output.contains(elsewhere),
                "a copy of another branch is listed: {output}"
            );
        }
    }

    /// The copies the branch narrowing dropped are counted and their houses named. A
    /// shorter list is otherwise indistinguishable from a library that holds that much,
    /// and CLAUDE.md forbids reporting a record as having shelfmarks it does not have in
    /// either direction.
    #[test]
    fn the_copies_a_branch_narrowed_away_are_stated() {
        let agb = branch("AGB", "DE-609", "SIG00036", "Amerika-Gedenkbibliothek");
        let output = rendered_show(&vorleser_voebb(), std::slice::from_ref(&agb));
        // Asserted on the unwrapped text: the sentence is longer than the terminal and is
        // now continued on a second line (round 2, §3.4). What this test is about is that
        // it is *said*, not where it breaks.
        assert!(
            one_line(&output).contains(
                "3 more copies of this library stand at other branches: \
                 Marzahn-Hellersdorf: Bezirkszentralbibliothek · Mitte: Hansabibliothek · \
                 Reinickendorf: Bibliothek Frohnau"
            ),
            "{output}"
        );

        // An institution drops nothing, so it says nothing.
        let whole = rendered_show(
            &vorleser_voebb(),
            &[institution("VOEBB", "DE-609", "Berlin VÖBB/ZLB")],
        );
        assert!(!whole.contains("more copies of this library"), "{whole}");
    }

    /// A terminal too narrow for anything must still not panic — the copy lines lay out
    /// through the same table as before, and the two new lines of this package (the
    /// narrowed-away copies and the branch that could not be applied) are plain lines that
    /// overflow rather than index into their own text.
    #[test]
    fn a_branch_answer_survives_a_twenty_column_terminal() {
        let agb = branch("AGB", "DE-609", "SIG00036", "Amerika-Gedenkbibliothek");
        let narrow = rendered_show_at(&vorleser_voebb(), std::slice::from_ref(&agb), 20);
        assert!(narrow.contains("L 248 Schlin 50 p"), "{narrow}");
        // Unwrapped: at twenty columns the sentence is broken after nearly every word,
        // which is the correct answer and not a claim this test is making.
        assert!(
            one_line(&narrow).contains("more copies of this library"),
            "{narrow}"
        );
    }

    /// §1.8: `0 of 2 available` next to two lines reading "status not confirmed" is a
    /// counting claim about copies nobody counted. The denominator is the copies that
    /// stated a status, and with fewer than two of those there is no fraction to print.
    #[test]
    fn a_count_is_only_printed_over_copies_that_stated_a_status() {
        let mut record = prozess();
        record.holdings = vec![holding(
            "DE-30",
            "BBAW Berlin",
            "Berlin-Brandenburgische Akademie der Wissenschaften",
            Status::Unknown,
            vec![
                item("Akademiebibliothek", "Zo 1000 - 12,5,1", Status::Unknown),
                item("Akademiebibliothek", "Zo 1000 - 12,5,1=a", Status::Unknown),
            ],
        )];
        let unstated = rendered_show(&record, &[]);
        assert!(
            !unstated.contains("available"),
            "nothing was said about either copy: {unstated}"
        );
        assert_eq!(unstated.matches("status not confirmed").count(), 2);

        // Two copies that did state something are counted, and the third that did not is
        // in neither number.
        record.holdings[0].items[0].status = Status::Available;
        record.holdings[0].items[1].status = Status::Unavailable;
        record.holdings[0]
            .items
            .push(item("Magazin", "Zo 1000 - 12,5,2", Status::Unknown));
        let stated = rendered_show(&record, &[]);
        assert!(stated.contains("· 1 of 2 available"), "{stated}");
    }

    /// §1.9: the availability service answered `red` / `not available` and said nothing
    /// about a loan. For an online resource with a full-text link, "on loan" is a fact
    /// this tool would have invented.
    #[test]
    fn an_online_resource_is_never_reported_as_on_loan() {
        let mut record = prozess();
        record.holdings = vec![holding(
            "DE-517",
            "UP Potsdam",
            "Universität Potsdam",
            Status::Unavailable,
            vec![item("", "", Status::Unavailable)],
        )];
        assert!(
            rendered_show(&record, &[]).contains("on loan"),
            "a printed book that is not in is out"
        );

        record.format = Format::Ebook;
        record.online = true;
        let electronic = rendered_show(&record, &[]);
        assert!(electronic.contains("currently unavailable"), "{electronic}");
        assert!(
            !electronic.contains("on loan"),
            "nobody borrowed the DOI: {electronic}"
        );
    }

    /// The same rule on the search path, where the copy lines are the block's.
    #[test]
    fn a_copy_line_of_an_online_record_uses_the_neutral_wording() {
        let (mut result, locations) = vorleser();
        for record in &mut result.records {
            record.format = Format::Ebook;
            record.online = true;
            for holding in &mut record.holdings {
                for item in &mut holding.items {
                    item.status = Status::Unavailable;
                }
            }
        }
        let output = rendered(&result, &locations);
        assert!(output.contains("currently unavailable"), "{output}");
        // And the legend follows: nothing in this output can be lent, so glossing `○` as
        // "on loan" would contradict every line under it.
        assert!(!output.contains("on loan"), "{output}");
        assert!(output.contains("○  currently unavailable"), "{output}");
    }

    /// Round 3, §1.5: the legend **defines** the symbols, so it decides over the records
    /// that wear one — not over the page. Six hits of which two were online resources
    /// glossed `○` as "on loan" while both lines wearing it read "currently unavailable":
    /// the fact round 2 took out of the copy line, back in its definition.
    #[test]
    fn the_legend_glosses_a_symbol_over_the_records_that_wear_it() {
        let (mut result, locations) = vorleser();
        // The Stabi hit becomes an e-book nobody can reach. The AGB one stays a book that
        // is out, so `○` stands over one of each and the page is the mixed case.
        let stabi = result
            .records
            .iter_mut()
            .find(|record| record.id.as_str() == "almafu_BV010111222")
            .expect("the fixture holds the Stabi record");
        stabi.format = Format::Ebook;
        stabi.online = true;
        for holding in &mut stabi.holdings {
            holding.summary = Status::Unavailable;
            for item in &mut holding.items {
                item.status = Status::Unavailable;
            }
        }

        let output = rendered(&result, &locations);
        assert!(output.contains("○  currently unavailable"), "{output}");
        assert!(
            !output.contains("○  on loan"),
            "no line wearing the symbol says it: {output}"
        );
        // The neutral legend costs the page nothing: the AGB copy keeps the sharper
        // wording of its own record, which is what says which of the two is which.
        assert!(
            output.contains("Schlink              on loan"),
            "the copy that is out is still out: {output}"
        );
    }

    /// The other half of the same rule: a `○` that is only ever a copy on loan keeps the
    /// lending voice, and a page whose online resource wears a different symbol never
    /// enters the question.
    #[test]
    fn a_page_whose_unavailable_records_can_all_be_lent_keeps_the_lending_voice() {
        let (mut result, locations) = vorleser();
        // Available, and online: it wears `●`, which both voices gloss the same way.
        let stabi = result
            .records
            .iter_mut()
            .find(|record| record.id.as_str() == "almafu_BV010111222")
            .expect("the fixture holds the Stabi record");
        stabi.format = Format::Ebook;
        stabi.online = true;

        let output = rendered(&result, &locations);
        assert!(output.contains("○  on loan"), "{output}");
    }

    /// §1.3: a block a window filter emptied keeps its total. `no results` denied 2005
    /// hits that the footer named three lines further down — the one arm of the heading
    /// that could be a lie.
    #[test]
    fn a_block_emptied_by_the_window_filter_keeps_its_total() {
        let (mut result, mut locations) = vorleser();
        locations.push(institution("TU", "DE-83", "TU Berlin"));
        result.at.push(at("TU", "DE-83", 2005, Engine::Kobv, &[]));
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 0,
            filtered: true,
            undelivered: 0,
            before_available: Some(0),
        };

        let output = rendered(&result, &locations);
        assert!(
            output.contains("TU Berlin · 2005 results · none of the 50 fetched records matched\n"),
            "{output}"
        );
        assert!(!output.contains("TU Berlin · no results"), "{output}");
        assert!(
            !output.contains("none available now"),
            "the filter emptied the page before a status was asked for: {output}"
        );
    }

    /// The prose footnote about the window is dropped where the engine already stated the
    /// same limitation as a tagged note: one limitation, one sentence.
    #[test]
    fn the_empty_window_filter_is_not_said_twice() {
        let (mut result, locations) = vorleser();
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 0,
            filtered: true,
            undelivered: 0,
            before_available: None,
        };
        let bare = rendered(&result, &locations);
        assert!(
            bare.contains("note: the filters saw the 50 fetched records"),
            "without the note the prose still has to say it: {bare}"
        );

        result.notes = vec![Note::new(
            note_kinds::WINDOW_FILTER_EMPTY,
            "none of the 50 fetched records matched --format map — the filter runs over the \
             fetched window, so this is not a statement about the whole result",
        )];
        let tagged = rendered(&result, &locations);
        assert!(tagged.contains("--format map"), "{tagged}");
        assert!(
            !tagged.contains("the filters saw the 50 fetched records"),
            "the note says it; the prose said it again: {tagged}"
        );
    }

    /// §2.1: with `--no-availability` a KOBV branch cannot be applied at all — the branch
    /// of a copy is named in the availability answer and nowhere else — so the block is
    /// the house's, and the heading has to say the house rather than claim the branch.
    #[test]
    fn a_branch_that_could_not_be_applied_names_the_house() {
        let (mut result, _) = vorleser();
        let mut germanistik = branch(
            "DE-11-105",
            "DE-11",
            "HUB00028",
            "Zweigbibliothek Germanistik/Skandinavistik",
        );
        germanistik.engine = Engine::Kobv;
        germanistik.display = "Zweigbibliothek Germanistik/Skandinavistik (HU Berlin)".to_owned();
        let locations = vec![germanistik];
        result.at = vec![{
            let mut block = at(
                "DE-11-105",
                "DE-11",
                0,
                Engine::Kobv,
                &["almahu_BV011234567", "almahu_BV019876543"],
            );
            block.total = None;
            block
        }];

        let applied = rendered(&result, &locations);
        assert!(
            applied.starts_with("Zweigbibliothek Germanistik/Skandinavistik (HU Berlin) · showing"),
            "with copies the branch is answered for: {applied}"
        );

        result.availability = AvailabilityMode::Skipped;
        let unapplied = rendered(&result, &locations);
        assert!(
            unapplied.starts_with("HU Berlin (branch not applied) · showing 2"),
            "{unapplied}"
        );
    }

    /// §2.9: `select::sort` runs over the whole anchored window and `take_page` cuts the
    /// page out of the ordered set afterwards, so the note used to understate its own
    /// work — and to contradict the heading above it, which counted the same window.
    #[test]
    fn the_sort_note_names_the_window_it_ordered() {
        let (mut result, locations) = vorleser();
        result.sort = SortSpec {
            by: SortKey::Year,
            scope: SortScope::Fetched,
        };
        result.total = Some(258);
        result.window = WindowInfo {
            fetched: 50,
            after_filter: 32,
            filtered: true,
            undelivered: 0,
            before_available: None,
        };
        let output = rendered(&result, &locations);
        assert!(
            output.contains(
                "note: --sort ordered the 32 records in this window, not all 258 \
                             results"
            ),
            "{output}"
        );

        // A window the sort saw whole is no limitation, so there is nothing to say.
        result.total = Some(32);
        assert!(
            !rendered(&result, &locations).contains("--sort ordered"),
            "the sort was complete"
        );
    }

    /// `--sort availability` is the exception: statuses exist only for the records
    /// availability was fetched for, so the ordering that decides the output is the second
    /// one, over the page.
    #[test]
    fn a_sort_by_availability_names_the_page_it_ordered() {
        let (mut result, locations) = vorleser();
        result.sort = SortSpec {
            by: SortKey::Availability,
            scope: SortScope::Fetched,
        };
        result.total = Some(258);
        result.window.after_filter = 32;
        let output = rendered(&result, &locations);
        assert!(
            output.contains("note: --sort ordered the 5 records shown, not all 258 results"),
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
            pad_right(label(Status::Available, Voice::Copy), STATUS_COLUMN)
        );
        assert!(output.contains(&expected), "{output:?}");
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
            Style::plain(WIDE),
        )
        .expect("bytes");
        let output = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            output,
            "774 results, but none of the 50 fetched records matched --format video\n\
             narrow the search itself (--title, --author) so the filter has more to work on\n"
        );
    }

    /// `show` reads a note the same way `search` does — one rule, one function.
    ///
    /// Both halves of the rule in one place, because the seam is what is being tested: the
    /// answer is a single record, so a note about *it* names nothing (the reader is looking
    /// at the id), and a note that names something else is listed rather than swallowed.
    /// The notes are built here, not taken from `ShowResult`, so that a reworded message
    /// upstream cannot turn this into a test of the wording.
    #[test]
    fn a_show_note_names_records_only_where_they_add_something() {
        let record = prozess();
        let rendered = |notes: &[Note]| {
            let mut out = Vec::new();
            show(&record, &[], notes, &mut out, Style::plain(WIDE)).expect("bytes");
            String::from_utf8(out).expect("UTF-8")
        };

        let about_itself = rendered(&[note_about(
            note_kinds::LOAN_WITHOUT_DUE_DATE,
            "a copy is on loan and the service states no due date",
            &[record.id.as_str()],
        )]);
        assert!(
            note_lines(&about_itself, "record").is_empty(),
            "{about_itself}"
        );
        assert!(
            note_lines(&about_itself, "all ").is_empty(),
            "{about_itself}"
        );

        let about_another = rendered(&[note_about(
            note_kinds::AVAILABILITY_NOT_STATED,
            "the availability service holds nothing for this record",
            &["almahu_BV011234567"],
        )]);
        assert_eq!(
            note_records(&about_another, "record:"),
            "record: almahu_BV011234567",
            "{about_another}"
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
