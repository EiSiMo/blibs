//! String comparison for the library list: how a typed alias is matched, and how a near
//! miss becomes a suggestion.

/// Fold a string for comparison: lowercase, with diacritics reduced to their base letter
/// (`ä` → `a`, `ß` → `ss`, `é` → `e`), so that `Universitätsbibliothek` and
/// `universitatsbibliothek` compare equal.
///
/// The base letter, not the German transliteration: someone searching without a German
/// keyboard types `kopenick`, not `koepenick`. `ß` → `ss` is the one exception, because
/// there is no single base letter for it.
///
/// Punctuation and spacing are **kept**: `--find` matches substrings, and dropping the
/// separators would let `am See` match `Amsee`. Alias comparison does not care either
/// way — aliases are `[A-Z0-9]{2,10}`.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for upper in s.chars() {
        for ch in upper.to_lowercase() {
            if is_combining_mark(ch) {
                continue;
            }
            match base_letter(ch) {
                Some(base) => out.push_str(base),
                None => out.push(ch),
            }
        }
    }
    out
}

/// Combining diacritical marks, so that a decomposed `a` + `◌̈` folds like a precomposed
/// `ä`. Both spellings occur in catalogue data.
fn is_combining_mark(ch: char) -> bool {
    matches!(ch, '\u{0300}'..='\u{036f}' | '\u{1ab0}'..='\u{1aff}' | '\u{20d0}'..='\u{20ff}')
}

/// The base letters of the precomposed characters that occur in Berlin/Brandenburg
/// library names and in what users type at them. Anything not listed is left alone —
/// an unfolded character can only ever cost a match, never produce a wrong one.
fn base_letter(ch: char) -> Option<&'static str> {
    Some(match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => "i",
        'ł' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ø' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ś' | 'š' | 'ş' => "s",
        'ß' => "ss",
        'ť' | 'ţ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// The largest edit distance still worth offering as a suggestion, scaled to how much the
/// user actually typed.
///
/// A fixed threshold is too generous for short input: at 4 characters, a distance of 3
/// matches almost every two-to-four-letter acronym in the list, so a suggestion there
/// would be noise, not a near miss. Short input relies on the prefix rule in
/// [`crate::libraries::resolve::suggest`] instead, and edits are allowed only once there
/// is enough typed for a distance to mean something.
pub fn max_distance(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=5 => 1,
        6..=8 => 2,
        _ => 3,
    }
}

/// Levenshtein distance, bounded — everything above `max` is reported as `max + 1` so the
/// full matrix never has to be computed for obviously unrelated strings.
///
/// Used only to offer up to three suggestions after an unknown `--at` entry, never to
/// pick a library on the user's behalf.
pub fn levenshtein(a: &str, b: &str, max: usize) -> usize {
    let over = max + 1;
    let left: Vec<char> = a.chars().collect();
    let right: Vec<char> = b.chars().collect();
    if left.len().abs_diff(right.len()) > max {
        return over;
    }
    if left.is_empty() {
        return right.len().min(over);
    }

    let mut previous: Vec<usize> = (0..=left.len()).collect();
    let mut current = vec![0usize; left.len() + 1];
    for (row, &rc) in right.iter().enumerate() {
        current[0] = row + 1;
        let mut row_min = current[0];
        for (col, &lc) in left.iter().enumerate() {
            let substitution = previous[col] + usize::from(lc != rc);
            current[col + 1] = substitution
                .min(previous[col + 1] + 1)
                .min(current[col] + 1);
            row_min = row_min.min(current[col + 1]);
        }
        if row_min > max {
            return over;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[left.len()].min(over)
}

#[cfg(test)]
mod tests {
    use super::{fold, levenshtein, max_distance};

    #[test]
    fn folds_german_diacritics() {
        assert_eq!(fold("Universitätsbibliothek"), "universitatsbibliothek");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("KÖPENICK"), "kopenick");
    }

    /// Decomposed input has to fold like precomposed input; MARC data carries both.
    #[test]
    fn folds_decomposed_the_same_as_precomposed() {
        assert_eq!(fold("Bo\u{0308}ll"), fold("Böll"));
    }

    #[test]
    fn keeps_separators_so_substrings_stay_honest() {
        assert_eq!(fold("Bibliothek am Luisenbad"), "bibliothek am luisenbad");
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(levenshtein("stabi2", "stabi", 3), 1);
        assert_eq!(levenshtein("hu", "hu", 3), 0);
        assert_eq!(levenshtein("", "hu", 3), 2);
    }

    /// Above the bound only "too far" matters, and the caller must not be able to tell
    /// how much too far — that is what makes the early exit safe.
    #[test]
    fn distance_saturates_above_the_bound() {
        assert_eq!(levenshtein("stabi", "kunstbibliothek", 3), 4);
        assert_eq!(levenshtein("a", "bbbbbbbb", 3), 4);
    }

    /// The table `suggest` relies on: generous enough at real length, strict enough that
    /// a four-letter typo cannot match every acronym in the list.
    #[test]
    fn max_distance_scales_with_input_length() {
        assert_eq!(max_distance(0), 0);
        assert_eq!(max_distance(3), 0);
        assert_eq!(max_distance(4), 1);
        assert_eq!(max_distance(5), 1);
        assert_eq!(max_distance(6), 2);
        assert_eq!(max_distance(8), 2);
        assert_eq!(max_distance(9), 3);
        assert_eq!(max_distance(20), 3);
    }
}
