//! Turning what the user typed, or what a record contained, into a library.

use crate::error::UsageError;
use crate::libraries::{Branch, LatLon, Library};
use crate::model::{Isil, Location};

/// Resolve one `--at` entry.
///
/// Aliases are tried first (case-folded), then ISILs (case-insensitively) — and the
/// result always carries the **canonical** ISIL from the list, never the user's spelling.
/// An unknown entry is a usage error carrying up to three suggestions within Levenshtein
/// distance 3, never a silent non-match.
///
/// The engine follows from what was named: a branch under `DE-609` resolves to
/// [`crate::model::Engine::Voebb`], an institution to
/// [`crate::model::Engine::Kobv`], and any other branch is rejected as not searchable on
/// its own — the KOBV record does not know which branch holds a copy, so a branch filter
/// there could never prove absence.
pub fn resolve(_input: &str) -> Result<Location, UsageError> {
    todo!("phase 2: libraries")
}

/// Look up an institution by its exact ISIL.
pub fn by_isil(_isil: &Isil) -> Option<&'static Library> {
    todo!("phase 2: libraries")
}

/// Look up an institution by the name the KOBV portal uses in the availability fragment.
pub fn by_portal_name(_name: &str) -> Option<&'static Library> {
    todo!("phase 2: libraries")
}

/// Look up a branch by its KOBV directory id, as found in a `bibids=` link.
pub fn by_kobvid(_kobvid: &str) -> Option<(&'static Library, &'static Branch)> {
    todo!("phase 2: libraries")
}

/// Free-text search over alias, name, short name and city, for `libraries --find`.
pub fn find(_query: &str) -> Vec<&'static Library> {
    todo!("phase 2: libraries")
}

/// All institutions ordered by distance from a point, for `libraries --near`.
pub fn near(_point: LatLon) -> Vec<(&'static Library, f64)> {
    todo!("phase 2: libraries")
}

/// Display name for an ISIL.
///
/// **Never `None`.** An ISIL that is not in the list renders as the bare code — an
/// unknown library must never make a holding disappear.
pub fn display_name(_isil: &Isil) -> String {
    todo!("phase 2: libraries")
}
