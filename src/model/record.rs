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

impl Record {
    /// Whether this is a resource nobody carries off a shelf.
    ///
    /// The question every sentence about a *loan* has to ask first. The availability
    /// service answers `{"color":"red","text":"not available"}` and says nothing about
    /// borrowing; for a printed book "on loan" is a fair reading of that, and for an
    /// online resource with a full-text link it is an invented fact — `blibs show
    /// gbv_1832903613` printed `on loan` under an `E-book` with a working DOI, and the
    /// note about missing due dates underneath it (round 2, §1.9).
    ///
    /// One predicate, because the copy line and that note must not disagree about the same
    /// record. Both halves are asked: [`Self::online`] comes from `007`/`338$b` and says
    /// the carrier is a network resource, while the `e`-formats say what the thing is.
    /// Either one is enough to rule out a shelf.
    pub fn is_online_resource(&self) -> bool {
        self.online
            || matches!(
                self.format,
                Format::Ebook | Format::Ejournal | Format::Electronic | Format::Database
            )
    }
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
    /// Relator term, free text — `$e`, e.g. `Verfasser/in`, `Übers.`.
    pub role: Option<String>,
    /// Relator code from MARC `$4`, normalised to its last `/`-segment when the record
    /// carries it as a `LoC` relator URI. **Not a closed vocabulary** — besides the `LoC`
    /// relator codes it also carries German extensions with no `LoC` counterpart (`kom`,
    /// `isb`, `dgg`, `dgs`, `wac`, seen in the wild). The voebb engine never fills this: it
    /// exposes no relator codes at all, so `None` there means "this catalogue does not
    /// carry codes", not a data loss relative to `kobv`.
    pub role_code: Option<String>,
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
    /// The vocabulary word for this material type — the same string the JSON carries and
    /// the same one `--format` accepts. A closed vocabulary has to be able to name itself:
    /// the value has to be quotable in a message ("none matched --format video") without
    /// a second table that can drift from this one.
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Book => "book",
            Format::Ebook => "ebook",
            Format::Journal => "journal",
            Format::Ejournal => "ejournal",
            Format::Article => "article",
            Format::Database => "database",
            Format::Map => "map",
            Format::Score => "score",
            Format::Audio => "audio",
            Format::Video => "video",
            Format::Image => "image",
            Format::Electronic => "electronic",
            Format::Manuscript => "manuscript",
            Format::Object => "object",
            Format::Mixed => "mixed",
            Format::Unknown => "unknown",
        }
    }

    /// Derive the format from leader positions 06 and 07 plus the online flag.
    ///
    /// `online` splits `book`/`ebook` and `journal`/`ejournal`; it comes from
    /// `007/00-01=cr` or `338$b=cr`, not from the leader, which is identical for both.
    ///
    /// The rule is two-stage: the material comes from position 06, and **only** for `a`
    /// (textual material) position 07 then splits monograph from serial from analytic.
    /// A combination that is in neither table is [`Format::Unknown`] — the leader is
    /// sometimes shifted (`||`, `20` occur) and a shifted leader must never make the
    /// record disappear.
    pub fn from_leader(l06: char, l07: char, online: bool) -> Self {
        match l06 {
            'a' => Self::from_text_leader(l07, online),
            't' => Format::Manuscript,
            // `d` is manuscript music, which is still notated music.
            'c' | 'd' => Format::Score,
            'e' | 'f' => Format::Map,
            'g' => Format::Video,
            'i' | 'j' => Format::Audio,
            'k' => Format::Image,
            // Not `software`: half of these records are e-book collections and
            // digitised journals, the other half CD-ROMs.
            'm' => Format::Electronic,
            'o' | 'p' => Format::Mixed,
            'r' => Format::Object,
            _ => Format::Unknown,
        }
    }

    /// Stage two, for leader/06 = `a`. This is the only place where `online` enters the
    /// `format` value at all: `database` and `article` are online almost by definition and
    /// get no `e`-variant, and for the other materials an `e`-variant would be invented.
    /// Every record carries `online` separately, so nothing is lost either way.
    fn from_text_leader(l07: char, online: bool) -> Self {
        match l07 {
            's' if online => Format::Ejournal,
            's' => Format::Journal,
            'i' => Format::Database,
            'a' | 'b' => Format::Article,
            // `m`, `c`, `d` and — deliberately — everything else: an unrecognised
            // bibliographic level on textual material is still text.
            _ if online => Format::Ebook,
            _ => Format::Book,
        }
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
    /// What the record states about this library's holdings **in prose**, verbatim.
    ///
    /// voebb.de's `Bestand` row, measured 2026-09-08 on `voebb_SAK13708822`:
    /// `Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.) Mikrofilm Standort: BStB
    /// Signatur: A 80 ZC 181 Beil.:Mikro`. It is carried **whole** and never split into
    /// a location and a shelfmark: the words `Standort:` and `Signatur:` inside it are
    /// free text and not a grammar — one of the two measured lines has neither — and a
    /// shelfmark guessed out of it would be quoted as though the catalogue had stated it
    /// as one.
    ///
    /// This is where a record with no copies of its own says what is held: such a record
    /// has an **empty** item table, so a filled statement next to an empty [`Self::items`]
    /// is the normal shape and not a loss. `None` for every KOBV holding — the union
    /// catalogue states its holdings as `924` fields, which arrive as items.
    pub holdings_statement: Option<String>,
    /// The individual copies, once availability has been fetched. Empty is ambiguous on
    /// its own — [`crate::model::AvailabilityMode`] on the result says whether it means
    /// "not asked" or "asked, nothing came back".
    pub items: Vec<Item>,
}

/// One copy on one shelf.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    /// The location cell of the availability table, verbatim. `None` when the cell was
    /// empty or held one of the portal's placeholders (`Library`, `Bibliothek`, `'`):
    /// what is not stated is null, never an empty string that renders as a blank shelf.
    pub location: Option<String>,
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
    /// The return date of a copy that is out, as **ISO-8601** (`2026-09-22`).
    ///
    /// voebb.de writes it into the availability cell behind the status word
    /// (`Ausgeliehen -  Fällig am: 22.9.2026`), with the day *and* the month unpadded —
    /// measured 2026-09-08, eight of them on `voebb_SAK34906286` alone. The JSON form is
    /// the ISO one because this tool is written for agents as much as for people; the
    /// German form is never passed through.
    ///
    /// `None` says the catalogue stated none, which is the normal case for a copy that is
    /// in, and the only case for KOBV — its availability service reports no return dates
    /// at all. A stated date this tool cannot read is never bent into shape: it stays
    /// `None` and the result carries `voebb_due_date_unreadable`, which names the form
    /// that was expected rather than the text that was not it — a date differs per copy,
    /// and a message that differs never folds into one note.
    ///
    /// It never decides [`Self::status`]. That comes from the marker class alone, so a
    /// changed date format costs a date and never a traffic light.
    pub due_date: Option<String>,
    /// Ordering option as free text, e.g. `Außenmagazin, bestellbar`. Only voebb.de
    /// supplies these; not an enum, because the vocabulary has never been collected.
    pub order_option: Option<String>,
}

/// Loan status.
///
/// A status is a light and never a date. Where a catalogue does state a return date it
/// travels beside the light in [`Item::due_date`], and voebb.de does state one — eight on
/// `voebb_SAK34906286`, measured 2026-09-08, which is what this comment used to deny.
/// KOBV's availability service states none, and there the output must not suggest a
/// deadline it cannot know.
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
    pub fn from_color(color: &str) -> Option<Status> {
        match color.trim() {
            "green" => Some(Status::Available),
            "yellow" => Some(Status::Reference),
            "red" => Some(Status::Unavailable),
            "black" => Some(Status::PossiblyAvailable),
            _ => None,
        }
    }

    /// The symbol used in terminal output.
    ///
    /// [`Status::PossiblyAvailable`] prints `?`, like [`Status::Unknown`]: `○` would be a
    /// claim the service did not make.
    pub fn symbol(self) -> char {
        match self {
            Status::Available => '●',
            Status::Reference => '◐',
            Status::Unavailable => '○',
            Status::PossiblyAvailable | Status::Unknown => '?',
        }
    }

    /// Ordering rank, best first: `Available` 4 … `Unknown` 0.
    ///
    /// Crate-visible because [`Status::summarize`] and the availability sort in
    /// [`crate::select`] must agree on one order; a second copy of the table elsewhere
    /// would be free to drift out of step with this one. Not part of the public
    /// interface — the numbers are an ordering, not a value anything may serialise.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Status::Available => 4,
            Status::Reference => 3,
            Status::PossiblyAvailable => 2,
            Status::Unavailable => 1,
            Status::Unknown => 0,
        }
    }

    /// Summarise several item statuses into one traffic light.
    ///
    /// Rank: `Available` > `Reference` > `PossiblyAvailable` > `Unavailable` >
    /// `Unknown`. An empty iterator summarises to [`Status::Unknown`].
    pub fn summarize(items: impl IntoIterator<Item = Status>) -> Status {
        items
            .into_iter()
            .max_by_key(|status| status.rank())
            .unwrap_or(Status::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every leader combination observed in the 1396-record sample, plus the two shifted
    /// leaders. Source: `plan/marc-mapping.md` § format.
    #[test]
    fn the_observed_leader_combinations_map_as_measured() {
        let cases = [
            (('a', 'm'), Format::Book),
            (('a', 's'), Format::Journal),
            (('a', 'i'), Format::Database),
            (('a', 'a'), Format::Article),
            (('a', 'b'), Format::Article),
            (('a', 'c'), Format::Book),
            (('a', 'd'), Format::Book),
            (('e', 'm'), Format::Map),
            (('f', 'm'), Format::Map),
            (('m', 'm'), Format::Electronic),
            (('m', 'i'), Format::Electronic),
            (('m', 's'), Format::Electronic),
            (('j', 'm'), Format::Audio),
            (('j', 'a'), Format::Audio),
            (('i', 'm'), Format::Audio),
            (('c', 'm'), Format::Score),
            (('c', 'a'), Format::Score),
            (('d', 'm'), Format::Score),
            (('d', 'a'), Format::Score),
            (('g', 'm'), Format::Video),
            (('g', 'a'), Format::Video),
            (('p', 'm'), Format::Mixed),
            (('o', 'm'), Format::Mixed),
            (('k', 'm'), Format::Image),
            (('t', 'm'), Format::Manuscript),
            (('r', 'm'), Format::Object),
        ];
        for ((l06, l07), expected) in cases {
            assert_eq!(
                Format::from_leader(l06, l07, false),
                expected,
                "leader/06={l06} leader/07={l07}"
            );
        }
    }

    /// Four records carry `||` (not coded) and two have a shifted leader that reads `20`.
    /// Neither is an error and neither may be guessed at.
    #[test]
    fn an_uncoded_or_shifted_leader_is_unknown() {
        assert_eq!(Format::from_leader('|', '|', false), Format::Unknown);
        assert_eq!(Format::from_leader('2', '0', false), Format::Unknown);
        assert_eq!(Format::from_leader('|', '|', true), Format::Unknown);
        assert_eq!(Format::from_leader('2', '0', true), Format::Unknown);
    }

    /// E-books look exactly like printed books in the leader; only `007`/`338` tell them
    /// apart, and that arrives here as `online`.
    #[test]
    fn online_splits_only_the_text_formats() {
        assert_eq!(Format::from_leader('a', 'm', true), Format::Ebook);
        assert_eq!(Format::from_leader('a', 's', true), Format::Ejournal);
        // No `e`-variant is invented for the rest.
        assert_eq!(Format::from_leader('a', 'i', true), Format::Database);
        assert_eq!(Format::from_leader('a', 'a', true), Format::Article);
        assert_eq!(Format::from_leader('m', 'm', true), Format::Electronic);
        assert_eq!(Format::from_leader('e', 'm', true), Format::Map);
        assert_eq!(Format::from_leader('c', 'm', true), Format::Score);
    }

    /// An unrecognised bibliographic level on textual material is still text — the
    /// fallback of the second table, not `unknown`.
    #[test]
    fn an_unknown_level_on_text_falls_back_to_book() {
        assert_eq!(Format::from_leader('a', 'z', false), Format::Book);
        assert_eq!(Format::from_leader('a', 'z', true), Format::Ebook);
    }

    #[test]
    fn formats_serialise_lowercase() {
        let json = serde_json::to_string(&Format::Ejournal).expect("Format serialises");
        assert_eq!(json, "\"ejournal\"");
    }

    #[test]
    fn the_four_traffic_light_colours_map_onto_statuses() {
        assert_eq!(Status::from_color("green"), Some(Status::Available));
        assert_eq!(Status::from_color("yellow"), Some(Status::Reference));
        assert_eq!(Status::from_color("red"), Some(Status::Unavailable));
        assert_eq!(Status::from_color("black"), Some(Status::PossiblyAvailable));
    }

    #[test]
    fn an_unknown_colour_is_none_not_a_guess() {
        assert_eq!(Status::from_color("blue"), None);
        assert_eq!(Status::from_color(""), None);
        assert_eq!(Status::from_color("GREEN"), None);
    }

    /// `? ` for `possibly_available` as well: `○` would claim the service said the copy
    /// is out, which it did not.
    #[test]
    fn possibly_available_prints_a_question_mark_like_unknown() {
        assert_eq!(Status::Available.symbol(), '●');
        assert_eq!(Status::Reference.symbol(), '◐');
        assert_eq!(Status::Unavailable.symbol(), '○');
        assert_eq!(Status::PossiblyAvailable.symbol(), '?');
        assert_eq!(Status::Unknown.symbol(), '?');
    }

    #[test]
    fn summarize_takes_the_best_status() {
        assert_eq!(
            Status::summarize([Status::Unavailable, Status::Available]),
            Status::Available
        );
        assert_eq!(
            Status::summarize([Status::Unknown, Status::Reference, Status::Unavailable]),
            Status::Reference
        );
        assert_eq!(
            Status::summarize([Status::Unavailable, Status::PossiblyAvailable]),
            Status::PossiblyAvailable
        );
        assert_eq!(
            Status::summarize([Status::Unknown, Status::Unavailable]),
            Status::Unavailable
        );
        assert_eq!(Status::summarize([Status::Unknown]), Status::Unknown);
    }

    /// A library with no items is not "nothing available", it is "nothing was said".
    #[test]
    fn summarizing_nothing_is_unknown() {
        assert_eq!(Status::summarize([]), Status::Unknown);
    }

    #[test]
    fn statuses_serialise_snake_case() {
        let json = serde_json::to_string(&Status::PossiblyAvailable).expect("Status serialises");
        assert_eq!(json, "\"possibly_available\"");
    }
}
