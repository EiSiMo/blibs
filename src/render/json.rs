//! JSON output: one document on stdout, and nothing else.
//!
//! The schema is derived from the domain types with `serde`, so it cannot drift away from
//! what the human renderer shows. Member order is part of the contract; adding a member
//! is allowed, renaming or removing one is a break.
//!
//! The one type that needs a view of its own is [`crate::libraries::Library`]: the library
//! list is *input* data, deserialised from a compiled-in file, and serialising it back
//! verbatim would publish its matching apparatus (`match`, `portal_name`, `kobvid`) as if
//! it were part of the interface. [`LibraryView`] states what `libraries --json` promises.

use std::io::Write;

use crate::error::{Error, ErrorEnvelope};
use crate::libraries::{Branch, Entry, Found, Library};
use crate::model::{Engine, Location};

/// Write one document, pretty-printed and closed by a newline.
///
/// The newline is not decoration: it is what makes the output usable from a shell and
/// line-oriented tooling without a trailing-byte special case.
///
/// A serialisation failure and a failed write both land in
/// [`crate::error::UnexpectedError::Output`] — from the caller's side they are the same
/// thing, "the answer could not be delivered", and neither is the user's mistake.
pub fn write<T: serde::Serialize>(value: &T, out: &mut dyn Write) -> Result<(), Error> {
    serde_json::to_writer_pretty(&mut *out, value).map_err(std::io::Error::from)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Write the error object — `{ "error": { code, kind, message, hint } }`.
///
/// **The only place it is built.** `code`, `kind`, `message` and `hint` come from
/// [`Error::exit`], [`Error::kind`], the error's `Display` and [`Error::hint`]
/// respectively, so a new variant cannot forget one of them.
pub fn error(error: &Error, out: &mut dyn Write) -> Result<(), Error> {
    write(&ErrorEnvelope::from(error), out)
}

/// Which kind of thing a `libraries --json` object describes.
///
/// Serialised as the `type` member, and it is the **first** member of every such object:
/// `libraries AGB` answers about a branch and `libraries STABI` about an institution, and
/// an agent must be able to read which of the two it holds instead of inferring it from
/// which members happen to be present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A whole house — [`LibraryView`].
    Institution,
    /// One branch of a house — [`BranchDetailView`].
    Branch,
}

/// One library, as `libraries --json` publishes it.
///
/// Borrowed rather than owned: the list is `'static`, and copying 123 entries to print
/// them would be work with no purpose. Member order is the document's member order.
#[derive(Debug, serde::Serialize)]
pub struct LibraryView<'a> {
    /// Always [`EntryKind::Institution`]. See there for why it is stated rather than
    /// implied.
    #[serde(rename = "type")]
    pub kind_of_entry: EntryKind,
    /// The shorthand `--at` takes, or `null` where the list carries no spoken one. Never
    /// invented (`plan/libraries.md` §5, rule 5).
    pub alias: Option<&'a str>,
    /// Every shorthand for this house — a house may have several (`STABI`, `SBB`).
    pub aliases: &'a [String],
    /// The ISIL, which is what identifies the house in a record's `924` fields.
    pub isil: &'a str,
    /// The short name used in tables and holdings lines.
    pub short_name: &'a str,
    /// The full name.
    pub name: &'a str,
    /// The kind of institution, as the list states it.
    pub kind: Option<&'a str>,
    /// City.
    pub city: &'a str,
    /// Street address.
    pub address: &'a str,
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// Telephone, as the list states it.
    pub phone: Option<&'a str>,
    /// E-mail.
    pub email: Option<&'a str>,
    /// Website.
    pub url: Option<&'a str>,
    /// Catalogue, for the questions this tool cannot answer — due dates, holds, holdings
    /// runs.
    pub opac: Option<&'a str>,
    /// Distance in kilometres, present only for `--near`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_km: Option<f64>,
    /// The branches of this house.
    pub branches: Vec<BranchView<'a>>,
}

/// One branch of a library, as it appears **inside** its house's document. Only branches
/// that are actually spoken about carry a shorthand; the rest have none rather than an
/// invented one.
///
/// The standalone form is [`BranchDetailView`], which adds the house and how to search it.
/// This one is nested under a house that states both already.
#[derive(Debug, serde::Serialize)]
pub struct BranchView<'a> {
    /// Always [`EntryKind::Branch`], and the first member, exactly as in every other
    /// object of this command — `plan/cli.md` promises that of *every* element, and an
    /// agent that walks a document should not have to know whether it descended into a
    /// house to know what it is holding.
    #[serde(rename = "type")]
    pub kind_of_entry: EntryKind,
    /// The portal's own id for the branch, which is what `items[].branch` carries.
    pub kobvid: &'a str,
    /// The branch's own ISIL, where it has one.
    pub isil: Option<&'a str>,
    /// Full name.
    pub name: &'a str,
    /// Short name.
    pub short_name: &'a str,
    /// Shorthands for this branch, usually empty.
    pub aliases: &'a [String],
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// Street address, where the list has one.
    pub address: Option<&'a str>,
}

impl<'a> From<&'a Library> for LibraryView<'a> {
    fn from(library: &'a Library) -> Self {
        LibraryView {
            kind_of_entry: EntryKind::Institution,
            alias: library.alias(),
            aliases: &library.aliases,
            isil: &library.isil,
            short_name: &library.short_name,
            name: &library.name,
            kind: library.kind.as_deref(),
            city: &library.city,
            address: &library.address,
            lat: library.lat,
            lon: library.lon,
            phone: library.phone.as_deref(),
            email: library.email.as_deref(),
            url: library.url.as_deref(),
            opac: library.opac.as_deref(),
            distance_km: None,
            branches: library.branches.iter().map(BranchView::from).collect(),
        }
    }
}

impl<'a> From<&'a Branch> for BranchView<'a> {
    fn from(branch: &'a Branch) -> Self {
        BranchView {
            kind_of_entry: EntryKind::Branch,
            kobvid: &branch.kobvid,
            isil: branch.isil.as_deref(),
            name: &branch.name,
            short_name: &branch.short_name,
            aliases: &branch.aliases,
            lat: branch.lat,
            lon: branch.lon,
            address: branch.address.as_deref(),
        }
    }
}

/// One branch, as `libraries <branch shorthand> --json` publishes it.
///
/// A view of its own rather than the parent's [`LibraryView`]: a branch has an address, a
/// KOBV id and possibly an ISIL of its own, it has **no** branches of its own, and the
/// answer to "how do I search it" is not the answer for its house. Handing back the house
/// would answer a different question than the one asked, without saying so.
#[derive(Debug, serde::Serialize)]
pub struct BranchDetailView<'a> {
    /// Always [`EntryKind::Branch`].
    #[serde(rename = "type")]
    pub kind_of_entry: EntryKind,
    /// The shorthand that names this branch. Only the branches that are actually spoken
    /// about have one; the rest are addressed by their `kobvid`.
    pub alias: Option<&'a str>,
    /// Every shorthand for this branch.
    pub aliases: &'a [String],
    /// The portal's own id for the branch, which is what `items[].branch` carries.
    pub kobvid: &'a str,
    /// The branch's own ISIL, where it has one. A KOBV record never states it — it names
    /// the house — so this identifies the branch in the directory, not in a record.
    pub isil: Option<&'a str>,
    /// What `--at <this branch's own ISIL>` actually answers about, when that is not this
    /// branch.
    ///
    /// Absent in the ordinary case, which is every branch but one: the ISIL names its
    /// branch back and there is nothing to say. Present, it carries the `--at` key the
    /// code really resolves to — the search then runs, succeeds and answers about
    /// somewhere else, with nothing in the result that looks wrong, so an agent that
    /// validates a key has to be able to read this rather than infer it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isil_answers_elsewhere: Option<String>,
    /// Full name, as the list states it.
    pub name: &'a str,
    /// Short name, which is what a block heading and the voebb.de house facet use.
    pub short_name: &'a str,
    /// Street address of this branch, not of its house.
    pub address: Option<&'a str>,
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// The house this branch belongs to.
    pub parent: ParentView<'a>,
    /// How, and whether, `--at` reaches this branch.
    pub search: BranchSearchView,
    /// Distance in kilometres, present only for `--near` — the same member, in the same
    /// unit, that a house carries there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_km: Option<f64>,
}

/// The house a branch belongs to, named the three ways a caller might need it: to type
/// (`alias`), to match a record's `924` (`isil`) and to read (`name`).
#[derive(Debug, serde::Serialize)]
pub struct ParentView<'a> {
    /// The parent's shorthand, or `null` where it has none.
    pub alias: Option<&'a str>,
    /// The parent's ISIL — the only one a KOBV record ever carries for this branch.
    pub isil: &'a str,
    /// The parent's full name.
    pub name: &'a str,
    /// The parent's short name.
    pub short_name: &'a str,
}

/// How `--at` reaches one branch.
///
/// Not a judgement made here: it is [`crate::libraries::branch_location`] verbatim, the
/// same call `--at` itself goes through, so this view and a real search can never come to
/// disagree. `at` is always something that can be typed.
///
/// `limitation` is what separates the two kinds of answer. A VÖBB branch is filtered
/// upstream by the house facet and its result is complete; a branch of any other house is
/// filtered upstream only by its *house*, and the branch itself is read off the copies of
/// the records that were fetched. The second is a real answer with a real edge, and the
/// tag is the same one the search itself puts in `notes[]`, so an agent switches on one
/// vocabulary rather than two.
#[derive(Debug, serde::Serialize)]
pub struct BranchSearchView {
    /// What to give `--at` to search this branch.
    pub at: String,
    /// The engine that answers that `--at` value.
    pub engine: Engine,
    /// The `notes[]` kind this search will carry, or `null` when the answer is complete.
    pub limitation: Option<&'static str>,
}

impl<'a> BranchDetailView<'a> {
    /// Build the view from the branch, its house and the location `--at` would resolve it
    /// to — see [`crate::libraries::branch_location`] for who decides that.
    pub fn new(parent: &'a Library, branch: &'a Branch, at: &Location) -> Self {
        BranchDetailView {
            kind_of_entry: EntryKind::Branch,
            alias: branch.alias(),
            aliases: &branch.aliases,
            kobvid: &branch.kobvid,
            isil: branch.isil.as_deref(),
            name: &branch.name,
            short_name: &branch.short_name,
            address: branch.address.as_deref(),
            lat: branch.lat,
            lon: branch.lon,
            parent: ParentView {
                alias: parent.alias(),
                isil: &parent.isil,
                name: &parent.name,
                short_name: &parent.short_name,
            },
            search: BranchSearchView::new(at),
            isil_answers_elsewhere: crate::libraries::isil_answers_elsewhere(branch)
                .map(|entry| crate::libraries::entry_location(entry).key),
            distance_km: None,
        }
    }

    /// The same view for a branch in a listing, which has no `at` computed yet.
    ///
    /// It is computed here rather than left to the caller for the reason the detail view
    /// takes one at all: [`crate::libraries::branch_location`] is the single place that
    /// decides how a branch is searched, and a listing that answered differently from the
    /// detail view of the same branch would be the worst kind of wrong.
    pub fn of(parent: &'a Library, branch: &'a Branch) -> Self {
        let at = crate::libraries::branch_location(parent, branch);
        Self::new(parent, branch, &at)
    }
}

impl BranchSearchView {
    /// Read one branch location into the shape the JSON promises.
    ///
    /// The limitation is derived from the engine, which is where the difference actually
    /// lives: `voebb` filters the branch upstream, `kobv` can only filter its house.
    fn new(at: &Location) -> Self {
        BranchSearchView {
            at: at.key.clone(),
            engine: at.engine,
            limitation: match at.engine {
                Engine::Kobv => Some(crate::model::note_kinds::BRANCH_FROM_COPIES),
                Engine::Voebb => None,
            },
        }
    }
}

/// One element of a listing that may hold both kinds of entry.
///
/// Untagged: each variant already states its own kind in its first member, so a wrapper
/// object would say the same thing twice and force an agent to unwrap before it could
/// read anything. `type` is the discriminator, in a listing exactly as in a detail view.
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum EntryView<'a> {
    /// A whole house.
    Institution(LibraryView<'a>),
    /// One branch, with the house it belongs to and how to search it.
    Branch(BranchDetailView<'a>),
}

impl EntryView<'static> {
    /// One entry of the library list, with the distance where one was measured.
    fn of(entry: Entry, distance: Option<f64>) -> Self {
        match entry {
            Entry::Institution(library) => {
                let mut view = LibraryView::from(library);
                view.distance_km = distance;
                EntryView::Institution(view)
            }
            Entry::Branch { parent, branch } => {
                let mut view = BranchDetailView::of(parent, branch);
                view.distance_km = distance;
                EntryView::Branch(view)
            }
        }
    }
}

/// The document `blibs libraries --json` writes: the houses, as they always were.
pub fn library_views<'a>(libraries: &[&'a Library]) -> Vec<LibraryView<'a>> {
    libraries.iter().copied().map(LibraryView::from).collect()
}

/// The document a listing of entries writes — `--branches`, and `--near`, which ranks
/// both kinds.
pub fn entry_views(rows: &[(Entry, Option<f64>)]) -> Vec<EntryView<'static>> {
    rows.iter()
        .map(|(entry, distance)| EntryView::of(*entry, *distance))
        .collect()
}

/// The document `--find` writes: every entry that matched, in the order the grouped human
/// output prints them.
///
/// **Only what matched.** A house that is in the grouped output as the heading its
/// branches hang under is not one of the answers, and an array element cannot be a
/// heading; the branches carry their `parent` with them, so nothing is lost by leaving it
/// out.
pub fn found_views(groups: &[Found]) -> Vec<EntryView<'static>> {
    groups
        .iter()
        .flat_map(|group| {
            let house = group
                .matched
                .then(|| EntryView::of(Entry::Institution(group.library), None));
            house.into_iter().chain(group.branches.iter().map(|branch| {
                EntryView::of(
                    Entry::Branch {
                        parent: group.library,
                        branch,
                    },
                    None,
                )
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Error, UsageError};

    fn envelope(error: &Error) -> serde_json::Value {
        let mut out = Vec::new();
        super::error(error, &mut out).expect("a vector never fails to accept bytes");
        assert!(
            out.ends_with(b"\n"),
            "the document must end in exactly one newline"
        );
        serde_json::from_slice(&out).expect("what was written is JSON")
    }

    /// The error object of `plan/cli.md`, member for member.
    #[test]
    fn the_error_object_carries_code_kind_message_and_hint() {
        let error = Error::Usage(UsageError::UnknownLibrary {
            input: "STABI2".to_owned(),
            suggestions: vec!["STABI".to_owned()],
        });
        let document = envelope(&error);
        let body = &document["error"];
        assert_eq!(body["code"], 2);
        assert_eq!(body["kind"], "unknown_library");
        assert_eq!(body["message"], "unknown library \"STABI2\"");
        assert!(
            body["hint"]
                .as_str()
                .expect("a usage error always says what to do next")
                .contains("blibs libraries"),
            "the hint must name the way out"
        );
    }

    /// `hint` is `null`, never absent and never an empty string: an agent that reads
    /// `error.hint` must not have to tell three shapes apart.
    #[test]
    fn a_hintless_error_still_carries_the_member() {
        let error = Error::Usage(UsageError::EmptyQuery);
        let document = envelope(&error);
        let body = document["error"].as_object().expect("an object");
        assert!(body.contains_key("hint"));
        assert!(body["hint"].is_null() || body["hint"].is_string());
    }

    #[test]
    fn a_document_is_pretty_printed_and_newline_terminated() {
        let mut out = Vec::new();
        write(&vec![1, 2, 3], &mut out).expect("a vector accepts bytes");
        assert_eq!(String::from_utf8_lossy(&out), "[\n  1,\n  2,\n  3\n]\n");
    }

    /// The view publishes what the interface promises and nothing the list uses for its
    /// own matching.
    #[test]
    fn a_library_view_omits_the_matching_apparatus() {
        let library = crate::libraries::all()
            .first()
            .expect("the compiled-in list is not empty");
        let view = LibraryView::from(library);
        let json = serde_json::to_value(&view).expect("the view serialises");
        let object = json.as_object().expect("an object");
        assert_eq!(object["isil"], library.isil.as_str());
        assert!(!object.contains_key("portal_name"));
        assert!(!object.contains_key("kobvid"));
        assert!(!object.contains_key("match"));
        assert!(
            !object.contains_key("distance_km"),
            "a plain listing states no distance at all, rather than a null one"
        );
    }

    /// The kind is a member, not something to be inferred from which members exist: an
    /// agent that asked for `AGB` must be able to read that it got a branch.
    #[test]
    fn every_entry_says_which_kind_it_is() {
        let library = crate::libraries::all()
            .first()
            .expect("the compiled-in list is not empty");
        let institution =
            serde_json::to_value(LibraryView::from(library)).expect("the view serialises");
        assert_eq!(institution["type"], "institution");

        assert_eq!(branch_document("AGB")["type"], "branch");
    }

    /// One branch of the public network: its own identity, its house, and an `at` that
    /// searches the branch itself.
    #[test]
    fn a_branch_view_carries_the_branch_its_house_and_its_engine() {
        let document = branch_document("AGB");

        assert_eq!(document["alias"], "AGB");
        assert_eq!(document["kobvid"], "SIG00036");
        assert_eq!(document["address"], "Blücherplatz 1, 10961 Berlin");
        assert_eq!(document["parent"]["isil"], "DE-609");
        assert_eq!(document["parent"]["alias"], "VOEBB");
        assert!(
            document["parent"]["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
        );
        assert_eq!(document["search"]["at"], "AGB");
        assert_eq!(document["search"]["engine"], "voebb");
        assert!(
            document["search"]["limitation"].is_null(),
            "the house facet filters upstream: {document}"
        );
        assert!(
            document.get("branches").is_none(),
            "a branch has no branches of its own"
        );
    }

    /// A branch of a KOBV house is searchable too, and says where the answer stops: the
    /// tag is the same one the search itself puts in `notes[]`, so an agent reads one
    /// vocabulary rather than two.
    #[test]
    fn a_kobv_branch_names_the_limitation_of_its_search() {
        let document = branch_document("PHILBIB");

        assert_eq!(document["parent"]["alias"], "FU");
        assert_eq!(document["search"]["at"], "PHILBIB");
        assert_eq!(document["search"]["engine"], "kobv");
        assert_eq!(
            document["search"]["limitation"],
            crate::model::note_kinds::BRANCH_FROM_COPIES,
            "{document}"
        );
    }

    /// The document `libraries <branch> --json` writes, as JSON.
    fn branch_document(alias: &str) -> serde_json::Value {
        let crate::libraries::Entry::Branch { parent, branch } =
            crate::libraries::look_up(alias).expect("the alias is in the list")
        else {
            panic!("{alias} must be a branch");
        };
        let at = crate::libraries::branch_location(parent, branch);
        serde_json::to_value(BranchDetailView::new(parent, branch, &at))
            .expect("the view serialises")
    }

    /// `--near` measures both kinds of entry, and states the distance the same way for
    /// both — one member, one unit, whatever kind the element is.
    #[test]
    fn near_adds_the_distance_it_measured_to_either_kind() {
        let library = crate::libraries::all()
            .first()
            .expect("the compiled-in list is not empty");
        let crate::libraries::Entry::Branch { parent, branch } =
            crate::libraries::look_up("AGB").expect("AGB is in the list")
        else {
            panic!("AGB is a branch");
        };
        let views = entry_views(&[
            (crate::libraries::Entry::Institution(library), Some(1.25)),
            (
                crate::libraries::Entry::Branch { parent, branch },
                Some(0.5),
            ),
        ]);
        let json = serde_json::to_value(&views).expect("the views serialise");
        assert_eq!(json[0]["type"], "institution");
        assert_eq!(json[0]["distance_km"], 1.25);
        assert_eq!(json[1]["type"], "branch");
        assert_eq!(json[1]["distance_km"], 0.5);
        assert_eq!(json[1]["parent"]["isil"], "DE-609");
    }

    /// §1.11: `--find` answers for branches, and the answer says which of the two kinds
    /// each element is. A house that is only a heading in the terminal is not an element
    /// here at all — an array element cannot be a heading.
    #[test]
    fn find_publishes_the_branches_it_matched_and_not_their_headings() {
        let groups = crate::libraries::find_entries("AGB");
        let views = serde_json::to_value(found_views(&groups)).expect("the views serialise");
        let elements = views.as_array().expect("an array");
        assert!(!elements.is_empty(), "AGB must be found: {views}");
        for element in elements {
            assert!(
                element["type"] == "institution" || element["type"] == "branch",
                "every element states its kind: {element}"
            );
        }
        assert!(
            elements
                .iter()
                .any(|element| element["type"] == "branch" && element["alias"] == "AGB"),
            "{views}"
        );
        assert!(
            !elements
                .iter()
                .any(|element| element["type"] == "institution" && element["isil"] == "DE-609"),
            "the public network is the heading of its branch, not a hit: {views}"
        );
    }

    /// §1.4: the one branch ISIL that two entries claim says so in its own document, so an
    /// agent validating a key does not have to work it out from a list of claimants.
    #[test]
    fn a_shared_isil_is_stated_in_the_branch_document() {
        let agb = branch_document("AGB");
        assert_eq!(agb["isil"], "DE-109");
        assert_eq!(agb["isil_answers_elsewhere"], "ZLBORG");

        let philbib = branch_document("PHILBIB");
        assert!(
            philbib.get("isil_answers_elsewhere").is_none(),
            "an ISIL that names its branch back states nothing: {philbib}"
        );
    }

    /// A branch nested in its house's document says what it is, exactly as a standalone
    /// one does — `plan/cli.md` promises the member of *every* element.
    #[test]
    fn a_nested_branch_states_its_kind_too() {
        let library = crate::libraries::all()
            .iter()
            .find(|library| !library.branches.is_empty())
            .expect("some houses have branches");
        let json = serde_json::to_value(LibraryView::from(library)).expect("the view serialises");
        assert_eq!(json["branches"][0]["type"], "branch");
    }
}
