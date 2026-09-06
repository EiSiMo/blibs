//! Output. Two renderers over the same domain types, which is what stops them drifting.
//!
//! The human renderer is the default: one readable line per hit, grouped by location
//! whenever `--at` was given, with the copies of *that* location beneath each line. No
//! flag turns the grouping off. The JSON renderer emits one document on stdout and
//! nothing else, and it is the only place where the error object is built.
//!
//! Colour is decoration and never information: symbols carry the status, and the layout
//! is byte-identical with and without colour — which is why [`table`] computes widths on
//! the *uncoloured* text.

pub mod human;
pub mod json;
pub mod style;
pub mod table;

pub use style::Style;
pub use table::{Cell, Table};
