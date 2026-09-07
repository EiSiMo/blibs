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
    /// Every engine, in the order an invocation runs them.
    ///
    /// This order — and not the order of `--at` — is what fixes `engines[]` and the order
    /// the records of two catalogues follow each other in. `--at AGB,HU` and `--at HU,AGB`
    /// ask the same question, so they must produce the same document; the *display* order
    /// is the user's and lives in `at[]`.
    pub const ALL: [Engine; 2] = [Engine::Kobv, Engine::Voebb];

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
    /// The whole id, exactly as it is printed and typed back in. Kept as one string
    /// rather than as two halves, so that [`RecordId::as_str`] can hand it out without
    /// rebuilding it — it is read once per record line and once per JSON member.
    full: Box<str>,
    /// Byte offset of the separating `_` in `full`. Always inside the string and always
    /// on an ASCII byte, because [`RecordId::split`] found it there.
    separator: usize,
    engine: Engine,
}

impl RecordId {
    /// Parse an id a user typed on the command line.
    ///
    /// Splits at the **first** `_`: local ids contain underscores and percent-escapes
    /// (`kobvindex_VBRD-...rüt20in209+20`), so a split at the last one would corrupt
    /// them. A `voebb_` prefix selects [`Engine::Voebb`], anything else
    /// [`Engine::Kobv`]. An id without a prefix is a usage error, never a silent miss.
    pub fn parse(s: &str) -> Result<Self, UsageError> {
        Self::split(s).ok_or_else(|| UsageError::RecordId {
            input: s.to_owned(),
        })
    }

    /// Parse an id that came out of a catalogue response.
    ///
    /// Same splitting rule, different failure category: data the tool received but
    /// cannot interpret is an [`UnexpectedError`], not the user's fault. Deliberately no
    /// stricter than [`RecordId::parse`] — everything after the first `_` is taken as the
    /// local id whatever it contains, because the catalogue's spelling is authoritative
    /// and rejecting a record over an umlaut or a `+` would lose a hit.
    pub fn from_catalog(s: &str) -> Result<Self, UnexpectedError> {
        Self::split(s).ok_or_else(|| UnexpectedError::MissingElement {
            what: format!("a source prefix in record id {s:?}"),
            context: "catalogue response".to_owned(),
        })
    }

    /// The one splitting rule, shared by both entry points.
    ///
    /// `None` when there is no `_` at all, or when either side of the first one is empty
    /// — those are the three shapes that carry no source, and a record without a source
    /// cannot be routed to an engine.
    fn split(s: &str) -> Option<Self> {
        let (source, local) = s.split_once('_')?;
        if source.is_empty() || local.is_empty() {
            return None;
        }
        Some(Self {
            full: Box::from(s),
            separator: source.len(),
            engine: Self::engine_of(source),
        })
    }

    /// Which catalogue a source prefix belongs to. Only `voebb` is `voebb.de`; every other
    /// prefix (`almafu`, `almahu`, `kobvindex`, `gbv`, `b3kat`, …) is a KOBV source, and
    /// an unknown one stays KOBV rather than becoming an error — sources come and go.
    fn engine_of(source: &str) -> Engine {
        if source == Engine::Voebb.as_str() {
            Engine::Voebb
        } else {
            Engine::Kobv
        }
    }

    /// Build the id of a voebb.de record from its local number.
    pub fn voebb(local: &str) -> Self {
        let source = Engine::Voebb.as_str();
        Self {
            full: Box::from(format!("{source}_{local}")),
            separator: source.len(),
            engine: Engine::Voebb,
        }
    }

    /// Which catalogue this id belongs to.
    pub fn engine(&self) -> Engine {
        self.engine
    }

    /// The source prefix, without the separating underscore.
    pub fn source(&self) -> &str {
        // Both slices are inside the string and on the boundary the split found, so
        // neither can be out of range or split a character in half.
        self.full.get(..self.separator).unwrap_or_default()
    }

    /// The local number, without the source prefix.
    pub fn local_id(&self) -> &str {
        self.full
            .get(self.separator.saturating_add(1)..)
            .unwrap_or_default()
    }

    /// The full id, as it is printed and as it must be typed back in.
    pub fn as_str(&self) -> &str {
        &self.full
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Serialises as four flat members — `id`, `engine`, `source`, `local_id` — because
/// [`crate::model::Record`] flattens it. Written by hand so that the derived parts and
/// the composed `id` cannot disagree.
impl Serialize for RecordId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("id", self.as_str())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_alma_id() {
        let id = RecordId::parse("almafu_BV008885798").expect("a prefixed id parses");
        assert_eq!(id.engine(), Engine::Kobv);
        assert_eq!(id.source(), "almafu");
        assert_eq!(id.local_id(), "BV008885798");
        assert_eq!(id.as_str(), "almafu_BV008885798");
    }

    #[test]
    fn the_voebb_prefix_selects_the_voebb_engine() {
        let id = RecordId::parse("voebb_SAK13776205").expect("a prefixed id parses");
        assert_eq!(id.engine(), Engine::Voebb);
        assert_eq!(id.source(), "voebb");
        assert_eq!(id.local_id(), "SAK13776205");
        assert_eq!(id, RecordId::voebb("SAK13776205"));
    }

    /// The local part is percent-escaped, carries umlauts, hyphens and `+`, and contains
    /// digits that look like escapes. Splitting anywhere but at the *first* `_` — or
    /// validating the local part at all — would corrupt it.
    #[test]
    fn a_local_id_may_contain_anything() {
        let raw = "kobvindex_VBRD-tollewgewewe16berrüt20in209+20";
        let id = RecordId::parse(raw).expect("the local part is never validated");
        assert_eq!(id.source(), "kobvindex");
        assert_eq!(id.local_id(), "VBRD-tollewgewewe16berrüt20in209+20");
        assert_eq!(id.as_str(), raw);
        assert_eq!(id.engine(), Engine::Kobv);
    }

    /// Underscores inside the local part belong to the local part.
    #[test]
    fn splits_at_the_first_underscore_only() {
        let id = RecordId::parse("kobvindex_ABC_DEF_1").expect("a prefixed id parses");
        assert_eq!(id.source(), "kobvindex");
        assert_eq!(id.local_id(), "ABC_DEF_1");
    }

    /// The prefix is part of the identity: two union-catalogue sources routinely carry
    /// the same local number for different records.
    #[test]
    fn the_same_local_id_under_two_sources_is_two_records() {
        let fu = RecordId::parse("almafu_9950038349602882").expect("a prefixed id parses");
        let hu = RecordId::parse("almahu_9950038349602882").expect("a prefixed id parses");
        assert_ne!(fu, hu);
        assert_eq!(fu.local_id(), hu.local_id());
        assert_ne!(fu.as_str(), hu.as_str());
    }

    #[test]
    fn an_id_without_a_prefix_is_a_usage_error() {
        let error = RecordId::parse("BV008885798").expect_err("no prefix, no engine");
        assert!(matches!(error, UsageError::RecordId { .. }));
    }

    #[test]
    fn an_empty_half_is_rejected_on_both_sides() {
        assert!(RecordId::parse("_BV008885798").is_err());
        assert!(RecordId::parse("almafu_").is_err());
        assert!(RecordId::parse("_").is_err());
        assert!(RecordId::parse("").is_err());
    }

    /// Same rule, different error category: bad catalogue data is not the user's fault.
    #[test]
    fn a_catalogue_id_without_a_prefix_is_an_unexpected_error() {
        assert_eq!(
            RecordId::from_catalog("almafu_BV008885798").expect("a prefixed id parses"),
            RecordId::parse("almafu_BV008885798").expect("a prefixed id parses")
        );
        let error = RecordId::from_catalog("BV008885798").expect_err("no prefix, no engine");
        assert!(matches!(error, UnexpectedError::MissingElement { .. }));
        assert!(error.to_string().contains("BV008885798"));
    }

    #[test]
    fn serialises_as_four_flat_members_in_order() {
        let id = RecordId::parse("almafu_BV008885798").expect("a prefixed id parses");
        let json = serde_json::to_string(&id).expect("RecordId serialises");
        assert_eq!(
            json,
            r#"{"id":"almafu_BV008885798","engine":"kobv","source":"almafu","local_id":"BV008885798"}"#
        );
    }

    #[test]
    fn engines_render_lowercase() {
        assert_eq!(Engine::Kobv.to_string(), "kobv");
        assert_eq!(Engine::Voebb.to_string(), "voebb");
    }

    #[test]
    fn availability_ids_are_carried_verbatim() {
        let key = AvailabilityId::new("DE-11;BV008885798,DE-1;275177939,");
        assert_eq!(key.as_str(), "DE-11;BV008885798,DE-1;275177939,");
        assert_eq!(key.to_string(), key.as_str());
    }
}
