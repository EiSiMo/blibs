//! Turning what the user typed, or what a record contained, into a library.

use crate::error::UsageError;
use crate::libraries::text::{fold, levenshtein, max_distance};
use crate::libraries::{Branch, LatLon, Library, all};
use crate::model::{BranchRef, Engine, Holding, Isil, Location};

/// The public library network of Berlin. **The one ISIL this crate names.**
///
/// It is not a house but a union of branches, and a KOBV record only ever says
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

/// How many characters of a name the KOBV library directory delivers.
///
/// A branch's `short_name` is derived from what that directory returned, so a branch whose
/// directory name was longer arrives **already cut** — mid-word, with no ellipsis, and
/// with the missing characters nowhere in the response. The full `name` beside it comes
/// from the same source and is not cut.
///
/// Nothing here repairs the data: the characters were never delivered and no code can
/// invent them. The width exists so that [`short_name_is_cut`] can say *that* a name is
/// short of its subject; how many entries that affects is measured in `tests/libraries.rs`
/// rather than asserted in prose here.
const DIRECTORY_NAME_WIDTH: usize = 60;

/// What separates the house from the branch in `HU/Germanistik`.
///
/// Split at the **first** one only: branch names carry slashes of their own
/// (`Zweigbibliothek Germanistik/Skandinavistik`), so everything behind the first is the
/// name being searched for.
pub const PATH_SEPARATOR: char = '/';

/// What a key the user typed named. Institutions and branches share one alias namespace
/// (`plan/libraries.md` §5, rule 9), so one lookup answers for both.
///
/// This is the **whole** result of reading a key — it says what was named and nothing
/// about what may be done with it. Whether a branch can be *searched* is a separate
/// question, answered by [`branch_location`] and only for `--at`: `blibs libraries
/// PHILBIB` is a perfectly ordinary question about a house, whichever engine searches it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Entry {
    /// A whole institution: the key was one of its aliases or its ISIL.
    Institution(&'static Library),
    /// One branch, and the institution it belongs to. The key was a branch alias, the
    /// branch's KOBV id or own ISIL, or a `<house>/<branch>` path — only three of the 211
    /// branches have an alias, so the other three ways are how the rest are named.
    Branch {
        /// The institution holding the branch.
        parent: &'static Library,
        /// The branch itself.
        branch: &'static Branch,
    },
}

impl Entry {
    /// Where this entry sits, when the list has usable coordinates for it.
    ///
    /// A branch answers with **its own** coordinates rather than its parent's: the point
    /// of naming the Amerika-Gedenkbibliothek is Blücherplatz, not the head office of the
    /// network. `None` for an entry the list places nowhere — see
    /// [`crate::libraries::Library::coords`] for why that path exists at all.
    pub fn coords(self) -> Option<LatLon> {
        match self {
            Entry::Institution(library) => library.coords(),
            Entry::Branch { branch, .. } => branch.coords(),
        }
    }
}

/// Look one key up: an alias, an ISIL, a branch key, or a `<house>/<branch>` path.
///
/// **The single lookup in the crate.** `--at` reaches it through [`resolve`] and
/// `blibs libraries <key>` and `--near <key>` reach it directly, so a key that names
/// something in one cannot fail to name it in the other. Four steps, in the order of
/// `plan/libraries.md` §6:
///
/// 1. an alias, of a house or of one of the few branches that have one (case-folded),
/// 2. a house's ISIL (case-insensitively),
/// 3. a **branch key**: its KOBV id, or its own ISIL where it has one — see
///    [`by_branch_key`] for why the id is the one that always works,
/// 4. a **path**, `HU/Germanistik`, matched inside the named house by [`by_path`].
///
/// An unknown key is a usage error carrying up to three near-miss suggestions (see
/// [`suggest`]), never a silent non-match and never an empty result: "there is no such
/// library" and "you mistyped one" are different answers, and an agent that gets an empty
/// list cannot tell them apart.
///
/// Ambiguity cannot arise in the first three steps and is therefore not handled there:
/// aliases are unique across the whole list and never contain a hyphen, so no alias can
/// look like an ISIL; KOBV ids are unique across houses and branches alike; and a branch
/// ISIL that is also a house's is decided by step 2 before step 3 is reached. All three
/// are invariants of the data file, checked in `tests/libraries.rs`. The path is the one
/// step where a name genuinely fits two branches, and it says so with
/// [`UsageError::AmbiguousBranch`] rather than picking one.
pub fn look_up(input: &str) -> Result<Entry, UsageError> {
    let typed = input.trim();
    if let Some(entry) = lookup(typed) {
        return Ok(entry);
    }
    match typed.split_once(PATH_SEPARATOR) {
        Some((house, wanted)) => by_path(typed, house.trim(), wanted.trim()),
        None => Err(unknown(typed)),
    }
}

/// The error for a key that names nothing, with whatever suggestions fit it.
fn unknown(typed: &str) -> UsageError {
    UsageError::UnknownLibrary {
        input: typed.to_string(),
        suggestions: suggest(typed),
    }
}

/// Resolve one `--at` entry.
///
/// [`look_up`] says what was named; this adds the one thing `--at` needs on top and
/// nothing else — **which engine answers for it**, and the refusal for the branches no
/// engine can answer for on its own. The result always carries the **canonical** ISIL
/// from the list, never the user's spelling.
///
/// Carried alongside the canonical key: [`Location::given`] is set to what was typed,
/// verbatim apart from the surrounding whitespace the shell may have left. Nothing
/// matches on it — it exists so the answer can be found again under the name it was asked
/// for, `--at ZLB` having resolved to the key `VOEBB`.
pub fn resolve(input: &str) -> Result<Location, UsageError> {
    let typed = input.trim();
    let mut location = match look_up(typed)? {
        Entry::Institution(library) => institution_location(library),
        Entry::Branch { parent, branch } => branch_location(parent, branch),
    };
    location.given = typed.to_string();
    Ok(location)
}

/// Alias, then house ISIL, then branch key — the order of `plan/libraries.md` §6.
///
/// The path step is **not** here: it can fail with an ambiguity that has to reach the
/// user, and an `Option` has nowhere to put it. [`look_up`] runs it after this.
fn lookup(typed: &str) -> Option<Entry> {
    if typed.is_empty() {
        return None;
    }
    by_alias(typed)
        .or_else(|| by_isil_ignoring_case(typed).map(Entry::Institution))
        .or_else(|| by_branch_key(typed))
}

/// A branch named by its own key: the KOBV directory id, or its ISIL where it has one.
///
/// **The id is the key that always works.** All 211 branches carry one, it is unique
/// across houses and branches together (`tests/libraries.rs`), and `blibs libraries`
/// prints it — so `--at SIG00036` is something a user or an agent can always type back.
/// Its ISIL is the friendlier key where it exists, and 137 branches have one.
///
/// A house's own id resolves to the house, so that the one identifier means the same
/// thing here as in a `bibids=` link ([`by_kobvid`]). Case is ignored throughout: the ids
/// are upper case in the directory and nobody types them that way.
fn by_branch_key(typed: &str) -> Option<Entry> {
    for library in all() {
        if library
            .kobvid
            .as_deref()
            .is_some_and(|kobvid| kobvid.eq_ignore_ascii_case(typed))
        {
            return Some(Entry::Institution(library));
        }
        for branch in &library.branches {
            let isil = branch.isil.as_deref().unwrap_or_default();
            if branch.kobvid.eq_ignore_ascii_case(typed) || isil.eq_ignore_ascii_case(typed) {
                return Some(Entry::Branch {
                    parent: library,
                    branch,
                });
            }
        }
    }
    None
}

/// One branch of a named house: `HU/Germanistik`, `VOEBB/Bruno-Lösche`.
///
/// The readable half of branch addressing — a key is exact but has to be looked up first,
/// while a path can be typed from what the search output already shows.
///
/// The house on the left goes through [`lookup`], so it may be named any way a house can
/// be; a branch alias on the left names its own house, which is the only reading that is
/// never wrong. The name on the right is matched **only inside that house**, in three
/// stages, weakest last, the same shape the voebb facet matcher uses:
///
/// 1. folds equal to the branch's `short_name`,
/// 2. folds equal to its full `name`, to the part of that name behind the last separator,
///    or to one of its aliases,
/// 3. is contained in either name.
///
/// Only the substring stage is meant to be typed at: `HU/Germanistik` is a fragment, not
/// a name. It is also the stage that reaches past the directory's own truncation — a short
/// name cut at [`DIRECTORY_NAME_WIDTH`] still has the full `name` beside it, and the
/// fragment is matched against both (see [`short_name_is_cut`]).
///
/// Several branches under one stage are [`UsageError::AmbiguousBranch`], never a pick:
/// branch short names are *not* unique inside a house (`plan/libraries.md` §8.3).
fn by_path(typed: &str, house: &str, name: &str) -> Result<Entry, UsageError> {
    let parent = match lookup(house) {
        Some(Entry::Institution(library)) => library,
        Some(Entry::Branch { parent, .. }) => parent,
        // The house is what is wrong, so the suggestions are houses — but each is offered
        // with the branch still attached, so that the answer stays a whole path.
        None => {
            return Err(UsageError::UnknownLibrary {
                input: typed.to_string(),
                suggestions: suggest(house)
                    .into_iter()
                    .map(|alias| format!("{alias}{PATH_SEPARATOR}{name}"))
                    .collect(),
            });
        }
    };
    let wanted = fold(name);
    if wanted.is_empty() {
        return Err(unknown(typed));
    }
    for stage in [
        PathStage::ShortName,
        PathStage::AlternateNames,
        PathStage::Fragment,
    ] {
        let hits: Vec<&'static Branch> = parent
            .branches
            .iter()
            .filter(|branch| stage.accepts(branch, &wanted))
            .collect();
        match hits.as_slice() {
            [] => {}
            [only] => {
                return Ok(Entry::Branch {
                    parent,
                    branch: only,
                });
            }
            several => {
                return Err(UsageError::AmbiguousBranch {
                    input: typed.to_string(),
                    house: parent.alias().unwrap_or(&parent.isil).to_string(),
                    candidates: several.iter().copied().map(candidate).collect(),
                });
            }
        }
    }
    Err(UsageError::UnknownLibrary {
        input: typed.to_string(),
        suggestions: suggest_branches(parent, house, &wanted),
    })
}

/// One branch as an ambiguity message names it: the key to type, and the name to read.
///
/// The name is [`distinguishing_name`], not the short name: an ambiguity message exists to
/// be chosen from, and a short name cut at [`DIRECTORY_NAME_WIDTH`] drops exactly the
/// characters two candidates differ in — a list of names that end in the same truncation
/// is not a choice.
fn candidate(branch: &Branch) -> String {
    format!("{} ({})", branch.kobvid, distinguishing_name(branch))
}

/// Whether a branch's short name is the directory's truncation rather than a whole name.
///
/// Both halves of the test are needed. The width alone would be a false accusation against
/// the one branch whose name happens to be exactly that long and complete; the comparison
/// with `name` alone would accuse every branch whose short name was legitimately shortened
/// by dropping a shared prefix or a trailing "Bibliothek", which is most of them.
///
/// This says something about the upstream data, and does not change it. A caller that has
/// to print the name uses [`branch_label`]; one that has to tell two branches apart uses
/// [`distinguishing_name`].
pub fn short_name_is_cut(branch: &Branch) -> bool {
    branch.short_name.chars().count() >= DIRECTORY_NAME_WIDTH
        && branch.name.chars().count() > branch.short_name.chars().count()
}

/// A branch name that does not pretend to be whole: the short name, with an ellipsis where
/// the directory cut it off.
///
/// For a column or a heading, where the full name has no room. The ellipsis is the honest
/// part — it says characters are missing; it does not bring them back, and it must not be
/// mistaken for the tool's own width-fitting, which happens later and on top of this.
pub fn branch_label(branch: &Branch) -> String {
    if short_name_is_cut(branch) {
        format!("{}…", branch.short_name.trim_end())
    } else {
        branch.short_name.clone()
    }
}

/// The fullest name the list holds for a branch: the short name, or the full one where the
/// short one was cut.
///
/// For anywhere two branches have to be told apart — an ambiguity message above all, and
/// any listing that puts several branches of one house next to each other. It is long on
/// purpose: the alternative is a set of candidates that read identically.
pub fn distinguishing_name(branch: &Branch) -> &str {
    if short_name_is_cut(branch) {
        &branch.name
    } else {
        &branch.short_name
    }
}

/// One stage of [`by_path`], from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathStage {
    /// The short name the tool prints.
    ShortName,
    /// The full name, its tail, and the branch's aliases.
    AlternateNames,
    /// A fragment of either name.
    Fragment,
}

impl PathStage {
    /// Whether this stage accepts a branch for an already folded query.
    fn accepts(self, branch: &Branch, wanted: &str) -> bool {
        match self {
            PathStage::ShortName => fold(&branch.short_name) == wanted,
            PathStage::AlternateNames => {
                fold(&branch.name) == wanted
                    || fold(name_tail(&branch.name)) == wanted
                    || branch.aliases.iter().any(|alias| fold(alias) == wanted)
            }
            PathStage::Fragment => {
                fold(&branch.short_name).contains(wanted) || fold(&branch.name).contains(wanted)
            }
        }
    }
}

/// The part of a branch name behind its last ` / ` or `, `.
///
/// The list writes a branch under its house — `Humboldt-Universität zu Berlin,
/// Universitätsbibliothek, Zweigbibliothek Germanistik/Skandinavistik` — and the house is
/// already named on the left of the path, so the tail is what the user is naming.
fn name_tail(name: &str) -> &str {
    let cut = [" / ", ", "]
        .into_iter()
        .filter_map(|separator| name.rfind(separator).map(|at| at + separator.len()))
        .max();
    cut.map_or(name, |at| &name[at..])
}

/// The alias step, over institutions and branches alike.
fn by_alias(typed: &str) -> Option<Entry> {
    let wanted = fold(typed);
    for library in all() {
        if library.aliases.iter().any(|alias| fold(alias) == wanted) {
            return Some(Entry::Institution(library));
        }
        for branch in &library.branches {
            if branch.aliases.iter().any(|alias| fold(alias) == wanted) {
                return Some(Entry::Branch {
                    parent: library,
                    branch,
                });
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
///
/// `key` is how the house is addressed on the command line — its canonical alias, or the
/// bare ISIL for a house that has none. It is public because it is also the answer to
/// "what do I put in `--at`", which the detail view of a house prints.
pub fn institution_location(library: &Library) -> Location {
    let key = library.alias().unwrap_or(library.isil.as_str()).to_string();
    Location {
        // The canonical name, until [`resolve`] replaces it with the user's own word.
        // Callers that build a location from the list rather than from `--at` — the
        // detail view of a branch, say — never had a user word to carry.
        given: key.clone(),
        key,
        isil: Isil::new(&library.isil),
        branch: None,
        engine: Engine::Kobv,
        display: library.short_name.clone(),
    }
}

/// One branch, and the engine that answers for it.
///
/// **Which engine is decided by the parent and by nothing else.** A branch of the public
/// library network goes to `voebb`, where the house facet filters upstream and the result
/// is complete. Every other branch goes to `kobv`, where its house is filtered upstream
/// and the branch itself is read off the copies — the availability answer names one per
/// copy (`bibids=`), which the record never does. The two are not the same kind of
/// answer, and `plan/cli.md` § *Das Fensterproblem* says which is which; nothing here
/// branches on a specific ISIL beyond that one comparison.
///
/// `key` is what names this branch back: its shorthand, its own ISIL, or its KOBV id.
/// An ISIL a *house* also carries is skipped, because [`lookup`] answers that with the
/// house — a key that resolves to something else is not a key.
///
/// [`crate::model::BranchRef::name`] carries the branch's `short_name`, not its full
/// name: the full name starts with the house ("Stadtbibliothek Spandau / …"), which no
/// heading has room for and which the voebb.de house facet never repeats. The facet
/// labels its checkboxes `<district>: <house>`, and that second half equals `short_name`
/// for 42 of the 84 labels in `tests/fixtures/voebb/results.html` — including both
/// aliased branches, `AGB` and `BSTB`. Matching the remaining labels is the facet
/// parser's problem (`plan/voebb.md`), not this function's: it must never guess, and a
/// label it cannot place has to be a named error rather than an empty filter.
pub fn branch_location(library: &Library, branch: &Branch) -> Location {
    let own_isil = branch
        .isil
        .as_deref()
        .filter(|isil| by_isil_ignoring_case(isil).is_none());
    let key = branch
        .alias()
        .or(own_isil)
        .unwrap_or(branch.kobvid.as_str())
        .to_string();
    let voebb = library.isil == VOEBB_NETWORK;
    Location {
        // As in `institution_location`: the canonical name, which [`resolve`] overwrites
        // with what stood in `--at`.
        given: key.clone(),
        display: format!(
            "{} ({})",
            branch.alias().unwrap_or(&branch.short_name),
            if voebb {
                VOEBB_LABEL
            } else {
                &library.short_name
            }
        ),
        key,
        isil: Isil::new(&library.isil),
        branch: Some(BranchRef {
            kobvid: branch.kobvid.clone(),
            name: branch.short_name.clone(),
        }),
        engine: if voebb { Engine::Voebb } else { Engine::Kobv },
    }
}

/// The `--at` location of any entry, whichever kind it is.
///
/// The one call a renderer needs: a listing that mixes houses and branches has to print a
/// key beside every line, and that key must be the one `--at` resolves — not a field that
/// merely looks like an identifier. Which of the two functions above answers is a detail of
/// the entry, not of the caller.
pub fn entry_location(entry: Entry) -> Location {
    match entry {
        Entry::Institution(library) => institution_location(library),
        Entry::Branch { parent, branch } => branch_location(parent, branch),
    }
}

/// Every entry the list reaches under one ISIL, in the order [`lookup`] tries them.
///
/// The house first, because a house's ISIL is decided one step before any branch key is
/// looked at, then the branches in list order. One element is the ordinary answer; more
/// than one means the same code is claimed twice and only the first of them can ever be
/// named by it.
///
/// Nothing here compares against a particular code: the collision is found by asking what
/// two entries claim, so a new one shows up the day the data file grows it and an existing
/// one disappears the day the file stops.
fn claimants(isil: &str) -> Vec<Entry> {
    let mut found: Vec<Entry> = Vec::new();
    if let Some(library) = by_isil_ignoring_case(isil) {
        found.push(Entry::Institution(library));
    }
    for library in all() {
        for branch in &library.branches {
            if branch
                .isil
                .as_deref()
                .is_some_and(|own| own.eq_ignore_ascii_case(isil))
            {
                found.push(Entry::Branch {
                    parent: library,
                    branch,
                });
            }
        }
    }
    found
}

/// What **else** answers to a branch's own ISIL.
///
/// Empty for almost every branch, which is what an ISIL is for: a code that names one
/// place. A non-empty answer is the case the caller has to put into words — the branch's
/// own ISIL is printed beside it as a key, and typing that key back reaches something in
/// this list instead.
///
/// The entries come in resolution order, so the caller can name them the way the answer
/// would arrive: the house that owns the code, then the sibling branches that also carry
/// it. Which one `--at` actually lands on is [`isil_answers_elsewhere`], because a branch
/// can also be the first claimant of its own code and then nothing is wrong.
///
/// Also empty for a branch the list gives no ISIL to at all — there is no key to be wrong
/// about, and its KOBV id names it back.
pub fn shares_isil(branch: &Branch) -> Vec<Entry> {
    let Some(isil) = branch.isil.as_deref() else {
        return Vec::new();
    };
    claimants(isil)
        .into_iter()
        .filter(|entry| !is_branch(*entry, branch))
        .collect()
}

/// What `--at <this branch's own ISIL>` answers with, when that is not this branch.
///
/// `None` is the ordinary case and means the ISIL names the branch back. `Some` is the
/// expensive one: the search runs, it succeeds, and it answers about somewhere else
/// entirely — there is nothing in the result that looks wrong, so the only place this can
/// be said is beside the key itself.
///
/// The answer is derived from the same order [`lookup`] walks, not from a rule about which
/// kind of entry wins, so it stays true if that order ever changes.
pub fn isil_answers_elsewhere(branch: &Branch) -> Option<Entry> {
    let isil = branch.isil.as_deref()?;
    let first = *claimants(isil).first()?;
    (!is_branch(first, branch)).then_some(first)
}

/// What a key names **besides** the entry it answers with.
///
/// The mirror of [`shares_isil`], and the direction the user actually travels: that one
/// starts at a branch and asks what else claims the key printed beside it, this one starts
/// at the word in `--at` and asks what that word does not reach. Both read the same
/// [`claimants`] list, so the two can never disagree about which codes are shared.
///
/// Empty for every key that names one place, which is nearly all of them: an alias, a KOBV
/// id and 136 of the 137 branch ISILs. A non-empty answer is the silent case — [`lookup`]
/// resolves the key to the first claimant, the search runs, it succeeds, and it says
/// nothing at all about the entries standing behind it.
///
/// The order is [`claimants`]' order, so the caller may name the entries the way the
/// resolution would have reached them. Nothing here compares against a particular code:
/// the collision is found by asking what two entries of the list claim, so it appears the
/// day the data file grows one and disappears the day the file stops.
pub fn shadowed_by_key(typed: &str) -> Vec<Entry> {
    let typed = typed.trim();
    let Some(answered) = lookup(typed) else {
        return Vec::new();
    };
    claimants(typed)
        .into_iter()
        .filter(|entry| !same_entry(*entry, answered))
        .collect()
}

/// Whether two entries are the same place.
///
/// Houses are compared by ISIL and branches by KOBV id — both unique across the list, and
/// both invariants checked in `tests/libraries.rs` — rather than by address, so this still
/// answers correctly for an entry that was copied out of the list.
fn same_entry(left: Entry, right: Entry) -> bool {
    match (left, right) {
        (Entry::Institution(one), Entry::Institution(other)) => one.isil == other.isil,
        (_, Entry::Branch { branch, .. }) => is_branch(left, branch),
        _ => false,
    }
}

/// Whether an entry is this very branch.
///
/// Compared by KOBV id rather than by address: ids are unique across houses and branches
/// together (an invariant of the data file, checked in `tests/libraries.rs`), so this
/// still answers correctly for a branch that was cloned out of the list.
fn is_branch(entry: Entry, wanted: &Branch) -> bool {
    matches!(entry, Entry::Branch { branch, .. } if branch.kobvid == wanted.kobvid)
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

/// One house, and what of it a `--find` query matched.
///
/// The **grouped** shape is the point: a query that fits a dozen branches of one house
/// produces one of these, not a dozen, so no house can be pushed out of the answer by its
/// own branches. That was the reason branches were left out of the search altogether, and
/// it survives here without the false statement that came with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The house. Present whether or not it matched on its own.
    pub library: &'static Library,
    /// Whether the house's **own** text matched.
    ///
    /// `false` for a house that is here only as the heading its matching branches hang
    /// under — a renderer shows it, because a branch name means nothing without its house,
    /// but it is not itself one of the things the user was looking for.
    pub matched: bool,
    /// The branches of this house that matched, in list order.
    ///
    /// Empty is the ordinary case. Nothing is dropped here: how many of them are worth a
    /// line is the renderer's decision, and it needs the whole set to say how many there
    /// were.
    pub branches: Vec<&'static Branch>,
}

impl Found {
    /// How many things in this group matched — the house counts as one of them.
    ///
    /// For a caller that has to say what the search found before it decides how much of it
    /// to print.
    pub fn count(&self) -> usize {
        usize::from(self.matched) + self.branches.len()
    }
}

/// Free-text search over everything `--at` accepts, for `libraries --find`.
///
/// Case- and diacritic-insensitive substring matching, in list order, over a house's
/// aliases, short name, full name, city, ISIL and KOBV id, and over a branch's aliases,
/// short name, full name, ISIL and KOBV id. **A key the tool prints has to be a key the
/// tool finds**: the listing shows the ISIL as a column and the branch view shows the KOBV
/// id as the thing to type, so a search that could not match either was refusing to find
/// what it had just recommended.
///
/// Branches are searched but never listed beside houses — see [`Found`] for why that is
/// what keeps the old reasoning intact. A house appears when it matched itself, when one
/// of its branches matched, or both; [`Found::matched`] is what tells the two apart, and a
/// renderer that ignored it would offer a heading as an answer.
///
/// An empty query matches **nothing**, not everything: `--find ""` is a query that named
/// nothing, and the whole list is what the command prints without `--find` at all.
pub fn find_entries(query: &str) -> Vec<Found> {
    let wanted = fold(query.trim());
    if wanted.is_empty() {
        return Vec::new();
    }
    all()
        .iter()
        .filter_map(|library| {
            let matched = matches_text(library, &wanted);
            let branches: Vec<&'static Branch> = library
                .branches
                .iter()
                .filter(|branch| branch_matches_text(branch, &wanted))
                .collect();
            (matched || !branches.is_empty()).then_some(Found {
                library,
                matched,
                branches,
            })
        })
        .collect()
}

/// The fields of a house that `--find` searches: what `plan/libraries.md` §6 names, plus
/// the two keys the listing prints.
fn matches_text(library: &Library, folded_query: &str) -> bool {
    let fields = [
        &library.short_name,
        &library.name,
        &library.city,
        &library.isil,
    ];
    fields
        .into_iter()
        .chain(library.kobvid.iter())
        .chain(library.aliases.iter())
        .any(|field| fold(field).contains(folded_query))
}

/// The same for a branch. No city: a branch has an address, and the house's city applies
/// to it — matching the house is what puts a branch in the answer at all.
fn branch_matches_text(branch: &Branch, folded_query: &str) -> bool {
    let fields = [&branch.short_name, &branch.name, &branch.kobvid];
    fields
        .into_iter()
        .chain(branch.isil.iter())
        .chain(branch.aliases.iter())
        .any(|field| fold(field).contains(folded_query))
}

/// Every entry in the list — houses **and** branches — ordered by distance from a point.
///
/// The branches are the reason this exists. Most of them are the neighbourhood libraries
/// of the public network, and "which library is round the corner from me" is a question
/// about those, not about the head office of a union that has no reading room. A branch
/// answers with its own coordinates ([`Entry::coords`]), which is why naming the
/// Amerika-Gedenkbibliothek measures from Blücherplatz.
///
/// Entries the list places nowhere are left out rather than sorted to the front — every
/// entry has coordinates today, and the path exists because the list changes. The sort is
/// stable, so several entries in one building — a house and its main branch, say — keep
/// list order instead of shuffling between runs.
///
/// This is the whole list, not a page of it: how many of 300-odd entries are worth
/// printing is the caller's decision, and it cannot make it from a set already cut.
pub fn near_entries(point: LatLon) -> Vec<(Entry, f64)> {
    let mut ranked: Vec<(Entry, f64)> = entries()
        .filter_map(|entry| Some((entry, point.distance_km(entry.coords()?))))
        .collect();
    ranked.sort_by(|(_, a), (_, b)| a.total_cmp(b));
    ranked
}

/// Every addressable entry in the list: each house, immediately followed by its own
/// branches.
///
/// The one place that says what "everything in the list" means, so that a count and a
/// listing cannot disagree about it.
pub fn entries() -> impl Iterator<Item = Entry> {
    all().iter().flat_map(|library| {
        std::iter::once(Entry::Institution(library)).chain(library.branches.iter().map(
            move |branch| Entry::Branch {
                parent: library,
                branch,
            },
        ))
    })
}

/// Every branch in the list with the house that holds it, in list order, for
/// `libraries --branches`.
///
/// A flat list on purpose: this is the view that answers "which branches exist", and it is
/// the one thing `blibs libraries` cannot show. Grouped by house is [`houses_with_branches`],
/// which is the same data read the other way round.
pub fn branches() -> Vec<(&'static Library, &'static Branch)> {
    all()
        .iter()
        .flat_map(|library| library.branches.iter().map(move |branch| (library, branch)))
        .collect()
}

/// The houses that have branches at all, in list order.
///
/// The footer under `blibs libraries` counts these, and the number is **derived** — the
/// list is data, houses gain and lose branches, and a count written into a sentence is
/// wrong the first time that happens.
pub fn houses_with_branches() -> Vec<&'static Library> {
    all()
        .iter()
        .filter(|library| !library.branches.is_empty())
        .collect()
}

/// How many branches the whole list holds. Derived, for the same reason.
pub fn branch_count() -> usize {
    all().iter().map(|library| library.branches.len()).sum()
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
            score(&wanted, bound, alias).map(|(rank, distance)| (rank, distance, alias.as_str()))
        })
        .collect();
    scored.sort_unstable();
    scored.truncate(SUGGEST_LIMIT);
    scored
        .into_iter()
        .map(|(_, _, alias)| alias.to_string())
        .collect()
}

/// How close a candidate is to what was typed, or `None` when it is not close at all.
///
/// The two rules of [`suggest`], in the one place both callers read them from. The first
/// element ranks: `false` sorts before `true`, so prefix hits come first and the edit
/// distance decides within them.
fn score(wanted: &str, bound: usize, candidate: &str) -> Option<(bool, usize)> {
    let folded = fold(candidate);
    let is_prefix = folded.starts_with(wanted) || wanted.starts_with(&folded);
    // The distance itself is only ever used to rank, never to disqualify a prefix hit, so
    // it is computed against a bound generous enough to be exact for anything this short
    // rather than the (possibly stricter) `bound`.
    let ceiling = wanted.chars().count().max(folded.chars().count());
    let distance = levenshtein(wanted, &folded, ceiling);
    (is_prefix || distance <= bound).then_some((!is_prefix, distance))
}

/// Up to three whole paths close to a branch name that named nothing.
///
/// The counterpart of [`suggest`] for the one step it cannot answer for: aliases are the
/// wrong vocabulary behind a slash, because 208 of the 211 branches have none.
///
/// What is scored instead are the branch's **single words**, not only its whole names: a
/// path is typed as a fragment (`HU/Germanistik`), and comparing `gremanistik` against
/// `Zweigbibliothek Germanistik/Skandinavistik` finds nothing at any edit distance worth
/// having. What is *offered* is still the whole name, taken from behind the last
/// separator, because that is the form the path step matches exactly.
///
/// Each suggestion is a whole path, with the house spelled the way it was typed, so that
/// it can be pasted back without editing.
fn suggest_branches(parent: &'static Library, house: &str, wanted: &str) -> Vec<String> {
    let bound = max_distance(wanted.chars().count());
    let mut scored: Vec<(bool, usize, &str)> = parent
        .branches
        .iter()
        .filter_map(|branch| {
            let name = name_tail(&branch.name);
            let best = [branch.short_name.as_str(), name]
                .into_iter()
                .flat_map(words)
                .filter_map(|word| score(wanted, bound, word))
                .min()?;
            Some((best.0, best.1, name))
        })
        .collect();
    scored.sort_unstable();
    scored.dedup_by(|a, b| a.2 == b.2);
    scored.truncate(SUGGEST_LIMIT);
    scored
        .into_iter()
        .map(|(_, _, name)| format!("{house}{PATH_SEPARATOR}{name}"))
        .collect()
}

/// A name and the words in it, as things a fragment could have been aiming at.
///
/// The whole string comes first so that a one-word name is not scored twice, and the
/// split is on everything that is not a letter or a digit — `Germanistik/Skandinavistik`
/// and `Kaulsdorf - Nord` are two words each, however they are punctuated.
fn words(name: &str) -> impl Iterator<Item = &str> {
    std::iter::once(name).chain(
        name.split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty()),
    )
}
