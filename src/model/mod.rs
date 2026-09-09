//! The domain types every other layer shares.
//!
//! This module is the boundary. Both engines produce these types, both renderers consume
//! them, and the JSON schema is derived from them with `serde` — which is the mechanism
//! that keeps human and agent output from drifting apart. A change here is a change to
//! the public interface: adding a field is allowed, renaming or removing one is a break.
//!
//! `model` performs no I/O and knows nothing about either catalogue beyond the
//! [`Engine`] tag that says which one answered.

pub mod catalog;
pub mod id;
pub mod isil;
pub mod page;
pub mod record;
pub mod search;

pub use catalog::{Catalog, EngineShow};
pub use id::{AvailabilityId, Engine, RecordId};
pub use isil::Isil;
pub use page::{FetchWindow, Limit, Page, SruPageSize};
pub use record::{Author, AuthorKind, Format, Holding, Item, Record, ResourceUrl, Status, UrlKind};
pub use search::note_kinds;
pub use search::{
    AtBlock, AvailabilityMode, BranchRef, EngineSearch, Identifier, Location, LocationRefusal,
    Note, QueryEcho, QuerySpec, SearchRequest, SearchResult, ShowAt, ShowResult, SortKey,
    SortScope, SortSpec, Term, WindowInfo,
};
pub use search::{ambiguous_key_notes, past_the_last_result_note, window_too_deep_note};
