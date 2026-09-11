//! `blibs` — search the libraries of Berlin and Brandenburg.
//!
//! The crate answers one question: *is this book in one of my libraries, is it in right
//! now, and where is it on the shelf?* It is a client for two catalogues that share
//! nothing but this crate's domain types:
//!
//! - [`engine::kobv`] — the KOBV union catalogue (SRU + the portal's availability
//!   service), which knows every institution in the region.
//! - [`engine::voebb`] — `voebb.de`, the only source that knows which *branch* of the
//!   public library network holds a copy.
//!
//! Which engine runs is decided once, in [`cli`], from `--at` or from a record id.
//! Nothing downstream branches on it: both engines produce [`model::Record`],
//! [`model::Holding`] and [`model::Item`], and both renderers in [`render`] consume only
//! those.
//!
//! The layering is a hard rule, not a suggestion:
//!
//! ```text
//! cli       → argument parsing, validation, engine choice, exit codes
//! render    → human and JSON renderers over the same domain types
//! engine    → client (I/O only) and parse (interpretation only), per catalogue
//! select    → client-side sort/filter, grouping, "my libraries"
//! model     → the domain types every layer above shares
//! libraries → the compiled-in list: resolves `--at` and a record's ISIL, ranks by distance
//! http      → transport, cache, backoff, per-host cap; interprets nothing
//! error     → one error enum; every variant says what failed and what to do
//! counts    → the English phrases for a count, shared by `error` and `render`
//! ```
//!
//! Two rules run through all of it. A parser never performs I/O and a client never
//! interprets a payload. And a missing structure is never rendered as an empty result:
//! an empty list is indistinguishable from "no hits", which is the worst answer this tool
//! can give an agent — so a missing element becomes a named error instead.
//!
//! # This library is not a public API
//!
//! **The interface of `blibs` is the `blibs` command**, and for an agent the `--json`
//! document it prints. Everything below this line is an implementation detail that is
//! published only because the binary and the integration tests are built from it: a
//! library target is the one way `tests/` can reach these modules at all.
//!
//! So **nothing here carries a stability guarantee**. Types, traits, function signatures
//! and whole modules may change, be renamed or disappear in any release, including a
//! patch release, without a deprecation and without a note in the changelog. Depending on
//! this crate as a library means pinning an exact version and expecting to rewrite.
//!
//! What *is* stable is documented in the README: the command-line surface, the JSON
//! schema, the `notes[].kind` vocabulary and the exit codes.

pub mod cli;
pub mod counts;
pub mod engine;
pub mod error;
pub mod http;
pub mod libraries;
pub mod model;
pub mod render;
pub mod select;
