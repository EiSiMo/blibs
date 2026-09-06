//! One bibliographic record and what is known about its copies.
//!
//! The field order in these structs is the field order of the JSON document. It is part
//! of the contract in `plan/cli.md`; reordering is a break, not a cosmetic change.

use crate::model::{Isil, RecordId};

/// A bibliographic record, with the holdings stated in it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Record {
    /// Flattened into `id`, `engine`, `source` and `local_id`.
    #[serde(flatten)]
    pub id: RecordId,
    /// Title as displayed, article included.
    pub title: String,
    /// Subtitle, if the record carries one separately.
    pub subtitle: Option<String>,
    /// Every author, editor and translator in the record, deduplicated but never
    /// filtered by role — filtering by role loses the people users search for.
    pub authors: Vec<Author>,
    /// Publication year. For serials this is the start of the run.
    pub year: Option<i32>,
    /// Publisher.
    pub publisher: Option<String>,
    /// Place of publication.
    pub place: Option<String>,
    /// Edition statement.
    pub edition: Option<String>,
    /// Physical extent, e.g. `345 Seiten`.
    pub extent: Option<String>,
    /// ISO-639-2/B codes exactly as the record has them (`ger`, not `de`).
    pub languages: Vec<String>,
    /// Material type, derived from the leader. Never guessed.
    pub format: Format,
    /// Whether this is an online resource. Carried separately so that the `format`
    /// vocabulary does not need an `e`-variant for every material type.
    pub online: bool,
    /// ISBNs, normalised to digits.
    pub isbns: Vec<String>,
    /// Subject headings, deduplicated by term.
    pub subjects: Vec<String>,
    /// Links from the record, classified. Three quarters of them are covers and tables
    /// of contents, so the kind matters more than the order.
    pub urls: Vec<ResourceUrl>,
    /// Holdings from MARC `924`. Empty means **"not stated in this record"** — 4.9 % of
    /// records have no `924` at all — and never "held nowhere".
    pub holdings: Vec<Holding>,
}

/// A person, body or meeting responsible for the work.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Author {
    /// Name in the form the record uses, usually inverted (`Kafka, Franz`).
    pub name: String,
    /// Which of the three MARC name classes this came from.
    pub kind: AuthorKind,
    /// Life or activity dates, if given.
    pub dates: Option<String>,
    /// GND identifier — always a **string**: check digits with `X` and hyphens occur.
    /// Only `$0` values prefixed `(DE-588)` count; other authority files are ignored.
    pub gnd: Option<String>,
    /// Relator term, e.g. `author`, `editor`.
    pub role: Option<String>,
}

/// The three MARC name classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorKind {
    /// Personal name (`100`/`700`).
    Person,
    /// Corporate body (`110`/`710`).
    Corporate,
    /// Meeting or conference (`111`/`711`).
    Meeting,
}

/// A link from the record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResourceUrl {
    /// The link target.
    pub url: String,
    /// What is behind it, derived from the link text. Never inferred from `ind2`, and
    /// never guessed — an unclassifiable link is [`UrlKind::Other`].
    pub kind: UrlKind,
    /// The link text as the record has it.
    pub label: Option<String>,
}

/// What a link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlKind {
    /// The resource itself.
    Fulltext,
    /// Table of contents.
    Toc,
    /// Cover image.
    Cover,
    /// Publisher or vendor page.
    Publisher,
    /// Anything the link text does not identify.
    Other,
}

/// Material type. A closed vocabulary derived from the leader; an unknown combination is
/// [`Format::Unknown`], never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Printed monograph.
    Book,
    /// Online monograph.
    Ebook,
    /// Printed serial.
    Journal,
    /// Online serial.
    Ejournal,
    /// Analytic — an article in a larger work.
    Article,
    /// Database or integrating resource.
    Database,
    /// Cartographic material.
    Map,
    /// Notated music.
    Score,
    /// Sound recording.
    Audio,
    /// Moving image.
    Video,
    /// Still image or graphic.
    Image,
    /// A computer file that is none of the above.
    Electronic,
    /// Manuscript.
    Manuscript,
    /// Three-dimensional object or realia.
    Object,
    /// Mixed materials.
    Mixed,
    /// The leader combination is not one this tool knows.
    Unknown,
}

impl Format {
    /// Derive the format from leader positions 06 and 07 plus the online flag.
    ///
    /// `online` splits `book`/`ebook` and `journal`/`ejournal`; it comes from
    /// `007/00-01=cr` or `338$b=cr`, not from the leader, which is identical for both.
    pub fn from_leader(_l06: char, _l07: char, _online: bool) -> Self {
        todo!("phase 1: model")
    }
}

/// What one library holds, from one MARC `924` field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Holding {
    /// The library's ISIL, if the field carried one.
    pub isil: Option<Isil>,
    /// The short alias from the library list (`HU`), if this ISIL is known.
    pub alias: Option<String>,
    /// Display name. **Never null** — falls back to the bare ISIL when the code is not
    /// in the list, because an unknown code must never remove a holding.
    pub library: String,
    /// The library's short display name, if known.
    pub short_name: Option<String>,
    /// The library's own record number for this title (`924$a`).
    pub local_id: Option<String>,
    /// Whether this library is one of the `--at` locations.
    pub mine: bool,
    /// The traffic light for this library, summarised over its items.
    pub summary: Status,
    /// The individual copies, once availability has been fetched. Empty is ambiguous on
    /// its own — [`crate::model::AvailabilityMode`] on the result says whether it means
    /// "not asked" or "asked, nothing came back".
    pub items: Vec<Item>,
}

/// One copy on one shelf.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    /// The location cell of the availability table, verbatim.
    pub location: String,
    /// The branch's KOBV id, from the `bibids=` link in the location cell.
    pub branch: Option<String>,
    /// The branch's name, from the text of that link.
    pub branch_name: Option<String>,
    /// Shelfmark.
    pub call_number: Option<String>,
    /// Volume or year, only populated for serials.
    pub volume: Option<String>,
    /// Loan status of this copy.
    pub status: Status,
    /// Ordering option as free text, e.g. `Außenmagazin, bestellbar`. Only voebb.de
    /// supplies these; not an enum, because the vocabulary has never been collected.
    pub order_option: Option<String>,
}

/// Loan status.
///
/// `on loan` never carries a date: due dates live behind a patron login, and this tool
/// never signs in. Output must not suggest a deadline it cannot know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Present and borrowable.
    Available,
    /// Present, in-library use only.
    Reference,
    /// On loan or otherwise not obtainable now.
    Unavailable,
    /// The service says nothing definite. The normal case for public libraries.
    PossiblyAvailable,
    /// Nothing was said at all.
    Unknown,
}

impl Status {
    /// Map a traffic-light colour from `isilAvailability` onto a status.
    ///
    /// `yellow` means `reference`, `black` means `possibly_available`. An unknown colour
    /// returns `None` rather than a guess.
    pub fn from_color(_color: &str) -> Option<Status> {
        todo!("phase 1: model")
    }

    /// The symbol used in terminal output.
    ///
    /// [`Status::PossiblyAvailable`] prints `?`, like [`Status::Unknown`]: `○` would be a
    /// claim the service did not make.
    pub fn symbol(self) -> char {
        todo!("phase 1: model")
    }

    /// Summarise several item statuses into one traffic light.
    ///
    /// Rank: `Available` > `Reference` > `PossiblyAvailable` > `Unavailable` >
    /// `Unknown`. An empty iterator summarises to [`Status::Unknown`].
    pub fn summarize(_items: impl IntoIterator<Item = Status>) -> Status {
        todo!("phase 1: model")
    }
}
