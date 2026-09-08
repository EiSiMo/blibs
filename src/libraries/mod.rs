//! The library list — 123 institutions and 211 branches, compiled into the binary.
//!
//! **This is data, not code.** Nothing in this crate branches on a specific ISIL. The
//! list only ever *adds* a display name, an alias and coordinates to something that came
//! out of a record; when it has nothing to add, the bare code is displayed and the
//! holding is still shown. Institutions join and leave the network, so every path through
//! the mapping degrades instead of failing.
//!
//! The one asymmetry, and it is deliberate: **what the user types is validated, what the
//! data contains is not.** An unknown library in `--at` is a usage error with a pointer to
//! `blibs libraries --find`; an unknown ISIL in a MARC `924` field is rendered as-is.

pub mod data;
pub mod geo;
pub mod resolve;
pub mod text;

pub use data::{Branch, Library, all};
pub use geo::{LatLon, NotAPoint};
pub use resolve::{
    Entry, Found, PATH_SEPARATOR, VOEBB_NETWORK, alias_for, branch_count, branch_label,
    branch_location, branches, by_isil, by_kobvid, by_portal_name, display_name,
    distinguishing_name, entries, entry_location, find_entries, houses_with_branches,
    institution_location, isil_answers_elsewhere, look_up, name_holding, near_entries, resolve,
    shares_isil, short_name_for, short_name_is_cut, suggest,
};
