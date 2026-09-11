//! The `/noaccess` check.
//!
//! aDISWeb answers a broken form with **HTTP 200** and a page that says the session is
//! gone. Read as a result list it looks exactly like "no hits" — which is why this runs
//! before every other parser and turns it into
//! [`crate::error::UnexpectedError::VoebbNoAccess`], exit 6.
//!
//! Three signals mark such a page, cheapest first: the end URL ends in `/noaccess`, the
//! document has **no `<form>`** (every real page has `Form0`), and the text says
//! *Bitte schließen Sie diesen Reiter*. The URL is the client's to check; this function
//! has only the body, so it uses the other two — and requires both, so that a page that
//! merely lost its form produces a selector error naming what is missing instead of a
//! wrong diagnosis.
//!
//! Fixtures: `tests/fixtures/voebb/noaccess.html` (the page) against every other voebb
//! fixture (a real page).

use std::sync::OnceLock;

use scraper::{Html, Selector};

use crate::error::{Error, UnexpectedError};

use super::compile;

/// The sentences the page carries, in both of its languages, plus the reason it gives.
///
/// Matched as substrings of the raw HTML: they are plain text in the body, and reading
/// them without a DOM keeps this check cheap enough to run before every parse.
const MARKERS: [&str; 3] = [
    "Bitte schließen Sie diesen Reiter",
    "Please close this tab",
    "Aus Sicherheitsgründen ist das nicht erlaubt.",
];

/// Fail if this page is the session-lost page.
///
/// `step` names the request that produced it and goes into the message, because "the
/// session was lost" is only actionable if it says where.
pub fn check(html: &str, step: &str) -> Result<(), Error> {
    if is_noaccess(html) {
        return Err(UnexpectedError::VoebbNoAccess {
            step: step.to_string(),
        }
        .into());
    }
    Ok(())
}

/// Whether the body is the `/noaccess` page: no form, and either one of the [`MARKERS`]
/// or no `<title>` — the page is the only aDISWeb response without one.
fn is_noaccess(html: &str) -> bool {
    if !MARKERS.iter().any(|marker| html.contains(marker)) && has_title(html) {
        return false;
    }
    let document = Html::parse_document(html);
    document.select(&selectors().form).next().is_none()
}

/// Whether the document has a non-empty `<title>`.
fn has_title(html: &str) -> bool {
    let document = Html::parse_document(html);
    document
        .select(&selectors().title)
        .any(|title| !title.text().collect::<String>().trim().is_empty())
}

/// The compiled selectors, both literals in this file.
struct Selectors {
    form: Selector,
    title: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        form: compile("form"),
        title: compile("title"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOACCESS: &str = include_str!("../../../../tests/fixtures/voebb/noaccess.html");
    const RESULTS: &str = include_str!("../../../../tests/fixtures/voebb/results.html");
    const START: &str = include_str!("../../../../tests/fixtures/voebb/start.html");

    /// The measured failure page: HTTP 200, no `<form>`, no `<title>`, and the sentence.
    /// The error names the step, because that is the only thing that says which request
    /// burnt the session.
    #[test]
    fn rejects_the_noaccess_page() {
        let error = check(NOACCESS, "the branch filter").expect_err("must be rejected");
        let message = error.to_string();
        assert!(message.contains("the branch filter"), "{message}");
        assert!(message.contains("session"), "{message}");
    }

    /// Exit 6 with a kind an agent can branch on, never "no hits" (exit 1).
    #[test]
    fn the_noaccess_page_is_a_named_error() {
        let error = check(NOACCESS, "the search").expect_err("must be rejected");
        assert_eq!(error.kind(), "voebb_session_lost");
    }

    /// Real pages pass. A result page with 71 hits must not look like a lost session.
    #[test]
    fn accepts_real_pages() {
        assert!(check(RESULTS, "the search").is_ok());
        assert!(check(START, "the entry page").is_ok());
    }

    /// A page that carries the sentence *and* a form is not the failure page — the
    /// sentence alone must not be enough, or a record about tab management would abort a
    /// search.
    #[test]
    fn a_page_with_a_form_is_never_the_failure_page() {
        let html = RESULTS.replace("Trefferliste", "Bitte schließen Sie diesen Reiter");
        assert!(check(&html, "the search").is_ok());
    }
}
