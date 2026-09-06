//! What was asked, where it was asked, and what came back.
//!
//! [`SearchResult`] is the JSON document. Its member order is the document's member order
//! and part of the contract in `plan/cli.md`.

use crate::model::{Engine, FetchWindow, Isil, Page, Record};

/// One search term.
///
/// The distinction comes from the shell and nowhere else: an argument that contains a
/// space was quoted by the user and is searched as a phrase; everything else is a word.
/// User input is never quoted on the user's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// A single word.
    Word(String),
    /// A quoted phrase.
    Phrase(String),
}

impl Term {
    /// Classify one command-line argument.
    ///
    /// The shell has already removed the quotes, so the *only* surviving evidence that
    /// the user quoted something is that the argument contains whitespace. Terms are
    /// never quoted on the user's behalf: an unquoted CQL/PQF term is an implicit AND and
    /// a quoted one is a phrase, and the two find wildly different numbers of records.
    pub fn from_argument(argument: &str) -> Self {
        let trimmed = argument.trim();
        if trimmed.chars().any(char::is_whitespace) {
            Term::Phrase(trimmed.to_owned())
        } else {
            Term::Word(trimmed.to_owned())
        }
    }

    /// The term's text, without the quoting decision.
    pub fn text(&self) -> &str {
        match self {
            Term::Word(text) | Term::Phrase(text) => text,
        }
    }

    /// Whether the term carries no text at all. An argument of only whitespace produces
    /// one of these, and `cli` rejects a query that is empty after they are dropped.
    pub fn is_empty(&self) -> bool {
        self.text().is_empty()
    }

    /// The term as it must be typed back in — a phrase keeps the quotes that made it one,
    /// so that `query.terms` in the JSON can be pasted straight back onto a command line.
    fn echo(&self) -> String {
        match self {
            Term::Word(text) => text.clone(),
            Term::Phrase(text) => format!("\"{text}\""),
        }
    }
}

/// A standard number the user searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// A validated ISBN, normalised to digits. Validated **before** sending: the
    /// identifier index discards the check digit, so a typo returns the wrong book
    /// instead of nothing.
    Isbn(String),
    /// An ISSN, passed through — there the check digit is significant upstream.
    Issn(String),
}

/// The query, decomposed. Assembled once in `cli` and never mutated afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuerySpec {
    /// Free terms, `@and`-ed together.
    pub terms: Vec<Term>,
    /// `--title`.
    pub title: Option<Term>,
    /// `--subject`.
    pub subject: Option<Term>,
    /// `--publisher`.
    pub publisher: Option<Term>,
    /// `--author`, always kept as a raw string because it is **always** searched as a
    /// word list, never as a phrase: `"Kafka, Franz"` finds 2911 records, `"Franz
    /// Kafka"` as a phrase finds 40.
    pub author: Option<String>,
    /// `--year`. Means "was running in this year" for serials, "published in" for
    /// monographs.
    pub year: Option<u16>,
    /// `--isbn`/`--issn`.
    pub identifier: Option<Identifier>,
}

impl QuerySpec {
    /// Whether this query would send nothing at all.
    ///
    /// Checked in `cli` before the first byte goes out: an empty query is diagnostic
    /// 1/10 upstream, which would surface as exit 5 for what is plainly a usage error.
    /// Free terms that are only whitespace do not count as content.
    pub fn is_empty(&self) -> bool {
        self.terms.iter().all(Term::is_empty)
            && self.title.is_none()
            && self.subject.is_none()
            && self.publisher.is_none()
            && self.author.is_none()
            && self.year.is_none()
            && self.identifier.is_none()
    }

    /// The free terms as the user typed them, for `query.terms` in the JSON.
    ///
    /// Only the free terms: the flagged fields are reproduced by their own flags, and
    /// folding them in here would produce a string that does not reproduce the search.
    /// Phrases keep their quotes, so the echo can be pasted back onto a command line and
    /// yield the same query. Empty when the query is made of flags alone.
    pub fn echo(&self) -> String {
        self.terms
            .iter()
            .filter(|term| !term.is_empty())
            .map(Term::echo)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One resolved entry of `--at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// What the user typed, normalised to the canonical alias.
    pub key: String,
    /// The canonical ISIL. Never the user's spelling — the upstream filter is
    /// case-sensitive.
    pub isil: Isil,
    /// The branch, when the location names one.
    pub branch: Option<BranchRef>,
    /// Which engine answers for this location.
    pub engine: Engine,
    /// Full display name for the block heading.
    pub display: String,
}

/// A branch of a library network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// The branch's KOBV id, e.g. `SIG00036`.
    pub kobvid: String,
    /// The branch's name.
    pub name: String,
}

/// Everything one engine needs to run one search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The query.
    pub query: QuerySpec,
    /// The locations this engine is responsible for. Empty means "no `--at`".
    pub locations: Vec<Location>,
    /// The record window to fetch.
    pub window: FetchWindow,
    /// Whether per-location totals should be fetched. They cost one extra request each.
    pub want_totals: bool,
}

/// What one engine returned, before selection and before availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSearch {
    /// Which engine produced this.
    pub engine: Engine,
    /// Total hits upstream, when the service states one.
    pub total: Option<u64>,
    /// How many records were delivered.
    pub fetched: usize,
    /// How many records the envelope announced but did not deliver.
    pub undelivered: usize,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// The records.
    pub records: Vec<Record>,
    /// The query as it was sent, for the JSON echo.
    pub query_echo: Option<String>,
    /// Limitations that are not errors.
    pub notes: Vec<Note>,
}

/// One entry of `at[]`: a location, and the true number of hits at it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AtBlock {
    /// The alias as it appears in `--at`.
    pub key: String,
    /// The canonical ISIL.
    pub isil: Isil,
    /// The branch id, when the location is a branch.
    pub branch: Option<String>,
    /// The engine that answered for it.
    pub engine: Engine,
    /// Hits at this location. `None` when the engine cannot state one.
    pub total: Option<u64>,
}

/// Whether availability was fetched.
///
/// This is what makes `items: []` unambiguous. Without it, "we did not ask" and "we asked
/// and got nothing" look identical — exactly the kind of ambiguity an agent cannot
/// resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityMode {
    /// Availability was requested for every displayed record.
    Fetched,
    /// `--no-availability`: nothing was asked.
    Skipped,
}

/// How large the fetched window was and what survived the client-side filters.
///
/// Both numbers are printed. Without them, "no hits" and "no hits *in the first 50*" are
/// indistinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    /// How many records came back.
    pub fetched: usize,
    /// How many were left after `--format`/`--language`.
    pub after_filter: usize,
    /// How many the envelope announced but did not deliver.
    #[serde(skip_serializing_if = "is_zero")]
    pub undelivered: usize,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature is dictated by serde's skip_serializing_if"
)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// How the displayed records were ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SortSpec {
    /// The key.
    pub by: SortKey,
    /// What the sort could see. Always [`SortScope::Fetched`] — SRU cannot sort at all,
    /// so every sort in this tool is client-side over the fetched window. The output
    /// must never imply otherwise.
    pub scope: SortScope,
}

/// What to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Upstream order, which is relevance.
    #[default]
    Relevance,
    /// Publication year, newest first.
    Year,
    /// Displayed title, article included.
    Title,
    /// First author.
    Author,
    /// Availability, best first. Costs no extra request: the data is already there for
    /// every displayed record.
    Availability,
}

/// The scope a sort had access to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortScope {
    /// Only the records that were fetched.
    #[default]
    Fetched,
}

/// A limitation that is not an error and must not be silently swallowed: a per-record
/// diagnostic, `hasAvailability: false`, an item group that carries no ISIL, a branch
/// facet that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Note {
    /// Stable machine-readable tag; one of the constants in [`note_kinds`].
    pub kind: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

impl Note {
    /// Build a note. `kind` is meant to be one of the constants in [`note_kinds`] — an
    /// agent switches on it, so a tag invented at a call site is a silent break.
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// Every value [`Note::kind`] can take.
///
/// The tags are the agent-facing half of a note: the message explains, the tag is what
/// can be branched on, so it is part of the JSON contract exactly like a field name.
/// They live here rather than next to the code that emits them because that code is
/// spread over two parse modules and two engines, and a vocabulary scattered over four
/// files is a vocabulary that grows two spellings of the same thing.
pub mod note_kinds {
    /// The catalogue announced a record and delivered a diagnostic in its place.
    pub const RECORD_UNDELIVERED: &str = "record_undelivered";

    /// A record arrived under a `recordSchema` this tool cannot read.
    pub const RECORD_SCHEMA_UNKNOWN: &str = "record_schema_unknown";

    /// The availability service holds no information for this record
    /// (`hasAvailability: false`); the copies come from the catalogue alone.
    pub const AVAILABILITY_NOT_STATED: &str = "availability_not_stated";

    /// A traffic-light colour was missing or is not one of the four known ones, so a
    /// status is reported as unknown rather than guessed.
    pub const AVAILABILITY_UNKNOWN_STATUS: &str = "availability_unknown_status";

    /// The positional match between traffic lights and blocks of copies did not apply,
    /// and the blocks were matched by library name instead.
    pub const AVAILABILITY_MATCHED_BY_NAME: &str = "availability_matched_by_name";

    /// Order and name disagree about which library a block of copies belongs to; the
    /// order won, and the disagreement is stated rather than hidden.
    pub const AVAILABILITY_MATCH_CONFLICT: &str = "availability_match_conflict";

    /// A block of copies could not be matched to any ISIL and is shown under the
    /// portal's own name for the library.
    pub const HOLDING_WITHOUT_ISIL: &str = "holding_without_isil";

    /// voebb.de listed no copies for a record because the copies belong to the volumes
    /// of a multi-part work, which are records of their own. Not "held nowhere".
    pub const VOEBB_MULTIVOLUME: &str = "voebb_multivolume";

    /// The record is an Onleihe title: it has no copies on a shelf, and its loan state
    /// is stated only as prose in the link to the lending platform.
    pub const VOEBB_ONLINE_ONLY: &str = "voebb_online_only";
}

/// The query, echoed back so a result can be reproduced without the shell history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueryEcho {
    /// The terms as the user typed them.
    pub terms: String,
    /// The assembled PQF query, when a PQF engine ran.
    pub pqf: Option<String>,
}

/// The complete result of one invocation — and the JSON document.
///
/// Record-centric, not location-centric: a record appears **once** in `records` even when
/// it is displayed under three location blocks. The grouping humans see is derived from
/// `at[]` and `holdings[].isil`, so there is one schema, not two.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    /// What was searched.
    pub query: QueryEcho,
    /// Total hits of the KOBV search. `None` when only `voebb` ran.
    pub total: Option<u64>,
    /// How many records are displayed.
    pub shown: usize,
    /// The page that was displayed.
    pub page: Page,
    /// How many records a full page holds — `--limit`.
    ///
    /// Additive, and it exists so that the range of a page can be *computed* rather than
    /// guessed: the first record of this page is `(page - 1) * limit + 1`, and a short
    /// last page must not make the reading aid restart from a smaller stride.
    pub limit: usize,
    /// How the displayed records were ordered.
    pub sort: SortSpec,
    /// What the client-side filters could see.
    pub window: WindowInfo,
    /// Which engines ran.
    pub engines: Vec<Engine>,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// Whether availability was fetched.
    pub availability: AvailabilityMode,
    /// Limitations that are not errors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The records.
    pub records: Vec<Record>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, ResourceUrl};
    use crate::model::{
        AuthorKind, Format, Holding, Item, Limit, RecordId, SortKey, SortScope, Status, UrlKind,
    };

    /// The note vocabulary is the agent-facing half of `notes[]`. Two modules spelling
    /// the same limitation two ways is exactly what the constants prevent, so the list is
    /// checked for duplicates and for the one shape agents can rely on.
    #[test]
    fn note_kinds_are_unique_lowercase_tags() {
        let all = [
            note_kinds::RECORD_UNDELIVERED,
            note_kinds::RECORD_SCHEMA_UNKNOWN,
            note_kinds::AVAILABILITY_NOT_STATED,
            note_kinds::AVAILABILITY_UNKNOWN_STATUS,
            note_kinds::AVAILABILITY_MATCHED_BY_NAME,
            note_kinds::AVAILABILITY_MATCH_CONFLICT,
            note_kinds::HOLDING_WITHOUT_ISIL,
            note_kinds::VOEBB_MULTIVOLUME,
            note_kinds::VOEBB_ONLINE_ONLY,
        ];
        for (index, kind) in all.iter().enumerate() {
            assert!(
                !kind.is_empty()
                    && kind
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{kind:?} is not a snake_case tag"
            );
            assert!(
                !all[index + 1..].contains(kind),
                "{kind:?} is used for two different notes"
            );
        }
    }

    /// The shell strips the quotes, so whitespace is the only evidence left that the user
    /// asked for a phrase.
    #[test]
    fn whitespace_is_the_only_thing_that_makes_a_phrase() {
        assert_eq!(Term::from_argument("Kafka"), Term::Word("Kafka".to_owned()));
        assert_eq!(
            Term::from_argument("Der Prozess"),
            Term::Phrase("Der Prozess".to_owned())
        );
        assert_eq!(
            Term::from_argument("  Kafka  "),
            Term::Word("Kafka".to_owned())
        );
        assert_eq!(
            Term::from_argument("Der\tProzess"),
            Term::Phrase("Der\tProzess".to_owned())
        );
    }

    #[test]
    fn a_whitespace_only_argument_is_an_empty_term() {
        assert!(Term::from_argument("   ").is_empty());
        assert!(!Term::from_argument("Kafka").is_empty());
        assert_eq!(Term::from_argument("Kafka").text(), "Kafka");
    }

    #[test]
    fn a_query_of_only_whitespace_terms_is_empty() {
        assert!(QuerySpec::default().is_empty());
        assert!(
            QuerySpec {
                terms: vec![Term::from_argument("  ")],
                ..QuerySpec::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn any_single_field_makes_a_query_non_empty() {
        let with = |spec: QuerySpec| assert!(!spec.is_empty());
        with(QuerySpec {
            terms: vec![Term::from_argument("Kafka")],
            ..QuerySpec::default()
        });
        with(QuerySpec {
            title: Some(Term::from_argument("Prozess")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            subject: Some(Term::from_argument("Roman")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            publisher: Some(Term::from_argument("Fischer")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            author: Some("Kafka, Franz".to_owned()),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            year: Some(1953),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            identifier: Some(Identifier::Isbn("9783596294331".to_owned())),
            ..QuerySpec::default()
        });
    }

    /// The echo must be paste-able: without the quotes, `blibs search "Der Prozess"`
    /// would come back as two and-ed words and find something else.
    #[test]
    fn the_echo_reproduces_the_free_terms_including_their_quotes() {
        let spec = QuerySpec {
            terms: vec![
                Term::from_argument("Kafka"),
                Term::from_argument("Der Prozess"),
            ],
            ..QuerySpec::default()
        };
        assert_eq!(spec.echo(), "Kafka \"Der Prozess\"");
    }

    /// Flags reproduce themselves; folding them into the term echo would produce a string
    /// that does not run.
    #[test]
    fn the_echo_covers_free_terms_only() {
        let spec = QuerySpec {
            author: Some("Kafka".to_owned()),
            year: Some(1953),
            ..QuerySpec::default()
        };
        assert_eq!(spec.echo(), "");
    }

    /// A fully populated result: both engines, two locations, two records, two holdings
    /// with items on the KOBV one, an `order_option` on the voebb one, and a note.
    fn full_result() -> SearchResult {
        SearchResult {
            query: QueryEcho {
                terms: "Kafka \"Der Prozess\"".to_owned(),
                pqf: Some("@and @attr 1=1016 \"Kafka\" @attr 1=1016 \"Der Prozess\"".to_owned()),
            },
            total: Some(774),
            shown: 2,
            page: Page::FIRST,
            limit: usize::from(Limit::DEFAULT.get()),
            sort: SortSpec {
                by: SortKey::Relevance,
                scope: SortScope::Fetched,
            },
            window: WindowInfo {
                fetched: 50,
                after_filter: 2,
                undelivered: 0,
            },
            engines: vec![Engine::Kobv, Engine::Voebb],
            at: vec![
                AtBlock {
                    key: "HU".to_owned(),
                    isil: Isil::new("DE-11"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: Some(6),
                },
                AtBlock {
                    key: "AGB".to_owned(),
                    isil: Isil::new("DE-609"),
                    branch: Some("SIG00036".to_owned()),
                    engine: Engine::Voebb,
                    total: Some(35),
                },
            ],
            availability: AvailabilityMode::Fetched,
            notes: vec![Note::new(
                note_kinds::RECORD_UNDELIVERED,
                "record 49 of the SRU response was a diagnostic and was skipped",
            )],
            records: vec![kobv_record(), voebb_record()],
        }
    }

    fn kobv_record() -> Record {
        Record {
            id: RecordId::parse("almafu_BV008885798").expect("a prefixed id parses"),
            title: "Der Prozess".to_owned(),
            subtitle: Some("Roman".to_owned()),
            authors: vec![
                Author {
                    name: "Kafka, Franz".to_owned(),
                    kind: AuthorKind::Person,
                    dates: Some("1883-1924".to_owned()),
                    gnd: Some("118559230".to_owned()),
                    role: Some("author".to_owned()),
                },
                Author {
                    name: "S. Fischer Verlag".to_owned(),
                    kind: AuthorKind::Corporate,
                    dates: None,
                    gnd: None,
                    role: None,
                },
            ],
            year: Some(1953),
            publisher: Some("S. Fischer".to_owned()),
            place: Some("Frankfurt am Main".to_owned()),
            edition: Some("2. Auflage".to_owned()),
            extent: Some("345 Seiten".to_owned()),
            languages: vec!["ger".to_owned()],
            format: Format::Book,
            online: false,
            isbns: vec!["9783596294331".to_owned()],
            subjects: vec!["Deutsche Literatur".to_owned(), "Roman".to_owned()],
            urls: vec![ResourceUrl {
                url: "https://d-nb.info/930000000/04".to_owned(),
                kind: UrlKind::Toc,
                label: Some("Inhaltsverzeichnis".to_owned()),
            }],
            holdings: vec![
                Holding {
                    isil: Some(Isil::new("DE-11")),
                    alias: Some("HU".to_owned()),
                    library: "Humboldt-Universität zu Berlin, Universitätsbibliothek".to_owned(),
                    short_name: Some("HU Berlin".to_owned()),
                    local_id: Some("BV008885798".to_owned()),
                    mine: true,
                    summary: Status::Available,
                    items: vec![
                        Item {
                            location: Some("ZB Grimm-Zentrum, 7. OG / Bereich B".to_owned()),
                            branch: Some("KOB00032".to_owned()),
                            branch_name: Some("Grimm-Zentrum".to_owned()),
                            call_number: Some("96 A 10064".to_owned()),
                            volume: None,
                            status: Status::Available,
                            order_option: None,
                        },
                        Item {
                            location: Some("ZB Grimm-Zentrum, Magazin".to_owned()),
                            branch: None,
                            branch_name: None,
                            call_number: Some("96 A 10064+1".to_owned()),
                            volume: Some("1".to_owned()),
                            status: Status::Unavailable,
                            order_option: None,
                        },
                    ],
                },
                // An ISIL that is not in the library list: the display name falls back to
                // the bare code and the holding is still shown.
                Holding {
                    isil: Some(Isil::new("DE-Zz999")),
                    alias: None,
                    library: "DE-Zz999".to_owned(),
                    short_name: None,
                    local_id: Some("275177939".to_owned()),
                    mine: false,
                    summary: Status::Reference,
                    // The portal wrote one of its placeholders in the location cell:
                    // what is not stated is `null`, never an empty string.
                    items: vec![Item {
                        location: None,
                        branch: None,
                        branch_name: None,
                        call_number: Some("A 1234".to_owned()),
                        volume: None,
                        status: Status::Reference,
                        order_option: None,
                    }],
                },
            ],
        }
    }

    fn voebb_record() -> Record {
        Record {
            id: RecordId::voebb("SAK13776205"),
            title: "Der Prozeß".to_owned(),
            subtitle: None,
            authors: vec![Author {
                name: "Kafka, Franz".to_owned(),
                kind: AuthorKind::Person,
                dates: None,
                gnd: None,
                role: None,
            }],
            year: Some(2008),
            publisher: None,
            place: None,
            edition: None,
            extent: None,
            languages: vec![],
            format: Format::Book,
            online: false,
            isbns: vec![],
            subjects: vec![],
            urls: vec![],
            holdings: vec![Holding {
                isil: Some(Isil::new("DE-609")),
                alias: Some("AGB".to_owned()),
                library: "Zentral- und Landesbibliothek Berlin, Amerika-Gedenkbibliothek"
                    .to_owned(),
                short_name: Some("AGB".to_owned()),
                local_id: None,
                mine: true,
                summary: Status::Reference,
                items: vec![Item {
                    location: Some("AGB Erwachsenenbibliothek".to_owned()),
                    branch: Some("SIG00036".to_owned()),
                    branch_name: Some("Amerika-Gedenkbibliothek".to_owned()),
                    call_number: Some("Kaf 1".to_owned()),
                    volume: None,
                    status: Status::Reference,
                    order_option: Some("nicht entleihbar (Freihand) - Präsenzbestand".to_owned()),
                }],
            }],
        }
    }

    /// The JSON document is the agent-facing contract: member **names and order** are
    /// fixed by `plan/cli.md`, missing values are `null` or `[]` and never an empty
    /// string. Adding a member is allowed and shows up here as a snapshot diff to be
    /// reviewed; renaming, reordering or removing one is a break.
    #[test]
    fn the_search_document_matches_the_committed_schema_snapshot() {
        let rendered =
            serde_json::to_string_pretty(&full_result()).expect("a SearchResult always serialises");
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/schema/search.json"
        ));
        assert_eq!(
            rendered.trim_end(),
            expected.trim_end(),
            "the JSON schema changed; review the diff before updating \
             tests/fixtures/schema/search.json"
        );
    }

    /// The record members, in the order `plan/cli.md` fixes them. Spelled out separately
    /// from the snapshot so that a reordering names itself instead of showing up as a
    /// wall of diff.
    #[test]
    fn record_members_appear_in_the_documented_order() {
        // `serde_json` is built with `preserve_order`, so the object keeps the order the
        // struct serialised them in.
        let value = serde_json::to_value(kobv_record()).expect("a Record always serialises");
        let object = value.as_object().expect("a Record is a JSON object");
        let members: Vec<&str> = object.keys().map(String::as_str).collect();
        let expected = [
            "id",
            "engine",
            "source",
            "local_id",
            "title",
            "subtitle",
            "authors",
            "year",
            "publisher",
            "place",
            "edition",
            "extent",
            "languages",
            "format",
            "online",
            "isbns",
            "subjects",
            "urls",
            "holdings",
        ];
        assert_eq!(members, expected);
    }

    /// `undelivered` is additive: absent when it is zero, present the moment SRU
    /// announces more records than it delivers.
    #[test]
    fn undelivered_is_reported_only_when_it_happened() {
        let quiet = serde_json::to_string(&WindowInfo {
            fetched: 50,
            after_filter: 2,
            undelivered: 0,
        })
        .expect("WindowInfo serialises");
        assert_eq!(quiet, r#"{"fetched":50,"after_filter":2}"#);

        let loud = serde_json::to_string(&WindowInfo {
            fetched: 48,
            after_filter: 2,
            undelivered: 2,
        })
        .expect("WindowInfo serialises");
        assert_eq!(loud, r#"{"fetched":48,"after_filter":2,"undelivered":2}"#);
    }

    /// Empty notes vanish from the document; a non-empty one must never be swallowed.
    #[test]
    fn notes_are_omitted_when_there_are_none() {
        let mut result = full_result();
        result.notes.clear();
        let json = serde_json::to_string(&result).expect("a SearchResult always serialises");
        assert!(!json.contains("\"notes\""));
        assert!(
            serde_json::to_string(&full_result())
                .expect("a SearchResult always serialises")
                .contains("\"notes\"")
        );
    }

    /// A holding whose ISIL is not in the library list keeps its display name and is
    /// never dropped — `library` is the one member that is never null.
    #[test]
    fn an_unknown_isil_still_renders_a_library_name() {
        let record = kobv_record();
        let unknown = record
            .holdings
            .last()
            .expect("the fixture record has two holdings");
        assert_eq!(unknown.alias, None);
        assert_eq!(unknown.library, "DE-Zz999");
    }

    /// The window and the fetched size are two different numbers; `plan` is the only
    /// place that relates them.
    #[test]
    fn a_search_request_carries_the_planned_window() {
        let request = SearchRequest {
            query: QuerySpec {
                terms: vec![Term::from_argument("Kafka")],
                ..QuerySpec::default()
            },
            locations: vec![Location {
                key: "HU".to_owned(),
                isil: Isil::new("DE-11"),
                branch: None,
                engine: Engine::Kobv,
                display: "Humboldt-Universität zu Berlin".to_owned(),
            }],
            window: FetchWindow::plan(Limit::new(10).expect("10 is in range"), Page::FIRST, false),
            want_totals: true,
        };
        assert_eq!(request.window.start, 1);
        assert_eq!(request.window.size.get(), 10);
    }
}
