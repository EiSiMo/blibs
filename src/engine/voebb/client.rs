//! voebb.de I/O: open a session, search, filter by branch, page, fetch a record.
//!
//! Every response passes [`super::parse::noaccess::check`] before any other parser sees
//! it. Cookies live in the shared `ureq` agent; **nothing here is cached** — a session
//! step is worthless a minute later and an item table is a statement about right now.
//!
//! Two rules shape every method:
//!
//! - **A request replays the page it came from.** The hidden fields, the pressed button's
//!   own name and its `data-fld` as `focus` — all read from the last answer, never
//!   written by hand ([`Session`]).
//! - **The visible fields are replayed too**, the way a browser would send them: the
//!   search box and the scope select ride along on the facet filter and on every further
//!   page, which is exactly what the measured recipe in `plan/voebb.md` § *Hausfacette*
//!   does.
//!
//! Everything runs **sequentially on one session**: the session is a state, and
//! concurrency on it is undefined. The transport agrees — voebb.de is capped at one
//! request in flight ([`crate::http::limit`]), so even the stateless record pages of
//! [`super::Voebb::fill_availability`] go out one at a time.

use crate::error::{Error, UnexpectedError};
use crate::http::{CachePolicy, Fetch, Replay, Request};
use crate::libraries::Branch;
use crate::model::{Identifier, Note, QuerySpec, RecordId, Term, note_kinds};

use super::parse::detail::{self, DetailPage};
use super::parse::facet::{self, BranchFacet};
use super::parse::form::{self, Button, fields};
use super::parse::noaccess;
use super::parse::results::{self, ResultPage};
use super::session::Session;

/// The public entry point of the search form.
pub const VOEBB_BASE: &str = "https://www.voebb.de";

/// The record page, which needs no session at all — only a cookie handshake.
const RECORD_PATH: &str = "/aDISWeb/app/prod00";

/// The query parameter the record page is addressed by. It appears twice: the product
/// first, the record second.
const RECORD_PARAM: &str = "sp";

/// The product the record page belongs to.
const RECORD_PRODUCT: &str = "SPROD00";

/// How many search rows the advanced form offers. Derived from the field names rather
/// than written out again: a fifth row would need a fifth `$Select$n`, and two numbers
/// that must agree are one number too many.
const ADVANCED_ROWS: usize = fields::ADVANCED_INDEX.len();

/// What names this client in an error message.
const DOCUMENT: &str = "voebb.de";

/// One answered result page: the rows, the branch facet that came with them, and the
/// visible fields that have to be replayed on every further request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultView {
    /// The rows, the stated total and the paging state.
    pub page: ResultPage,
    /// The branch facet of this page. Empty when the page states no hits — a search
    /// without hits has no tree to offer, and that is not a broken page.
    pub facet: BranchFacet,
    /// The search box and the scope select as this page carries them.
    replay: Vec<(String, String)>,
}

/// A session-bound client.
pub struct VoebbClient<'f> {
    fetch: &'f dyn Fetch,
}

impl<'f> VoebbClient<'f> {
    /// Build the client over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self { fetch }
    }

    /// The transport this client was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.fetch
    }

    /// Fetch the entry page and read the initial form state out of it.
    ///
    /// One `GET` on the bare host; the server redirects twice and sets the session
    /// cookies on the way, which the shared agent keeps.
    pub fn open(&self) -> Result<Session, Error> {
        let step = "the entry page";
        let html = self.send(Request::get(format!("{VOEBB_BASE}/")), step)?;
        Ok(Session::from_form(form::parse_form(&html, step)?))
    }

    /// Run a search and read the first page of results.
    ///
    /// Free terms alone go through the single search box. As soon as `--title`,
    /// `--author`, `--subject` or `--isbn` is involved the advanced form is opened first
    /// (one extra request) and one row per flag is filled in, joined with `UND`.
    ///
    /// The returned notes state what the site could not be asked exactly as the user
    /// wrote it: the form has no free-text index and only four rows. They are notes and
    /// not errors — the search still ran, and silently narrowing it would be the worse
    /// answer.
    pub fn search(
        &self,
        session: &mut Session,
        query: &QuerySpec,
    ) -> Result<(ResultView, Vec<Note>), Error> {
        let (rows, notes) = advanced_rows(query);
        let view = if rows.is_empty() {
            self.simple_search(session, &free_terms(query))?
        } else {
            self.advanced_search(session, &rows)?
        };
        Ok((view, notes))
    }

    /// The single search box on the entry page: scope, terms, *Suchen*.
    fn simple_search(&self, session: &mut Session, terms: &str) -> Result<ResultView, Error> {
        let replay = replay_fields(terms);
        let html = self.press(
            session,
            fields::label::SEARCH,
            &borrowed(&replay),
            "the search",
        )?;
        result_view(session, &html, replay, "the search")
    }

    /// The advanced form: open it, fill the rows, submit it.
    ///
    /// Two requests, because the form does not exist until it has been asked for, and its
    /// row fields would be meaningless on any other page.
    fn advanced_search(
        &self,
        session: &mut Session,
        rows: &[(&'static str, String)],
    ) -> Result<ResultView, Error> {
        // The advanced form is opened with the scope already set, so that the search it
        // submits searches the network's holdings and not "everywhere".
        let opening = replay_fields("");
        let step = "the advanced search form";
        let html = self.press(session, fields::label::ADVANCED, &borrowed(&opening), step)?;
        session.advance(form::parse_form(&html, step)?);

        let mut filled: Vec<(String, String)> = opening.clone();
        for (position, (index, value)) in rows.iter().enumerate() {
            let Some(name) = fields::ADVANCED_INDEX.get(position) else {
                break;
            };
            let Some(term) = fields::ADVANCED_TERM.get(position) else {
                break;
            };
            filled.push(((*name).to_string(), (*index).to_string()));
            filled.push(((*term).to_string(), value.clone()));
            if let Some(join) = fields::ADVANCED_JOIN.get(position) {
                filled.push(((*join).to_string(), fields::join::AND.to_string()));
            }
        }
        // The advanced form's own *Suchen*, not the header search box's, which comes
        // first in the document and would run an empty free-text search instead.
        let html = self.press_last(
            session,
            fields::label::SEARCH,
            &borrowed(&filled),
            "the advanced search",
        )?;
        // The result page carries an empty search box, so that is what is replayed from
        // here on — the same thing a browser would send.
        result_view(session, &html, opening, "the advanced search")
    }

    /// Apply the branch facet.
    ///
    /// `Ok(None)` means the facet does not list this branch, which is the site's way of
    /// saying it has **no hits there** — the tree only carries branches that have some.
    /// That is an honest answer and not an error; the caller reports a total of zero.
    pub fn filter_branch(
        &self,
        session: &mut Session,
        view: &ResultView,
        branch: &Branch,
    ) -> Result<Option<ResultView>, Error> {
        let Some(entry) = view.facet.lookup(branch)? else {
            return Ok(None);
        };
        let checkbox = entry.checkbox_id.clone();
        let mut extra = borrowed(&view.replay);
        extra.push((fields::FACET_CHECKBOX, checkbox.as_str()));

        let step = "the branch filter";
        let html = self.press(session, fields::label::FILTER, &extra, step)?;
        result_view(session, &html, view.replay.clone(), step).map(Some)
    }

    /// Ask for the next page of the current result list.
    ///
    /// `$Toolbar=3` — one field carrying the button index as its value, never
    /// `$Toolbar_3=…`. The facet filter survives paging and is deliberately **not** sent
    /// again.
    pub fn next_page(&self, session: &mut Session, view: &ResultView) -> Result<ResultView, Error> {
        let next = fields::toolbar::NEXT.to_string();
        let mut extra = borrowed(&view.replay);
        extra.push((fields::TOOLBAR, next.as_str()));

        let step = "the next result page";
        let payload = session.form(&extra);
        let html = self.post(session, payload, step)?;
        result_view(session, &html, view.replay.clone(), step)
    }

    /// Fetch one record's detail page.
    ///
    /// **Stateless**: the record URL works without a session and without any form state,
    /// which is what makes `show voebb_…` a single request. It does need the cookie
    /// handshake — without a jar the server answers 302 onto the same URL with an empty
    /// body — and the shared agent provides it by following the redirect with its cookies
    /// attached.
    ///
    /// `Ok(None)` means the catalogue holds no such record. Measured 2026-09-06 with
    /// `sp=SAK00000000`: voebb.de answers an unknown number with **its search entry
    /// page** — status 200, a valid form, and neither the record's `div#R03` nor a single
    /// `table.gi`. Anything else that lacks those tables is a changed site and comes back
    /// as an error naming the selector, never as "no such record".
    pub fn detail(&self, id: &RecordId) -> Result<Option<DetailPage>, Error> {
        let step = "the record page";
        let request = Request::get(format!("{VOEBB_BASE}{RECORD_PATH}"))
            .query(RECORD_PARAM, RECORD_PRODUCT)
            .query(RECORD_PARAM, id.local_id());
        let html = self.send(request, step)?;
        if detail::is_missing_record(&html) {
            return Ok(None);
        }
        detail::parse_detail(&html, id).map(Some)
    }

    /// Press the page's first button with this caption.
    ///
    /// The button is looked up by its caption and everything about it comes from the page:
    /// its field name, and its `data-fld`, which is what `focus` has to carry. A caption
    /// the page does not offer is an error naming it — the site changed, and carrying on
    /// would burn the session on a request nobody can explain afterwards.
    fn press(
        &self,
        session: &mut Session,
        label: &'static str,
        extra: &[(&str, &str)],
        step: &'static str,
    ) -> Result<String, Error> {
        let button = session
            .button_labelled(label)
            .ok_or_else(|| missing_button(label, step))?
            .clone();
        self.press_button(session, &button, extra, step)
    }

    /// Press the page's **last** button with this caption — the form's own, rather than
    /// the site's header search box, which carries the same caption on every page.
    fn press_last(
        &self,
        session: &mut Session,
        label: &'static str,
        extra: &[(&str, &str)],
        step: &'static str,
    ) -> Result<String, Error> {
        let button = session
            .last_button_labelled(label)
            .ok_or_else(|| missing_button(label, step))?
            .clone();
        self.press_button(session, &button, extra, step)
    }

    /// Submit the form with one button pressed: its own name, `source=$B` and the
    /// `focus` the button's `data-fld` states.
    fn press_button(
        &self,
        session: &mut Session,
        button: &Button,
        extra: &[(&str, &str)],
        step: &'static str,
    ) -> Result<String, Error> {
        let mut all: Vec<(&str, &str)> = extra.to_vec();
        all.push((button.name.as_str(), fields::PRESSED));
        all.push((fields::SOURCE, fields::SOURCE_BUTTON));
        all.push((fields::FOCUS, button.focus.as_str()));

        let payload = session.form(&all);
        self.post(session, payload, step)
    }

    /// Send one form submission and hand back the body.
    ///
    /// Every `POST` here answers 303 and the answer is behind the `Location`; the
    /// transport follows it. A body that comes back as `/noaccess` is a lost session and
    /// is refused before any other parser sees it.
    ///
    /// **This is the one place in the crate that knows a request may not be sent twice**,
    /// and [`Replay::SingleUse`] is how it says so. The session's `requestCount` is
    /// consumed by the *sending*, not by the answering: a submission that times out has
    /// still spent it, and replaying the same payload is a request voebb.de answers with
    /// `/noaccess` by design (`plan/voebb.md` § *Das Absendeprotokoll*). A transport
    /// failure here is therefore reported as what it is — the host did not answer in time
    /// — instead of being converted into a lost session by blibs's own second attempt
    /// (`plan/feedback_round_3.md` §1.1).
    ///
    /// The `GET`s in this client stay repeatable: [`VoebbClient::open`] would simply start
    /// another session, and [`VoebbClient::detail`] addresses a stateless URL.
    fn post(
        &self,
        session: &Session,
        payload: Vec<(String, String)>,
        step: &'static str,
    ) -> Result<String, Error> {
        let mut request = Request::post(format!("{VOEBB_BASE}{}", session.action));
        request.form = payload;
        self.send(request.replay(Replay::SingleUse), step)
    }

    /// Execute one request, never cached, and check it for the failure page.
    fn send(&self, request: Request, step: &str) -> Result<String, Error> {
        let response = self.fetch.fetch(&request.cache(CachePolicy::Never))?;
        noaccess::check(&response.body, step)?;
        Ok(response.body)
    }
}

/// Parse a result page, advance the session onto it, and keep its facet.
fn result_view(
    session: &mut Session,
    html: &str,
    replay: Vec<(String, String)>,
    step: &'static str,
) -> Result<ResultView, Error> {
    let page = results::parse_results(html)?;
    session.advance(page.form.clone());
    let facet = branch_facet(html, page.total, step)?;
    Ok(ResultView {
        page,
        facet,
        replay,
    })
}

/// The branch facet of a result page.
///
/// A page that states no hits carries no tree, and that is not a missing selector: there
/// is nothing to offer a facet over. With hits, an absent tree *is* an error — reporting
/// "this branch has nothing" because the tree stopped being found would be the worst
/// possible answer.
fn branch_facet(html: &str, total: u64, step: &'static str) -> Result<BranchFacet, Error> {
    if total == 0 {
        return Ok(BranchFacet {
            entries: Vec::new(),
        });
    }
    facet::parse_facet(html).map_err(|error| match error {
        Error::Unexpected(UnexpectedError::MissingSelector { selector, .. }) => {
            UnexpectedError::MissingSelector {
                selector,
                document: format!("{DOCUMENT} ({step})"),
            }
            .into()
        }
        other => other,
    })
}

/// The visible fields of the search area, as a browser would send them.
fn replay_fields(terms: &str) -> Vec<(String, String)> {
    vec![
        (fields::SEARCH_TERMS.to_string(), terms.to_string()),
        (
            fields::SEARCH_SCOPE.to_string(),
            fields::SCOPE_HOLDINGS.to_string(),
        ),
    ]
}

/// Borrow an owned field list for [`Session::form`].
fn borrowed(fields: &[(String, String)]) -> Vec<(&str, &str)> {
    fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The free terms of a query as one search string.
fn free_terms(query: &QuerySpec) -> String {
    query
        .terms
        .iter()
        .map(Term::text)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rows of the advanced form, and what could not be expressed exactly.
///
/// The form has four rows and an index vocabulary with **no free-text entry** (measured:
/// `Titel`, `Titelanfang`, `Titel präzis`, `Person`, `Schlagwort`, `ISBN, ISSN, ISMN`,
/// `Quelle/Serie/Zeitschrift`, `Institution`, `Systematikgruppe`, `Systematiktitel`).
/// Two consequences, and both are stated in a note rather than hidden:
///
/// - free terms next to a field flag are searched as a **title**, because that is the
///   closest index the form has;
/// - a query that needs more than four rows loses the last of them, the least selective
///   first.
///
/// **The notes describe what was sent, not what was intended**, which is why the row list
/// is cut to length *before* either of them is worded. Announcing the free-term row first
/// and truncating it away afterwards produced two notes about one query that contradicted
/// each other — "`Sommer` was searched as a title" next to "`Titel = "Sommer"` was not
/// sent" (round 2, §1.13). The free-term row is the last one pushed and therefore the
/// first one dropped, so it is exactly the row that used to be announced in vain.
///
/// An empty result means the query is free terms only and belongs in the single search
/// box instead.
fn advanced_rows(query: &QuerySpec) -> (Vec<(&'static str, String)>, Vec<Note>) {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(identifier) = &query.identifier {
        let value = match identifier {
            Identifier::Isbn(digits) | Identifier::Issn(digits) => digits.clone(),
        };
        rows.push((fields::index::IDENTIFIER, value));
    }
    if let Some(title) = &query.title {
        rows.push((fields::index::TITLE, title.text().to_string()));
    }
    if let Some(author) = &query.author {
        rows.push((fields::index::PERSON, author.clone()));
    }
    if let Some(subject) = &query.subject {
        rows.push((fields::index::SUBJECT, subject.text().to_string()));
    }

    // Free terms on their own belong in the single search box, which is what an empty
    // row list tells the caller. Next to a flag they have nowhere else to go.
    let terms = free_terms(query);
    let free_row = (!rows.is_empty() && !terms.is_empty()).then(|| {
        rows.push((fields::index::TITLE, terms.clone()));
        rows.len() - 1
    });

    let truncated = (rows.len() > ADVANCED_ROWS).then(|| {
        let dropped: Vec<String> = rows
            .split_off(ADVANCED_ROWS)
            .into_iter()
            .map(|(index, value)| format!("{index} = {value:?}"))
            .collect();
        Note::new(
            note_kinds::VOEBB_QUERY_TRUNCATED,
            format!(
                "voebb.de's advanced search has {ADVANCED_ROWS} rows, so {} was not sent",
                dropped.join(", ")
            ),
        )
    });

    // Only now, and only if that row survived the cut: the note states what the form was
    // actually asked, and a row that was dropped is the truncation note's business alone.
    let mut notes = Vec::new();
    if free_row.is_some_and(|position| position < rows.len()) {
        notes.push(Note::new(
            note_kinds::VOEBB_FREE_TERMS_AS_TITLE,
            format!(
                "voebb.de's advanced search has no free-text index, so {terms:?} was \
                 searched as a title next to the other fields"
            ),
        ));
    }
    notes.extend(truncated);
    (rows, notes)
}

/// A button the page stopped offering.
fn missing_button(label: &str, step: &str) -> Error {
    UnexpectedError::MissingSelector {
        selector: format!("input[type=\"submit\"][value=\"{label}\"]"),
        document: format!("{DOCUMENT} ({step})"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::http::{Method, Response};
    use crate::model::Term;

    const START: &str = include_str!("../../../tests/fixtures/voebb/start.html");
    const RESULTS: &str = include_str!("../../../tests/fixtures/voebb/results.html");

    /// A transport that answers the entry page and then the result list, and keeps every
    /// request it was handed.
    struct Recording {
        seen: Mutex<Vec<Request>>,
    }

    impl Fetch for Recording {
        fn fetch(&self, request: &Request) -> Result<Response, Error> {
            let body = match request.method {
                Method::Get => START,
                Method::Post => RESULTS,
            };
            crate::http::lock(&self.seen).push(request.clone());
            Ok(Response {
                status: 200,
                body: body.to_owned(),
                content_type: None,
                from_cache: false,
            })
        }
    }

    /// The session's `requestCount` is spent by the *sending*, so a form submission that
    /// fails in transit may not be sent again — voebb.de answers the replay with
    /// `/noaccess` by design (`plan/voebb.md` § *Das Absendeprotokoll*). The client is the
    /// only place that knows this, and this is the assertion that it still says so
    /// (`plan/feedback_round_3.md` §1.1).
    #[test]
    fn every_form_submission_is_declared_single_use() {
        let transport = Recording {
            seen: Mutex::new(Vec::new()),
        };
        let client = VoebbClient::new(&transport);
        let mut session = client.open().expect("the entry page fixture parses");
        client
            .search(&mut session, &spec(&["Der Vorleser"]))
            .expect("the result fixture parses");

        let seen = crate::http::lock(&transport.seen);
        let posts: Vec<&Request> = seen
            .iter()
            .filter(|request| request.method == Method::Post)
            .collect();
        assert!(!posts.is_empty(), "the search has to submit a form");
        for request in posts {
            assert_eq!(
                request.replay,
                Replay::SingleUse,
                "a form submission that may be replayed burns the session: {request:?}"
            );
        }
    }

    /// The `GET`s stay repeatable: the entry page would simply start another session, and
    /// the record page is a stateless URL. Losing that would turn every hiccup on a
    /// stateless request into a failed invocation for no reason.
    #[test]
    fn the_stateless_requests_stay_repeatable() {
        let transport = Recording {
            seen: Mutex::new(Vec::new()),
        };
        let client = VoebbClient::new(&transport);
        client.open().expect("the entry page fixture parses");
        // The record page: the fixture answers it with the entry page, which is exactly
        // what an unknown record looks like, so the request is what matters here.
        let _ = client.detail(&RecordId::voebb("SAK13776205"));

        let seen = crate::http::lock(&transport.seen);
        let gets: Vec<&Request> = seen
            .iter()
            .filter(|request| request.method == Method::Get)
            .collect();
        assert_eq!(gets.len(), 2, "the entry page and the record page");
        for request in gets {
            assert_eq!(request.replay, Replay::Repeatable, "{request:?}");
        }
    }

    fn spec(terms: &[&str]) -> QuerySpec {
        QuerySpec {
            terms: terms.iter().map(|t| Term::from_argument(t)).collect(),
            ..QuerySpec::default()
        }
    }

    /// Free terms alone need no advanced form.
    #[test]
    fn free_terms_alone_use_the_single_search_box() {
        let (rows, notes) = advanced_rows(&spec(&["Der Vorleser"]));
        assert!(rows.is_empty());
        assert!(notes.is_empty());
    }

    /// One row per flag, joined with `UND`, in a fixed order.
    #[test]
    fn each_field_flag_becomes_one_row() {
        let query = QuerySpec {
            title: Some(Term::from_argument("Vorleser")),
            author: Some("Schlink".to_owned()),
            subject: Some(Term::from_argument("Roman")),
            identifier: Some(Identifier::Isbn("9783257229530".to_owned())),
            ..QuerySpec::default()
        };
        let (rows, notes) = advanced_rows(&query);
        assert_eq!(
            rows,
            vec![
                (fields::index::IDENTIFIER, "9783257229530".to_owned()),
                (fields::index::TITLE, "Vorleser".to_owned()),
                (fields::index::PERSON, "Schlink".to_owned()),
                (fields::index::SUBJECT, "Roman".to_owned()),
            ]
        );
        assert!(notes.is_empty());
    }

    /// The form has no free-text index, so free terms next to a flag become a title —
    /// and the note says so rather than letting the user believe otherwise.
    #[test]
    fn free_terms_next_to_a_flag_are_searched_as_a_title_with_a_note() {
        let query = QuerySpec {
            author: Some("Schlink".to_owned()),
            ..spec(&["Vorleser"])
        };
        let (rows, notes) = advanced_rows(&query);
        assert_eq!(
            rows,
            vec![
                (fields::index::PERSON, "Schlink".to_owned()),
                (fields::index::TITLE, "Vorleser".to_owned()),
            ]
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, note_kinds::VOEBB_FREE_TERMS_AS_TITLE);
    }

    /// Five rows do not fit into four. The least selective one goes, and it is named —
    /// and, since round 2 (§1.13), nothing claims it was searched as a title after all:
    /// the dropped row is exactly the free-term row, and two notes about one query that
    /// contradict each other are worse than one note that is short.
    #[test]
    fn a_query_that_needs_more_than_four_rows_says_what_was_dropped() {
        let query = QuerySpec {
            title: Some(Term::from_argument("Vorleser")),
            author: Some("Schlink".to_owned()),
            subject: Some(Term::from_argument("Roman")),
            identifier: Some(Identifier::Isbn("9783257229530".to_owned())),
            ..spec(&["Diogenes"])
        };
        let (rows, notes) = advanced_rows(&query);
        assert_eq!(rows.len(), ADVANCED_ROWS);
        let kinds: Vec<&str> = notes.iter().map(|note| note.kind).collect();
        assert!(
            kinds.contains(&note_kinds::VOEBB_QUERY_TRUNCATED),
            "{kinds:?}"
        );
        let truncated = notes
            .iter()
            .find(|note| note.kind == note_kinds::VOEBB_QUERY_TRUNCATED)
            .expect("the note exists");
        assert!(truncated.message.contains("Diogenes"), "{truncated:?}");
        assert!(
            !kinds.contains(&note_kinds::VOEBB_FREE_TERMS_AS_TITLE),
            "the row that was dropped is never also announced as sent: {kinds:?}"
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    /// The scope is always the network's holdings — never the entry page's default of
    /// "search everywhere", which would drag in interlibrary loan and article indexes.
    #[test]
    fn the_scope_is_always_the_networks_holdings() {
        let replay = replay_fields("Vorleser");
        assert_eq!(
            replay,
            vec![
                (fields::SEARCH_TERMS.to_owned(), "Vorleser".to_owned()),
                (
                    fields::SEARCH_SCOPE.to_owned(),
                    fields::SCOPE_HOLDINGS.to_owned()
                ),
            ]
        );
    }
}
