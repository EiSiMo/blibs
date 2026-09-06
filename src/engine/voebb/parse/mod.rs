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
