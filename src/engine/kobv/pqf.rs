//! Building the Z39.50 prefix query.
//!
//! PQF, not CQL. Same indexes, same hit counts, plus the one attribute CQL lacks: `1044`
//! (ISIL), which restricts a search to an institution upstream. Verified 2026-09-06,
//! `plan/scraping.md` §A.4a.
//!
//! Two rules keep this safe. **User input is never written raw into a query**: every term
//! is quoted, and a `"` inside a term is removed before quoting, which makes every other
//! punctuation mark harmless (measured with `@`, `{`, `\`). And **user input is never
//! quoted on the user's behalf**: a phrase is a phrase because the user's shell delivered
//! it as one argument, not because this module decided so.
//!
//! What does not exist upstream is caught in `cli`, not here: wildcards (diagnostic
//! 1/48), ranges (which return zero hits *silently*), sorting, and any index for material
//! type or language.

use crate::error::UsageError;
use crate::model::{Identifier, Isil, QuerySpec, RecordId, Term};

/// Bib-1 use attribute for the default "any" index.
pub const ATTR_ANY: u16 = 1016;
/// Bib-1 use attribute for title.
pub const ATTR_TITLE: u16 = 4;
/// Bib-1 use attribute for author.
pub const ATTR_AUTHOR: u16 = 1;
/// Bib-1 attribute *type* 4 (structure). Every other clause here uses type 1 (use), which
/// is why it is the only type spelled out in a constant of its own.
pub const ATTR_TYPE_STRUCTURE: u16 = 4;
/// Bib-1 structure attribute for a word list — what `--author` always uses.
pub const ATTR_STRUCTURE_WORDLIST: u16 = 6;
/// Bib-1 use attribute for subject.
pub const ATTR_SUBJECT: u16 = 21;
/// Bib-1 use attribute for publisher.
pub const ATTR_PUBLISHER: u16 = 59;
/// Bib-1 use attribute for date of publication.
pub const ATTR_YEAR: u16 = 31;
/// Bib-1 use attribute for ISBN.
pub const ATTR_ISBN: u16 = 7;
/// Bib-1 use attribute for ISSN.
pub const ATTR_ISSN: u16 = 8;
/// Bib-1 use attribute for a record identifier.
pub const ATTR_RECORD_ID: u16 = 12;
/// Bib-1 use attribute for ISIL — the holdings filter, and the reason this module exists.
/// **Case-sensitive**: `de-11` matches nothing, silently.
pub const ATTR_ISIL: u16 = 1044;

/// The largest query the service accepts before answering HTTP 414 as diagnostic 1/2.
pub const MAX_QUERY_CHARS: usize = 1000;

/// An assembled prefix query. Constructed only in this module, so there is exactly one
/// place where user input reaches the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pqf(String);

impl Pqf {
    /// The query string for the `x-pquery` parameter.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in characters, which is what the upstream length limit is measured in
    /// here — never `len()`, which would count an umlaut twice and reject a query the
    /// service would have accepted.
    pub fn char_len(&self) -> usize {
        self.0.chars().count()
    }

    /// Reject a query that would come back as diagnostic 1/2 (HTTP 414). The measured
    /// limit lies somewhere between ~1200 and ~2400 URL characters and was never pinned
    /// down; 1000 is the conservative cut-off from `plan/cql-verified.md` § *Leere und
    /// überlange Eingaben*.
    fn checked(self) -> Result<Self, UsageError> {
        let chars = self.char_len();
        if chars >= MAX_QUERY_CHARS {
            return Err(UsageError::QueryTooLong { chars });
        }
        Ok(self)
    }
}

impl std::fmt::Display for Pqf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Build the search query.
///
/// Every field of the spec that is set contributes one clause, in a fixed order (free
/// terms, title, author, subject, publisher, year, identifier), joined left-nested with
/// `@and`: three clauses become `@and @and a b c`. The locations become one `@or` over
/// `@attr 1=1044` clauses — also left-nested, so three ISILs are `@or @or A B C` — and
/// are `@and`-ed onto the right of everything else, exactly as measured. An empty
/// location list adds no clause at all.
///
/// The ISILs must already be the **canonical** spelling from the library list; the
/// attribute is case-sensitive and `de-11` matches nothing without saying so.
///
/// # Errors
///
/// [`UsageError::EmptyQuery`] when the spec contributes no clause *and* there are no
/// locations — an empty query is diagnostic 1/10 upstream. A filter without search terms
/// is a legitimate query (`@attr 1=1044 DE-B1533` answers with that library's whole
/// stock); `cli` rejects it earlier for its own reasons, this module does not.
/// [`UsageError::QueryTooLong`] when the assembled query reaches
/// [`MAX_QUERY_CHARS`].
pub fn search(spec: &QuerySpec, locations: &[Isil]) -> Result<Pqf, UsageError> {
    let mut clauses = spec_clauses(spec);
    if let Some(filter) = isil_filter(locations) {
        clauses.push(filter);
    }
    let query = join(&clauses, "@and").ok_or(UsageError::EmptyQuery)?;
    Pqf(query).checked()
}

/// Look one record up by its id.
///
/// The id is quoted like any user term although it is not free text: local ids carry
/// umlauts, `+` and percent-escapes (`kobvindex_VBRD-...rüt20in209+20`), and inside
/// `"…"` every one of those is harmless. Unquoted, a `=` or `/` in an id would be read
/// as syntax and the lookup would fail with a diagnostic instead of a hit.
pub fn record_lookup(id: &RecordId) -> Pqf {
    Pqf(use_attr(ATTR_RECORD_ID, &quote(id.as_str())))
}

/// One clause per field of the spec, in the order they are `@and`-ed.
fn spec_clauses(spec: &QuerySpec) -> Vec<String> {
    let mut clauses = Vec::new();
    for term in &spec.terms {
        push_term(&mut clauses, ATTR_ANY, term);
    }
    if let Some(title) = &spec.title {
        push_term(&mut clauses, ATTR_TITLE, title);
    }
    if let Some(author) = &spec.author {
        push_clause(&mut clauses, author, author_clause);
    }
    if let Some(subject) = &spec.subject {
        push_term(&mut clauses, ATTR_SUBJECT, subject);
    }
    if let Some(publisher) = &spec.publisher {
        push_term(&mut clauses, ATTR_PUBLISHER, publisher);
    }
    if let Some(year) = spec.year {
        // Never quoted: measured as `@attr 1=31 2020`, and a `u16` cannot carry syntax.
        clauses.push(use_attr(ATTR_YEAR, &year.to_string()));
    }
    if let Some(identifier) = &spec.identifier {
        let (attr, value) = match identifier {
            Identifier::Isbn(value) => (ATTR_ISBN, value),
            Identifier::Issn(value) => (ATTR_ISSN, value),
        };
        push_clause(&mut clauses, value, |value| use_attr(attr, &quote(value)));
    }
    clauses
}

/// A term clause. [`Term::Word`] and [`Term::Phrase`] are built identically — the
/// difference is already made: a phrase arrived as one shell argument and is therefore
/// one quoted term, a word arrived as its own argument and gets its own clause.
fn push_term(clauses: &mut Vec<String>, attr: u16, term: &Term) {
    let text = match term {
        Term::Word(text) | Term::Phrase(text) => text,
    };
    push_clause(clauses, text, |text| use_attr(attr, &quote(text)));
}

/// Add a clause unless the value is empty once its `"` are gone.
///
/// A term of nothing but quotes would become `""`, which upstream is an empty field value
/// and answers with diagnostic 1/10 for the *whole* query — one degenerate argument would
/// take the rest of the search with it. `cli` rejects empty search text before it gets
/// here; this is the guard for the case it cannot see.
fn push_clause(clauses: &mut Vec<String>, value: &str, build: impl FnOnce(&str) -> String) {
    if strip_quotes(value).trim().is_empty() {
        return;
    }
    clauses.push(build(value));
}

/// `--author` is never a phrase.
///
/// The author index holds authority forms: `"Kafka, Franz"` finds 2911 records, the
/// natural `"Franz Kafka"` as a phrase finds 40. Structure attribute `4=6` searches it as
/// an order-independent word list, which is the only spelling a user can be expected to
/// get right.
fn author_clause(author: &str) -> String {
    format!(
        "@attr 1={ATTR_AUTHOR} @attr {ATTR_TYPE_STRUCTURE}={ATTR_STRUCTURE_WORDLIST} {}",
        quote(author)
    )
}

/// The holdings filter: one `@attr 1=1044` per location, `@or`-ed together. `None` for no
/// locations, so the caller adds no clause rather than an always-true one.
fn isil_filter(locations: &[Isil]) -> Option<String> {
    let clauses: Vec<String> = locations
        .iter()
        // Never quoted: measured as `@attr 1=1044 DE-11`, and the value is the canonical
        // code from the library list, not user text.
        .map(|isil| use_attr(ATTR_ISIL, isil.as_str()))
        .collect();
    join(&clauses, "@or")
}

/// `@attr 1=<attr> <value>`, the shape every clause in this module has.
fn use_attr(attr: u16, value: &str) -> String {
    format!("@attr 1={attr} {value}")
}

/// Quote a term for the wire.
///
/// **Every** term is quoted, single words and numbers included, so that there is exactly
/// one quoting rule to reason about. Inside `"…"` every punctuation mark measured is
/// inert (`@`, `{`, `}`, `\`, `:`, `(`, `)`, `/`, `=`, `'`); outside them several of them
/// are syntax and turn the query into a diagnostic — or, worse, into a different search.
/// A `"` in the user's text is removed rather than escaped: an unbalanced one silently
/// flips the query between phrase and AND without any error.
fn quote(term: &str) -> String {
    format!("\"{}\"", strip_quotes(term))
}

/// The only thing removed from user text, ever.
fn strip_quotes(term: &str) -> String {
    term.replace('"', "")
}

/// Left-nested prefix combination: `[a, b, c]` becomes `@op @op a b c`.
///
/// Prefix notation has no precedence to get wrong, so the nesting direction is a free
/// choice; left is the one the measured queries use. `None` for no clauses — the callers
/// turn that into "no filter" and "empty query" respectively, which are different
/// answers.
fn join(clauses: &[String], op: &str) -> Option<String> {
    let mut parts = clauses.iter();
    let mut query = parts.next()?.clone();
    for clause in parts {
        query = format!("{op} {query} {clause}");
    }
    Some(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> Term {
        Term::Word(text.to_owned())
    }

    fn phrase(text: &str) -> Term {
        Term::Phrase(text.to_owned())
    }

    fn spec_with_terms(terms: Vec<Term>) -> QuerySpec {
        QuerySpec {
            terms,
            ..QuerySpec::default()
        }
    }

    fn query_of(spec: &QuerySpec) -> String {
        search(spec, &[])
            .expect("test specs are non-empty and short")
            .as_str()
            .to_owned()
    }

    fn isil(code: &str) -> Isil {
        Isil::new(code)
    }

    // --- the mapping table of `plan/cli.md`, one test per row ---

    #[test]
    fn free_words_are_quoted_one_by_one() {
        let spec = spec_with_terms(vec![word("Kafka"), word("Prozess")]);
        assert_eq!(
            query_of(&spec),
            r#"@and @attr 1=1016 "Kafka" @attr 1=1016 "Prozess""#
        );
    }

    #[test]
    fn a_shell_quoted_argument_stays_one_phrase() {
        let spec = spec_with_terms(vec![phrase("Der Prozess")]);
        assert_eq!(query_of(&spec), r#"@attr 1=1016 "Der Prozess""#);
    }

    #[test]
    fn title_uses_attribute_4() {
        let spec = QuerySpec {
            title: Some(phrase("Der Prozess")),
            ..QuerySpec::default()
        };
        assert_eq!(query_of(&spec), r#"@attr 1=4 "Der Prozess""#);
    }

    #[test]
    fn author_is_always_a_word_list_never_a_phrase() {
        let spec = QuerySpec {
            author: Some("Franz Kafka".to_owned()),
            ..QuerySpec::default()
        };
        assert_eq!(query_of(&spec), r#"@attr 1=1 @attr 4=6 "Franz Kafka""#);
    }

    #[test]
    fn locations_become_an_or_on_the_right() {
        let spec = spec_with_terms(vec![word("Kafka")]);
        let query = search(&spec, &[isil("DE-11"), isil("DE-1")]).expect("valid query");
        assert_eq!(
            query.as_str(),
            r#"@and @attr 1=1016 "Kafka" @or @attr 1=1044 DE-11 @attr 1=1044 DE-1"#
        );
    }

    #[test]
    fn year_goes_unquoted() {
        let spec = QuerySpec {
            year: Some(2020),
            ..QuerySpec::default()
        };
        assert_eq!(query_of(&spec), "@attr 1=31 2020");
    }

    #[test]
    fn isbn_uses_attribute_7() {
        let spec = QuerySpec {
            identifier: Some(Identifier::Isbn("9783596294312".to_owned())),
            ..QuerySpec::default()
        };
        assert_eq!(query_of(&spec), r#"@attr 1=7 "9783596294312""#);
    }

    #[test]
    fn issn_uses_attribute_8() {
        let spec = QuerySpec {
            identifier: Some(Identifier::Issn("0002-9769".to_owned())),
            ..QuerySpec::default()
        };
        assert_eq!(query_of(&spec), r#"@attr 1=8 "0002-9769""#);
    }

    // --- quoting ---

    #[test]
    fn quotes_in_user_text_are_removed() {
        let spec = spec_with_terms(vec![phrase(r#"Der "Prozess""#)]);
        assert_eq!(query_of(&spec), r#"@attr 1=1016 "Der Prozess""#);
    }

    #[test]
    fn punctuation_that_is_harmless_inside_quotes_survives() {
        let spec = spec_with_terms(vec![word("@Kafka"), word("{Kafka}"), word(r"Ka\fka")]);
        assert_eq!(
            query_of(&spec),
            r#"@and @and @attr 1=1016 "@Kafka" @attr 1=1016 "{Kafka}" @attr 1=1016 "Ka\fka""#
        );
    }

    #[test]
    fn a_term_of_nothing_but_quotes_does_not_become_an_empty_field_value() {
        let spec = spec_with_terms(vec![word("Kafka"), word("\"\"")]);
        assert_eq!(query_of(&spec), r#"@attr 1=1016 "Kafka""#);
    }

    // --- combination ---

    #[test]
    fn three_clauses_nest_to_the_left() {
        let spec = QuerySpec {
            terms: vec![word("Kafka")],
            title: Some(phrase("Der Prozess")),
            year: Some(1925),
            ..QuerySpec::default()
        };
        assert_eq!(
            query_of(&spec),
            r#"@and @and @attr 1=1016 "Kafka" @attr 1=4 "Der Prozess" @attr 1=31 1925"#
        );
    }

    #[test]
    fn three_locations_nest_to_the_left_too() {
        let spec = spec_with_terms(vec![word("Kafka")]);
        let query =
            search(&spec, &[isil("DE-11"), isil("DE-1"), isil("DE-83")]).expect("valid query");
        assert_eq!(
            query.as_str(),
            concat!(
                r#"@and @attr 1=1016 "Kafka" "#,
                "@or @or @attr 1=1044 DE-11 @attr 1=1044 DE-1 @attr 1=1044 DE-83"
            )
        );
    }

    #[test]
    fn one_location_needs_no_or() {
        let spec = spec_with_terms(vec![word("Kafka")]);
        let query = search(&spec, &[isil("DE-11")]).expect("valid query");
        assert_eq!(
            query.as_str(),
            r#"@and @attr 1=1016 "Kafka" @attr 1=1044 DE-11"#
        );
    }

    #[test]
    fn a_filter_without_search_terms_is_a_query_of_its_own() {
        let query = search(&QuerySpec::default(), &[isil("DE-B1533")]).expect("valid query");
        assert_eq!(query.as_str(), "@attr 1=1044 DE-B1533");
    }

    // --- refusals ---

    #[test]
    fn nothing_to_search_and_nowhere_to_search_is_an_empty_query() {
        assert!(matches!(
            search(&QuerySpec::default(), &[]),
            Err(UsageError::EmptyQuery)
        ));
    }

    #[test]
    fn a_query_of_a_thousand_characters_is_refused_before_it_is_sent() {
        let spec = spec_with_terms(vec![word(&"a".repeat(1000))]);
        match search(&spec, &[]) {
            Err(UsageError::QueryTooLong { chars }) => assert!(chars >= MAX_QUERY_CHARS),
            other => panic!("expected QueryTooLong, got {other:?}"),
        }
    }

    #[test]
    fn a_query_just_under_the_limit_is_still_sent() {
        // `@attr 1=1016 ""` is 15 characters around the term.
        let spec = spec_with_terms(vec![word(&"a".repeat(MAX_QUERY_CHARS - 16))]);
        let query = search(&spec, &[]).expect("just short enough");
        assert_eq!(query.char_len(), MAX_QUERY_CHARS - 1);
    }

    #[test]
    fn the_length_limit_counts_characters_not_bytes() {
        let spec = spec_with_terms(vec![word(&"ä".repeat(MAX_QUERY_CHARS - 16))]);
        let query = search(&spec, &[]).expect("umlauts must not count double");
        assert_eq!(query.char_len(), MAX_QUERY_CHARS - 1);
    }

    // --- lookup ---

    #[test]
    fn lookup_quotes_the_id() {
        let id = RecordId::parse("almafu_BV008885798").expect("well-formed id");
        assert_eq!(
            record_lookup(&id).as_str(),
            r#"@attr 1=12 "almafu_BV008885798""#
        );
    }

    #[test]
    fn lookup_survives_an_id_with_punctuation_in_it() {
        let id =
            RecordId::parse("kobvindex_VBRD-i9783959087rüt20in209+20").expect("well-formed id");
        assert_eq!(
            record_lookup(&id).as_str(),
            r#"@attr 1=12 "kobvindex_VBRD-i9783959087rüt20in209+20""#
        );
    }
}
