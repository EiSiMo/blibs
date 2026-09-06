//! MARC `008`, the fixed-length coded data field.
//!
//! Its offsets are only valid if the field has its full length. Some records in this
//! catalogue carry a 34-character `008` instead of 40, which shifts everything by six —
//! reading position 07 on such a record returns the wrong century. The offset is
//! therefore detected once, when the field is wrapped, and never assumed.

/// A wrapped `008` with its offset resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coded008 {
    raw: String,
    offset: usize,
}

impl Coded008 {
    /// Wrap an `008` value and work out its offset: 34 characters means the field starts
    /// six positions late, 40 means it does not. Anything else yields `None` rather than
    /// a guess.
    pub fn new(_raw: &str) -> Option<Self> {
        todo!("phase 2: coded parse")
    }

    /// Date 1, from positions 07–10. For serials this is the start of the run, which is
    /// what makes `--year` a membership test rather than an equality test.
    pub fn year(&self) -> Option<i32> {
        todo!("phase 2: coded parse")
    }

    /// Language, from positions 35–37. An ISO-639-2/B code as the record has it.
    pub fn language(&self) -> Option<&str> {
        todo!("phase 2: coded parse")
    }

    /// The raw field.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The detected offset.
    pub fn offset(&self) -> usize {
        self.offset
    }
}
