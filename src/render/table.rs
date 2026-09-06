//! Column layout.
//!
//! Widths are computed with `unicode-width` on the **uncoloured** text: escape sequences
//! have no width, and CJK and combining characters do not have the width of their byte
//! count. Getting either wrong misaligns every non-Latin record in the catalogue.
//!
//! What may be truncated is a policy decision, not a layout one: titles are truncated,
//! **record ids never are** — a shortened id cannot be typed back into `show`.

use crate::render::Style;

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
        todo!("phase 2: render table")
    }
}

/// A set of rows laid out into aligned columns.
#[derive(Debug, Clone, Default)]
pub struct Table {
    rows: Vec<Vec<Cell>>,
}

impl Table {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row.
    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    /// The rows added so far.
    pub fn rows(&self) -> &[Vec<Cell>] {
        &self.rows
    }

    /// Render the table.
    ///
    /// Columns are sized to their widest cell; when the total exceeds the available
    /// width, only truncatable cells give way, and they give way from the widest column
    /// first so that one long title does not squeeze every other column.
    pub fn render(&self, _style: Style) -> String {
        todo!("phase 2: render table")
    }
}
