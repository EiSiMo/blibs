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
//! **The item table has three states, and two of them are empty for different reasons.**
//! Present with rows is the normal case; present without a header row is a multi-part
//! work whose volumes are records of their own (`detail_multivolume`); absent altogether
//! is an electronic title whose loan state sits in the link text (`detail_online`,
//! `detail_overdrive`). Each
//! empty item list carries a note saying which — an empty list on its own is
//! indistinguishable from "held nowhere", which is the worst answer this tool can give.
//!
//! **The item table names no ISIL and no branch id.** The branch is matched from the text
//! of the *Bibliothek* column against the VÖBB branches of the library list, folded and
//! in stages. A name the list does not carry keeps its text in
//! [`crate::model::Item::branch_name`] and leaves [`crate::model::Item::branch`] empty —
//! it is never guessed at, and the copy is never dropped.

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};

use crate::error::{Error, UnexpectedError};
use crate::libraries::{self, Branch, VOEBB_NETWORK, text::fold};
use crate::model::{
    Author, AuthorKind, Format, Holding, Isil, Item, Note, Record, RecordId, ResourceUrl, Status,
    UrlKind, note_kinds,
};

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
/// Fails only when the page is not a detail page at all — no `table.gi`, no `Titel` row,
/// an item table whose header this parser does not recognise, or a missing item table on
/// a record that has no lending link either. Everything softer is a note.
pub fn parse_detail(html: &str, id: &RecordId) -> Result<DetailPage, Error> {
    let document = Html::parse_document(html);
    let bibliographic = Bibliographic::read(&document)?;
    let mut notes = Vec::new();
    let items = items_of(&document, &bibliographic, &mut notes)?;
    let record = record_of(id, &bibliographic, items);
    Ok(DetailPage { record, notes })
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
    let online = lending_link(bibliographic).is_some();
    let (format, online) = format_of(bibliographic.first("Medienart").unwrap_or_default(), online);

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
        urls: urls_of(bibliographic),
        holdings: vec![holding_of(id, items)],
    }
}

/// The one holding of a voebb.de record: the network itself.
///
/// voebb.de answers for `DE-609` and nothing else, so a record has exactly one holding
/// and the branches live in its copies. An empty item list means "the page stated no
/// copies", never "held nowhere" — the note that comes with it says which of the two
/// states of the item table produced it.
fn holding_of(id: &RecordId, items: Vec<Item>) -> Holding {
    let mut holding = Holding {
        isil: Some(Isil::new(VOEBB_NETWORK)),
        alias: None,
        library: String::new(),
        short_name: None,
        local_id: Some(id.local_id().to_string()),
        mine: false,
        summary: Status::summarize(items.iter().map(|item| item.status)),
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
/// | `Hörbuch`, `CD`, `Tonträger` | [`Format::Audio`] |
/// | `DVD`, `Blu-ray`, `Video` | [`Format::Video`] |
/// | `Medienkombination` | [`Format::Mixed`] |
/// | anything else, `Mehrteiliges Werk` included | [`Format::Unknown`] |
///
/// `Mehrteiliges Werk` is deliberately in the last row: it states a bibliographic
/// *level*, not a material, and the volumes that carry the material are records of their
/// own. `online` also comes in from a `Link zu …` row, which is why it is an
/// argument here and not derived from the table alone.
pub(in crate::engine::voebb) fn format_of(medienart: &str, online: bool) -> (Format, bool) {
    let value = fold(medienart.trim_matches(['[', ']']).trim());
    let format = match value.as_str() {
        "band" | "buch" => Format::Book,
        "e-ressource" | "e-book" | "onleihe" => return (Format::Ebook, true),
        "horbuch" | "cd" | "tontrager" => Format::Audio,
        "dvd" | "blu-ray" | "video" => Format::Video,
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

/// The links of the bibliographic tables, classified by the label of their row.
///
/// Only labelled rows are read. The unlabelled first row of every page is the record's
/// own permalink, which is already in [`crate::model::RecordId`] and would otherwise put
/// a self-link into every record.
fn urls_of(bibliographic: &Bibliographic) -> Vec<ResourceUrl> {
    let mut urls = Vec::new();
    for row in &bibliographic.rows {
        let kind = url_kind(&row.label);
        for (url, label) in &row.links {
            urls.push(ResourceUrl {
                url: url.clone(),
                kind,
                label: non_empty(label),
            });
        }
    }
    urls
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
/// The three states of `table#resptable-1`, all three fixture-backed:
///
/// | State | Meaning | Result |
/// | --- | --- | --- |
/// | table with rows | a normal record | the copies |
/// | table without rows (and without `<thead>`) | a multi-part work; the volumes are records of their own | empty, [`note_kinds::VOEBB_MULTIVOLUME`] |
/// | no table, but a `Link zu …` row | an electronic title; the loan state is in the link text | empty, [`note_kinds::VOEBB_ONLINE_ONLY`] |
///
/// A missing table on a record that has no lending link is a named error: at that
/// point the page has stopped being the page this parser knows, and an empty copy list
/// would read as "held nowhere".
fn items_of(
    document: &Html,
    bibliographic: &Bibliographic,
    notes: &mut Vec<Note>,
) -> Result<Vec<Item>, Error> {
    let selectors = selectors();
    let Some(table) = document.select(&selectors.item_table).next() else {
        let Some(row) = lending_link(bibliographic) else {
            return Err(missing_selector("table#resptable-1"));
        };
        let state = row.values.first().map_or("", String::as_str);
        notes.push(Note::new(
            note_kinds::VOEBB_ONLINE_ONLY,
            format!(
                "this is an electronic title ({}): it has no copies on a shelf, and \
                 voebb.de states its loan status only as {state:?}",
                row.label
            ),
        ));
        return Ok(Vec::new());
    };

    let rows: Vec<ElementRef<'_>> = table.select(&selectors.item_row).collect();
    if rows.is_empty() {
        notes.push(Note::new(
            note_kinds::VOEBB_MULTIVOLUME,
            "voebb.de lists no copies for this record — for a multi-part work the copies \
             belong to the volumes, which are records of their own; search for the volume",
        ));
        return Ok(Vec::new());
    }

    let columns = Columns::from_header(table)?;
    Ok(rows
        .into_iter()
        .map(|row| item(row, &columns, notes))
        .collect())
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
        order_option,
    }
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
/// — never a guess, and never quietly [`Status::Available`].
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

/// Compile one selector literal.
fn compile(css: &str) -> Selector {
    Selector::parse(css).expect("every selector in this module is a literal and must parse")
}

/// All text below an element, whitespace collapsed. The page indents its cells over
/// several lines and pads the order column with runs of spaces.
fn text_of(element: ElementRef<'_>) -> String {
    collapse(&element.text().collect::<String>())
}

/// Collapse runs of whitespace and trim. `&nbsp;` counts as whitespace here, which is
/// what turns the unlabelled permalink row into an empty label.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `None` for an empty string, so that "the cell was blank" and "there is no such column"
/// stay the same thing in the output.
fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// A missing-selector error naming what stopped matching.
fn missing_selector(selector: &str) -> Error {
    UnexpectedError::MissingSelector {
        selector: selector.to_string(),
        document: DOCUMENT.to_string(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABLE: &str = include_str!("../../../../tests/fixtures/voebb/detail_available.html");
    const ON_LOAN: &str = include_str!("../../../../tests/fixtures/voebb/detail_on_loan.html");
    const REFERENCE: &str = include_str!("../../../../tests/fixtures/voebb/detail_reference.html");
    const ONLINE: &str = include_str!("../../../../tests/fixtures/voebb/detail_online.html");
    const MULTIVOLUME: &str =
        include_str!("../../../../tests/fixtures/voebb/detail_multivolume.html");
    const UNKNOWN: &str = include_str!("../../../../tests/fixtures/voebb/detail_unknown.html");
    const OVERDRIVE: &str = include_str!("../../../../tests/fixtures/voebb/detail_overdrive.html");

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
        assert_eq!(item.branch.as_deref(), Some("SIG00036"));
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
        assert_eq!(page.record.holdings[0].summary, Status::Unknown);

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

    /// An item table whose header this parser does not recognise is an error, not a table
    /// read with shifted columns — that is how a four-column parser mangles a five-column
    /// table without anyone noticing.
    #[test]
    fn an_unrecognised_item_header_names_the_selector() {
        let broken = AVAILABLE.replace(">Verfügbarkeit</th>", ">Status</th>");
        let error = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect_err("a copy without a status column says nothing");
        assert!(error.to_string().contains("Verfügbarkeit"), "{error}");
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
        let note = page
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_ONLINE_ONLY)
            .expect("an empty copy list is never silent");
        assert!(note.message.contains("Overdrive"), "{note:?}");
        assert!(
            page.record
                .urls
                .iter()
                .any(|url| url.kind == UrlKind::Fulltext),
            "the lending link is the resource itself: {:?}",
            page.record.urls
        );
    }

    /// A missing item table on a record with no lending link at all is a named error:
    /// an empty copy list would read as "held nowhere".
    #[test]
    fn a_missing_item_table_without_a_lending_link_is_an_error() {
        let broken = AVAILABLE.replace("id=\"resptable-1\"", "id=\"resptable-2\"");
        let error = parse_detail(&broken, &RecordId::voebb("SAK13776205"))
            .expect_err("a record with neither copies nor a lending link is broken");
        assert!(error.to_string().contains("resptable-1"), "{error}");
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
