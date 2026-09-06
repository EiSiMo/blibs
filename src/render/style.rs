//! When to colour, and with what.
//!
//! Colour is emitted only on a terminal, only when `NO_COLOR` is unset, and never when
//! `TERM=dumb`. The styles here are data — [`anstyle`] describes them without deciding
//! whether they are written.

use crate::model::Status;

/// Whether this invocation writes colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    coloured: bool,
    width: usize,
}

impl Style {
    /// Decide from the environment: a TTY, no `NO_COLOR`, `TERM` not `dumb`.
    ///
    /// Also fixes the output width, so that a piped invocation is reproducible instead of
    /// depending on the terminal that happened to launch it.
    pub fn detect() -> Self {
        todo!("phase 2: render style")
    }

    /// A style that never colours, at a fixed width. For tests and for `--json`.
    pub fn plain(width: usize) -> Self {
        Self {
            coloured: false,
            width,
        }
    }

    /// Whether colour is written.
    pub fn coloured(self) -> bool {
        self.coloured
    }

    /// The output width to lay out for.
    pub fn width(self) -> usize {
        self.width
    }

    /// The style for a status symbol. Returns the default style when colour is off, so
    /// callers never branch on [`Style::coloured`].
    pub fn status(self, _status: Status) -> anstyle::Style {
        todo!("phase 2: render style")
    }

    /// The style for secondary text — shelfmarks, counts, the legend.
    pub fn dim(self) -> anstyle::Style {
        todo!("phase 2: render style")
    }
}
