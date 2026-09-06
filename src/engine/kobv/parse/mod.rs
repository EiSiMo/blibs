//! Interpreting what KOBV returns. **Never performs I/O.**
//!
//! Every module here takes a `&str` and returns either a value or an error naming the
//! thing that was missing. None of them ever turns a missing structure into an empty
//! result: "the element is not there" and "there were no hits" must stay
//! distinguishable, because an agent cannot tell them apart afterwards.
//!
//! Tests run against saved fixtures in `tests/fixtures/kobv/`, one test per proven trap.

pub mod availability;
pub mod coded;
pub mod marc;
pub mod record;
pub mod sru;
