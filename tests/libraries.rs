//! The library list is data, and its invariants are the file's, not the code's.

mod common;

use blibs::error::UsageError;
use blibs::libraries::data::{LIBRARIES_JSON, Library};
use blibs::libraries::{
    alias_for, by_isil, by_kobvid, by_portal_name, display_name, find, name_holding, near, resolve,
    short_name_for,
};
use blibs::model::{Engine, Holding, Isil, Status};

/// The file, parsed straight from the embedded string.
///
/// These tests are about the *data*, so they deliberately do not go through
/// `libraries::all()` — an invariant of the file has to keep being checked when the
/// loader around it changes.
fn parsed() -> Vec<Library> {
    serde_json::from_str(LIBRARIES_JSON).expect("data/libraries.json must parse")
}

/// Institutions and branches share one alias namespace (`plan/libraries.md` §5, rule 9).
fn all_aliases(library: &Library) -> impl Iterator<Item = &String> {
    library
        .aliases
        .iter()
        .chain(library.branches.iter().flat_map(|branch| &branch.aliases))
}

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

/// Every short name obeys the rules of `plan/libraries.md` §5: `A-Z0-9`, 2–10 characters,
/// and never a hyphen. The last part is what makes the resolution order safe — an alias
/// can never be mistaken for an ISIL, so no input can name two houses.
#[test]
fn aliases_follow_the_shape_rules() {
    for library in &parsed() {
        for alias in all_aliases(library) {
            assert!(
                (2..=10).contains(&alias.chars().count())
                    && alias
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "{alias:?} at {} is not 2-10 chars of A-Z0-9",
                library.isil
            );
            assert!(!alias.contains('-'), "{alias:?} could collide with an ISIL");
        }
    }
}

/// ISILs are the other half of the resolution order and have to be unique too.
#[test]
fn isils_are_unique() {
    let libraries = parsed();
    let mut seen: Vec<String> = Vec::new();
    for library in &libraries {
        let key = library.isil.to_lowercase();
        assert!(!seen.contains(&key), "ISIL {} appears twice", library.isil);
        seen.push(key);
    }
}

/// `bibids=` links are resolved through the KOBV id, so one id must name one place —
/// house or branch, never both.
#[test]
fn kobvids_are_unique() {
    let libraries = parsed();
    let mut seen: Vec<(String, String)> = Vec::new();
    for library in &libraries {
        let ids = library
            .kobvid
            .iter()
            .cloned()
            .chain(library.branches.iter().map(|branch| branch.kobvid.clone()));
        for id in ids {
            if let Some((_, owner)) = seen.iter().find(|(existing, _)| *existing == id) {
                panic!("kobvid {id} is used by both {owner} and {}", library.isil);
            }
            seen.push((id, library.isil.clone()));
        }
    }
}

/// `--near` sorts houses, so every house needs coordinates. It is a property of the file:
/// no code can invent a location.
#[test]
fn every_institution_has_coordinates() {
    for library in &parsed() {
        assert!(
            library.coords().is_some(),
            "{} has no usable coordinates",
            library.isil
        );
    }
}

/// `portal_name` is a parser key, not a display field: the availability fragment carries
/// no ISIL, so a name that named two houses would silently mis-assign items.
#[test]
fn portal_names_are_unique() {
    let libraries = parsed();
    let mut seen: Vec<(String, String)> = Vec::new();
    for library in &libraries {
        let Some(portal) = &library.portal_name else {
            continue;
        };
        if let Some((_, owner)) = seen.iter().find(|(existing, _)| existing == portal) {
            panic!(
                "portal_name {portal:?} is used by both {owner} and {}",
                library.isil
            );
        }
        seen.push((portal.clone(), library.isil.clone()));
    }
}

/// An alias resolves regardless of how it was typed, and the result carries the canonical
/// ISIL — never the user's spelling, because the upstream filter is case-sensitive.
#[test]
fn resolves_an_institution_by_alias_and_by_isil() {
    for input in ["hu", "HU", "Hu", " hu ", "DE-11", "de-11"] {
        let location = resolve(input).unwrap_or_else(|_| panic!("{input:?} must resolve"));
        assert_eq!(location.isil.as_str(), "DE-11", "{input:?}");
        assert_eq!(location.engine, Engine::Kobv, "{input:?}");
        assert_eq!(location.key, "HU", "{input:?}");
        assert_eq!(location.branch, None, "{input:?}");
        assert_eq!(location.display, "HU Berlin", "{input:?}");
    }
}

/// A VÖBB branch is the one case that routes to the other engine — and it keeps the
/// network's ISIL, because that is all a KOBV record would ever say about it.
#[test]
fn resolves_a_voebb_branch_to_the_voebb_engine() {
    let agb = resolve("agb").expect("AGB is a VÖBB branch");

    assert_eq!(agb.engine, Engine::Voebb);
    assert_eq!(agb.isil.as_str(), "DE-609");
    assert_eq!(agb.key, "AGB");
    assert_eq!(agb.display, "AGB (VÖBB)");
    let branch = agb.branch.expect("a branch location names its branch");
    assert_eq!(branch.kobvid, "SIG00036");
    assert_eq!(branch.name, "Amerika-Gedenkbibliothek (AGB)");
}

/// The network as a whole is an institution like any other: `--at ZLB` is a KOBV search
/// over `DE-609`, and only naming one of its houses switches engines.
#[test]
fn resolves_the_voebb_network_itself_to_kobv() {
    for input in ["zlb", "voebb"] {
        let location = resolve(input).unwrap_or_else(|_| panic!("{input:?} must resolve"));
        assert_eq!(location.isil.as_str(), "DE-609", "{input:?}");
        assert_eq!(location.engine, Engine::Kobv, "{input:?}");
        assert_eq!(location.branch, None, "{input:?}");
    }
}

/// A typo is a usage error that points somewhere, never a silent non-match. `STABI2`
/// extends `STABI` by one character, so the prefix rule catches it regardless of the
/// distance table — and `SBB`, the Stabi's other alias, is far enough in edit distance
/// that it must not show up as noise.
#[test]
fn an_unknown_entry_suggests_the_near_misses() {
    let error = resolve("STABI2").expect_err("STABI2 is not a library");

    match error {
        UsageError::UnknownLibrary { input, suggestions } => {
            assert_eq!(input, "STABI2");
            assert_eq!(suggestions, vec!["STABI".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// A truncated alias is still a prefix hit even though it is only four characters long,
/// where the distance table alone would allow no edits at all.
#[test]
fn a_short_prefix_still_suggests_its_alias() {
    match resolve("STAB").expect_err("STAB is not a library") {
        UsageError::UnknownLibrary { suggestions, .. } => {
            assert_eq!(suggestions, vec!["STABI".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// `HUB` is not an alias anywhere in the list, but `HU` is a prefix of it, so it must
/// still be offered even though the input is only three characters long.
#[test]
fn a_prefix_of_a_two_letter_alias_is_still_offered() {
    match resolve("HUB").expect_err("HUB is not a library") {
        UsageError::UnknownLibrary { suggestions, .. } => {
            assert_eq!(suggestions, vec!["HU".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// `AGBB` extends the VÖBB branch alias `AGB` by one letter — branch aliases are
/// suggestion candidates on the same footing as institution aliases.
#[test]
fn a_branch_alias_is_offered_like_any_other() {
    match resolve("AGBB").expect_err("AGBB is not a library") {
        UsageError::UnknownLibrary { suggestions, .. } => {
            assert_eq!(suggestions, vec!["AGB".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// Long enough input relies on the distance rule rather than the prefix rule:
/// `CHARITEE` is one letter longer than `CHARITE`, at a length where up to two edits are
/// allowed.
#[test]
fn a_longer_typo_is_still_found_by_distance() {
    match resolve("CHARITEE").expect_err("CHARITEE is not a library") {
        UsageError::UnknownLibrary { suggestions, .. } => {
            assert_eq!(suggestions, vec!["CHARITE".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// Same again at nine characters, where up to three edits are allowed — but the prefix
/// rule already covers a doubled trailing letter.
#[test]
fn a_doubled_letter_is_still_found() {
    match resolve("VIADRINAA").expect_err("VIADRINAA is not a library") {
        UsageError::UnknownLibrary { suggestions, .. } => {
            assert_eq!(suggestions, vec!["VIADRINA".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// Nothing resembling a library gets no invented suggestions — and in particular `NOPE`
/// must not drag in `TOPO`, `FHP` or `HNEE` the way a fixed distance-3 threshold used to.
#[test]
fn a_hopeless_entry_suggests_nothing() {
    for input in ["qqqqqqqqzzz", "NOPE"] {
        match resolve(input).expect_err("not a library") {
            UsageError::UnknownLibrary { suggestions, .. } => {
                assert!(suggestions.is_empty(), "{input:?}: {suggestions:?}");
            }
            other => panic!("expected UnknownLibrary, got {other:?}"),
        }
    }
}

/// A branch outside the VÖBB cannot be searched on its own: the KOBV record does not know
/// the branch, so the error names the house to use instead.
#[test]
fn a_branch_of_another_institution_names_its_institution() {
    match resolve("philbib").expect_err("PHILBIB is an FU branch") {
        UsageError::BranchNotSearchable { input, fallback } => {
            assert_eq!(input, "philbib");
            assert_eq!(fallback, "FU");
        }
        other => panic!("expected BranchNotSearchable, got {other:?}"),
    }
}

/// `libraries --find` searches names as well as aliases, case- and diacritic-insensitive.
#[test]
fn find_matches_alias_name_and_city() {
    let hits = find("grimm");
    assert!(
        hits.iter().any(|library| library.isil == "DE-11"),
        "grimm must find the Grimm-Zentrum"
    );

    // Typed without a German keyboard, and in the wrong case.
    assert!(
        find("universitat")
            .iter()
            .any(|library| library.isil == "DE-11"),
        "folded search must ignore the umlaut"
    );
    assert!(
        find("FURSTENWALDE")
            .iter()
            .any(|library| library.isil == "DE-Fur5"),
        "folded search must ignore case and umlaut in the city"
    );
    assert!(find("   ").is_empty(), "an empty query matches nothing");
}

/// `--near` puts the point's own house first, and the walk from the Grimm-Zentrum to the
/// Staatsbibliothek Unter den Linden is a few hundred metres.
#[test]
fn near_orders_by_distance() {
    let hu = by_isil(&Isil::new("DE-11")).expect("DE-11 is in the list");
    let ranked = near(hu.coords().expect("DE-11 has coordinates"));

    assert_eq!(ranked.len(), 123, "every house has coordinates");
    let (first, distance) = ranked[0];
    assert_eq!(first.isil, "DE-11");
    assert!(distance < 1e-9, "distance to itself was {distance}");

    let (_, stabi) = ranked
        .iter()
        .find(|(library, _)| library.isil == "DE-1")
        .expect("DE-1 is in the list");
    assert!(
        (0.2..0.6).contains(stabi),
        "HU→Stabi was {stabi} km, expected a few hundred metres"
    );

    let mut previous = 0.0;
    for (_, distance) in &ranked {
        assert!(*distance >= previous, "not sorted at {distance}");
        previous = *distance;
    }
}

/// An ISIL the list does not know renders as the bare code. A holding must never vanish
/// because a house left the network.
///
/// `display_name` is the **full** official name — that is what `holdings[].library`
/// promises in `plan/cli.md`; the short one is a separate member and a separate lookup.
#[test]
fn display_name_falls_back_to_the_bare_code() {
    assert_eq!(display_name(&Isil::new("DE-XYZ")), "DE-XYZ");
    assert_eq!(
        display_name(&Isil::new("DE-11")),
        "Humboldt-Universität zu Berlin, Universitätsbibliothek, Jacob-und-Wilhelm-Grimm-Zentrum"
    );
    assert_eq!(short_name_for(&Isil::new("DE-11")), Some("HU Berlin"));
    assert_eq!(short_name_for(&Isil::new("DE-XYZ")), None);
    assert_eq!(alias_for(&Isil::new("DE-11")), Some("HU"));
    assert_eq!(alias_for(&Isil::new("DE-XYZ")), None);
}

/// The naming rule lives in one function, and both engines and the availability parser go
/// through it: `library` is the full name, `short_name` the short one, and a code the
/// list does not know keeps the bare ISIL rather than losing its copies.
#[test]
fn name_holding_is_the_one_naming_rule() {
    let mut known = Holding {
        isil: Some(Isil::new("DE-11")),
        alias: None,
        library: String::new(),
        short_name: None,
        local_id: None,
        mine: false,
        summary: Status::Unknown,
        items: Vec::new(),
    };
    name_holding(&mut known);
    assert_eq!(
        known.library,
        "Humboldt-Universität zu Berlin, Universitätsbibliothek, Jacob-und-Wilhelm-Grimm-Zentrum"
    );
    assert_eq!(known.short_name.as_deref(), Some("HU Berlin"));
    assert_eq!(known.alias.as_deref(), Some("HU"));

    let mut unknown = Holding {
        isil: Some(Isil::new("DE-XYZ")),
        library: String::new(),
        ..known.clone()
    };
    unknown.alias = None;
    unknown.short_name = None;
    name_holding(&mut unknown);
    assert_eq!(unknown.library, "DE-XYZ");
    assert_eq!(unknown.short_name, None);
    assert_eq!(unknown.alias, None);
}

/// The KOBV id is how a `bibids=` link becomes a place; it names a house or a branch.
#[test]
fn kobvid_resolves_house_and_branch() {
    let (library, branch) = by_kobvid("HUB00028").expect("the HU's own id");
    assert_eq!(library.isil, "DE-11");
    assert!(branch.is_none(), "an institution id has no branch");

    let (library, branch) = by_kobvid("SIG00036").expect("the AGB's id");
    assert_eq!(library.isil, "DE-609");
    assert_eq!(
        branch.map(|b| b.short_name.as_str()),
        Some("Amerika-Gedenkbibliothek (AGB)")
    );

    assert!(by_kobvid("NOPE00001").is_none());
}

/// The availability fragment carries no ISIL; `portal_name` is the fallback key.
#[test]
fn portal_name_resolves_to_its_house() {
    let library = by_portal_name("HU Berlin").expect("a known portal group name");
    assert_eq!(library.isil, "DE-11");
    assert!(by_portal_name("Stadtbibliothek Nirgendwo").is_none());
}
