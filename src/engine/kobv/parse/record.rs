//! MARC to [`Record`].
//!
//! One function per output field, each independently testable against a fixture. Which
//! MARC field fills which output field is settled in `plan/marc-mapping.md` and is not
//! re-derived here.
//!
//! The `924` rules are the ones with teeth: subfields are always `$a$b$c$d`, `$d` is
//! ignored, `$b` is the ISIL and `$a` the library's own record number, an ISIL may occur
//! several times and is deduplicated when the availability key is built, and a record with
//! no `924` at all — 4.9 % of them — yields an **empty** holdings list that means "not
//! stated in this record", never "held nowhere". `852` also occurs in SRU records and is
//! deliberately not used.
//!
//! Only the record's identity can fail. Everything else degrades, because a record whose
//! `264` is malformed still answers "is this book in one of my libraries" — dropping it
//! would answer that question wrongly.

use crate::error::{Error, UnexpectedError};
use crate::model::{
    Author, AuthorKind, AvailabilityId, Format, Holding, Isil, Record, RecordId, ResourceUrl,
    Status, UrlKind,
};

use super::coded::{Coded008, is_language_code, plausible_year};
use super::marc::{Field, MarcRecord};

/// Trailing ISBD punctuation on a title, subtitle or edition statement.
///
/// `.` is **not** in the set: it is meaningful in abbreviations and volume designations
/// (`Vol. 49`, `2. Aufl.`), and `...` is an ellipsis inside the title itself
/// (`kobvindex_LDAz00124`).
const TITLE_MARKS: &[char] = &[' ', '/', ':', ';', ',', '='];

/// Trailing punctuation on a place or publisher: `Frankfurt am Main :` and
/// `S. Fischer,` are how AACR2 records spell them.
const IMPRINT_MARKS: &[char] = &[' ', ':', ';', ','];

/// Trailing punctuation on an extent statement. `:` and `;` introduce `300$b`/`$c`; the
/// full stop belongs to the abbreviation (`293 S.`) and stays.
const EXTENT_MARKS: &[char] = &[' ', ':', ';'];

/// Trailing punctuation on a subject heading (`Seerecht.`).
const SUBJECT_MARKS: &[char] = &[' ', '.'];

/// Trailing punctuation on life dates (`1911-2004.`, `1821-1881,`).
const DATE_MARKS: &[char] = &[' ', '.', ','];

/// The name fields, in the order their authors are emitted: main entries first, then
/// added entries. 254 records have no `1XX` at all and only a `700`, so added entries are
/// never "just contributors" — dropping them would empty `authors` for a fifth of the
/// records that have any.
const NAME_FIELDS: [(&str, AuthorKind); 6] = [
    ("100", AuthorKind::Person),
    ("110", AuthorKind::Corporate),
    ("111", AuthorKind::Meeting),
    ("700", AuthorKind::Person),
    ("710", AuthorKind::Corporate),
    ("711", AuthorKind::Meeting),
];

/// The `$0` prefix of a GND identifier. `$0` is repeatable and carries K10plus, SWB and
/// KOBV numbers alongside it, so taking the *first* `$0` yields the wrong authority two
/// times out of three.
const GND_PREFIX: &str = "(DE-588)";

/// The other spelling of the same identifier, as a URI.
const GND_URI: &str = "d-nb.info/gnd/";

/// Values of `008/35-37` that name no language: "undetermined", "no linguistic content"
/// and "multiple languages". Emitting them would be a false statement, and neither is a
/// hit for `--language ger`.
const UNSPECIFIC_LANGUAGES: [&str; 3] = ["und", "zxx", "mul"];

/// The carrier code that means "online resource", in `007/00-01` and in `338$b`.
const ONLINE_CARRIER: &str = "cr";

/// Build a [`Record`] from a MARC record.
///
/// The only failure is the record's identity: a missing `001`, or one without a source
/// prefix, cannot be turned into a [`RecordId`], and an id is what `show` and the
/// availability call are addressed with. Every other field degrades — a missing `245`
/// yields an empty title, a shifted leader yields [`Format::Unknown`], a record without
/// `924` yields no holdings — because a record that answers the user's question must not
/// be dropped over a field that does not.
pub fn from_marc(marc: &MarcRecord) -> Result<Record, Error> {
    let id = record_id(marc)?;
    let title_field = marc.first_with("245", 'a');
    let online = is_online(marc);
    let (place, publisher) = imprint(marc);
    Ok(Record {
        id,
        title: title(title_field),
        subtitle: title_field.and_then(subtitle),
        authors: authors(marc),
        year: year(marc),
        publisher,
        place,
        edition: edition(marc),
        extent: extent(marc),
        languages: languages(marc),
        format: format(marc, online),
        online,
        isbns: isbns(marc),
        subjects: subjects(marc),
        urls: urls(marc),
        holdings: holdings(marc),
    })
}

/// Assemble the availability key from the holdings.
///
/// `ISIL;LocalId` pairs joined by commas and terminated by one, exactly as the portal
/// writes it into `data-availability-id`. The ISIL is deduplicated while the field order
/// is kept — 6.3 % of records name the same library twice in `924`, and the response is
/// keyed by ISIL, so a repeated key would collide with itself. The local id is the
/// library's own (`924$a`) and is **never** reconstructed from `001`: a quarter of them
/// differ.
///
/// `None` when the record has no usable holding — there is then nothing to ask about, and
/// the call is skipped rather than sent empty.
pub fn availability_id(holdings: &[Holding]) -> Option<AvailabilityId> {
    let mut key = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for holding in holdings {
        let (Some(isil), Some(local_id)) = (holding.isil.as_ref(), holding.local_id.as_deref())
        else {
            continue;
        };
        if seen.contains(&isil.as_str()) {
            continue;
        }
        seen.push(isil.as_str());
        key.push_str(isil.as_str());
        key.push(';');
        key.push_str(local_id);
        key.push(',');
    }
    (!key.is_empty()).then(|| AvailabilityId::new(&key))
}

/// The record's identity, from `001`.
///
/// `003` (always `DE-602`) and `092$a` (always identical to `001`) carry no information
/// and are ignored. The id is opaque: it may hold umlauts, `+` and further underscores,
/// so it is passed on verbatim.
fn record_id(marc: &MarcRecord) -> Result<RecordId, Error> {
    let raw = marc
        .control("001")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| UnexpectedError::MissingElement {
            what: "controlfield 001".to_owned(),
            context: "MARCXML record from SRU".to_owned(),
        })?;
    Ok(RecordId::from_catalog(raw)?)
}

/// `245$a`, with the volume designation appended.
///
/// `$n` (volume number) and `$p` (volume title) are both repeatable and interleave; the
/// subfield order inside the field is the reading order, so they are appended in that
/// order rather than grouped by code. Without them a volume is indistinguishable from its
/// series — `kobvindex_ZLB12845080` would otherwise be three identical lines.
fn title(field: Option<&Field>) -> String {
    let Some(field) = field else {
        return String::new();
    };
    let mut title = trimmed(field.sub('a').unwrap_or_default(), TITLE_MARKS);
    for (code, value) in &field.subfields {
        if *code != 'n' && *code != 'p' {
            continue;
        }
        let part = trimmed(value, TITLE_MARKS);
        if part.is_empty() {
            continue;
        }
        if !title.is_empty() {
            title.push_str(". ");
        }
        title.push_str(&part);
    }
    title
}

/// `245$b`, taken from the **same** field as the title.
///
/// Ten records carry two `245` fields and in two of them the first has no `$a`; reading
/// `$b` from the record globally would then pair one work's title with another's subtitle.
fn subtitle(field: &Field) -> Option<String> {
    non_empty(trimmed(field.sub('b')?, TITLE_MARKS))
}

/// Every name in the record, main entries before added entries, deduplicated.
///
/// Deduplication is over `(name, dates, gnd)`: a name-title added entry repeats the same
/// person once per work, six times over in `almahu_BV010644426`. Roles are never filtered
/// — an editor or a translator is who the user is looking for as often as the author.
fn authors(marc: &MarcRecord) -> Vec<Author> {
    let mut authors: Vec<Author> = Vec::new();
    for (tag, kind) in NAME_FIELDS {
        for field in marc.fields(tag) {
            let Some(candidate) = author(field, kind) else {
                continue;
            };
            let duplicate = authors.iter().any(|seen| {
                seen.name == candidate.name
                    && seen.dates == candidate.dates
                    && seen.gnd == candidate.gnd
            });
            if !duplicate {
                authors.push(candidate);
            }
        }
    }
    authors
}

/// One name field.
///
/// The name is passed through as the record spells it, usually inverted. It is **never**
/// split at the comma to build "forename surname": 1.4 % of names are single-part or
/// initials, and the inverted form is the one library users search with anyway.
fn author(field: &Field, kind: AuthorKind) -> Option<Author> {
    let name = match kind {
        AuthorKind::Person => field.sub('a')?.to_owned(),
        AuthorKind::Corporate | AuthorKind::Meeting => corporate_name(field)?,
    };
    Some(Author {
        name: non_empty(name)?,
        kind,
        dates: dates(field, kind),
        gnd: gnd(field),
        role: role(field),
    })
}

/// A corporate or meeting name: `$a`, then the subordinate unit `$b`, then the place
/// `$g`. For theses, official publications and maps this is often the only creator in the
/// record (`gbv_014140950`), so leaving it out would empty `authors` for whole material
/// classes.
fn corporate_name(field: &Field) -> Option<String> {
    let parts: Vec<&str> = ['a', 'b', 'g']
        .into_iter()
        .filter_map(|code| field.sub(code))
        .filter(|value| !value.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(", "))
}

/// `$d`, for persons only.
///
/// A passed-through string, never a parsed interval: the observed forms include an open
/// start (`-1204`), a fuzzy end (`1742-18XX`) and stray punctuation. On `110`/`710` `$d`
/// means the date *of the body*, which is a different statement, and occurs once.
fn dates(field: &Field, kind: AuthorKind) -> Option<String> {
    if kind != AuthorKind::Person {
        return None;
    }
    non_empty(trimmed(field.sub('d')?, DATE_MARKS))
}

/// The GND identifier from `$0`, in either of its two spellings.
///
/// Two traps: `$0` is repeatable and holds `(DE-627)`, `(DE-576)`, `(DE-609)` and
/// `(orcid)` values alongside the GND, so the *first* one is usually wrong; and the value
/// is a string, not a number — 533 of them carry a check digit (`2083235-7`, `5058098-X`).
///
/// A field with `$t` is a name-title entry, where `$0` identifies the **work**, not the
/// person (`almahu_BV010644426` has seven such entries for one author, each with its own
/// number). Taking it would attach a work's GND to a person and, worse, would make every
/// one of those entries look like a different human being.
fn gnd(field: &Field) -> Option<String> {
    if field.sub('t').is_some() {
        return None;
    }
    field.subs('0').find_map(|value| {
        if let Some(identifier) = value.strip_prefix(GND_PREFIX) {
            return non_empty(identifier.to_owned());
        }
        if value.contains(GND_URI) {
            return non_empty(value.rsplit('/').next().unwrap_or_default().to_owned());
        }
        None
    })
}

/// The relator from `$4`, falling back to the free-text `$e`.
///
/// `$4` is sometimes the full `LoC` URI instead of the code, so the last path segment is
/// what counts. The vocabulary is open, not an enum: `kom`, `isb`, `dgg`, `dgs` and `wac`
/// are German extensions that no `LoC` list contains, and `$e` is free text in two
/// languages with inconsistent full stops.
fn role(field: &Field) -> Option<String> {
    if let Some(code) = field.sub('4') {
        let segment = code.rsplit('/').next().unwrap_or(code);
        if let Some(role) = non_empty(segment.to_owned()) {
            return Some(role);
        }
    }
    non_empty(field.sub('e')?.to_owned())
}

/// The publication year: `008/07-10` first, the imprint date only as a fallback.
///
/// **The order is not interchangeable.** Where the two disagree, `008` is right in every
/// checked case: `almafu_BV026062583` has `264$c = "1413 [1992]"`, where the regular
/// expression grabs the Hijri year and `008` says 1992.
fn year(marc: &MarcRecord) -> Option<i32> {
    let coded = marc.control("008").and_then(Coded008::new);
    coded
        .as_ref()
        .and_then(Coded008::year)
        .or_else(|| imprint_year(marc))
}

/// The first four-digit year in `264$c`, else in `260$c`.
///
/// A sixth of these fields are not a bare year: spans, `[ca. 1990]`, `c 2005`, `19XX-`,
/// and two foreign calendars. The first four-digit group is the best available guess, and
/// it is discarded when it cannot be a publication date — printing `5740` from
/// `5740- [1979/1980-]` would be worse than printing nothing.
fn imprint_year(marc: &MarcRecord) -> Option<i32> {
    marc.fields("264")
        .chain(marc.fields("260"))
        .filter_map(|field| field.sub('c'))
        .find_map(first_year)
}

/// The first group of exactly four digits in a free-text date.
///
/// "Exactly four" matters: `PublicationDate: 20231231` must not read as 2023.
fn first_year(text: &str) -> Option<i32> {
    let mut digits = String::new();
    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        if digits.len() == 4 {
            return digits.parse::<i32>().ok().and_then(plausible_year);
        }
        digits.clear();
    }
    None
}

/// Place and publisher, taken from **one** field so that the two cannot come from
/// different decades of the same serial.
fn imprint(marc: &MarcRecord) -> (Option<String>, Option<String>) {
    let Some(field) = imprint_field(marc) else {
        return (None, None);
    };
    (
        field
            .sub('a')
            .and_then(|value| non_empty(trimmed(value, IMPRINT_MARKS))),
        field
            .sub('b')
            .and_then(|value| non_empty(trimmed(value, IMPRINT_MARKS))),
    )
}

/// The imprint field: `264` with `ind2=1` (publisher), else any `264`, else `260`.
///
/// The loosening is measured, not defensive: 30 `ind2=4` fields (copyright date, which
/// should hold nothing but `$c`) carry the only place and publisher the record has. And
/// `264` repeats — `gbv_130857440` has twelve, one per change of publisher over a century
/// — so the first usable one wins and the rest are dropped; a second imprint has nowhere
/// to go in a two-line output.
fn imprint_field(marc: &MarcRecord) -> Option<&Field> {
    let usable = |field: &&Field| field.sub('a').is_some() || field.sub('b').is_some();
    marc.fields("264")
        .find(|field| field.ind2_digit() == Some(1) && usable(field))
        .or_else(|| marc.fields("264").find(usable))
        .or_else(|| marc.fields("260").find(usable))
}

/// `250$a`. Free text in the language of the work, with no extractable number. The full
/// stop stays — `2. Aufl.` is not a sentence.
fn edition(marc: &MarcRecord) -> Option<String> {
    let field = marc.first_with("250", 'a')?;
    non_empty(trimmed(field.sub('a')?, TITLE_MARKS))
}

/// `300$a` of the first `300` that has one — 43 records have a `300` without `$a`, and 29
/// have several `300` fields, one per volume.
fn extent(marc: &MarcRecord) -> Option<String> {
    let field = marc.first_with("300", 'a')?;
    non_empty(trimmed(field.sub('a')?, EXTENT_MARKS))
}

/// The union of `008/35-37` and `041$a`, `008` first.
///
/// A union rather than a fallback: 49 records have a broken `008` (`|||`) and a correct
/// `041` (`kobvindex_SLB42822`), and 132 are genuinely multilingual, where `008` names
/// the first language and `041` the rest (`gbv_1603494723`: `tur`, `eng`, `tam`).
/// Anything that is not three lowercase letters is discarded rather than repaired — the
/// value `lateng` is two codes in one subfield and splitting it would invent a claim.
fn languages(marc: &MarcRecord) -> Vec<String> {
    let mut languages: Vec<String> = Vec::new();
    let coded = marc.control("008").and_then(Coded008::new);
    if let Some(code) = coded.as_ref().and_then(Coded008::language) {
        if !UNSPECIFIC_LANGUAGES.contains(&code) {
            languages.push(code.to_owned());
        }
    }
    for field in marc.fields("041") {
        for code in field.subs('a') {
            if is_language_code(code) && !languages.iter().any(|seen| seen == code) {
                languages.push(code.to_owned());
            }
        }
    }
    languages
}

/// Whether the resource is online, from `007/00-01` or `338$b`.
///
/// Both sources are needed: 51 records have `007 = cr` and no `338` at all, 19 the other
/// way round. This is the only thing that tells an e-book from a printed book — in the
/// leader they are identical.
fn is_online(marc: &MarcRecord) -> bool {
    marc.controls("007")
        .any(|value| value.starts_with(ONLINE_CARRIER))
        || marc
            .fields("338")
            .any(|field| field.subs('b').any(|value| value == ONLINE_CARRIER))
}

/// The material type, from the leader alone plus the online flag.
///
/// A leader too short to hold positions 06 and 07 is [`Format::Unknown`], like the six
/// records whose leader is uncoded or shifted. `format` is a display and filter field,
/// never an identity, so an unknown value costs nothing and a guess would cost a hit.
fn format(marc: &MarcRecord, online: bool) -> Format {
    let leader = marc.leader();
    match (leader.kind(), leader.level()) {
        (Some(kind), Some(level)) => Format::from_leader(kind, level, online),
        _ => Format::Unknown,
    }
}

/// `020$a`, cut at the first space.
///
/// `$z` is a cancelled ISBN, `$9` a hyphenated KOBV duplicate of `$a` and `$q` the
/// binding — taking any of them would either list a wrong number or list every ISBN twice.
/// Hyphens are kept as the record has them; normalising for comparison happens in `cli`.
fn isbns(marc: &MarcRecord) -> Vec<String> {
    marc.fields("020")
        .flat_map(|field| field.subs('a'))
        .filter_map(|value| {
            let isbn = value.split_once(' ').map_or(value, |(head, _)| head);
            non_empty(isbn.to_owned())
        })
        .collect()
}

/// `650$a` ∪ `689$a` ∪ `653$a`, deduplicated by exact text.
///
/// A union, not a concatenation: 137 of the 180 records that have both `650` and `689`
/// repeat at least one heading verbatim. `689` fields without `$a` are skipped rather than
/// emitted as empty headings — the last link of every RSWK chain carries only `$5`, the
/// contributing institution.
///
/// The result is deliberately mixed-language: GND headings stand next to LCSH and FAST
/// terms in the same record, and filtering to one vocabulary would strip the indexing off
/// every foreign-language title.
fn subjects(marc: &MarcRecord) -> Vec<String> {
    let mut subjects: Vec<String> = Vec::new();
    for tag in ["650", "689", "653"] {
        for field in marc.fields(tag) {
            for value in field.subs('a') {
                let Some(subject) = non_empty(trimmed(value, SUBJECT_MARKS)) else {
                    continue;
                };
                if !subjects.contains(&subject) {
                    subjects.push(subject);
                }
            }
        }
    }
    subjects
}

/// `856$u`, with the link text and what it points at.
///
/// Three quarters of these links are covers and tables of contents. An agent that takes
/// the first link for the full text would be wrong most of the time, which is why the
/// kind is carried alongside. It comes from the link **text** (`$3` → `$y` → `$z`), never
/// from `ind2`: `ind2=0` means "the resource itself" in theory and marks cover images in
/// practice.
fn urls(marc: &MarcRecord) -> Vec<ResourceUrl> {
    marc.fields("856")
        .flat_map(|field| {
            let label = ['3', 'y', 'z']
                .into_iter()
                .find_map(|code| field.sub(code))
                .and_then(|value| non_empty(value.to_owned()));
            let kind = url_kind(label.as_deref());
            field
                .subs('u')
                .filter(|url| !url.is_empty())
                .map(|url| ResourceUrl {
                    url: url.to_owned(),
                    kind,
                    label: label.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Classify a link by its label. An unlabelled or unrecognised link is
/// [`UrlKind::Other`] — never a guess, because guessing "full text" is exactly the error
/// this field exists to prevent.
fn url_kind(label: Option<&str>) -> UrlKind {
    let Some(label) = label else {
        return UrlKind::Other;
    };
    let label = label.to_lowercase();
    if label.contains("volltext") || label.contains("full text") || label == "full" {
        UrlKind::Fulltext
    } else if label.contains("inhaltsverzeichnis") || label.contains("table of contents") {
        UrlKind::Toc
    } else if label.contains("cover") {
        UrlKind::Cover
    } else if label.contains("verlag") || label.contains("publisher") {
        UrlKind::Publisher
    } else {
        UrlKind::Other
    }
}

/// The holdings stated in the record, one per `924`.
///
/// `library` is filled with the bare ISIL here; the display name and the short alias are
/// added later from the library list, which is data and may not know a code. `$c` is
/// always `KOBV` and `$d` is a property of the delivering source system, constant per
/// ISIL — neither says anything about a copy, and both are ignored. `summary` and `items`
/// stay empty until availability has been fetched, which is a separate call.
///
/// A `924` without `$b` names no library at all: it cannot be resolved, cannot be asked
/// about and would render as a blank line, so it is skipped. Every one of the 1776 fields
/// in the sample has all four subfields, so this is a guard, not a case.
fn holdings(marc: &MarcRecord) -> Vec<Holding> {
    marc.fields("924")
        .filter_map(|field| {
            let isil = field.sub('b').filter(|value| !value.is_empty())?;
            Some(Holding {
                isil: Some(Isil::new(isil)),
                alias: None,
                library: isil.to_owned(),
                short_name: None,
                local_id: field
                    .sub('a')
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                mine: false,
                summary: Status::Unknown,
                items: Vec::new(),
            })
        })
        .collect()
}

/// Drop trailing ISBD punctuation from a display value.
fn trimmed(value: &str, marks: &[char]) -> String {
    value
        .trim_end_matches(|character| marks.contains(&character))
        .to_owned()
}

/// `None` for an empty string, so that an emptied field reads as "not stated" rather than
/// as a blank line in the output.
fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::super::marc::fixture;
    use super::*;

    /// Convert one fixture record, or fail the test naming it.
    fn convert(file: &str, id: &str) -> Record {
        let marc = fixture::record(file, id);
        from_marc(&marc).unwrap_or_else(|error| panic!("{id} converts: {error}"))
    }

    /// The reference record used throughout `plan/`: every field populated, ISBD
    /// punctuation on title, subtitle, place and publisher.
    #[test]
    fn the_reference_record_maps_every_field() {
        let record = convert("record.xml", "almafu_BV008885798");
        assert_eq!(record.id.as_str(), "almafu_BV008885798");
        assert_eq!(record.title, "Augenblick und Irritation");
        assert_eq!(
            record.subtitle.as_deref(),
            Some("Figuren der Zeit in Kafkas \"Prozess\"")
        );
        assert_eq!(record.year, Some(1994));
        assert_eq!(record.place.as_deref(), Some("Münster [u.a.]"));
        assert_eq!(record.publisher.as_deref(), Some("Lit"));
        assert_eq!(record.extent.as_deref(), Some("293 S."));
        assert_eq!(record.languages, ["ger"]);
        assert_eq!(record.format, Format::Book);
        assert!(!record.online);
        assert_eq!(record.isbns, ["3-89473-790-5"]);
        assert_eq!(record.authors.len(), 1);
        assert_eq!(record.authors[0].name, "Kleinschmidt, Klaus");
        assert_eq!(record.authors[0].kind, AuthorKind::Person);
    }

    /// Every `924` becomes a holding; the availability key deduplicates the ISIL that
    /// occurs twice and keeps the library's own local id, which differs from `001` here.
    #[test]
    fn the_availability_key_deduplicates_isils_and_keeps_field_order() {
        let record = convert("record.xml", "almafu_BV008885798");
        assert_eq!(record.holdings.len(), 6);
        let key = availability_id(&record.holdings).expect("the record has holdings");
        assert_eq!(
            key.as_str(),
            "DE-188;BV008885798,DE-609;12349009,DE-11;BV008885798,DE-521;BV008885798,\
             DE-1;275177939,"
        );
    }

    /// The five-holding case from the test list, all ISILs distinct.
    #[test]
    fn five_holdings_yield_five_pairs() {
        let record = convert("mono_kafka.xml", "almahu_BV019771323");
        let key = availability_id(&record.holdings).expect("the record has holdings");
        assert_eq!(
            key.as_str(),
            "DE-11;BV019771323,DE-188;BV019771323,DE-521;BV019771323,DE-1;48035670X,\
             DE-517;48035670X,"
        );
    }

    /// Six identical `924` fields, one ISIL: the key must name it once, while the
    /// holdings list stays as the record has it.
    #[test]
    fn a_repeated_isil_appears_once_in_the_key() {
        let record = convert("av_dvd.xml", "gbv_519092074");
        assert_eq!(record.holdings.len(), 6);
        let key = availability_id(&record.holdings).expect("the record has holdings");
        assert_eq!(key.as_str(), "DE-B11;519092074,");
    }

    /// 4.9 % of records state no holdings. That is "not stated here", and there is
    /// nothing to ask the availability service about.
    #[test]
    fn no_holdings_means_no_availability_call() {
        assert_eq!(availability_id(&[]), None);
    }

    /// The 34-character `008`: year at 01–04, language at 29–31. Read naively, this
    /// record has no year and a language of `"  o"`.
    #[test]
    fn a_shifted_008_still_yields_year_and_language() {
        let record = convert("festschrift.xml", "almahu_9950038349602882");
        assert_eq!(record.year, Some(2002));
        assert_eq!(record.languages, ["ger"]);
        assert!(record.online, "007 is `cr uuu---uuuuu`");
        assert_eq!(record.format, Format::Ebook, "an e-book, not a book");
    }

    /// The first `245` of this record carries only `$h Musikdruck`; taking "the first
    /// `245`" would leave the record untitled.
    #[test]
    fn the_title_comes_from_the_first_245_that_has_a_subfield_a() {
        let record = convert("score.xml", "kobvindex_SLB24672");
        assert_eq!(record.title, "Sei Lob und Ehr dem höchsten Gut");
        assert!(
            record
                .subtitle
                .as_deref()
                .is_some_and(|subtitle| subtitle.starts_with("BWV 117")),
            "the subtitle comes from the same field"
        );
    }

    /// A leader whose record-length field has slipped reads `2`/`0` at 06/07. That is not
    /// a format and must not become a guess — the record itself is fine and is kept.
    #[test]
    fn a_shifted_leader_is_an_unknown_format_not_a_dropped_record() {
        let record = convert("festschrift.xml", "kobvindex_LDAz01080");
        assert_eq!(record.format, Format::Unknown);
        assert_eq!(
            record.title,
            "Zeitschrift des Historischen Vereins für Steiermark"
        );
        assert_eq!(record.holdings.len(), 1);
    }

    /// `008/35-37` is `|||` here and `041$a` is `ger`: the union rescues the language.
    /// The record also carries `245$n` twice and XML comments inside the record element.
    #[test]
    fn a_broken_008_is_rescued_by_041() {
        let record = convert("newspaper.xml", "kobvindex_SLB42822");
        assert_eq!(record.languages, ["ger"]);
        assert_eq!(
            record.year, None,
            "008 says `||||` and there is no 264/260 $c"
        );
        assert_eq!(
            record.title,
            "Märkische Allgemeine [Potsdamer Tageszeitung : PDM]. 50.1995,Februar. PDM"
        );
    }

    /// `008` names the first language, `041` the rest.
    #[test]
    fn a_multilingual_record_unions_008_and_041() {
        let record = convert("arabic.xml", "gbv_1603494723");
        assert_eq!(record.languages, ["tur", "eng", "tam"]);
    }

    /// `020$9` is a hyphenated KOBV duplicate of `$a`; taking it would list every ISBN
    /// twice.
    #[test]
    fn only_020_subfield_a_becomes_an_isbn() {
        let record = convert("arabic.xml", "gbv_1603494723");
        assert_eq!(record.isbns, ["3700101198"]);
    }

    /// The union deduplicates: this record names `Kairo` and `Geschichte 1599` in two
    /// separate RSWK chains, and the chain-closing `689` carries only `$5`.
    #[test]
    fn subjects_are_the_deduplicated_union_of_650_689_and_653() {
        let record = convert("arabic.xml", "gbv_1603494723");
        assert_eq!(
            record.subjects,
            [
                "Jahrhundert, 16. / Geistes- und Kulturleben",
                "Kairo",
                "Geschichte 1599",
                "Osmanisches Reich",
                "Cairo (Egypt)",
                "Description and travel",
                "Egypt",
                "History",
                "1517-1882",
                "Âlî, Mustafa bin Ahmet",
                "1541-1599",
            ]
        );
    }

    /// The two C1 controls are the only place the non-sorting article is marked here —
    /// `245 ind2` is `0`. They must not reach the terminal, and `Der` must stay.
    #[test]
    fn the_c1_sort_controls_never_reach_the_title() {
        let record = convert("mono_kafka.xml", "b3kat_BV044513648");
        assert_eq!(record.title, "Der Prozess");
        assert_eq!(record.format, Format::Video);
    }

    /// `$n` and `$p` interleave; the subfield order is the reading order.
    #[test]
    fn volume_designations_are_appended_in_subfield_order() {
        let record = convert("av_dvd.xml", "gbv_519092074");
        assert_eq!(record.title, "Summa artis. Vol. 49. Museos de España. 2");
        assert_eq!(
            record.subtitle.as_deref(),
            Some("historia general del arte")
        );
    }

    /// Twelve `264` fields, one per change of publisher over a century. The first usable
    /// one wins; the rest have nowhere to go.
    #[test]
    fn the_first_usable_264_wins() {
        let record = convert("journal2.xml", "gbv_130857440");
        assert_eq!(record.place.as_deref(), Some("[Linz]"));
        assert_eq!(
            record.publisher.as_deref(),
            Some("im Selbstverlag der Anstalt")
        );
    }

    /// This record has no `264` at all; `260` carries two `$a` and two `$b`.
    #[test]
    fn a_record_without_264_falls_back_to_260() {
        let record = convert("ebook2.xml", "kobvindex_ZLB15770715");
        assert_eq!(record.place.as_deref(), Some("Saarbrücken"));
        assert_eq!(record.publisher.as_deref(), Some("juris GmbH"));
    }

    /// Leader/06 = `m` with `007 = cr`: an electronic resource that is online. The
    /// `e`-variant is not invented for it; `online` carries that instead.
    #[test]
    fn an_electronic_leader_with_007_cr_is_electronic_and_online() {
        let record = convert("ebook2.xml", "kobvindex_ZLB15770715");
        assert_eq!(record.format, Format::Electronic);
        assert!(record.online);
        assert_eq!(record.title, "Reichsgesetzblatt. Teil 1, Inneres");
    }

    /// Seven name-title added entries for one person, each with the GND of a *different
    /// work*. Using that as the person's identifier would make each one a separate human.
    #[test]
    fn identical_name_title_entries_are_deduplicated() {
        let record = convert("nonlatin_ru.xml", "almahu_BV010644426");
        let dostoevskij: Vec<&Author> = record
            .authors
            .iter()
            .filter(|author| author.name.starts_with("Dostoevskij"))
            .collect();
        assert_eq!(
            dostoevskij.len(),
            3,
            "the 100 plus two spellings of the added entry, not eight entries: {:?}",
            record.authors
        );
        assert!(
            dostoevskij
                .iter()
                .filter(|author| author.gnd.is_none())
                .count()
                == 2,
            "a name-title entry contributes no GND"
        );
        assert_eq!(
            dostoevskij[0].gnd.as_deref(),
            Some("118527053"),
            "the 100 keeps the GND of the person"
        );
        assert_eq!(dostoevskij[0].dates.as_deref(), Some("1821-1881"));
    }

    /// `$t` is dropped and the person kept: two added entries for the same translator's
    /// two works collapse into one author.
    #[test]
    fn a_name_title_entry_keeps_the_person_and_drops_the_work() {
        let record = convert("arabic.xml", "gbv_660841622");
        assert_eq!(record.authors.len(), 3, "{:?}", record.authors);
        assert_eq!(record.authors[0].name, "Maḥfūẓ, Naǧīb");
        assert_eq!(record.authors[0].gnd.as_deref(), Some("118576259"));
        assert_eq!(record.authors[0].role.as_deref(), Some("aut"));
        assert_eq!(record.authors[1].name, "Fähndrich, Hartmut");
        assert_eq!(record.authors[1].dates.as_deref(), Some("1944-"));
        assert_eq!(record.authors[1].role.as_deref(), Some("trl"));
    }

    /// `689` chains skip their `$5`-only closing link instead of emitting an empty
    /// heading.
    #[test]
    fn a_chain_link_without_a_subfield_a_is_not_an_empty_subject() {
        let record = convert("arabic.xml", "gbv_660841622");
        assert_eq!(
            record.subjects,
            [
                "Kairo",
                "Student",
                "Stellensuche",
                "Kompromiss",
                "Lebenskrise",
                "Geschichte 1931",
            ]
        );
    }

    /// A corporate body is the creator of this record; `$b` and `$g` belong to the name,
    /// and the main entry comes before the two added ones.
    #[test]
    fn a_corporate_author_joins_a_b_and_g() {
        let record = convert("arabic.xml", "gbv_014140950");
        assert_eq!(record.authors.len(), 3, "{:?}", record.authors);
        assert_eq!(
            record.authors[0].name,
            "Kanīsat as-Saiyida al-ʿAḏrā', Maktaba, al-Qāhira"
        );
        assert_eq!(record.authors[0].kind, AuthorKind::Corporate);
        assert_eq!(
            record.authors[0].dates, None,
            "$d on a body is not life dates"
        );
        assert_eq!(record.authors[0].gnd.as_deref(), Some("5051414-3"));
        assert_eq!(record.authors[0].role.as_deref(), Some("aut"));
    }

    /// The doubled-angle-bracket form of the non-sorting mark, here around a nobiliary
    /// particle that belongs to the name.
    #[test]
    fn angle_brackets_are_gone_from_an_author_name() {
        let record = convert("oldprint.xml", "kobvindex_MFN19797");
        assert_eq!(record.authors[0].name, "Buffon, Georges Louis Le Clerc de");
        assert_eq!(record.authors[0].gnd.as_deref(), Some("118517252"));
        assert_eq!(record.authors[0].role, None, "neither $4 nor $e");
    }

    /// `264$c` is `1413 [1992]` — a Hijri year followed by the Gregorian one. The regular
    /// expression would take 1413; `008` says 1992, and `008` goes first.
    #[test]
    fn the_008_beats_a_foreign_calendar_in_the_imprint() {
        let record = convert("arabic.xml", "almafu_BV026062583");
        assert_eq!(record.year, Some(1992));
        assert_eq!(first_year("1413 [1992]"), Some(1413), "the fallback alone");
    }

    /// The Latin transliteration lives in the regular field and the original script in
    /// the `880`. Counting both would print every title twice.
    #[test]
    fn an_alternate_script_record_has_one_title_and_it_is_the_transliteration() {
        let record = convert("arabic.xml", "almafu_BV026062583");
        assert!(record.title.starts_with("Miṣr baina"), "{}", record.title);
        assert!(
            !record
                .title
                .chars()
                .any(|c| ('\u{600}'..'\u{700}').contains(&c)),
            "no Arabic script in the title: {}",
            record.title
        );
        assert_eq!(record.place.as_deref(), Some("Al-Qāhira"));

        let hebrew = convert("nonlatin_he.xml", "gbv_1944932879");
        assert!(
            !hebrew
                .title
                .chars()
                .any(|c| ('\u{590}'..'\u{600}').contains(&c)),
            "no Hebrew script in the title: {}",
            hebrew.title
        );
        assert_eq!(hebrew.year, Some(1973), "008, not the Hebrew calendar year");
    }

    /// The label decides what a link is, never `ind2`.
    #[test]
    fn links_are_classified_by_their_label() {
        let ebook = convert("ebook.xml", "b3kat_BV035602531");
        assert!(
            ebook.urls.iter().any(
                |url| url.kind == UrlKind::Fulltext && url.label.as_deref() == Some("Volltext")
            ),
            "{:?}",
            ebook.urls
        );

        let print = convert("mono_kafka.xml", "almahu_BV019771323");
        assert!(
            print.urls.iter().any(|url| url.kind == UrlKind::Toc
                && url.label.as_deref() == Some("Inhaltsverzeichnis")),
            "{:?}",
            print.urls
        );
        assert!(
            print.urls.iter().any(
                |url| url.kind == UrlKind::Other && url.label.as_deref() == Some("Klappentext")
            ),
            "an unrecognised label is never guessed at"
        );
    }

    #[test]
    fn an_unlabelled_link_is_other_not_a_guess() {
        assert_eq!(url_kind(None), UrlKind::Other);
        assert_eq!(url_kind(Some("Volltext")), UrlKind::Fulltext);
        assert_eq!(url_kind(Some("FULL")), UrlKind::Fulltext);
        assert_eq!(url_kind(Some("Table of Contents")), UrlKind::Toc);
        assert_eq!(url_kind(Some("Cover")), UrlKind::Cover);
        assert_eq!(url_kind(Some("Verlag")), UrlKind::Publisher);
        assert_eq!(url_kind(Some("Inhaltsbeschreibung")), UrlKind::Other);
    }

    /// `PublicationDate: 20231231` is not the year 2023.
    #[test]
    fn only_a_group_of_exactly_four_digits_is_a_year() {
        assert_eq!(first_year("20231231"), None);
        assert_eq!(first_year("[ca. 1990]"), Some(1990));
        assert_eq!(first_year("© 2019"), Some(2019));
        assert_eq!(first_year("19XX-"), None);
        assert_eq!(
            first_year("5740- [1979/1980-]"),
            None,
            "implausible, not 5740"
        );
        assert_eq!(
            first_year("Le 27 Nivôse An VIII. [17. Januar 1800]"),
            Some(1800)
        );
    }

    /// Only the identity can fail. A record without `001` cannot be shown or asked about.
    #[test]
    fn a_record_without_001_is_an_error_naming_the_field() {
        let xml = r#"<record xmlns="http://www.loc.gov/MARC21/slim">
              <leader>00817cam a2200277   4500</leader>
              <datafield tag="245" ind1="1" ind2="0">
                <subfield code="a">Titel ohne Nummer</subfield>
              </datafield>
            </record>"#;
        let document = roxmltree::Document::parse(xml).expect("the snippet is XML");
        let marc = MarcRecord::from_node(document.root_element());
        let error = from_marc(&marc).expect_err("no 001, no identity");
        assert!(
            error.to_string().contains("controlfield 001"),
            "the error names the field: {error}"
        );
    }

    /// The prefix is part of the identity; without it the record cannot be routed to an
    /// engine.
    #[test]
    fn a_001_without_a_source_prefix_is_an_error() {
        let xml = r#"<record xmlns="http://www.loc.gov/MARC21/slim">
              <controlfield tag="001">BV008885798</controlfield>
            </record>"#;
        let document = roxmltree::Document::parse(xml).expect("the snippet is XML");
        let marc = MarcRecord::from_node(document.root_element());
        let error = from_marc(&marc).expect_err("no source prefix, no engine");
        assert!(
            error.to_string().contains("BV008885798"),
            "the error quotes the id: {error}"
        );
    }

    /// Every record in every saved response converts. The surrogate diagnostic in
    /// `kids.xml` is not a `<record>` and never reaches this function.
    #[test]
    fn every_fixture_record_converts() {
        let records = fixture::all_records();
        assert_eq!(records.len(), 41);
        for (file, marc) in records {
            let id = marc.control("001").unwrap_or("<no 001>").to_owned();
            let record =
                from_marc(&marc).unwrap_or_else(|error| panic!("{id} in {file} converts: {error}"));
            assert!(!record.title.is_empty(), "{id} in {file} has a title");
            assert_eq!(record.id.as_str(), id);
        }
    }
}
