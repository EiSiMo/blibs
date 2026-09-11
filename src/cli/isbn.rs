//! ISBN and ISSN validation.
//!
//! This is not tidiness. `dc.identifier` throws away hyphens, the `978`/`979` prefix
//! **and the check digit** — so a mistyped ISBN does not return nothing, it returns a
//! *different book*: the example `978-3-596-29433-4` finds 17 editions of *Der
//! Zauberberg*. Validating before sending is the only way the user finds out.
//!
//! An ISSN is recognised and passed on, because there the check digit is significant
//! upstream; it is nevertheless verified here, for the same reason. A DOI is not
//! accepted at all — the index does not hold them, so it would fall through as a length
//! error, which is exactly what it is.

use crate::error::{IsbnProblem, IsbnScheme};
use crate::model::Identifier;

/// Separators that are stripped before anything is checked.
///
/// The ASCII hyphen is the one the standard uses; the others are what a copy from a
/// typeset page or a word processor produces, and rejecting those as "not a digit" would
/// blame the user for their clipboard.
const SEPARATORS: [char; 4] = ['-', '\u{2010}', '\u{2011}', '\u{2013}'];

/// The symbols a check digit can take. Index 10 is the ISBN-10/ISSN `X`.
const CHECK_SYMBOLS: [char; 11] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'X'];

/// Validate one `--isbn` argument and normalise it.
///
/// Accepts an ISBN-10, an ISBN-13 or an ISSN, with or without hyphens and spaces, and
/// returns it stripped to bare characters with a lowercase `x` folded to `X`. The check
/// digit is verified in every case, and [`IsbnProblem::CheckDigit`] names the digit the
/// rest of the number implies — that is the one failure a user cannot see for themselves,
/// and the one that would otherwise return the wrong book.
pub fn parse(input: &str) -> Result<Identifier, IsbnProblem> {
    let compact = compact(input);
    let Some((&given, body)) = compact.split_last() else {
        return Err(IsbnProblem::Length { digits: 0 });
    };
    if let Some(found) = first_bad_character(&compact) {
        return Err(IsbnProblem::Characters {
            found,
            scheme: IsbnScheme::from_len(compact.len()),
        });
    }
    let normalized: String = compact.iter().copied().map(fold_x).collect();
    match compact.len() {
        8 => verify(mod11_check(body), given, IsbnScheme::Issn)
            .map(|()| Identifier::Issn(normalized)),
        10 => verify(mod11_check(body), given, IsbnScheme::Isbn)
            .map(|()| Identifier::Isbn(normalized)),
        13 => {
            // The 13-digit form has no `X` position at all; `check_digit` would happily
            // compare one, so it is rejected as the character error it is.
            if !given.is_ascii_digit() {
                return Err(IsbnProblem::Characters {
                    found: given,
                    scheme: Some(IsbnScheme::Isbn),
                });
            }
            verify(mod10_check(body), given, IsbnScheme::Isbn)
                .map(|()| Identifier::Isbn(normalized))
        }
        digits => Err(IsbnProblem::Length { digits }),
    }
}

/// Strip the separators a written ISBN carries, leaving the characters that are checked.
fn compact(input: &str) -> Vec<char> {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && !SEPARATORS.contains(c))
        .collect()
}

/// The first character that cannot appear in any of the three number formats.
///
/// Everything must be an ASCII digit, except the last position, where `X` stands for a
/// check value of ten. Whether that `X` is legal in *this* format is decided by the
/// length, not here — an `X` at the end of a 13-digit number is a character error, an
/// `X` in the middle of anything is one too.
fn first_bad_character(compact: &[char]) -> Option<char> {
    let last = compact.len().saturating_sub(1);
    compact.iter().copied().enumerate().find_map(|(index, c)| {
        let legal = c.is_ascii_digit() || (index == last && matches!(c, 'X' | 'x'));
        (!legal).then_some(c)
    })
}

/// Compare the computed check character against the one that was typed.
fn verify(expected: char, given: char, scheme: IsbnScheme) -> Result<(), IsbnProblem> {
    if expected == fold_x(given) {
        Ok(())
    } else {
        Err(IsbnProblem::CheckDigit {
            expected,
            found: given,
            scheme,
        })
    }
}

/// Whether a record's own ISBN is the one that was searched for.
///
/// Needed because the identifier index is coarser than the question: it discards hyphens,
/// the `978`/`979` prefix **and the check digit**, so a search for one ISBN answers with
/// every record whose number agrees in the middle nine digits. Sieving those out again
/// needs a comparison the index does not make, and this is it.
///
/// Both sides are compared in their 13-digit form, because the same edition is catalogued
/// as an ISBN-10 in one record and as an ISBN-13 in the next and they are the same book.
/// Anything that is not a well-formed ISBN on either side is **not** a match — not an
/// error and not a wildcard: a record whose `020` holds something this function cannot
/// read is a record that says nothing about the question, and the caller keeps such a
/// record rather than dropping it.
pub fn same_book(searched: &str, candidate: &str) -> bool {
    match (isbn13(searched), isbn13(candidate)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// One ISBN in its 13-digit form, or `None` if it is not an ISBN at all.
///
/// An ISBN-10 becomes `978` plus its first nine digits plus a freshly computed check
/// digit — the conversion the standard defines, and the reason it is safe here is that
/// the nine digits carry the whole identity; both check digits are derived from them.
/// The check digit of the input is **not** verified: `--isbn` has already been through
/// [`parse`], and a record's own number is what the record says, right or wrong. What is
/// rejected is a length or a character that cannot be an ISBN in any reading.
fn isbn13(value: &str) -> Option<String> {
    let compact: Vec<char> = compact(value).into_iter().map(fold_x).collect();
    match compact.len() {
        10 => {
            let body: Vec<char> = compact.get(..9)?.to_vec();
            if !body.iter().all(char::is_ascii_digit) {
                return None;
            }
            let mut thirteen: Vec<char> = "978".chars().chain(body).collect();
            thirteen.push(mod10_check(&thirteen));
            Some(thirteen.into_iter().collect())
        }
        13 => {
            if !compact.iter().all(char::is_ascii_digit) {
                return None;
            }
            Some(compact.into_iter().collect())
        }
        _ => None,
    }
}

/// Fold a lowercase check `x` to the canonical `X`; everything else passes through.
fn fold_x(c: char) -> char {
    if c == 'x' { 'X' } else { c }
}

/// The check character of an ISBN-10 or an ISSN, both of which are modulo 11 with
/// descending weights.
///
/// The weights run from `body.len() + 1` down to 2 — 10..2 for the nine digits of an
/// ISBN-10, 8..2 for the seven of an ISSN — so one function answers for both.
fn mod11_check(body: &[char]) -> char {
    let weighted: u32 = body
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let weight = u32::try_from(body.len() - index + 1).unwrap_or(0);
            digit(*c) * weight
        })
        .sum();
    symbol((11 - weighted % 11) % 11)
}

/// The check digit of an ISBN-13: modulo 10 with alternating weights 1 and 3.
fn mod10_check(body: &[char]) -> char {
    let weighted: u32 = body
        .iter()
        .enumerate()
        .map(|(index, c)| digit(*c) * if index % 2 == 0 { 1 } else { 3 })
        .sum();
    symbol((10 - weighted % 10) % 10)
}

/// The numeric value of a character that [`first_bad_character`] has already accepted as
/// a digit. The fallback keeps the function total without an `unwrap` on a path reachable
/// from user input; it cannot be taken, because the body of a number is all digits by the
/// time either check runs.
fn digit(c: char) -> u32 {
    c.to_digit(10).unwrap_or(0)
}

/// The character for a residue in `0..=10`. Same reasoning as [`digit`]: both callers
/// pass a residue, so the fallback is unreachable and exists only to avoid a panic path.
fn symbol(residue: u32) -> char {
    usize::try_from(residue)
        .ok()
        .and_then(|index| CHECK_SYMBOLS.get(index).copied())
        .unwrap_or('X')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isbn(input: &str) -> String {
        match parse(input) {
            Ok(Identifier::Isbn(value)) => value,
            other => panic!("expected an ISBN for {input:?}, got {other:?}"),
        }
    }

    /// The hyphens are decoration and the normalised form is what goes into the query.
    #[test]
    fn a_hyphenated_isbn13_is_accepted_and_stripped() {
        assert_eq!(isbn("978-3-596-29433-6"), "9783596294336");
        assert_eq!(isbn("978 3 596 29433 6"), "9783596294336");
        assert_eq!(isbn("9783596294336"), "9783596294336");
    }

    /// The dangerous case: the index ignores the check digit, so this would come back as
    /// a different book rather than as nothing. The message has to name the digit the
    /// rest of the number implies, or the user cannot tell which position they mistyped.
    ///
    /// The check digit for `978-3-596-29433` is `6`, so both `-1` and `-4` are rejected.
    /// The arithmetic is the only authority here: a specification that names a different
    /// digit is a specification with a typo in it.
    #[test]
    fn a_broken_check_digit_names_the_expected_one() {
        assert_eq!(
            parse("978-3-596-29433-4"),
            Err(IsbnProblem::CheckDigit {
                expected: '6',
                found: '4',
                scheme: IsbnScheme::Isbn,
            })
        );
        assert_eq!(
            parse("978-3-596-29433-1"),
            Err(IsbnProblem::CheckDigit {
                expected: '6',
                found: '1',
                scheme: IsbnScheme::Isbn,
            })
        );
    }

    /// The textbook ISBN-13, as a check on the weighting itself.
    #[test]
    fn the_reference_isbn13_validates() {
        assert_eq!(isbn("978-3-16-148410-0"), "9783161484100");
    }

    #[test]
    fn an_isbn10_validates_modulo_eleven() {
        assert_eq!(isbn("3-935221-46-0"), "3935221460");
        assert_eq!(
            parse("3-935221-46-1"),
            Err(IsbnProblem::CheckDigit {
                expected: '0',
                found: '1',
                scheme: IsbnScheme::Isbn,
            })
        );
    }

    /// `X` is a check value of ten, not a letter, and it is folded to upper case.
    #[test]
    fn an_isbn10_may_end_in_x() {
        assert_eq!(isbn("0-8044-2957-X"), "080442957X");
        assert_eq!(isbn("0-8044-2957-x"), "080442957X");
    }

    #[test]
    fn an_issn_is_recognised_and_checked() {
        assert_eq!(parse("0028-0836"), Ok(Identifier::Issn("00280836".into())));
        assert_eq!(
            parse("0028-0837"),
            Err(IsbnProblem::CheckDigit {
                expected: '6',
                found: '7',
                scheme: IsbnScheme::Issn,
            })
        );
    }

    #[test]
    fn an_issn_may_end_in_x() {
        assert_eq!(parse("2434-561X"), Ok(Identifier::Issn("2434561X".into())));
    }

    /// A letter anywhere but the check position is a character error, whatever the
    /// length — that message is far more use than "must be 8, 10 or 13 digits". "Kafka"
    /// is 5 characters, which fits no scheme, so it carries none; the 13-character second
    /// case does fit one, and the character error still says so.
    #[test]
    fn letters_are_reported_as_letters() {
        assert_eq!(
            parse("Kafka"),
            Err(IsbnProblem::Characters {
                found: 'K',
                scheme: None,
            })
        );
        assert_eq!(
            parse("978-3-59A-29433-6"),
            Err(IsbnProblem::Characters {
                found: 'A',
                scheme: Some(IsbnScheme::Isbn),
            })
        );
    }

    /// An `X` is only a check value where the format has one; in a 13-digit number it is
    /// simply a wrong character, and the scheme is still named because the length fits
    /// one.
    #[test]
    fn an_isbn13_has_no_x_position() {
        assert_eq!(
            parse("978-3-596-29433-X"),
            Err(IsbnProblem::Characters {
                found: 'X',
                scheme: Some(IsbnScheme::Isbn),
            })
        );
    }

    #[test]
    fn a_wrong_length_says_how_many_digits_there_were() {
        assert_eq!(parse("12345"), Err(IsbnProblem::Length { digits: 5 }));
        assert_eq!(
            parse("978359629433612"),
            Err(IsbnProblem::Length { digits: 15 })
        );
        assert_eq!(parse(""), Err(IsbnProblem::Length { digits: 0 }));
        assert_eq!(parse("---"), Err(IsbnProblem::Length { digits: 0 }));
    }

    /// A DOI is not an identifier this index holds, and it must not be mistaken for one.
    #[test]
    fn a_doi_is_refused() {
        assert!(parse("10.1000/182").is_err());
    }

    /// Every check character in `0..=10` must have a symbol; a residue that fell off the
    /// table would silently produce the wrong expectation in an error message.
    #[test]
    fn every_residue_has_a_symbol() {
        let symbols: Vec<char> = (0..=10).map(symbol).collect();
        assert_eq!(symbols.iter().collect::<String>(), "0123456789X");
    }

    /// The whole point of the function: the index answers a search for one ISBN with the
    /// neighbours that share its first twelve digits, and they are different books.
    #[test]
    fn a_neighbouring_check_digit_is_not_the_same_book() {
        // 978-3-16-148410-0 is well formed and unassigned; ...-9 is Popper's *Logik der
        // Forschung*. The index returns the second for a search for the first.
        assert!(!same_book("9783161484100", "9783161484109"));
        assert!(same_book("9783161484100", "978-3-16-148410-0"));
    }

    /// The same edition is an ISBN-10 in one record and an ISBN-13 in the next.
    #[test]
    fn the_ten_and_thirteen_digit_forms_of_one_edition_match() {
        assert!(same_book("9783465046639", "3465046633"));
        assert!(same_book("3465046633", "9783465046639"));
        assert!(same_book("3-465-04663-3", "978-3-465-04663-9"));
    }

    /// An ISBN-10 whose check digit is `X` still converts: the nine digits carry the
    /// identity and the 13-digit check is computed fresh, which for this number is `0`.
    #[test]
    fn an_x_check_digit_converts() {
        assert!(same_book("316148410X", "9783161484100"));
        assert!(!same_book("316148410X", "9783161484109"));
    }

    /// Not an ISBN on either side is not a match, and never a wildcard — the caller is
    /// keeping records it cannot judge, so a `true` here would drop them instead.
    #[test]
    fn anything_that_is_not_an_isbn_matches_nothing() {
        assert!(!same_book("9783161484100", ""));
        assert!(!same_book("9783161484100", "3161484"));
        assert!(!same_book("9783161484100", "EUR 24,00"));
        assert!(!same_book("", "9783161484100"));
        // An ISSN is eight characters and is not an ISBN in any reading.
        assert!(!same_book("00029549", "00029549"));
    }
}
