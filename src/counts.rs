//! The English phrases for a count, in the one place both the error texts and the
//! terminal renderer can reach.
//!
//! `error` and `render::human` say the same sentences about the same numbers — *"6
//! results"*, *"10 records on this page"* — and `error` must not depend on `render`
//! (`lib.rs`, the layering). Two private copies of the wording is how a variant ends up
//! reading *"none of the 1 records"* while the block heading beside it reads *"1
//! result"*, so the wording lives below both instead.

/// `1 record` / `10 records`, for a count that speaks about the page rather than the
/// catalogue.
pub fn records(count: usize) -> String {
    if count == 1 {
        "1 record".to_owned()
    } else {
        format!("{count} records")
    }
}

/// `1 result` / `774 results`, for a catalogue's hit count.
pub fn results(total: u64) -> String {
    if total == 1 {
        "1 result".to_owned()
    } else {
        format!("{total} results")
    }
}

/// `1 fetched record` / `50 fetched records`, for the window a client-side filter saw.
///
/// Its own function rather than [`records`] with a word in front of it, because the
/// adjective sits *inside* the phrase the three callers share — the error text, the block
/// heading and the `window_filter_empty` note all say "none of the N fetched records
/// matched", and a caller that built it from `records` would have to know that the count
/// and its noun can be split. The window is a `usize` for the same reason `records` takes
/// one: it is a length, not a catalogue's claim.
pub fn fetched_records(count: usize) -> String {
    if count == 1 {
        "1 fetched record".to_owned()
    } else {
        format!("{count} fetched records")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The singular is the case the callers get wrong, because the plural is what every
    /// example in `plan/cli.md` shows.
    #[test]
    fn one_is_singular_and_everything_else_is_plural() {
        assert_eq!(records(1), "1 record");
        assert_eq!(records(0), "0 records");
        assert_eq!(records(10), "10 records");
        assert_eq!(results(1), "1 result");
        assert_eq!(results(0), "0 results");
        assert_eq!(results(774), "774 results");
        assert_eq!(fetched_records(1), "1 fetched record");
        assert_eq!(fetched_records(0), "0 fetched records");
        assert_eq!(fetched_records(50), "50 fetched records");
    }
}
