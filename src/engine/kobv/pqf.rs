//! Building the Z39.50 prefix query.
//!
//! PQF, not CQL. Same indexes, same hit counts, plus the one attribute CQL lacks: `1044`
//! (ISIL), which restricts a search to an institution upstream. Verified 2026-09-06,
//! `plan/scraping.md` §A.4a.
//!
//! Two rules keep this safe. **User input is never written raw into a query**: every term
//! is quoted, and a `"` inside a term is removed before quoting, which makes every other
//! punctuation mark harmless (measured with `@`, `{`, `\`). And **user input is never
//! quoted on the user's behalf**: a phrase is a phrase because the user's shell delivered
//! it as one argument, not because this module decided so.
//!
//! What does not exist upstream is caught in `cli`, not here: wildcards (diagnostic
//! 1/48), ranges (which return zero hits *silently*), sorting, and any index for material
//! type or language.

use crate::model::{Isil, QuerySpec, RecordId};

/// Bib-1 use attribute for the default "any" index.
pub const ATTR_ANY: u16 = 1016;
/// Bib-1 use attribute for title.
pub const ATTR_TITLE: u16 = 4;
/// Bib-1 use attribute for author.
pub const ATTR_AUTHOR: u16 = 1;
/// Bib-1 structure attribute for a word list — what `--author` always uses.
pub const ATTR_STRUCTURE_WORDLIST: u16 = 6;
/// Bib-1 use attribute for subject.
pub const ATTR_SUBJECT: u16 = 21;
/// Bib-1 use attribute for publisher.
pub const ATTR_PUBLISHER: u16 = 59;
/// Bib-1 use attribute for date of publication.
pub const ATTR_YEAR: u16 = 31;
/// Bib-1 use attribute for ISBN.
pub const ATTR_ISBN: u16 = 7;
/// Bib-1 use attribute for ISSN.
pub const ATTR_ISSN: u16 = 8;
/// Bib-1 use attribute for a record identifier.
pub const ATTR_RECORD_ID: u16 = 12;
/// Bib-1 use attribute for ISIL — the holdings filter, and the reason this module exists.
/// **Case-sensitive**: `de-11` matches nothing, silently.
pub const ATTR_ISIL: u16 = 1044;

/// The largest query the service accepts before answering HTTP 414 as diagnostic 1/2.
pub const MAX_QUERY_CHARS: usize = 1000;

/// An assembled prefix query. Constructed only in this module, so there is exactly one
/// place where user input reaches the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pqf(String);

impl Pqf {
    /// The query string for the `x-pquery` parameter.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Pqf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Build the search query.
///
/// Every field of the spec that is set contributes one clause, joined with `@and`. The
/// locations become one `@or` over `@attr 1=1044` clauses, using the **canonical** ISIL
/// spelling. An empty location list adds no clause at all.
pub fn search(_spec: &QuerySpec, _locations: &[Isil]) -> Pqf {
    todo!("phase 2: pqf")
}

/// The same query restricted to a single library, for the per-location hit count.
pub fn count_for(_spec: &QuerySpec, _isil: &Isil) -> Pqf {
    todo!("phase 2: pqf")
}

/// Look one record up by its id.
pub fn record_lookup(_id: &RecordId) -> Pqf {
    todo!("phase 2: pqf")
}
