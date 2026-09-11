//! Column layout.
//!
//! Widths are computed with `unicode-width` on the **uncoloured** text: escape sequences
//! have no width, and CJK and combining characters do not have the width of their byte
//! count. Getting either wrong misaligns every non-Latin record in the catalogue.
//!
//! What may be truncated is a policy decision, not a layout one: titles are truncated,
//! **record ids never are** — a shortened id cannot be typed back into `show`.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::render::Style;

/// The width to lay out for when there is no terminal and `COLUMNS` says nothing.
///
/// A fixed number rather than "unlimited": a piped or redirected invocation must produce
/// the same bytes on every machine, and an agent that diffs two runs should not see the
/// launching terminal in the diff.
///
/// **Deliberately generous.** The other audience of this tool captures the human output
/// of a piped invocation, and a title cut down to a terminal-sized column is information
/// destroyed for no reason — nothing is watching those columns. 160 is wide enough that
/// the example layouts print in full, and still finite so the output stays reproducible.
pub const DEFAULT_WIDTH: usize = 160;

/// The narrowest width this tool lays out for.
///
/// Below it nothing can be laid out honestly: a record id runs to 24 columns and is never
/// shortened, the status column is [`crate::render::human`]'s widest wording at 20, and the
/// two of them plus an indent already spend more than 40. A width of 20 would therefore not
/// produce a narrow layout but a column of ellipses.
///
/// So a smaller width is **not** made smaller still: the layout stays at 40 and the
/// terminal wraps what does not fit, which loses nothing — where the width matters, the
/// text is word-wrapped by [`wrap`] rather than cut. The number is a floor on the layout,
/// never a claim about the terminal.
pub const MIN_WIDTH: usize = 40;

/// The environment variable that overrides the width where there is no terminal to ask.
///
/// The name shells already use for this, so `COLUMNS=100 blibs search …` needs no flag of
/// its own. It is consulted *after* the terminal, never instead of it: a real terminal
/// knows its own width, and shells are not consistent about exporting the variable.
const COLUMNS_VAR: &str = "COLUMNS";

/// The marker put where text was cut away.
const ELLIPSIS: char = '…';

/// The width to lay out for: the terminal's own, else `COLUMNS`, else [`DEFAULT_WIDTH`].
pub fn terminal_columns() -> usize {
    let columns = std::env::var(COLUMNS_VAR).ok();
    columns_from(
        terminal_size::terminal_size().map(|(width, _)| usize::from(width.0)),
        columns.as_deref(),
    )
}

/// The width decision itself, with both inputs handed in.
///
/// Split out from [`terminal_columns`] because that one reads the process environment,
/// which a test cannot change safely. The order is the whole rule: a terminal knows its
/// width, `COLUMNS` is what the caller says when there is none, and [`DEFAULT_WIDTH`] is
/// the answer when neither speaks.
///
/// A width of zero is treated as "not stated" from either source — some terminals report
/// zero while resizing, and a `COLUMNS=0` layout would be nothing but ellipses.
///
/// A width that *is* stated but is narrower than [`MIN_WIDTH`] is raised to it: see there
/// for why a narrower layout would destroy text rather than fit it.
pub fn columns_from(size: Option<usize>, env: Option<&str>) -> usize {
    size.filter(|width| *width > 0)
        .or_else(|| {
            env.and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|width| *width > 0)
        })
        .map_or(DEFAULT_WIDTH, |width| width.max(MIN_WIDTH))
}

/// Display width of text in terminal columns.
///
/// Not `str::len` and not the character count: a CJK ideograph occupies two columns, a
/// combining accent none. Both occur in this catalogue.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Shorten text to `width` columns, marking the cut with `…`.
///
/// Never cuts inside a character, and never separates a combining mark from the
/// character it belongs to: zero-width marks cost nothing, so they are always taken
/// along with their base. Text that already fits is returned unchanged — the ellipsis is
/// only ever added where something was actually removed.
pub fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    // One column is spent on the ellipsis itself.
    let budget = width - 1;
    let mut used = 0;
    let mut out = String::with_capacity(text.len().min(width * 4));
    for c in text.chars() {
        let step = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + step > budget {
            break;
        }
        used += step;
        out.push(c);
    }
    // The cut usually lands mid-word; a space in front of the ellipsis reads like a
    // second cut mark and wastes a column that the text could have used.
    let mut out = out.trim_end().to_owned();
    out.push(ELLIPSIS);
    out
}

/// The list separator this renderer joins values with, and the one token a wrapped line
/// may never end on: a line closing with `·` reads like a cut.
const SEPARATOR: &str = "·";

/// Break text into lines of at most `width` columns, at spaces.
///
/// The counterpart of [`truncate`] for text that is a sentence rather than a cell: a note,
/// a library name, a list of subject headings. Nothing is ever removed — a line that does
/// not fit is continued, not cut — which is why this and not truncation is what the lines
/// outside a table use.
///
/// **A word wider than `width` is never broken.** A URL and a record id have to survive
/// being copied out of the terminal, and a break would put a space in the middle of one;
/// such a word gets its own line and overflows it, visibly.
///
/// A lone `·` is carried to the next line together with the value behind it, so a
/// continuation reads `· Roman` and no line ends on a separator with nothing after it.
///
/// Always at least one line, so a caller can write the result unconditionally. A `width`
/// of zero means "do not wrap".
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    wrap_hanging(text, width, width)
}

/// [`wrap`] where the continuation lines have a width of their own.
///
/// For a hanging indent: the continuations start further right and therefore have fewer
/// columns left, and passing one width for both would make the right margin ragged by
/// exactly the indent.
pub fn wrap_hanging(text: &str, first: usize, rest: usize) -> Vec<String> {
    if first == 0 || rest == 0 || display_width(text) <= first {
        return vec![text.to_owned()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for unit in units(text) {
        let width = if lines.is_empty() { first } else { rest };
        if !line.is_empty() && display_width(&line) + 1 + display_width(&unit) > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&unit);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The units [`wrap`] may put a line break between: whitespace-separated words, except
/// that a lone separator is glued to the word behind it.
fn units(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut units: Vec<String> = Vec::new();
    let mut pending: Option<&str> = None;
    for word in words {
        match pending.take() {
            Some(separator) => units.push(format!("{separator} {word}")),
            None if word == SEPARATOR => pending = Some(word),
            None => units.push(word.to_owned()),
        }
    }
    // A separator with nothing behind it is text like any other; dropping it would remove
    // a character the caller wrote.
    if let Some(separator) = pending {
        units.push(separator.to_owned());
    }
    units
}

/// Pad text on the right to `width` columns. Text that is already wider is returned
/// unchanged — padding never truncates, so the two policies stay separable.
pub fn pad_right(text: &str, width: usize) -> String {
    let mut out = text.to_owned();
    for _ in display_width(text)..width {
        out.push(' ');
    }
    out
}

/// One cell: text plus how to paint it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The text, uncoloured. This is what widths are measured on.
    pub text: String,
    /// How to paint it.
    pub style: anstyle::Style,
    /// Whether this cell may be shortened when the row does not fit.
    pub truncatable: bool,
}

impl Cell {
    /// A plain, truncatable cell.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: anstyle::Style::new(),
            truncatable: true,
        }
    }

    /// A cell that must be printed in full — a record id, a shelfmark.
    #[must_use]
    pub fn whole(mut self) -> Self {
        self.truncatable = false;
        self
    }

    /// Paint this cell.
    #[must_use]
    pub fn styled(mut self, style: anstyle::Style) -> Self {
        self.style = style;
        self
    }

    /// Display width of the text, in terminal columns.
    pub fn width(&self) -> usize {
        display_width(&self.text)
    }
}

/// How one column is sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Sized to its widest cell, and the first to give way when the row does not fit.
    Auto,
    /// Always exactly this wide, whatever the content. This is what reproduces the
    /// example output, whose columns do not depend on the records that happen to be on
    /// the page.
    Fixed(usize),
    /// Sized to its widest cell but never narrower than `ideal`, giving way down to
    /// `min` but no further — the title column, which absorbs the shortfall for
    /// everything else.
    ///
    /// The two numbers are what a fixed width cannot express at once: `ideal` is the
    /// width the column keeps when everything in it is shorter, which is what pins the
    /// example layouts to their stated columns; content wider than that widens the
    /// column instead of being cut, as long as the row has the room. **Never pre-shorten
    /// a cell to `ideal` at the call site** — that caps the column at the constant even
    /// in a wide terminal, and lets a narrowed column cut a value that was complete.
    Flex {
        /// The width the column takes when no cell in it is wider.
        ideal: usize,
        /// The narrowest this column may become when the row does not fit.
        min: usize,
    },
    /// **At least** this wide, and wider where a cell needs it — never shortened and never
    /// shrunk. For a value that may not be cut and may not be guessed at: a shelfmark, a
    /// status, a year.
    ///
    /// The variant exists because [`Column::Fixed`] is a promise the content can break. A
    /// fixed column neither grows nor cuts, so **one** cell wider than the stated width
    /// pushes the rest of *its* row to the right and leaves every other row of the block
    /// standing where it was — measured on `voebb_SAK34906286` at 120 columns, where an
    /// 18-character shelfmark in a 17-column shelfmark column moved that one copy's status
    /// two columns out of line, over 23 copies. Either a column may cut its cells, and then
    /// it must, or it may not, and then its width has to follow its longest cell.
    Least(usize),
    /// Sized to its widest cell, never shortened and never padded. The record id: a
    /// shortened id cannot be typed back into `show`, so this column overflows the
    /// terminal rather than lie.
    Last,
}

impl Column {
    /// The narrowest this column may become, or `None` if it may not shrink at all.
    fn floor(self) -> Option<usize> {
        match self {
            Column::Auto => Some(1),
            Column::Flex { min, .. } => Some(min),
            Column::Fixed(_) | Column::Least(_) | Column::Last => None,
        }
    }
}

/// The column plan of a table: how wide each column is, and what separates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    columns: Vec<Column>,
    gap: usize,
}

/// Blank columns between two laid-out columns; [`Layout::with_gap`] changes it.
pub const DEFAULT_GAP: usize = 2;

impl Layout {
    /// A layout from explicit column rules, one per column, separated by
    /// [`DEFAULT_GAP`]. Rows wider than the plan are laid out as [`Column::Auto`];
    /// [`Layout::with_gap`] changes the separation.
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            gap: DEFAULT_GAP,
        }
    }

    /// `count` columns sized to their content, the last one [`Column::Last`] — never
    /// shortened, because the last column is where the record id goes and a shortened id
    /// cannot be typed back into `show`. `count` of zero is an empty plan, not an error.
    pub fn auto(count: usize) -> Self {
        let mut columns = vec![Column::Auto; count];
        if let Some(last) = columns.last_mut() {
            *last = Column::Last;
        }
        Self::new(columns)
    }

    /// Change the gap between columns. Zero is right when the widths already include
    /// their own padding, which is how the examples are written.
    #[must_use]
    pub fn with_gap(mut self, gap: usize) -> Self {
        self.gap = gap;
        self
    }

    /// The rule for one column; columns beyond the plan are [`Column::Auto`].
    fn column(&self, index: usize) -> Column {
        self.columns.get(index).copied().unwrap_or(Column::Auto)
    }

    /// How many columns this layout lays out for the given rows.
    fn count(&self, rows: &[Vec<Cell>]) -> usize {
        let widest = rows.iter().map(Vec::len).max().unwrap_or(0);
        self.columns.len().max(widest)
    }

    /// The width of each column, given the rows and the width available.
    ///
    /// Columns start at their natural width — [`Column::Fixed`] at its stated one,
    /// [`Column::Flex`] and [`Column::Least`] at their widest cell but never below the
    /// stated one, every other at its widest cell — and are then shrunk, widest first,
    /// until the row fits. Widest
    /// first so that one long title gives way before four short columns do.
    /// [`Column::Fixed`] and [`Column::Last`] never give way, so a row can still overflow
    /// a very narrow terminal; that is preferable to a cut record id.
    pub fn widths(&self, rows: &[Vec<Cell>], available: usize) -> Vec<usize> {
        let count = self.count(rows);
        let mut widths: Vec<usize> = (0..count)
            .map(|index| match self.column(index) {
                Column::Fixed(width) => width,
                Column::Least(least) => natural_width(rows, index).max(least),
                Column::Flex { ideal, .. } => natural_width(rows, index).max(ideal),
                _ => natural_width(rows, index),
            })
            .collect();
        self.shrink(&mut widths, available);
        widths
    }

    /// Take columns away from the widest shrinkable column until the row fits.
    fn shrink(&self, widths: &mut [usize], available: usize) {
        while self.total(widths) > available {
            let Some(widest) = self.widest_shrinkable(widths) else {
                // Nothing left that may give way: the row overflows, openly.
                return;
            };
            widths[widest] -= 1;
        }
    }

    /// The laid-out width of a whole row, gaps included.
    fn total(&self, widths: &[usize]) -> usize {
        let gaps = self.gap * widths.len().saturating_sub(1);
        widths.iter().sum::<usize>() + gaps
    }

    /// The widest column that may still give way, if any.
    fn widest_shrinkable(&self, widths: &[usize]) -> Option<usize> {
        widths
            .iter()
            .enumerate()
            .filter(|(index, width)| {
                self.column(*index)
                    .floor()
                    .is_some_and(|floor| **width > floor)
            })
            .max_by_key(|(_, width)| **width)
            .map(|(index, _)| index)
    }

    /// Render one row into the given column widths.
    ///
    /// A cell is shortened only if it is both truncatable and in a column that may be
    /// shortened; anything else is written in full and pushes the rest of its row to the
    /// right. Trailing space is trimmed, so a short row does not carry invisible padding
    /// into a diff.
    pub fn render_row(&self, cells: &[Cell], widths: &[usize], style: Style) -> String {
        let mut out = String::new();
        for (index, width) in widths.iter().enumerate() {
            if index > 0 {
                out.push_str(&" ".repeat(self.gap));
            }
            let Some(cell) = cells.get(index) else {
                out.push_str(&" ".repeat(*width));
                continue;
            };
            let text = if cell.truncatable && self.column(index).floor().is_some() {
                truncate(&cell.text, *width)
            } else {
                cell.text.clone()
            };
            let padding = width.saturating_sub(display_width(&text));
            out.push_str(&style.paint(&text, cell.style));
            out.push_str(&" ".repeat(padding));
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// The widest cell in one column, in terminal columns.
fn natural_width(rows: &[Vec<Cell>], index: usize) -> usize {
    rows.iter()
        .filter_map(|row| row.get(index))
        .map(Cell::width)
        .max()
        .unwrap_or(0)
}

/// Removes SGR sequences (`ESC [ … m`), which is all `anstyle` emits.
///
/// Shared by the tests of both render modules: the claim that colour never changes the
/// layout is checked by stripping it and comparing.
#[cfg(test)]
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        for escape in chars.by_ref() {
            if escape == 'm' {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstyle::{AnsiColor, Color};

    fn green() -> anstyle::Style {
        anstyle::Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)))
    }

    /// Lay out and render a block of rows, exactly the way [`crate::render::human`] does
    /// it: size the columns once over all the rows, then render each row into those
    /// widths. There is no `Table` type — a renderer that keeps its rows in a struct
    /// would only be a second place for the widths to be computed.
    fn render(layout: &Layout, rows: &[Vec<Cell>], style: Style) -> String {
        let widths = layout.widths(rows, style.width());
        rows.iter()
            .map(|row| layout.render_row(row, &widths, style))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The column that may not cut its cells follows its longest one, and gives no width
    /// back when the row is tight.
    ///
    /// [`Column::Fixed`] does neither, which is the whole bug this variant exists for: one
    /// cell wider than the stated width moved its own row two columns to the right and left
    /// the rest of the block standing.
    #[test]
    fn a_least_column_grows_with_its_content_and_never_shrinks() {
        let layout = Layout::new(vec![
            Column::Flex { ideal: 20, min: 8 },
            Column::Least(16),
            Column::Last,
        ]);
        let short = vec![vec![
            Cell::new("Mitte"),
            Cell::new("EDV 945,7"),
            Cell::new("in"),
        ]];
        assert_eq!(
            layout.widths(&short, 60)[1],
            16,
            "the stated width is a floor"
        );

        let long = vec![vec![
            Cell::new("Mitte"),
            Cell::new("Konsolenspiel Fire"),
            Cell::new("in"),
        ]];
        assert_eq!(
            layout.widths(&long, 60)[1],
            display_width("Konsolenspiel Fire")
        );
        // Not even when there is no room: the shortfall comes out of the flexible column,
        // and a cell here is written whole whatever the width.
        assert_eq!(
            layout.widths(&long, 24)[1],
            display_width("Konsolenspiel Fire")
        );
        assert!(render(&layout, &long, Style::plain(24)).contains("Konsolenspiel Fire"));
    }

    /// A terminal is asked first and believed, whatever `COLUMNS` claims.
    #[test]
    fn a_terminal_width_wins_over_the_environment() {
        assert_eq!(columns_from(Some(80), Some("200")), 80);
        assert_eq!(columns_from(Some(80), None), 80);
    }

    /// Without a terminal, `COLUMNS` is what the caller says. Whitespace around the
    /// number is what `COLUMNS=" 90 "` and a shell that pads it produce.
    #[test]
    fn columns_is_used_when_there_is_no_terminal() {
        assert_eq!(columns_from(None, Some("90")), 90);
        assert_eq!(columns_from(None, Some(" 90 ")), 90);
    }

    /// Neither source, or neither usable: the generous default, so that a captured
    /// human output carries whole titles rather than ellipses.
    #[test]
    fn an_unusable_width_falls_back_to_the_default() {
        assert_eq!(columns_from(None, None), DEFAULT_WIDTH);
        assert_eq!(columns_from(None, Some("")), DEFAULT_WIDTH);
        assert_eq!(columns_from(None, Some("wide")), DEFAULT_WIDTH);
        assert_eq!(columns_from(None, Some("-10")), DEFAULT_WIDTH);
        assert_eq!(
            columns_from(None, Some("0")),
            DEFAULT_WIDTH,
            "0 is no width"
        );
        assert_eq!(
            columns_from(Some(0), Some("90")),
            90,
            "a terminal reporting zero has not stated a width"
        );
        assert_eq!(columns_from(Some(0), None), DEFAULT_WIDTH);
    }

    /// The default is wide enough that the two example layouts print in full: it exists
    /// so that a captured human output carries whole titles rather than ellipses.
    #[test]
    fn the_default_width_does_not_truncate_the_example_layouts() {
        let row = vec![
            Cell::new("1 ●"),
            Cell::new("Der Prozess : Roman einer Verwandlung"),
            Cell::new("Kafka, Franz"),
            Cell::new("1953"),
            Cell::new("almafu_BV008885798").whole(),
        ];
        let layout = Layout::auto(row.len());
        let widths = layout.widths(std::slice::from_ref(&row), DEFAULT_WIDTH);
        let rendered = layout.render_row(&row, &widths, Style::plain(DEFAULT_WIDTH));
        assert!(!rendered.contains(ELLIPSIS), "{rendered}");
    }

    #[test]
    fn width_is_columns_not_bytes_and_not_characters() {
        assert_eq!(display_width("Prozess"), 7);
        // Umlauts are one column and two bytes.
        assert_eq!(display_width("Der Prozeß"), 10);
        assert_eq!("Der Prozeß".len(), 11);
        // CJK ideographs are two columns each.
        assert_eq!(display_width("審判"), 4);
        assert_eq!(display_width("審判 Kafka"), 10);
    }

    /// A decomposed umlaut is a base plus a zero-width mark: one column, two characters.
    #[test]
    fn combining_marks_have_no_width() {
        let decomposed = "Prozeu\u{308}";
        assert_eq!(decomposed.chars().count(), 7);
        assert_eq!(display_width(decomposed), 6);
    }

    #[test]
    fn text_that_fits_is_returned_untouched() {
        assert_eq!(truncate("Der Prozess", 11), "Der Prozess");
        assert_eq!(truncate("Der Prozess", 40), "Der Prozess");
        assert!(!truncate("Der Prozess", 11).contains('…'));
    }

    #[test]
    fn text_that_does_not_fit_is_cut_and_marked() {
        let cut = truncate("Der Prozess : Roman", 12);
        assert_eq!(cut, "Der Prozess…");
        assert_eq!(display_width(&cut), 12);
    }

    /// The cut respects the *column* width, so a wide character is never halved.
    #[test]
    fn a_wide_character_is_never_cut_in_half() {
        let cut = truncate("審判審判審判", 5);
        assert_eq!(cut, "審判…");
        assert_eq!(display_width(&cut), 5);
        assert!(cut.chars().all(|c| c != '\u{fffd}'));
    }

    /// A combining mark costs nothing, so it is always taken along with its base rather
    /// than orphaned at the cut.
    #[test]
    fn a_combining_mark_stays_with_its_base() {
        let cut = truncate("abu\u{308}cdef", 4);
        assert_eq!(cut, "abu\u{308}…");
        assert_eq!(display_width(&cut), 4);
    }

    /// The ellipsis replaces the cut, not a space: `"Der Prozess …"` reads like two
    /// marks and wastes a column.
    #[test]
    fn no_space_is_left_in_front_of_the_ellipsis() {
        assert_eq!(
            truncate("Der Prozess : Roman einer Verwandlung", 13),
            "Der Prozess…"
        );
        assert!(display_width(&truncate("Der Prozess : Roman", 13)) <= 13);
    }

    #[test]
    fn truncating_to_nothing_yields_nothing() {
        assert_eq!(truncate("Der Prozess", 0), "");
        assert_eq!(truncate("", 0), "");
        assert_eq!(truncate("Der Prozess", 1), "…");
    }

    #[test]
    fn padding_measures_columns_and_never_shortens() {
        assert_eq!(pad_right("審判", 6), "審判  ");
        assert_eq!(display_width(&pad_right("審判", 6)), 6);
        assert_eq!(pad_right("Der Prozess", 4), "Der Prozess");
    }

    #[test]
    fn a_cell_measures_its_uncoloured_text() {
        let cell = Cell::new("審判").styled(green());
        assert_eq!(cell.width(), 4);
        assert!(cell.truncatable);
        assert!(!Cell::new("almafu_BV008885798").whole().truncatable);
    }

    /// The `--at` example, character for character. The widths there include their own
    /// padding, so the gap is zero.
    #[test]
    fn the_grouped_example_row_is_reproducible() {
        let layout = Layout::new(vec![
            Column::Fixed(3),
            Column::Fixed(36),
            Column::Fixed(20),
            Column::Fixed(7),
            Column::Last,
        ])
        .with_gap(0);
        let row = vec![
            Cell::new("●"),
            Cell::new("Der Vorleser : Roman"),
            Cell::new("Schlink, Bernhard"),
            Cell::new("1997"),
            Cell::new("almahu_BV011234567").whole(),
        ];
        let widths = layout.widths(std::slice::from_ref(&row), 100);
        let rendered = format!("  {}", layout.render_row(&row, &widths, Style::plain(100)));
        assert_eq!(
            rendered,
            "  ●  Der Vorleser : Roman                Schlink, Bernhard   1997   almahu_BV011234567"
        );
    }

    /// The flat example, which uses a running number and a wider title column.
    #[test]
    fn the_flat_example_row_is_reproducible() {
        let layout = Layout::new(vec![
            Column::Fixed(5),
            Column::Fixed(40),
            Column::Fixed(17),
            Column::Fixed(6),
            Column::Last,
        ])
        .with_gap(0);
        let row = vec![
            Cell::new("1 ●"),
            Cell::new("Der Prozess"),
            Cell::new("Kafka, Franz"),
            Cell::new("1953"),
            Cell::new("almafu_BV008885798").whole(),
        ];
        let widths = layout.widths(std::slice::from_ref(&row), 100);
        let rendered = format!("  {}", layout.render_row(&row, &widths, Style::plain(100)));
        assert_eq!(
            rendered,
            "  1 ●  Der Prozess                             Kafka, Franz     1953  almafu_BV008885798"
        );
    }

    #[test]
    fn auto_columns_are_sized_to_their_widest_cell() {
        let rows = vec![
            vec![Cell::new("Kafka"), Cell::new("1953")],
            vec![Cell::new("Schlink, Bernhard"), Cell::new("1997")],
        ];
        assert_eq!(
            render(&Layout::auto(2), &rows, Style::plain(100)),
            "Kafka              1953\nSchlink, Bernhard  1997"
        );
    }

    /// The widest column gives way first, so one long title does not squeeze the rest.
    #[test]
    fn the_widest_column_gives_way_first() {
        let layout = Layout::new(vec![Column::Auto, Column::Auto, Column::Last]);
        let rows = vec![vec![
            Cell::new("Der Prozess : Roman einer Verwandlung"),
            Cell::new("Kafka"),
            Cell::new("almafu_BV008885798").whole(),
        ]];
        let rendered = render(&layout, &rows, Style::plain(40));
        assert_eq!(rendered, "Der Prozess…   Kafka  almafu_BV008885798");
        assert_eq!(
            display_width(&rendered),
            40,
            "the row fills the width exactly"
        );
    }

    /// A record id is never shortened, even when that means overflowing the terminal —
    /// a cut id cannot be typed back into `show`.
    #[test]
    fn the_last_column_overflows_rather_than_being_cut() {
        let layout = Layout::new(vec![Column::Flex { ideal: 4, min: 4 }, Column::Last]);
        let rows = vec![vec![
            Cell::new("Der Prozess : Roman"),
            Cell::new("almafu_BV008885798").whole(),
        ]];
        let rendered = render(&layout, &rows, Style::plain(20));
        assert_eq!(rendered, "Der…  almafu_BV008885798");
        assert!(rendered.ends_with("almafu_BV008885798"));
    }

    /// `Flex` stops at its minimum instead of collapsing to a bare ellipsis.
    #[test]
    fn a_flex_column_does_not_shrink_past_its_minimum() {
        let layout = Layout::new(vec![Column::Flex { ideal: 8, min: 8 }, Column::Last]);
        let rows = vec![vec![
            Cell::new("Der Prozess : Roman einer Verwandlung"),
            Cell::new("almafu_BV008885798").whole(),
        ]];
        let rendered = render(&layout, &rows, Style::plain(10));
        assert_eq!(rendered, "Der Pro…  almafu_BV008885798");
    }

    /// `ideal` is the width the column keeps when everything in it is shorter — that is
    /// what reproduces the example layouts, whose columns do not move with the records
    /// that happen to be on the page.
    #[test]
    fn a_flex_column_keeps_its_ideal_width_for_short_content() {
        let layout = Layout::new(vec![Column::Flex { ideal: 32, min: 12 }, Column::Last]);
        let rows = vec![vec![Cell::new("Kafka, Franz"), Cell::new("author").whole()]];
        assert_eq!(layout.widths(&rows, 100), vec![32, 6]);
    }

    /// Regression: content longer than `ideal` widens the column while the row has room.
    /// Pre-shortening the cell to the constant used to cap the column at it, so a value
    /// stayed cut in a 200-column terminal with everything else fitting four times over.
    #[test]
    fn a_flex_column_grows_past_its_ideal_when_there_is_room() {
        let layout = Layout::new(vec![
            Column::Flex { ideal: 32, min: 12 },
            Column::Fixed(12),
            Column::Last,
        ]);
        let name = "Enzensberger, Hans Magnus (1929-2022)";
        let rows = vec![vec![
            Cell::new(name),
            Cell::new("author"),
            Cell::new("GND 118530550").whole(),
        ]];
        let rendered = render(&layout, &rows, Style::plain(200));
        assert!(rendered.starts_with(name), "{rendered}");
        assert!(!rendered.contains(ELLIPSIS), "{rendered}");
    }

    /// Regression: a cell that fits its column is never marked as cut. A column shrunk
    /// below its `ideal` used to measure the padding baked into the cell as content, so
    /// a complete value came out with an ellipsis behind it and nothing missing.
    #[test]
    fn a_shrunken_flex_column_does_not_mark_a_cut_it_did_not_make() {
        let layout = Layout::new(vec![
            Column::Flex { ideal: 32, min: 12 },
            Column::Fixed(12),
            Column::Last,
        ]);
        let name = "Kafka, Franz (1883-1924)";
        let rows = vec![vec![
            Cell::new(name),
            Cell::new("author"),
            Cell::new("GND 118559230").whole(),
        ]];
        // Too narrow for the ideal width, wide enough for the text itself.
        let rendered = render(&layout, &rows, Style::plain(58));
        assert!(rendered.starts_with(name), "{rendered}");
        assert!(!rendered.contains(ELLIPSIS), "{rendered}");
    }

    /// The layout claim of the whole crate: colour is decoration. Strip the escapes and
    /// the coloured render is byte-identical to the plain one.
    #[test]
    fn colour_does_not_change_the_layout() {
        let layout = Layout::new(vec![Column::Auto, Column::Auto, Column::Last]);
        let rows = vec![
            vec![
                Cell::new("●").styled(green()),
                Cell::new("審判 : Roman"),
                Cell::new("almafu_BV008885798").whole(),
            ],
            vec![
                Cell::new("○").styled(green()),
                Cell::new("Der Prozeß"),
                Cell::new("b3kat_BV005550341").whole(),
            ],
        ];
        let plain = render(&layout, &rows, Style::plain(100));
        let coloured = render(&layout, &rows, Style::new(true, 100));
        assert_ne!(plain, coloured, "colour must actually have been written");
        assert_eq!(plain, strip_ansi(&coloured));
    }

    /// Widths come from the uncoloured text, so a painted cell claims no extra columns.
    #[test]
    fn a_painted_cell_is_measured_without_its_escapes() {
        let layout = Layout::auto(2);
        let rows = vec![vec![
            Cell::new("Kafka").styled(green()),
            Cell::new("1953").whole(),
        ]];
        assert_eq!(layout.widths(&rows, 100), vec![5, 4]);
    }

    #[test]
    fn a_short_row_is_padded_but_carries_no_trailing_space() {
        let rows = vec![
            vec![
                Cell::new("Kafka"),
                Cell::new("1953"),
                Cell::new("almafu_BV008885798"),
            ],
            vec![Cell::new("Schlink")],
        ];
        let rendered = render(&Layout::auto(3), &rows, Style::plain(100));
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[1], "Schlink");
        assert!(!lines[0].ends_with(' '));
    }

    #[test]
    fn no_rows_render_to_nothing() {
        assert_eq!(render(&Layout::auto(2), &[], Style::plain(100)), "");
    }

    /// A width the caller could not possibly lay out is raised to the floor instead of
    /// being obeyed: at 20 columns a record id alone overflows the line, and every
    /// truncatable cell would come out as a bare ellipsis.
    #[test]
    fn a_width_below_the_minimum_is_raised_to_it() {
        assert_eq!(columns_from(None, Some("20")), MIN_WIDTH);
        assert_eq!(columns_from(Some(20), None), MIN_WIDTH);
        assert_eq!(columns_from(Some(1), None), MIN_WIDTH);
        assert_eq!(
            columns_from(None, Some("40")),
            40,
            "the floor itself is a width like any other"
        );
        assert_eq!(columns_from(None, Some("90")), 90);
    }

    #[test]
    fn text_that_fits_is_one_line() {
        assert_eq!(wrap("Der Prozess", 20), vec!["Der Prozess"]);
        assert_eq!(wrap("", 20), vec![""]);
        assert_eq!(
            wrap("Der Prozess", 0),
            vec!["Der Prozess"],
            "a width of zero means: do not wrap"
        );
    }

    /// The point of wrapping rather than truncating: every word is still there.
    #[test]
    fn a_wrapped_line_loses_no_word() {
        let text = "the availability service holds no information for this record";
        let lines = wrap(text, 24);
        assert!(
            lines.iter().all(|line| display_width(line) <= 24),
            "{lines:?}"
        );
        assert_eq!(lines.join(" "), text);
        assert!(lines.len() > 1);
    }

    /// A URL and a record id must survive being copied out of the terminal, so a word
    /// wider than the line overflows it rather than being broken in two.
    #[test]
    fn a_word_wider_than_the_line_is_never_broken() {
        let lines = wrap(
            "Online https://d-nb.info/1234567890/04 (table of contents)",
            20,
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "https://d-nb.info/1234567890/04"),
            "{lines:?}"
        );
        assert_eq!(
            lines.join(" "),
            "Online https://d-nb.info/1234567890/04 (table of contents)"
        );
    }

    /// A line ending on `·` reads like a cut, so the separator travels with the value
    /// behind it.
    #[test]
    fn a_wrapped_list_never_ends_a_line_on_its_separator() {
        let lines = wrap("Deutsche Literatur · Roman · Prag · Gerichtsverfahren", 22);
        assert!(lines.len() > 1, "{lines:?}");
        for line in &lines {
            assert!(!line.ends_with('·'), "{lines:?}");
        }
        assert_eq!(
            lines.join(" "),
            "Deutsche Literatur · Roman · Prag · Gerichtsverfahren"
        );
    }

    /// A layout that names no columns at all still sizes every column to its content —
    /// which is what makes a plain list of cells renderable without a column plan.
    #[test]
    fn a_layout_without_columns_sizes_every_column_to_its_content() {
        let layout = Layout::default();
        let rows = vec![
            vec![Cell::new("a"), Cell::new("bb")],
            vec![Cell::new("ccc"), Cell::new("d")],
        ];
        assert_eq!(render(&layout, &rows, Style::plain(100)), "a    bb\nccc  d");
        assert_eq!(layout.widths(&rows, 100), vec![3, 2]);
    }
}
