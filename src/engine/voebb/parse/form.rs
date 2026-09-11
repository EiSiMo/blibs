//! The aDISWeb form state: the action path, the nine hidden fields and the buttons.
//!
//! Every page of voebb.de is one `<form name="Form0">`, and every request replays that
//! form. Two of its hidden fields are single-use — `identity` changes with **every**
//! response and `requestCount` counts up — so the state is read out of the page that was
//! just received and never written by hand; sending a stale one produces a `/noaccess`
//! page with status 200.
//!
//! The buttons are read out of the page for the same reason: a button's `focus` value is
//! its `data-fld`, and that differs from page to page ( *Erweiterte Suche* is
//! `$$GFBO_3` on the entry page and `$$GFBO_7` on a result page). **Field names are not
//! stable across pages either**: `$Button$6` is *Suchen* on the advanced form and
//! *Verfasser* on the result list. Look a button up by name and check its label, or look
//! it up by label — never hard-wire the pair.
//!
//! Fixtures: `tests/fixtures/voebb/{start,results,advanced_form}.html`.

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};

use crate::error::Error;

use super::compile;

/// The form field names that do not change between pages.
///
/// Everything here is verified against the fixtures named in the module docs. Names that
/// *do* change between pages (`$Button…`) are deliberately absent: they are looked up
/// through [`FormState::button`] and [`FormState::button_labelled`] instead.
pub mod fields {
    /// The nine hidden fields every aDISWeb page carries, in document order.
    ///
    /// All nine are required: carrying on with eight of them produces a `/noaccess` page
    /// one request later, where the cause is no longer visible.
    pub const HIDDEN: [&str; 9] = [
        IDENTITY,
        "keyCode",
        FOCUS,
        "stz",
        SOURCE,
        "selected",
        REQUEST_COUNT,
        "scriptEnabled",
        "scrollPos",
    ];

    /// Single-use session token; changes with every response.
    pub const IDENTITY: &str = "identity";
    /// Step counter; a repeated value is rejected with `/noaccess`.
    pub const REQUEST_COUNT: &str = "requestCount";
    /// Set to `$B` when a button was pressed.
    pub const SOURCE: &str = "source";
    /// Set to the pressed button's `data-fld`.
    pub const FOCUS: &str = "focus";

    /// The value `source` carries when a button was pressed.
    pub const SOURCE_BUTTON: &str = "$B";
    /// The value a pressed button sends for itself.
    pub const PRESSED: &str = "pressed";

    /// The single search field on the entry page and on the result page.
    pub const SEARCH_TERMS: &str = "$Autosuggest";
    /// The search scope, i.e. which catalogue is searched.
    pub const SEARCH_SCOPE: &str = "$Select";
    /// The only scope this tool sends: the network's physical and electronic holdings.
    /// Not the branch selector — that vocabulary is coarser than the facet's and does not
    /// even contain the AGB.
    pub const SCOPE_HOLDINGS: &str = "Bibliotheksbestand";

    /// One of these per ticked facet box, carrying the checkbox element's **id**
    /// (`sub-PTL1_tree_1_90`), not its name.
    pub const FACET_CHECKBOX: &str = "$CbTree_text";

    /// The paging control: one field whose *value* is the button index.
    pub const TOOLBAR: &str = "$Toolbar";

    /// The four index selects of the advanced form, in row order.
    pub const ADVANCED_INDEX: [&str; 4] = ["$Select$0", "$Select$2", "$Select$4", "$Select$6"];
    /// The four term fields of the advanced form, in row order.
    pub const ADVANCED_TERM: [&str; 4] = [
        "$Autosuggest$0",
        "$Autosuggest$1",
        "$Autosuggest$2",
        "$Autosuggest$3",
    ];
    /// The three boolean selects between the four rows.
    pub const ADVANCED_JOIN: [&str; 3] = ["$Select$1", "$Select$3", "$Select$5"];

    /// The index vocabulary of the advanced form. The option values *are* these strings.
    pub mod index {
        /// `--title`.
        pub const TITLE: &str = "Titel";
        /// `--author`.
        pub const PERSON: &str = "Person";
        /// `--isbn` and `--issn`.
        pub const IDENTIFIER: &str = "ISBN, ISSN, ISMN";
        /// `--subject`.
        pub const SUBJECT: &str = "Schlagwort";
    }

    /// The boolean vocabulary between two rows of the advanced form.
    pub mod join {
        /// The default, and the only one this tool sends.
        pub const AND: &str = "UND";
        /// Offered by the form, unused.
        pub const OR: &str = "ODER";
        /// Offered by the form, unused.
        pub const NOT: &str = "NICHT";
    }

    /// The values of [`TOOLBAR`]. One field, the index as its value — never
    /// `$Toolbar_3=…`.
    pub mod toolbar {
        /// Jump to the first page.
        pub const FIRST: u8 = 1;
        /// One page back.
        pub const PREVIOUS: u8 = 2;
        /// One page on.
        pub const NEXT: u8 = 3;
        /// Jump to the last page.
        pub const LAST: u8 = 4;
    }

    /// Button labels. Stable where the field names are not — the label is what the user
    /// would click.
    pub mod label {
        /// Runs the single-field search.
        pub const SEARCH: &str = "Suchen";
        /// Opens the advanced search form.
        pub const ADVANCED: &str = "Erweiterte Suche";
        /// Applies the ticked facet boxes. Present twice on a result page (head and foot
        /// of the tree); either one works.
        pub const FILTER: &str = "Filtern";
    }
}

/// The document name used in errors from this module.
const DOCUMENT: &str = "voebb.de form";

/// One submit button of the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// The field name to send, e.g. `$Button$2`. **Page-dependent** — see the module docs.
    pub name: String,
    /// The button's caption, e.g. `Filtern`.
    pub label: String,
    /// The value the `focus` field must carry when this button is pressed: the button's
    /// `data-fld`. Differs per page and must never be hard-wired.
    pub focus: String,
}

/// The state of one page's form, ready to be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    /// The form's `action`, an absolute path carrying the session id.
    pub action: String,
    /// The hidden fields, name and value, in document order.
    pub fields: Vec<(String, String)>,
    /// The submit buttons the page offers.
    pub buttons: Vec<Button>,
}

impl FormState {
    /// The value of one hidden field.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The page's `requestCount`, when it is a number.
    ///
    /// `None` rather than an error: the field's *presence* is checked in
    /// [`parse_form`], and a value that is not a number is something only the site can
    /// cause. The client replays the field verbatim; this is for logging and for the
    /// counter-check that the session advanced.
    pub fn request_count(&self) -> Option<u32> {
        self.get(fields::REQUEST_COUNT)?.trim().parse().ok()
    }

    /// A button by field name. Check its [`Button::label`] before pressing it — the same
    /// name means different things on different pages.
    pub fn button(&self, name: &str) -> Option<&Button> {
        self.buttons.iter().find(|button| button.name == name)
    }

    /// The first button with this caption, in document order.
    ///
    /// *Filtern* appears twice on a result page (`$Button$2` at the head of the facet
    /// tree, `$Button$3` at its foot); they do the same thing, so the first is taken.
    pub fn button_labelled(&self, label: &str) -> Option<&Button> {
        self.buttons.iter().find(|button| button.label == label)
    }
}

/// Read the form state out of a page.
///
/// Selectors: `form[name="Form0"]`, `input[type="hidden"]`, `input[type="submit"]`.
/// `step` names the request that produced the page and goes into the error, because a
/// changed form is only actionable if it says which step broke.
///
/// Fails with [`crate::error::UnexpectedError::MissingSelector`] when the form, its
/// `action` or any of
/// the nine fields in [`fields::HIDDEN`] is absent. It never returns a partial state: a
/// form replayed without `identity` produces a `/noaccess` page one request later, and
/// the cause is then invisible.
pub fn parse_form(html: &str, step: &'static str) -> Result<FormState, Error> {
    let document = Html::parse_document(html);
    let form = document
        .select(&selectors().form)
        .next()
        .ok_or_else(|| missing_selector("form[name=\"Form0\"]", step))?;

    let action = form
        .value()
        .attr("action")
        .filter(|action| !action.trim().is_empty())
        .ok_or_else(|| missing_selector("form[name=\"Form0\"][action]", step))?
        .to_string();

    let state = FormState {
        action,
        fields: hidden_fields(form),
        buttons: buttons(form),
    };

    for required in fields::HIDDEN {
        if state.get(required).is_none() {
            return Err(missing_selector(
                &format!("input[type=\"hidden\"][name=\"{required}\"]"),
                step,
            ));
        }
    }
    Ok(state)
}

/// Every hidden field, in document order. A field without a `value` attribute counts as
/// present and empty — `focus`, `stz`, `source` and `selected` are empty on every page.
fn hidden_fields(form: ElementRef<'_>) -> Vec<(String, String)> {
    form.select(&selectors().hidden)
        .filter_map(|input| {
            let name = input.value().attr("name")?;
            let value = input.value().attr("value").unwrap_or_default();
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// Every submit button, in document order. A button without a `data-fld` is kept with an
/// empty `focus`: the page decides what `focus` means, and dropping the button would hide
/// it from the caller entirely.
fn buttons(form: ElementRef<'_>) -> Vec<Button> {
    form.select(&selectors().submit)
        .filter_map(|input| {
            let element = input.value();
            Some(Button {
                name: element.attr("name")?.to_string(),
                label: element.attr("value").unwrap_or_default().trim().to_string(),
                focus: element.attr("data-fld").unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// The compiled selectors. Every one is a literal in this file, so a parse failure would
/// be a typo in this source and nothing a user or the service can cause.
struct Selectors {
    form: Selector,
    hidden: Selector,
    submit: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        form: compile("form[name=\"Form0\"]"),
        hidden: compile("input[type=\"hidden\"]"),
        submit: compile("input[type=\"submit\"]"),
    })
}

/// A missing-selector error naming what stopped matching and at which step of the
/// session — the step is what says *which* page stopped carrying the form.
fn missing_selector(selector: &str, step: &str) -> Error {
    super::missing_selector(selector, &format!("{DOCUMENT} ({step})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = include_str!("../../../../tests/fixtures/voebb/start.html");
    const RESULTS: &str = include_str!("../../../../tests/fixtures/voebb/results.html");
    const ADVANCED: &str = include_str!("../../../../tests/fixtures/voebb/advanced_form.html");

    fn parsed(html: &str) -> FormState {
        match parse_form(html, "test") {
            Ok(state) => state,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    /// The entry page: session id in the action path, nine hidden fields, `requestCount`
    /// at 1.
    #[test]
    fn reads_the_entry_form() {
        let form = parsed(START);
        assert_eq!(
            form.action,
            "/aDISWeb/_21g4ad7n7gpvxuzq2ryo9pn9w87rqtq7/app"
        );
        assert_eq!(form.get(fields::IDENTITY), Some("5faUoXJ8MCxJfwi-"));
        assert_eq!(form.request_count(), Some(1));
    }

    /// All nine hidden fields, in document order — the order aDISWeb's own form has.
    #[test]
    fn keeps_the_nine_hidden_fields_in_document_order() {
        let form = parsed(START);
        let names: Vec<&str> = form
            .fields
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| fields::HIDDEN.contains(name))
            .collect();
        assert_eq!(names, fields::HIDDEN.to_vec());
    }

    /// Four of the nine are empty on every page. Empty is a value, not an absence.
    #[test]
    fn empty_fields_are_present_and_empty() {
        let form = parsed(START);
        for name in [fields::FOCUS, "stz", fields::SOURCE, "selected"] {
            assert_eq!(form.get(name), Some(""), "{name}");
        }
    }

    /// `identity` differs between two pages of the same session, and `requestCount`
    /// counts up. This is why the state is re-read rather than carried forward.
    #[test]
    fn identity_and_request_count_advance_with_the_session() {
        let start = parsed(START);
        let results = parsed(RESULTS);
        assert_ne!(start.get(fields::IDENTITY), results.get(fields::IDENTITY));
        assert_eq!(results.request_count(), Some(2));
    }

    /// The `focus` a button needs is its `data-fld`, and it differs per page: *Erweiterte
    /// Suche* is `$$GFBO_3` on the entry page and `$$GFBO_7` on the result page.
    #[test]
    fn button_focus_is_read_from_the_page() {
        let start = parsed(START);
        let results = parsed(RESULTS);
        let on_start = start
            .button_labelled(fields::label::ADVANCED)
            .expect("the entry page offers the advanced search");
        let on_results = results
            .button_labelled(fields::label::ADVANCED)
            .expect("the result page offers the advanced search");
        assert_eq!(on_start.focus, "$$GFBO_3");
        assert_eq!(on_results.focus, "$$GFBO_7");
        assert_eq!(on_start.name, on_results.name, "both are $Button$0");
    }

    /// The trap that makes a hard-wired button name wrong: `$Button$6` submits the
    /// advanced search on one page and sorts by author on the other.
    #[test]
    fn the_same_button_name_means_different_things_per_page() {
        let advanced = parsed(ADVANCED);
        let results = parsed(RESULTS);
        assert_eq!(
            advanced.button("$Button$6").map(|b| b.label.as_str()),
            Some(fields::label::SEARCH)
        );
        assert_eq!(
            results.button("$Button$6").map(|b| b.label.as_str()),
            Some("Verfasser")
        );
    }

    /// The facet's *Filtern* button, which the branch filter presses.
    #[test]
    fn finds_the_filter_button_on_the_result_page() {
        let results = parsed(RESULTS);
        let filter = results
            .button_labelled(fields::label::FILTER)
            .expect("a result page offers the facet filter");
        assert_eq!(filter.name, "$Button$2");
        assert_eq!(filter.focus, "$$GFBO_11");
    }

    /// The advanced form's four rows exist under the names the client sends.
    #[test]
    fn the_advanced_form_has_the_four_documented_rows() {
        let document = Html::parse_document(ADVANCED);
        for name in fields::ADVANCED_INDEX
            .iter()
            .chain(fields::ADVANCED_TERM.iter())
            .chain(fields::ADVANCED_JOIN.iter())
        {
            let selector = compile(&format!("[name=\"{name}\"]"));
            assert!(
                document.select(&selector).next().is_some(),
                "the advanced form has no {name}"
            );
        }
    }

    /// A form without its `identity` field is an error naming that field, not a state
    /// with eight fields that fails two requests later.
    #[test]
    fn a_missing_hidden_field_names_itself() {
        let broken = START.replace("name=\"identity\"", "name=\"identityX\"");
        let error = parse_form(&broken, "the entry page").expect_err("must not be accepted");
        let message = error.to_string();
        assert!(message.contains("identity"), "{message}");
        assert!(message.contains("the entry page"), "{message}");
    }

    /// A page without the form at all names the form selector.
    #[test]
    fn a_missing_form_names_the_form_selector() {
        let error = parse_form("<html><body>nothing</body></html>", "the entry page")
            .expect_err("must not be accepted");
        assert!(error.to_string().contains("Form0"), "{error}");
    }
}
