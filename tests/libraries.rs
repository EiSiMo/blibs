//! The library list is data, and its invariants are the file's, not the code's.

mod common;

use blibs::error::UsageError;
use blibs::libraries::data::{LIBRARIES_JSON, Library};
use blibs::libraries::resolve::{
    Found, branch_count, branch_label, branches, distinguishing_name, find_entries,
    houses_with_branches, isil_answers_elsewhere, near_entries, shadowed_by_key, shares_isil,
    short_name_is_cut,
};
use blibs::libraries::{
    Entry, alias_for, branch_location, by_isil, by_kobvid, by_portal_name, display_name,
    entry_location, look_up, name_holding, resolve, short_name_for,
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
    assert_eq!(branches, 211, "branches");
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

/// Two branches of the *same* house must never share `short_name`, coordinates **and**
/// having no ISIL of their own — that combination means the same real-world branch was
/// entered twice under different `kobvid`s (as `BIB000000252`/`BIB000000372` were for the
/// Treptow-Köpenick "Kleiner Bus"), not two distinct branches.
///
/// None of the three alone is suspicious, and neither is any two of them: `short_name` is
/// truncated to ~60 characters and legitimately collides; coordinates collide whenever
/// several branches share a building; and a shared building plus a truncated name
/// together still describe two real, separately addressable branches when each carries
/// its own ISIL — the two Museum für Byzantinische Kunst / Skulpturensammlung entries
/// (`DE-B11u`, `DE-B11t`) collide on both `short_name` and coordinates but must not be
/// merged. An ISIL-less branch has no such distinguishing mark, so for those two
/// conditions together are already the signal.
#[test]
fn no_house_has_a_duplicate_branch() {
    for library in &parsed() {
        for (i, a) in library.branches.iter().enumerate() {
            for b in &library.branches[i + 1..] {
                // Bit-exact on purpose: a genuine duplicate is the same decimal literal
                // copied into two entries, not two independently measured points that
                // happen to be close — an epsilon would blur that distinction away.
                if a.short_name == b.short_name
                    && a.lat.to_bits() == b.lat.to_bits()
                    && a.lon.to_bits() == b.lon.to_bits()
                    && a.isil.is_none()
                    && b.isil.is_none()
                {
                    panic!(
                        "{} has duplicate branches {} and {} (both {:?} at {},{}, neither has its own ISIL)",
                        library.isil, a.kobvid, b.kobvid, a.short_name, a.lat, a.lon
                    );
                }
            }
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

/// A branch outside the VÖBB is searched by `kobv`, under its house's ISIL: that is the
/// only restriction the catalogue offers, and the branch itself is decided later, from the
/// copies the availability answer names.
#[test]
fn a_branch_of_another_institution_is_searched_under_its_house() {
    let philbib = resolve("philbib").expect("PHILBIB is an FU branch");

    assert_eq!(philbib.engine, Engine::Kobv);
    assert_eq!(philbib.isil.as_str(), "DE-188");
    assert_eq!(philbib.key, "PHILBIB");
    let branch = philbib.branch.expect("a branch location names its branch");
    assert_eq!(branch.kobvid, "FUB00019");
}

/// A lookup answers for **every** branch, whichever engine searches it, and however the
/// key was spelled or padded.
///
/// This is the split that used to be a second, parallel lookup in `cli::run`, where a
/// branch alias silently answered with its parent house.
#[test]
fn a_lookup_answers_for_every_branch() {
    for input in ["philbib", "PHILBIB", " philbib "] {
        match look_up(input).unwrap_or_else(|_| panic!("{input:?} must name something")) {
            Entry::Branch { parent, branch } => {
                assert_eq!(parent.isil, "DE-188", "{input:?}");
                assert_eq!(branch.kobvid, "FUB00019", "{input:?}");
            }
            other @ Entry::Institution(_) => panic!("expected a branch, got {other:?}"),
        }
        let location = resolve(input).unwrap_or_else(|error| panic!("{input:?}: {error}"));
        assert_eq!(location.key, "PHILBIB", "{input:?}");
    }
}

/// The same lookup answers `--at` and `libraries <key>`: an alias, an ISIL and a branch
/// alias each name the same thing in both, because there is only one search left.
#[test]
fn a_lookup_and_a_resolution_agree_on_what_a_key_names() {
    match look_up("de-11").expect("an ISIL names its house") {
        Entry::Institution(library) => assert_eq!(library.isil, "DE-11"),
        other @ Entry::Branch { .. } => panic!("expected an institution, got {other:?}"),
    }
    match look_up("agb").expect("a branch alias names its branch") {
        Entry::Branch { parent, branch } => {
            assert_eq!(parent.isil, "DE-609");
            assert_eq!(branch.kobvid, "SIG00036");
        }
        other @ Entry::Institution(_) => panic!("expected a branch, got {other:?}"),
    }

    // And an unknown key is the same usage error `--at` raises, suggestions included.
    match look_up("STABI2").expect_err("STABI2 is not a library") {
        UsageError::UnknownLibrary { input, suggestions } => {
            assert_eq!(input, "STABI2");
            assert_eq!(suggestions, vec!["STABI".to_string()], "{suggestions:?}");
        }
        other => panic!("expected UnknownLibrary, got {other:?}"),
    }
}

/// `branch_location` is the one place that decides which engine answers for a branch, and
/// it decides it from the parent being the public network — not from a branch's own name.
#[test]
fn branch_location_answers_from_the_parent_network() {
    for (alias, engine) in [
        ("AGB", Engine::Voebb),
        ("BSTB", Engine::Voebb),
        ("PHILBIB", Engine::Kobv),
    ] {
        let Ok(Entry::Branch { parent, branch }) = look_up(alias) else {
            panic!("{alias} must be a branch");
        };
        let location = branch_location(parent, branch);
        assert_eq!(location.key, alias);
        assert_eq!(location.engine, engine, "{alias}: parent {}", parent.isil);
        assert_eq!(location.isil.as_str(), parent.isil);
        assert_eq!(
            location.branch.expect("a branch location names it").kobvid,
            branch.kobvid
        );
    }
}

/// `libraries --find` searches names as well as aliases, case- and diacritic-insensitive.
#[test]
fn find_matches_alias_name_and_city() {
    assert!(
        matched_house("grimm", "DE-11"),
        "grimm must find the Grimm-Zentrum"
    );

    // Typed without a German keyboard, and in the wrong case.
    assert!(
        matched_house("universitat", "DE-11"),
        "folded search must ignore the umlaut"
    );
    assert!(
        matched_house("FURSTENWALDE", "DE-Fur5"),
        "folded search must ignore case and umlaut in the city"
    );
    assert!(
        find_entries("   ").is_empty(),
        "an empty query matches nothing"
    );
}

/// Whether a query answers with one house on the house's **own** text — `matched`, not
/// merely a group that carries the house as the heading of its branches.
fn matched_house(query: &str, isil: &str) -> bool {
    find_entries(query)
        .iter()
        .any(|group| group.matched && group.library.isil == isil)
}

/// `--near` puts the point's own house first, and the walk from the Grimm-Zentrum to the
/// Staatsbibliothek Unter den Linden is a few hundred metres.
#[test]
fn near_orders_by_distance() {
    let hu = by_isil(&Isil::new("DE-11")).expect("DE-11 is in the list");
    let ranked = near_entries(hu.coords().expect("DE-11 has coordinates"));

    assert_eq!(
        ranked.len(),
        parsed().len() + parsed_branches().len(),
        "every house and every branch has coordinates"
    );
    let (first, distance) = ranked[0];
    assert_eq!(entry_location(first).isil.as_str(), "DE-11");
    assert!(distance < 1e-9, "distance to itself was {distance}");

    let (_, stabi) = ranked
        .iter()
        .find(|(entry, _)| matches!(entry, Entry::Institution(library) if library.isil == "DE-1"))
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
        // Prose holdings: only voebb.de states any.
        holdings_statement: None,
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

/// No alias may look like a KOBV id, because `--at` reads both out of one namespace.
///
/// The alias rules (`plan/libraries.md` §5, rule 3) keep an alias from looking like an
/// ISIL by forbidding the hyphen. They say nothing about KOBV ids, and an eight-character
/// id such as `SIG00036` satisfies every one of them — so this is the missing half of
/// that rule, and it has to be an invariant of the file rather than a tie-break in the
/// code: whichever one won, the other would silently become unaddressable.
#[test]
fn no_alias_is_a_kobvid() {
    let libraries = parsed();
    let ids: Vec<String> = libraries
        .iter()
        .flat_map(|library| {
            library
                .kobvid
                .iter()
                .cloned()
                .chain(library.branches.iter().map(|branch| branch.kobvid.clone()))
        })
        .map(|id| id.to_lowercase())
        .collect();
    for library in &libraries {
        for alias in all_aliases(library) {
            assert!(
                !ids.contains(&alias.to_lowercase()),
                "alias {alias} is also a KOBV id"
            );
        }
    }
}

/// A branch ISIL names one branch — unless a *house* already owns it, in which case the
/// house wins and the branch is simply reached by its KOBV id instead.
///
/// `DE-109` is that case and the only one: the list gives it to the house `ZLBORG` and to
/// both ZLB branches. Because the house is resolved one step earlier, `--at DE-109` stays
/// the house and the duplicate is never reached — but a *new* duplicate among branches
/// alone would make one of them unreachable without anything saying so.
#[test]
fn branch_isils_are_unique_unless_a_house_owns_them() {
    let libraries = parsed();
    let houses: Vec<String> = libraries
        .iter()
        .map(|library| library.isil.to_lowercase())
        .collect();
    let mut seen: Vec<(String, String)> = Vec::new();
    for library in &libraries {
        for branch in &library.branches {
            let Some(isil) = branch.isil.as_ref().map(|isil| isil.to_lowercase()) else {
                continue;
            };
            if houses.contains(&isil) {
                continue;
            }
            if let Some((_, owner)) = seen.iter().find(|(existing, _)| *existing == isil) {
                panic!(
                    "branch ISIL {isil} is used by both {owner} and {}",
                    branch.kobvid
                );
            }
            seen.push((isil, branch.kobvid.clone()));
        }
    }
}

/// Every branch is addressable by its KOBV id, and 137 of them also by their own ISIL.
///
/// The id is the key that always exists; case does not matter, because the directory
/// writes it upper case and nobody types it that way.
#[test]
fn a_branch_is_named_by_its_key() {
    for input in ["SIG00036", "sig00036", "DE-109"] {
        let entry = look_up(input).unwrap_or_else(|error| panic!("{input:?}: {error}"));
        match (input, entry) {
            // `DE-109` belongs to a house as well, and the house wins.
            ("DE-109", Entry::Institution(library)) => assert_eq!(library.isil, "DE-109"),
            (_, Entry::Branch { branch, .. }) => assert_eq!(branch.kobvid, "SIG00036"),
            (_, other) => panic!("{input:?} named {other:?}"),
        }
    }
    let Ok(Entry::Branch { branch, parent }) = look_up("de-11-105") else {
        panic!("a branch ISIL must name its branch");
    };
    assert_eq!(parent.isil, "DE-11");
    assert_eq!(branch.kobvid, "HUB00043");
}

/// A path names a branch inside one house, and the fragment need not be the whole name.
#[test]
fn a_path_names_a_branch_inside_its_house() {
    for input in [
        "HU/Germanistik",
        "hu/zweigbibliothek germanistik/skandinavistik",
        "DE-11/Germanistik",
    ] {
        let Ok(Entry::Branch { branch, parent }) = look_up(input) else {
            panic!("{input:?} must name a branch");
        };
        assert_eq!(parent.isil, "DE-11", "{input:?}");
        assert_eq!(branch.kobvid, "HUB00043", "{input:?}");
    }
}

/// A fragment that fits several branches is a named error listing every one of them, with
/// the key to type instead — never a pick between two shelves.
#[test]
fn an_ambiguous_path_names_every_candidate() {
    let error = look_up("HU/Zweigbibliothek").expect_err("ten branches match");
    match error {
        UsageError::AmbiguousBranch {
            input,
            house,
            candidates,
        } => {
            assert_eq!(input, "HU/Zweigbibliothek");
            assert_eq!(house, "HU");
            assert!(candidates.len() > 1, "{candidates:?}");
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.starts_with("HUB00043 (")),
                "{candidates:?}"
            );
        }
        other => panic!("expected AmbiguousBranch, got {other:?}"),
    }
}

/// A path that names nothing suggests whole paths, so the answer can be pasted back.
///
/// Both halves are answered for: a mistyped house keeps the branch attached, and a
/// mistyped branch is matched word by word, because a path is typed as a fragment and no
/// edit distance brings `gremanistik` near `Zweigbibliothek Germanistik/Skandinavistik`.
#[test]
fn a_path_that_misses_suggests_whole_paths() {
    for (input, expected) in [
        ("HUX/Germanistik", "HU/Germanistik"),
        (
            "HU/Gremanistik",
            "HU/Zweigbibliothek Germanistik/Skandinavistik",
        ),
    ] {
        match look_up(input) {
            Err(UsageError::UnknownLibrary {
                input: echoed,
                suggestions,
            }) => {
                assert_eq!(echoed, input, "the whole path is echoed, not half of it");
                assert!(
                    suggestions.contains(&expected.to_string()),
                    "{suggestions:?}"
                );
            }
            other => panic!("{input:?}: expected UnknownLibrary, got {other:?}"),
        }
    }
}

/// Every branch of every house, as the file itself holds them.
fn parsed_branches() -> Vec<(String, blibs::libraries::Branch)> {
    parsed()
        .into_iter()
        .flat_map(|library| {
            library
                .branches
                .into_iter()
                .map(move |branch| (library.isil.clone(), branch))
        })
        .collect()
}

/// A branch that `--at` accepts is a branch `--find` has to find.
///
/// The five names in the report (§1.11): two branch aliases, a fragment of a branch name,
/// a VÖBB neighbourhood library, and a name shared by several branches of one house. Each
/// of them resolves in `--at`, so "no library matched" was a false statement about all
/// five.
#[test]
fn find_reaches_branches() {
    for query in ["AGB", "PHILBIB", "Philologische", "Frohnau", "Germanistik"] {
        let found = find_entries(query);
        assert!(
            found.iter().any(|group| !group.branches.is_empty()),
            "{query:?} found no branch: {:?}",
            found.len()
        );
    }
}

/// The listing prints the ISIL as a column and the branch view prints the KOBV id as the
/// key to type; a search that could not match either was refusing to find what it had just
/// recommended.
#[test]
fn find_reaches_the_keys_the_tool_prints() {
    let house = by_isil(&Isil::new("DE-11")).expect("DE-11 is in the list");
    assert!(
        find_entries(&house.isil)
            .iter()
            .any(|group| group.matched && group.library.isil == "DE-11"),
        "a house must be findable by its own ISIL"
    );

    let branch = house
        .branches
        .iter()
        .find(|branch| branch.isil.is_some())
        .expect("HU branches carry their own ISILs");
    let isil = branch.isil.as_deref().unwrap_or_default();
    assert!(
        find_entries(isil)
            .iter()
            .any(|group| group.branches.iter().any(|hit| hit.kobvid == branch.kobvid)),
        "a branch must be findable by its own ISIL"
    );
    assert!(
        find_entries(&branch.kobvid)
            .iter()
            .any(|group| group.branches.iter().any(|hit| hit.kobvid == branch.kobvid)),
        "a branch must be findable by the key the tool tells the user to type"
    );
}

/// Branches are searched, and houses still are not buried by them.
///
/// The reason branches were excluded stands — it is the flat listing that would bury the
/// houses, not the search. Grouping makes burying impossible by construction: whatever the
/// query, the answer has at most one entry per house, so a house can never be pushed off
/// it by its own branches.
#[test]
fn find_groups_branches_under_their_house() {
    let found = find_entries("Zweigbibliothek");
    let matched_branches: usize = found.iter().map(|group| group.branches.len()).sum();
    assert!(
        matched_branches > found.len(),
        "the query has to match more branches than houses for this to prove anything"
    );
    assert!(
        found.len() <= parsed().len(),
        "{} groups for {} houses",
        found.len(),
        parsed().len()
    );

    let mut seen: Vec<&str> = Vec::new();
    for group in &found {
        assert!(
            !seen.contains(&group.library.isil.as_str()),
            "{} appears twice",
            group.library.isil
        );
        seen.push(&group.library.isil);
    }
}

/// A house is still findable by its own name, and it says so — `matched` is what tells a
/// heading apart from an answer.
#[test]
fn find_still_answers_for_houses() {
    let group = find_entries("grimm")
        .into_iter()
        .find(|group: &Found| group.library.isil == "DE-11")
        .expect("grimm must find the Grimm-Zentrum");
    assert!(group.matched, "the house matched on its own text");
    assert!(group.count() >= 1);

    assert!(
        find_entries("   ").is_empty(),
        "an empty query matches nothing"
    );
}

/// A house that is only a heading says so, so a renderer does not offer it as a hit.
#[test]
fn a_house_carried_in_by_a_branch_is_not_itself_a_hit() {
    let group = find_entries("Frohnau")
        .into_iter()
        .find(|group: &Found| !group.branches.is_empty())
        .expect("Frohnau is a branch of the public network");
    assert!(
        !group.matched,
        "{} matched on its own text, which makes this test prove nothing",
        group.library.isil
    );
    assert!(
        !find_entries("Frohnau")
            .iter()
            .any(|other| other.matched && other.library.isil == group.library.isil),
        "a heading must not be offered as a hit"
    );
}

/// An ISIL that two entries claim is a key that names only one of them back.
///
/// Derived from the file, never from a code: the test finds the collisions the same way
/// the resolver does — by asking what two entries claim — so it keeps working when the
/// list gains or loses one.
#[test]
fn a_shared_isil_is_found_by_comparing_what_entries_claim() {
    let libraries = parsed();
    let houses: Vec<String> = libraries
        .iter()
        .map(|library| library.isil.to_lowercase())
        .collect();
    let branch_isils: Vec<String> = libraries
        .iter()
        .flat_map(|library| &library.branches)
        .filter_map(|branch| branch.isil.as_deref().map(str::to_lowercase))
        .collect();

    let mut collisions = 0usize;
    for (_, branch) in parsed_branches() {
        let Some(isil) = branch.isil.as_deref().map(str::to_lowercase) else {
            assert!(
                shares_isil(&branch).is_empty(),
                "a branch without an ISIL has no key to be wrong about"
            );
            continue;
        };
        let claimed = usize::from(houses.contains(&isil))
            + branch_isils.iter().filter(|other| **other == isil).count();
        let shared = shares_isil(&branch);
        assert_eq!(
            shared.len(),
            claimed - 1,
            "{} claims {isil} together with {claimed} entries",
            branch.kobvid
        );
        if claimed == 1 {
            assert!(
                isil_answers_elsewhere(&branch).is_none(),
                "{} owns {isil} alone",
                branch.kobvid
            );
            continue;
        }
        collisions += 1;

        // The expensive half: typing the key back answers about something else, and it is
        // that something the caller has to be able to name.
        let elsewhere = isil_answers_elsewhere(&branch)
            .unwrap_or_else(|| panic!("{} shares {isil} and must say so", branch.kobvid));
        assert!(
            shared.contains(&elsewhere),
            "what --at answers with must be among the entries that share the key"
        );
        match (elsewhere, look_up(&isil)) {
            (Entry::Institution(house), Ok(Entry::Institution(resolved))) => {
                assert!(std::ptr::eq(house, resolved), "the lookup must agree");
            }
            (_, resolved) => panic!("{isil} resolved to {resolved:?}, expected the house"),
        }
    }
    assert!(
        collisions > 0,
        "the file has to carry at least one shared branch ISIL for this to prove anything"
    );
}

/// A key one entry claims is shadowed by nothing; a key several claim names the rest.
///
/// The other half of the shared-ISIL problem, and the direction the user travels: the
/// tests above start at a branch and ask what else claims its printed key, this one starts
/// at the typed word and asks what that word fails to reach. Both have to agree, and they
/// do because both read the same claim list.
///
/// **The ambiguous key is derived, never named.** Every ISIL in the file is counted, and
/// the assertion is over the count — so this keeps meaning something when the list gains a
/// second collision or loses the one it has. The unambiguous keys are checked wholesale
/// rather than by example, which is what makes `--at ZLB`, `--at AGB` and `--at SIG00036`
/// silent by proof.
#[test]
fn a_key_several_entries_claim_names_the_ones_it_did_not_reach() {
    let libraries = parsed();

    // Every key that cannot collide by construction: aliases never look like an ISIL and
    // KOBV ids are unique across houses and branches alike (both asserted above).
    for library in &libraries {
        for alias in all_aliases(library) {
            assert!(
                shadowed_by_key(alias).is_empty(),
                "the alias {alias} names one place"
            );
        }
        for branch in &library.branches {
            assert!(
                shadowed_by_key(&branch.kobvid).is_empty(),
                "the KOBV id {} names one place",
                branch.kobvid
            );
        }
    }

    let mut claims: Vec<(String, usize)> = Vec::new();
    let mut claim = |isil: &str| {
        let folded = isil.to_lowercase();
        match claims.iter_mut().find(|(seen, _)| *seen == folded) {
            Some((_, count)) => *count += 1,
            None => claims.push((folded, 1)),
        }
    };
    for library in &libraries {
        claim(&library.isil);
        for branch in &library.branches {
            if let Some(isil) = branch.isil.as_deref() {
                claim(isil);
            }
        }
    }

    let mut collisions = 0usize;
    for (isil, claimed) in &claims {
        let shadowed = shadowed_by_key(isil);
        assert_eq!(
            shadowed.len(),
            claimed - 1,
            "{isil} is claimed by {claimed} entries"
        );
        if *claimed == 1 {
            continue;
        }
        collisions += 1;

        let answered = look_up(isil).unwrap_or_else(|error| panic!("{isil}: {error}"));
        assert!(
            !shadowed.contains(&answered),
            "{isil} must not shadow the entry it answers with"
        );
        // What the caller puts in front of the user: every shadowed entry has a key of
        // its own, it is not the ambiguous one, and typing it back arrives there.
        for entry in shadowed {
            let key = entry_location(entry).key;
            assert_ne!(
                &key.to_lowercase(),
                isil,
                "a shadowed entry needs another key"
            );
            assert_eq!(
                look_up(&key).ok(),
                Some(entry),
                "{key} has to name the entry {isil} did not reach"
            );
        }
    }
    assert!(
        collisions > 0,
        "the file has to carry at least one key two entries claim for this to prove anything"
    );
}

/// `--near` ranks branches too — that is the set the question is about.
#[test]
fn near_entries_ranks_branches_beside_houses() {
    let libraries = parsed();
    let total = libraries.len() + libraries.iter().map(|l| l.branches.len()).sum::<usize>();

    let Ok(Entry::Branch { branch, .. }) = look_up("AGB") else {
        panic!("AGB is a branch of the public network");
    };
    let ranked = near_entries(branch.coords().expect("every entry has coordinates"));
    assert_eq!(
        ranked.len(),
        total,
        "houses and branches, all with coordinates"
    );

    let (first, distance) = ranked[0];
    assert!(
        matches!(first, Entry::Branch { branch: nearest, .. } if nearest.kobvid == branch.kobvid),
        "the point's own branch must come first, got {first:?}"
    );
    assert!(distance < 1e-9, "distance to itself was {distance}");

    let mut previous = 0.0;
    for (_, distance) in &ranked {
        assert!(*distance >= previous, "not sorted at {distance}");
        previous = *distance;
    }

    // Houses are still in it, and still all of them: ranking branches must not have made
    // the answer a branch listing.
    let houses = ranked
        .iter()
        .filter(|(entry, _)| matches!(entry, Entry::Institution(_)))
        .count();
    assert_eq!(houses, libraries.len());
}

/// The directory cuts a branch name at a fixed width, and the tool has to say so rather
/// than print half a word as if it were a name.
#[test]
fn a_cut_short_name_is_marked_and_has_a_fuller_form() {
    let mut cut = 0usize;
    let mut whole = 0usize;
    for (_, branch) in parsed_branches() {
        if short_name_is_cut(&branch) {
            cut += 1;
            assert!(
                branch_label(&branch).ends_with('…'),
                "{} prints a cut name as if it were whole",
                branch.kobvid
            );
            assert_eq!(
                distinguishing_name(&branch),
                branch.name,
                "{} has to fall back to the name the directory did deliver whole",
                branch.kobvid
            );
            assert!(
                branch.name.chars().count() > branch.short_name.chars().count(),
                "{} would gain nothing from the fuller form",
                branch.kobvid
            );
        } else {
            whole += 1;
            assert_eq!(branch_label(&branch), branch.short_name);
            assert_eq!(distinguishing_name(&branch), branch.short_name);
        }
    }
    assert!(cut > 0, "the file carries cut short names");
    assert!(whole > cut, "most branch names are whole");
}

/// A branch whose short name is exactly the directory's width but complete is not cut —
/// an ellipsis there would be a false statement about data that is fine.
#[test]
fn a_complete_name_at_the_full_width_is_not_marked_as_cut() {
    let complete: Vec<String> = parsed_branches()
        .into_iter()
        .filter(|(_, branch)| branch.short_name == branch.name && short_name_is_cut(branch))
        .map(|(_, branch)| branch.kobvid)
        .collect();
    assert!(
        complete.is_empty(),
        "a name equal to the full name cannot be cut: {complete:?}"
    );
}

/// The ambiguity message exists to be chosen from, so its candidates must differ where the
/// branches differ — which a name cut at the directory's width does not.
#[test]
fn an_ambiguous_path_names_candidates_in_full() {
    let error = look_up("TU/Institut für Architektur").expect_err("several branches match");
    let UsageError::AmbiguousBranch { candidates, .. } = error else {
        panic!("expected AmbiguousBranch");
    };
    assert!(
        candidates.len() > 1,
        "the fragment has to fit several branches"
    );

    let known = parsed_branches();
    let mut cut = 0usize;
    for candidate in &candidates {
        let (kobvid, printed) = candidate
            .split_once(" (")
            .expect("a candidate is `<key> (<name>)`");
        let printed = printed.trim_end_matches(')');
        let (_, branch) = known
            .iter()
            .find(|(_, branch)| branch.kobvid == kobvid)
            .unwrap_or_else(|| panic!("{kobvid} is not a branch in the list"));
        assert_eq!(printed, distinguishing_name(branch), "{kobvid}");
        if short_name_is_cut(branch) {
            cut += 1;
            assert!(
                printed.chars().count() > branch.short_name.chars().count(),
                "{kobvid} lost the characters that tell it apart"
            );
        }
    }
    assert!(
        cut > 0,
        "this house has to have a cut branch name to prove anything"
    );

    // And what is printed has to distinguish: two candidates reading the same are not a
    // choice, which is exactly what the truncation produced.
    let mut printed: Vec<&str> = candidates
        .iter()
        .filter_map(|candidate| candidate.split_once(" ("))
        .map(|(_, name)| name.trim_end_matches(')'))
        .collect();
    printed.sort_unstable();
    let count = printed.len();
    printed.dedup();
    assert_eq!(printed.len(), count, "two candidates read the same");
}

/// `libraries --branches` needs the branches and the footer needs the count, and both are
/// derived from the file — a number written into a sentence is wrong the first time a
/// house opens or closes one.
#[test]
fn branch_enumeration_matches_the_file() {
    let libraries = parsed();
    let expected: usize = libraries.iter().map(|library| library.branches.len()).sum();
    assert_eq!(branch_count(), expected);
    assert_eq!(branches().len(), expected);
    assert_eq!(
        houses_with_branches().len(),
        libraries
            .iter()
            .filter(|library| !library.branches.is_empty())
            .count()
    );
    for library in houses_with_branches() {
        assert!(!library.branches.is_empty());
    }
    // Every branch is listed under the house that holds it, and every house that has
    // branches is one of the houses the footer counts.
    for (library, branch) in branches() {
        assert!(
            library
                .branches
                .iter()
                .any(|held| held.kobvid == branch.kobvid),
            "{} is listed under {}",
            branch.kobvid,
            library.isil
        );
        assert!(
            houses_with_branches()
                .iter()
                .any(|house| house.isil == library.isil)
        );
    }
}
