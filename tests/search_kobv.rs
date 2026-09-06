//! End-to-end tests of the KOBV engine against saved fixtures.
//!
//! Filled in phase 3.1. What has to be proven here, from `plan/`:
//!
//! - `--at HU,STABI` issues exactly one search request and two counting requests;
//! - the PQF that goes out is character-for-character what `plan/scraping.md` §A.4a
//!   documents, with canonical ISIL spelling;
//! - ten displayed records cause ten availability requests, a record without MARC `924`
//!   causes none, and `--no-availability` causes none at all;
//! - the recorder never sees more than six requests in flight at once.

mod common;
