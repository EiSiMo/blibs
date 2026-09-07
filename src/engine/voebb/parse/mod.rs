//! Interpreting voebb.de's HTML. **Never performs I/O.**
//!
//! Everything here uses a real HTML parser, and every failed selector produces an error
//! that names the selector — that string is the only thing that tells a maintainer what
//! the site changed. A missing selector is never an empty result.

pub mod detail;
pub mod facet;
pub mod form;
pub mod noaccess;
pub mod results;

use scraper::Selector;

use crate::error::{Error, UnexpectedError};

/// Compile one selector literal.
///
/// Every selector in these parsers is a literal written here, so a parse failure is a
/// typo in this crate and not something a page can cause. The message states that
/// invariant rather than the selector, which the compiler already shows.
pub(super) fn compile(css: &str) -> Selector {
    Selector::parse(css).expect("every selector in this crate's voebb parsers is a literal")
}

/// Collapse runs of whitespace and trim.
///
/// The site indents its cells over several lines and pads columns with runs of spaces,
/// and an empty cell carries a `&nbsp;` — which is whitespace here, so an unlabelled row
/// collapses to an empty string rather than to a character nobody can see.
pub(super) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A missing-selector error naming what stopped matching and in which document.
///
/// The one place the error is built, so that all four parsers report a changed site the
/// same way: the selector string is the only thing that tells a maintainer what moved.
pub(super) fn missing_selector(selector: &str, document: &str) -> Error {
    UnexpectedError::MissingSelector {
        selector: selector.to_string(),
        document: document.to_string(),
    }
    .into()
}
