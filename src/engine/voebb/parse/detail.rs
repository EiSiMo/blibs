//! A record's detail page: the bibliographic tables and the complete item table.
//!
//! Two structures carry everything this module reads, and both are fixture-backed
//! (`tests/fixtures/voebb/detail_*.html`):
//!
//! - **`table.gi`** — one row per field, `<th scope="row">Label</th><td>Wert</td>`. The
//!   label vocabulary is open: an unknown label is ignored, never an error, exactly like
//!   an unknown ISIL in the library list. A page without any `table.gi` is a named error
//!   ([`crate::error::UnexpectedError::MissingSelector`]), never an empty record.
//! - **`table#resptable-1`** — the copies. Its columns are mapped by their `<th>` text,
//!   never by position: the table has **five** columns for `detail_available` and
//!   **four** for `detail_on_loan`, where the shelfmark sits behind the area in
//!   *Standort* instead of in a column of its own.
//!
//! Three traps live here, each of which produces a *wrong* answer rather than a failure
//! when got wrong.
//!
//! **Präsenz is not availability.** `nicht entleihbar (Freihand) - Präsenzbestand` stands
//! next to `Verfügbarkeit = Verfügbar` (`detail_reference`). Reading only the availability
//! column promises a loan that does not exist, so the order column decides between
//! [`Status::Available`] and [`Status::Reference`].
//!
//! **The item table has four states, and three of them are empty for different reasons.**
//! Present with rows is the normal case; present without a header row is a multi-part
//! work whose volumes are records of their own (`detail_multivolume`); absent with a
//! lending link is an electronic title whose loan state sits in the *text* of that link
//! (`detail_online`, `detail_online_available`, `detail_overdrive`) and is read from
//! there by [`lending_status`]; absent with no lending link at all is an electronic title
//! whose access is a plain `URL` row and whose loan state is stated nowhere
//! (`detail_online_url`, [`online_access_url`]). Each empty item list carries a note
//! saying which — an empty list on its own is indistinguishable from "held nowhere",
//! which is the worst answer this tool can give.
//!
//! A fifth shape is not a state but a failure, and it costs **one record** rather than
//! the answer: a page with no readable item table and none of those three explanations
//! yields [`crate::model::note_kinds::VOEBB_PAGE_UNREADABLE`] and a record without
//! copies. Only a page whose *bibliographic* half is gone is still an error — there is no
//! record left to report at that point.
//!
//! **The item table names no ISIL and no branch id.** The branch is matched from the text
//! of the *Bibliothek* column against the VÖBB branches of the library list, folded and
//! in stages. A name the list does not carry — **and a name it carries twice**, such as
//! the `ZLB: Außenmagazin` that is either of the ZLB's two outlying stacks — keeps its
//! text in [`crate::model::Item::branch_name`] and leaves [`crate::model::Item::branch`]
//! empty: it is never guessed at, and the copy is never dropped.

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};

use crate::error::{Error, REPORT_URL};
use crate::libraries::{self, Branch, VOEBB_NETWORK, text::fold};
use crate::model::{
    Author, AuthorKind, Format, Holding, Isil, Item, Note, Record, RecordId, ResourceUrl, Status,
    UrlKind, note_kinds,
};

use super::{collapse, compile};

/// What names this document in an error message.
const DOCUMENT: &str = "voebb detail page";

/// One record with everything its detail page states, including every copy the network
/// holds — voebb.de delivers the item list with the record, so no second request follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailPage {
    /// The bibliographic record, holdings included.
    pub record: Record,
    /// What the page could not state cleanly: an item table that is empty and why, a
    /// status word this tool does not know. Never an error — but never silent either.
    pub notes: Vec<Note>,
}

/// Parse a detail page into a record.
///
/// `id` is the id the page was fetched under and becomes the record's identity verbatim:
/// the page itself repeats the number in several shapes, and reconstructing it from one
/// of them is how a `voebb_` prefix gets lost.
///
/// Fails only when the page is not a **detail page at all** — no `table.gi`, no `Titel`
/// row. Everything below that line is a note, the item table included: a table this
/// parser cannot read costs the record its copies and nothing else, because the record
/// is right there on the page and throwing it away over its copy list answers less than
/// keeping it. The record then carries no items and [`Status::Unknown`], and
/// [`page_unreadable`] says which selector stopped matching.
pub fn parse_detail(html: &str, id: &RecordId) -> Result<DetailPage, Error> {
    let document = Html::parse_document(html);
    let bibliographic = Bibliographic::read(&document)?;
    let mut notes = Vec::new();
    let items = match items_of(&document, &bibliographic, &mut notes) {
        Ok(items) => items,
        Err(error) => {
            notes.push(page_unreadable(&error));
            Vec::new()
        }
    };
    let record = record_of(id, &bibliographic, items);
    // Every note this page produced is about this one record, and it is named here
    // rather than at each `push` because that is true of all of them without exception:
    // the page *is* the record. Three `voebb_online_only` notes over two blocks are
    // guesswork otherwise, and `message` is prose an agent may not parse.
    for note in &mut notes {
        note.records.push(id.clone());
    }
    Ok(DetailPage { record, notes })
}

/// The note that stands in for a record page this parser could not read.
///
/// The **one** place this note is worded, and both paths reach it: this module builds it
/// when the item table alone is unreadable and the record survives, and
/// [`crate::engine::voebb`] builds it when the whole page is unreadable and the result
/// row is all that is left. One rule with two spellings is how `show` once painted a
/// green light over a book a branch had lent out, and a note is no safer than a filter.
///
/// The caller decides *whether* an error may become a note —
/// [`crate::error::Error::is_unreadable_document`] is that decision, and a timeout must
/// never arrive here. This function only words it.
///
/// The message carries the failure verbatim: it names the selector, which is the only
/// thing that tells a maintainer what the site changed. It names no record — the note's
/// `records[]` does that, and a message that enumerated them could not be folded together
/// with the identical note about the next record.
pub fn page_unreadable(error: &Error) -> Note {
    Note::new(
        note_kinds::VOEBB_PAGE_UNREADABLE,
        format!(
            "voebb.de's record page is no longer a page blibs can read in full ({error}), \
             so no copies could be listed for it — the record itself stands, its copies \
             are simply unknown here; please report that selector at {REPORT_URL}"
        ),
    )
}

/// Whether this page is voebb.de's answer to a record number it does not hold.
///
/// Measured 2026-09-06 with `sp=SAK00000000`: the site answers an unknown number with
/// **its search entry page** — status 200, a valid `Form0`, and none of the containers a
/// record page has. Read as a detail page that would be a missing selector; read as a
/// result list it would be "no hits". It is neither, so it is recognised here and turned
/// into `Ok(None)` by the client.
///
/// Two positive signals are required, not one. The absence of `table.gi` alone would make
/// every future redesign of the record page look like a record that does not exist —
/// which is exactly the silent wrong answer this crate exists to avoid. So the entry
/// page's own container (`div#R04`) has to be there as well, and the record page's
/// (`div#R03`) has to be absent.
pub fn is_missing_record(html: &str) -> bool {
    let document = Html::parse_document(html);
    document.select(&selectors().bib_table).next().is_none()
        && document.select(&selectors().record_body).next().is_none()
        && document.select(&selectors().entry_body).next().is_some()
}

/// Assemble the record from the bibliographic tables and the copies.
///
/// Every field is filled by one small function below, so that a change to voebb.de's
/// wording touches exactly one of them.
fn record_of(id: &RecordId, bibliographic: &Bibliographic, items: Vec<Item>) -> Record {
    let (title, subtitle) = title_of(bibliographic.first("Titel").unwrap_or_default());
    let publication = bibliographic.first("Veröffentlichung").unwrap_or_default();
    let (place, publisher) = imprint_of(publication);
    let link = lending_link(bibliographic);
    let (format, online) = format_of(
        bibliographic.first("Medienart").unwrap_or_default(),
        link.is_some(),
    );

    Record {
        id: id.clone(),
        title,
        subtitle,
        authors: authors_of(bibliographic),
        year: year_of(publication),
        publisher,
        place,
        edition: bibliographic.first("Ausgabe").map(str::to_string),
        extent: bibliographic.first("Umfang / Format").map(str::to_string),
        languages: languages_of(bibliographic.values("Sprache")),
        format,
        online,
        isbns: isbns_of(bibliographic.values("ISBN")),
        subjects: subjects_of(bibliographic.values("Schlagwortkette")),
        urls: urls_of(bibliographic, id),
        holdings: vec![holding_of(
            id,
            items,
            lending_status(link),
            holdings_statement_of(bibliographic),
        )],
    }
}

/// The one holding of a voebb.de record: the network itself.
///
/// voebb.de answers for `DE-609` and nothing else, so a record has exactly one holding
/// and the branches live in its copies. An empty item list means "the page stated no
/// copies", never "held nowhere" — the note that comes with it says which of the two
/// states of the item table produced it.
///
/// `without_items` is the light for exactly that case: an electronic title has no copies
/// to summarise and states its loan state in the text of its lending link instead
/// ([`lending_status`]). It is only ever consulted when `items` is empty — a copy on a
/// shelf is a stronger statement than a sentence in a link, and where both exist the
/// copies decide.
///
/// `holdings_statement` is the *other* thing a record with no copies can still say: the
/// prose `Bestand` line ([`holdings_statement_of`]). It is carried verbatim and never
/// turned into a status — what a run of volumes says about a loan is nothing.
fn holding_of(
    id: &RecordId,
    items: Vec<Item>,
    without_items: Status,
    holdings_statement: Option<String>,
) -> Holding {
    let summary = if items.is_empty() {
        without_items
    } else {
        Status::summarize(items.iter().map(|item| item.status))
    };
    let mut holding = Holding {
        isil: Some(Isil::new(VOEBB_NETWORK)),
        alias: None,
        library: String::new(),
        short_name: None,
        local_id: Some(id.local_id().to_string()),
        mine: false,
        summary,
        holdings_statement,
        items,
    };
    // The naming rule lives in one place for both engines; this parser only supplies the
    // ISIL and the local number.
    libraries::name_holding(&mut holding);
    holding
}

/// Title and subtitle out of the ISBD title statement.
///
/// voebb.de writes one field, `Titel : Untertitel / Verantwortlichkeit`, and the
/// separators are the ISBD ones — **space-delimited**, which is what keeps
/// `Bernhard Schlink: Der Vorleser` (`detail_online`) in one piece while
/// `Der Vorleser : Roman` (`detail_on_loan`) splits. The statement of responsibility is
/// dropped here: the same people arrive properly separated in `Person`/`Verfasser`.
fn title_of(statement: &str) -> (String, Option<String>) {
    let without_responsibility = statement
        .split_once(" / ")
        .map_or(statement, |(title, _)| title);
    match without_responsibility.split_once(" : ") {
        Some((title, subtitle)) => (title.trim().to_string(), non_empty(subtitle.trim())),
        None => (without_responsibility.trim().to_string(), None),
    }
}

/// Place and publisher out of `Ort : Verlag, Jahr`.
///
/// The year is cut off the publisher only when the tail actually is one, so that
/// `Heilbronn, Neckar : Kleist-Archiv Sembdner, 2015` keeps its comma'd place and
/// `Leipzig : Brockhaus` (no year at all, `detail_multivolume`) keeps its publisher.
fn imprint_of(publication: &str) -> (Option<String>, Option<String>) {
    let Some((place, rest)) = publication.split_once(" : ") else {
        return (None, non_empty(publication.trim()));
    };
    let publisher = match rest.rsplit_once(", ") {
        Some((head, tail)) if year_of(tail).is_some() => head,
        _ => rest,
    };
    (non_empty(place.trim()), non_empty(publisher.trim()))
}

/// The publication year: the last four-digit run in the statement.
///
/// The last, not the first: `Berlin : Cornelsen, 2004` puts it at the end, and a place or
/// a series title can carry digits of its own. Anything outside 1000–2999 is not a year
/// this catalogue can hold and is left out rather than reported.
fn year_of(publication: &str) -> Option<i32> {
    let mut year = None;
    for run in publication.split(|c: char| !c.is_ascii_digit()) {
        if run.len() == 4
            && let Ok(value) = run.parse::<i32>()
            && (1000..3000).contains(&value)
        {
            year = Some(value);
        }
    }
    year
}

/// Everyone the page names, from `Person` and `Verfasser` alike.
///
/// The role is the bracketed suffix voebb.de appends (`Bienert, Michael [Vorredner/in]`),
/// carried **verbatim and in German**: this catalogue states no relator codes, and
/// translating `Vorredner/in` into a MARC term would be an invention. Everyone is a
/// [`AuthorKind::Person`] — the page draws no line between persons and bodies, and
/// guessing one from the shape of a name is how `Deutscher Bundestag` becomes a person.
fn authors_of(bibliographic: &Bibliographic) -> Vec<Author> {
    let mut authors: Vec<Author> = Vec::new();
    for line in bibliographic
        .values("Verfasser")
        .iter()
        .chain(bibliographic.values("Person"))
    {
        let (name, role) = split_role(line);
        if name.is_empty() || authors.iter().any(|author| author.name == name) {
            continue;
        }
        authors.push(Author {
            name,
            kind: AuthorKind::Person,
            dates: None,
            gnd: None,
            role,
            // voebb.de states the role in words ("Verfasser"), never a MARC relator code.
            role_code: None,
        });
    }
    authors
}

/// Split `Name [Rolle]` into its two halves. A line without brackets is all name.
fn split_role(line: &str) -> (String, Option<String>) {
    let Some(open) = line.rfind('[') else {
        return (line.trim().to_string(), None);
    };
    let Some(close) = line[open..].find(']').map(|offset| open + offset) else {
        return (line.trim().to_string(), None);
    };
    (
        line[..open].trim().to_string(),
        non_empty(line[open + 1..close].trim()),
    )
}

/// Language codes for the language names voebb.de prints.
///
/// The page states a **name**, not a code, so this table is the only way to reach the
/// ISO-639-2/B codes the JSON schema promises (`ger`, not `de`, and not `Deutsch`). It
/// covers the languages of the Berlin public catalogue and nothing else: a name that is
/// not in it is **left out**, because a language guessed from a prefix is worse than a
/// language not stated. `Deutsch` is the only entry the fixtures prove; the rest follow
/// the same 639-2/B column of the same standard.
const LANGUAGE_CODES: &[(&str, &str)] = &[
    ("Deutsch", "ger"),
    ("Englisch", "eng"),
    ("Französisch", "fre"),
    ("Spanisch", "spa"),
    ("Italienisch", "ita"),
    ("Niederländisch", "dut"),
    ("Polnisch", "pol"),
    ("Russisch", "rus"),
    ("Türkisch", "tur"),
    ("Arabisch", "ara"),
    ("Portugiesisch", "por"),
    ("Schwedisch", "swe"),
    ("Dänisch", "dan"),
    ("Norwegisch", "nor"),
    ("Finnisch", "fin"),
    ("Tschechisch", "cze"),
    ("Griechisch", "gre"),
    ("Latein", "lat"),
    ("Chinesisch", "chi"),
    ("Japanisch", "jpn"),
    ("Ungarisch", "hun"),
    ("Rumänisch", "rum"),
    ("Ukrainisch", "ukr"),
    ("Hebräisch", "heb"),
    ("Persisch", "per"),
    ("Vietnamesisch", "vie"),
    ("Koreanisch", "kor"),
];

/// The language codes of a `Sprache` cell. See [`LANGUAGE_CODES`] for what is not mapped.
fn languages_of(values: &[String]) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for value in values {
        let name = fold(value);
        if let Some((_, code)) = LANGUAGE_CODES
            .iter()
            .find(|(german, _)| fold(german) == name)
            && !codes.iter().any(|known| known == code)
        {
            codes.push((*code).to_string());
        }
    }
    codes
}

/// Material type and the online flag, from `Medienart`.
///
/// The value arrives in square brackets (`[Band]`, `[E-Ressource]`,
/// `[Mehrteiliges Werk]`). The vocabulary is open, so this table maps what has been
/// measured and leaves everything else [`Format::Unknown`] rather than guessing:
///
/// | Medienart | Format |
/// | --- | --- |
/// | `Band`, `Buch` | [`Format::Book`] |
/// | `E-Ressource`, `E-Book`, `Onleihe` | [`Format::Ebook`], and `online` |
/// | `E-Audio` | [`Format::Audio`], and `online` |
/// | `Hörbuch`, `CD`, `Tonträger` | [`Format::Audio`] |
/// | `DVD`, `Blu-ray`, `Video` | [`Format::Video`] |
/// | `Noten` | [`Format::Score`] |
/// | `Karte/Plan` | [`Format::Map`] |
/// | `Zeitung`, `Zeitschrift`, `Zeitschriftenartige Reihe`, `Zeitschriftenheft` | [`Format::Journal`] |
/// | `Konsolenspiel` | [`Format::Object`] |
/// | `Medienkombination` | [`Format::Mixed`] |
/// | anything else, `Mehrteiliges Werk` included | [`Format::Unknown`] |
///
/// `Mehrteiliges Werk` is deliberately in the last row: it states a bibliographic
/// *level*, not a material, and the volumes that carry the material are records of their
/// own. `online` also comes in from a `Link zu …` row, which is why it is an
/// argument here and not derived from the table alone.
///
/// Six rows were added on 2026-09-08, after a sample of 23 records across the material
/// types found that many `Medienart` values falling through to [`Format::Unknown`] and
/// out of every `--format` filter. Two of the decisions deserve their reason stated:
///
/// - `[E-Audio]` is [`Format::Audio`] *plus* `online`, not an `e`-variant of its own: the
///   vocabulary deliberately has no `e`-form per material, and every record carries
///   `online` separately ([`Format`]).
/// - `[Konsolenspiel]` is [`Format::Object`] — **not** [`Format::Electronic`], and that is
///   a trap rather than an oversight, so please do not tidy it. `Electronic` reads like
///   the obvious answer (MARC codes a video game as leader/06 `m`, a computer file), but
///   in this crate that word does not mean "a computer file", it means "a resource nobody
///   carries off a shelf": [`crate::model::Record::is_online_resource`] counts it among
///   the online formats. A cartridge on a shelf in Spandau is not that, and the mapping
///   was measured doing the damage — the copy that is out turned from `on loan` into
///   `currently unavailable` and the note about the missing due date disappeared with it.
///   `Object` is the only one of the fifteen words that claims nothing false: a carrier on
///   a shelf is a thing. A vocabulary word of its own was considered and rejected —
///   `--format game` would be a filter `kobv` can never answer, and the vocabulary is
///   filled by both engines.
///
/// Two values in the table were not in the sample. `Zeitschrift` stands beside the three
/// serial spellings that were measured, because leaving the plainest of them out would be
/// a hole rather than a caution. No second *game* spelling is here: the 23 pages were
/// searched for one and carry none — `Konsolenspiel` is the only one, and the `Spiel` on
/// `voebb_SAK34906286` is the value of its `Art/Inhalt` row, not a `Medienart`. Guessing
/// at whether the site would write `Brettspiel`, `Gesellschaftsspiel` or `Spiel` would be
/// inventing vocabulary, not extending it.
pub(in crate::engine::voebb) fn format_of(medienart: &str, online: bool) -> (Format, bool) {
    let value = fold(medienart.trim_matches(['[', ']']).trim());
    let format = match value.as_str() {
        "band" | "buch" => Format::Book,
        "e-ressource" | "e-book" | "onleihe" => return (Format::Ebook, true),
        "e-audio" => return (Format::Audio, true),
        "horbuch" | "cd" | "tontrager" => Format::Audio,
        "dvd" | "blu-ray" | "video" => Format::Video,
        "noten" => Format::Score,
        "karte/plan" => Format::Map,
        "zeitung" | "zeitschrift" | "zeitschriftenartige reihe" | "zeitschriftenheft" => {
            Format::Journal
        }
        // `Object`, never `Electronic` — see the note above this function; the wrong one
        // of those two turns a cartridge on a shelf into an online resource.
        "konsolenspiel" => Format::Object,
        "medienkombination" => Format::Mixed,
        _ => Format::Unknown,
    };
    (format, online)
}

/// The ISBNs of an `ISBN` cell, hyphens removed.
///
/// A line that is not an ISBN-10 or ISBN-13 after the hyphens come out is skipped: the
/// cell also carries binding and price notes, and a price normalised into the `isbns`
/// array would be searched for later as though it were one.
fn isbns_of(values: &[String]) -> Vec<String> {
    let mut isbns: Vec<String> = Vec::new();
    for value in values {
        if let Some(isbn) = normalise_isbn(value)
            && !isbns.contains(&isbn)
        {
            isbns.push(isbn);
        }
    }
    isbns
}

/// One ISBN line, normalised — `None` when the line is not an ISBN.
fn normalise_isbn(line: &str) -> Option<String> {
    let token = line.split([' ', ':', ';']).next()?;
    let digits: String = token.chars().filter(|c| *c != '-').collect();
    let last = digits.len().checked_sub(1)?;
    let plausible = matches!(digits.len(), 10 | 13)
        && digits.char_indices().all(|(index, c)| {
            c.is_ascii_digit() || (index == last && c.eq_ignore_ascii_case(&'x'))
        });
    plausible.then(|| digits.to_ascii_uppercase())
}

/// The terms of a `Schlagwortkette` cell.
///
/// One cell is a *chain* of terms joined by `;`, and each cell is one chain — a record
/// has several. The union's non-sorting markers (`¬Der¬ Vorleser`) come out on the way:
/// they are a filing instruction, not part of the word.
fn subjects_of(values: &[String]) -> Vec<String> {
    let mut subjects: Vec<String> = Vec::new();
    for chain in values {
        for term in chain.split(';') {
            let term = term.replace('¬', "");
            let term = collapse(&term);
            if !term.is_empty() && !subjects.contains(&term) {
                subjects.push(term);
            }
        }
    }
    subjects
}

/// The label of the prose holdings statement.
const HOLDINGS_ROW: &str = "Bestand";

/// What the record states about the network's holdings in prose, or `None`.
///
/// The `Bestand` row, measured 2026-09-08 on the two of 23 sampled records that carry one
/// — both serials whose item table is present and empty:
///
/// ```text
/// Bestand in ZLB: 1993 - 2012(2013) Signatur: A 5 Brock 100
/// Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.) Mikrofilm Standort: BStB Signatur: A 80 ZC 181 Beil.:Mikro
/// ```
///
/// Carried **whole**. The `Standort:`/`Signatur:` words inside it look like a structure
/// and are not one — the first line has no `Standort:` at all, the second puts a carrier
/// (`Mikrofilm`) between the run and the location — so splitting it would mean guessing
/// where a shelfmark begins and then quoting the guess as the catalogue's own word. A
/// human reads the shelfmark out of the line; an agent at least has the line.
///
/// Without it the tool answered "no copies" for `voebb_SAK13708822`, whose page names a
/// location *and* a shelfmark two rows above the empty table.
fn holdings_statement_of(bibliographic: &Bibliographic) -> Option<String> {
    let lines = bibliographic.values(HOLDINGS_ROW);
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// The label prefix of every e-lending link voebb.de states.
///
/// Measured: `Link zur Onleihe` and `Link zu Overdrive`. The vendor behind it is not the
/// point and is never matched on — a third platform tomorrow would be read the same way.
const LENDING_LINK: &str = "Link zu";

/// The row that points at the lending platform, when the record has one.
///
/// This is what tells an electronic title without copies apart from a record page whose
/// item table stopped being found. The row's own text carries the loan state in prose
/// (`(Das Medium ist ausgeliehen / Vormerkung möglich)`), which is the only statement
/// about availability such a title has.
fn lending_link(bibliographic: &Bibliographic) -> Option<&BibRow> {
    bibliographic
        .rows
        .iter()
        .find(|row| row.label.starts_with(LENDING_LINK))
}

/// The label of the row that carries a bare access link.
const URL_ROW: &str = "URL";

/// Where an electronic title without a lending link says its access is, or `None`.
///
/// The fourth state of the item table (`detail_online_url.html`, `voebb_SAK34364366`,
/// measured 2026-09-08): `Medienart` `[E-Ressource]`, no `table#resptable-1`, no
/// `Link zu …` row at all, and one `URL` row pointing at a URN resolver. It is a regular
/// record and not a broken page — and it took a whole search down before it had a name.
///
/// **Two positive markers, never one.** The `URL` row on its own is not enough: a printed
/// book carries one for its table of contents, and reading a missing item table as "an
/// electronic title" because of it would turn a page this parser stopped understanding
/// into a confident answer with no copies. So `Medienart` has to say the record *is*
/// electronic as well — the same pairing [`is_missing_record`] uses, for the same reason.
///
/// That second marker is [`format_of`]'s **online flag**, not its `Format::Ebook`. The two
/// stopped being the same thing when `[E-Audio]` was measured (2026-09-08): it is an
/// electronic title with a material of its own, and asking for `Ebook` would have read
/// its page as broken.
///
/// Consulted only after [`lending_link`], because `detail_overdrive.html` carries **both**
/// rows and its loan state, such as it is, belongs to the lending link.
///
/// The label is matched on a row of `table.gi`, never on the page's text: the hint block
/// beside this very record reads "Bitte klicken Sie auf den Link zum Anbieter", which
/// begins with [`LENDING_LINK`] and is prose in a `p.info`, not a row.
fn online_access_url(bibliographic: &Bibliographic) -> Option<&str> {
    let medienart = bibliographic.first("Medienart").unwrap_or_default();
    if !format_of(medienart, false).1 {
        return None;
    }
    bibliographic
        .rows
        .iter()
        .find(|row| row.label.eq_ignore_ascii_case(URL_ROW))
        .and_then(|row| row.links.first())
        .map(|(url, _)| url.as_str())
}

/// The two loan states an e-lending link states about itself, folded.
///
/// Transcribed from the live site (`plan/voebb.md`); the three measured wordings are
///
/// ```text
/// Zugang zum Titel erhalten Sie hier. (Das Medium ist verfügbar / Ausleihe keine Vormerkung möglich)
/// Zugang zum Titel erhalten Sie hier. (Das Medium ist ausgeliehen / Vormerkung möglich)
/// Zugang zum Titel erhalten Sie hier.                                  ← Overdrive, no parenthesis
/// ```
///
/// Both entries are **positive** markers, and that is the point: the absence of one is
/// never read as the other. Overdrive states nothing, and "nothing" stays
/// [`Status::Unknown`] rather than becoming the optimistic half of a guess.
const LENDING_STATES: [(&str, Status); 2] = [
    ("ist verfugbar", Status::Available),
    ("ist ausgeliehen", Status::Unavailable),
];

/// The loan state an electronic title states in the text of its lending link.
///
/// [`Status::Unknown`] for a record with no such link, for one whose link carries no
/// parenthesis (Overdrive), and for a parenthesis worded in a way this parser has not
/// seen — a wording nobody measured must never become a guess, and the row's raw text
/// travels on in [`note_kinds::VOEBB_ONLINE_STATE_UNSTATED`] so that a new wording is
/// visible rather than silently swallowed. That is also the tag [`items_of`] chooses by:
/// [`Status::Unknown`] here means the note has to say that nothing was readable.
///
/// The platform's name is never matched on; see [`LENDING_LINK`].
fn lending_status(row: Option<&BibRow>) -> Status {
    let Some(row) = row else {
        return Status::Unknown;
    };
    let text = fold(&row.values.join(" "));
    LENDING_STATES
        .iter()
        .find(|(marker, _)| text.contains(marker))
        .map_or(Status::Unknown, |(_, status)| *status)
}

/// The links of the bibliographic tables, classified by the label of their row.
///
/// Every link but the record's own permalink ([`is_self_link`]) — a record never points
/// at itself.
fn urls_of(bibliographic: &Bibliographic, id: &RecordId) -> Vec<ResourceUrl> {
    let mut urls = Vec::new();
    for row in &bibliographic.rows {
        let kind = url_kind(&row.label);
        for (url, label) in &row.links {
            if is_self_link(url, id) {
                continue;
            }
            urls.push(ResourceUrl {
                url: url.clone(),
                kind,
                label: non_empty(label),
            });
        }
    }
    urls
}

/// Whether a link is the page's own permalink rather than a link out of the record.
///
/// Every detail page opens with an unlabelled `table.gi` row holding a copy button
/// (`class="permalink-unclicked"`, text `Kopierlink`) whose target is this very record —
/// all 23 pages of the 2026-09-08 sample carry one. Left in, it puts a link from every
/// record to itself into [`crate::model::Record::urls`], which is how `blibs show
/// voebb_SAK34364366` came to print `Online https://www.voebb.de/…?sp=SAK34364366 (link)`
/// under a record that *is* that page. The doc comment above [`urls_of`] had promised the
/// opposite since the module was written; nothing implemented it.
///
/// Recognised by **what it points at**, never by where it sits. The unlabelled row is not
/// reliably the permalink: three of the 23 pages carry a second unlabelled row with a real
/// link — a `d-nb.info` table of contents, a URN resolver, a ZLB viewer — so dropping
/// unlabelled rows would have cost those. A link that names this record's own number is
/// this record, whichever row it stands in, and that is exactly the promise being kept.
fn is_self_link(url: &str, id: &RecordId) -> bool {
    url.contains(id.local_id())
}

/// What a link under this label points at. A lending link is the resource itself; the
/// two `Inhalt…` rows are the catalogue's own scans; anything else is unclassified rather
/// than guessed.
fn url_kind(label: &str) -> UrlKind {
    if label.starts_with(LENDING_LINK) {
        UrlKind::Fulltext
    } else if label.starts_with("Inhaltsverzeichnis") {
        UrlKind::Toc
    } else {
        UrlKind::Other
    }
}

/// The copies, and a note whenever there are none.
///
/// The four states of `table#resptable-1`, all four fixture-backed:
///
/// | State | Meaning | Result |
/// | --- | --- | --- |
/// | table with rows | a normal record | the copies |
/// | table without rows (and without `<thead>`) | the record states no copies of its own | empty, [`note_kinds::VOEBB_MULTIVOLUME`] or [`note_kinds::VOEBB_NO_COPIES_LISTED`] — [`no_copies_note`] decides from `Medienart` |
/// | no table, but a `Link zu …` row | an electronic title; the loan state is in the link text, and [`lending_status`] reads it into the holding | empty, [`note_kinds::VOEBB_ONLINE_ONLY`] or [`note_kinds::VOEBB_ONLINE_STATE_UNSTATED`] |
/// | no table and no lending link, but `Medienart` says electronic and a `URL` row points somewhere | an electronic title whose access is a plain link, with no loan state anywhere | empty, [`note_kinds::VOEBB_ONLINE_URL_ONLY`] |
///
/// A missing table with none of those three explanations is a named error: at that point
/// the page has stopped being the page this parser knows, and an empty copy list would
/// read as "held nowhere". [`parse_detail`] turns that error into
/// [`note_kinds::VOEBB_PAGE_UNREADABLE`] and keeps the record — so the answer never goes
/// silent, and one unknown page never costs the other nine hits their result.
///
/// ## Why the electronic title has **two** tags
///
/// Since the lending link's parenthesis is read ([`lending_status`]), "an electronic
/// title" is no longer one situation but two: the Onleihe states `(Das Medium ist
/// ausgeliehen / …)` and the record is then judged like any record with a status, while
/// Overdrive states nothing at all and the record stays genuinely unjudged. `cli::run`
/// counts the second kind to word the `--available` footnote — "none of them is known to
/// be on loan" is true of Overdrive and false of the Onleihe, and it used to be said of
/// both.
///
/// A field on the note would have been the alternative, and it is the wrong one: `kind`
/// is the only member of [`Note`] an agent is allowed to switch on, `message` is prose it
/// may not parse, and `records[]` answers *which* record and not *what happened*. So the
/// distinction is a tag, and the two live next to each other in
/// [`crate::model::note_kinds`] where one limitation cannot grow two spellings.
fn items_of(
    document: &Html,
    bibliographic: &Bibliographic,
    notes: &mut Vec<Note>,
) -> Result<Vec<Item>, Error> {
    let selectors = selectors();
    let Some(table) = document.select(&selectors.item_table).next() else {
        let Some(row) = lending_link(bibliographic) else {
            if online_access_url(bibliographic).is_none() {
                return Err(missing_selector("table#resptable-1"));
            }
            // The URL proves the state and stays **out** of the sentence: it is different
            // for every record, and a message that differs never folds. Three such
            // records in one window produced three near-identical paragraphs that
            // differed only in a link (measured on `--author "von Schirach" --at AGB`).
            // The link itself is in `record.urls[]`, which `show` prints and `--json`
            // carries, and `records[]` says which records this note is about.
            notes.push(Note::new(
                note_kinds::VOEBB_ONLINE_URL_ONLY,
                "an electronic title has no copies on a shelf, and states no lending link \
                 either — the access voebb.de names is a plain URL on the record, and it \
                 says nothing about whether that access is free",
            ));
            return Ok(Vec::new());
        };
        let state = row.values.first().map_or("", String::as_str);
        let label = &row.label;
        // Worded for one record and for many alike, and enumerating none of them: an
        // identical note about the next record is folded into this one by
        // `Note::merged`, and the records are then the renderer's own line beneath the
        // message. "this ... it" in front of four records was both wrong and unreadable.
        notes.push(match lending_status(Some(row)) {
            // No parenthesis (Overdrive), or one worded in a way nobody measured. The
            // raw text travels with the note so a new wording is visible rather than
            // swallowed.
            Status::Unknown => Note::new(
                note_kinds::VOEBB_ONLINE_STATE_UNSTATED,
                format!(
                    "an electronic title has no copies on a shelf, and the {label} row \
                     states no loan status this tool can read — {state:?}"
                ),
            ),
            _ => Note::new(
                note_kinds::VOEBB_ONLINE_ONLY,
                format!(
                    "an electronic title has no copies on a shelf; the loan status shown \
                     is the one the {label} row states — {state:?}"
                ),
            ),
        });
        return Ok(Vec::new());
    };

    let rows: Vec<ElementRef<'_>> = table.select(&selectors.item_row).collect();
    if rows.is_empty() {
        notes.push(no_copies_note(
            bibliographic.first("Medienart").unwrap_or_default(),
            holdings_statement_of(bibliographic).as_deref(),
        ));
        return Ok(Vec::new());
    }

    let columns = Columns::from_header(table)?;
    Ok(rows
        .into_iter()
        .map(|row| item(row, &columns, notes))
        .collect())
}

/// The `Medienart` of a record whose parts carry the copies, folded as [`format_of`] folds.
const MULTIVOLUME: &str = "mehrteiliges werk";

/// Why an item table that is present and empty is empty, worded from what the page says
/// about **itself**.
///
/// The shape of the table says only *that* there are no copies. Until 2026-09-08 it was
/// also read as saying *why*, and every such record was told "the copies belong to the
/// volumes, which are records of their own; search for the volume". Measured over 23
/// records across the material types, nine have that shape and only two are multi-part
/// works: the other seven are a newspaper, a journal-like series, a magazine issue, a
/// score at work level and three film and audiobook series. Sending the reader of a
/// newspaper record off to "search for the volume" is not merely unhelpful, it is a
/// direction to nowhere.
///
/// So `Medienart` decides, and everything it does not name gets a sentence that claims
/// nothing: the table is empty, this is what the page calls the record, and an empty table
/// is never "held nowhere".
///
/// Where the record states its holdings in prose the note **points at** that statement and
/// does not repeat it. It used to quote the line, which was right while nothing else
/// showed it; now [`crate::model::Holding::holdings_statement`] is rendered above the
/// copies in both paths, and quoting it again put `Standort: BStB Signatur: A 80 ZC 181`
/// on the screen twice, two lines apart. A note that repeats what stands above it teaches
/// the reader to skip notes.
fn no_copies_note(medienart: &str, statement: Option<&str>) -> Note {
    if fold(medienart.trim_matches(['[', ']']).trim()) == MULTIVOLUME {
        return Note::new(
            note_kinds::VOEBB_MULTIVOLUME,
            "voebb.de lists no copies for a multi-part work — the copies belong to its \
             volumes, which are records of their own; search for the volume",
        );
    }
    let called = match non_empty(medienart) {
        Some(kind) => format!("the page calls the record {kind}"),
        None => "the page states no Medienart for it".to_string(),
    };
    let stated = match statement {
        Some(_) => {
            ". What this record states about its holdings it states in prose, above — \
             whole, because that form cannot be taken apart without guessing"
        }
        None => "",
    };
    Note::new(
        note_kinds::VOEBB_NO_COPIES_LISTED,
        format!(
            "voebb.de lists no copies on this record — its item table is present and \
             empty and {called}. That is never \"held nowhere\": where such a record has \
             parts, the copies stand on their records{stated}"
        ),
    )
}

/// One row of the item table. Every cell may legitimately be empty, so nothing here
/// fails: what is absent becomes `None` and the copy stays.
fn item(row: ElementRef<'_>, columns: &Columns, notes: &mut Vec<Note>) -> Item {
    let cells: Vec<ElementRef<'_>> = row.select(&selectors().cell).collect();
    let cell = |index: Option<usize>| index.and_then(|index| cells.get(index)).copied();

    let library = cell(Some(columns.library)).map(text_of).unwrap_or_default();
    let branch = branch_of(&library);
    let stated = cell(columns.location).map(text_of).unwrap_or_default();
    let (location, from_location) = match columns.call_number {
        // The shelfmark has a column of its own; the location cell is all location.
        Some(_) => (non_empty(&stated), None),
        // It does not, and then it sits behind the area in the location cell
        // (`Freihand Erwachsene - Roman Schlink`, `detail_on_loan`).
        None => split_location(&stated),
    };
    let order_option = cell(columns.order_option).and_then(|cell| non_empty(&text_of(cell)));

    Item {
        location,
        branch: branch.map(|branch| branch.kobvid.clone()),
        branch_name: non_empty(&library),
        call_number: cell(columns.call_number)
            .and_then(|cell| non_empty(&text_of(cell)))
            .or(from_location),
        // voebb.de states no volume in the item table; a multi-part work's volumes are
        // records of their own.
        volume: None,
        status: status_of(
            cell(Some(columns.availability)),
            order_option.as_deref(),
            notes,
        ),
        due_date: due_date_of(cell(Some(columns.availability)), notes),
        order_option,
    }
}

/// What voebb.de writes in front of a return date in the availability cell.
const DUE_MARKER: &str = "Fällig am:";

/// The return date of a copy that is out, as ISO-8601, or `None`.
///
/// `plan/voebb.md` §9 says a return date "steht nirgends in der Tabelle", checked against
/// 46 copies. That was true of those 46 and is not true of the catalogue: the availability
/// cell reads `Ausgeliehen -  Fällig am: 22.9.2026`, eight times on `voebb_SAK34906286`
/// and once on the plain novel `voebb_SAK34954522` (measured 2026-09-08). It is the answer
/// to "when is it back", which is a question `plan/usecases.md` asks and this parser was
/// throwing away.
///
/// It is read from the same cell as the status and decides nothing about it: the traffic
/// light comes from the marker class ([`status_of`]), so a date this function cannot read
/// costs a date and never a light.
///
/// A cell with no `Fällig am:` has no date and says nothing — the common case, and no
/// note. A cell that *has* the marker and a value that is not a date means the site
/// changed how it writes them, and that is
/// [`note_kinds::VOEBB_DUE_DATE_UNREADABLE`] rather than a guess — naming the form that
/// was expected, and not the text that was not it: a date differs per **copy**, so
/// quoting it would turn one changed format into one paragraph per borrowed copy.
/// `records[]` names where the new form can be read.
fn due_date_of(cell: Option<ElementRef<'_>>, notes: &mut Vec<Note>) -> Option<String> {
    let text = text_of(cell?);
    let (_, stated) = text.split_once(DUE_MARKER)?;
    let stated = stated.trim();
    if let Some(date) = parse_due_date(stated) {
        return Some(date);
    }
    notes.push(Note::new(
        note_kinds::VOEBB_DUE_DATE_UNREADABLE,
        "voebb.de stated a return date blibs cannot read — it is not a date in the \
         `d.m.yyyy` form the site has written them in so far, so the copy is reported \
         without one rather than with a guess",
    ));
    None
}

/// `22.9.2026` → `2026-09-22`, or `None` for anything that is not that.
///
/// Day **and** month arrive unpadded, so neither may be assumed two digits; the year must
/// be four, which is what keeps a truncated or reordered value from passing. The check is
/// a range check and not a calendar: `31.2.2026` survives it. Rejecting a date the site
/// printed because this parser disagrees about February would lose a real return date over
/// a data-entry error upstream, and the value is quoted, not computed with.
fn parse_due_date(stated: &str) -> Option<String> {
    let mut parts = stated.split('.');
    let day = decimal(parts.next()?)?;
    let month = decimal(parts.next()?)?;
    let year_field = parts.next()?.trim();
    if parts.next().is_some() || year_field.len() != 4 {
        return None;
    }
    let year = decimal(year_field)?;
    ((1..=31).contains(&day) && (1..=12).contains(&month))
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// One field of a date: ASCII digits and nothing else, so that `+9` and `٩` are not
/// numbers here even where `str::parse` would take them.
fn decimal(field: &str) -> Option<u32> {
    let field = field.trim();
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// Area and shelfmark out of a location cell that has no shelfmark column.
///
/// Split at the **first** ` - `, which is where voebb.de puts it; a cell without one is
/// all area, and the shelfmark is then simply not stated.
fn split_location(stated: &str) -> (Option<String>, Option<String>) {
    match stated.split_once(" - ") {
        Some((area, shelfmark)) if !area.trim().is_empty() && !shelfmark.trim().is_empty() => {
            (non_empty(area.trim()), non_empty(shelfmark.trim()))
        }
        _ => (non_empty(stated), None),
    }
}

/// The loan status of one copy.
///
/// The CSS class is the vocabulary, not the word: `available`/`notavailable` survives a
/// new status word, a word list does not (four words seen in ~120 measured rows). The
/// order column then splits the available ones, because **Präsenz is not availability**:
/// `nicht entleihbar (Freihand) - Präsenzbestand` stands next to `Verfügbar`, and reading
/// only the status column promises a loan that does not exist.
///
/// A cell with neither a known class nor a known word is [`Status::Unknown`] plus a note
/// — never a guess, and never quietly [`Status::Available`]. A cell whose class and word
/// *contradict* each other resolves to the pessimistic one, also with a note: an
/// unexplained conflict must not end as a promise that the copy is on the shelf.
fn status_of(
    cell: Option<ElementRef<'_>>,
    order_option: Option<&str>,
    notes: &mut Vec<Note>,
) -> Status {
    let Some(cell) = cell else {
        return Status::Unknown;
    };
    let text = text_of(cell);
    let class = cell
        .select(&selectors().status_span)
        .next()
        .and_then(|span| span.attr("class"))
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let available = class == "available" || text == "Verfügbar";
    let unavailable = class == "notavailable"
        || matches!(text.as_str(), "Ausgeliehen" | "Nicht im Regal" | "Verloren");

    if available && unavailable {
        notes.push(Note::new(
            note_kinds::AVAILABILITY_STATUS_CONFLICT,
            format!(
                "voebb.de marked a copy {class:?} and called it {text:?} at the same \
                 time; it is reported as unavailable rather than as a loan that may not \
                 exist"
            ),
        ));
        return Status::Unavailable;
    }
    if available {
        if is_reference(order_option) {
            return Status::Reference;
        }
        return Status::Available;
    }
    if unavailable {
        return Status::Unavailable;
    }
    notes.push(Note::new(
        note_kinds::AVAILABILITY_UNKNOWN_STATUS,
        format!(
            "voebb.de reported the status {text:?} for a copy, which blibs does not know; \
             it is shown as unknown rather than assumed to be available"
        ),
    ));
    Status::Unknown
}

/// Whether the order column says this copy cannot leave the building.
///
/// The three markers measured in `plan/voebb.md` §9: `Präsenzbestand`, `nicht entleihbar`
/// and `Nicht bestellbar`. They are read from `Bestellmöglichkeit` because the
/// availability column says `Verfügbar` for all of them.
fn is_reference(order_option: Option<&str>) -> bool {
    let Some(order) = order_option.map(fold) else {
        return false;
    };
    ["prasenz", "nicht entleihbar", "nicht bestellbar"]
        .iter()
        .any(|marker| order.contains(marker))
}

/// The VÖBB branch a *Bibliothek* cell names, or `None`.
///
/// The cell carries a display name and nothing else — no ISIL, no id — so the name is all
/// there is to match on. Three shapes of the name are tried, because the catalogue writes
/// `Bezirk: Haus` while the library list writes `Bezirk / Haus`:
///
/// 1. the cell text as it stands,
/// 2. with the first `: ` turned into ` / `,
/// 3. the part behind the first `: `.
///
/// Each shape is compared first against the branch names (full, short, and the part of
/// the full name behind the last separator) and only then against the list's explicit
/// `match` strings, so that a house named in both places is found under its own name.
/// Everything is folded (`plan/libraries.md`), an ambiguous hit counts as no hit, and a
/// miss is not an error: 40 of the 43 distinct names in the fixtures resolve, and the
/// three that do not are houses the list does not carry.
fn branch_of(library: &str) -> Option<&'static Branch> {
    let candidates = name_candidates(library);
    for names in [branch_names, branch_match_strings] {
        for candidate in &candidates {
            let mut hit = None;
            for branch in voebb_branches() {
                if names(branch).iter().any(|name| fold(name) == *candidate) {
                    if hit.is_some() {
                        // Two branches under one name: no answer is better than the
                        // wrong one, and the cell text survives in `branch_name`.
                        hit = None;
                        break;
                    }
                    hit = Some(branch);
                }
            }
            if hit.is_some() {
                return hit;
            }
        }
    }
    None
}

/// The three shapes of a *Bibliothek* cell, folded. See [`branch_of`].
fn name_candidates(library: &str) -> Vec<String> {
    let mut candidates = vec![fold(library)];
    if let Some((district, house)) = library.split_once(": ") {
        candidates.push(fold(&format!("{district} / {house}")));
        candidates.push(fold(house));
    }
    candidates
}

/// A branch's own names: full, short, and the part of the full name behind the last
/// separator — the list writes `Stadtbibliothek Mitte / Schiller-Bibliothek`, the
/// catalogue writes `Mitte: Schiller-Bibliothek`.
fn branch_names(branch: &'static Branch) -> Vec<&'static str> {
    let mut names = vec![branch.name.as_str(), branch.short_name.as_str()];
    if let Some(tail) = name_tail(&branch.name) {
        names.push(tail);
    }
    names
}

/// The strings the library list carries for identifying this branch in a location cell.
fn branch_match_strings(branch: &'static Branch) -> Vec<&'static str> {
    branch.match_strings.iter().map(String::as_str).collect()
}

/// The part of a branch name behind its last ` / ` or `, `.
fn name_tail(name: &'static str) -> Option<&'static str> {
    let slash = name.rfind(" / ").map(|at| at + 3);
    let comma = name.rfind(", ").map(|at| at + 2);
    slash.max(comma).map(|at| &name[at..])
}

/// Every branch of the public library network.
///
/// The one place in this module that names an ISIL, and it names the constant rather than
/// the code: voebb.de answers for `DE-609` and for nothing else, which is the reason this
/// engine exists at all.
fn voebb_branches() -> impl Iterator<Item = &'static Branch> {
    libraries::all()
        .iter()
        .filter(|library| library.isil == VOEBB_NETWORK)
        .flat_map(|library| library.branches.iter())
}

/// Which column of the item table holds what, read from its header row.
///
/// `Bibliothek` and `Verfügbarkeit` are required: without them a copy has neither an
/// owner nor a status. The other three are optional, because the table is **five** columns
/// wide for `detail_available` and **four** for `detail_on_loan`, and an unrecognised
/// heading is ignored rather than shifting the ones behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    /// The house. Required.
    library: usize,
    /// Where in the house the copy stands.
    location: Option<usize>,
    /// Its shelfmark — absent for a third of the measured records.
    call_number: Option<usize>,
    /// Loan condition and ordering option, e.g. `4 Wochen - Vormerkung möglich - 1x`.
    order_option: Option<usize>,
    /// Its traffic light. Required.
    availability: usize,
}

impl Columns {
    /// Map the header cells onto column indices, by their text.
    fn from_header(table: ElementRef<'_>) -> Result<Self, Error> {
        let mut library = None;
        let mut location = None;
        let mut call_number = None;
        let mut order_option = None;
        let mut availability = None;

        for (index, cell) in table.select(&selectors().header_cell).enumerate() {
            match fold(&text_of(cell)).as_str() {
                "bibliothek" => library = Some(index),
                "standort" => location = Some(index),
                "signatur" => call_number = Some(index),
                "bestellmoglichkeit" => order_option = Some(index),
                "verfugbarkeit" => availability = Some(index),
                _ => {}
            }
        }

        Ok(Self {
            library: library
                .ok_or_else(|| missing_selector("table#resptable-1 thead th:Bibliothek"))?,
            location,
            call_number,
            order_option,
            availability: availability
                .ok_or_else(|| missing_selector("table#resptable-1 thead th:Verfügbarkeit"))?,
        })
    }
}

/// The bibliographic tables of a detail page, as labelled rows in document order.
///
/// Several `table.gi` follow each other and the split between them carries no meaning, so
/// they are read as one list. Labels are not a closed vocabulary — an unknown one is kept
/// and simply never asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bibliographic {
    rows: Vec<BibRow>,
}

/// One `<th scope="row">Label</th><td>Wert</td>` row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BibRow {
    /// The label, whitespace collapsed. Empty for the permalink row.
    label: String,
    /// The value, one entry per `<br>`-separated line: a record has several persons,
    /// several subject chains and several titles in one cell.
    values: Vec<String>,
    /// The external links of the value cell, as `(url, text)`. In-page links (`href="#"`)
    /// are not links, they are the catalogue's own search anchors.
    links: Vec<(String, String)>,
}

impl Bibliographic {
    /// Read every `table.gi` row of the page.
    ///
    /// A page with no `table.gi` at all, or with no `Titel` row, is not a detail page —
    /// that is [`crate::error::UnexpectedError::MissingSelector`] naming what stopped
    /// matching, never a record with an empty title.
    fn read(document: &Html) -> Result<Self, Error> {
        let selectors = selectors();
        if document.select(&selectors.bib_table).next().is_none() {
            return Err(missing_selector("table.gi"));
        }
        let rows: Vec<BibRow> = document
            .select(&selectors.bib_row)
            .filter_map(|row| BibRow::read(row, selectors))
            .collect();
        let bibliographic = Self { rows };
        if bibliographic.first("Titel").is_none() {
            return Err(missing_selector("table.gi th[scope=row]:Titel"));
        }
        Ok(bibliographic)
    }

    /// The lines of the first row with this label; empty when the page has no such row.
    fn values(&self, label: &str) -> &[String] {
        self.rows
            .iter()
            .find(|row| row.label.eq_ignore_ascii_case(label))
            .map_or(&[], |row| row.values.as_slice())
    }

    /// The first line of the first row with this label.
    fn first(&self, label: &str) -> Option<&str> {
        self.values(label).first().map(String::as_str)
    }
}

impl BibRow {
    /// Read one row, or `None` when it is not a label/value pair.
    fn read(row: ElementRef<'_>, selectors: &Selectors) -> Option<Self> {
        let label = text_of(row.select(&selectors.row_header).next()?);
        let cell = row.select(&selectors.cell).next()?;
        Some(Self {
            label,
            values: lines_of(cell),
            links: cell
                .select(&selectors.link)
                .filter_map(|link| {
                    let href = link.attr("href")?;
                    href.starts_with("http")
                        .then(|| (href.to_string(), text_of(link)))
                })
                .collect(),
        })
    }
}

/// The `<br>`-separated lines of a value cell, whitespace collapsed, empties dropped.
///
/// `<br>` is the separator voebb.de uses for repeated values — two persons, two subject
/// chains — and taking the cell's text as a whole would run them into one string.
fn lines_of(cell: ElementRef<'_>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for node in cell.descendants() {
        if let Some(text) = node.value().as_text() {
            current.push_str(text);
        } else if node
            .value()
            .as_element()
            .is_some_and(|element| element.name() == "br")
        {
            push_line(&mut lines, &mut current);
        }
    }
    push_line(&mut lines, &mut current);
    lines
}

/// Flush one collected line into the list, unless it is blank.
fn push_line(lines: &mut Vec<String>, current: &mut String) {
    let line = collapse(current);
    current.clear();
    if !line.is_empty() {
        lines.push(line);
    }
}

/// The compiled selectors. Every one is a literal in this file, so a parse failure would
/// be a typo in this source and nothing a user or a service can cause.
struct Selectors {
    bib_table: Selector,
    bib_row: Selector,
    row_header: Selector,
    cell: Selector,
    link: Selector,
    item_table: Selector,
    header_cell: Selector,
    item_row: Selector,
    status_span: Selector,
    /// The container every record page has and the entry page has not.
    record_body: Selector,
    /// The container the entry page has and no record page has.
    entry_body: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        bib_table: compile("table.gi"),
        bib_row: compile("table.gi tr"),
        row_header: compile("th"),
        cell: compile("td"),
        link: compile("a[href]"),
        item_table: compile("table#resptable-1"),
        header_cell: compile("thead th"),
        item_row: compile("tbody tr"),
        status_span: compile("span[class]"),
        record_body: compile("div#R03"),
        entry_body: compile("div#R04"),
    })
}

/// All text below an element, whitespace collapsed. The page indents its cells over
/// several lines and pads the order column with runs of spaces.
fn text_of(element: ElementRef<'_>) -> String {
    collapse(&element.text().collect::<String>())
}

/// `None` for an empty string, so that "the cell was blank" and "there is no such column"
/// stay the same thing in the output.
fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// A missing-selector error naming what stopped matching in this document.
fn missing_selector(selector: &str) -> Error {
    super::missing_selector(selector, DOCUMENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::UnexpectedError;

    const AVAILABLE: &str = include_str!("../../../../tests/fixtures/voebb/detail_available.html");
    const ON_LOAN: &str = include_str!("../../../../tests/fixtures/voebb/detail_on_loan.html");
    const REFERENCE: &str = include_str!("../../../../tests/fixtures/voebb/detail_reference.html");
    const ONLINE: &str = include_str!("../../../../tests/fixtures/voebb/detail_online.html");
    const ONLINE_AVAILABLE: &str =
        include_str!("../../../../tests/fixtures/voebb/detail_online_available.html");
    const MULTIVOLUME: &str =
        include_str!("../../../../tests/fixtures/voebb/detail_multivolume.html");
    const UNKNOWN: &str = include_str!("../../../../tests/fixtures/voebb/detail_unknown.html");
    const OVERDRIVE: &str = include_str!("../../../../tests/fixtures/voebb/detail_overdrive.html");
    const ONLINE_URL: &str =
        include_str!("../../../../tests/fixtures/voebb/detail_online_url.html");
    const NEWSPAPER: &str = include_str!("../../../../tests/fixtures/voebb/detail_newspaper.html");
    const SERIES: &str = include_str!("../../../../tests/fixtures/voebb/detail_series.html");
    const DUE_DATES: &str = include_str!("../../../../tests/fixtures/voebb/detail_due_dates.html");

    /// Every record page fixture, with the number it was fetched under. The rules that
    /// have to hold for *all* of them — no record links to itself — are checked over this.
    const RECORD_PAGES: [(&str, &str); 11] = [
        (AVAILABLE, "SAK13776205"),
        (ON_LOAN, "SAK00177143"),
        (REFERENCE, "SAK15912360"),
        (ONLINE, "SAK16112988"),
        (ONLINE_AVAILABLE, "SAK16112988"),
        (ONLINE_URL, "SAK34364366"),
        (OVERDRIVE, "SAK34672596"),
        (MULTIVOLUME, "SAK13927817"),
        (NEWSPAPER, "SAK13708822"),
        (SERIES, "SAK34799780"),
        (DUE_DATES, "SAK34906286"),
    ];

    /// The one holding every voebb.de record has.
    fn holding(page: &DetailPage) -> &Holding {
        match page.record.holdings.as_slice() {
            [holding] => holding,
            other => panic!(
                "a voebb record has exactly one holding, found {}",
                other.len()
            ),
        }
    }

    /// The kinds of the notes a page produced, in order.
    fn kinds(page: &DetailPage) -> Vec<&str> {
        page.notes.iter().map(|note| note.kind).collect()
    }

    /// Parse a fixture under the id it was fetched with.
    fn parsed(html: &str, local: &str) -> DetailPage {
        match parse_detail(html, &RecordId::voebb(local)) {
            Ok(page) => page,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    /// The copies of the one holding every voebb.de record has.
    fn items(page: &DetailPage) -> &[Item] {
        match page.record.holdings.as_slice() {
            [holding] => &holding.items,
            other => panic!(
                "a voebb record has exactly one holding, found {}",
                other.len()
            ),
        }
    }

    fn statuses(page: &DetailPage) -> Vec<Status> {
        items(page).iter().map(|item| item.status).collect()
    }

    /// A record number the catalogue does not hold comes back as the search entry page.
    /// Recognising it is what keeps `show` at exit 1 instead of a selector error.
    #[test]
    fn an_unknown_record_number_answers_with_the_entry_page() {
        assert!(is_missing_record(UNKNOWN));
    }

    /// Every real record page is a record page, and none of them is mistaken for a
    /// number the catalogue does not hold — including the two whose item table is empty.
    #[test]
    fn a_real_record_page_is_never_read_as_a_missing_record() {
        for html in [AVAILABLE, ON_LOAN, REFERENCE, ONLINE, MULTIVOLUME] {
            assert!(!is_missing_record(html));
        }
    }

    /// A record page whose bibliographic tables vanished is a **changed site**, not a
    /// missing record: it keeps the record page's own container, so it fails loudly.
    #[test]
    fn a_record_page_without_tables_is_an_error_not_a_missing_record() {
        let broken = AVAILABLE.replace("class=\"gi\"", "class=\"gone\"");
        assert!(!is_missing_record(&broken));
        let error = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect_err("a record page without any table.gi must fail");
        assert!(error.to_string().contains("table.gi"), "{error}");
    }

    /// `detail_available` (`SAK13776205`): the normal case, five columns, one copy.
    #[test]
    fn reads_the_bibliographic_fields_of_a_normal_record() {
        let page = parsed(AVAILABLE, "SAK13776205");
        let record = &page.record;
        assert_eq!(record.id.as_str(), "voebb_SAK13776205");
        assert_eq!(record.title, "Bernhard Schlink, Der Vorleser");
        assert_eq!(record.subtitle, None);
        assert_eq!(record.year, Some(2004));
        assert_eq!(record.place.as_deref(), Some("Berlin"));
        assert_eq!(record.publisher.as_deref(), Some("Cornelsen"));
        assert_eq!(record.edition.as_deref(), Some("1. Aufl., 1. Dr."));
        assert_eq!(
            record.extent.as_deref(),
            Some("48 Seiten : Ill., graph. Darst.")
        );
        assert_eq!(record.languages, vec!["ger"]);
        assert_eq!(record.isbns, vec!["3464616347"]);
        assert_eq!(record.format, Format::Book);
        assert!(!record.online);
        assert_eq!(
            record
                .authors
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Mittelberg, Ekkehart"]
        );
        assert!(page.notes.is_empty(), "notes: {:?}", page.notes);
    }

    /// The non-sorting markers of the union (`¬Der¬`) are a filing instruction, not part
    /// of the term, and one cell is a chain of several terms.
    #[test]
    fn subject_chains_split_and_lose_their_non_sorting_markers() {
        let page = parsed(AVAILABLE, "SAK13776205");
        assert_eq!(
            page.record.subjects,
            vec![
                "Schlink, Bernhard <1944->. Der Vorleser",
                "Deutschunterricht",
                "Unterrichtseinheit"
            ]
        );
    }

    /// The one holding of a voebb.de record is the network; the branch lives in the copy.
    #[test]
    fn the_record_carries_exactly_one_holding_for_the_network() {
        let page = parsed(AVAILABLE, "SAK13776205");
        let holding = &page.record.holdings[0];
        assert_eq!(holding.isil.as_ref().map(Isil::as_str), Some("DE-609"));
        assert_eq!(holding.local_id.as_deref(), Some("SAK13776205"));
        assert!(!holding.mine);
        assert_eq!(holding.summary, Status::Available);
        assert!(!holding.library.is_empty());
    }

    /// Five columns: the shelfmark has one of its own and the location cell is all
    /// location.
    #[test]
    fn maps_five_columns_by_their_header_text() {
        let page = parsed(AVAILABLE, "SAK13776205");
        let [item] = items(&page) else {
            panic!("detail_available has exactly one copy");
        };
        assert_eq!(item.branch_name.as_deref(), Some("ZLB: Außenmagazin"));
        // Changed in round 2 (§2.6): this used to assert `SIG00036`. The ZLB runs **two**
        // outlying stacks — the facet tree lists `ZLB: Außenmagazin Amerika-Gedenk-
        // bibliothek` and `… Berliner Stadtbibliothek` — and the item table writes only
        // `ZLB: Außenmagazin`, so crediting the AGB was a coin flip an agent read as fact.
        // See `an_ambiguous_house_name_resolves_to_no_branch_at_all` for the mechanism.
        assert_eq!(item.branch, None);
        assert_eq!(item.location.as_deref(), Some("Magazin (OG1)"));
        assert_eq!(item.call_number.as_deref(), Some("004/000 042 193"));
        assert_eq!(
            item.order_option.as_deref(),
            Some("Außenmagazin - Außenmagazin, bestellbar")
        );
        assert_eq!(item.status, Status::Available);
        assert_eq!(item.volume, None);
    }

    /// `detail_on_loan` (`SAK00177143`): four columns — **no** shelfmark column, so the
    /// shelfmark sits behind the area in the location cell.
    #[test]
    fn maps_four_columns_and_splits_the_shelfmark_out_of_the_location() {
        let page = parsed(ON_LOAN, "SAK00177143");
        let first = &items(&page)[0];
        assert_eq!(
            first.branch_name.as_deref(),
            Some("Charlottenburg-Wilmersdorf: Ingeborg-Bachmann-Bibliothek")
        );
        assert_eq!(first.location.as_deref(), Some("Freihand Erwachsene"));
        assert_eq!(first.call_number.as_deref(), Some("Roman Schlin"));
        assert_eq!(
            first.order_option.as_deref(),
            Some("4 Wochen - Bestellwunsch möglich - 1x")
        );
    }

    /// The measured status vocabulary of `detail_on_loan`: 45 copies, five of them out.
    #[test]
    fn reads_every_copy_and_the_statuses_that_are_not_available() {
        let page = parsed(ON_LOAN, "SAK00177143");
        let statuses = statuses(&page);
        assert_eq!(statuses.len(), 45);
        let unavailable = statuses
            .iter()
            .filter(|status| **status == Status::Unavailable)
            .count();
        assert_eq!(unavailable, 7, "5 Ausgeliehen + Verloren + Nicht im Regal");
        assert!(statuses.iter().all(|status| *status != Status::Unknown));
        assert!(items(&page).iter().all(|item| item.order_option.is_some()));
        assert_eq!(page.record.title, "Der Vorleser");
        assert_eq!(page.record.subtitle.as_deref(), Some("Roman"));
        assert_eq!(page.record.year, Some(1997));
        assert_eq!(page.record.isbns, vec!["3257229534"]);
        assert_eq!(
            page.record
                .authors
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Schlink, Bernhard"]
        );
    }

    /// The branch of a copy comes from the name of the house alone — there is no id and
    /// no ISIL in the table. 40 of the 43 distinct names in the fixtures resolve; the
    /// three that do not are houses the library list does not carry, and they keep their
    /// text instead of borrowing someone else's id.
    #[test]
    fn matches_branches_by_name_and_leaves_the_unknown_ones_named_but_unresolved() {
        let page = parsed(ON_LOAN, "SAK00177143");
        let resolved = items(&page)
            .iter()
            .filter(|item| item.branch.is_some())
            .count();
        assert!(
            resolved >= 40,
            "only {resolved} of 45 copies found their branch"
        );
        assert!(items(&page).iter().all(|item| item.branch_name.is_some()));

        let agb = items(&page)
            .iter()
            .find(|item| item.branch_name.as_deref() == Some("ZLB: Amerika-Gedenkbibliothek (AGB)"))
            .expect("the AGB holds a copy of Der Vorleser");
        assert_eq!(agb.branch.as_deref(), Some("SIG00036"));

        let unknown = items(&page)
            .iter()
            .find(|item| {
                item.branch_name.as_deref()
                    == Some("Humboldtschule (nicht öffentlich) - ist zurzeit geschlossen")
            })
            .expect("the fixture holds a house the list does not carry");
        assert_eq!(unknown.branch, None);
    }

    /// `detail_reference` (`SAK15912360`): Präsenz is not availability. Both copies say
    /// `Verfügbar`; only the one that can be ordered is [`Status::Available`].
    #[test]
    fn presence_is_read_from_the_order_column_not_from_the_status_column() {
        let page = parsed(REFERENCE, "SAK15912360");
        let copies = items(&page);
        assert_eq!(copies.len(), 2);

        let reading_room = copies
            .iter()
            .find(|item| item.location.as_deref() == Some("Lesesaal"))
            .expect("the Berlin-Sammlungen copy is in the reading room");
        assert_eq!(reading_room.status, Status::Reference);
        assert!(
            reading_room
                .order_option
                .as_deref()
                .is_some_and(|order| order.contains("Präsenzbestand"))
        );

        let stack = copies
            .iter()
            .find(|item| item.location.as_deref() == Some("Magazin BStB (EG1)"))
            .expect("the BStB copy comes from the stacks");
        assert_eq!(stack.status, Status::Available);
        assert_eq!(page.record.holdings[0].summary, Status::Available);
        assert_eq!(page.record.year, Some(2015));
        assert_eq!(page.record.place.as_deref(), Some("Heilbronn, Neckar"));
        assert_eq!(
            page.record.publisher.as_deref(),
            Some("Kleist-Archiv Sembdner")
        );
        assert_eq!(page.record.isbns, vec!["9783940494702"]);
    }

    /// The role is the bracketed suffix, kept in the catalogue's own words.
    #[test]
    fn a_bracketed_suffix_becomes_the_role() {
        let page = parsed(REFERENCE, "SAK15912360");
        let roles: Vec<(&str, Option<&str>)> = page
            .record
            .authors
            .iter()
            .map(|author| (author.name.as_str(), author.role.as_deref()))
            .collect();
        assert_eq!(
            roles,
            vec![
                ("Bienert, Michael", Some("Vorredner/in")),
                ("Neander, Joachim Friedrich", None)
            ]
        );
    }

    /// `detail_online` (`SAK16112988`): no item table at all. The loan state exists only
    /// in the link text, so it becomes a note — never an empty copy list on its own.
    #[test]
    fn an_onleihe_title_has_no_copies_but_says_so() {
        let page = parsed(ONLINE, "SAK16112988");
        assert!(items(&page).is_empty());
        assert!(page.record.online);
        assert_eq!(page.record.format, Format::Ebook);
        // Changed in round 2 (§3.6): this used to assert `Unknown`. The link text says
        // `(Das Medium ist ausgeliehen / …)` and the tool printed that sentence in a note
        // while reporting "not known to be on loan" one line above it.
        assert_eq!(page.record.holdings[0].summary, Status::Unavailable);

        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_ONLINE_ONLY)
            .expect("a missing item table must be stated");
        assert!(
            note.message.contains("ausgeliehen"),
            "the loan state from the link text is missing: {}",
            note.message
        );

        let onleihe = page
            .record
            .urls
            .iter()
            .find(|url| url.kind == UrlKind::Fulltext)
            .expect("the Onleihe link is the resource itself");
        assert!(onleihe.url.contains("onleihe.de"));
    }

    /// `detail_online_available` (derived from `detail_online`, §3.6): the other half of
    /// the same sentence. `(Das Medium ist verfügbar / …)` is a statement that the title
    /// can be borrowed right now, and it is the marker that `--available` keeps a record
    /// on.
    #[test]
    fn an_onleihe_title_that_is_in_is_read_as_available() {
        let page = parsed(ONLINE_AVAILABLE, "SAK16112988");
        assert!(items(&page).is_empty());
        assert_eq!(page.record.holdings[0].summary, Status::Available);
        // The empty copy list still says why it is empty: the status came from prose.
        assert!(
            page.notes
                .iter()
                .any(|note| note.kind == note_kinds::VOEBB_ONLINE_ONLY),
            "{:?}",
            page.notes
        );
    }

    /// A parenthesis worded in a way nobody measured stays [`Status::Unknown`], with the
    /// raw text in the note. The two markers are matched **positively**: a wording this
    /// parser has not seen is a gap to notice, never an availability to guess.
    #[test]
    fn an_unrecognised_loan_wording_stays_unknown_and_keeps_its_text() {
        let broken = ONLINE.replace(
            "(Das Medium ist ausgeliehen / Vormerkung möglich)",
            "(Das Medium wird gerade eingearbeitet)",
        );
        let page = parse_detail(&broken, &RecordId::voebb("SAK16112988"))
            .expect("an unknown wording is not a parse failure");
        assert_eq!(page.record.holdings[0].summary, Status::Unknown);
        // Changed in round 2 (§3.6): an unreadable parenthesis is the *unstated* tag now,
        // because that is the one `cli::run` may say "not known to be on loan" about.
        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_ONLINE_STATE_UNSTATED)
            .expect("an empty copy list is never silent");
        assert!(
            note.message.contains("eingearbeitet"),
            "the wording nobody knows has to be readable: {}",
            note.message
        );
    }

    /// The three measured wordings, against the function that reads them. Overdrive's
    /// link has no parenthesis at all, and that is an answer of "nothing stated" rather
    /// than a gap in this table.
    #[test]
    fn only_the_two_measured_markers_state_a_loan_status() {
        let row = |text: &str| BibRow {
            label: "Link zur Onleihe".to_string(),
            values: vec![text.to_string()],
            links: Vec::new(),
        };
        assert_eq!(
            lending_status(Some(&row(
                "Zugang zum Titel erhalten Sie hier. (Das Medium ist verfügbar / \
                 Ausleihe keine Vormerkung möglich)"
            ))),
            Status::Available
        );
        assert_eq!(
            lending_status(Some(&row(
                "Zugang zum Titel erhalten Sie hier. (Das Medium ist ausgeliehen / \
                 Vormerkung möglich)"
            ))),
            Status::Unavailable
        );
        assert_eq!(
            lending_status(Some(&row("Zugang zum Titel erhalten Sie hier."))),
            Status::Unknown
        );
        assert_eq!(lending_status(None), Status::Unknown);
    }

    /// A house name the item table writes for **two** of the list's branches resolves to
    /// none of them (round 2, §2.6). The ZLB runs two outlying stacks and the cell says
    /// only `ZLB: Außenmagazin`; the table carries no feature that separates them, so the
    /// copy keeps its text and borrows no id. This is the same rule that already covers a
    /// name the list does not carry at all — an ambiguous hit counts as no hit.
    #[test]
    fn an_ambiguous_house_name_resolves_to_no_branch_at_all() {
        assert_eq!(branch_of("ZLB: Außenmagazin"), None);
        // The two houses it could be are each still reachable under their own name, so
        // the ambiguity costs nothing but the guess it removes.
        assert_eq!(
            branch_of("ZLB: Amerika-Gedenkbibliothek (AGB)").map(|branch| branch.kobvid.as_str()),
            Some("SIG00036")
        );
        assert_eq!(
            branch_of("ZLB: Berliner Stadtbibliothek (BStB)").map(|branch| branch.kobvid.as_str()),
            Some("BIB000000072")
        );
    }

    /// `detail_multivolume` (`SAK13927817`): the table is there and empty, without a
    /// header row. The volumes are records of their own — this is not "held nowhere".
    #[test]
    fn a_multi_part_work_has_an_empty_item_table_and_a_note() {
        let page = parsed(MULTIVOLUME, "SAK13927817");
        assert!(items(&page).is_empty());
        assert_eq!(page.record.holdings[0].summary, Status::Unknown);
        assert!(
            page.notes
                .iter()
                .any(|note| note.kind == note_kinds::VOEBB_MULTIVOLUME),
            "an empty item table must say why: {:?}",
            page.notes
        );
        assert_eq!(page.record.title, "Brockhaus Enzyklopädie");
        assert_eq!(page.record.subtitle.as_deref(), Some("in 30 Bänden"));
        // `Leipzig : Brockhaus` — a publisher and no year at all.
        assert_eq!(page.record.year, None);
        assert_eq!(page.record.publisher.as_deref(), Some("Brockhaus"));
        // `[Mehrteiliges Werk]` states a level, not a material, and is never guessed at.
        assert_eq!(page.record.format, Format::Unknown);
        assert_eq!(page.record.isbns, vec!["3765341479"]);
    }

    /// The regression the multi-part note used to be. Five of the seven material types
    /// whose item table is present and empty are not multi-part works, and every one of
    /// them was told "search for the volume" — a newspaper included.
    #[test]
    fn an_empty_item_table_only_claims_a_multi_part_work_when_the_page_says_so() {
        for (html, local) in [(NEWSPAPER, "SAK13708822"), (SERIES, "SAK34799780")] {
            let page = parsed(html, local);
            assert!(items(&page).is_empty(), "{local}");
            assert_eq!(
                kinds(&page),
                [note_kinds::VOEBB_NO_COPIES_LISTED],
                "{local}"
            );
            let message = &page.notes[0].message;
            assert!(
                !message.contains("volume"),
                "a record that is not a multi-part work must not be sent after one: \
                 {message}"
            );
        }
        // And the one that is one keeps the sentence that is true of it.
        let multivolume = parsed(MULTIVOLUME, "SAK13927817");
        assert_eq!(kinds(&multivolume), [note_kinds::VOEBB_MULTIVOLUME]);
    }

    /// `voebb_SAK13708822` names a location *and* a shelfmark two rows above its empty
    /// item table, and the tool answered "no copies" for it. The line is carried whole:
    /// `Standort:`/`Signatur:` inside it look like structure and are free text.
    #[test]
    fn a_serial_states_its_holdings_in_prose_and_the_line_is_carried_whole() {
        let page = parsed(NEWSPAPER, "SAK13708822");
        assert_eq!(
            holding(&page).holdings_statement.as_deref(),
            Some(
                "Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.) Mikrofilm \
                 Standort: BStB Signatur: A 80 ZC 181 Beil.:Mikro"
            )
        );
        // The note points at the statement and does **not** repeat it. It quoted the
        // line until the renderer printed `holdings_statement` itself; then the shelfmark
        // stood on the screen twice, two lines apart, and a note that repeats what is
        // above it teaches the reader to skip notes.
        let message = &page.notes[0].message;
        assert!(
            !message.contains("A 80 ZC 181"),
            "the note must not repeat the statement the renderer prints: {message:?}"
        );
        assert!(
            message.contains("in prose, above"),
            "the note must still point at it: {message:?}"
        );
        // A newspaper is a serial, not an unknown material.
        assert_eq!(page.record.format, Format::Journal);
    }

    /// Only voebb.de states holdings in prose, and only some records do. Everything else
    /// keeps `None` — an empty string here would render as a blank statement.
    #[test]
    fn a_record_without_a_bestand_row_states_no_prose_holdings() {
        for (html, local) in RECORD_PAGES {
            if local == "SAK13708822" {
                continue;
            }
            let page = parsed(html, local);
            assert_eq!(
                holding(&page).holdings_statement,
                None,
                "{local} has no Bestand row"
            );
        }
    }

    /// `plan/voebb.md` §9 says a return date "steht nirgends", measured over 46 copies.
    /// It is in the availability cell behind the status word, on both a console game and
    /// a plain novel (2026-09-08). ISO-8601 out, because the day *and* the month arrive
    /// unpadded and an agent must not have to parse `22.9.2026`.
    #[test]
    fn a_copy_on_loan_carries_the_return_date_the_cell_states() {
        let page = parsed(DUE_DATES, "SAK34906286");
        let dates: Vec<&str> = items(&page)
            .iter()
            .filter_map(|item| item.due_date.as_deref())
            .collect();
        assert_eq!(
            dates,
            [
                "2026-09-22",
                "2026-10-05",
                "2026-10-05",
                "2026-09-15",
                "2026-09-15",
                "2026-10-01",
                "2026-09-12",
                "2026-10-05",
            ]
        );
        // A date only ever stands beside a copy that is out. The converse does not hold
        // and must not be asserted: `Verloren` and `Nicht im Regal` are out with no date
        // to give, which is exactly why the date is a field of its own and not a status.
        for item in items(&page) {
            assert!(
                item.due_date.is_none() || item.status == Status::Unavailable,
                "{item:?}"
            );
        }
    }

    /// The date is an extra, never a second route to a status: the light comes from the
    /// marker class, so `Ausgeliehen -  Fällig am: …` is still simply unavailable and the
    /// unmeasured wording `verfügbar oder "Heute zurückverbucht"` is still available.
    #[test]
    fn a_return_date_beside_the_status_word_changes_no_traffic_light() {
        let page = parsed(DUE_DATES, "SAK34906286");
        let lit = statuses(&page);
        // 13 in — the twelve `Verfügbar` and the unmeasured wording
        // `verfügbar oder "Heute zurückverbucht"`, which rides on the class.
        assert_eq!(lit.iter().filter(|s| **s == Status::Available).count(), 13);
        // 10 out — the eight with a date, plus `Verloren` and `Nicht im Regal`.
        assert_eq!(
            lit.iter().filter(|s| **s == Status::Unavailable).count(),
            10
        );
        assert!(
            !lit.contains(&Status::Unknown),
            "no copy fell through: {lit:?}"
        );
        // Nothing about the dates produced a note.
        assert!(page.notes.is_empty(), "{:?}", page.notes);
    }

    /// A stated date this parser cannot read is neither guessed nor swallowed: the copy
    /// keeps its light, loses the date, and a note says the form stopped being readable.
    ///
    /// The **text** stays out of that note. A date differs per copy, and a message that
    /// differs never folds: two copies with two unreadable dates would otherwise be two
    /// paragraphs about one changed format.
    #[test]
    fn an_unreadable_return_date_is_a_note_and_never_a_guess() {
        let changed = DUE_DATES
            .replace("Fällig am: 22.9.2026", "Fällig am: nächste Woche")
            .replace("Fällig am: 12.9.2026", "Fällig am: bald");
        let page = parsed(&changed, "SAK34906286");
        assert_eq!(
            items(&page)
                .iter()
                .filter(|item| item.due_date.is_some())
                .count(),
            6
        );
        // Two unreadable copies, and the note has to be **one** sentence about both.
        assert_eq!(
            kinds(&page),
            [
                note_kinds::VOEBB_DUE_DATE_UNREADABLE,
                note_kinds::VOEBB_DUE_DATE_UNREADABLE
            ]
        );
        assert_eq!(page.notes[0].message, page.notes[1].message);
        assert_eq!(
            Note::merged(page.notes.clone()).len(),
            1,
            "{:?}",
            page.notes
        );
        for note in &page.notes {
            assert!(
                !note.message.contains("nächste Woche") && !note.message.contains("bald"),
                "the unreadable text belongs in records[], not in the message: {}",
                note.message
            );
        }
        // The light is untouched.
        assert_eq!(
            statuses(&page)
                .iter()
                .filter(|status| **status == Status::Unavailable)
                .count(),
            10
        );
    }

    /// The forms the site writes, and the ones it does not. Day and month are unpadded,
    /// the year is always four digits, and everything else stays `None`.
    #[test]
    fn a_return_date_is_read_unpadded_and_only_in_the_form_the_site_writes() {
        assert_eq!(parse_due_date("22.9.2026").as_deref(), Some("2026-09-22"));
        assert_eq!(parse_due_date("5.10.2026").as_deref(), Some("2026-10-05"));
        assert_eq!(parse_due_date("05.10.2026").as_deref(), Some("2026-10-05"));
        for wrong in [
            "",
            "22.9.26",
            "2026-09-22",
            "22/9/2026",
            "22.9.2026.1",
            "22..2026",
            "0.9.2026",
            "22.13.2026",
            "+2.9.2026",
            "nächste Woche",
        ] {
            assert_eq!(parse_due_date(wrong), None, "{wrong:?}");
        }
    }

    /// The doc comment above `urls_of` promised this since the module was written and
    /// nothing implemented it: every page's first `table.gi` row is a copy button
    /// pointing at the record itself, and it ended up in `urls[]` of every voebb record.
    #[test]
    fn no_record_links_to_itself() {
        for (html, local) in RECORD_PAGES {
            let page = parsed(html, local);
            for url in &page.record.urls {
                assert!(
                    !url.url.contains(local),
                    "{local} links to itself: {:?}",
                    url.url
                );
            }
        }
    }

    /// Two electronic titles whose access URLs differ are **one** note about two records.
    ///
    /// The note used to name the URL, which made its message unique per record, and
    /// [`Note::merged`] folds on `kind` *and* `message`: three such records in one window
    /// printed three near-identical paragraphs differing only in a link (measured on
    /// `--author "von Schirach" --at AGB`, round 3). The sample holds three of them, so
    /// they do meet in one window.
    #[test]
    fn two_access_urls_are_one_note_about_two_records() {
        let other = ONLINE_URL.replace(
            "nbn-resolving.de/urn:nbn:de:kobv:109-1-15402775",
            "digital.zlb.de/viewer/metadata/1398120723/",
        );
        assert_ne!(other, ONLINE_URL, "the replacement has to bite");

        let first = parsed(ONLINE_URL, "SAK34364366");
        let second = parsed(&other, "SAK35521371");
        assert_eq!(kinds(&first), [note_kinds::VOEBB_ONLINE_URL_ONLY]);
        assert_eq!(kinds(&second), [note_kinds::VOEBB_ONLINE_URL_ONLY]);

        let mut notes = first.notes.clone();
        notes.extend(second.notes.clone());
        let merged = Note::merged(notes);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(
            merged[0].records,
            vec![
                RecordId::voebb("SAK34364366"),
                RecordId::voebb("SAK35521371")
            ]
        );
        // And the URLs are still there — on the records, where `show` prints them.
        assert!(
            first.record.urls.iter().any(|url| url.url.contains("nbn-"))
                && second
                    .record
                    .urls
                    .iter()
                    .any(|url| url.url.contains("digital.zlb.de")),
            "the link belongs in urls[], not in the note"
        );
    }

    /// The rule behind that, over every fixture: a note's **message** never carries a
    /// value that is different for every record, because such a message cannot fold and
    /// `records[]` already answers "which record".
    ///
    /// The two values that are always record-specific are checked: the record's own
    /// number and any of its links. A quoted status word or platform wording is not one
    /// of them — those are vocabulary, they are the statement itself, and identical
    /// wordings fold correctly.
    #[test]
    fn no_note_message_carries_a_value_that_is_unique_to_its_record() {
        for (html, local) in RECORD_PAGES {
            let page = parsed(html, local);
            for note in &page.notes {
                assert!(
                    !note.message.contains(local),
                    "{local}: a record number in a message is what records[] is for: {}",
                    note.message
                );
                for url in &page.record.urls {
                    assert!(
                        !note.message.contains(&url.url),
                        "{local}: a link in a message never folds: {}",
                        note.message
                    );
                }
                if let Some(statement) = &holding(&page).holdings_statement {
                    assert!(
                        !note.message.contains(statement.as_str()),
                        "{local}: a quoted shelfmark never folds: {}",
                        note.message
                    );
                }
            }
        }
    }

    /// Dropping *unlabelled* rows would have been the cheap filter and would have cost
    /// three real links: three of the 23 sampled pages carry a second unlabelled row with
    /// a table of contents, a URN resolver or a viewer behind it.
    #[test]
    fn the_links_that_are_not_the_permalink_survive() {
        let page = parsed(ONLINE_URL, "SAK34364366");
        assert!(
            page.record
                .urls
                .iter()
                .any(|url| url.url.contains("nbn-resolving.de")),
            "{:?}",
            page.record.urls
        );
    }

    /// Six `Medienart` values fell through to `Unknown` and out of every `--format`
    /// filter. Each value here was read off a record page of the 2026-09-08 sample,
    /// except `Zeitschrift`, which stands beside the three serial spellings that were.
    #[test]
    fn the_measured_medienart_values_map_onto_the_format_vocabulary() {
        let cases = [
            ("[Buch]", Format::Book, false),
            ("[Band]", Format::Book, false),
            ("[CD]", Format::Audio, false),
            ("[DVD]", Format::Video, false),
            ("[Medienkombination]", Format::Mixed, false),
            ("[Noten]", Format::Score, false),
            ("[Karte/Plan]", Format::Map, false),
            ("[Konsolenspiel]", Format::Object, false),
            ("[Zeitung]", Format::Journal, false),
            ("[Zeitschrift]", Format::Journal, false),
            ("[Zeitschriftenartige Reihe]", Format::Journal, false),
            ("[Zeitschriftenheft]", Format::Journal, false),
            ("[E-Ressource]", Format::Ebook, true),
            ("[E-Book]", Format::Ebook, true),
            ("[E-Audio]", Format::Audio, true),
            // A level, not a material — deliberately unknown.
            ("[Mehrteiliges Werk]", Format::Unknown, false),
        ];
        for (medienart, format, online) in cases {
            assert_eq!(format_of(medienart, false), (format, online), "{medienart}");
        }
    }

    /// The trap behind `[Konsolenspiel]`, pinned so that "tidying" it to `electronic`
    /// fails loudly. `Format::Electronic` is one of the four words
    /// [`Record::is_online_resource`] treats as "nothing carries this off a shelf", and a
    /// cartridge in Spandau is carried off a shelf 23 times over: with `electronic` the
    /// copy that is out reads `currently unavailable` instead of `on loan` and loses the
    /// note that no return date is known.
    #[test]
    fn a_console_game_is_a_thing_on_a_shelf_and_not_an_online_resource() {
        let page = parsed(DUE_DATES, "SAK34906286");
        assert_eq!(page.record.format, Format::Object);
        assert!(!page.record.online);
        assert!(
            !page.record.is_online_resource(),
            "a cartridge on a shelf is borrowed like a book"
        );
        // The wrong word, spelled out so the test says what it is guarding against.
        assert!(
            Record {
                format: Format::Electronic,
                ..page.record.clone()
            }
            .is_online_resource()
        );
    }

    /// An electronic title is recognised by the online flag, not by `Format::Ebook`: the
    /// two parted company when `[E-Audio]` was measured, and asking for `Ebook` would
    /// read such a page as broken the moment it has a bare `URL` row instead of a
    /// lending link.
    #[test]
    fn an_electronic_title_of_any_material_can_state_a_bare_url() {
        let audio = ONLINE_URL.replace("[E-Ressource]", "[E-Audio]");
        let page = parsed(&audio, "SAK34364366");
        assert!(items(&page).is_empty());
        assert_eq!(kinds(&page), [note_kinds::VOEBB_ONLINE_URL_ONLY]);
        assert_eq!(page.record.format, Format::Audio);
        assert!(page.record.online);
    }

    /// A house the library list does not carry keeps its text and loses only the branch —
    /// and it is the *name* that decides, never the markup. `Stadtteilbibliothek
    /// Hakenfelde - ist zurzeit geschlossen` has no link around it, which is not why it
    /// does not resolve: the list writes the same house `Stadtbibliothek Spandau /
    /// Stadtteilbibliothek Hakenfelde (Eröffnung 2026)`, and no shape of the cell text is
    /// that. Guessing across the difference is what this module refuses to do.
    #[test]
    fn a_library_cell_without_a_link_still_names_its_house() {
        let page = parsed(DUE_DATES, "SAK34906286");
        let closed = items(&page)
            .iter()
            .find(|item| {
                item.branch_name
                    .as_deref()
                    .is_some_and(|name| name.starts_with("Stadtteilbibliothek Hakenfelde"))
            })
            .expect("the fixture carries the closed branch");
        assert_eq!(
            closed.branch_name.as_deref(),
            Some("Stadtteilbibliothek Hakenfelde - ist zurzeit geschlossen")
        );
        assert_eq!(closed.branch, None);
        // The copy is never dropped over it.
        assert_eq!(closed.status, Status::Unavailable);
    }

    /// A page without the bibliographic table is not a detail page. The error names the
    /// selector, because that string is the only thing that tells a maintainer what the
    /// site changed.
    #[test]
    fn a_page_without_the_bibliographic_table_names_the_selector() {
        let broken = AVAILABLE.replace("class=\"gi\"", "class=\"was-gi\"");
        let error = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect_err("a page without table.gi is not a detail page");
        assert!(
            matches!(
                error,
                Error::Unexpected(UnexpectedError::MissingSelector { .. })
            ),
            "{error}"
        );
        assert!(error.to_string().contains("table.gi"), "{error}");
    }

    /// The same rule one level down: a `Titel` row is what makes the tables a record.
    #[test]
    fn a_page_without_a_title_row_names_the_selector() {
        let broken = AVAILABLE.replace(">Titel</th>", ">Titelchen</th>");
        let error = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect_err("a record without a title is not a record");
        assert!(error.to_string().contains("Titel"), "{error}");
    }

    /// An item table whose header this parser does not recognise never yields a table read
    /// with shifted columns — that is how a four-column parser mangles a five-column
    /// table without anyone noticing.
    ///
    /// This used to assert an `Err`, and the assertion was right about everything but its
    /// blast radius: [`crate::http::scope_map`] fetches one record page per displayed
    /// record and is all-or-nothing, so the one unreadable page took the other nine hits'
    /// result with it (measured 2026-09-08). The record now stands with no copies and the
    /// selector travels in [`note_kinds::VOEBB_PAGE_UNREADABLE`] — the same string, in a
    /// place that costs one record instead of the answer.
    #[test]
    fn an_unrecognised_item_header_names_the_selector() {
        let broken = AVAILABLE.replace(">Verfügbarkeit</th>", ">Status</th>");
        let page = parsed(&broken, "SAK13776205");
        assert!(
            items(&page).is_empty(),
            "a header nobody understands yields no copies, never shifted ones"
        );
        assert_eq!(page.record.holdings[0].summary, Status::Unknown);
        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_PAGE_UNREADABLE)
            .expect("an empty copy list is never silent");
        assert!(note.message.contains("Verfügbarkeit"), "{note:?}");
        assert!(
            note.message.contains(REPORT_URL),
            "the selector is worth nothing without somewhere to report it: {note:?}"
        );
        assert_eq!(note.records, vec![RecordId::voebb("SAK13776205")]);
    }

    /// The second lending platform, found live on 2026-09-06: `Link zu Overdrive` where
    /// the fixture that shaped this parser had `Link zur Onleihe`. The vendor's name is
    /// not what makes a title electronic — the lending link is — so this record reads the
    /// same way, with a note that names the label it went by.
    #[test]
    fn a_lending_link_from_another_platform_is_read_the_same_way() {
        let page = parsed(OVERDRIVE, "SAK34672596");
        assert!(items(&page).is_empty());
        assert!(page.record.online, "an e-lending title is online");
        // Overdrive writes no parenthesis behind "Zugang zum Titel erhalten Sie hier.",
        // so nothing is stated about the loan — and nothing stated stays `Unknown`. The
        // two markers of §3.6 are matched positively; the absence of one is never the
        // other (round 2, §3.6).
        assert_eq!(page.record.holdings[0].summary, Status::Unknown);
        // And a platform that states nothing gets the tag that says so: this is the one
        // record `--available` may report as "not known to be on loan".
        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_ONLINE_STATE_UNSTATED)
            .expect("an empty copy list is never silent");
        assert!(note.message.contains("Overdrive"), "{note:?}");
        assert_eq!(
            note.records,
            vec![RecordId::voebb("SAK34672596")],
            "a note about one record names it: {note:?}"
        );
        assert!(
            page.record
                .urls
                .iter()
                .any(|url| url.kind == UrlKind::Fulltext),
            "the lending link is the resource itself: {:?}",
            page.record.urls
        );
    }

    /// A missing item table on a printed record with no lending link and no access URL is
    /// never a silently empty copy list — it is the loud note that names the selector.
    ///
    /// It used to be an error, and the sentence it was written for still holds: an empty
    /// copy list on its own reads as "held nowhere". What changed is only *where* the
    /// noise goes. `CLAUDE.md`'s rule is that a missing selector must never become an
    /// empty result; a note carrying the selector, tagged for agents and printed for
    /// humans, keeps that promise while leaving the other records of the window alone.
    #[test]
    fn a_missing_item_table_without_a_lending_link_is_a_loud_note() {
        let broken = AVAILABLE.replace("id=\"resptable-1\"", "id=\"resptable-2\"");
        let page = parsed(&broken, "SAK13776205");
        assert!(items(&page).is_empty());
        assert_eq!(
            page.record.holdings[0].summary,
            Status::Unknown,
            "no copies were read, so nothing may be claimed about them"
        );
        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_PAGE_UNREADABLE)
            .expect("a missing selector is never a silent empty result");
        assert!(note.message.contains("resptable-1"), "{note:?}");
    }

    /// The fourth state of the item table, live on 2026-09-08 (`voebb_SAK34364366`): an
    /// electronic title with **no** item table, **no** `Link zu …` row, and its access in
    /// a plain `URL` row. Nine sound hits of `--author "von Schirach" --at AGB` were
    /// thrown away over this one page before it had a name.
    #[test]
    fn an_electronic_title_may_state_its_access_as_a_bare_url() {
        let page = parsed(ONLINE_URL, "SAK34364366");
        assert!(items(&page).is_empty(), "an e-resource has no copies");
        assert_eq!(page.record.format, Format::Ebook);
        assert!(page.record.online);
        assert_eq!(
            page.record.holdings[0].summary,
            Status::Unknown,
            "voebb.de says nothing about whether this access is free, so neither do we"
        );

        let kinds: Vec<&str> = page.notes.iter().map(|note| note.kind).collect();
        assert_eq!(kinds, vec![note_kinds::VOEBB_ONLINE_URL_ONLY], "{kinds:?}");
        let note = &page.notes[0];
        // This assertion used to require the URL **in the message** ("the note says where
        // the access points"). It is inverted rather than deleted, because the behaviour
        // it pinned was the bug: a message carrying the record's own link is unique per
        // record, so `Note::merged` never folded two of them and three such records in one
        // window printed three near-identical paragraphs. The link is on the record.
        assert!(
            !note
                .message
                .contains("http://nbn-resolving.de/urn:nbn:de:kobv:109-1-15402775"),
            "the link belongs in urls[], not in the message: {note:?}"
        );
        assert!(
            page.record
                .urls
                .iter()
                .any(|url| url.url == "http://nbn-resolving.de/urn:nbn:de:kobv:109-1-15402775"),
            "{:?}",
            page.record.urls
        );
        assert_eq!(note.records, vec![RecordId::voebb("SAK34364366")]);
    }

    /// The trap this page carries: its hint block reads "Bitte klicken Sie auf den Link
    /// zum Anbieter", which begins with the very prefix [`LENDING_LINK`] matches. It is
    /// prose in a `p.info` and not a row of `table.gi`, so a parser that searched the
    /// page's *text* would read a lending link that is not there — and then report a loan
    /// status nobody stated.
    #[test]
    fn the_hint_prose_is_not_mistaken_for_a_lending_link() {
        let page = parsed(ONLINE_URL, "SAK34364366");
        assert!(
            ONLINE_URL.contains("Link zum Anbieter"),
            "the fixture has to keep the trap it exists for"
        );
        assert!(
            page.notes
                .iter()
                .all(|note| note.kind != note_kinds::VOEBB_ONLINE_ONLY
                    && note.kind != note_kinds::VOEBB_ONLINE_STATE_UNSTATED),
            "prose in a p.info is not a lending link: {:?}",
            page.notes
        );
        assert!(
            page.record
                .urls
                .iter()
                .all(|url| url.kind != UrlKind::Fulltext),
            "no row is labelled with a lending link, so nothing is one: {:?}",
            page.record.urls
        );
    }

    /// The two markers of the fourth state are both required. A printed book whose item
    /// table stopped matching also carries a `URL` row — for its table of contents — and
    /// reading that as "an electronic title without copies" would turn a page this parser
    /// no longer understands into a confident answer.
    #[test]
    fn a_bare_url_alone_does_not_make_a_record_electronic() {
        let broken = ON_LOAN
            .replace("id=\"resptable-1\"", "id=\"resptable-2\"")
            .replace(">Inhaltsverzeichnis</th>", ">URL</th>");
        let page = parsed(&broken, "SAK00177143");
        assert_eq!(page.record.format, Format::Book, "Medienart says [Band]");
        let kinds: Vec<&str> = page.notes.iter().map(|note| note.kind).collect();
        assert_eq!(
            kinds,
            vec![note_kinds::VOEBB_PAGE_UNREADABLE],
            "a book with a URL row is a page we cannot read, not an e-resource: {kinds:?}"
        );
    }

    /// A status word this tool does not know degrades one copy to
    /// [`Status::Unknown`] plus a note — it never becomes "available".
    #[test]
    fn an_unknown_status_word_is_unknown_and_stated() {
        let broken = AVAILABLE.replace(
            "<span class=\"available\">Verfügbar</span>",
            "<span>Steht zur Ansicht bereit</span>",
        );
        let page = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect("an unknown status word is not a parse failure");
        assert_eq!(statuses(&page), vec![Status::Unknown]);
        assert!(
            page.notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_UNKNOWN_STATUS),
            "{:?}",
            page.notes
        );
    }

    /// Class and word contradicting each other resolves to the pessimistic status, with
    /// the disagreement stated — a conflict must never end as "go and fetch it".
    #[test]
    fn a_contradicting_class_and_word_is_unavailable_and_stated() {
        let broken = AVAILABLE.replace(
            "<span class=\"available\">Verfügbar</span>",
            "<span class=\"notavailable\">Verfügbar</span>",
        );
        let page = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect("a contradiction is not a parse failure");
        assert_eq!(statuses(&page), vec![Status::Unavailable]);
        assert!(
            page.notes
                .iter()
                .any(|note| note.kind == note_kinds::AVAILABILITY_STATUS_CONFLICT),
            "{:?}",
            page.notes
        );
    }

    #[test]
    fn the_isbd_title_splits_only_on_the_spaced_separators() {
        assert_eq!(
            title_of("Bernhard Schlink: Der Vorleser / Lars Hofmann ; Sascha Feuchert"),
            ("Bernhard Schlink: Der Vorleser".to_string(), None)
        );
        assert_eq!(
            title_of("Der Vorleser : Roman / Bernhard Schlink"),
            ("Der Vorleser".to_string(), Some("Roman".to_string()))
        );
    }

    /// A language name the table does not carry is left out rather than guessed at.
    #[test]
    fn only_known_language_names_become_codes() {
        assert_eq!(languages_of(&["Deutsch".to_string()]), vec!["ger"]);
        assert_eq!(
            languages_of(&["Deutsch".to_string(), "Englisch".to_string()]),
            vec!["ger", "eng"]
        );
        assert!(languages_of(&["Sorbisch".to_string()]).is_empty());
    }

    /// The `ISBN` cell also carries binding and price notes; only an ISBN comes through.
    #[test]
    fn isbns_lose_their_hyphens_and_a_price_is_not_one() {
        assert_eq!(
            isbns_of(&["978-3-15-950138-3".to_string()]),
            vec!["9783159501383"]
        );
        assert_eq!(isbns_of(&["3-464-61634-X".to_string()]), vec!["346461634X"]);
        assert!(isbns_of(&["EUR 9,95".to_string()]).is_empty());
    }
}
