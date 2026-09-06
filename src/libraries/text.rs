//! String comparison for the library list: how a typed alias is matched, and how a near
//! miss becomes a suggestion.

/// Fold a string for comparison: lowercase, diacritics removed, punctuation and spaces
/// dropped, so that `Universitätsbibliothek` and `universitatsbibliothek` compare equal.
pub fn casefold(_s: &str) -> String {
    todo!("phase 2: libraries")
}

/// Levenshtein distance, bounded — everything above `max` is reported as `max + 1` so the
/// full matrix never has to be computed for obviously unrelated strings.
///
/// Used only to offer up to three suggestions after an unknown `--at` entry, never to
/// pick a library on the user's behalf.
pub fn levenshtein(_a: &str, _b: &str, _max: usize) -> usize {
    todo!("phase 2: libraries")
}
