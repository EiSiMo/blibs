//! The library list is data, and its invariants are the file's, not the code's.

mod common;

use blibs::libraries::data::{LIBRARIES_JSON, Library};

/// The compiled-in list parses, and it has the size `plan/libraries.md` states.
///
/// Deliberately parsed here rather than through `libraries::all()`: this test is about
/// the *file*, and it has to keep working when the loader around it changes.
#[test]
fn list_parses_with_the_expected_size() {
    let libraries: Vec<Library> =
        serde_json::from_str(LIBRARIES_JSON).expect("data/libraries.json must parse");

    assert_eq!(libraries.len(), 123, "institutions");
    let branches: usize = libraries.iter().map(|library| library.branches.len()).sum();
    assert_eq!(branches, 212, "branches");
}

/// Every short name resolves to exactly one library. `--at HU` must never be ambiguous,
/// and this is an invariant of the data file — nothing in the code can repair it.
#[test]
fn aliases_are_unique() {
    let libraries: Vec<Library> =
        serde_json::from_str(LIBRARIES_JSON).expect("data/libraries.json must parse");

    let mut seen: Vec<(String, String)> = Vec::new();
    for library in &libraries {
        for alias in library
            .aliases
            .iter()
            .chain(library.branches.iter().flat_map(|branch| &branch.aliases))
        {
            let key = alias.to_lowercase();
            if let Some((_, owner)) = seen.iter().find(|(existing, _)| *existing == key) {
                panic!(
                    "alias {alias:?} is used by both {owner} and {}",
                    library.isil
                );
            }
            seen.push((key, library.isil.clone()));
        }
    }
}
