//! The compiled-in list and the types it deserialises into.

use std::sync::OnceLock;

use crate::libraries::geo::LatLon;

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

impl Library {
    /// The canonical alias, i.e. the one the tool prints and accepts first.
    ///
    /// `None` for a house that has none — the list carries no invented abbreviations
    /// (`plan/libraries.md` §5, rule 5), and such a house is addressed by its ISIL.
    pub fn alias(&self) -> Option<&str> {
        self.aliases.first().map(String::as_str)
    }

    /// The coordinates, when the entry has usable ones.
    ///
    /// Every entry has coordinates today. The `None` path exists anyway: an institution
    /// that joins the network without them must drop out of `--near`, never sort to the
    /// front of it.
    pub fn coords(&self) -> Option<LatLon> {
        LatLon::checked(self.lat, self.lon)
    }
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

impl Branch {
    /// The canonical alias. Only three branches have one (`AGB`, `BSTB`, `PHILBIB`).
    pub fn alias(&self) -> Option<&str> {
        self.aliases.first().map(String::as_str)
    }

    /// The coordinates, when the entry has usable ones. See [`Library::coords`].
    pub fn coords(&self) -> Option<LatLon> {
        LatLon::checked(self.lat, self.lon)
    }
}

static LIBRARIES: OnceLock<Vec<Library>> = OnceLock::new();

/// The whole list, parsed once on first use.
///
/// Parsed lazily rather than at start-up: `blibs search` touches the list only for the
/// handful of ISILs it actually displays, and cold start is a stated requirement.
pub fn all() -> &'static [Library] {
    LIBRARIES.get_or_init(|| {
        // Not a runtime failure mode: the file is compiled in, and `tests/libraries.rs`
        // parses it independently of this loader. If this panics, the binary was built
        // from a data file that does not match `Library` — a programmer error.
        serde_json::from_str(LIBRARIES_JSON)
            .expect("data/libraries.json is compiled in and must deserialise into Library")
    })
}

#[cfg(test)]
mod tests {
    use super::all;

    /// The compiled-in list is reachable through the loader, and the loader hands out the
    /// same slice every time — `OnceLock`, not a fresh parse per call.
    #[test]
    fn loads_once() {
        let first = all();
        assert!(!first.is_empty());
        assert!(std::ptr::eq(first, all()));
    }

    /// Every entry in the file has coordinates today; `coords()` must say so rather than
    /// quietly hiding houses from `--near`.
    #[test]
    fn every_entry_has_coordinates() {
        for library in all() {
            assert!(
                library.coords().is_some(),
                "{} has no coordinates",
                library.isil
            );
            for branch in &library.branches {
                assert!(
                    branch.coords().is_some(),
                    "branch {} of {} has no coordinates",
                    branch.kobvid,
                    library.isil
                );
            }
        }
    }

    /// The `None` path of `coords()` is not decoration: a non-finite coordinate has to
    /// drop the entry out of `--near` instead of sorting it to the front.
    #[test]
    fn non_finite_coordinates_are_not_coordinates() {
        let mut library = all()[0].clone();
        library.lat = f64::NAN;
        assert!(library.coords().is_none());
    }
}
