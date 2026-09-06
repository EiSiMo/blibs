//! The two catalogues.
//!
//! `kobv` and `voebb` share nothing but [`crate::model`] and [`crate::error`]. There is
//! deliberately **no** common "scraper" base: an SRU endpoint returning MARCXML and a
//! session-bound aDISWeb form have nothing in common worth abstracting, and a shared
//! base would only make both of them harder to read.
//!
//! What they do share is the shape of the work. In each engine, `client` performs I/O and
//! interprets nothing, and `parse` interprets and performs no I/O. Parser tests run
//! against saved fixtures under `tests/fixtures/<engine>/`.

pub mod kobv;
pub mod voebb;
