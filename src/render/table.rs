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

/// The width to lay out for when there is no terminal to ask.
///
/// A fixed number rather than "unlimited": a piped or redirected invocation must produce
/// the same bytes on every machine, and an agent that diffs two runs should not see the
/// launching terminal in the diff.
pub const DEFAULT_WIDTH: usize = 100;

/// The marker put where text was cut away.
const ELLIPSIS: char = '…';

/// The terminal's width in columns, or [`DEFAULT_WIDTH`] when there is no terminal.
pub fn terminal_columns() -> usize {
    terminal_size::terminal_size().map_or(DEFAULT_WIDTH, |(width, _)| usize::from(width.0))
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
    /// example output in `plan/cli.md`, whose columns do not depend on the records that
    /// happen to be on the page.
    Fixed(usize),
    /// Sized to its widest cell, giving way down to `min` but no further — the title
    /// column, which absorbs the shortfall for everything else.
    Flex {
        /// The narrowest this column may become.
        min: usize,
    },
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
            Column::Flex { min } => Some(min),
            Column::Fixed(_) | Column::Last => None,
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
    /// their own padding, which is how the examples in `plan/cli.md` are written.
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
    /// Columns start at their natural width — [`Column::Fixed`] at its stated one, every
    /// other at its widest cell — and are then shrunk, widest first, until the row fits.
    /// Widest first so that one long title gives way before four short columns do.
    /// [`Column::Fixed`] and [`Column::Last`] never give way, so a row can still overflow
    /// a very narrow terminal; that is preferable to a cut record id.
    pub fn widths(&self, rows: &[Vec<Cell>], available: usize) -> Vec<usize> {
        let count = self.count(rows);
        let mut widths: Vec<usize> = (0..count)
            .map(|index| match self.column(index) {
                Column::Fixed(width) => width,
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

/// A set of rows laid out into aligned columns.
#[derive(Debug, Clone, Default)]
pub struct Table {
    rows: Vec<Vec<Cell>>,
    layout: Layout,
}

impl Table {
    /// An empty table whose columns are sized to their content.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty table with an explicit column plan.
    pub fn with_layout(layout: Layout) -> Self {
        Self {
            rows: Vec::new(),
            layout,
        }
    }

    /// Append a row.
    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    /// The rows added so far.
    pub fn rows(&self) -> &[Vec<Cell>] {
        &self.rows
    }

    /// The column plan.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Render the table.
    ///
    /// Columns are sized to their widest cell; when the total exceeds the available
    /// width, only truncatable cells give way, and they give way from the widest column
    /// first so that one long title does not squeeze every other column.
    pub fn render(&self, style: Style) -> String {
        let widths = self.layout.widths(&self.rows, style.width());
        self.rows
            .iter()
            .map(|row| self.layout.render_row(row, &widths, style))
            .collect::<Vec<_>>()
            .join("\n")
    }
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

    /// The `--at` example in `plan/cli.md`, character for character. The widths there
    /// include their own padding, so the gap is zero.
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
        let mut table = Table::with_layout(Layout::auto(2));
        table.push(vec![Cell::new("Kafka"), Cell::new("1953")]);
        table.push(vec![Cell::new("Schlink, Bernhard"), Cell::new("1997")]);
        assert_eq!(
            table.render(Style::plain(100)),
            "Kafka              1953\nSchlink, Bernhard  1997"
        );
    }

    /// The widest column gives way first, so one long title does not squeeze the rest.
    #[test]
    fn the_widest_column_gives_way_first() {
        let layout = Layout::new(vec![Column::Auto, Column::Auto, Column::Last]);
        let mut table = Table::with_layout(layout);
        table.push(vec![
            Cell::new("Der Prozess : Roman einer Verwandlung"),
            Cell::new("Kafka"),
            Cell::new("almafu_BV008885798").whole(),
        ]);
        let rendered = table.render(Style::plain(40));
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
        let layout = Layout::new(vec![Column::Flex { min: 4 }, Column::Last]);
        let mut table = Table::with_layout(layout);
        table.push(vec![
            Cell::new("Der Prozess : Roman"),
            Cell::new("almafu_BV008885798").whole(),
        ]);
        let rendered = table.render(Style::plain(20));
        assert_eq!(rendered, "Der…  almafu_BV008885798");
        assert!(rendered.ends_with("almafu_BV008885798"));
    }

    /// `Flex` stops at its minimum instead of collapsing to a bare ellipsis.
    #[test]
    fn a_flex_column_does_not_shrink_past_its_minimum() {
        let layout = Layout::new(vec![Column::Flex { min: 8 }, Column::Last]);
        let mut table = Table::with_layout(layout);
        table.push(vec![
            Cell::new("Der Prozess : Roman einer Verwandlung"),
            Cell::new("almafu_BV008885798").whole(),
        ]);
        let rendered = table.render(Style::plain(10));
        assert_eq!(rendered, "Der Pro…  almafu_BV008885798");
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
        let mut plain = Table::with_layout(layout.clone());
        let mut coloured = Table::with_layout(layout);
        for row in rows {
            plain.push(row.clone());
            coloured.push(row);
        }
        let plain = plain.render(Style::plain(100));
        let coloured = coloured.render(Style::new(true, 100));
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
        let layout = Layout::auto(3);
        let mut table = Table::with_layout(layout);
        table.push(vec![
            Cell::new("Kafka"),
            Cell::new("1953"),
            Cell::new("almafu_BV008885798"),
        ]);
        table.push(vec![Cell::new("Schlink")]);
        let rendered = table.render(Style::plain(100));
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[1], "Schlink");
        assert!(!lines[0].ends_with(' '));
    }

    #[test]
    fn an_empty_table_renders_nothing() {
        assert_eq!(Table::new().render(Style::plain(100)), "");
        assert!(Table::new().rows().is_empty());
    }

    #[test]
    fn a_table_without_a_layout_sizes_every_column_to_its_content() {
        let mut table = Table::new();
        table.push(vec![Cell::new("a"), Cell::new("bb")]);
        table.push(vec![Cell::new("ccc"), Cell::new("d")]);
        assert_eq!(table.render(Style::plain(100)), "a    bb\nccc  d");
        assert_eq!(table.layout().widths(table.rows(), 100), vec![3, 2]);
    }
}
