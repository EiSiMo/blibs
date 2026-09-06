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
use crate::libraries::{Branch, Library};

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

/// One library, as `libraries --json` publishes it.
///
/// Borrowed rather than owned: the list is `'static`, and copying 123 entries to print
/// them would be work with no purpose. Member order is the document's member order.
#[derive(Debug, serde::Serialize)]
pub struct LibraryView<'a> {
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

/// One branch of a library. Only branches that are actually spoken about carry a
/// shorthand; the rest have none rather than an invented one.
#[derive(Debug, serde::Serialize)]
pub struct BranchView<'a> {
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

/// The document `blibs libraries --json` writes.
pub fn library_views<'a>(libraries: &[&'a Library]) -> Vec<LibraryView<'a>> {
    libraries.iter().copied().map(LibraryView::from).collect()
}

/// The same, with the distance `--near` measured.
pub fn library_views_near<'a>(libraries: &[(&'a Library, f64)]) -> Vec<LibraryView<'a>> {
    libraries
        .iter()
        .map(|(library, distance)| {
            let mut view = LibraryView::from(*library);
            view.distance_km = Some(*distance);
            view
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

    #[test]
    fn near_adds_the_distance_it_measured() {
        let library = crate::libraries::all()
            .first()
            .expect("the compiled-in list is not empty");
        let views = library_views_near(&[(library, 1.25)]);
        let json = serde_json::to_value(&views).expect("the views serialise");
        assert_eq!(json[0]["distance_km"], 1.25);
    }
}
