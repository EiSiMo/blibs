//! The library views: the listings, the search, and the detail view of one house or one
//! branch.
//!
//! Its own module because it renders a different subject than [`super::human`]: that one
//! renders *search results* — records, copies, availability — while everything here
//! renders the compiled-in list itself, which has no engine, no network and no notes. The
//! two share the terminal primitives (a field block, a wrapped line, a laid-out row) and
//! nothing else.
//!
//! Three rules hold across every view here:
//!
//! - **Every line shows a key that can be typed into `--at`.** For a house that is
//!   [`institution_location`], for a branch [`branch_location`] — the same calls `--at`
//!   itself goes through, so a listing can never recommend a key that does not resolve.
//! - **Every count is derived from the list**, never written into a sentence: houses gain
//!   and lose branches, and a number in prose is wrong the first time that happens.
//! - **A cut name says that it is cut.** The KOBV directory ends a branch's short name at
//!   60 characters, mid-word; [`branch_label`] adds the ellipsis the directory did not,
//!   and [`distinguishing_name`] is used wherever two branches stand next to each other
//!   with only one name between them.

use std::io::{self, Write};

use crate::libraries::{
    Branch, Entry, Found, Library, all, branch_count, branch_label, branch_location,
    distinguishing_name, entry_location, houses_with_branches, institution_location,
    isil_answers_elsewhere, shares_isil,
};
use crate::model::{Engine, Location};
use crate::render::Style;
use crate::render::human::{push_field, write_fields, write_line, write_row};
use crate::render::table::{Cell, Column, Layout};

/// The narrowest the shorthand column of the house table may be — the width the example
/// output is written at.
const HOUSE_KEY: usize = 8;
/// The narrowest the ISIL column may be, for the same reason.
const ISIL_COLUMN: usize = 10;
/// The narrowest the short-name column of the house table may be.
const SHORT_COLUMN: usize = 16;
/// How far a branch line is indented under the house it hangs from in `--find`.
const BRANCH_INDENT: usize = 2;
/// The narrowest the whereabouts column may become: `branch of ` and enough of a key to
/// name a house. Below that the column says a branch belongs to something and not what.
const WHEREABOUTS_MIN: usize = 15;

/// Render the house table: shorthand, ISIL, short name, full name, city.
///
/// Houses only, and it stays that way: this is the listing whose columns are pinned
/// character for character, and the 211 branches are one `--branches` away rather than
/// mixed into it.
pub fn houses(rows: &[&Library], out: &mut dyn Write, style: Style) -> io::Result<()> {
    let alias_width = column_width(HOUSE_KEY, rows.iter().map(|row| row.alias().unwrap_or("")));
    let isil_width = column_width(ISIL_COLUMN, rows.iter().map(|row| row.isil.as_str()));
    let short_width = column_width(SHORT_COLUMN, rows.iter().map(|row| row.short_name.as_str()));
    let city_width = column_width(1, rows.iter().map(|row| row.city.as_str()));
    let layout = Layout::new(vec![
        Column::Fixed(alias_width),
        Column::Fixed(isil_width),
        Column::Fixed(short_width),
        Column::Flex { ideal: 16, min: 16 },
        Column::Fixed(city_width),
        Column::Last,
    ])
    .with_gap(1);

    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|library| {
            vec![
                Cell::new(library.alias().unwrap_or("").to_owned())
                    .styled(style.bold())
                    .whole(),
                Cell::new(library.isil.clone()).styled(style.dim()).whole(),
                Cell::new(library.short_name.clone()),
                Cell::new(library.name.clone()),
                Cell::new(library.city.clone()),
            ]
        })
        .collect();
    let widths = layout.widths(&cells, style.width());
    for row in &cells {
        write_row(out, &layout, row, &widths, 0, style)?;
    }
    Ok(())
}

/// What the house table cannot say about itself: that 211 more libraries hang below it,
/// and how to see them.
///
/// Every number and the example key are **read off the list**. Three counts in this
/// tool's prose had gone stale by the time round 2 measured them, which is what a written
/// number does when the data it describes is a file that changes.
pub fn footer(out: &mut dyn Write, style: Style) -> io::Result<()> {
    let houses_with = houses_with_branches();
    let Some(largest) = houses_with
        .iter()
        .max_by_key(|library| library.branches.len())
    else {
        // No house in the list has a branch: there is nothing to point at, and an
        // invitation to look at branches that do not exist would be worse than silence.
        return Ok(());
    };
    let total = branch_count();
    let sentence = format!(
        "{} of these {} {} have branches, {} in all — `blibs libraries --branches` lists them, \
         `blibs libraries {}` the {} of one house.",
        houses_with.len(),
        all().len(),
        plural(all().len(), "house", "houses"),
        total,
        institution_location(largest).key,
        largest.branches.len(),
    );
    writeln!(out)?;
    write_line(out, &sentence, 0, style.dim(), style)
}

/// Render a flat list of entries — houses, branches or both — with the distance where one
/// was measured.
///
/// This is what `--branches` and `--near` print. A branch row names the house it belongs
/// to in the last column, because a branch name without its house is a name without an
/// address; the ISIL column carries the **house's** ISIL for a branch, which is the code a
/// record actually states for its copies, and never the branch's own — that one is already
/// in the key column wherever it is the key.
pub fn entries(rows: &[(Entry, Option<f64>)], out: &mut dyn Write, style: Style) -> io::Result<()> {
    let plan = EntryTable::of(rows, 0);
    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|(entry, distance)| plan.cells(*entry, *distance, style))
        .collect();
    let widths = plan.layout.widths(&cells, style.width());
    for row in &cells {
        write_row(out, &plan.layout, row, &widths, 0, style)?;
    }
    Ok(())
}

/// Render what `--find` matched, grouped by house.
///
/// **Grouping is what keeps a house findable.** A query that fits a dozen branches of one
/// house produces one heading and a dozen indented lines, never a dozen entries that push
/// the houses off the answer — the reason branches were once left out of the search
/// altogether, kept without the false statement that came with it.
///
/// A house that is here only because its branches matched is written as a **heading**, not
/// as a row: it is context for the lines below it and not itself one of the things that
/// were found. It still names its own key, because the house is a perfectly good thing to
/// ask about next.
pub fn found(groups: &[Found], out: &mut dyn Write, style: Style) -> io::Result<()> {
    let rows: Vec<(Entry, Option<f64>)> = groups
        .iter()
        .flat_map(|group| {
            let house = group
                .matched
                .then_some((Entry::Institution(group.library), None));
            house.into_iter().chain(group.branches.iter().map(|branch| {
                (
                    Entry::Branch {
                        parent: group.library,
                        branch,
                    },
                    None,
                )
            }))
        })
        .collect();
    // Laid out over every row of every group at once: the branches of the last house line
    // up with the first house's, which is what makes the indent read as nesting rather
    // than as a second table.
    let plan = EntryTable::of(&rows, BRANCH_INDENT);
    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|(entry, _)| plan.cells(*entry, None, style))
        .collect();
    let widths = plan.layout.widths(&cells, style.width());

    let mut next = 0usize;
    for group in groups {
        if group.matched {
            write_row(out, &plan.layout, &cells[next], &widths, 0, style)?;
            next += 1;
        } else {
            write_line(out, &heading(group.library), 0, style.dim(), style)?;
        }
        for _ in &group.branches {
            write_row(out, &plan.layout, &cells[next], &widths, 0, style)?;
            next += 1;
        }
    }
    Ok(())
}

/// A house that did not match itself, written as what it is: the heading its branches hang
/// under.
///
/// It says so in words rather than in colour — the same output goes down a pipe, where a
/// dimmed row and a hit look identical.
fn heading(library: &Library) -> String {
    format!(
        "{} — {} (no match in the house itself)",
        institution_location(library).key,
        library.name
    )
}

/// The column plan of the entry table, and which columns this particular set of rows needs.
///
/// The plan is decided once per table rather than per row: an all-branch listing has no
/// city to show and a listing with no distances has no distance column, and a column that
/// is empty in every row would still claim its `ideal` width.
struct EntryTable {
    layout: Layout,
    /// How far a branch row is indented, so that its key is still laid out in the key
    /// column and not pushed through the one beside it.
    branch_indent: usize,
    /// Whether any row states a full name the short one does not already carry.
    full_names: bool,
    /// Whether any row was measured from a point.
    distances: bool,
}

impl EntryTable {
    /// Size the columns to the rows they will carry.
    ///
    /// A column no row fills is left out of the plan rather than sized to its `ideal`: an
    /// all-branch listing usually has no full name to add and no distance to state, and an
    /// empty column 24 wide would take the room from the names that are there.
    fn of(rows: &[(Entry, Option<f64>)], branch_indent: usize) -> Self {
        let indent =
            |entry: &Entry| usize::from(matches!(entry, Entry::Branch { .. })) * branch_indent;
        let key_width = column_width(
            HOUSE_KEY,
            rows.iter().map(|(entry, _)| {
                format!(
                    "{}{}",
                    " ".repeat(indent(entry)),
                    entry_location(*entry).key
                )
            }),
        );
        let isil_width = column_width(
            ISIL_COLUMN,
            rows.iter().map(|(entry, _)| record_isil(*entry).to_owned()),
        );
        let full_names = rows.iter().any(|(entry, _)| !full_name(*entry).is_empty());
        let mut columns = vec![
            Column::Fixed(key_width),
            Column::Fixed(isil_width),
            Column::Flex { ideal: 16, min: 12 },
        ];
        if full_names {
            columns.push(Column::Flex { ideal: 24, min: 16 });
        }
        // It gives way like the names do, but not past the point where it stops
        // answering: "branch of V…" no longer says which house, and that is the whole
        // reason the column is here (round 2, §3.7).
        columns.push(Column::Flex {
            ideal: column_width(1, rows.iter().map(|(entry, _)| whereabouts(*entry))),
            min: WHEREABOUTS_MIN,
        });
        columns.push(Column::Last);
        Self {
            layout: Layout::new(columns).with_gap(1),
            branch_indent,
            full_names,
            distances: rows.iter().any(|(_, distance)| distance.is_some()),
        }
    }

    /// One row: key, the ISIL a record states, short name, full name, whereabouts.
    fn cells(&self, entry: Entry, distance: Option<f64>, style: Style) -> Vec<Cell> {
        let indent = usize::from(matches!(entry, Entry::Branch { .. })) * self.branch_indent;
        let short = match entry {
            Entry::Institution(library) => library.short_name.clone(),
            Entry::Branch { branch, .. } => branch_label(branch),
        };
        let mut row = vec![
            Cell::new(format!(
                "{}{}",
                " ".repeat(indent),
                entry_location(entry).key
            ))
            .styled(style.bold())
            .whole(),
            Cell::new(record_isil(entry).to_owned())
                .styled(style.dim())
                .whole(),
            Cell::new(short),
        ];
        if self.full_names {
            row.push(Cell::new(full_name(entry)));
        }
        row.push(Cell::new(whereabouts(entry)));
        if self.distances {
            row.push(
                Cell::new(distance.map(distance_text).unwrap_or_default()).styled(style.dim()),
            );
        }
        row
    }
}

/// The full name, where it says more than the short one already does.
///
/// A house always states it. A branch states it only where its short name is **not** how
/// the full name ends — which is where the directory cut the short name mid-word, and
/// where the two names are genuinely different (a "Teilbibliothek" under a
/// "Zweigbibliothek", say). Everywhere else the full name is the house's name with the
/// short one appended, and the house is already named in the column beside it: printing it
/// would fill a column with what two other columns have said.
fn full_name(entry: Entry) -> String {
    match entry {
        Entry::Institution(library) => library.name.clone(),
        Entry::Branch { branch, .. } => {
            if branch.name.trim_end().ends_with(branch.short_name.trim()) {
                String::new()
            } else {
                branch.name.clone()
            }
        }
    }
}

/// The ISIL a **record** carries for this entry: the house's own, and for a branch its
/// house's.
///
/// A branch's own ISIL is never here. It identifies the branch in the KOBV directory and
/// not in a record — and where it is a key at all it is already the key column, so
/// printing it twice would be the one thing this column could get wrong.
fn record_isil(entry: Entry) -> &'static str {
    match entry {
        Entry::Institution(library) => &library.isil,
        Entry::Branch { parent, .. } => &parent.isil,
    }
}

/// Where an entry is: the city for a house, the house for a branch.
///
/// A branch has an address rather than a city of its own, and its house's city applies to
/// it — so the useful thing to say in a mixed listing is which house it hangs from, which
/// is also the thing a reader needs to ask the next question.
fn whereabouts(entry: Entry) -> String {
    match entry {
        Entry::Institution(library) => library.city.clone(),
        Entry::Branch { parent, .. } => format!("branch of {}", institution_location(parent).key),
    }
}

/// A column at least `minimum` wide, and as wide as its widest value when that is wider.
///
/// Nothing in these columns may be shortened — a key is what the user types back into
/// `--at` — so the column grows instead, and the layout's gap keeps the separation.
fn column_width(minimum: usize, values: impl Iterator<Item = impl AsRef<str>>) -> usize {
    values
        .map(|value| crate::render::table::display_width(value.as_ref()))
        .max()
        .map_or(minimum, |widest| minimum.max(widest))
}

/// A distance, rounded to the precision the coordinates actually support.
fn distance_text(km: f64) -> String {
    format!("{km:.1} km")
}

/// `3 branches`, or `1 branch`.
fn plural_count(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", plural(count, one, many))
}

/// The word that goes with a count.
fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// Render one library in detail: what the list knows and nothing it does not.
///
/// Opening hours are deliberately absent — the compiled list would be days out of date.
pub fn library_detail(library: &Library, out: &mut dyn Write, style: Style) -> io::Result<()> {
    write_line(out, &library.name, 0, style.bold(), style)?;
    writeln!(out)?;
    let mut fields = Vec::new();
    push_field(&mut fields, "Shorthand", library.aliases.join(" · "));
    push_field(&mut fields, "ISIL", library.isil.clone());
    push_field(&mut fields, "Short name", library.short_name.clone());
    push_field(
        &mut fields,
        "Type",
        library.kind.clone().unwrap_or_default(),
    );
    push_field(&mut fields, "Address", library.address.clone());
    push_field(&mut fields, "City", library.city.clone());
    if let Some(coords) = library.coords() {
        push_field(
            &mut fields,
            "Coordinates",
            format!("{:.5}, {:.5}", coords.lat, coords.lon),
        );
    }
    push_field(
        &mut fields,
        "Phone",
        library.phone.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "Email",
        library.email.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "Website",
        library.url.clone().unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "OPAC",
        library.opac.clone().unwrap_or_default(),
    );
    push_branches(&mut fields, library);
    write_fields(out, &fields, style)
}

/// The branches of one house, each with the key that searches it.
///
/// The key, not only the name: this listing is the one place a branch without a shorthand
/// becomes addressable at all, and a name with nothing to type next to it is a dead end.
/// It is [`branch_location`]'s key — the same one `--at` resolves — padded to the widest,
/// because the keys come in several lengths and a ragged column reads as two columns.
///
/// The name is [`distinguishing_name`]: these are the branches of one house standing next
/// to each other, and the directory's 60-character cut falls exactly where two of them
/// differ.
fn push_branches(fields: &mut Vec<(String, String)>, library: &Library) {
    if library.branches.is_empty() {
        return;
    }
    fields.push((
        "Branches".to_owned(),
        plural_count(library.branches.len(), "branch", "branches"),
    ));
    let keys: Vec<String> = library
        .branches
        .iter()
        .map(|branch| branch_location(library, branch).key)
        .collect();
    let width = keys
        .iter()
        .map(|key| key.chars().count())
        .max()
        .unwrap_or(0);
    for (branch, key) in library.branches.iter().zip(&keys) {
        fields.push((
            String::new(),
            format!("{key:width$}  {}", distinguishing_name(branch)),
        ));
    }
}

/// Render one branch in detail: the branch itself, the house it belongs to, and how it
/// can be searched.
///
/// **Not the parent's detail view.** Someone who asks about the Amerika-Gedenkbibliothek
/// wants the house at Blücherplatz, not the branch listing of the network it belongs to;
/// the parent is named in one line and the listing stays with the question it answers.
///
/// `at` is [`branch_location`] verbatim — this function prints it and decides nothing
/// itself, so the view and `--at` cannot come to differ about which engine searches a
/// branch.
pub fn branch_detail(
    parent: &Library,
    branch: &Branch,
    at: &Location,
    out: &mut dyn Write,
    style: Style,
) -> io::Result<()> {
    write_line(out, &branch.name, 0, style.bold(), style)?;
    writeln!(out)?;
    let mut fields = Vec::new();
    push_field(&mut fields, "Shorthand", branch.aliases.join(" · "));
    push_field(&mut fields, "Branch of", parent_line(parent));
    push_isil(&mut fields, branch);
    push_field(&mut fields, "KOBV id", branch.kobvid.clone());
    push_field(&mut fields, "Short name", branch_label(branch));
    push_field(
        &mut fields,
        "Address",
        branch.address.clone().unwrap_or_default(),
    );
    if let Some(coords) = branch.coords() {
        push_field(
            &mut fields,
            "Coordinates",
            format!("{:.5}, {:.5}", coords.lat, coords.lon),
        );
    }
    push_search_field(&mut fields, at);
    write_fields(out, &fields, style)
}

/// The `ISIL` field — and, for the one code two entries claim, what typing it back really
/// answers.
///
/// The view prints this code as an identifier, and `--at` accepts it: for every branch but
/// one those two facts agree. Where they do not, the search runs, succeeds and answers
/// about somewhere else entirely, with nothing in the result that looks wrong — so this is
/// the only place it can be said. What to type instead is named here, not left to be
/// worked out.
fn push_isil(fields: &mut Vec<(String, String)>, branch: &Branch) {
    let Some(isil) = branch.isil.clone() else {
        return;
    };
    fields.push(("ISIL".to_owned(), isil.clone()));
    let Some(elsewhere) = isil_answers_elsewhere(branch) else {
        return;
    };
    let sharing = join_keys(&shares_isil(branch));
    fields.push((
        String::new(),
        format!(
            "shared with {sharing} — `--at {isil}` answers for {}; use {}",
            entry_location(elsewhere).key,
            join_keys_or(&own_keys(branch)),
        ),
    ));
}

/// Every key that names this branch and nothing else: its shorthand, where it has one, and
/// its KOBV id, which all 211 branches carry.
fn own_keys(branch: &Branch) -> Vec<String> {
    branch
        .aliases
        .iter()
        .cloned()
        .chain(std::iter::once(branch.kobvid.clone()))
        .collect()
}

/// `A, B and C` — the keys of the entries that claim one code.
fn join_keys(entries: &[Entry]) -> String {
    join(
        &entries
            .iter()
            .map(|entry| entry_location(*entry).key)
            .collect::<Vec<_>>(),
        "and",
    )
}

/// `A or B` — the keys the reader is asked to choose one of.
fn join_keys_or(keys: &[String]) -> String {
    join(keys, "or")
}

/// A readable list: `A`, `A and B`, `A, B and C`.
fn join(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., end] => format!("{} {last} {end}", rest.join(", ")),
    }
}

/// The house behind a branch: its shorthand and its full name, or just the name where the
/// list carries no shorthand for it.
fn parent_line(parent: &Library) -> String {
    match parent.alias() {
        Some(alias) => format!("{alias} — {}", parent.name),
        None => parent.name.clone(),
    }
}

/// The `Search` field: the `--at` value that searches this branch, and — where the answer
/// has an edge — what that edge is.
///
/// A `kobv` branch is not filtered upstream: its *house* is, and the branch is read off
/// the copies of the records that come back. Saying so here is the whole point of the
/// field, because it is the difference between "nothing there" and "nothing on this page".
fn push_search_field(fields: &mut Vec<(String, String)>, at: &Location) {
    push_field(
        fields,
        "Search",
        format!("--at {} ({} engine)", at.key, at.engine),
    );
    if at.engine == Engine::Kobv && at.branch.is_some() {
        fields.push((
            String::new(),
            "narrowed from the copies of its house's records, so a page \
             can be short and absence is never proven"
                .to_owned(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width the examples were written for.
    const WIDE: usize = 100;

    fn rendered(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("a vector never fails to accept bytes");
        String::from_utf8(out).expect("the renderer writes UTF-8")
    }

    fn house(isil: &str) -> &'static Library {
        all()
            .iter()
            .find(|library| library.isil == isil)
            .unwrap_or_else(|| panic!("{isil} is in the list"))
    }

    fn branch_of(alias: &str) -> (&'static Library, &'static Branch) {
        match crate::libraries::look_up(alias).expect("the alias is in the list") {
            Entry::Branch { parent, branch } => (parent, branch),
            other @ Entry::Institution(_) => panic!("{alias} must be a branch, got {other:?}"),
        }
    }

    /// The house table, character for character.
    #[test]
    fn the_library_table_is_reproduced() {
        let wanted = ["DE-1", "DE-11", "DE-B1533", "DE-609"];
        let rows: Vec<&Library> = wanted.iter().map(|isil| house(isil)).collect();
        let output = rendered(|out| houses(&rows, out, Style::plain(WIDE)));
        for line in output.lines() {
            assert_eq!(
                &line[5..9],
                "    ",
                "the shorthand column is nine wide: {line:?}"
            );
        }
        assert!(output.starts_with("STABI    DE-1       Stabi Berlin     "));
        assert!(output.contains("\nHU       DE-11      HU Berlin        "));
        assert!(output.contains("\nASH      DE-B1533   Alice Salomon HS "));
    }

    /// §3.1: the listing that shows 123 houses says that 211 more libraries hang below
    /// them — and every number in the sentence is read off the list.
    #[test]
    fn the_footer_counts_the_branches_the_listing_cannot_show() {
        let output = rendered(|out| footer(out, Style::plain(WIDE)));
        assert!(
            output.contains(&format!("{} in all", branch_count())),
            "{output:?}"
        );
        assert!(
            output.contains(&format!("{} of these", houses_with_branches().len())),
            "{output:?}"
        );
        assert!(output.contains("--branches"), "{output:?}");
    }

    /// §3.7: `--near` ranks branches too, and a branch in that list names the house it
    /// belongs to — a neighbourhood library's name alone is not an answer to "which
    /// library is round the corner".
    #[test]
    fn an_entry_listing_names_the_house_of_every_branch() {
        let (parent, branch) = branch_of("AGB");
        let rows = vec![
            (Entry::Institution(house("DE-1")), Some(1.5)),
            (Entry::Branch { parent, branch }, Some(0.25)),
        ];
        let output = rendered(|out| entries(&rows, out, Style::plain(WIDE)));
        let mut lines = output.lines();
        let stabi = lines.next().unwrap_or_default();
        let agb = lines.next().unwrap_or_default();
        assert!(stabi.starts_with("STABI "), "{stabi:?}");
        assert!(stabi.contains("1.5 km"), "{stabi:?}");
        assert!(agb.starts_with("AGB "), "{agb:?}");
        assert!(
            agb.contains("branch of VOEBB"),
            "a branch names its house: {agb:?}"
        );
        assert!(
            agb.contains("DE-609"),
            "the ISIL column is the one a record states: {agb:?}"
        );
        assert!(
            !agb.contains("DE-109"),
            "the branch's own ISIL is not a record's, and typing it answers elsewhere: {agb:?}"
        );
        assert!(agb.contains("0.2 km"), "{agb:?}");
    }

    /// A branch whose short name the directory cut at 60 characters is printed with the
    /// ellipsis the directory did not write, so a listing never claims a whole name.
    #[test]
    fn a_cut_branch_name_is_marked_as_cut() {
        let (parent, branch) = all()
            .iter()
            .flat_map(|library| library.branches.iter().map(move |branch| (library, branch)))
            .find(|(_, branch)| crate::libraries::short_name_is_cut(branch))
            .expect("the directory cuts some names at 60 characters");
        let rows = vec![(Entry::Branch { parent, branch }, None)];
        let output = rendered(|out| entries(&rows, out, Style::plain(400)));
        assert!(output.contains('…'), "{output:?}");
    }

    /// §1.11: a house carried in by its branches is a heading, and it says so in words —
    /// the same output goes down a pipe, where colour says nothing.
    #[test]
    fn a_house_that_only_carries_branches_is_written_as_a_heading() {
        let groups = crate::libraries::find_entries("Frohnau");
        let output = rendered(|out| found(&groups, out, Style::plain(WIDE)));
        assert!(
            output.contains("no match in the house itself"),
            "{output:?}"
        );
        assert!(output.contains("Frohnau"), "{output:?}");
        // The key the heading names resolves, and so does every key on a branch line.
        for line in output.lines() {
            let key = line.split_whitespace().next().unwrap_or_default();
            let key = key.trim_end_matches('—');
            assert!(
                crate::libraries::look_up(key).is_ok(),
                "{key:?} is printed as a key but does not resolve: {line:?}"
            );
        }
    }

    /// A house that matched on its own text is a row, not a heading, and its matching
    /// branches hang under it indented.
    #[test]
    fn a_matched_house_keeps_its_row_and_its_branches_hang_under_it() {
        let groups = crate::libraries::find_entries("grimm");
        let output = rendered(|out| found(&groups, out, Style::plain(WIDE)));
        assert!(output.starts_with("HU "), "{output:?}");
        assert!(
            !output.contains("no match in the house itself"),
            "the house matched itself: {output:?}"
        );
    }

    /// The detail view states what the list knows and leaves out what it does not.
    #[test]
    fn the_detail_view_leaves_out_what_is_not_stated() {
        let library = house("DE-1");
        let output = rendered(|out| library_detail(library, out, Style::plain(WIDE)));
        assert!(output.starts_with(&library.name));
        assert!(output.contains("\n  Shorthand    STABI · SBB\n"));
        assert!(output.contains("\n  ISIL         DE-1\n"));
        assert!(output.contains("\n  Coordinates  52.51755, 13.39162\n"));
        assert!(!output.contains("Opening"), "hours are never compiled in");
        assert!(
            output.contains(" branches\n"),
            "the branch count comes before the branch names"
        );
    }

    /// Every branch a house lists carries the key `--at` resolves — the listing is the one
    /// place most branches become addressable at all.
    #[test]
    fn a_house_lists_its_branches_with_keys_that_resolve() {
        let library = house("DE-11");
        let output = rendered(|out| library_detail(library, out, Style::plain(WIDE)));
        for branch in &library.branches {
            let key = branch_location(library, branch).key;
            assert!(output.contains(&key), "{key} is not offered: {output:?}");
            assert!(
                crate::libraries::look_up(&key).is_ok(),
                "{key} must resolve"
            );
        }
    }

    fn branch_output(alias: &str) -> String {
        let (parent, branch) = branch_of(alias);
        let at = branch_location(parent, branch);
        rendered(|out| branch_detail(parent, branch, &at, out, Style::plain(WIDE)))
    }

    /// A branch of the public library network: its own address, its house named in one
    /// line, and the `--at` that searches it.
    #[test]
    fn the_branch_view_states_the_branch_and_how_to_search_it() {
        let output = branch_output("AGB");
        let (parent, branch) = branch_of("AGB");

        assert!(output.starts_with(&branch.name), "{output:?}");
        assert!(output.contains("\n  Shorthand    AGB\n"), "{output:?}");
        assert!(output.contains("\n  KOBV id      SIG00036\n"), "{output:?}");
        assert!(
            output.contains("Blücherplatz 1, 10961 Berlin"),
            "the branch's own address, not the house's: {output:?}"
        );
        assert!(
            output.contains("\n  Branch of    VOEBB — "),
            "the house is named: {output:?}"
        );
        assert!(
            output.contains("\n  Search       --at AGB (voebb engine)\n"),
            "{output:?}"
        );
        for other in parent.branches.iter().filter(|b| b.kobvid != branch.kobvid) {
            assert!(
                !output.contains(&other.short_name),
                "{} must not appear: {output:?}",
                other.short_name
            );
        }
    }

    /// §1.4: the one ISIL that two entries claim is printed with what typing it back
    /// really answers, and with the keys that answer about this branch.
    #[test]
    fn a_shared_isil_names_what_it_answers_for_and_what_to_type() {
        let output = branch_output("AGB");
        assert!(output.contains("\n  ISIL         DE-109\n"), "{output:?}");
        assert!(output.contains("shared with"), "{output:?}");
        assert!(
            output.contains("`--at DE-109` answers for ZLBORG"),
            "{output:?}"
        );
        assert!(output.contains("use AGB or SIG00036"), "{output:?}");
    }

    /// The ordinary case says nothing: 136 of the 137 branch ISILs name their branch back,
    /// and a warning on every one of them would be noise that hides the one that counts.
    #[test]
    fn an_isil_that_names_its_branch_back_is_printed_without_a_word() {
        let output = branch_output("PHILBIB");
        assert!(output.contains("ISIL"), "{output:?}");
        assert!(!output.contains("shared with"), "{output:?}");
    }

    /// A branch outside the public network: the same view, the `--at` that searches it —
    /// and the edge that `--at` has there, because `kobv` can only filter its house.
    #[test]
    fn a_kobv_branch_states_the_edge_of_its_search() {
        let output = branch_output("PHILBIB");

        assert!(output.contains("\n  Branch of    FU — "), "{output:?}");
        assert!(
            output.contains("\n  Search       --at PHILBIB (kobv engine)\n"),
            "{output:?}"
        );
        assert!(output.contains("from the copies"), "{output:?}");
    }

    /// Round 2, §3.4: nothing here may run past the terminal. At the 40-column floor the
    /// sentences wrap and the rows shorten what may be shortened.
    #[test]
    fn the_views_respect_a_narrow_terminal() {
        let (parent, branch) = branch_of("AGB");
        let at = branch_location(parent, branch);
        let outputs = [
            rendered(|out| footer(out, Style::plain(40))),
            rendered(|out| library_detail(house("DE-1"), out, Style::plain(40))),
            rendered(|out| branch_detail(parent, branch, &at, out, Style::plain(40))),
        ];
        for output in outputs {
            for line in output.lines() {
                let longest = line
                    .split_whitespace()
                    .map(crate::render::table::display_width)
                    .max()
                    .unwrap_or(0);
                // A field block spends 15 columns on its indent and its label, so a
                // single word longer than what is left is the one thing no wrap can
                // shorten — an address or a URL. Everything else has to fit.
                assert!(
                    crate::render::table::display_width(line) <= 40 || longest + 15 > 40,
                    "{line:?} is wider than the terminal and could have been broken"
                );
            }
        }
    }
}
