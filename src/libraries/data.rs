//! The compiled-in list and the types it deserialises into.

use std::sync::OnceLock;

/// The library list, compiled into the binary. There is no runtime file to lose and no
/// path to configure — `blibs libraries` must work with a cold cache and no network.
pub const LIBRARIES_JSON: &str = include_str!("../../data/libraries.json");

/// One institution.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Library {
    /// Canonical ISIL. The spelling here is authoritative: the upstream holdings filter
    /// is case-sensitive.
    pub isil: String,
    /// Spoken short names, e.g. `STABI` and `SBB` for the same house. Unique across the
    /// whole list — an invariant of the file, checked in `tests/libraries.rs`.
    pub aliases: Vec<String>,
    /// Short display name for the table and for holding lines.
    pub short_name: String,
    /// The name the KOBV portal's availability fragment uses. Needed because that
    /// fragment carries no ISIL and groups have to be matched by name when the
    /// positional match does not apply.
    pub portal_name: Option<String>,
    /// Full official name.
    pub name: String,
    /// Institution type, e.g. `Universitätsbibliothek`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// City.
    pub city: String,
    /// Postal address.
    pub address: String,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Homepage.
    pub url: Option<String>,
    /// Catalogue.
    pub opac: Option<String>,
    /// Contact address.
    pub email: Option<String>,
    /// Contact number.
    pub phone: Option<String>,
    /// The id the KOBV library directory uses.
    pub kobvid: Option<String>,
    /// Branches of this institution.
    pub branches: Vec<Branch>,
}

/// One branch of an institution.
///
/// Only the few branches that are actually spoken about carry an alias (`AGB`); the other
/// 209 have none, rather than an invented one.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Branch {
    /// The id the KOBV library directory uses; also what the `bibids=` link in the
    /// availability table points at.
    pub kobvid: String,
    /// The branch's own ISIL, where it has one.
    pub isil: Option<String>,
    /// Full name.
    pub name: String,
    /// Short display name.
    pub short_name: String,
    /// Spoken short names.
    pub aliases: Vec<String>,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Postal address.
    pub address: Option<String>,
    /// Strings that identify this branch in a location cell, for the cases where the
    /// portal names a branch in prose instead of linking it.
    #[serde(rename = "match")]
    pub match_strings: Vec<String>,
}

static LIBRARIES: OnceLock<Vec<Library>> = OnceLock::new();

/// The whole list, parsed once on first use.
///
/// Parsed lazily rather than at start-up: `blibs search` touches the list only for the
/// handful of ISILs it actually displays, and cold start is a stated requirement.
pub fn all() -> &'static [Library] {
    LIBRARIES.get_or_init(|| todo!("phase 2: libraries"))
}
