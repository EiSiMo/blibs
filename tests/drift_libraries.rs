//! `data/libraries.json` against the two services it was built from.
//!
//! The library list is data, not code (`CLAUDE.md`), and data goes stale silently:
//! institutions move, rename themselves, lose a branch or gain one, and nothing in the
//! tool would ever notice. This test is the thing that notices. It fetches the two
//! upstream sources named in `plan/libraries.md` §1 — lobid-organisations for the
//! institutions, the KOBV library directory (`bibinfo.kobv.de`) for the KOBV ids and the
//! branches — re-derives every field that has an upstream source, and fails with a
//! grouped diff when the file and the services disagree.
//!
//! **It is deliberately not `#[ignore]`d.** An ignored drift check is a check nobody
//! runs: the file makes a claim about the world, and the claim has to be re-examined
//! without anyone remembering to do it. The cost is bounded by the cache — every request
//! here is [`CachePolicy::Normal`] against `Http::new(true)`, and
//! `blibs::http::cache::ENTRY_TTL` is 24 hours, so a machine pays for at most one real
//! round trip per day and every other `cargo test` reads three files off the disk. (The
//! `#[ignore]`d file next door, `tests/capture_voebb.rs`, is a fixture *tool* and not a
//! check; it stays ignored for that reason.)
//!
//! Exactly one failure is tolerated: **the host is not reachable**, i.e.
//! [`NetworkError::Transport`] — no DNS, no route, refused connection. A laptop on a
//! train must not turn the whole suite red, or the suite stops being trusted. Everything
//! else stays red on purpose: a 429 or 503, any other HTTP status, a body that does not
//! parse, and a body that parses but is not plausible (no `member`, `totalItems` of
//! zero, a directory that shrank to a handful of entries). That is the rule from
//! `CLAUDE.md` — a missing structure is never turned into an empty result, and here an
//! empty result would read as "no drift", which is the one answer this test must never
//! give by accident.
//!
//! Three fields are **not** compared, because no upstream source states them
//! (`plan/libraries.md` §1, §5, §8.3): `aliases` and `short_name` are hand-curated by the
//! rules in §5, and `portal_name` is a parser key observed in the KOBV portal's
//! availability fragment, not something lobid or bibinfo knows. `match[]` is likewise
//! observed, not derived. Their invariants live in `tests/libraries.rs`, offline.
//!
//! Nor can this test see an institution that ought to be *added*: the lobid query asks
//! for the 123 ISILs the file already carries, and the bibinfo directory contains 313
//! further houses that were deliberately not adopted (`plan/libraries.md` §8.2, §11).
//! Widening the list is a decision, not drift.
//!
//! ```text
//! cargo test --test drift_libraries -- --nocapture
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use blibs::error::{Error, NetworkError};
use blibs::http::{CachePolicy, Fetch, Http, Request};
use blibs::libraries::data::{Branch, LIBRARIES_JSON, Library};
use blibs::libraries::geo::LatLon;

/// The collective lobid query. One request for all 123 ISILs (`plan/drift.md`).
const LOBID_SEARCH: &str = "https://lobid.org/organisations/search";

/// The KOBV library directory: institutions, branches, coordinates, KOBV ids.
const BIBINFO_LIBRARIES: &str = "https://bibinfo.kobv.de/json/libraries.json";

/// The directory's controlled vocabulary. Only one code is read from it, and reading it
/// is what proves that code still means what the branch rule assumes.
const BIBINFO_TERMS: &str = "https://bibinfo.kobv.de/json/terms.json";

/// The group code that makes a directory entry a house of the public network
/// (`plan/libraries.md` §8.2, rule 4). The VÖBB has no ISIL hierarchy, so this code is
/// the only thing that ties its 98 houses to `DE-609`.
const VOEBB_GROUP: &str = "gVOEBB";

/// What `terms.json` resolves [`VOEBB_GROUP`] to. Quoted in `plan/libraries.md` §8.2; if
/// the label changes, the anchor of rule 4 has moved and wants looking at.
const VOEBB_GROUP_LABEL: &str = "Öffentliche Bibliotheken Berlin (VÖBB)";

/// How far a coordinate may drift before it counts as a different place.
///
/// 50 m. The file states five decimals (≈1 m), and today the *worst* of the 333
/// comparisons is 0.65 m — the sources restate the same measurement rather than
/// re-geocoding it, so there is no jitter to absorb and the threshold has a 75× margin.
/// `plan/libraries.md` §8.4 used 300 m for a different question ("is bibinfo better than
/// lobid, should we overwrite?"); this one only has to *notice*, so it is tighter. A real
/// relocation is hundreds of metres at least.
const COORDINATE_TOLERANCE_KM: f64 = 0.05;

/// The smallest directory that is still plausibly the KOBV directory.
///
/// It held 645 entries on 2026-09-04 and on 2026-09-07. A drastically shorter answer is a
/// broken service, not a network that lost 90 % of its libraries, and must not be read as
/// "every branch disappeared".
const MIN_DIRECTORY_ENTRIES: usize = 500;

// ---------------------------------------------------------------------------
// The deliberate deviations
// ---------------------------------------------------------------------------

/// One difference between the file and its sources that is **intended**.
///
/// Checked in both directions. A difference not listed here fails the test, and an entry
/// here that the run no longer observes fails it too — a table of exceptions that is
/// allowed to rot makes the test worthless, so a stale entry is as loud as new drift.
struct Exception {
    /// ISIL of an institution, or `kobvid` of a directory entry.
    key: &'static str,
    /// The field the deviation is reported under. Matches [`Deviation::field`].
    field: &'static str,
    /// Why the file is right and the source is not, in plain words.
    reason: &'static str,
}

/// Every deviation the file makes on purpose, from `plan/libraries.md` §8.4 and from the
/// first real run of this test (2026-09-07).
static EXCEPTIONS: &[Exception] = &[
    Exception {
        key: "DE-578",
        field: "record",
        reason: "the Charité's collective ISIL has no lobid record and no bibinfo main \
                 entry — only its four campus libraries exist, and those are its branches \
                 (plan/libraries.md §8.4). Nothing in this row has an upstream source, so \
                 none of its fields is compared",
    },
    Exception {
        key: "DE-609",
        field: "name",
        reason: "lobid writes 'Verbund der  Öffentlichen Bibliotheken Berlins' with a \
                 double space; the file normalises it. Cosmetic, and the name is printed",
    },
    Exception {
        key: "DE-109",
        field: "kobvid",
        reason: "bibinfo carries two entries under DE-109 — the AGB (SIG00036) and the \
                 Berliner Stadtbibliothek (BIB000000072) — so there is no single main \
                 entry. Both hang under DE-609, where their holdings are reported \
                 (plan/libraries.md §3, §8.4), and DE-109 itself keeps kobvid null",
    },
    Exception {
        key: "SIG00036",
        field: "parent",
        reason: "the Amerika-Gedenkbibliothek: bibinfo files it under DE-109, the file \
                 under DE-609, because DE-109 never appears in a MARC 924 and DE-609 \
                 does (plan/libraries.md §3)",
    },
    Exception {
        key: "BIB000000072",
        field: "parent",
        reason: "the Berliner Stadtbibliothek — same as SIG00036, same reason",
    },
    Exception {
        key: "BIB000000252",
        field: "branch",
        reason: "'Fahrbibliothek Treptow-Köpenick Kleiner Bus' is entered twice in the \
                 directory, as BIB000000252 and BIB000000372 with identical name and \
                 coordinates and no ISIL on either. The file keeps one of the two; \
                 tests/libraries.rs guards against the pair coming back",
    },
    // §8.2 rule 5, the name-prefix cases: a directory entry with no ISIL and no group
    // code, tied to its house by its name. Nine entries, each checked by hand — the rule
    // is not mechanical enough to re-run here (one of them misspells the very name it
    // would have to match), so each one is named instead.
    Exception {
        key: "BIB000000236",
        field: "parent",
        reason: "'Stadtbibliothek Pankow / Museum Pankow': no ISIL and not in the VÖBB \
                 group, tied to DE-609 by its name prefix (plan/libraries.md §8.2 rule 5)",
    },
    Exception {
        key: "BIB000000307",
        field: "parent",
        reason: "'Universität Potsdam, Universitätsbibliothek Bereichsbibliothek Neues \
                 Palais': no ISIL, tied to DE-517 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "BIB000000308",
        field: "parent",
        reason: "'Universität Potsdam, Universitätsbibliothek, Bereichsbibliothek \
                 Babelsberg': no ISIL, tied to DE-517 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "BIB000000316",
        field: "parent",
        reason: "'Stadt- und Landesbibliothek Potsdam, Zweigbibliothek Waldstadt': no \
                 ISIL, tied to DE-186 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "BIB000000317",
        field: "parent",
        reason: "'Stadt- und Landesbilbiblothek Potsdam, Zweigbibliothek Am Stern': no \
                 ISIL, tied to DE-186 by its name prefix — and bibinfo misspells the \
                 house name in this one entry, so no prefix rule could reach it (§8.2 \
                 rule 5)",
    },
    Exception {
        key: "BIB000000359",
        field: "parent",
        reason: "'Europa-Universität Viadrina, Universitätsbibliothek, Selbstlernzentrum': \
                 no ISIL, tied to DE-521 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "BIB000000412",
        field: "parent",
        reason: "'BTU Cottbus - Senftenberg, …, Campus Cottbus Sachsendorf': no ISIL, \
                 tied to DE-634 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "BIB000000413",
        field: "parent",
        reason: "'BTU Cottbus - Senftenberg, …, Campus Senftenberg': no ISIL, tied to \
                 DE-634 by its name prefix (§8.2 rule 5)",
    },
    Exception {
        key: "HUB00068",
        field: "parent",
        reason: "'HU Berlin, …, Zweigbibliothek Asien- und Afrikawissenschaften, \
                 Teilbibliothek Japanzentrum': no ISIL, tied to DE-11 by its name prefix \
                 (§8.2 rule 5)",
    },
];

// ---------------------------------------------------------------------------
// Upstream shapes
// ---------------------------------------------------------------------------

/// A JSON scalar that the KOBV directory spells inconsistently.
///
/// `lat`/`lng` arrive as a number for 571 entries and as a string for 74, one of which
/// uses a German decimal comma (`"52,526128"`); `postalCode` arrives as a number unless
/// it has a leading zero. Modelling it loosely here is not sloppiness — rejecting the
/// string form would report a hundred phantom coordinate drifts.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum Scalar {
    /// A whole number, e.g. a postal code without a leading zero.
    Int(i64),
    /// A decimal, e.g. a coordinate.
    Float(f64),
    /// The same value written as text.
    Text(String),
}

impl Scalar {
    /// The value as a number, accepting the decimal comma. `None` when the text is not a
    /// number at all — which is drift, not something to paper over.
    fn as_f64(&self) -> Option<f64> {
        match self {
            #[expect(
                clippy::cast_precision_loss,
                reason = "coordinates and postal codes, never near 2^53"
            )]
            Scalar::Int(value) => Some(*value as f64),
            Scalar::Float(value) => Some(*value),
            Scalar::Text(text) => text.trim().replace(',', ".").parse().ok(),
        }
    }

    /// The value as it would be written into an address line.
    fn as_text(&self) -> String {
        match self {
            Scalar::Int(value) => value.to_string(),
            Scalar::Float(value) => value.to_string(),
            Scalar::Text(text) => text.clone(),
        }
    }
}

/// The envelope of a lobid search.
#[derive(Debug, serde::Deserialize)]
struct LobidResponse {
    /// How many organisations matched, before `size` truncated the answer.
    #[serde(rename = "totalItems")]
    total_items: usize,
    /// The organisations themselves.
    member: Vec<LobidOrg>,
}

/// One organisation as lobid describes it.
#[derive(Debug, serde::Deserialize)]
struct LobidOrg {
    /// The ISIL, which is what the file is keyed by.
    isil: String,
    /// The full official name.
    name: Option<String>,
    /// The library type, wrapped in a SKOS concept.
    classification: Option<LobidClassification>,
    /// Places, plural: five of the 122 organisations list more than one.
    #[serde(default)]
    location: Vec<LobidPlace>,
    /// Homepage.
    url: Option<String>,
    /// Catalogue, as a `provides` relation.
    provides: Option<String>,
    /// Contact address, always `mailto:`-prefixed.
    email: Option<String>,
    /// Contact number.
    telephone: Option<String>,
}

/// The `classification` concept; only its German label is a display value.
#[derive(Debug, serde::Deserialize)]
struct LobidClassification {
    /// Labels by language tag.
    label: BTreeMap<String, String>,
}

/// One place an organisation sits at.
#[derive(Debug, serde::Deserialize)]
struct LobidPlace {
    /// The postal address.
    address: Option<LobidAddress>,
    /// The coordinates, given as strings.
    geo: Option<LobidGeo>,
}

/// A postal address as lobid splits it up.
#[derive(Debug, serde::Deserialize)]
struct LobidAddress {
    /// Street and number.
    #[serde(rename = "streetAddress")]
    street_address: Option<String>,
    /// Postal code.
    #[serde(rename = "postalCode")]
    postal_code: Option<String>,
    /// City — also the file's `city`.
    #[serde(rename = "addressLocality")]
    address_locality: Option<String>,
}

/// Coordinates, as decimal degrees in strings.
#[derive(Debug, serde::Deserialize)]
struct LobidGeo {
    /// Latitude.
    lat: String,
    /// Longitude.
    lon: String,
}

/// One entry of the KOBV library directory.
#[derive(Debug, serde::Deserialize)]
struct BibEntry {
    /// The directory's own key, which is also the file's `kobvid`.
    kobvid: String,
    /// The ISIL, where the entry has one — 550 of 645 do.
    isil: Option<String>,
    /// The full name.
    name: String,
    /// Latitude.
    lat: Scalar,
    /// Longitude, spelled `lng` here and `lon` everywhere else.
    lng: Scalar,
    /// Street address, contact details and city, in one object.
    address: Option<BibAddress>,
    /// Homepage. Fills `url` where lobid has none.
    #[serde(rename = "homeUrl")]
    home_url: Option<String>,
    /// Catalogue. Fills `opac` where lobid has none.
    #[serde(rename = "opacUrl")]
    opac_url: Option<String>,
    /// Group, region and subject codes; [`VOEBB_GROUP`] is read from here.
    #[serde(default)]
    terms: Vec<String>,
}

/// The address block of a directory entry.
#[derive(Debug, serde::Deserialize)]
struct BibAddress {
    /// Street and number. Absent for the mobile libraries, whose `place` says so.
    street: Option<String>,
    /// Postal code, number or string.
    #[serde(rename = "postalCode")]
    postal_code: Option<Scalar>,
    /// City.
    place: Option<String>,
    /// Contact address. Fills `email` where lobid has none.
    email: Option<String>,
}

/// One code of the directory's vocabulary.
#[derive(Debug, serde::Deserialize)]
struct BibTerm {
    /// The human-readable label.
    label: String,
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

/// Why the comparison could not be made.
///
/// Two variants because they have opposite consequences, and the split is the whole point
/// of running this test unattended: [`Failure::Offline`] is skipped loudly, everything
/// else fails the suite.
enum Failure {
    /// The host could not be reached at all. The list was not checked this run.
    Offline(String),
    /// Anything else — a status, a body that will not parse, a body that is not
    /// plausible. Fails the test, because it means the sources changed shape.
    Fatal(String),
}

/// Fetch one JSON document through the crate's only I/O seam and deserialise it.
///
/// The cache policy is deliberately [`CachePolicy::Normal`]: this is the one thing that
/// keeps an unattended live test to a single round trip per host per day.
fn fetch_json<T: serde::de::DeserializeOwned>(
    http: &Http,
    request: &Request,
    what: &str,
) -> Result<T, Failure> {
    let response = match http.fetch(request) {
        Ok(response) => response,
        Err(Error::Network(transport @ NetworkError::Transport { .. })) => {
            return Err(Failure::Offline(transport.to_string()));
        }
        Err(other) => {
            return Err(Failure::Fatal(format!(
                "{what}: {} ({})\n  {}",
                other,
                request.url(),
                other.hint().unwrap_or_else(|| "no hint".to_string())
            )));
        }
    };
    serde_json::from_str(&response.body).map_err(|err| {
        Failure::Fatal(format!(
            "{what}: the answer did not deserialise: {err}\n  {}\n  {} bytes, \
             content-type {:?}, from cache: {}",
            request.url(),
            response.body.len(),
            response.content_type,
            response.from_cache
        ))
    })
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// The collective lobid query for exactly the ISILs the file carries.
fn lobid_request(libraries: &[Library]) -> Request {
    let terms: Vec<String> = libraries
        .iter()
        .map(|library| format!("\"{}\"", library.isil))
        .collect();
    Request::get(LOBID_SEARCH)
        .query("q", format!("isil:({})", terms.join(" OR ")))
        .query("format", "json")
        // 200 > 123, so `totalItems` and `member.len()` must agree; when they stop
        // agreeing the answer was truncated and the comparison is not valid.
        .query("size", "200")
        .cache(CachePolicy::Normal)
}

/// The lobid records, keyed by ISIL.
///
/// Fails rather than returning an empty map when the answer is not plausible: zero hits
/// for 123 known ISILs is a broken query or a broken service, never a region that lost
/// all its libraries.
fn fetch_lobid(http: &Http, libraries: &[Library]) -> Result<BTreeMap<String, LobidOrg>, Failure> {
    let request = lobid_request(libraries);
    let response: LobidResponse = fetch_json(http, &request, "lobid-organisations")?;
    if response.total_items == 0 || response.member.is_empty() {
        return Err(Failure::Fatal(format!(
            "lobid-organisations answered with totalItems={} and {} members for {} ISILs \
             — that is a broken query or a broken service, not an empty region\n  {}",
            response.total_items,
            response.member.len(),
            libraries.len(),
            request.url()
        )));
    }
    if response.member.len() < response.total_items {
        return Err(Failure::Fatal(format!(
            "lobid-organisations truncated its answer: totalItems={}, {} members returned \
             — raise `size` before trusting any of this",
            response.total_items,
            response.member.len()
        )));
    }
    Ok(response
        .member
        .into_iter()
        .map(|org| (org.isil.clone(), org))
        .collect())
}

/// The KOBV library directory, keyed by `kobvid`.
fn fetch_bibinfo(http: &Http) -> Result<BTreeMap<String, BibEntry>, Failure> {
    let request = Request::get(BIBINFO_LIBRARIES).cache(CachePolicy::Normal);
    let entries: BTreeMap<String, BibEntry> = fetch_json(http, &request, "bibinfo libraries")?;
    if entries.len() < MIN_DIRECTORY_ENTRIES {
        return Err(Failure::Fatal(format!(
            "the KOBV directory answered with {} entries, at least {MIN_DIRECTORY_ENTRIES} \
             were expected — treating that as 'every branch is gone' would be the worst \
             possible reading\n  {}",
            entries.len(),
            request.url()
        )));
    }
    Ok(entries)
}

/// The directory's vocabulary, keyed by code.
///
/// Only [`VOEBB_GROUP`] is read, and its absence is fatal: rule 4 of `plan/libraries.md`
/// §8.2 hangs 98 houses off that one code, and a silent miss would delete them all.
fn fetch_terms(http: &Http) -> Result<BTreeMap<String, BibTerm>, Failure> {
    let request = Request::get(BIBINFO_TERMS).cache(CachePolicy::Normal);
    let terms: BTreeMap<String, BibTerm> = fetch_json(http, &request, "bibinfo terms")?;
    if !terms.contains_key(VOEBB_GROUP) {
        return Err(Failure::Fatal(format!(
            "the KOBV vocabulary no longer knows {VOEBB_GROUP}, which is the only thing \
             that ties the 98 VÖBB houses to DE-609 (plan/libraries.md §8.2 rule 4)\n  {}",
            request.url()
        )));
    }
    Ok(terms)
}

// ---------------------------------------------------------------------------
// Deviations
// ---------------------------------------------------------------------------

/// Which part of the report a deviation belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// An institution the file carries that no source describes any more.
    MissingInstitution,
    /// A record came back under an ISIL the file does not carry.
    NewInstitution,
    /// A field of an institution or a branch that no longer matches its source.
    ChangedField,
    /// A branch the file carries that the directory no longer places here.
    MissingBranch,
    /// A branch the directory places at one of our houses that the file does not carry.
    NewBranch,
}

impl Group {
    /// The heading this group is printed under.
    fn heading(self) -> &'static str {
        match self {
            Group::MissingInstitution => "institutions with no upstream record",
            Group::NewInstitution => "records under an unknown ISIL",
            Group::ChangedField => "changed fields",
            Group::MissingBranch => "branches the directory no longer places here",
            Group::NewBranch => "branches the directory places here that the file lacks",
        }
    }
}

/// One observed difference between the file and its sources.
struct Deviation {
    /// Where in the report it goes.
    group: Group,
    /// ISIL of an institution, or `kobvid` of a directory entry.
    key: String,
    /// Stable field name, matched against [`Exception::field`].
    field: &'static str,
    /// What the file says and what the source says, in one line.
    detail: String,
}

impl Deviation {
    /// Record a difference.
    fn new(group: Group, key: impl Into<String>, field: &'static str, detail: String) -> Self {
        Self {
            group,
            key: key.into(),
            field,
            detail,
        }
    }
}

/// Compare one optional string field and record a deviation when it changed.
fn compare_field(
    key: &str,
    field: &'static str,
    have: Option<&str>,
    want: Option<&str>,
    into: &mut Vec<Deviation>,
) {
    if have == want {
        return;
    }
    into.push(Deviation::new(
        Group::ChangedField,
        key,
        field,
        format!("file {have:?}, upstream {want:?}"),
    ));
}

/// Compare a coordinate pair by distance rather than by equality, and report the distance.
///
/// Float equality would fail on a re-rounded decimal that names the same doorway; a
/// distance says how far the place actually moved.
fn compare_coordinates(
    key: &str,
    have: Option<LatLon>,
    want: Option<LatLon>,
    into: &mut Vec<Deviation>,
) {
    match (have, want) {
        (Some(have), Some(want)) => {
            let km = have.distance_km(want);
            if km > COORDINATE_TOLERANCE_KM {
                into.push(Deviation::new(
                    Group::ChangedField,
                    key,
                    "coordinates",
                    format!(
                        "file {},{}, upstream {},{} — {:.0} m apart",
                        have.lat,
                        have.lon,
                        want.lat,
                        want.lon,
                        km * 1000.0
                    ),
                ));
            }
        }
        (_, None) => into.push(Deviation::new(
            Group::ChangedField,
            key,
            "coordinates",
            "no source states coordinates for this entry any more".to_string(),
        )),
        (None, Some(_)) => into.push(Deviation::new(
            Group::ChangedField,
            key,
            "coordinates",
            "the file has no usable coordinates, the source does".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Institutions
// ---------------------------------------------------------------------------

/// Build an address line the way the file writes it: street, then postal code and city.
///
/// Both sources are compared through this one function, because both split the address
/// the same way and only differ in what they call the parts.
fn address_line(
    street: Option<&str>,
    postal_code: Option<&str>,
    place: Option<&str>,
) -> Option<String> {
    let tail = [postal_code.unwrap_or_default(), place.unwrap_or_default()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let parts: Vec<&str> = [street.unwrap_or_default(), tail.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The first place of a lobid record that states an address, and the line it makes.
fn lobid_address(org: &LobidOrg) -> Option<String> {
    org.location.iter().find_map(|place| {
        let address = place.address.as_ref()?;
        address_line(
            address.street_address.as_deref(),
            address.postal_code.as_deref(),
            address.address_locality.as_deref(),
        )
    })
}

/// The city of the first place that states one.
fn lobid_city(org: &LobidOrg) -> Option<&str> {
    org.location
        .iter()
        .find_map(|place| place.address.as_ref()?.address_locality.as_deref())
}

/// The coordinates of the first place that states them.
fn lobid_coords(org: &LobidOrg) -> Option<LatLon> {
    org.location.iter().find_map(|place| {
        let geo = place.geo.as_ref()?;
        LatLon::checked(geo.lat.trim().parse().ok()?, geo.lon.trim().parse().ok()?)
    })
}

/// The address line of a directory entry.
fn bib_address(entry: &BibEntry) -> Option<String> {
    let address = entry.address.as_ref()?;
    address_line(
        address.street.as_deref(),
        address.postal_code.as_ref().map(Scalar::as_text).as_deref(),
        address.place.as_deref(),
    )
}

/// The coordinates of a directory entry, whichever way it spells them.
fn bib_coords(entry: &BibEntry) -> Option<LatLon> {
    LatLon::checked(entry.lat.as_f64()?, entry.lng.as_f64()?)
}

/// The directory entries that carry a given ISIL.
///
/// Plural on purpose: `DE-109` has two, and collapsing that to "the first one" is exactly
/// the kind of silent choice this test exists to prevent.
fn directory_entries_for<'a>(
    directory: &'a BTreeMap<String, BibEntry>,
    isil: &str,
) -> Vec<&'a BibEntry> {
    directory
        .values()
        .filter(|entry| entry.isil.as_deref() == Some(isil))
        .collect()
}

/// Compare one institution against lobid, with the directory as the documented fallback
/// for the three fields lobid leaves empty (`plan/libraries.md` §8.4).
fn compare_institution(
    library: &Library,
    org: &LobidOrg,
    main: Option<&BibEntry>,
    into: &mut Vec<Deviation>,
) {
    let isil = library.isil.as_str();
    compare_field(isil, "name", Some(&library.name), org.name.as_deref(), into);
    compare_field(
        isil,
        "type",
        library.kind.as_deref(),
        org.classification
            .as_ref()
            .and_then(|c| c.label.get("de"))
            .map(String::as_str),
        into,
    );
    compare_field(isil, "city", Some(&library.city), lobid_city(org), into);
    compare_field(
        isil,
        "address",
        Some(&library.address),
        lobid_address(org).as_deref(),
        into,
    );
    compare_field(
        isil,
        "url",
        library.url.as_deref(),
        org.url
            .as_deref()
            .or_else(|| main.and_then(|entry| entry.home_url.as_deref())),
        into,
    );
    compare_field(
        isil,
        "opac",
        library.opac.as_deref(),
        org.provides
            .as_deref()
            .or_else(|| main.and_then(|entry| entry.opac_url.as_deref())),
        into,
    );
    compare_field(
        isil,
        "email",
        library.email.as_deref(),
        org.email
            .as_deref()
            .map(|mail| mail.trim_start_matches("mailto:"))
            .or_else(|| main.and_then(|entry| entry.address.as_ref()?.email.as_deref())),
        into,
    );
    compare_field(
        isil,
        "phone",
        library.phone.as_deref(),
        org.telephone.as_deref(),
        into,
    );
    compare_coordinates(
        isil,
        library.coords(),
        lobid_coords(org).or_else(|| main.and_then(bib_coords)),
        into,
    );
}

/// Compare the KOBV id of an institution, which comes from the directory alone.
fn compare_kobvid(library: &Library, entries: &[&BibEntry], into: &mut Vec<Deviation>) {
    let want = match entries {
        [] => None,
        [only] => Some(only.kobvid.as_str()),
        several => {
            let ids: Vec<&str> = several.iter().map(|entry| entry.kobvid.as_str()).collect();
            into.push(Deviation::new(
                Group::ChangedField,
                &library.isil,
                "kobvid",
                format!(
                    "file {:?}, but the directory has {} entries for this ISIL: {}",
                    library.kobvid,
                    ids.len(),
                    ids.join(", ")
                ),
            ));
            return;
        }
    };
    compare_field(
        &library.isil,
        "kobvid",
        library.kobvid.as_deref(),
        want,
        into,
    );
}

/// Compare every institution, and both directions of the institution set.
fn compare_institutions(
    libraries: &[Library],
    lobid: &BTreeMap<String, LobidOrg>,
    directory: &BTreeMap<String, BibEntry>,
    into: &mut Vec<Deviation>,
) {
    for library in libraries {
        let entries = directory_entries_for(directory, &library.isil);
        compare_kobvid(library, &entries, into);
        let Some(org) = lobid.get(&library.isil) else {
            into.push(Deviation::new(
                Group::MissingInstitution,
                &library.isil,
                "record",
                format!(
                    "{:?} — lobid returned no record for this ISIL",
                    library.name
                ),
            ));
            continue;
        };
        let main = match entries.as_slice() {
            [only] => Some(*only),
            _ => None,
        };
        compare_institution(library, org, main, into);
    }

    let known: BTreeSet<&str> = libraries
        .iter()
        .map(|library| library.isil.as_str())
        .collect();
    for isil in lobid.keys() {
        if !known.contains(isil.as_str()) {
            into.push(Deviation::new(
                Group::NewInstitution,
                isil,
                "record",
                "lobid answered with an ISIL that was not asked for — a merge or a \
                 redirect upstream"
                    .to_string(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

/// The institution a sub-ISIL belongs to, longest match first.
///
/// The order matters: `DE-578-3` is a list entry of its own (`CVK`), and a shortest-match
/// rule would swallow it into `DE-578` as a branch (`plan/libraries.md` §8.2 rule 3).
fn sub_isil_parent<'a>(isil: &str, known: &BTreeSet<&'a str>) -> Option<&'a str> {
    known
        .iter()
        .filter(|parent| {
            isil != **parent
                && isil.strip_prefix(*parent).is_some_and(|rest| {
                    rest.starts_with('-') || rest.chars().all(char::is_alphabetic)
                })
        })
        .max_by_key(|parent| parent.len())
        .copied()
}

/// Re-derive which directory entries are branches of which institution, by the rules of
/// `plan/libraries.md` §8.2 in their stated order.
///
/// Rules 2 (same ISIL, i.e. the main entry), 3 (sub-ISIL) and 4 (the VÖBB group code) are
/// mechanical and run here. Rule 1 (observed in a live availability answer) and rule 5
/// (name prefix) are not: rule 1 needs the 100 ids of §9, and rule 5 was hand-checked for
/// nine entries — one of which misspells the very house name it would have to match. Both
/// therefore appear as named entries in [`EXCEPTIONS`] rather than as a guessed rule that
/// would quietly re-assign a branch the day a name changes.
fn derive_branches(
    libraries: &[Library],
    directory: &BTreeMap<String, BibEntry>,
) -> BTreeMap<String, String> {
    let known: BTreeSet<&str> = libraries
        .iter()
        .map(|library| library.isil.as_str())
        .collect();
    let mut branches = BTreeMap::new();
    for entry in directory.values() {
        match entry.isil.as_deref() {
            // Rule 2: the entry *is* the institution, so it is not a branch of anything.
            Some(isil) if known.contains(isil) => continue,
            // Rule 3: a sub-ISIL hangs under the longest institution ISIL it extends.
            Some(isil) => {
                if let Some(parent) = sub_isil_parent(isil, &known) {
                    branches.insert(entry.kobvid.clone(), parent.to_string());
                    continue;
                }
            }
            None => {}
        }
        // Rule 4: the public network's group code. It applies to an entry with an ISIL of
        // its own too — the VÖBB houses carry `DE-B700`, `DE-B458` or nothing at all, and
        // none of those extends `DE-609`, so the group code is all there is to go on.
        if entry.terms.iter().any(|term| term == VOEBB_GROUP) {
            branches.insert(entry.kobvid.clone(), "DE-609".to_string());
        }
    }
    branches
}

/// Compare one branch against its directory entry.
fn compare_branch(branch: &Branch, entry: &BibEntry, into: &mut Vec<Deviation>) {
    let key = branch.kobvid.as_str();
    compare_field(
        key,
        "isil",
        branch.isil.as_deref(),
        entry.isil.as_deref(),
        into,
    );
    compare_field(key, "name", Some(&branch.name), Some(&entry.name), into);
    compare_field(
        key,
        "address",
        branch.address.as_deref(),
        bib_address(entry).as_deref(),
        into,
    );
    compare_coordinates(key, branch.coords(), bib_coords(entry), into);
}

/// Compare every branch, and both directions of the branch set.
fn compare_branches(
    libraries: &[Library],
    directory: &BTreeMap<String, BibEntry>,
    into: &mut Vec<Deviation>,
) {
    let derived = derive_branches(libraries, directory);
    let mut in_file: BTreeSet<&str> = BTreeSet::new();

    for library in libraries {
        for branch in &library.branches {
            in_file.insert(&branch.kobvid);
            let Some(entry) = directory.get(&branch.kobvid) else {
                into.push(Deviation::new(
                    Group::MissingBranch,
                    &branch.kobvid,
                    "record",
                    format!(
                        "{:?} of {} — the directory no longer has this entry",
                        branch.name, library.isil
                    ),
                ));
                continue;
            };
            compare_branch(branch, entry, into);
            match derived.get(&branch.kobvid) {
                Some(parent) if *parent == library.isil => {}
                Some(parent) => into.push(Deviation::new(
                    Group::ChangedField,
                    &branch.kobvid,
                    "parent",
                    format!(
                        "file has it under {}, the rules of §8.2 derive {parent}",
                        library.isil
                    ),
                )),
                None => into.push(Deviation::new(
                    Group::ChangedField,
                    &branch.kobvid,
                    "parent",
                    format!(
                        "file has it under {}, but rules 2-4 of §8.2 do not reach it \
                         ({:?}, ISIL {:?})",
                        library.isil, entry.name, entry.isil
                    ),
                )),
            }
        }
    }

    for (kobvid, parent) in &derived {
        if !in_file.contains(kobvid.as_str()) {
            let name = directory
                .get(kobvid)
                .map_or("?", |entry| entry.name.as_str());
            into.push(Deviation::new(
                Group::NewBranch,
                kobvid,
                "branch",
                format!("{name:?} would be a branch of {parent}"),
            ));
        }
    }
}

/// Check that the one vocabulary code the branch rules depend on still means what §8.2
/// says it means.
fn compare_group_code(terms: &BTreeMap<String, BibTerm>, into: &mut Vec<Deviation>) {
    let Some(term) = terms.get(VOEBB_GROUP) else {
        return; // Already fatal in `fetch_terms`; nothing to add.
    };
    compare_field(
        VOEBB_GROUP,
        "label",
        Some(VOEBB_GROUP_LABEL),
        Some(&term.label),
        into,
    );
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// What one run found, ready to be printed.
struct Report {
    /// How many institutions were compared.
    institutions: usize,
    /// How many branches were compared.
    branches: usize,
    /// Deviations that no exception covers. Any of these fails the test.
    drift: Vec<Deviation>,
    /// Deviations an exception covers, kept for the summary line.
    tolerated: usize,
    /// Exceptions the run did not observe. Any of these fails the test too.
    stale: Vec<&'static Exception>,
}

impl Report {
    /// Split the observed deviations against [`EXCEPTIONS`], in both directions: a
    /// difference no entry covers is drift, and an entry nothing matched is stale.
    fn new(institutions: usize, branches: usize, deviations: Vec<Deviation>) -> Self {
        let covered = |deviation: &Deviation| {
            EXCEPTIONS
                .iter()
                .any(|e| e.key == deviation.key && e.field == deviation.field)
        };
        let stale: Vec<&'static Exception> = EXCEPTIONS
            .iter()
            .filter(|exception| {
                !deviations
                    .iter()
                    .any(|d| d.key == exception.key && d.field == exception.field)
            })
            .collect();
        let tolerated = deviations.iter().filter(|d| covered(d)).count();
        let drift: Vec<Deviation> = deviations.into_iter().filter(|d| !covered(d)).collect();
        Self {
            institutions,
            branches,
            drift,
            tolerated,
            stale,
        }
    }

    /// Whether the file still matches its sources.
    fn is_clean(&self) -> bool {
        self.drift.is_empty() && self.stale.is_empty()
    }

    /// The one-line summary, which leads both the green line and the panic message.
    fn summary(&self) -> String {
        format!(
            "{} institutions and {} branches compared against lobid and the KOBV \
             directory: {} unexplained difference(s), {} stale exception(s), {} tolerated",
            self.institutions,
            self.branches,
            self.drift.len(),
            self.stale.len(),
            self.tolerated
        )
    }

    /// The full report: summary first, then the groups, then the stale exceptions.
    ///
    /// Built as a string rather than printed, because `cargo test` swallows stdout unless
    /// someone thought to pass `--nocapture` — and the diff is worth nothing if it is
    /// only visible to the person who already suspected something.
    fn render(&self) -> String {
        let mut out = String::new();
        writeln!(out, "{}", self.summary()).expect("writing to a String cannot fail");
        for group in [
            Group::MissingInstitution,
            Group::NewInstitution,
            Group::ChangedField,
            Group::MissingBranch,
            Group::NewBranch,
        ] {
            let lines: Vec<&Deviation> = self.drift.iter().filter(|d| d.group == group).collect();
            if lines.is_empty() {
                continue;
            }
            writeln!(out, "\n{} ({}):", group.heading(), lines.len())
                .expect("writing to a String cannot fail");
            for line in lines {
                writeln!(out, "  {} {}: {}", line.key, line.field, line.detail)
                    .expect("writing to a String cannot fail");
            }
        }
        if !self.stale.is_empty() {
            writeln!(
                out,
                "\nstale exceptions — listed as deliberate, no longer observed ({}):",
                self.stale.len()
            )
            .expect("writing to a String cannot fail");
            for exception in &self.stale {
                writeln!(
                    out,
                    "  {} {}: {}",
                    exception.key, exception.field, exception.reason
                )
                .expect("writing to a String cannot fail");
            }
        }
        writeln!(
            out,
            "\nEvery unexplained difference is either real drift — then fix \
             data/libraries.json — or a new deliberate deviation, and then it belongs in \
             EXCEPTIONS in this file with the reason spelled out."
        )
        .expect("writing to a String cannot fail");
        out
    }
}

/// Fetch, derive and compare. Returns the report, or why it could not be made.
fn run(http: &Http) -> Result<Report, Failure> {
    let libraries: Vec<Library> = serde_json::from_str(LIBRARIES_JSON)
        .map_err(|err| Failure::Fatal(format!("data/libraries.json does not parse: {err}")))?;
    let lobid = fetch_lobid(http, &libraries)?;
    let directory = fetch_bibinfo(http)?;
    let terms = fetch_terms(http)?;

    let mut deviations = Vec::new();
    compare_group_code(&terms, &mut deviations);
    compare_institutions(&libraries, &lobid, &directory, &mut deviations);
    compare_branches(&libraries, &directory, &mut deviations);

    let branches = libraries
        .iter()
        .map(|library| library.branches.len())
        .sum::<usize>();
    Ok(Report::new(libraries.len(), branches, deviations))
}

/// The list still says what its sources say.
///
/// Runs live on every `cargo test`; see the module header for why, and for the one
/// failure that is skipped rather than reported.
#[test]
fn the_library_list_matches_its_upstream_sources() {
    let http = Http::new(true);
    let report = match run(&http) {
        Ok(report) => report,
        Err(Failure::Offline(message)) => {
            eprintln!(
                "SKIPPED: data/libraries.json was NOT checked in this run — {message}.\n\
                 Re-run it on a connected machine with:\n  \
                 cargo test --test drift_libraries -- --nocapture"
            );
            return;
        }
        Err(Failure::Fatal(message)) => panic!("{message}"),
    };

    assert!(report.is_clean(), "{}", report.render());
    println!("{}", report.summary());
}
