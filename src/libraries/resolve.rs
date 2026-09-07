//! Turning what the user typed, or what a record contained, into a library.

use crate::error::UsageError;
use crate::libraries::text::{fold, levenshtein, max_distance};
use crate::libraries::{Branch, LatLon, Library, all};
use crate::model::{BranchRef, Engine, Holding, Isil, Location};

/// The public library network of Berlin. **The one ISIL this crate names.**
///
/// It is not a house but a union of 98 branches, and a KOBV record only ever says
/// `DE-609` — never which branch holds the copy. Naming one of its branches is therefore
/// the single case in which a location is answered by the `voebb` engine instead of by
/// `kobv`; every other branch cannot be searched on its own at all. The rule is
/// documented in CLAUDE.md § *Two engines* and in `plan/voebb.md`; nothing else in the
/// crate branches on an ISIL.
pub const VOEBB_NETWORK: &str = "DE-609";

/// How the VÖBB is named in a block heading. The list's `short_name` for the network is
/// `Berlin VÖBB/ZLB`, which is right for a table column and too long behind a branch.
const VOEBB_LABEL: &str = "VÖBB";

/// How many suggestions an unknown entry carries.
const SUGGEST_LIMIT: usize = 3;

/// Resolve one `--at` entry.
///
/// Aliases are tried first (case-folded), then ISILs (case-insensitively) — and the
/// result always carries the **canonical** ISIL from the list, never the user's spelling.
/// An unknown entry is a usage error carrying up to three near-miss suggestions (see
/// [`suggest`]), never a silent non-match.
///
/// The engine follows from what was named: a branch under `DE-609` resolves to
/// [`crate::model::Engine::Voebb`], an institution to
/// [`crate::model::Engine::Kobv`], and any other branch is rejected as not searchable on
/// its own — the KOBV record does not know which branch holds a copy, so a branch filter
/// there could never prove absence.
///
/// Ambiguity cannot arise and is therefore not handled: aliases are unique across the
/// whole list and never contain a hyphen, so no alias can look like an ISIL. That is an
/// invariant of the data file, checked in `tests/libraries.rs`.
pub fn resolve(input: &str) -> Result<Location, UsageError> {
    let typed = input.trim();
    match lookup(typed) {
        Some(Match::Institution(library)) => Ok(institution_location(library)),
        Some(Match::Branch(library, branch)) if library.isil == VOEBB_NETWORK => {
            Ok(branch_location(library, branch))
        }
        Some(Match::Branch(library, _)) => Err(UsageError::BranchNotSearchable {
            input: typed.to_string(),
            fallback: library.alias().unwrap_or(library.isil.as_str()).to_string(),
        }),
        None => Err(UsageError::UnknownLibrary {
            input: typed.to_string(),
            suggestions: suggest(typed),
        }),
    }
}

/// What a typed entry named. Institutions and branches share one alias namespace
/// (`plan/libraries.md` §5, rule 9), so one lookup answers for both.
enum Match {
    Institution(&'static Library),
    Branch(&'static Library, &'static Branch),
}

/// Alias first, then ISIL — the order of `plan/libraries.md` §6.
fn lookup(typed: &str) -> Option<Match> {
    if typed.is_empty() {
        return None;
    }
    by_alias(typed).or_else(|| by_isil_ignoring_case(typed).map(Match::Institution))
}

/// The alias step, over institutions and branches alike.
fn by_alias(typed: &str) -> Option<Match> {
    let wanted = fold(typed);
    for library in all() {
        if library.aliases.iter().any(|alias| fold(alias) == wanted) {
            return Some(Match::Institution(library));
        }
        for branch in &library.branches {
            if branch.aliases.iter().any(|alias| fold(alias) == wanted) {
                return Some(Match::Branch(library, branch));
            }
        }
    }
    None
}

/// The ISIL step. Case-insensitive because the prefixes are mixed-case (`DE-Po82`,
/// `DE-2070s`) and nobody types those exactly; the canonical spelling is what comes back.
fn by_isil_ignoring_case(typed: &str) -> Option<&'static Library> {
    all()
        .iter()
        .find(|library| library.isil == typed)
        .or_else(|| {
            all()
                .iter()
                .find(|library| library.isil.eq_ignore_ascii_case(typed))
        })
}

/// A whole institution, searched upstream through Bib-1 attribute `1044`.
fn institution_location(library: &Library) -> Location {
    Location {
        key: library.alias().unwrap_or(library.isil.as_str()).to_string(),
        isil: Isil::new(&library.isil),
        branch: None,
        engine: Engine::Kobv,
        display: library.short_name.clone(),
    }
}

/// One branch of the public library network, searched on voebb.de.
///
/// [`crate::model::BranchRef::name`] carries the branch's `short_name`, not its full
/// name: the full name starts with the district ("Stadtbibliothek Spandau / …"), which
/// no heading has room for and which the voebb.de house facet never repeats. The facet
/// labels its checkboxes `<district>: <house>`, and that second half equals `short_name`
/// for 42 of the 84 labels in `tests/fixtures/voebb/results.html` — including both
/// aliased branches, `AGB` and `BSTB`. Matching the remaining labels is the facet
/// parser's problem (`plan/voebb.md`), not this function's: it must never guess, and a
/// label it cannot place has to be a named error rather than an empty filter.
fn branch_location(library: &Library, branch: &Branch) -> Location {
    let key = branch
        .alias()
        .or(branch.isil.as_deref())
        .unwrap_or(branch.kobvid.as_str())
        .to_string();
    Location {
        display: format!("{key} ({VOEBB_LABEL})"),
        key,
        isil: Isil::new(&library.isil),
        branch: Some(BranchRef {
            kobvid: branch.kobvid.clone(),
            name: branch.short_name.clone(),
        }),
        engine: Engine::Voebb,
    }
}

/// Look up an institution by its exact ISIL.
///
/// Falls back to a case-insensitive comparison, because ISILs also arrive from catalogue
/// data, where the spelling of the prefix is not guaranteed. A miss is not an error: the
/// caller renders the bare code (see [`display_name`]).
pub fn by_isil(isil: &Isil) -> Option<&'static Library> {
    by_isil_ignoring_case(isil.as_str())
}

/// Look up an institution by the name the KOBV portal uses in the availability fragment.
///
/// This is the fallback path of the three-step match in `plan/scraping.md` §B.5.1; the
/// comparison is folded so that a stray double space or a differently spelled umlaut in
/// the portal's HTML does not drop a whole item group.
pub fn by_portal_name(name: &str) -> Option<&'static Library> {
    let wanted = fold(name.trim());
    all().iter().find(|library| {
        library
            .portal_name
            .as_deref()
            .is_some_and(|portal| fold(portal) == wanted)
    })
}

/// Look up an institution or a branch by its KOBV directory id, as found in a `bibids=`
/// link.
///
/// The second element is `Some` only when the id names a branch; a house's own id
/// resolves to the house with no branch. Ids are unique across houses and branches — an
/// invariant of the data file, checked in `tests/libraries.rs`.
pub fn by_kobvid(kobvid: &str) -> Option<(&'static Library, Option<&'static Branch>)> {
    for library in all() {
        if library.kobvid.as_deref() == Some(kobvid) {
            return Some((library, None));
        }
        if let Some(branch) = library.branches.iter().find(|b| b.kobvid == kobvid) {
            return Some((library, Some(branch)));
        }
    }
    None
}

/// Free-text search over alias, name, short name and city, for `libraries --find`.
///
/// Case- and diacritic-insensitive substring matching, in list order. Branches are not
/// searched: `--find` answers "which house do I mean", and 212 branch names would bury
/// the 123 houses.
pub fn find(query: &str) -> Vec<&'static Library> {
    let wanted = fold(query.trim());
    if wanted.is_empty() {
        return Vec::new();
    }
    all()
        .iter()
        .filter(|library| matches_text(library, &wanted))
        .collect()
}

/// The fields `plan/libraries.md` §6 names, and only those.
fn matches_text(library: &Library, folded_query: &str) -> bool {
    let fields = [&library.short_name, &library.name, &library.city];
    fields
        .into_iter()
        .chain(library.aliases.iter())
        .any(|field| fold(field).contains(folded_query))
}

/// All institutions ordered by distance from a point, for `libraries --near`.
///
/// Entries without usable coordinates are left out rather than sorted to the front. Every
/// entry has coordinates today; the path exists because the list changes.
pub fn near(point: LatLon) -> Vec<(&'static Library, f64)> {
    let mut ranked: Vec<(&'static Library, f64)> = all()
        .iter()
        .filter_map(|library| Some((library, point.distance_km(library.coords()?))))
        .collect();
    ranked.sort_by(|(_, a), (_, b)| a.total_cmp(b));
    ranked
}

/// The full official name of an ISIL, for [`crate::model::Holding::library`].
///
/// **Never `None` and never empty.** An ISIL that is not in the list renders as the bare
/// code — an unknown library must never make a holding disappear. The full name is what
/// the JSON contract in `plan/cli.md` puts in `library` ("Humboldt-Universität zu Berlin,
/// Universitätsbibliothek, …"); the short one lives in `short_name` and comes from
/// [`short_name_for`].
pub fn display_name(isil: &Isil) -> String {
    let Some(library) = by_isil(isil) else {
        return isil.as_str().to_string();
    };
    for candidate in [&library.name, &library.short_name] {
        if !candidate.trim().is_empty() {
            return candidate.clone();
        }
    }
    isil.as_str().to_string()
}

/// The short name of an ISIL, for [`crate::model::Holding::short_name`].
///
/// `None` for a code the list does not know, and for an entry whose short name is blank:
/// the short name is an addition, and an empty column is worse than none.
pub fn short_name_for(isil: &Isil) -> Option<&'static str> {
    by_isil(isil)
        .map(|library| library.short_name.trim())
        .filter(|short| !short.is_empty())
}

/// Add the library list's names to one holding.
///
/// **The only place the naming rule lives.** Both engines and the availability parser go
/// through it, so `library` cannot mean the full name in one and the short one in the
/// other.
///
/// The list only ever *adds*: a holding whose ISIL it does not know keeps what the parser
/// put there, and a holding with no ISIL at all is left untouched — a library that joined
/// the network yesterday must still have its copies shown. `library` is filled with the
/// bare code rather than left empty, because it is the one member that is never null.
pub fn name_holding(holding: &mut Holding) {
    let Some(isil) = holding.isil.clone() else {
        return;
    };
    if holding.library.trim().is_empty() {
        holding.library = isil.as_str().to_string();
    }
    let Some(library) = by_isil(&isil) else {
        return;
    };
    holding.alias = library.alias().map(str::to_owned);
    if let Some(short) = short_name_for(&isil) {
        holding.short_name = Some(short.to_owned());
    }
    if !library.name.trim().is_empty() {
        holding.library.clone_from(&library.name);
    }
}

/// The canonical alias of an ISIL, for the `alias` field of a holding.
///
/// `None` where the list carries no spoken abbreviation for the house — the list holds no
/// invented ones (`plan/libraries.md` §5, rule 5).
pub fn alias_for(isil: &Isil) -> Option<&'static str> {
    by_isil(isil).and_then(Library::alias)
}

/// Up to three aliases close to what the user typed: prefix matches first, then the
/// nearest edit distance, alphabetical within a tie so the message is stable between
/// runs.
///
/// An alias is a candidate when either holds:
///
/// 1. **Prefix**: the folded input is a prefix of the folded alias, or the other way
///    round (`STAB` → `STABI`, `STABI2` → `STABI`) — this is what catches a short,
///    truncated or extended alias that a fixed edit distance would otherwise miss or
///    swamp with noise.
/// 2. **Distance**: the Levenshtein distance between the folded input and the folded
///    alias is at most [`max_distance`] of the input's length — scaled so a short typo
///    does not match half the list (`CHARITEE` → `CHARITE`, `VIADRINAA` → `VIADRINA`).
///
/// Branch aliases (`AGB`, `BSTB`, `PHILBIB`, …) are candidates on the same footing as
/// institution aliases — the list has one alias namespace (`plan/libraries.md` §5, rule
/// 9) and `suggest` does not care which kind it is offering.
///
/// Suggestions only ever *offer*; nothing here picks a library on the user's behalf. A
/// name search belongs to `libraries --find`, not here — the hint text points there.
pub fn suggest(typed: &str) -> Vec<String> {
    let wanted = fold(typed);
    if wanted.is_empty() {
        return Vec::new();
    }
    let bound = max_distance(wanted.chars().count());
    let mut scored: Vec<(bool, usize, &str)> = all()
        .iter()
        .flat_map(|library| {
            library
                .aliases
                .iter()
                .chain(library.branches.iter().flat_map(|branch| &branch.aliases))
        })
        .filter_map(|alias| {
            let folded_alias = fold(alias);
            let is_prefix = folded_alias.starts_with(&wanted) || wanted.starts_with(&folded_alias);
            // The distance itself is only ever used to rank, never to disqualify a
            // prefix hit, so it is computed against a bound generous enough to be exact
            // for anything this short rather than the (possibly stricter) `bound`.
            let ceiling = wanted.chars().count().max(folded_alias.chars().count());
            let distance = levenshtein(&wanted, &folded_alias, ceiling);
            (is_prefix || distance <= bound).then_some((!is_prefix, distance, alias.as_str()))
        })
        .collect();
    scored.sort_unstable();
    scored.truncate(SUGGEST_LIMIT);
    scored
        .into_iter()
        .map(|(_, _, alias)| alias.to_string())
        .collect()
}
