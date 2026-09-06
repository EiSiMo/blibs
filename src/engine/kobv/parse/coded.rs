//! MARC `008`, the fixed-length coded data field.
//!
//! Its offsets are only valid if the field has its full length. Some records in this
//! catalogue carry a 34-character `008` instead of 40, which shifts everything by six —
//! reading position 07 on such a record returns the wrong century. The offset is
//! therefore detected once, when the field is wrapped, and never assumed.
//!
//! Eight distinct lengths occur in the sample (34, 35, 38, 39, 40, 41, 63, absent). Only
//! the 34-character case is a *shift*; the others are padded or truncated ends, where the
//! offsets still hold and a read past the end must simply yield nothing. Every read
//! therefore goes through [`str::get`], which returns `None` instead of panicking —
//! `kobvindex_MFN13563` has a 34-character `008` that is truncated rather than shifted,
//! and asking it for a language must answer "not stated", not abort the record.

use std::time::{SystemTime, UNIX_EPOCH};

/// The length at which the six-character creation date is missing from the front.
const SHIFTED_LENGTH: usize = 34;

/// How many characters are missing in that case.
const SHIFT: usize = 6;

/// Publication years below this are not credible in this catalogue and are treated as
/// coding noise (`0.00`, `1uuu`) rather than as dates.
const EARLIEST_PLAUSIBLE_YEAR: i32 = 1000;

/// How far into the future a publication date may plausibly reach. Forthcoming titles are
/// catalogued ahead of publication, so "this year" is too tight a bound.
const FUTURE_TOLERANCE: i32 = 5;

/// Average length of a Gregorian year in seconds, used to turn the wall clock into a year
/// without pulling in a date library. Precision of a day is irrelevant next to the
/// five-year tolerance above.
const SECONDS_PER_YEAR: u64 = 31_556_952;

/// A wrapped `008` with its offset resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coded008 {
    raw: String,
    offset: usize,
}

impl Coded008 {
    /// Wrap an `008` value and work out its offset.
    ///
    /// A field of exactly 34 characters whose first six are **not** all digits has lost
    /// its creation date and starts six positions late — `almahu_9950038349602882` reads
    /// `s2002    gw      o           ger d`, where the year sits at 01–04 and the language
    /// at 29–31. Every other length keeps offset 0: 41, 39 and 63 are padded at the end,
    /// 38 and 35 are truncated there, and a 34-character field that *does* start with six
    /// digits (`kobvindex_MFN13563`) is truncated too.
    ///
    /// `None` only for an empty field, which carries no positions at all.
    pub fn new(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        let shifted = raw.chars().count() == SHIFTED_LENGTH
            && !raw
                .chars()
                .take(SHIFT)
                .all(|character| character.is_ascii_digit());
        Some(Self {
            raw: raw.to_owned(),
            offset: if shifted { SHIFT } else { 0 },
        })
    }

    /// The characters at the given `008` positions, addressed as if the field were
    /// complete. `None` when the field is too short or the shift puts the window before
    /// its start — never a panic and never a silently wrong window.
    fn at(&self, start: usize, end: usize) -> Option<&str> {
        let start = start.checked_sub(self.offset)?;
        let end = end.checked_sub(self.offset)?;
        self.raw.get(start..end)
    }

    /// Position 06 — the type of date, which is what says whether [`Coded008::year`] is a
    /// single date or the start of a run (`d`, `c` and `m` mean a continuing resource).
    pub fn kind(&self) -> Option<char> {
        self.at(6, 7).and_then(|slice| slice.chars().next())
    }

    /// Date 1, from positions 07–10. For serials this is the start of the run, which is
    /// what makes `--year` a membership test rather than an equality test.
    ///
    /// Only four digits count. The sample's unusable spellings — `||||`, `####`, `uuuu`,
    /// `19uu`, `1uuu`, `0.00`, blanks — all fail that test, and a value outside
    /// 1000..=(this year + 5) is coding noise rather than a date.
    pub fn year(&self) -> Option<i32> {
        let digits = self.at(7, 11)?;
        if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let year = digits.parse::<i32>().ok()?;
        plausible_year(year)
    }

    /// Language, from positions 35–37. An ISO-639-2/B code as the record has it.
    ///
    /// Only three lowercase letters count: `|||`, `###` and blanks are "not coded", and
    /// the field of a truncated `008` simply is not there.
    pub fn language(&self) -> Option<&str> {
        let code = self.at(35, 38)?;
        is_language_code(code).then_some(code)
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

/// Whether a value is a well-formed ISO-639-2/B code: exactly three lowercase ASCII
/// letters. Shared with the `041` reader, which meets the same broken spellings — the
/// two-codes-in-one-subfield value `lateng` is rejected here rather than split, because
/// splitting it would invent a language the record does not claim.
pub fn is_language_code(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Keep a year only if it could be a publication date, discarding the parse noise that
/// four digits can also produce.
pub fn plausible_year(year: i32) -> Option<i32> {
    (EARLIEST_PLAUSIBLE_YEAR..=current_year() + FUTURE_TOLERANCE)
        .contains(&year)
        .then_some(year)
}

/// This year, from the wall clock. A clock set before the epoch yields 1970, which only
/// tightens the upper bound and never rejects a real publication year.
fn current_year() -> i32 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    1970 + i32::try_from(seconds / SECONDS_PER_YEAR).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dangerous case: 34 characters, creation date missing, everything shifted by
    /// six. Reading 07–10 naively would return `02  ` and the language would be garbage.
    #[test]
    fn a_thirty_four_character_field_shifts_by_six() {
        let coded = Coded008::new("s2002    gw      o           ger d").expect("a non-empty 008");
        assert_eq!(coded.offset(), 6);
        assert_eq!(coded.year(), Some(2002));
        assert_eq!(coded.language(), Some("ger"));
        assert_eq!(coded.kind(), Some('s'));
    }

    /// Also 34 characters, but truncated at the end rather than shifted at the front —
    /// the six leading digits are the creation date, so the offset stays 0.
    #[test]
    fn a_thirty_four_character_field_with_a_date_is_truncated_not_shifted() {
        let coded = Coded008::new("100510c19729999gw hr pso 0 b0ger c").expect("a non-empty 008");
        assert_eq!(coded.offset(), 0);
        assert_eq!(coded.year(), Some(1972));
        assert_eq!(coded.language(), None, "the field ends before position 35");
    }

    #[test]
    fn a_full_length_field_needs_no_offset() {
        let coded = Coded008::new("110525s2011    sz |||||      00| ||ger c").expect("a 008");
        assert_eq!(coded.offset(), 0);
        assert_eq!(coded.year(), Some(2011));
        assert_eq!(coded.language(), Some("ger"));
    }

    /// 48 records carry one filler character too many. The offsets still hold.
    #[test]
    fn an_over_long_field_keeps_offset_zero() {
        let coded = Coded008::new("071110p19931993||||||||||||||||||||ger|||").expect("a 008");
        assert_eq!(coded.offset(), 0);
        assert_eq!(coded.year(), Some(1993));
        assert_eq!(coded.language(), Some("ger"));
    }

    /// Every unusable spelling observed in the sample yields `None`, never a number.
    #[test]
    fn uncoded_years_are_none() {
        for date in [
            "||||", "####", "uuuu", "19uu", "1uuu", "0.00", "    ", "¿¿¿Û",
        ] {
            let raw = format!("071110s{date}xx |||||      00| ||ger c");
            let coded = Coded008::new(&raw).expect("a 008");
            assert_eq!(coded.year(), None, "date field {date:?}");
        }
    }

    /// A four-digit number that cannot be a publication date is coding noise.
    #[test]
    fn an_implausible_year_is_none() {
        let early = Coded008::new("071110s0999    xx |||||      00| ||ger c").expect("a 008");
        assert_eq!(early.year(), None);
        let distant = Coded008::new("071110s9999    xx |||||      00| ||ger c").expect("a 008");
        assert_eq!(distant.year(), None);
    }

    /// `|||`, `###` and blanks are "not coded"; only three lowercase letters are a code.
    #[test]
    fn malformed_languages_are_none() {
        for language in ["|||", "###", "   ", "GER", "ge1"] {
            let raw = format!("071110s2011    xx |||||      00| ||{language} c");
            let coded = Coded008::new(&raw).expect("a 008");
            assert_eq!(coded.language(), None, "language field {language:?}");
        }
    }

    /// A field that stops before the language positions answers "not stated" rather than
    /// panicking on the slice.
    #[test]
    fn reading_past_a_short_field_is_none_not_a_panic() {
        let coded = Coded008::new("07").expect("a non-empty 008");
        assert_eq!(coded.year(), None);
        assert_eq!(coded.language(), None);
        assert_eq!(coded.kind(), None);
        assert_eq!(coded.raw(), "07");
    }

    #[test]
    fn an_empty_field_is_not_wrapped() {
        assert_eq!(Coded008::new(""), None);
    }

    /// `lateng` — two codes in one subfield — is rejected, not split: inventing the
    /// second language would be a claim the record does not make.
    #[test]
    fn language_codes_are_three_lowercase_letters() {
        assert!(is_language_code("ger"));
        assert!(is_language_code("zxx"));
        assert!(!is_language_code("lateng"));
        assert!(!is_language_code("de"));
        assert!(!is_language_code("Ger"));
    }
}
