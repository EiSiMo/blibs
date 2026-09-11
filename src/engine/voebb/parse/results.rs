//! The result list.
//!
//! One page of aDISWeb hits: the count in the header, up to 22 rows, the paging toolbar
//! and the form state needed to ask for the next page or to apply the branch facet.
//!
//! The rows carry less than a record: `div.rList_name` is responsibility, edition and
//! publisher in **one** unsplit string, and the traffic light is a summary over *all*
//! branches. Nothing here answers "is it in my branch" — that needs the detail page. The
//! hit exists so that a search can be listed and paged without fetching 22 detail pages.
//!
//! Fixtures: `tests/fixtures/voebb/results.html` (71 hits, page 1 of 4),
//! `results_filtered.html` (35, the facet applied), `results_filtered_page2.html`
//! (positions 23–35, the last page) and `results_isbn.html` (4, a single page).

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};

use crate::error::{Error, UnexpectedError};
use crate::model::RecordId;

use super::form::{self, FormState};
use super::{collapse, compile};

/// The document name used in errors from this module.
const DOCUMENT: &str = "voebb.de result list";

/// The words aDISWeb uses when a search found nothing at all.
///
/// Two of them, because the two search forms word it differently, and both were captured
/// from the live site (`tests/fixtures/voebb/results_empty*.html`):
///
/// - the simple search: *"Ihre Suche mit \"Freie Suche = …\" in allen Suchbereichen war
///   **erfolglos**."*
/// - the advanced search: *"**Kein Treffer** im Suchbereich \"Bibliotheksbestand\" …"*,
///   sometimes followed by a note that interlibrary loan has hits — which is not a
///   holding in this region and never a hit for this tool.
///
/// Only the fragments are matched: the rest of each sentence quotes the query back and
/// therefore varies with it. Nothing is guessed here — a third wording will land in
/// [`total_hits`] as a missing selector and be captured before it is added.
const NO_HITS_MARKERS: [&str; 2] = ["erfolglos", "Kein Treffer"];

/// One row of the result list.
///
/// Every field except the id and the title is optional, because every one of them was
/// seen empty in the measurement — an empty cell is not a broken page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The record id, `voebb_SAK…`, taken from the title link's `sp=` parameter. The
    /// row's `data-ajax` carries the same number without the `S` and is deliberately not
    /// used: the link is what `show` has to reproduce.
    pub id: RecordId,
    /// The absolute position in the result set, from `div.rList_num`. Counts across
    /// pages: page 2 starts at 23.
    pub position: Option<u32>,
    /// The title, as the link text has it.
    pub title: String,
    /// Responsibility, edition and publisher in one string, exactly as
    /// `div.rList_name` has it (`… . - <edition>. - <publisher>`). **Not** split into
    /// fields: the separated values exist only on the detail page, and splitting on the
    /// dashes here would invent structure the list does not have.
    pub statement: Option<String>,
    /// The year, when the row states one that is a number.
    pub year: Option<i32>,
    /// The material type, from the `alt` of the icon in `div.rList_medium` — measured
    /// `Band`, `Buch`, `CD`, `E-Ressource`, `Medienkombination`. Kept as the site's own
    /// word; mapping it is the engine's job, not the parser's.
    pub media_kind: Option<String>,
    /// The traffic light of `div.rList_availability`, as text — measured `Verfügbar`,
    /// `Zurzeit nicht verfügbar` and `siehe Vollanzeige`. It is a **summary over every
    /// branch** and must never be reported as a branch's answer.
    pub availability: Option<String>,
    /// The shelfmark column, which was empty in every row of the measurement. Kept so
    /// that a page which does fill it is not silently dropped.
    pub shelfmark: Option<String>,
}

/// One page of results, plus what is needed to ask for the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultPage {
    /// The hit count the page states in its header. The same on every page of a result
    /// set, and unaffected by paging.
    pub total: u64,
    /// The rows of this page, in the order the page has them.
    pub hits: Vec<Hit>,
    /// Whether the toolbar offers a next page. Read from the toolbar rather than
    /// computed from `total`, because 22 rows per page is an observation, not a promise.
    pub has_next: bool,
    /// Whether the toolbar offers a previous page.
    pub has_prev: bool,
    /// The form state of this page — the branch facet and the next page are both a
    /// replay of it.
    pub form: FormState,
}

/// Parse a result page.
///
/// Selectors: `div#R06 p.info` (the count), `div.resultlist li.rList_li` (the rows),
/// `input#Toolbar_2`/`#Toolbar_3` (paging), plus the form (see [`super::form`]).
///
/// A page that states **zero** hits and carries no rows is a valid result, not an error.
/// Every other absence is [`UnexpectedError::MissingSelector`] naming the selector: a
/// stated count with no rows below it, or a stated count that the last row's position
/// contradicts, is silent truncation and the worst possible answer for an agent.
pub fn parse_results(html: &str) -> Result<ResultPage, Error> {
    let form = form::parse_form(html, "the result list")?;
    let document = Html::parse_document(html);

    let total = total_hits(&document)?;
    let hits = hits(&document)?;
    if total > 0 && hits.is_empty() {
        return Err(missing_selector("div.resultlist li.rList_li"));
    }

    let page = ResultPage {
        total,
        hits,
        has_next: toolbar_offers(&document, "Toolbar_3", total)?,
        has_prev: toolbar_offers(&document, "Toolbar_2", total)?,
        form,
    };
    check_not_truncated(&page)?;
    Ok(page)
}

/// Whether the page states, in so many words, that the search found nothing.
///
/// A search with no hits anywhere in the network does not carry `p.info` at all: aDISWeb
/// replaces it, in the same `div#R06`, with a `p.wichtig` reading *"Ihre Suche mit … war
/// erfolglos"*. That is a **positive** marker and this is why it has to be one — every
/// empty result at a branch used to reach `missing_selector("div#R06 p.info")` and leave
/// as exit 6 telling the user to report a scraper bug, which is the most ordinary answer
/// a search can have.
///
/// Both halves are required. `p.wichtig` on its own is not read as "no hits": the class
/// is aDISWeb's general one for a prominent message, so a page carrying one for some
/// other reason must still fail loudly rather than be reported as an empty result. Doubt
/// runs in exactly one direction here: towards the loud failure, never towards the empty
/// answer.
fn states_no_hits(document: &Html) -> bool {
    document.select(&selectors().notice).any(|notice| {
        let text = collapse(&text_of(notice));
        NO_HITS_MARKERS.iter().any(|marker| text.contains(marker))
    })
}

/// The hit count out of `div#R06 p.info`.
///
/// Read as a number after the word `Treffer:`, never by comparing the sentence: the
/// single-field search writes `Treffer: 71 in Bibliotheksbestand` and the advanced one
/// `Treffer: 4 im "Bibliotheksbestand"`.
///
/// A page that states no hits has no count to read and is zero — see [`states_no_hits`].
fn total_hits(document: &Html) -> Result<u64, Error> {
    let Some(info) = document.select(&selectors().info).next() else {
        if states_no_hits(document) {
            return Ok(0);
        }
        return Err(missing_selector("div#R06 p.info"));
    };
    let text = collapse(&text_of(info));
    count_after_marker(&text).ok_or_else(|| {
        UnexpectedError::MissingElement {
            what: format!("a hit count (`Treffer: <n>`) in {text:?}"),
            context: DOCUMENT.to_string(),
        }
        .into()
    })
}

/// The number following `Treffer:`, with the German thousands separator removed.
fn count_after_marker(text: &str) -> Option<u64> {
    let tail = text.split_once("Treffer:")?.1.trim_start();
    let mut digits = String::new();
    let mut chars = tail.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            '0'..='9' => digits.push(ch),
            // A dot only counts as a separator when a digit follows it, so that a count
            // at the end of a sentence does not swallow the full stop.
            '.' if digits.is_empty() => break,
            '.' => {}
            _ => break,
        }
        chars.next();
        if ch == '.' && !matches!(chars.peek(), Some('0'..='9')) {
            break;
        }
    }
    digits.parse().ok()
}

/// Every row of the list, in document order.
fn hits(document: &Html) -> Result<Vec<Hit>, Error> {
    document.select(&selectors().row).map(hit).collect()
}

/// One row.
fn hit(row: ElementRef<'_>) -> Result<Hit, Error> {
    let link = row
        .select(&selectors().title_link)
        .next()
        .ok_or_else(|| missing_selector("li.rList_li div.rList_titel a[href]"))?;
    let href = link
        .value()
        .attr("href")
        .ok_or_else(|| missing_selector("li.rList_li div.rList_titel a[href]"))?;

    Ok(Hit {
        id: RecordId::voebb(&local_id(href)?),
        position: cell(row, &selectors().number).and_then(|text| text.parse().ok()),
        title: collapse(&text_of(link)),
        statement: statement(row),
        year: cell(row, &selectors().year).and_then(|text| text.parse().ok()),
        media_kind: icon_text(row, &selectors().medium),
        availability: icon_text(row, &selectors().availability),
        shelfmark: cell(row, &selectors().shelfmark),
    })
}

/// The record number out of the title link.
///
/// The link is `…/app/prod00?sp=SPROD00&sp=SAK13776205`: the **last** `sp=` parameter is
/// the record, the first is the product. A link without one is an error rather than a
/// row with an invented id.
fn local_id(href: &str) -> Result<String, Error> {
    href.split(['?', '&'])
        .filter_map(|parameter| parameter.strip_prefix("sp="))
        .rfind(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            UnexpectedError::MissingElement {
                what: format!("an `sp=` record number in the title link {href:?}"),
                context: DOCUMENT.to_string(),
            }
            .into()
        })
}

/// The responsibility statement. A row has two `div.rList_name` cells, the first of them
/// a spacer; every non-empty one is kept and joined.
fn statement(row: ElementRef<'_>) -> Option<String> {
    let parts: Vec<String> = row
        .select(&selectors().name)
        .map(|cell| collapse(&text_of(cell)))
        .filter(|text| !text.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The text of one cell, or `None` when it is empty or a `&nbsp;` spacer.
fn cell(row: ElementRef<'_>, selector: &Selector) -> Option<String> {
    let text = collapse(&text_of(row.select(selector).next()?));
    (!text.is_empty()).then_some(text)
}

/// The label of an icon cell: `alt` first, `title` second, both of which the site fills
/// with the same word.
fn icon_text(row: ElementRef<'_>, selector: &Selector) -> Option<String> {
    let icon = row.select(selector).next()?;
    let value = icon.value();
    let label = value.attr("alt").or_else(|| value.attr("title"))?.trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// Whether a paging button is present and enabled.
///
/// The end of the list is the `disabled` attribute on `#Toolbar_3` (*next*) and
/// `#Toolbar_4` (*to the end*) — more reliable than `ceil(total / 22)`, which assumes the
/// page size never changes. A page that states hits but has no toolbar at all is an
/// error: without it, the last page and a truncated one look the same.
fn toolbar_offers(document: &Html, id: &str, total: u64) -> Result<bool, Error> {
    let selector = compile(&format!("input#{id}"));
    match document.select(&selector).next() {
        Some(button) => Ok(button.value().attr("disabled").is_none()),
        None if total == 0 => Ok(false),
        None => Err(missing_selector(&format!("input#{id}"))),
    }
}

/// Counter-check against silent truncation: the last row's position must reach the stated
/// total unless the toolbar offers another page.
///
/// Positions are absolute, so this holds on every page of a result set. When the rows
/// carry no position the row count is used instead, which is the same statement for
/// page 1.
fn check_not_truncated(page: &ResultPage) -> Result<(), Error> {
    if page.has_next || page.total == 0 {
        return Ok(());
    }
    let reached = match page.hits.last().and_then(|hit| hit.position) {
        Some(position) => u64::from(position),
        None => page.hits.len() as u64,
    };
    if reached < page.total {
        return Err(UnexpectedError::CountMismatch {
            context: format!("{DOCUMENT} (last page, no further page offered)"),
            expected: usize::try_from(page.total).unwrap_or(usize::MAX),
            found: usize::try_from(reached).unwrap_or(usize::MAX),
        }
        .into());
    }
    Ok(())
}

/// The compiled selectors. Every one is a literal in this file, so a parse failure would
/// be a typo in this source and nothing a user or the service can cause.
struct Selectors {
    info: Selector,
    notice: Selector,
    row: Selector,
    title_link: Selector,
    number: Selector,
    name: Selector,
    year: Selector,
    medium: Selector,
    availability: Selector,
    shelfmark: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        info: compile("div#R06 p.info"),
        notice: compile("div#R06 p.wichtig"),
        row: compile("div.resultlist li.rList_li"),
        title_link: compile("div.rList_titel a[href]"),
        number: compile("div.rList_num"),
        name: compile("div.rList_name"),
        year: compile("div.rList_jahr"),
        medium: compile("div.rList_medium img"),
        availability: compile("div.rList_availability img"),
        shelfmark: compile("div.rList_signatur"),
    })
}

/// All text below an element.
fn text_of(element: ElementRef<'_>) -> String {
    element.text().collect()
}

/// A missing-selector error naming what stopped matching in this document.
fn missing_selector(selector: &str) -> Error {
    super::missing_selector(selector, DOCUMENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULTS: &str = include_str!("../../../../tests/fixtures/voebb/results.html");
    const FILTERED: &str = include_str!("../../../../tests/fixtures/voebb/results_filtered.html");
    const EMPTY: &str = include_str!("../../../../tests/fixtures/voebb/results_empty.html");
    const EMPTY_ADVANCED: &str =
        include_str!("../../../../tests/fixtures/voebb/results_empty_advanced.html");
    const PAGE2: &str =
        include_str!("../../../../tests/fixtures/voebb/results_filtered_page2.html");
    const ISBN: &str = include_str!("../../../../tests/fixtures/voebb/results_isbn.html");

    fn parsed(html: &str) -> ResultPage {
        match parse_results(html) {
            Ok(page) => page,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    /// Page 1 of the free search: 71 hits, 22 rows, a next page on offer and no previous
    /// one.
    #[test]
    fn reads_the_first_page() {
        let page = parsed(RESULTS);
        assert_eq!(page.total, 71);
        assert_eq!(page.hits.len(), 22);
        assert!(page.has_next);
        assert!(!page.has_prev);
    }

    /// The id comes from the `sp=` parameter of the title link and carries the engine as
    /// its prefix.
    #[test]
    fn reads_the_record_id_from_the_title_link() {
        let page = parsed(RESULTS);
        let first = &page.hits[0];
        assert_eq!(first.id.as_str(), "voebb_SAK13776205");
        assert_eq!(first.id.local_id(), "SAK13776205");
    }

    /// The fields one row states, exactly as it states them — the responsibility
    /// statement unsplit, the year a number, the icons by their `alt`.
    #[test]
    fn reads_one_row() {
        let page = parsed(RESULTS);
        let first = &page.hits[0];
        assert_eq!(first.position, Some(1));
        assert_eq!(first.title, "Bernhard Schlink, Der Vorleser");
        assert_eq!(
            first.statement.as_deref(),
            Some("erarb. von Ekkehart Mittelberg. - 1. Aufl., 1. Dr.. - Cornelsen")
        );
        assert_eq!(first.year, Some(2004));
        assert_eq!(first.media_kind.as_deref(), Some("Band"));
        assert_eq!(first.availability.as_deref(), Some("Verfügbar"));
        // Empty in every row of the measurement, and a `&nbsp;` is not content.
        assert_eq!(first.shelfmark, None);
    }

    /// The branch facet is an upstream filter: the same query, 71 hits down to 35.
    #[test]
    fn reads_the_filtered_page() {
        let page = parsed(FILTERED);
        assert_eq!(page.total, 35);
        assert!(page.has_next);
        assert!(!page.has_prev);
    }

    /// Paging: positions are absolute and continue across pages, the total does not
    /// change, and the toolbar says this is the last page.
    #[test]
    fn reads_the_second_page() {
        let page = parsed(PAGE2);
        assert_eq!(page.total, 35);
        assert_eq!(page.hits.first().and_then(|hit| hit.position), Some(23));
        assert_eq!(page.hits.last().and_then(|hit| hit.position), Some(35));
        assert!(!page.has_next);
        assert!(page.has_prev);
    }

    /// The advanced search writes the header differently (`Treffer: 4 im
    /// "Bibliotheksbestand"`), which is why the count is read as a number and never by
    /// comparing the sentence.
    #[test]
    fn reads_the_other_spelling_of_the_header() {
        let page = parsed(ISBN);
        assert_eq!(page.total, 4);
        assert_eq!(page.hits.len(), 4);
        assert!(!page.has_next);
        assert!(!page.has_prev);
    }

    /// Every hit of every fixture carries an id and a title. A row without them would be
    /// an error, never a hit with an empty title.
    #[test]
    fn every_row_has_an_id_and_a_title() {
        for html in [RESULTS, FILTERED, PAGE2, ISBN] {
            for hit in parsed(html).hits {
                assert!(hit.id.local_id().starts_with("SAK"), "{:?}", hit.id);
                assert!(!hit.title.is_empty(), "{:?}", hit.id);
            }
        }
    }

    /// The German thousands separator, which no fixture has but the site will produce
    /// above 999 hits.
    #[test]
    fn reads_a_grouped_count() {
        assert_eq!(count_after_marker("Treffer: 1.234 in X"), Some(1234));
        assert_eq!(count_after_marker("Treffer: 71 in X"), Some(71));
        assert_eq!(count_after_marker("… Treffer: 4 im \"X\""), Some(4));
        assert_eq!(count_after_marker("Treffer: 12."), Some(12));
        assert_eq!(count_after_marker("no count here"), None);
    }

    /// A header without a count is a named error, not zero hits.
    #[test]
    fn a_header_without_a_count_is_an_error() {
        let broken = RESULTS.replace("Treffer: 71", "Treffer werden gezählt");
        let error = parse_results(&broken).expect_err("must not be accepted");
        assert_eq!(error.kind(), "missing_structure");
    }

    /// A stated count with no rows below it is silent truncation, and names the row
    /// selector rather than returning an empty page.
    #[test]
    fn a_count_without_rows_names_the_row_selector() {
        let broken = RESULTS.replace("rList_li ", "rList_gone ");
        let error = parse_results(&broken).expect_err("must not be accepted");
        let message = error.to_string();
        assert!(message.contains("rList_li"), "{message}");
    }

    /// The header selector is named when the header moves.
    #[test]
    fn a_missing_header_names_its_selector() {
        let broken = RESULTS.replace("id=\"R06\"", "id=\"R06x\"");
        let error = parse_results(&broken).expect_err("must not be accepted");
        assert!(error.to_string().contains("div#R06 p.info"), "{error}");
    }

    /// A search that finds nothing anywhere carries no `p.info` at all — aDISWeb writes a
    /// `p.wichtig` saying the search "war erfolglos" instead. Every empty result at a
    /// branch used to come out of here as `missing_selector` and exit 6, telling the user
    /// to report a scraper bug for the most ordinary answer a search can have.
    #[test]
    fn a_search_that_found_nothing_is_zero_hits_and_not_an_error() {
        let page = parsed(EMPTY);
        assert_eq!(page.total, 0);
        assert!(page.hits.is_empty());
        assert!(!page.has_next);
    }

    /// The advanced form words the same emptiness differently — "Kein Treffer im
    /// Suchbereich …" rather than "war erfolglos" — which is why the first fix left the
    /// reported case `--title "Prometheus LernAtlas" Skelett --at AGB` still failing.
    #[test]
    fn the_advanced_forms_wording_for_nothing_found_is_zero_hits_too() {
        let page = parsed(EMPTY_ADVANCED);
        assert_eq!(page.total, 0);
        assert!(page.hits.is_empty());
    }

    /// The advanced page adds that interlibrary loan has hits. That is not a holding in
    /// this region and must not become one.
    #[test]
    fn hits_in_interlibrary_loan_are_not_hits_here() {
        assert!(
            EMPTY_ADVANCED.contains("Fernleihe"),
            "the fixture is the one that mentions it"
        );
        assert_eq!(parsed(EMPTY_ADVANCED).total, 0);
    }

    /// The marker has to be positive on both halves. `p.wichtig` is aDISWeb's general
    /// class for a prominent message, so one that does not say "erfolglos" is an unknown
    /// page and must still fail loudly — never be reported as an empty result.
    #[test]
    fn a_prominent_message_that_is_not_the_empty_one_still_fails() {
        let other = EMPTY.replace("war erfolglos", "wurde eingeschränkt");
        let error = parse_results(&other).expect_err("an unknown page is not zero hits");
        assert!(error.to_string().contains("div#R06 p.info"), "{error}");
    }

    /// The last page must account for every stated hit. A page that shows 22 of 71 and
    /// offers no next page is truncated, and saying so is the whole point of reading the
    /// toolbar.
    #[test]
    fn a_short_last_page_is_a_count_mismatch() {
        let broken = RESULTS
            .replace(
                "alt=\"nächster\" id=\"Toolbar_3\"",
                "id=\"Toolbar_3\" disabled",
            )
            .replace(
                "alt=\"zum Ende\" id=\"Toolbar_4\"",
                "id=\"Toolbar_4\" disabled",
            );
        let error = parse_results(&broken).expect_err("must not be accepted");
        assert_eq!(error.kind(), "count_mismatch");
    }

    /// The page's form state comes back with it: the facet and the next page are both a
    /// replay of it.
    #[test]
    fn carries_the_form_state_of_the_page() {
        let page = parsed(RESULTS);
        assert_eq!(page.form.request_count(), Some(2));
        assert!(page.form.button_labelled("Filtern").is_some());
    }
}
