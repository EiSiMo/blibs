//! The SRU envelope.
//!
//! What is an error here and what is not has been settled against live responses:
//!
//! - **Errors.** A body that is not XML at all (an HTML page with status 200 is the usual
//!   form), and a missing `numberOfRecords`.
//! - **Not errors.** Fewer records than announced — SRU caps `maximumRecords` at 50
//!   silently — XML comments *inside* a `<record>` element, and per-record surrogate
//!   diagnostics. The last two are real: `sru_newspaper.xml` has comments between record
//!   children, so children must be filtered to elements; `sru_kids.xml` delivers a
//!   diagnostic at record position 49, leaving 49 valid records and one note.
//! - **Rejected upstream.** A top-level `<diagnostics>` element is
//!   [`crate::error::RejectedError`], exit 5 — the service understood the query and
//!   refused it.
//!
//! Three details of the live envelope that a naive walk gets wrong, each with the fixture
//! that proves it:
//!
//! - **Elements are matched by namespace and local name, never by prefix.** The prefixes
//!   `zs:` and `diag:` are the service's choice and may change without notice. The
//!   diagnostic namespace is not even constant *within* one service: the top-level
//!   diagnostics of `diagnostic.xml` are in `…/ns/search-ws/diagnostic`, while the
//!   surrogate diagnostic inside `kids.xml` is in `…/zing/srw/diagnostic/`. Both are
//!   accepted.
//! - **A rejected query carries no `numberOfRecords` at all** (`diagnostic.xml`,
//!   `pqf_diag_truncation.xml`). Demanding it unconditionally would turn every exit-5
//!   rejection into an exit-6 "missing structure", so it is required only when the
//!   response carries no top-level diagnostic.
//! - **A record's payload is kept as raw XML**, sliced out of the body by the parser's
//!   byte range. [`super::marc`] takes it apart; this module never looks inside it, so a
//!   change to the MARC parser cannot break envelope handling and vice versa.

use roxmltree::{Document, Node};

use crate::error::{Error, RejectedError, UnexpectedError};
use crate::model::Note;

/// Namespaces an SRU envelope element may live in. `k2` speaks SRU 2.0; the 1.x
/// namespace is accepted because the same code path serves any `searchRetrieve`
/// response, and an element without a namespace is accepted because a proxy that strips
/// declarations must not make the response unreadable.
const SRU_NAMESPACES: &[&str] = &[
    "http://docs.oasis-open.org/ns/search-ws/sruResponse",
    "http://www.loc.gov/zing/srw/",
];

/// Namespaces a diagnostic may live in. Both are observed against the same service —
/// see the module documentation.
const DIAGNOSTIC_NAMESPACES: &[&str] = &[
    "http://docs.oasis-open.org/ns/search-ws/diagnostic",
    "http://www.loc.gov/zing/srw/diagnostic/",
];

/// The MARCXML namespace, declared on the `<record>` element inside `<recordData>`.
const MARC_NAMESPACES: &[&str] = &["http://www.loc.gov/MARC21/slim"];

/// What names this response for the reader of an error message.
const CONTEXT: &str = "SRU response";

/// The parsed envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SruResponse {
    /// What the service says the total is. Never assumed to equal `records.len()`.
    pub number_of_records: u64,
    /// Where the next window starts, when the service says.
    pub next_record_position: Option<u32>,
    /// The delivered records, in delivery order.
    pub records: Vec<SruRecord>,
    /// Top-level diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// One `<zs:record>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SruRecord {
    /// `recordPosition`, kept as delivered. Never renumbered — the fixtures rely on the
    /// original numbering to reproduce the surrogate-diagnostic case.
    pub position: Option<u32>,
    /// What the record actually contained. The `recordSchema` is checked per record, not
    /// once for the response.
    pub payload: RecordPayload,
}

/// What one delivered record turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordPayload {
    /// A MARCXML record, as raw XML for [`super::marc`] to take apart.
    Marc(String),
    /// A surrogate diagnostic in place of a record.
    Diagnostic(Diagnostic),
    /// A schema this tool does not know. Kept rather than dropped, so it can be counted
    /// and reported in a note.
    Unknown {
        /// The declared `recordSchema`.
        schema: String,
    },
}

/// An SRU diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The diagnostic URI, e.g. `info:srw/diagnostic/1/48`.
    pub uri: String,
    /// The service's message.
    pub message: Option<String>,
    /// Its `details` element. Kept exactly as the service worded it — the XML parser has
    /// already decoded the entities, and re-escaping or trimming it would change what the
    /// service said.
    pub details: Option<String>,
}

/// Parse an SRU response body.
///
/// Fails with [`crate::error::UnexpectedError::NotXml`] for a body that does not parse
/// and for one whose root element is not `searchRetrieveResponse` — a proxy error page
/// is both, and neither is an SRU document. Fails with
/// [`crate::error::UnexpectedError::MissingElement`] when the envelope has no
/// `numberOfRecords` *and* no top-level diagnostic to explain its absence, and when a
/// record declares a schema whose payload element is not there. Never fails because
/// there are no records.
pub fn parse(body: &str) -> Result<SruResponse, Error> {
    let document = Document::parse(body).map_err(|error| not_xml(body, &error.to_string()))?;
    let root = document.root_element();
    if !is_element(root, SRU_NAMESPACES, "searchRetrieveResponse") {
        let reason = format!("the root element is <{}>", root.tag_name().name());
        return Err(not_xml(body, &reason));
    }

    let diagnostics = top_level_diagnostics(root)?;
    let number_of_records = number_of_records(root, &diagnostics)?;
    let next_record_position = child_text(root, SRU_NAMESPACES, "nextRecordPosition")
        .and_then(|text| text.parse::<u32>().ok());
    let records = match child(root, SRU_NAMESPACES, "records") {
        Some(list) => list
            .children()
            .filter(|node| is_element(*node, SRU_NAMESPACES, "record"))
            .map(|node| record(node, body))
            .collect::<Result<Vec<_>, Error>>()?,
        None => Vec::new(),
    };

    Ok(SruResponse {
        number_of_records,
        next_record_position,
        records,
        diagnostics,
    })
}

/// Turn top-level diagnostics into an error.
///
/// Separate from [`parse`] so that a caller can look at a partially usable response
/// before deciding; `search` calls it immediately. A response may carry diagnostics
/// *and* records at the same time (observed with `sortKeys`), which is why the two steps
/// exist at all.
///
/// Diagnostic `1/2` becomes [`crate::error::RejectedError::TooLongUpstream`]: the service
/// words it as "System temporarily unavailable", but it comes from an HTTP 414 and is
/// permanent, so it must never be retried.
pub fn check(response: &SruResponse) -> Result<(), Error> {
    let Some(diagnostic) = response.diagnostics.first() else {
        return Ok(());
    };
    if diagnostic_code(&diagnostic.uri) == "1/2" {
        return Err(RejectedError::TooLongUpstream {
            uri: diagnostic.uri.clone(),
        }
        .into());
    }
    Err(RejectedError::Diagnostic {
        uri: diagnostic.uri.clone(),
        message: diagnostic
            .message
            .clone()
            .unwrap_or_else(|| "the catalogue gave no message".to_string()),
        details: diagnostic.details.clone(),
    }
    .into())
}

/// Notes for everything that limited the response without breaking it.
///
/// Two things land here, and both mean "a slot in the result list arrived empty": a
/// record delivered as a surrogate diagnostic (`kids.xml`, position 49) and one delivered
/// under a schema this tool cannot read. Their count is what `EngineSearch::undelivered`
/// reports; the shortfall against `maximumRecords` is *not* a note, because the 50-record
/// cap is normal.
pub fn record_notes(response: &SruResponse) -> Vec<Note> {
    response
        .records
        .iter()
        .filter_map(|record| match &record.payload {
            RecordPayload::Marc(_) => None,
            RecordPayload::Diagnostic(diagnostic) => Some(Note::new(
                "record_undelivered",
                format!(
                    "the catalogue could not deliver the record at {}: {} ({})",
                    position_text(record.position),
                    diagnostic.message.as_deref().unwrap_or("no message"),
                    diagnostic.uri
                ),
            )),
            RecordPayload::Unknown { schema } => Some(Note::new(
                "record_schema_unknown",
                format!(
                    "the record at {} arrived as {}, which blibs cannot read",
                    position_text(record.position),
                    schema_text(schema)
                ),
            )),
        })
        .collect()
}

/// One `<zs:record>`: its position, its declared schema, and the payload that schema
/// promises. A declared schema whose payload element is absent is an error — an empty
/// record list and "the element moved" must stay distinguishable.
fn record(node: Node<'_, '_>, body: &str) -> Result<SruRecord, Error> {
    let position = child_text(node, SRU_NAMESPACES, "recordPosition")
        .and_then(|text| text.parse::<u32>().ok());
    let context = format!("{CONTEXT}, record at {}", position_text(position));
    let schema = child_text(node, SRU_NAMESPACES, "recordSchema").unwrap_or_default();
    let data = child(node, SRU_NAMESPACES, "recordData")
        .ok_or_else(|| missing_element("<recordData>", &context))?;

    let payload = if is_marcxml(&schema) {
        let record = child(data, MARC_NAMESPACES, "record")
            .ok_or_else(|| missing_element("a MARCXML <record> element", &context))?;
        RecordPayload::Marc(body[record.range()].to_string())
    } else if is_diagnostic_schema(&schema) {
        let element = child(data, DIAGNOSTIC_NAMESPACES, "diagnostic")
            .ok_or_else(|| missing_element("a <diagnostic> element", &context))?;
        RecordPayload::Diagnostic(diagnostic(element, &context)?)
    } else {
        RecordPayload::Unknown { schema }
    };

    Ok(SruRecord { position, payload })
}

/// The `<zs:diagnostics>` block, if there is one. Its children may be in either of the
/// two diagnostic namespaces.
fn top_level_diagnostics(root: Node<'_, '_>) -> Result<Vec<Diagnostic>, Error> {
    let Some(container) = child(root, SRU_NAMESPACES, "diagnostics") else {
        return Ok(Vec::new());
    };
    container
        .children()
        .filter(|node| is_element(*node, DIAGNOSTIC_NAMESPACES, "diagnostic"))
        .map(|node| diagnostic(node, CONTEXT))
        .collect()
}

/// One `<diagnostic>`. `uri` is what decides the remedy, so its absence is loud rather
/// than defaulted to an empty string that would produce a hintless error.
fn diagnostic(node: Node<'_, '_>, context: &str) -> Result<Diagnostic, Error> {
    let uri = child_text(node, DIAGNOSTIC_NAMESPACES, "uri")
        .ok_or_else(|| missing_element("<uri> inside a diagnostic", context))?;
    Ok(Diagnostic {
        uri,
        message: child_text(node, DIAGNOSTIC_NAMESPACES, "message"),
        details: child_text(node, DIAGNOSTIC_NAMESPACES, "details"),
    })
}

/// `numberOfRecords`, or the one case in which it may be absent.
///
/// A response that was rejected carries diagnostics and nothing else. Requiring the
/// count there would report "the response no longer contains `numberOfRecords`" (exit 6)
/// instead of the service's own reason for the refusal (exit 5).
fn number_of_records(root: Node<'_, '_>, diagnostics: &[Diagnostic]) -> Result<u64, Error> {
    match child_text(root, SRU_NAMESPACES, "numberOfRecords") {
        Some(text) => text.parse::<u64>().map_err(|_| {
            missing_element(
                &format!("a numeric <numberOfRecords> (it said {text:?})"),
                CONTEXT,
            )
        }),
        None if !diagnostics.is_empty() => Ok(0),
        None => Err(missing_element("<numberOfRecords>", CONTEXT)),
    }
}

/// Whether `node` is an element with this local name in one of these namespaces.
///
/// An element without a namespace matches too: prefixes are the service's business, and
/// a stripped declaration must not silently turn a present element into an absent one.
fn is_element(node: Node<'_, '_>, namespaces: &[&str], local_name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == local_name
        && node
            .tag_name()
            .namespace()
            .is_none_or(|namespace| namespaces.contains(&namespace))
}

/// The first matching child element. Comments and whitespace are skipped by
/// [`is_element`] — `newspaper.xml` has `<!--…-->` between record children.
fn child<'a, 'input>(
    parent: Node<'a, 'input>,
    namespaces: &[&str],
    local_name: &str,
) -> Option<Node<'a, 'input>> {
    parent
        .children()
        .find(|node| is_element(*node, namespaces, local_name))
}

/// The trimmed text of the first matching child element, or `None` when it is absent or
/// empty.
fn child_text(parent: Node<'_, '_>, namespaces: &[&str], local_name: &str) -> Option<String> {
    let text = text_of(child(parent, namespaces, local_name)?);
    (!text.is_empty()).then_some(text)
}

/// All text below a node, concatenated and trimmed. Only text nodes are read, because
/// `Node::text` on an element repeats its first text child.
fn text_of(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text())
        .collect::<String>()
        .trim()
        .to_string()
}

/// The `1/48` part of `info:srw/diagnostic/1/48`, and the whole string when it is not
/// shaped like that.
fn diagnostic_code(uri: &str) -> &str {
    match uri.rsplit_once("/diagnostic/") {
        Some((_, code)) => code,
        None => uri,
    }
}

/// `k2` answers `marcxml`; the SRU-registered long form is accepted as well.
fn is_marcxml(schema: &str) -> bool {
    let schema = schema.to_ascii_lowercase();
    schema == "marcxml" || schema.contains("marcxml-v1")
}

/// The surrogate-diagnostic schema, `info:srw/schema/1/diagnostics-v1.1`.
fn is_diagnostic_schema(schema: &str) -> bool {
    schema.to_ascii_lowercase().contains("diagnostic")
}

/// How a record position reads inside a message.
fn position_text(position: Option<u32>) -> String {
    position.map_or_else(
        || "an unstated position".to_string(),
        |position| format!("position {position}"),
    )
}

/// How a record schema reads inside a message.
fn schema_text(schema: &str) -> String {
    if schema.is_empty() {
        "a record with no stated schema".to_string()
    } else {
        format!("schema {schema:?}")
    }
}

/// The "this is not an SRU document" error, with the first line of the body so that a
/// bug report shows what actually arrived.
fn not_xml(body: &str, reason: &str) -> Error {
    UnexpectedError::NotXml {
        context: format!("{CONTEXT} ({reason})"),
        snippet: snippet(body),
    }
    .into()
}

/// A missing-structure error naming the element and where it was looked for.
fn missing_element(what: &str, context: &str) -> Error {
    UnexpectedError::MissingElement {
        what: what.to_string(),
        context: context.to_string(),
    }
    .into()
}

/// The first 120 characters of the body, whitespace collapsed, for an error message.
fn snippet(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ExitCode;

    const MONO_KAFKA: &str = include_str!("../../../../tests/fixtures/kobv/sru/mono_kafka.xml");
    const EMPTY: &str = include_str!("../../../../tests/fixtures/kobv/sru/empty.xml");
    const DIAGNOSTIC: &str = include_str!("../../../../tests/fixtures/kobv/sru/diagnostic.xml");
    const TRUNCATION: &str =
        include_str!("../../../../tests/fixtures/kobv/sru/pqf_diag_truncation.xml");
    const KIDS: &str = include_str!("../../../../tests/fixtures/kobv/sru/kids.xml");
    const NEWSPAPER: &str = include_str!("../../../../tests/fixtures/kobv/sru/newspaper.xml");
    const COUNT: &str = include_str!("../../../../tests/fixtures/kobv/sru/count.xml");
    const UNKNOWN_ID: &str = include_str!("../../../../tests/fixtures/kobv/sru/unknown_id.xml");
    const RECORD: &str = include_str!("../../../../tests/fixtures/kobv/sru/record.xml");
    const NOT_XML: &str = include_str!("../../../../tests/fixtures/kobv/sru/not_xml.html");

    fn parsed(body: &str) -> SruResponse {
        match parse(body) {
            Ok(response) => response,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    fn marc_payloads(response: &SruResponse) -> Vec<&str> {
        response
            .records
            .iter()
            .filter_map(|record| match &record.payload {
                RecordPayload::Marc(xml) => Some(xml.as_str()),
                _ => None,
            })
            .collect()
    }

    /// `mono_kafka.xml`: the envelope's count is the catalogue's total and has nothing to
    /// do with how many records were delivered — the fixture was trimmed to two.
    #[test]
    fn the_envelope_count_is_independent_of_the_delivered_records() {
        let response = parsed(MONO_KAFKA);
        assert_eq!(response.number_of_records, 304);
        assert_eq!(response.records.len(), 2);
        assert_eq!(response.next_record_position, Some(51));
        assert_eq!(
            response
                .records
                .iter()
                .map(|record| record.position)
                .collect::<Vec<_>>(),
            vec![Some(12), Some(44)]
        );
        assert_eq!(marc_payloads(&response).len(), 2);
        assert!(response.diagnostics.is_empty());
        assert!(record_notes(&response).is_empty());
        assert!(check(&response).is_ok());
    }

    /// `empty.xml`: zero hits is a clean answer, not a diagnostic and not an error.
    #[test]
    fn zero_hits_is_an_ordinary_response() {
        let response = parsed(EMPTY);
        assert_eq!(response.number_of_records, 0);
        assert!(response.records.is_empty());
        assert!(response.diagnostics.is_empty());
        assert_eq!(response.next_record_position, None);
        assert!(check(&response).is_ok());
    }

    /// `unknown_id.xml`: a well-formed id that matches nothing answers exactly like any
    /// other empty result.
    #[test]
    fn an_unknown_record_id_is_empty_and_not_an_error() {
        let response = parsed(UNKNOWN_ID);
        assert_eq!(response.number_of_records, 0);
        assert!(response.records.is_empty());
        assert!(check(&response).is_ok());
    }

    /// `count.xml`: `maximumRecords=0` returns the total with no record list at all.
    #[test]
    fn a_count_only_response_has_a_total_and_no_records() {
        let response = parsed(COUNT);
        assert_eq!(response.number_of_records, 3718);
        assert!(response.records.is_empty());
        assert!(check(&response).is_ok());
    }

    /// `record.xml`: a single-record lookup, and the payload is the raw MARCXML element —
    /// re-parsable on its own, because the namespace is declared on `<record>` itself.
    #[test]
    fn a_record_payload_is_the_raw_marcxml_element() {
        let response = parsed(RECORD);
        assert_eq!(response.number_of_records, 1);
        assert_eq!(response.records.len(), 1);
        assert_eq!(response.records[0].position, Some(1));

        let payloads = marc_payloads(&response);
        let xml = payloads.first().expect("the record must carry MARCXML");
        assert!(xml.starts_with("<record "), "payload starts at the element");
        assert!(xml.ends_with("</record>"), "payload ends at the element");
        assert!(xml.contains("almafu_BV008885798"));

        let document = roxmltree::Document::parse(xml).expect("the payload re-parses alone");
        assert_eq!(document.root_element().tag_name().name(), "record");
        assert_eq!(
            document.root_element().tag_name().namespace(),
            Some("http://www.loc.gov/MARC21/slim")
        );
    }

    /// `diagnostic.xml`: a rejected query carries no `numberOfRecords`, which must not be
    /// reported as a broken envelope. The diagnostic namespace here is the OASIS one.
    #[test]
    fn a_rejection_parses_and_check_turns_it_into_exit_five() {
        let response = parsed(DIAGNOSTIC);
        assert_eq!(response.number_of_records, 0);
        assert!(response.records.is_empty());
        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(response.diagnostics[0].uri, "info:srw/diagnostic/1/16");
        assert_eq!(
            response.diagnostics[0].message.as_deref(),
            Some("Unsupported index")
        );
        assert_eq!(
            response.diagnostics[0].details.as_deref(),
            Some("nonexistentindex")
        );

        let error = check(&response).expect_err("a top-level diagnostic is a rejection");
        assert_eq!(error.exit(), ExitCode::Rejected);
        assert_eq!(error.kind(), "query_rejected");
        assert!(error.to_string().contains("Unsupported index"));
    }

    /// `pqf_diag_truncation.xml`: the wildcard rejection `cli` is meant to catch before it
    /// is ever sent.
    #[test]
    fn the_truncation_diagnostic_is_a_rejection() {
        let response = parsed(TRUNCATION);
        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(response.diagnostics[0].uri, "info:srw/diagnostic/1/48");
        let error = check(&response).expect_err("1/48 is a rejection");
        assert_eq!(error.exit(), ExitCode::Rejected);
    }

    /// Diagnostic `1/2` is the one that must never be retried, so it has its own variant.
    #[test]
    fn diagnostic_one_two_becomes_the_too_long_variant() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:diagnostics xmlns:diag="http://docs.oasis-open.org/ns/search-ws/diagnostic">
    <diag:diagnostic>
      <diag:uri>info:srw/diagnostic/1/2</diag:uri>
      <diag:message>System temporarily unavailable</diag:message>
    </diag:diagnostic>
  </zs:diagnostics>
</zs:searchRetrieveResponse>"#;
        let response = parsed(body);
        let error = check(&response).expect_err("1/2 is a rejection");
        assert_eq!(error.kind(), "query_too_long_upstream");
    }

    /// `kids.xml`: record position 49 is a surrogate diagnostic in a *different*
    /// diagnostic namespace from the top-level one, sitting between two ordinary records.
    /// It costs one record and one note — never the whole response.
    #[test]
    fn a_surrogate_diagnostic_costs_one_record_and_one_note() {
        let response = parsed(KIDS);
        assert_eq!(response.number_of_records, 13946);
        assert_eq!(response.records.len(), 3);
        assert_eq!(response.next_record_position, Some(51));

        let positions: Vec<_> = response
            .records
            .iter()
            .map(|record| record.position)
            .collect();
        assert_eq!(positions, vec![Some(48), Some(49), Some(50)]);

        match &response.records[1].payload {
            RecordPayload::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.uri, "info:srw/diagnostic/1/63");
                assert_eq!(
                    diagnostic.message.as_deref(),
                    Some("System error in retrieving records")
                );
                assert_eq!(diagnostic.details.as_deref(), Some("xmlParseMemory failed"));
            }
            other => panic!("position 49 must be a surrogate diagnostic, got {other:?}"),
        }
        assert_eq!(marc_payloads(&response).len(), 2);

        // A per-record diagnostic is not a rejection: the other records are usable.
        assert!(check(&response).is_ok());

        let notes = record_notes(&response);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "record_undelivered");
        assert!(notes[0].message.contains("position 49"));
        assert!(notes[0].message.contains("1/63"));
    }

    /// `newspaper.xml`: metaproxy injects XML comments *between* the children of
    /// `<record>`. Walking children without filtering to elements loses the record.
    #[test]
    fn comments_inside_a_record_do_not_disturb_the_envelope() {
        assert!(NEWSPAPER.contains("<!--"), "fixture must carry comments");
        let response = parsed(NEWSPAPER);
        assert_eq!(response.number_of_records, 3144);
        assert_eq!(response.records.len(), 1);
        let payloads = marc_payloads(&response);
        let xml = payloads.first().expect("the record must carry MARCXML");
        assert!(xml.contains("<!--"), "comments stay in the raw payload");
        assert!(xml.contains("kobvindex_SLB42822"));
    }

    /// Records and a top-level diagnostic can arrive together — measured with `sortKeys`
    /// (`plan/scraping.md` §A.5). Both are parsed; `check` still refuses.
    #[test]
    fn records_and_a_top_level_diagnostic_can_arrive_together() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:numberOfRecords>7</zs:numberOfRecords>
  <zs:records>
    <zs:record>
      <zs:recordSchema>marcxml</zs:recordSchema>
      <zs:recordData><record xmlns="http://www.loc.gov/MARC21/slim"><leader>x</leader></record></zs:recordData>
      <zs:recordPosition>1</zs:recordPosition>
    </zs:record>
  </zs:records>
  <zs:diagnostics xmlns:diag="http://docs.oasis-open.org/ns/search-ws/diagnostic">
    <diag:diagnostic>
      <diag:uri>info:srw/diagnostic/1/80</diag:uri>
      <diag:message>Sort not supported</diag:message>
    </diag:diagnostic>
  </zs:diagnostics>
</zs:searchRetrieveResponse>"#;
        let response = parsed(body);
        assert_eq!(response.number_of_records, 7);
        assert_eq!(response.records.len(), 1);
        assert_eq!(response.diagnostics.len(), 1);
        assert!(check(&response).is_err());
    }

    /// An unknown `recordSchema` is kept and counted, never dropped.
    #[test]
    fn an_unknown_record_schema_becomes_a_note() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:numberOfRecords>1</zs:numberOfRecords>
  <zs:records>
    <zs:record>
      <zs:recordSchema>dc</zs:recordSchema>
      <zs:recordData><dc/></zs:recordData>
      <zs:recordPosition>3</zs:recordPosition>
    </zs:record>
  </zs:records>
</zs:searchRetrieveResponse>"#;
        let response = parsed(body);
        assert_eq!(
            response.records[0].payload,
            RecordPayload::Unknown {
                schema: "dc".to_string()
            }
        );
        let notes = record_notes(&response);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "record_schema_unknown");
        assert!(notes[0].message.contains("position 3"));
    }

    /// `not_xml.html`: the proxy error page. It happens to be well-formed markup, so the
    /// XML parser alone would accept it — the root element is what gives it away.
    #[test]
    fn an_html_error_page_is_not_an_sru_document() {
        let error = parse(NOT_XML).expect_err("an HTML page is not an SRU response");
        assert_eq!(error.exit(), ExitCode::Unexpected);
        assert_eq!(error.kind(), "not_xml");
        assert!(error.to_string().contains("<html>"));
    }

    /// A body that is not markup at all fails the same way.
    #[test]
    fn a_body_that_is_not_xml_at_all_fails_as_not_xml() {
        let error = parse("upstream connect error").expect_err("plain text is not XML");
        assert_eq!(error.kind(), "not_xml");
    }

    /// Without a diagnostic to explain it, a missing count is a broken envelope and says
    /// which element is gone.
    #[test]
    fn a_missing_count_without_a_diagnostic_names_the_element() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:records/>
</zs:searchRetrieveResponse>"#;
        let error = parse(body).expect_err("no count and no diagnostic is a broken envelope");
        assert_eq!(error.kind(), "missing_structure");
        assert!(error.to_string().contains("numberOfRecords"));
    }

    /// A count that is not a number is the same class of problem, and quotes what came.
    #[test]
    fn a_non_numeric_count_names_what_arrived() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:numberOfRecords>many</zs:numberOfRecords>
</zs:searchRetrieveResponse>"#;
        let error = parse(body).expect_err("a non-numeric count is a broken envelope");
        assert_eq!(error.kind(), "missing_structure");
        assert!(error.to_string().contains("many"));
    }

    /// A record that declares MARCXML and delivers none is a loud error, never a record
    /// with an empty title.
    #[test]
    fn a_declared_marc_record_without_a_payload_is_an_error() {
        let body = r#"<?xml version="1.0"?>
<zs:searchRetrieveResponse xmlns:zs="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <zs:numberOfRecords>1</zs:numberOfRecords>
  <zs:records>
    <zs:record>
      <zs:recordSchema>marcxml</zs:recordSchema>
      <zs:recordData></zs:recordData>
      <zs:recordPosition>1</zs:recordPosition>
    </zs:record>
  </zs:records>
</zs:searchRetrieveResponse>"#;
        let error = parse(body).expect_err("a declared payload that is absent is an error");
        assert_eq!(error.kind(), "missing_structure");
        assert!(error.to_string().contains("position 1"));
    }

    /// Prefixes carry no meaning: the same envelope under different prefixes parses the
    /// same way.
    #[test]
    fn elements_are_matched_by_namespace_not_by_prefix() {
        let body = r#"<?xml version="1.0"?>
<x:searchRetrieveResponse xmlns:x="http://docs.oasis-open.org/ns/search-ws/sruResponse">
  <x:numberOfRecords>2</x:numberOfRecords>
  <x:nextRecordPosition>3</x:nextRecordPosition>
</x:searchRetrieveResponse>"#;
        let response = parsed(body);
        assert_eq!(response.number_of_records, 2);
        assert_eq!(response.next_record_position, Some(3));
    }

    #[test]
    fn the_diagnostic_code_is_the_tail_of_the_uri() {
        assert_eq!(diagnostic_code("info:srw/diagnostic/1/2"), "1/2");
        assert_eq!(diagnostic_code("1/2"), "1/2");
    }
}
