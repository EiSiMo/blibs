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

use crate::error::Error;

/// A parsed MARC record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarcRecord {
    leader: String,
    controls: Vec<(String, String)>,
    fields: Vec<Field>,
}

impl MarcRecord {
    /// Parse one `<record>` element.
    pub fn parse(_xml: &str) -> Result<Self, Error> {
        todo!("phase 2: marc parse")
    }

    /// The leader.
    pub fn leader(&self) -> Leader<'_> {
        Leader(&self.leader)
    }

    /// A control field, e.g. `001` or `008`.
    pub fn control(&self, tag: &str) -> Option<&str> {
        self.controls
            .iter()
            .find(|(t, _)| t == tag)
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
    /// `$a` — taking "the first `245`" would lose the title.
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
}

/// The 24-character leader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leader<'a>(&'a str);

impl Leader<'_> {
    /// Position 06 — type of record. Read with `get`, never by indexing: a short leader
    /// must degrade to `None`, not panic.
    pub fn kind(&self) -> Option<char> {
        self.0.chars().nth(6)
    }

    /// Position 07 — bibliographic level.
    pub fn level(&self) -> Option<char> {
        self.0.chars().nth(7)
    }
}

/// Clean a MARC text value: drop the sort characters U+0098/U+009C and U+00AD, fold LF and
/// TAB into spaces, collapse runs of spaces. Bidirectional marks are preserved.
pub fn clean(_raw: &str) -> String {
    todo!("phase 2: marc parse")
}
