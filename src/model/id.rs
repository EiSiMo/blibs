//! Record identity: which catalogue a record came from, and its number in that catalogue.

use std::fmt;

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::error::{UnexpectedError, UsageError};

/// Which of the two catalogues answered.
///
/// Nothing downstream of [`crate::cli`] branches on this; it exists so that record
/// numbers from two catalogues cannot be confused, and so the JSON says which service a
/// statement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// The KOBV union catalogue: SRU plus the portal's availability service.
    Kobv,
    /// `voebb.de`: the public library network, one entry per edition, branch-aware.
    Voebb,
}

impl Engine {
    /// The lowercase name used in JSON, in messages and as the id prefix.
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Kobv => "kobv",
            Engine::Voebb => "voebb",
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source-prefixed record number, e.g. `almafu_BV008885798` or `voebb_SAK13776205`.
///
/// The prefix is part of the identity, not decoration: it selects the engine in `show`
/// and it distinguishes `almafu_` from `almahu_` records that share a local id. It is
/// therefore **never reconstructed from MARC `001`**, which differs between SRU and the
/// portal export — it is taken verbatim from where it was delivered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordId {
    engine: Engine,
    source: Box<str>,
    local: Box<str>,
}

impl RecordId {
    /// Parse an id a user typed on the command line.
    ///
    /// Splits at the **first** `_`: local ids contain underscores and percent-escapes
    /// (`kobvindex_VBRD-...rüt20in209+20`), so a split at the last one would corrupt
    /// them. A `voebb_` prefix selects [`Engine::Voebb`], anything else
    /// [`Engine::Kobv`]. An id without a prefix is a usage error, never a silent miss.
    pub fn parse(_s: &str) -> Result<Self, UsageError> {
        todo!("phase 1: model")
    }

    /// Parse an id that came out of a catalogue response.
    ///
    /// Same splitting rule, different failure category: data the tool received but
    /// cannot interpret is an [`UnexpectedError`], not the user's fault.
    pub fn from_catalog(_s: &str) -> Result<Self, UnexpectedError> {
        todo!("phase 1: model")
    }

    /// Build the id of a voebb.de record from its local number.
    pub fn voebb(local: &str) -> Self {
        Self {
            engine: Engine::Voebb,
            source: Box::from("voebb"),
            local: Box::from(local),
        }
    }

    /// Which catalogue this id belongs to.
    pub fn engine(&self) -> Engine {
        self.engine
    }

    /// The source prefix, without the separating underscore.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The local number, without the source prefix.
    pub fn local_id(&self) -> &str {
        &self.local
    }

    /// The full id, as it is printed and as it must be typed back in.
    pub fn as_str(&self) -> String {
        format!("{}_{}", self.source, self.local)
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.source, self.local)
    }
}

/// Serialises as four flat members — `id`, `engine`, `source`, `local_id` — because
/// [`crate::model::Record`] flattens it. Written by hand so that the derived parts and
/// the composed `id` cannot disagree.
impl Serialize for RecordId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("id", &self.as_str())?;
        map.serialize_entry("engine", &self.engine)?;
        map.serialize_entry("source", self.source())?;
        map.serialize_entry("local_id", self.local_id())?;
        map.end()
    }
}

/// The opaque key the KOBV portal's availability service is called with.
///
/// A comma-terminated list of `ISIL;LocalId` pairs, e.g.
/// `DE-11;BV008885798,DE-1;275177939,`. Reconstructed from MARC `924 $b;$a` so that no
/// HTML has to be fetched first. It is deliberately a newtype: the string is never built
/// anywhere but in [`crate::engine::kobv::parse::record`], and it must never be
/// concatenated across records — the response is keyed by ISIL and would collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AvailabilityId(Box<str>);

impl AvailabilityId {
    /// Wrap an already-assembled availability key.
    pub fn new(value: &str) -> Self {
        Self(Box::from(value))
    }

    /// The key as the query parameter expects it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AvailabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
