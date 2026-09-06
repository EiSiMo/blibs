//! MARCXML, taken apart.
//!
//! Three rules, each of which has already cost a debugging session:
//!
//! - **`880` fields are skipped.** They are alternate-script duplicates; including them
//!   yields two titles for one record.
//! - **Subfield codes are matched exactly.** `689 $D` is not `689 $d`.
//! - **Text is cleaned when the record is built**, not at each use site: the sort
//!   characters U+0098/U+009C are dropped, LF and TAB become spaces, U+00AD (soft hyphen)
//!   is removed. Bidirectional marks are left alone — they are meaningful in the Hebrew
//!   and Arabic records.
//!
//! Building a record never fails. A record with no `<leader>` degrades to an empty leader
//! and therefore to [`crate::model::Format::Unknown`]; a record with no `001` is only
//! rejected one layer up, in [`super::record::from_marc`], where the identity is needed.
//! Failing here would drop a hit over a field that has no bearing on whether the record
//! answers the user's question.

use roxmltree::Node;

/// The alternate-script field. Dropped while parsing, so that no caller downstream has to
/// remember to exclude it. See `plan/marc-mapping.md` § `880`: the Latin transliteration
/// is in the regular field and the original script in the `880`, so keeping both would
/// print every title and every author twice.
const ALTERNATE_SCRIPT_TAG: &str = "880";

/// A parsed MARC record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarcRecord {
    leader: String,
    controls: Vec<(String, String)>,
    fields: Vec<Field>,
}

impl MarcRecord {
    /// Build a record from one `<record>` element of a MARCXML document.
    ///
    /// Never fails and never panics: unknown child elements, comments and stray text are
    /// ignored, a missing leader becomes an empty one, and a `datafield` without a `tag`
    /// attribute is skipped. `880` fields are dropped here and nowhere else.
    ///
    /// Leader and control fields are stored **verbatim**; only subfield values go through
    /// [`clean`]. The `008` field is a fixed-length string whose spaces are positions —
    /// collapsing them would shift every offset (`plan/marc-mapping.md` § `008`).
    pub fn from_node(node: Node<'_, '_>) -> Self {
        let mut leader = String::new();
        let mut controls = Vec::new();
        let mut fields = Vec::new();

        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "leader" => child.text().unwrap_or_default().clone_into(&mut leader),
                "controlfield" => {
                    if let Some(tag) = child.attribute("tag") {
                        controls
                            .push((tag.to_owned(), child.text().unwrap_or_default().to_owned()));
                    }
                }
                "datafield" => {
                    if let Some(field) = Field::from_node(child) {
                        fields.push(field);
                    }
                }
                _ => {}
            }
        }

        Self {
            leader,
            controls,
            fields,
        }
    }

    /// The leader. Empty when the record had none, which reads as an unknown format
    /// rather than as an error.
    pub fn leader(&self) -> Leader<'_> {
        Leader(&self.leader)
    }

    /// The first control field with this tag, e.g. `001` or `008`.
    pub fn control(&self, tag: &str) -> Option<&str> {
        self.controls
            .iter()
            .find(|(candidate, _)| candidate == tag)
            .map(|(_, value)| value.as_str())
    }

    /// Every control field with this tag, in record order.
    ///
    /// `007` is repeatable — up to three per record — and the online test asks whether
    /// **any** of them starts with `cr`, so reading only the first would misclassify
    /// e-books as print.
    pub fn controls<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a str> {
        self.controls
            .iter()
            .filter(move |(t, _)| t == tag)
            .map(|(_, value)| value.as_str())
    }

    /// Every data field with this tag, in record order.
    ///
    /// Never yields `880`: those are dropped when the record is parsed, so no caller has
    /// to remember to exclude them.
    pub fn fields<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Field> {
        self.fields.iter().filter(move |field| field.tag == tag)
    }

    /// The first field with this tag that has the given subfield.
    ///
    /// Needed because `245` occurs twice in some records and only one of the two carries
    /// `$a` — taking "the first `245`" would lose the title. `kobvindex_SLB24672` is the
    /// proof: its first `245` holds nothing but `$h Musikdruck`.
    pub fn first_with(&self, tag: &str, code: char) -> Option<&Field> {
        self.fields
            .iter()
            .find(|field| field.tag == tag && field.sub(code).is_some())
    }
}

/// One data field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The three-character tag.
    pub tag: String,
    /// First indicator.
    pub ind1: char,
    /// Second indicator.
    pub ind2: char,
    /// Subfields in record order; a code may repeat.
    pub subfields: Vec<(char, String)>,
}

impl Field {
    /// Build one field from a `<datafield>` element, or `None` when it carries no `tag`.
    ///
    /// Subfield codes are taken as the **first character** of the `code` attribute and are
    /// never case-folded: `689 $D` (entity type, 1319 occurrences) and `689 $d` (142) are
    /// different subfields, and folding them together would turn a person's dates into a
    /// subject heading.
    fn from_node(node: Node<'_, '_>) -> Option<Self> {
        let tag = node.attribute("tag")?;
        if tag == ALTERNATE_SCRIPT_TAG {
            return None;
        }
        let subfields = node
            .children()
            .filter(Node::is_element)
            .filter(|child| child.tag_name().name() == "subfield")
            .filter_map(|child| {
                let code = child.attribute("code")?.chars().next()?;
                Some((code, clean(child.text().unwrap_or_default())))
            })
            .collect();
        Some(Self {
            tag: tag.to_owned(),
            ind1: indicator(node.attribute("ind1")),
            ind2: indicator(node.attribute("ind2")),
            subfields,
        })
    }

    /// The first subfield with this exact code.
    pub fn sub(&self, code: char) -> Option<&str> {
        self.subfields
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, value)| value.as_str())
    }

    /// Every subfield with this exact code.
    pub fn subs(&self, code: char) -> impl Iterator<Item = &str> {
        self.subfields
            .iter()
            .filter(move |(c, _)| *c == code)
            .map(|(_, value)| value.as_str())
    }

    /// The second indicator read as a digit.
    ///
    /// Two fields need it as a number rather than as a character: `264`, where `1` means
    /// "publisher" and other values mean distributor, manufacturer or copyright date, and
    /// `245`, where it counts the leading characters to skip when sorting. A blank
    /// indicator — 4.7 % of `245` fields have one — is `0`, not an error; anything else
    /// is `None`.
    pub fn ind2_digit(&self) -> Option<u32> {
        if self.ind2 == ' ' {
            return Some(0);
        }
        self.ind2.to_digit(10)
    }
}

/// An absent or empty indicator attribute reads as a blank, which is how MARC spells
/// "not applicable" and how 4.7 % of the sample spells `0`.
fn indicator(raw: Option<&str>) -> char {
    raw.and_then(|value| value.chars().next()).unwrap_or(' ')
}

/// The 24-character leader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leader<'a>(&'a str);

impl Leader<'_> {
    /// Position 06 — type of record. Read with `nth`, never by indexing: six records in
    /// the sample carry a shifted leader (`kobvindex_LDAz01080` reads `2` here) and one
    /// that is merely short must degrade to `None`, not panic.
    pub fn kind(&self) -> Option<char> {
        self.0.chars().nth(6)
    }

    /// Position 07 — bibliographic level.
    pub fn level(&self) -> Option<char> {
        self.0.chars().nth(7)
    }

    /// The leader as written, for tests and diagnostics.
    pub fn as_str(&self) -> &str {
        self.0
    }
}

/// Clean a MARC text value.
///
/// Guards the four control characters that reach the terminal from this index
/// (`plan/marc-mapping.md` § Steuerzeichen im Text):
///
/// - U+0098 / U+009C bracket the non-sorting article **inside the field content**, and in
///   `b3kat_BV044513648` they are the only place that information exists — `245 ind2` is
///   `0` there. They are dropped and the article is kept: it belongs to the title.
/// - `<<` … `>>` are the same bracket in a different notation, e.g. the nobiliary particle
///   in `kobvindex_MFN19797` (`Buffon, Georges Louis Le Clerc <<de>>`).
/// - U+000A and U+0009 occur inside `505`/`520` and would break the one-line output.
/// - U+00AD (soft hyphen) is invisible and would survive into a copied shelfmark.
///
/// U+200E / U+200F are **kept**: they carry the reading direction of the Hebrew and
/// Arabic fields. Double-encoded UTF-8 (`almatuudk_9922218477402884`) is passed through
/// untouched — it is broken in the index, not in transport, and every repair heuristic
/// also hits correct records.
pub fn clean(raw: &str) -> String {
    let mut folded = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\u{98}' | '\u{9c}' | '\u{ad}' => {}
            '\n' | '\r' | '\t' => folded.push(' '),
            other => folded.push(other),
        }
    }
    let unbracketed = if folded.contains("<<") || folded.contains(">>") {
        folded.replace("<<", "").replace(">>", "")
    } else {
        folded
    };
    collapse_spaces(&unbracketed)
}

/// Squeeze runs of plain spaces and trim the ends.
///
/// Only U+0020 is touched — the folding above has already turned the other whitespace
/// this index contains into spaces, and no-break spaces are content.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch == ' ' {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
            }
            pending_space = false;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Loading MARC records straight out of the saved SRU responses.
    //!
    //! Deliberately **not** built on `parse::sru`: these tests exercise the MARC layer,
    //! and a bug in the envelope parser must not be able to make them pass or fail.

    use std::path::{Path, PathBuf};

    use super::MarcRecord;

    /// The MARC21 slim namespace. `<record>` elements are matched on it, so that the SRU
    /// envelope's own `<zs:record>` wrapper — a different namespace, same local name —
    /// can never be mistaken for a MARC record.
    const MARCXML_NS: &str = "http://www.loc.gov/MARC21/slim";

    /// The directory holding the saved SRU responses.
    pub(crate) fn sru_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kobv/sru")
    }

    /// Every MARC record in one saved response, in document order.
    ///
    /// Finds `<record>` by namespace, which is what keeps the SRU envelope's own
    /// `<zs:record>` and the surrogate diagnostic in `kids.xml` out of the result.
    pub(crate) fn records_in(file: &str) -> Vec<MarcRecord> {
        let path = sru_dir().join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("fixture {} is readable: {error}", path.display()));
        let document = roxmltree::Document::parse(&text)
            .unwrap_or_else(|error| panic!("fixture {} is XML: {error}", path.display()));
        document
            .descendants()
            .filter(|node| {
                node.is_element()
                    && node.tag_name().name() == "record"
                    && node.tag_name().namespace() == Some(MARCXML_NS)
            })
            .map(MarcRecord::from_node)
            .collect()
    }

    /// The record with this `001`, from this file.
    pub(crate) fn record(file: &str, id: &str) -> MarcRecord {
        records_in(file)
            .into_iter()
            .find(|record| record.control("001") == Some(id))
            .unwrap_or_else(|| panic!("fixture {file} contains record {id}"))
    }

    /// Every MARC record in every saved SRU response, paired with its file name.
    pub(crate) fn all_records() -> Vec<(String, MarcRecord)> {
        let dir = sru_dir();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("fixture dir {} is readable: {error}", dir.display()))
            .filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned()))
            .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "xml"))
            .collect();
        names.sort();
        names
            .into_iter()
            .flat_map(|name| {
                records_in(&name)
                    .into_iter()
                    .map(move |record| (name.clone(), record))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed record yields leader, control fields and data fields in order.
    #[test]
    fn parses_leader_control_fields_and_data_fields() {
        let record = fixture::record("score.xml", "kobvindex_SLB24672");
        assert_eq!(record.leader().as_str(), "00817ccm a2200277   4500");
        assert_eq!(record.leader().kind(), Some('c'));
        assert_eq!(record.leader().level(), Some('m'));
        assert_eq!(record.control("003"), Some("DE-602"));
        assert_eq!(record.fields("245").count(), 2);
    }

    /// `sru_newspaper` carries two metaproxy warnings as XML comments *inside* the
    /// `<record>` element. Iterating raw children would trip over them; iterating
    /// elements does not.
    #[test]
    fn comments_inside_a_record_are_skipped() {
        let record = fixture::record("newspaper.xml", "kobvindex_SLB42822");
        assert_eq!(record.control("001"), Some("kobvindex_SLB42822"));
        assert_eq!(record.control("007"), Some("t|"));
        assert_eq!(
            record.fields("041").next().and_then(|f| f.sub('a')),
            Some("ger")
        );
    }

    /// The first `245` of this record has only `$h`; the title is in the second.
    #[test]
    fn first_with_skips_a_field_that_lacks_the_subfield() {
        let record = fixture::record("score.xml", "kobvindex_SLB24672");
        let first = record.fields("245").next().expect("the record has a 245");
        assert_eq!(first.sub('a'), None);
        assert_eq!(first.sub('h'), Some("Musikdruck"));

        let titled = record
            .first_with("245", 'a')
            .expect("the second 245 has $a");
        assert_eq!(titled.sub('a'), Some("Sei Lob und Ehr dem höchsten Gut"));
    }

    /// Alternate-script fields never reach a caller: `almafu_BV026062583` has eight of
    /// them, including a second `245` and a second `264`.
    #[test]
    fn alternate_script_fields_are_dropped() {
        let record = fixture::record("arabic.xml", "almafu_BV026062583");
        assert_eq!(record.fields("880").count(), 0);
        assert_eq!(record.fields("245").count(), 1);
        assert_eq!(record.fields("264").count(), 1);
        let title = record
            .first_with("245", 'a')
            .and_then(|field| field.sub('a'))
            .expect("the record has a title");
        assert!(
            title.starts_with("Miṣr baina"),
            "the transliteration is the regular field, not the 880: {title}"
        );
    }

    /// `689 $D` (entity type) and `689 $d` (dates) are different subfields. Case-folding
    /// the code would make a subject chain read as a person's life dates.
    #[test]
    fn subfield_codes_are_case_sensitive() {
        let record = fixture::record("arabic.xml", "gbv_660841622");
        let first_chain = record.fields("689").next().expect("the record has a 689");
        assert_eq!(first_chain.sub('D'), Some("g"));
        assert_eq!(first_chain.sub('d'), None);
        assert_eq!(first_chain.sub('a'), Some("Kairo"));
    }

    /// `007` is repeatable and the online test asks about all of them.
    #[test]
    fn control_fields_can_repeat() {
        let record = fixture::record("festschrift.xml", "almahu_9950038349602882");
        assert_eq!(record.control("007"), Some("cr uuu---uuuuu"));
        assert_eq!(record.controls("007").count(), 1);
        assert_eq!(record.controls("099").count(), 0);
    }

    /// Control fields and the leader keep their spaces: `008` positions are only
    /// meaningful if nothing has been squeezed out of the field.
    #[test]
    fn control_fields_are_stored_verbatim() {
        let record = fixture::record("festschrift.xml", "almahu_9950038349602882");
        assert_eq!(
            record.control("008"),
            Some("s2002    gw      o           ger d")
        );
    }

    /// The two C1 controls bracket the article inside the field. `245 ind2` is `0` here,
    /// so dropping them is the only thing that makes this title readable.
    #[test]
    fn c1_sort_characters_are_removed_from_the_content() {
        let record = fixture::record("mono_kafka.xml", "b3kat_BV044513648");
        let title = record.first_with("245", 'a').expect("the record has a 245");
        assert_eq!(title.sub('a'), Some("Der Prozess"));
        assert_eq!(title.ind2_digit(), Some(0));
    }

    /// The same bracket in its other notation, here around a nobiliary particle.
    #[test]
    fn doubled_angle_brackets_are_removed() {
        let record = fixture::record("oldprint.xml", "kobvindex_MFN19797");
        let author = record.fields("100").next().expect("the record has a 100");
        assert_eq!(author.sub('a'), Some("Buffon, Georges Louis Le Clerc de"));
    }

    #[test]
    fn clean_folds_line_breaks_and_tabs_into_single_spaces() {
        assert_eq!(clean("a\nb\tc"), "a b c");
        assert_eq!(clean("  padded  value  "), "padded value");
        assert_eq!(clean("soft\u{ad}hyphen"), "softhyphen");
    }

    /// Bidirectional marks are content in the Hebrew and Arabic records and stay.
    #[test]
    fn clean_keeps_bidirectional_marks() {
        assert_eq!(clean("\u{200f}القاهرة\u{200e}"), "\u{200f}القاهرة\u{200e}");
    }

    /// Broken in the index, not in transport. Repairing it would corrupt correct records.
    #[test]
    fn clean_does_not_repair_double_encoded_utf8() {
        let record = fixture::record("multivol.xml", "almatuudk_9922218477402884");
        let variant = record
            .fields("246")
            .filter_map(|field| field.sub('a'))
            .find(|value| value.contains("Sobranie"))
            .expect("the record has the double-encoded 246");
        assert!(
            variant.contains('\u{c4}'),
            "the mojibake is passed through unchanged: {variant}"
        );
    }

    /// A blank second indicator is `0`, not a parse failure.
    #[test]
    fn a_blank_second_indicator_reads_as_zero() {
        let record = fixture::record("score.xml", "kobvindex_SLB24672");
        let field = record.first_with("245", 'a').expect("the record has a 245");
        assert_eq!(field.ind2, ' ');
        assert_eq!(field.ind2_digit(), Some(0));
    }

    /// A record with no `<leader>` is not an error here — it becomes an unknown format
    /// one layer up, and its holdings are still shown.
    #[test]
    fn a_missing_leader_degrades_to_an_empty_one() {
        let xml = r#"<record xmlns="http://www.loc.gov/MARC21/slim">
              <controlfield tag="001">gbv_1</controlfield>
            </record>"#;
        let document = roxmltree::Document::parse(xml).expect("the snippet is XML");
        let record = MarcRecord::from_node(document.root_element());
        assert_eq!(record.leader().as_str(), "");
        assert_eq!(record.leader().kind(), None);
        assert_eq!(record.leader().level(), None);
        assert_eq!(record.control("001"), Some("gbv_1"));
    }

    /// Repeated subfield codes keep their order — `245 $n` occurs twice in this record.
    #[test]
    fn repeated_subfields_keep_record_order() {
        let record = fixture::record("newspaper.xml", "kobvindex_SLB42822");
        let title = record.first_with("245", 'a').expect("the record has a 245");
        assert_eq!(
            title.subs('n').collect::<Vec<_>>(),
            ["50.1995,Februar", "PDM"]
        );
    }

    /// Every saved response parses, and none of them is empty by accident.
    #[test]
    fn every_fixture_record_parses() {
        let records = fixture::all_records();
        assert_eq!(records.len(), 41, "41 MARC records across the SRU fixtures");
        for (file, record) in records {
            assert!(
                record.control("001").is_some(),
                "record in {file} has a 001"
            );
            assert_eq!(record.fields("880").count(), 0, "no 880 survives in {file}");
        }
    }
}
