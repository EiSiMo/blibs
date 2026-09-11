//! When to colour, and with what.
//!
//! Colour is emitted only on a terminal, only when `NO_COLOR` is unset, and never when
//! `TERM=dumb`. The styles here are data — [`anstyle`] describes them without deciding
//! whether they are written.
//!
//! Colour is **decoration, never information**: every status is carried by its symbol
//! (`●◐○?`) and named in the legend, so a pipe, a screen reader and `NO_COLOR` all see
//! the same answer. That is also why widths are measured on the uncoloured text in
//! [`crate::render::table`] — the layout must be byte-identical with and without colour.

use std::io::IsTerminal;

use anstyle::{AnsiColor, Color, Effects};

use crate::model::Status;
use crate::render::table::{display_width, terminal_columns};

/// Whether this invocation writes colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    coloured: bool,
    width: usize,
}

impl Style {
    /// Decide from the environment: a TTY, no `NO_COLOR`, `TERM` not `dumb`.
    ///
    /// Also fixes the output width, so that a piped invocation is reproducible instead of
    /// depending on the terminal that happened to launch it.
    pub fn detect() -> Self {
        Self::detect_for(&std::io::stdout(), terminal_columns())
    }

    /// The same decision against a given stream and width.
    ///
    /// Exists so the rule can be exercised without a terminal: `detect` reads the
    /// process environment, which cannot be changed safely from a test.
    pub fn detect_for(stream: &impl IsTerminal, width: usize) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let term = std::env::var("TERM").ok();
        Self::new(
            allows_colour(stream.is_terminal(), no_color, term.as_deref()),
            width,
        )
    }

    /// Build a style explicitly. `plain` and the detected style are the two callers that
    /// matter; this one exists for tests and for a future `--color` flag.
    pub fn new(coloured: bool, width: usize) -> Self {
        Self { coloured, width }
    }

    /// A style that never colours, at a fixed width. For tests and for `--json`.
    pub fn plain(width: usize) -> Self {
        Self {
            coloured: false,
            width,
        }
    }

    /// Whether colour is written.
    pub fn coloured(self) -> bool {
        self.coloured
    }

    /// The output width to lay out for.
    pub fn width(self) -> usize {
        self.width
    }

    /// The style for a status symbol. Returns the default style when colour is off, so
    /// callers never branch on [`Style::coloured`].
    ///
    /// `possibly_available` and `unknown` share the dim style, as they share the `?`
    /// symbol: neither is a statement about the copy, and a traffic-light colour would
    /// be one.
    pub fn status(self, status: Status) -> anstyle::Style {
        if !self.coloured {
            return anstyle::Style::new();
        }
        match status {
            Status::Available => fg(AnsiColor::Green),
            Status::Reference => fg(AnsiColor::Yellow),
            Status::Unavailable => fg(AnsiColor::Red),
            Status::PossiblyAvailable | Status::Unknown => anstyle::Style::new() | Effects::DIMMED,
        }
    }

    /// The style for secondary text — shelfmarks, counts, the legend.
    pub fn dim(self) -> anstyle::Style {
        if self.coloured {
            anstyle::Style::new() | Effects::DIMMED
        } else {
            anstyle::Style::new()
        }
    }

    /// The style for text that carries the answer — a title, a library name.
    pub fn bold(self) -> anstyle::Style {
        if self.coloured {
            anstyle::Style::new() | Effects::BOLD
        } else {
            anstyle::Style::new()
        }
    }

    /// The style for a block heading. Deliberately its own method rather than a second
    /// name for [`Style::bold`]: headings are the one thing likely to be restyled.
    pub fn heading(self) -> anstyle::Style {
        self.bold()
    }

    /// Wrap text in a style, or return it unchanged when colour is off.
    ///
    /// The reset is emitted only when the style actually wrote something, so plain
    /// output carries no escape sequences at all — not even empty ones.
    pub fn paint(self, text: &str, style: anstyle::Style) -> String {
        if !self.coloured || style.is_plain() {
            return text.to_owned();
        }
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    /// A status symbol, painted. The symbol itself is never omitted — it, not the
    /// colour, is what says whether the book is there.
    pub fn symbol(self, status: Status) -> String {
        self.paint(&status.symbol().to_string(), self.status(status))
    }

    /// The legend under a result: only the symbols that actually occur.
    ///
    /// `voice` is what the symbols *mean* here, and saying the wrong one would be a false
    /// promise: under a location heading `●` is "you can borrow it there", in the flat
    /// list only "someone in the region lends it", and over an output of nothing but
    /// online resources `○` is "currently unavailable" rather than "on loan". The caller
    /// decides it, because only it knows what is in the list — see
    /// [`crate::render::human`].
    ///
    /// `None` when nothing occurred — an empty legend line is noise.
    ///
    /// The entries are packed into as many lines as the width needs, and **never broken
    /// between a symbol and its wording**: `●` on one line and `available` on the next
    /// would be a legend that has to be decoded rather than read. That is also why this is
    /// not laid out by [`crate::render::table`] — a generic wrap breaks at any space, and
    /// the two spaces inside an entry are exactly the ones it must not use.
    pub fn legend(self, present: &[Status], voice: Voice) -> Option<String> {
        let entries: Vec<(String, String)> = LEGEND_ORDER
            .iter()
            .filter(|status| {
                present
                    .iter()
                    .any(|shown| shown.symbol() == status.symbol())
            })
            .map(|status| {
                let words = label(*status, voice);
                (
                    format!("{}  {words}", status.symbol()),
                    format!(
                        "{}  {}",
                        self.symbol(*status),
                        self.paint(words, self.dim())
                    ),
                )
            })
            .collect();
        if entries.is_empty() {
            return None;
        }
        Some(self.pack(&entries))
    }

    /// Put the legend entries on as few lines as the width allows, measuring the
    /// **uncoloured** halves and writing the painted ones.
    fn pack(self, entries: &[(String, String)]) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0;
        for (plain, painted) in entries {
            let width = display_width(plain);
            match lines.last_mut() {
                Some(line) if used + LEGEND_GAP.len() + width <= self.width => {
                    line.push_str(LEGEND_GAP);
                    line.push_str(painted);
                    used += LEGEND_GAP.len() + width;
                }
                // The first entry, and every entry the line has no room for. An entry
                // wider than the whole terminal still gets its own line rather than being
                // cut: it is four words, and the legend is what explains the symbols.
                _ => {
                    lines.push(painted.clone());
                    used = width;
                }
            }
        }
        lines.join("\n")
    }
}

/// What separates two legend entries. Six spaces, so that the gap between two entries is
/// unmistakably wider than the two inside one.
const LEGEND_GAP: &str = "      ";

/// The statuses a legend may list, in the order it lists them. `possibly_available`
/// stands in for `unknown` as well — they share the `?` symbol and the same wording.
const LEGEND_ORDER: [Status; 4] = [
    Status::Available,
    Status::Reference,
    Status::Unavailable,
    Status::PossiblyAvailable,
];

/// Who a status wording speaks for, and about what kind of thing.
///
/// Not cosmetic in any of the three cases: the service says `red` / `not available` and
/// nothing else, and every word beyond that is this tool's reading of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// One library's **copy** of something it lends out. `○` is then "on loan": a
    /// physical copy that is not in is out, and that is what the reader wants to know.
    Copy,
    /// One library's **access** to an online resource. There is no copy on a shelf and
    /// nothing was said about a loan, so `○` may only say "currently unavailable" —
    /// "on loan" would be a fact this tool invented (round 2, §1.9).
    Access,
    /// The whole region, which is what the flat list without `--at` answers about. `●`
    /// is then "somebody lends it", never "you can borrow it there".
    Region,
}

/// The wording for a status, in the legend and on a copy line.
///
/// The label itself never carries a date — the due date is appended by the copy line
/// (`on loan, due 22 Sep 2026`), so that one wording serves the legend, where there is no
/// copy and no date, and the line, where there may be both.
pub fn label(status: Status, voice: Voice) -> &'static str {
    match (status, voice) {
        (Status::Available, Voice::Copy | Voice::Access) => "available",
        (Status::Available, Voice::Region) => "available somewhere",
        (Status::Reference, _) => "reference only",
        (Status::Unavailable, Voice::Copy) => "on loan",
        (Status::Unavailable, Voice::Access | Voice::Region) => "currently unavailable",
        (Status::PossiblyAvailable | Status::Unknown, _) => "status not confirmed",
    }
}

/// The colour rule, as a function of its three inputs.
///
/// Split out from [`Style::detect_for`] so it can be tested: changing `NO_COLOR` or
/// `TERM` in-process is `unsafe` in edition 2024 and races every other test thread.
///
/// An **empty** `NO_COLOR` still counts as set (no-color.org): the check is presence,
/// not content.
fn allows_colour(is_terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    is_terminal && !no_color && term != Some("dumb")
}

/// A foreground colour, as a style.
fn fg(colour: AnsiColor) -> anstyle::Style {
    anstyle::Style::new().fg_color(Some(Color::Ansi(colour)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::table::strip_ansi;

    #[test]
    fn colour_needs_a_terminal_an_unset_no_color_and_a_real_term() {
        assert!(allows_colour(true, false, Some("xterm-256color")));
        assert!(allows_colour(true, false, None), "TERM may be unset");
    }

    #[test]
    fn a_pipe_is_never_coloured() {
        assert!(!allows_colour(false, false, Some("xterm-256color")));
    }

    /// no-color.org: presence is the signal, so `NO_COLOR=` counts too. The caller only
    /// passes whether the variable exists, which is what makes that true here.
    #[test]
    fn no_color_switches_colour_off() {
        assert!(!allows_colour(true, true, Some("xterm-256color")));
    }

    #[test]
    fn a_dumb_terminal_is_not_coloured() {
        assert!(!allows_colour(true, false, Some("dumb")));
        assert!(
            allows_colour(true, false, Some("dumb-but-not-really")),
            "only the exact value `dumb` counts"
        );
    }

    /// `detect_for` still consults the real environment for `NO_COLOR`/`TERM`, so the
    /// only thing that can be asserted unconditionally is the stream half of the rule.
    /// `IsTerminal` is sealed, so the not-a-terminal stream has to be a real file.
    #[test]
    fn detecting_on_a_redirected_stream_yields_a_plain_style_of_the_given_width() {
        let file = tempfile::tempfile().expect("a temporary file can be created");
        let style = Style::detect_for(&file, 72);
        assert!(!style.coloured());
        assert_eq!(style.width(), 72);
    }

    #[test]
    fn a_plain_style_writes_no_escape_sequences() {
        let style = Style::plain(80);
        assert!(!style.coloured());
        for status in [
            Status::Available,
            Status::Reference,
            Status::Unavailable,
            Status::PossiblyAvailable,
            Status::Unknown,
        ] {
            assert_eq!(style.symbol(status), status.symbol().to_string());
            assert!(style.status(status).is_plain());
        }
        assert!(style.dim().is_plain());
        assert!(style.bold().is_plain());
        assert!(style.heading().is_plain());
        assert_eq!(style.paint("Der Prozess", style.bold()), "Der Prozess");
    }

    #[test]
    fn a_coloured_style_writes_them() {
        let style = Style::new(true, 80);
        let green = style.symbol(Status::Available);
        assert!(green.starts_with('\u{1b}'), "{green:?} should be coloured");
        assert!(green.ends_with("\u{1b}[0m"));
        assert!(green.contains('●'));
        assert_ne!(
            style.status(Status::Available),
            style.status(Status::Reference)
        );
        assert_ne!(
            style.status(Status::Unavailable),
            style.status(Status::Unknown)
        );
    }

    /// The symbol is the information; it is present in both modes and identical.
    #[test]
    fn the_symbol_survives_both_modes() {
        let plain = Style::plain(80);
        let coloured = Style::new(true, 80);
        for status in [
            Status::Available,
            Status::Reference,
            Status::Unavailable,
            Status::PossiblyAvailable,
            Status::Unknown,
        ] {
            let painted = coloured.symbol(status);
            assert!(painted.contains(status.symbol()));
            assert_eq!(plain.symbol(status), status.symbol().to_string());
        }
    }

    #[test]
    fn the_legend_lists_only_the_symbols_that_occur() {
        let style = Style::plain(80);
        let legend = style
            .legend(&[Status::Available, Status::Reference], Voice::Copy)
            .expect("two statuses occur");
        assert_eq!(legend, "●  available      ◐  reference only");
        assert!(!legend.contains('○'));
    }

    /// The legend example, both forms.
    #[test]
    fn the_legend_says_what_the_symbols_mean_where_they_are_shown() {
        let style = Style::plain(80);
        let shown = [Status::Available, Status::Reference, Status::Unavailable];
        assert_eq!(
            style
                .legend(&shown, Voice::Copy)
                .expect("three statuses occur"),
            "●  available      ◐  reference only      ○  on loan"
        );
        assert_eq!(
            style
                .legend(&shown, Voice::Region)
                .expect("three statuses occur"),
            "●  available somewhere      ◐  reference only      ○  currently unavailable"
        );
    }

    /// `?` is one entry however it arose: `possibly_available` and `unknown` are the same
    /// symbol and the same sentence.
    #[test]
    fn the_two_question_mark_statuses_share_one_entry() {
        let style = Style::plain(80);
        let legend = style
            .legend(&[Status::Unknown, Status::PossiblyAvailable], Voice::Copy)
            .expect("the question mark occurs");
        assert_eq!(legend, "?  status not confirmed");
    }

    #[test]
    fn the_legend_keeps_its_order_whatever_order_the_statuses_arrive_in() {
        let style = Style::plain(80);
        let legend = style
            .legend(&[Status::Unavailable, Status::Available], Voice::Copy)
            .expect("two statuses occur");
        assert_eq!(legend, "●  available      ○  on loan");
    }

    /// A legend too wide for the terminal is broken **between** entries, never inside
    /// one: a `●` alone on a line explains nothing (round 2, §3.4).
    #[test]
    fn a_legend_wider_than_the_terminal_breaks_between_entries() {
        let style = Style::plain(40);
        let legend = style
            .legend(
                &[Status::Available, Status::Reference, Status::Unavailable],
                Voice::Region,
            )
            .expect("three statuses occur");
        assert_eq!(
            legend,
            "●  available somewhere\n◐  reference only\n○  currently unavailable"
        );
        // Two entries do share a line where the width allows it.
        assert_eq!(
            Style::plain(60)
                .legend(&[Status::Reference, Status::Unavailable], Voice::Region)
                .expect("two statuses occur"),
            "◐  reference only      ○  currently unavailable"
        );
        for line in legend.lines() {
            assert!(!line.trim().is_empty());
            assert!(line.starts_with('●') || line.starts_with('◐') || line.starts_with('○'));
        }
    }

    /// An entry wider than the whole terminal keeps its line rather than being cut: the
    /// legend is what explains the symbols, and half of it explains nothing.
    #[test]
    fn a_legend_entry_is_never_cut() {
        let style = Style::plain(crate::render::table::MIN_WIDTH);
        let legend = style
            .legend(&[Status::PossiblyAvailable], Voice::Copy)
            .expect("one status occurs");
        assert_eq!(legend, "?  status not confirmed");
    }

    #[test]
    fn an_empty_legend_is_none_not_a_blank_line() {
        assert_eq!(Style::plain(80).legend(&[], Voice::Copy), None);
    }

    /// Colour changes what is written, never what is said: strip the escapes and the two
    /// legends are the same string.
    #[test]
    fn colour_does_not_change_the_legend_text() {
        let shown = [Status::Available, Status::Reference, Status::Unknown];
        let plain = Style::plain(80)
            .legend(&shown, Voice::Region)
            .expect("statuses occur");
        let coloured = Style::new(true, 80)
            .legend(&shown, Voice::Region)
            .expect("statuses occur");
        assert_ne!(plain, coloured);
        assert_eq!(plain, strip_ansi(&coloured));
    }
}
