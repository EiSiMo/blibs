//! The branch facet on the result page.
//!
//! The facet is the only place in either catalogue that knows which *branch* holds a
//! copy, and ticking one of its boxes is a real upstream filter (71 hits → 35, exactly
//! the number the facet states). Everything else about it is hostile to a program:
//!
//! - **There is no key.** No ISIL, no KOBV id, nothing but the display name. A branch is
//!   found by its label and by nothing else.
//! - **The checkbox id is a running index over the rendered tree**, not an identifier:
//!   the AGB is `sub-PTL1_tree_1_90` on the unfiltered page and `sub-PTL1_tree_1_92` on
//!   the filtered one, because an *Aktive/r Filter* rubric appeared above it. It must be
//!   read from the page it is going to be sent back to.
//! - **The name vocabulary differs from the library list's.** Of the 84 branches the
//!   facet listed, 41 match a `short_name` exactly; the rest need the full name, the
//!   `match` strings from the list, or a substring — and some are ambiguous until the
//!   district in front of the colon decides.
//!
//! A branch that is absent from the tree is **not** an error: the facet only lists
//! branches that have hits, so absence means "no hits here", which is an honest answer.
//! [`BranchFacet::lookup`] says so with `Ok(None)`; [`BranchFacet::find`] is the strict
//! variant for the caller that must apply a filter.
//!
//! Fixtures: `tests/fixtures/voebb/results.html` (85 entries, AGB at index 90) and
//! `results_filtered.html` (the same entries, shifted).

use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};

use crate::error::{Error, UnexpectedError};
use crate::libraries::{Branch, text};

use super::{collapse, compile};

/// The document name used in errors from this module.
const DOCUMENT: &str = "voebb.de branch facet";

/// The rubric of the facet tree that lists branches.
const RUBRIC: &str = "Bibliothek";

/// The separator between the district and the branch name in a facet label.
const DISTRICT_SEPARATOR: &str = ": ";

/// One branch offered by the facet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetEntry {
    /// The label as the tree has it, e.g. `ZLB: Amerika-Gedenkbibliothek (AGB)`.
    pub label: String,
    /// The checkbox element's **id**, which is what `$CbTree_text` carries. Valid only
    /// for the page it was read from.
    pub checkbox_id: String,
    /// The hit count the facet states. `None` when the tree states none — reported
    /// either way, because it stays a true statement even when the filter is not applied.
    pub count: Option<u64>,
}

impl FacetEntry {
    /// The branch name without the district in front of it.
    pub fn name(&self) -> &str {
        match self.label.split_once(DISTRICT_SEPARATOR) {
            Some((_, name)) => name,
            None => &self.label,
        }
    }

    /// The district in front of the colon, when the label has one. `in allen
    /// Bibliotheken`, the tree's own collective entry, has none.
    pub fn district(&self) -> Option<&str> {
        self.label
            .split_once(DISTRICT_SEPARATOR)
            .map(|(district, _)| district)
    }
}

/// The `Bibliothek` rubric of one result page's facet tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchFacet {
    /// Every branch the tree lists, in document order.
    pub entries: Vec<FacetEntry>,
}

impl BranchFacet {
    /// Find the entry for a branch, or say that the tree does not list it.
    ///
    /// Three stages, weakest last, so that a strong match is never overruled by a weak
    /// one:
    ///
    /// 1. the label's name folds equal to the branch's `short_name`,
    /// 2. it folds equal to the branch's full name, to that name without its
    ///    `Stadtbibliothek <district> /` prefix, or to one of the `match` strings the
    ///    library list carries for exactly this purpose,
    /// 3. one of the two is a substring of the other **and the districts agree**.
    ///
    /// The district is what separates `Mitte: Kurt-Tucholsky-Bibliothek` from `Pankow:
    /// Kurt-Tucholsky-Bibliothek`. In the first two stages it only breaks a tie, and a
    /// narrowing that leaves nothing is discarded rather than applied. In the third it is
    /// a precondition, because a substring is weak enough to be wrong on its own:
    /// `Bezirkszentralbibliothek Philipp Schaeffer` contains
    /// `Bezirkszentralbibliothek`, which is another district's whole name.
    ///
    /// `Ok(None)` means the tree does not list this branch, i.e. it has no hits for this
    /// search. An error means the tree lists several and the name cannot decide between
    /// them — never a silent pick.
    pub fn lookup(&self, branch: &Branch) -> Result<Option<&FacetEntry>, Error> {
        for stage in [Stage::ShortName, Stage::AlternateNames, Stage::Substring] {
            let candidates = self.candidates(branch, stage);
            let candidates = narrow_by_district(candidates, branch);
            match candidates.as_slice() {
                [] => {}
                [only] => return Ok(Some(only)),
                _ => return Err(ambiguous(branch, &candidates)),
            }
        }
        Ok(None)
    }

    /// Find the entry for a branch, treating its absence as an error.
    ///
    /// For the caller that has to apply the filter and cannot fall back: without an
    /// entry there is no `$CbTree_text` to send, and reporting the unfiltered result
    /// would answer a different question than the one asked.
    pub fn find(&self, branch: &Branch) -> Result<&FacetEntry, Error> {
        self.lookup(branch)?.ok_or_else(|| {
            UnexpectedError::MissingElement {
                what: format!(
                    "a facet entry for {:?} among the {} branches the page lists",
                    branch.short_name,
                    self.entries.len()
                ),
                context: DOCUMENT.to_string(),
            }
            .into()
        })
    }

    /// The entries one stage accepts for this branch.
    fn candidates(&self, branch: &Branch, stage: Stage) -> Vec<&FacetEntry> {
        let keys: Vec<String> = stage
            .keys(branch)
            .iter()
            .map(|key| text::fold(key))
            .collect();
        let district = district_key(&branch.name);
        self.entries
            .iter()
            .filter(|entry| {
                if stage.requires_district()
                    && entry.district().map(district_key).as_deref() != Some(district.as_str())
                {
                    return false;
                }
                let name = text::fold(entry.name());
                keys.iter().any(|key| stage.accepts(&name, key))
            })
            .collect()
    }
}

/// One stage of the match, from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The list's short name, which is what the tool prints.
    ShortName,
    /// The full name, the name without its district prefix, and the `match` strings.
    AlternateNames,
    /// Either string contains the other.
    Substring,
}

impl Stage {
    /// The branch strings this stage compares against.
    ///
    /// Borrowed from the branch: every one of them is folded before it is compared, so
    /// copying it first would only be an allocation thrown away one line later.
    fn keys(self, branch: &Branch) -> Vec<&str> {
        match self {
            Stage::ShortName | Stage::Substring => vec![branch.short_name.as_str()],
            Stage::AlternateNames => {
                let mut keys = vec![branch.name.as_str(), after_prefix(&branch.name)];
                for matched in &branch.match_strings {
                    keys.push(matched.as_str());
                    keys.push(after_prefix(matched));
                }
                keys
            }
        }
    }

    /// Whether a folded label name and a folded key match under this stage.
    fn accepts(self, name: &str, key: &str) -> bool {
        match self {
            Stage::ShortName | Stage::AlternateNames => name == key,
            Stage::Substring => name.contains(key) || key.contains(name),
        }
    }

    /// Whether this stage only compares entries of the branch's own district.
    fn requires_district(self) -> bool {
        matches!(self, Stage::Substring)
    }
}

/// Narrow several candidates to those whose district matches the branch's, unless that
/// leaves none.
fn narrow_by_district<'e>(candidates: Vec<&'e FacetEntry>, branch: &Branch) -> Vec<&'e FacetEntry> {
    if candidates.len() < 2 {
        return candidates;
    }
    let district = district_key(&branch.name);
    let narrowed: Vec<&FacetEntry> = candidates
        .iter()
        .copied()
        .filter(|entry| {
            entry
                .district()
                .is_some_and(|label| district_key(label) == district)
        })
        .collect();
    if narrowed.is_empty() {
        candidates
    } else {
        narrowed
    }
}

/// The part of a library-list name after its `Stadtbibliothek <district> /` prefix.
///
/// The list writes a branch as `Stadtbibliothek Pankow / Bibliothek Buch` and the facet
/// as `Pankow: Bibliothek Buch`; either the slash or a comma separates the two halves.
fn after_prefix(name: &str) -> &str {
    for separator in [" / ", ", "] {
        if let Some((_, tail)) = name.split_once(separator) {
            return tail;
        }
    }
    name
}

/// A district reduced to something the two vocabularies agree on.
///
/// The list spells districts as they are transliterated for ASCII (`Treptow-Koepenick`,
/// `Neukoelln`), the facet keeps the umlauts (`Treptow-Köpenick`). Folding alone leaves
/// `koepenick` next to `kopenick`, so the German digraphs are collapsed too. This is only
/// ever used to choose **between candidates that already matched by name**, never to
/// produce a match on its own — the collapse is too crude for that (`Museum` would become
/// `musum`).
fn district_key(name: &str) -> String {
    let district = name
        .split_once(DISTRICT_SEPARATOR)
        .map_or(name, |(district, _)| district);
    let district = district
        .split([',', '/'])
        .next()
        .unwrap_or(district)
        .trim()
        .trim_start_matches("Stadtbibliothek")
        .trim();
    text::fold(district)
        .replace("oe", "o")
        .replace("ae", "a")
        .replace("ue", "u")
}

/// The error for a branch the facet lists more than once under names that cannot be told
/// apart. It names the branch and every candidate, because the fix is a `match` string in
/// the library list and that needs both halves.
fn ambiguous(branch: &Branch, candidates: &[&FacetEntry]) -> Error {
    let labels: Vec<&str> = candidates
        .iter()
        .map(|entry| entry.label.as_str())
        .collect();
    UnexpectedError::MissingElement {
        what: format!(
            "an unambiguous facet entry for {:?}; {} entries match it ({})",
            branch.short_name,
            candidates.len(),
            labels.join(", ")
        ),
        context: DOCUMENT.to_string(),
    }
    .into()
}

/// Read the branch facet of a result page.
///
/// Selectors: `div#PTL1_tree_1` (the tree), `li.cbtree_branch_li` with a
/// `button.cbtree_branch_a` reading `Bibliothek` (the rubric), and below it
/// `li.cbtree_leaf_li` with `input[type="checkbox"]` and `a.cbtree_leaf_a`.
///
/// A tree or a rubric that is gone is [`UnexpectedError::MissingSelector`]. An empty
/// rubric is not: a search whose hits are all online has no branch in it.
pub fn parse_facet(html: &str) -> Result<BranchFacet, Error> {
    let document = Html::parse_document(html);
    let tree = document
        .select(&selectors().tree)
        .next()
        .ok_or_else(|| missing_selector("div#PTL1_tree_1"))?;
    let rubric = tree
        .select(&selectors().rubric)
        .find(is_branch_rubric)
        .ok_or_else(|| {
            missing_selector("div#PTL1_tree_1 li.cbtree_branch_li (rubric \"Bibliothek\")")
        })?;

    let entries = rubric
        .select(&selectors().leaf)
        .filter_map(entry)
        .collect::<Vec<_>>();
    Ok(BranchFacet { entries })
}

/// Whether this rubric of the tree is the one listing branches. The rubrics are
/// `Medienart`, `Bibliothek`, `Schlagwort`, `Jahr`, `Sprache`, `Zielgruppe` and
/// `Art/Inhalt`; only their heading text tells them apart.
fn is_branch_rubric(rubric: &ElementRef<'_>) -> bool {
    rubric
        .select(&selectors().rubric_heading)
        .next()
        .is_some_and(|heading| collapse(&heading.text().collect::<String>()) == RUBRIC)
}

/// One leaf of the rubric. A leaf without a checkbox id cannot be sent back, so it is
/// dropped rather than offered as something that can be filtered on.
fn entry(leaf: ElementRef<'_>) -> Option<FacetEntry> {
    let checkbox_id = leaf
        .select(&selectors().checkbox)
        .next()?
        .value()
        .attr("id")?
        .to_string();
    let link = leaf.select(&selectors().leaf_label).next()?;
    let count = link
        .select(&selectors().count)
        .next()
        .and_then(|count| parse_count(&collapse(&count.text().collect::<String>())));
    let label = collapse(
        &link
            .children()
            .filter_map(|child| child.value().as_text().map(|text| text.to_string()))
            .collect::<String>(),
    );
    (!label.is_empty()).then_some(FacetEntry {
        label,
        checkbox_id,
        count,
    })
}

/// The number out of a `(35)` count badge.
fn parse_count(text: &str) -> Option<u64> {
    text.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .replace('.', "")
        .parse()
        .ok()
}

/// The compiled selectors. Every one is a literal in this file, so a parse failure would
/// be a typo in this source and nothing a user or the service can cause.
struct Selectors {
    tree: Selector,
    rubric: Selector,
    rubric_heading: Selector,
    leaf: Selector,
    checkbox: Selector,
    leaf_label: Selector,
    count: Selector,
}

/// The selectors, compiled on first use.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(|| Selectors {
        tree: compile("div#PTL1_tree_1"),
        rubric: compile("li.cbtree_branch_li"),
        rubric_heading: compile("button.cbtree_branch_a"),
        leaf: compile("li.cbtree_leaf_li"),
        checkbox: compile("input[type=\"checkbox\"]"),
        leaf_label: compile("a.cbtree_leaf_a"),
        count: compile("span.count"),
    })
}

/// A missing-selector error naming what stopped matching in this document.
fn missing_selector(selector: &str) -> Error {
    super::missing_selector(selector, DOCUMENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libraries;

    const RESULTS: &str = include_str!("../../../../tests/fixtures/voebb/results.html");
    const FILTERED: &str = include_str!("../../../../tests/fixtures/voebb/results_filtered.html");

    fn parsed(html: &str) -> BranchFacet {
        match parse_facet(html) {
            Ok(facet) => facet,
            Err(error) => panic!("fixture must parse: {error}"),
        }
    }

    /// A branch of the VÖBB network, by the short name the library list gives it.
    fn branch(short_name: &str) -> &'static Branch {
        libraries::all()
            .iter()
            .flat_map(|library| &library.branches)
            .find(|branch| branch.short_name == short_name)
            .unwrap_or_else(|| panic!("{short_name} must be in the library list"))
    }

    /// An invented branch, for the case that has to fail.
    fn invented(short_name: &str) -> Branch {
        Branch {
            kobvid: "KOB99999".to_string(),
            isil: None,
            name: format!("Stadtbibliothek Nirgendwo / {short_name}"),
            short_name: short_name.to_string(),
            aliases: Vec::new(),
            lat: 0.0,
            lon: 0.0,
            address: None,
            match_strings: Vec::new(),
        }
    }

    /// The rubric lists 84 branches plus the tree's own collective entry.
    #[test]
    fn reads_every_branch_of_the_rubric() {
        let facet = parsed(RESULTS);
        assert_eq!(facet.entries.len(), 85);
        assert!(
            facet
                .entries
                .iter()
                .any(|entry| entry.label == "in allen Bibliotheken")
        );
    }

    /// The AGB, with the count that the filter later confirms: 35 of 71.
    #[test]
    fn finds_the_agb_with_its_count() {
        let facet = parsed(RESULTS);
        let entry = facet
            .find(branch("Amerika-Gedenkbibliothek (AGB)"))
            .expect("the AGB is in the tree");
        assert_eq!(entry.checkbox_id, "sub-PTL1_tree_1_90");
        assert_eq!(entry.count, Some(35));
        assert_eq!(entry.label, "ZLB: Amerika-Gedenkbibliothek (AGB)");
    }

    /// The same house on the filtered page: the same branch, a different checkbox id.
    /// This is the whole reason the id is read per page instead of remembered.
    #[test]
    fn the_checkbox_id_shifts_between_pages() {
        let unfiltered = parsed(RESULTS);
        let filtered = parsed(FILTERED);
        let agb = branch("Amerika-Gedenkbibliothek (AGB)");
        let before = unfiltered.find(agb).expect("the AGB is in the tree");
        let after = filtered.find(agb).expect("the AGB is still in the tree");
        assert_eq!(before.label, after.label);
        assert_ne!(before.checkbox_id, after.checkbox_id);
        assert_eq!(after.checkbox_id, "sub-PTL1_tree_1_92");
    }

    /// The second branch that has a spoken alias; its facet label abbreviates the name in
    /// brackets exactly as the list does.
    #[test]
    fn finds_the_berliner_stadtbibliothek() {
        let facet = parsed(RESULTS);
        let entry = facet
            .find(branch("Berliner Stadtbibliothek (BStB)"))
            .expect("the BStB is in the tree");
        assert_eq!(entry.label, "ZLB: Berliner Stadtbibliothek (BStB)");
        assert_eq!(entry.name(), "Berliner Stadtbibliothek (BStB)");
        assert_eq!(entry.district(), Some("ZLB"));
    }

    /// Stage two: the facet abbreviates (`Bibl. Reinickendorf-West`) where the list
    /// writes it out, and the `match` string in the library list is what bridges the two.
    #[test]
    fn finds_a_branch_through_its_match_string() {
        let facet = parsed(RESULTS);
        let entry = facet
            .find(branch("Bibliothek Reinickendorf - West"))
            .expect("the branch is in the tree");
        assert_eq!(entry.label, "Reinickendorf: Bibl. Reinickendorf-West");
    }

    /// Stage three: the list qualifies the name with the district (`Hauptbibliothek
    /// Spandau`), the facet puts the district in front of the colon instead.
    #[test]
    fn finds_a_branch_by_substring() {
        let facet = parsed(RESULTS);
        let entry = facet
            .find(branch("Hauptbibliothek Spandau"))
            .expect("the branch is in the tree");
        assert_eq!(entry.label, "Spandau: Hauptbibliothek");
    }

    /// The substring stage only ever compares within one district. A house of Mitte whose
    /// name contains the whole label of `Tempelhof-Schöneberg: Bezirkszentralbibliothek`
    /// must not be matched to it — that would be worse than not matching at all.
    ///
    /// The branch is invented rather than taken from the list on purpose: the real house
    /// this used to be written with, `Bezirkszentralbibliothek Philipp Schaeffer`, now
    /// carries a `match` string and never reaches the substring stage, so it would test
    /// nothing.
    #[test]
    fn a_substring_never_crosses_a_district() {
        let facet = parsed(RESULTS);
        let mut foreign = invented("Bezirkszentralbibliothek Nirgendwo");
        foreign.name = "Stadtbibliothek Mitte / Bezirkszentralbibliothek Nirgendwo".to_string();
        let entry = facet.lookup(&foreign).expect("must not be ambiguous");
        assert_eq!(entry, None);
    }

    /// Two houses share a name across districts. The district in front of the colon is
    /// what tells them apart, and it has to survive the two spellings of `Köpenick`.
    #[test]
    fn the_district_separates_two_houses_of_the_same_name() {
        let facet = parsed(RESULTS);
        let mitte = facet
            .find(branch("Kurt-Tucholsky-Bibliothek (Mitte)"))
            .expect("Mitte's Tucholsky is in the tree");
        let pankow = facet
            .find(branch("Kurt-Tucholsky-Bibliothek (Pankow)"))
            .expect("Pankow's Tucholsky is in the tree");
        assert_eq!(mitte.label, "Mitte: Kurt-Tucholsky-Bibliothek");
        assert_eq!(pankow.label, "Pankow: Kurt-Tucholsky-Bibliothek");
    }

    /// A branch that is not in the tree has no hits for this search. That is an answer,
    /// not a broken page — `lookup` says so, and only `find` turns it into an error.
    #[test]
    fn a_branch_without_hits_is_not_an_error() {
        let facet = parsed(RESULTS);
        let invented = invented("Bibliothek Nirgendwo");
        assert!(matches!(facet.lookup(&invented), Ok(None)));
    }

    /// `find` names the branch it could not place, because that name is the only thing
    /// that says what to add to the library list.
    #[test]
    fn find_names_the_branch_it_cannot_place() {
        let facet = parsed(RESULTS);
        let invented = invented("Bibliothek Nirgendwo");
        let error = facet.find(&invented).expect_err("must not be accepted");
        let message = error.to_string();
        assert!(message.contains("Bibliothek Nirgendwo"), "{message}");
        assert!(message.contains("85"), "{message}");
    }

    /// A name that fits several entries and no district is a named error listing all of
    /// them — never a silent pick between two houses.
    #[test]
    fn an_ambiguous_name_names_every_candidate() {
        let facet = parsed(RESULTS);
        let mut ambiguous = invented("Fahrbibliothek");
        ambiguous.name = "Bücherei Nirgendwo / Fahrbibliothek".to_string();
        let error = facet.find(&ambiguous).expect_err("must not be accepted");
        let message = error.to_string();
        assert!(
            message.contains("Reinickendorf: Fahrbibliothek"),
            "{message}"
        );
        assert!(
            message.contains("Treptow-Köpenick: Fahrbibliothek"),
            "{message}"
        );
    }

    /// What a `match` string is *for*, on the case that used to be the module's own
    /// example of an unresolvable name.
    ///
    /// The list calls the house `Musikbibliothek (in der Mark-Twain-Bibliothek)`, and the
    /// tree offers both `Marzahn-Hellersdorf: Musikbibliothek` and `Marzahn-Hellersdorf:
    /// Mark-Twain-Bibliothek`. The substring stage cannot choose between them and used to
    /// report both as an error; the observed label, written into `match`, decides it one
    /// stage earlier — and its neighbour keeps its own entry rather than losing it to the
    /// tie.
    #[test]
    fn a_match_string_settles_a_tie_the_substring_stage_cannot() {
        let facet = parsed(RESULTS);
        let music = facet
            .find(branch("Musikbibliothek (in der Mark-Twain-Bibliothek)"))
            .expect("the match string decides it");
        let twain = facet
            .find(branch("Bezirkszentralbibliothek Mark Twain"))
            .expect("the match string decides it");
        assert_eq!(music.label, "Marzahn-Hellersdorf: Musikbibliothek");
        assert_eq!(twain.label, "Marzahn-Hellersdorf: Mark-Twain-Bibliothek");
    }

    /// A tree that moved names its selector rather than reporting a facet with no
    /// branches in it.
    #[test]
    fn a_missing_tree_names_its_selector() {
        let broken = RESULTS.replace("id=\"PTL1_tree_1\"", "id=\"PTL1_tree_2\"");
        let error = parse_facet(&broken).expect_err("must not be accepted");
        assert!(error.to_string().contains("PTL1_tree_1"), "{error}");
    }

    /// A tree without the branch rubric is the same kind of failure: every other rubric
    /// is still there, so an empty result would look like "no branch has this".
    #[test]
    fn a_missing_rubric_names_it() {
        let broken = RESULTS.replace(">Bibliothek<", ">Haeuser<");
        let error = parse_facet(&broken).expect_err("must not be accepted");
        assert!(error.to_string().contains("Bibliothek"), "{error}");
    }

    /// The labels the tree lists that no branch of the list answers for.
    ///
    /// Three houses voebb.de offers are absent from `data/libraries.json` — the two
    /// outlying stacks, which are storage rather than branches, and the larger of the two
    /// Treptow-Köpenick buses, which the KOBV directory does not carry (only the *Kleiner
    /// Bus* is in it). Plus the tree's own collective entry, which is not a house at all.
    ///
    /// They are named here rather than tolerated by a threshold: a label that stops
    /// resolving has to show up as a failure, and one that was never ours has to be
    /// visible as a decision instead of hiding inside a count.
    const UNCLAIMED: [&str; 4] = [
        "ZLB: Außenmagazin Amerika-Gedenkbibliothek",
        "ZLB: Außenmagazin Berliner Stadtbibliothek",
        "Treptow-Köpenick: Fahrbibliothek",
        "in allen Bibliotheken",
    ];

    /// Labels that answer for **two** houses of the list, because the tree's vocabulary is
    /// coarser than the directory's.
    ///
    /// Steglitz-Zehlendorf runs two mobile libraries and the KOBV directory carries both;
    /// voebb.de offers one checkbox for the service. Ticking it is the right answer for
    /// either bus, so this is the vocabularies disagreeing about granularity, not a
    /// mismatch — but it is listed rather than tolerated, so that a *new* label with two
    /// claimants fails instead of passing quietly.
    const SHARED: [&str; 1] = ["Steglitz-Zehlendorf: Fahrbibliothek"];

    /// How far the two vocabularies agree, measured rather than assumed.
    ///
    /// The strong form of the question: **every** label the tree lists is answered by
    /// exactly one branch, except the four in [`UNCLAIMED`]. A branch that resolves to
    /// the wrong house therefore leaves the label it *should* have taken free, and the
    /// test names it.
    ///
    /// The reverse direction is deliberately not asserted: 15 branches of the network do
    /// not appear in this tree at all, because this one search has no hits at them. That
    /// is an honest `Ok(None)` (see [`BranchFacet::lookup`]), not a gap in the mapping.
    #[test]
    fn every_label_of_the_tree_is_claimed_by_one_branch() {
        let facet = parsed(RESULTS);
        let branches: Vec<&Branch> = libraries::all()
            .iter()
            .filter(|library| library.isil == "DE-609")
            .flat_map(|library| &library.branches)
            .collect();

        let mut claimed: Vec<(&str, &str)> = Vec::new();
        for branch in &branches {
            match facet.lookup(branch) {
                Ok(Some(entry)) => claimed.push((entry.label.as_str(), branch.short_name.as_str())),
                Ok(None) => {}
                Err(error) => panic!("{}: {error}", branch.short_name),
            }
        }

        for entry in &facet.entries {
            let takers: Vec<&str> = claimed
                .iter()
                .filter(|(label, _)| *label == entry.label)
                .map(|(_, branch)| *branch)
                .collect();
            let expected = if UNCLAIMED.contains(&entry.label.as_str()) {
                0
            } else if SHARED.contains(&entry.label.as_str()) {
                2
            } else {
                1
            };
            assert_eq!(
                takers.len(),
                expected,
                "{:?} is claimed by {takers:?}",
                entry.label
            );
        }
    }
}
