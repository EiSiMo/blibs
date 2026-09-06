//! The branch facet on the result page.

use crate::error::Error;

/// One branch offered by the facet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetEntry {
    /// The branch's display name.
    pub name: String,
    /// The form value that selects it.
    pub value: String,
    /// The hit count the facet states. Reported even when the facet cannot be applied —
    /// it stays a true statement either way.
    pub count: Option<u64>,
}

/// Read the branch facet.
pub fn parse(_html: &str) -> Result<Vec<FacetEntry>, Error> {
    todo!("phase 5: voebb parse")
}
