//! The aDISWeb session, as a value.
//!
//! Every request to voebb.de replays the complete form state of the page it came from:
//! the action URL and the nine hidden fields, `identity` and `requestCount` among them.
//! Both of those are **single-use** — `identity` changes with every response and
//! `requestCount` counts up — and getting either wrong produces a `/noaccess` page with
//! status 200.
//!
//! That is why the session is a value that is threaded through the client and **replaced
//! wholesale** after every answer ([`Session::advance`]) rather than something written by
//! hand: nothing here increments a counter or reuses a token, because the next correct
//! values are always in the page that just arrived.

use super::parse::form::{Button, FormState, fields};

/// The form state of the last page received, ready to be replayed.
///
/// The buttons belong to the state as much as the fields do: a button's `focus` value is
/// its `data-fld` and differs from page to page, and the same field name means different
/// things on different pages ( *Erweiterte Suche* is `$Button$0` here and `$Button$6`
/// there). They are read from the page that is about to be replayed, never hard-wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The form's `action`, an absolute path carrying the session id.
    pub action: String,
    /// The hidden fields of the last response, name and value, in document order.
    pub fields: Vec<(String, String)>,
    /// The submit buttons the last response offered.
    pub buttons: Vec<Button>,
}

impl Session {
    /// Start a session from the form of the page that opened it.
    pub fn from_form(form: FormState) -> Self {
        Self {
            action: form.action,
            fields: form.fields,
            buttons: form.buttons,
        }
    }

    /// Replace the state with the form of the answer that just arrived.
    ///
    /// A complete replacement, never a patch: `identity` is new, `requestCount` has moved
    /// on, the buttons have new `data-fld` values, and the action path can carry a
    /// different session id when the server decided to open one. Carrying anything
    /// forward is how a session is lost.
    pub fn advance(&mut self, form: FormState) {
        self.action = form.action;
        self.fields = form.fields;
        self.buttons = form.buttons;
    }

    /// The **first** button with this caption, in document order. `None` when the page
    /// does not offer it — which the client turns into an error naming the caption,
    /// because a button that vanished is a changed site and not an empty result.
    pub fn button_labelled(&self, label: &str) -> Option<&Button> {
        self.buttons.iter().find(|button| button.label == label)
    }

    /// The **last** button with this caption.
    ///
    /// Every page carries the site's own search box at the top, and its button is called
    /// *Suchen* like the one that submits the form the user is actually looking at. The
    /// header's comes first in the document, the page's own comes after it — so the
    /// advanced form's *Suchen* (`$Button$6`, measured) is the last one and the entry
    /// page's only one is both.
    pub fn last_button_labelled(&self, label: &str) -> Option<&Button> {
        self.buttons.iter().rfind(|button| button.label == label)
    }

    /// The fields for the next request: everything the last page carried, plus `extra`.
    ///
    /// An entry of `extra` **replaces** a hidden field of the same name instead of being
    /// appended next to it — `source` and `focus` exist on every page with an empty value,
    /// and sending `source=&source=$B` leaves it to the server which of the two it
    /// believes. Several `extra` entries with the same name are all kept, in the given
    /// order: one `$CbTree_text` per ticked facet box is exactly that case.
    pub fn form(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut fields: Vec<(String, String)> = self
            .fields
            .iter()
            .filter(|(name, _)| !extra.iter().any(|(key, _)| key == name))
            .cloned()
            .collect();
        fields.extend(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        );
        fields
    }

    /// The page's `requestCount`, when it is a number. For the counter-check that the
    /// session actually advanced; the field itself is always replayed verbatim.
    pub fn request_count(&self) -> Option<u32> {
        self.fields
            .iter()
            .find(|(name, _)| name == fields::REQUEST_COUNT)
            .and_then(|(_, value)| value.trim().parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::voebb::parse::form::parse_form;

    const START: &str = include_str!("../../../tests/fixtures/voebb/start.html");
    const RESULTS: &str = include_str!("../../../tests/fixtures/voebb/results.html");
    const ADVANCED: &str = include_str!("../../../tests/fixtures/voebb/advanced_form.html");

    fn session(html: &str) -> Session {
        let form = match parse_form(html, "test") {
            Ok(form) => form,
            Err(error) => panic!("fixture must parse: {error}"),
        };
        Session::from_form(form)
    }

    fn value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The action carries the session id, and the nine hidden fields come along.
    #[test]
    fn a_session_is_the_action_and_the_hidden_fields() {
        let session = session(START);
        assert_eq!(
            session.action,
            "/aDISWeb/_21g4ad7n7gpvxuzq2ryo9pn9w87rqtq7/app"
        );
        assert_eq!(session.fields.len(), fields::HIDDEN.len());
        assert_eq!(session.request_count(), Some(1));
    }

    /// Two buttons say *Suchen* on the advanced form: the site's header search box first,
    /// the form's own submit after it. Pressing the first would run an empty free-text
    /// search and throw the filled-in rows away.
    #[test]
    fn the_last_button_of_a_caption_is_the_pages_own() {
        let advanced = session(ADVANCED);
        let header = advanced
            .button_labelled(fields::label::SEARCH)
            .expect("every page carries the header search box");
        let own = advanced
            .last_button_labelled(fields::label::SEARCH)
            .expect("the advanced form submits itself");
        assert_eq!(header.name, "$Button");
        assert_eq!(own.name, "$Button$6");
        assert_eq!(own.focus, "$$GFBO_4");
    }

    /// On a page with one such button the two lookups are the same button.
    #[test]
    fn one_button_of_a_caption_is_both_the_first_and_the_last() {
        let start = session(START);
        assert_eq!(
            start.button_labelled(fields::label::SEARCH),
            start.last_button_labelled(fields::label::SEARCH)
        );
    }

    /// The buttons come with the state, because their `focus` differs per page.
    #[test]
    fn a_session_keeps_the_buttons_of_its_page() {
        let start = session(START);
        let results = session(RESULTS);
        let on_start = start
            .button_labelled(fields::label::ADVANCED)
            .expect("the entry page offers the advanced search");
        let on_results = results
            .button_labelled(fields::label::ADVANCED)
            .expect("the result page offers the advanced search");
        assert_eq!(on_start.focus, "$$GFBO_3");
        assert_eq!(on_results.focus, "$$GFBO_7");
    }

    /// Everything the page carried goes back out, and the extras are appended.
    #[test]
    fn the_next_form_replays_every_hidden_field() {
        let session = session(START);
        let form = session.form(&[(fields::SEARCH_TERMS, "Der Vorleser")]);
        for name in fields::HIDDEN {
            assert!(value(&form, name).is_some(), "{name} was dropped");
        }
        assert_eq!(value(&form, fields::SEARCH_TERMS), Some("Der Vorleser"));
    }

    /// `source` and `focus` are present and empty on every page. An extra replaces them
    /// rather than doubling them — two values for one field is a decision left to the
    /// server, and the server answers such things with `/noaccess`.
    #[test]
    fn an_extra_replaces_a_hidden_field_of_the_same_name() {
        let session = session(RESULTS);
        let form = session.form(&[
            (fields::SOURCE, fields::SOURCE_BUTTON),
            (fields::FOCUS, "$$GFBO_11"),
        ]);
        assert_eq!(
            form.iter()
                .filter(|(name, _)| name == fields::SOURCE)
                .count(),
            1
        );
        assert_eq!(value(&form, fields::SOURCE), Some(fields::SOURCE_BUTTON));
        assert_eq!(value(&form, fields::FOCUS), Some("$$GFBO_11"));
    }

    /// Two extras of the same name are both sent: one `$CbTree_text` per ticked box.
    #[test]
    fn several_extras_of_one_name_are_all_kept() {
        let session = session(RESULTS);
        let form = session.form(&[
            (fields::FACET_CHECKBOX, "sub-PTL1_tree_1_90"),
            (fields::FACET_CHECKBOX, "sub-PTL1_tree_1_92"),
        ]);
        let ticked: Vec<&str> = form
            .iter()
            .filter(|(name, _)| name == fields::FACET_CHECKBOX)
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(ticked, ["sub-PTL1_tree_1_90", "sub-PTL1_tree_1_92"]);
    }

    /// The state is replaced, not patched: `identity` and `requestCount` of the new page
    /// are the only ones that will be accepted.
    #[test]
    fn advancing_replaces_the_whole_state() {
        let mut session = session(START);
        let identity = value(&session.fields, fields::IDENTITY).map(str::to_owned);
        let form = match parse_form(RESULTS, "test") {
            Ok(form) => form,
            Err(error) => panic!("fixture must parse: {error}"),
        };
        session.advance(form);
        assert_ne!(
            value(&session.fields, fields::IDENTITY).map(str::to_owned),
            identity
        );
        assert_eq!(session.request_count(), Some(2));
    }
}
