//! What was asked, where it was asked, and what came back.
//!
//! [`SearchResult`] and [`ShowResult`] are the two JSON documents. Their member order is
//! the documents' member order and part of the JSON contract.

use crate::model::{Engine, FetchWindow, Format, Isil, Page, Record, RecordId, Status};

/// One search term.
///
/// The distinction comes from the shell and nowhere else: an argument that contains a
/// space was quoted by the user and is searched as a phrase; everything else is a word.
/// User input is never quoted on the user's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// A single word.
    Word(String),
    /// A quoted phrase.
    Phrase(String),
}

impl Term {
    /// Classify one command-line argument.
    ///
    /// The shell has already removed the quotes, so the *only* surviving evidence that
    /// the user quoted something is that the argument contains whitespace. Terms are
    /// never quoted on the user's behalf: an unquoted CQL/PQF term is an implicit AND and
    /// a quoted one is a phrase, and the two find wildly different numbers of records.
    pub fn from_argument(argument: &str) -> Self {
        let trimmed = argument.trim();
        if trimmed.chars().any(char::is_whitespace) {
            Term::Phrase(trimmed.to_owned())
        } else {
            Term::Word(trimmed.to_owned())
        }
    }

    /// The term's text, without the quoting decision.
    pub fn text(&self) -> &str {
        match self {
            Term::Word(text) | Term::Phrase(text) => text,
        }
    }

    /// Whether the term carries no text at all. An argument of only whitespace produces
    /// one of these, and `cli` refuses it outright — an empty value is nearly always a
    /// shell variable that did not expand, and a search without it answers a different
    /// question.
    pub fn is_empty(&self) -> bool {
        self.text().is_empty()
    }

    /// The term as it must be typed back in — a phrase keeps the quotes that made it one,
    /// so that `query.terms` in the JSON can be pasted straight back onto a command line.
    fn echo(&self) -> String {
        match self {
            Term::Word(text) => text.clone(),
            Term::Phrase(text) => format!("\"{text}\""),
        }
    }
}

/// A standard number the user searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// A validated ISBN, normalised to digits. Validated **before** sending: the
    /// identifier index discards the check digit, so a typo returns the wrong book
    /// instead of nothing.
    Isbn(String),
    /// An ISSN, passed through — there the check digit is significant upstream.
    Issn(String),
}

/// The query, decomposed. Assembled once in `cli` and never mutated afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuerySpec {
    /// Free terms, `@and`-ed together.
    pub terms: Vec<Term>,
    /// `--title`.
    pub title: Option<Term>,
    /// `--subject`.
    pub subject: Option<Term>,
    /// `--publisher`.
    pub publisher: Option<Term>,
    /// `--author`, always kept as a raw string because it is **always** searched as a
    /// word list, never as a phrase: the index holds authority forms, so a phrase in the
    /// natural name order finds two orders of magnitude fewer records than the inverted
    /// one.
    pub author: Option<String>,
    /// `--year`. Means "was running in this year" for serials, "published in" for
    /// monographs.
    pub year: Option<u16>,
    /// `--isbn`/`--issn`.
    pub identifier: Option<Identifier>,
}

impl QuerySpec {
    /// Whether this query would send nothing at all.
    ///
    /// Checked in `cli` before the first byte goes out: an empty query is diagnostic
    /// 1/10 upstream, which would surface as exit 5 for what is plainly a usage error.
    /// This is the "you gave me nothing at all" case — a value that was *given* but is
    /// empty is refused earlier, by name, in `cli::validate`.
    pub fn is_empty(&self) -> bool {
        self.terms.iter().all(Term::is_empty)
            && self.title.is_none()
            && self.subject.is_none()
            && self.publisher.is_none()
            && self.author.is_none()
            && self.year.is_none()
            && self.identifier.is_none()
    }

    /// The free terms as the user typed them, for `query.terms` in the JSON.
    ///
    /// Only the free terms: the flagged fields are reproduced by their own flags, and
    /// folding them in here would produce a string that does not reproduce the search.
    /// Phrases keep their quotes, so the echo can be pasted back onto a command line and
    /// yield the same query. Empty when the query is made of flags alone.
    pub fn echo(&self) -> String {
        self.terms
            .iter()
            .filter(|term| !term.is_empty())
            .map(Term::echo)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One resolved entry of `--at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The **canonical** key: the library's own alias, or the bare ISIL or KOBV id where
    /// it has none. This is what everything downstream matches on — `at[]` blocks against
    /// locations, a block heading against its total — so it has to be one spelling per
    /// library rather than whichever of its names was typed.
    ///
    /// It is therefore *not* what the user wrote: `--at ZLB` resolves to `VOEBB` and
    /// `--at HU/Germanistik` to `DE-11-105`. The typed word is [`Self::given`].
    pub key: String,
    /// The word the user actually put in `--at`, verbatim apart from surrounding
    /// whitespace.
    ///
    /// Carried purely so that the answer can be found again by what was asked: an agent
    /// that sends `--at ZLB` and then looks for `at[] | select(.key == "ZLB")` finds
    /// nothing, because `key` is canonical. It is never matched on and never sent
    /// upstream — the ISIL attribute is case-sensitive, so the user's spelling must not
    /// reach the catalogue.
    pub given: String,
    /// The canonical ISIL. Never the user's spelling — the upstream filter is
    /// case-sensitive.
    pub isil: Isil,
    /// The branch, when the location names one.
    pub branch: Option<BranchRef>,
    /// Which engine answers for this location.
    pub engine: Engine,
    /// Full display name for the block heading.
    pub display: String,
}

/// A branch of a library network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// The branch's KOBV id, e.g. `SIG00036`.
    pub kobvid: String,
    /// The branch's name.
    pub name: String,
}

/// Everything one engine needs to run one search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The query.
    pub query: QuerySpec,
    /// The locations this engine is responsible for. Empty means "no `--at`".
    pub locations: Vec<Location>,
    /// The record window to fetch, **per location**: `--limit` applies to every block,
    /// so a location's search asks for a full window of its own.
    pub window: FetchWindow,
}

/// What one engine returned, before selection and before availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSearch {
    /// Which engine produced this.
    pub engine: Engine,
    /// Total hits upstream, when **one** search states one. `None` as soon as an engine
    /// ran several searches — one per location — because no single number is then true
    /// of the whole answer, and adding them up would double-count every record two
    /// locations both hold. The per-location counts live in [`Self::at`].
    pub total: Option<u64>,
    /// How many records were delivered.
    pub fetched: usize,
    /// How many records the envelope announced but did not deliver.
    pub undelivered: usize,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// The records.
    pub records: Vec<Record>,
    /// The query as it was sent, for the JSON echo. `None` when the engine has no query
    /// language (`voebb`) or sent more than one query — echoing one of several would name
    /// a search that produced only part of the answer.
    pub query_echo: Option<String>,
    /// Limitations that are not errors.
    pub notes: Vec<Note>,
}

/// One entry of `at[]`: a location, the true number of hits at it, and which of the
/// displayed records belong under it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AtBlock {
    /// The library's **canonical** key — its alias, or its bare ISIL or KOBV id where it
    /// has none. Not necessarily the word that stood in `--at`: that one is
    /// [`Self::given`], and looking a block up by it is what an agent will try first.
    pub key: String,
    /// The word the user put in `--at` for this block, verbatim.
    ///
    /// [`Location::given`] carried through, so that a block can be found again by the
    /// name it was asked for. Equal to `key` whenever the user typed the canonical name.
    pub given: String,
    /// The canonical ISIL.
    pub isil: Isil,
    /// The branch id, when the location is a branch.
    pub branch: Option<String>,
    /// The engine that answered for it.
    pub engine: Engine,
    /// Hits at this location. `None` when the engine cannot state one.
    pub total: Option<u64>,
    /// The ids of the displayed records this location holds, in the order of `records[]`.
    ///
    /// This is what makes the human grouping *derivable* rather than a second schema: the
    /// block under a heading is exactly these records. It exists because a holding cannot
    /// always answer the question — every VÖBB holding carries `DE-609`, the network's
    /// ISIL, so two branches in `--at` would otherwise show each other's hits. The engine
    /// states which records its search for this location returned; `cli` cuts the list
    /// down to the records the page actually shows.
    #[serde(serialize_with = "record_ids")]
    pub records: Vec<RecordId>,
    /// Why this location was **not searched**, when it was not. `None` means it answered.
    ///
    /// The field exists because `total: null` plus `records: []` is otherwise three
    /// things at once — a branch nobody can count, a location no engine reported on, and
    /// a location whose window this page could never reach. Only the last of them is a
    /// statement about *this invocation* rather than about the catalogue, and it is the
    /// only one a caller can act on by changing a flag, so it is the one that gets a
    /// machine-readable answer instead of prose in a note.
    ///
    /// Always serialised, `null` included: a field that vanishes when nothing was refused
    /// would put an agent back to guessing whether its absence means "answered" or "this
    /// version does not say".
    pub refused: Option<LocationRefusal>,
}

/// Why a location in `--at` was not searched at all.
///
/// Deliberately **not** a `bool`. Two reasons already exist the day the field is
/// introduced, they carry different advice, and one of them would be a lie if it were
/// printed for the other: KOBV's is the catalogue answering "this result set ends before
/// your window", voebb's is blibs refusing to walk that far before a byte goes out. A
/// `past_the_last_result: true` on a VÖBB branch would claim the branch has fewer hits
/// than the page asked for, which nobody ever looked up. Renaming a published member is a
/// contract break, so the shape that can hold the second reason is the shape to publish
/// first.
///
/// Both leave the same trace otherwise: an empty block, no total, and a note of the
/// matching `kind` naming the locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationRefusal {
    /// The catalogue refused the window because **this location's** result set ends
    /// before it begins — SRU diagnostic `1/61`, raised for one of several searches.
    /// Says how far that result set reaches and nothing about what is held.
    PastTheLastResult,
    /// The window lies deeper than the answering catalogue can be paged at all, so
    /// nothing was asked. voebb.de stops at [`crate::engine::voebb::MAX_POSITION`], and
    /// the refusal happens in `cli::validate`, before the session is opened.
    WindowTooDeep,
}

/// Serialise record ids as the plain strings a user types back in.
///
/// [`RecordId`] serialises as the four flat members `id`/`engine`/`source`/`local_id`,
/// which is what [`Record`] flattens — inside a list of ids that shape would be four
/// objects' worth of noise for one string that is already unique.
fn record_ids<S: serde::Serializer>(ids: &[RecordId], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(ids.iter().map(RecordId::as_str))
}

/// Whether availability was fetched.
///
/// This is what makes `items: []` unambiguous. Without it, "we did not ask" and "we asked
/// and got nothing" look identical — exactly the kind of ambiguity an agent cannot
/// resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityMode {
    /// Availability was requested for every displayed record.
    Fetched,
    /// `--no-availability`: nothing was asked.
    Skipped,
}

/// How large the fetched window was and what survived the client-side filters.
///
/// The numbers are printed. Without them, "no hits" and "no hits *in the first 50*" are
/// indistinguishable, and a page thinned by `--available` is indistinguishable from a
/// short one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    /// How many records came back.
    pub fetched: usize,
    /// How many were left after `--format`/`--language`.
    pub after_filter: usize,
    /// Whether `--format` or `--language` was set at all.
    ///
    /// Always present, never skipped: `after_filter == fetched` is true both when no
    /// filter ran and when one ran and matched everything, and an agent that cannot tell
    /// those apart cannot tell whether this window is the whole addressable set. When it
    /// is true the window is one anchored block of 50 raw records and `--page` walks the
    /// matches inside it, so `total` is out of reach past that block.
    pub filtered: bool,
    /// How many the envelope announced but did not deliver.
    #[serde(skip_serializing_if = "is_zero")]
    pub undelivered: usize,
    /// How many of the displayed records `--available` judged, and `None` when the flag
    /// did not run — which is how a reader tells a page that was thinned from one that
    /// was always this short.
    ///
    /// How many *survived* is [`SearchResult::shown`] and is deliberately not repeated
    /// here: two names for one count drift apart. The hidden records are the difference
    /// between the two.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_available: Option<usize>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature is dictated by serde's skip_serializing_if"
)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// How the displayed records were ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SortSpec {
    /// The key.
    pub by: SortKey,
    /// What the sort could see. Always [`SortScope::Fetched`] — SRU cannot sort at all,
    /// so every sort in this tool is client-side over the fetched window. The output
    /// must never imply otherwise.
    pub scope: SortScope,
}

/// What to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Upstream order, which is relevance.
    #[default]
    Relevance,
    /// Publication year, newest first.
    Year,
    /// Displayed title, article included.
    Title,
    /// First author.
    Author,
    /// Availability, best first. Costs no extra request: the data is already there for
    /// every displayed record.
    Availability,
}

impl SortKey {
    /// The word `--sort` is written with, which is also the word the JSON carries.
    ///
    /// One spelling for both directions: `cli::validate` parses these words, `serde`
    /// lowercases the variant into the same ones, and a message that names the key —
    /// [`crate::error::EmptyReason::PastTheLastSorted`] — reads it from here rather than
    /// spelling the vocabulary a third time.
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Relevance => "relevance",
            SortKey::Year => "year",
            SortKey::Title => "title",
            SortKey::Author => "author",
            SortKey::Availability => "availability",
        }
    }
}

/// The scope a sort had access to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortScope {
    /// Only the records that were fetched.
    #[default]
    Fetched,
}

/// A limitation that is not an error and must not be silently swallowed: a per-record
/// diagnostic, `hasAvailability: false`, an item group that carries no ISIL, a branch
/// facet that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Note {
    /// Stable machine-readable tag; one of the constants in [`note_kinds`].
    pub kind: &'static str,
    /// Human-readable explanation.
    pub message: String,
    /// The records this note is about, when it is about particular ones.
    ///
    /// Empty — and then absent from the JSON — for a note about the whole answer: a
    /// window that was truncated, a query that could not be sent in full, a location the
    /// other catalogue answers for. A note that *is* about records must name them:
    /// three `voebb_online_only` notes over two blocks are unreadable otherwise, and
    /// `message` is prose an agent is forbidden to parse.
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "record_ids")]
    pub records: Vec<RecordId>,
}

impl Note {
    /// Build a note about the answer as a whole. `kind` is meant to be one of the
    /// constants in [`note_kinds`] — an agent switches on it, so a tag invented at a call
    /// site is a silent break.
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            records: Vec::new(),
        }
    }

    /// Build a note about particular records.
    ///
    /// The same note as [`Note::new`] plus the ids it applies to, so that a reader does
    /// not have to guess which of the displayed records a limitation belongs to. Passing
    /// an empty `records` is allowed and produces exactly [`Note::new`] — a caller that
    /// found nothing to name says so by naming nothing, rather than by inventing an id.
    pub fn about(
        kind: &'static str,
        message: impl Into<String>,
        records: impl IntoIterator<Item = RecordId>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            records: records.into_iter().collect(),
        }
    }

    /// Fold notes that say **the same thing** into one, uniting their records.
    ///
    /// One note per record is how the engines produce them — the note is about a record,
    /// so it names one — and four identical paragraphs under a ten-line result is how
    /// that reads at a terminal: the reader cannot tell which four of the ten lines are
    /// meant, although `records[]` has said so all along. Folding turns those four into
    /// one note with four records, which is the same information in the shape both
    /// audiences can use.
    ///
    /// **`kind` *and* `message` have to match.** Two notes of one kind whose messages
    /// differ say different things — another platform's name, another quoted wording,
    /// another selector — and merging them would keep one sentence and silently drop the
    /// other. That is also why the messages this crate writes for a per-record note are
    /// worded for one record and for many alike: nothing in them enumerates the records,
    /// which is the renderer's line beneath the message.
    ///
    /// Order is first appearance, for the notes and for the records inside them, so a
    /// document does not reshuffle when one record more or less matched. A record named
    /// twice by two folded notes appears once.
    ///
    /// It lives here, on [`Note`], and not in either engine: `notes[]` from `kobv` and
    /// from `voebb` arrive in the same list, a fold in one client would leave the other
    /// engine repeating itself, and the two documents that carry notes —
    /// [`SearchResult`] and [`ShowResult`] — must not grow two ideas of what a duplicate
    /// note is.
    pub fn merged(notes: Vec<Note>) -> Vec<Note> {
        let mut merged: Vec<Note> = Vec::with_capacity(notes.len());
        for note in notes {
            match merged
                .iter_mut()
                .find(|kept| kept.kind == note.kind && kept.message == note.message)
            {
                Some(kept) => {
                    for id in note.records {
                        if !kept.records.contains(&id) {
                            kept.records.push(id);
                        }
                    }
                }
                None => merged.push(note),
            }
        }
        merged
    }
}

/// Every value [`Note::kind`] can take.
///
/// The tags are the agent-facing half of a note: the message explains, the tag is what
/// can be branched on, so it is part of the JSON contract exactly like a field name.
/// They live here rather than next to the code that emits them because that code is
/// spread over two parse modules and two engines, and a vocabulary scattered over four
/// files is a vocabulary that grows two spellings of the same thing.
pub mod note_kinds {
    /// The catalogue announced a record and delivered a diagnostic in its place.
    pub const RECORD_UNDELIVERED: &str = "record_undelivered";

    /// A record arrived under a `recordSchema` this tool cannot read.
    pub const RECORD_SCHEMA_UNKNOWN: &str = "record_schema_unknown";

    /// A client-side window filter ran and matched **none** of the fetched records.
    ///
    /// The distinction it keeps alive is the one this tool exists to keep: "the
    /// catalogue holds nothing" versus "nothing in the 50 records that were fetched
    /// matched `--format map`". `window.after_filter == 0` next to a non-zero
    /// `window.fetched` says the same thing, but only to a reader who already knows to
    /// look — and the human output has been saying it in prose since before there was a
    /// tag to switch on.
    pub const WINDOW_FILTER_EMPTY: &str = "window_filter_empty";

    /// The catalogue delivered the same record more than once inside one window, and the
    /// repeats were dropped. Measured against `sru.kobv.de/k2` directly, so it is the
    /// union catalogue's doing rather than this tool's — but `records[]` promises each
    /// record exactly once, and that promise is kept here.
    pub const DUPLICATE_RECORDS_DROPPED: &str = "duplicate_records_dropped";

    /// The identifier index answered a `--isbn` search with records carrying a different
    /// ISBN, and they were dropped.
    ///
    /// Not a filter the user asked for and not a limitation of the window: it is the
    /// catalogue's own index being coarser than the question: measured 2026-09-11,
    /// `@attr 1=7 "9783596294336"`, `…330`, `…331` and the ISBN-10 form `3596294336` all
    /// answer with the same 18 records. Every number that agrees in the leading twelve
    /// digits is therefore a hit, and a page of somebody else's editions is the worst
    /// kind of wrong answer — it looks exactly like a right one. The tag exists so that
    /// an agent can tell the narrowing from the search, and so that a page shorter than
    /// the count beside it has a stated reason.
    pub const ISBN_NEIGHBOURS_DROPPED: &str = "isbn_neighbours_dropped";

    /// The KOBV catalogue does not order a result stably: two identical requests return
    /// different records in a different order, so consecutive pages can overlap and can
    /// leave records out. Set from the second page on, because page one alone cannot
    /// show the effect.
    pub const RESULT_ORDER_UNSTABLE: &str = "result_order_unstable";

    /// The availability service holds no information for this record
    /// (`hasAvailability: false`); the copies come from the catalogue alone.
    pub const AVAILABILITY_NOT_STATED: &str = "availability_not_stated";

    /// A traffic-light colour was missing or is not one of the four known ones, so a
    /// status is reported as unknown rather than guessed.
    pub const AVAILABILITY_UNKNOWN_STATUS: &str = "availability_unknown_status";

    /// The positional match between traffic lights and blocks of copies did not apply,
    /// and the blocks were matched by library name instead.
    pub const AVAILABILITY_MATCHED_BY_NAME: &str = "availability_matched_by_name";

    /// Order and name disagree about which library a block of copies belongs to; the
    /// order won, and the disagreement is stated rather than hidden.
    pub const AVAILABILITY_MATCH_CONFLICT: &str = "availability_match_conflict";

    /// The two signals a copy's loan status is read from — the marker class and the
    /// status word — contradict each other. The pessimistic one wins, because promising
    /// a loan that does not exist is the worse of the two mistakes.
    pub const AVAILABILITY_STATUS_CONFLICT: &str = "availability_status_conflict";

    /// `--available` was asked which copies are in and met records that say nothing at
    /// all. Three sources produce one: a record with no `924` holdings (4.9 % of the
    /// catalogue), a portal answer of `hasAvailability: false`, and a voebb branch whose
    /// copies could not be resolved — an Onleihe or multi-part record without copies of
    /// its own among them. They are hidden like an on-loan record, and the note is what
    /// keeps "nothing was said" from reading as "it is out".
    pub const AVAILABILITY_FILTER_UNSTATED: &str = "availability_filter_unstated";

    /// A block of copies could not be matched to any ISIL and is shown under the
    /// portal's own name for the library.
    pub const HOLDING_WITHOUT_ISIL: &str = "holding_without_isil";

    /// voebb.de listed no copies for a record because the copies belong to the volumes
    /// of a multi-part work, which are records of their own. Not "held nowhere".
    ///
    /// Set from what the page says about **itself** — `Medienart` = `[Mehrteiliges
    /// Werk]` — and never from the shape of its item table. An empty table is not the
    /// same statement: of the nine records in the 2026-09-08 sample whose table is
    /// present and empty, only two are multi-part works, and the other seven are a
    /// newspaper, a journal-like series, a magazine issue, a score at work level and
    /// three film and audiobook series. "Search for the volume" is advice a newspaper
    /// does not deserve; that case
    /// is [`VOEBB_NO_COPIES_LISTED`].
    pub const VOEBB_MULTIVOLUME: &str = "voebb_multivolume";

    /// voebb.de listed no copies for a record and said nothing about why.
    ///
    /// The sibling of [`VOEBB_MULTIVOLUME`] and the larger half of it: the item table is
    /// **present and empty**, and the record is not a multi-part work. Measured
    /// 2026-09-08 on `[Zeitung]`, `[Zeitschriftenartige Reihe]`, `[Zeitschriftenheft]`,
    /// `[Noten]`, `[DVD]` and `[CD]` records. Two tags rather than one because the
    /// difference is a sentence a reader acts on: a multi-part work has volumes to search
    /// for, and this has nothing this tool can name.
    ///
    /// What such a record *does* state about its holdings, where it states anything, is
    /// prose in [`crate::model::Holding::holdings_statement`] — a field rather than a
    /// third tag, because it is data and not a limitation, and because a tag could only
    /// say that it exists.
    pub const VOEBB_NO_COPIES_LISTED: &str = "voebb_no_copies_listed";

    /// voebb.de stated a return date for a copy in a form this tool cannot read, so the
    /// copy carries none rather than a guessed one.
    ///
    /// The message names the form that was **expected** and never the text that was not
    /// it. A date differs per copy, not even per record, so quoting one would turn a
    /// single changed format into one paragraph per borrowed copy —
    /// [`crate::model::Note::merged`] folds on `kind` *and* `message`, and `records[]` is what says
    /// where the new form can be read.
    ///
    /// Never set for a copy that simply has no date — that is the normal case and says
    /// nothing. Only a `Fällig am:` whose value is not a date reaches here, which means
    /// the site changed how it writes them.
    pub const VOEBB_DUE_DATE_UNREADABLE: &str = "voebb_due_date_unreadable";

    /// The record is a lending-platform title: it has no copies on a shelf, and its loan
    /// state is stated only as prose in the link to the platform — where this tool
    /// **read** it, so the holding's status is the platform's own statement.
    ///
    /// The sibling of [`VOEBB_ONLINE_STATE_UNSTATED`], and the two are two tags rather
    /// than one because the difference decides a sentence: a record whose state was read
    /// is judged like any other, while one whose link says nothing is genuinely
    /// unjudged. `message` is prose an agent may not parse, and `records[]` says *which*
    /// record — neither can carry the distinction, so `kind` has to.
    pub const VOEBB_ONLINE_ONLY: &str = "voebb_online_only";

    /// The same record, with nothing to read: an electronic title whose lending link
    /// states **no** loan status this tool knows.
    ///
    /// Two sources, both measured: Overdrive prints no parenthesis at all, and a wording
    /// nobody has seen must never become a guess. The note carries the link's raw text so
    /// that a new wording shows up
    /// instead of being swallowed, and the holding stays [`crate::model::Status::Unknown`].
    pub const VOEBB_ONLINE_STATE_UNSTATED: &str = "voebb_online_state_unstated";

    /// The third shape of an electronic title, and a completely regular one: no copies,
    /// **no lending link either**, and the access stated as a plain `URL` row.
    ///
    /// Measured 2026-09-08 on `voebb_SAK34364366` (`detail_online_url.html`): `Medienart`
    /// `[E-Ressource]`, no `table#resptable-1`, no `Link zu …` row, and one `URL` row
    /// pointing at a URN resolver. The parser used to know only the two lending-link
    /// shapes and called this page broken, which took a whole search down with it.
    ///
    /// Its own tag rather than [`VOEBB_ONLINE_ONLY`]'s, because the two answer different
    /// questions: a lending link states a loan state and this states none at all. The
    /// holding is [`crate::model::Status::Unknown`] — whether the resolver's target is free to read is
    /// something voebb.de does not say, and a guess here would be a promise the tool
    /// cannot keep.
    ///
    /// The message names **no** URL. It used to, and that made it unique per record, so
    /// [`crate::model::Note::merged`] never folded two of them: three such records in one window printed
    /// three near-identical paragraphs differing only in a link (measured on
    /// `--author "von Schirach" --at AGB`). The link belongs to
    /// [`crate::model::Record::urls`], which `show` prints and `--json` carries.
    pub const VOEBB_ONLINE_URL_ONLY: &str = "voebb_online_url_only";

    /// A voebb.de record page arrived and is not a page this parser recognises, so that
    /// **one record** lost its copies while the rest of the answer stood.
    ///
    /// The granularity is the point. A single unknown page used to abort the whole
    /// invocation with `selector "table#resptable-1" matched nothing`, so nine sound hits
    /// were thrown away for the tenth (measured 2026-09-08, `--author "von Schirach"
    /// --at AGB`). Now the record keeps its place with [`crate::model::Status::Unknown`], and this note
    /// carries the selector that stopped matching plus where to report it — the two
    /// things the old error message was right to say.
    ///
    /// It is **never** set for a network failure, a 429/503 or a lost voebb.de session:
    /// [`crate::error::Error::is_unreadable_document`] draws that line, and those still
    /// stop the invocation rather than come back as "status unknown".
    pub const VOEBB_PAGE_UNREADABLE: &str = "voebb_page_unreadable";

    /// voebb.de's advanced search has no free-text index, so free terms next to a field
    /// flag were searched as a title — the closest index the form offers.
    pub const VOEBB_FREE_TERMS_AS_TITLE: &str = "voebb_free_terms_as_title";

    /// The query needed more rows than voebb.de's advanced form has, and the note names
    /// the part that was not sent.
    pub const VOEBB_QUERY_TRUNCATED: &str = "voebb_query_truncated";

    /// The branch facet does not list this branch for this search, which is the site's
    /// way of saying it holds nothing matching. Not an error, and not a broken filter.
    ///
    /// Only ever set when the **unfiltered** search had hits — otherwise there is no
    /// facet tree on the page at all, and describing one would be a claim about a
    /// structure that was never there. That case is
    /// [`VOEBB_NO_HITS_IN_NETWORK`] instead.
    pub const VOEBB_BRANCH_NOT_LISTED: &str = "voebb_branch_not_listed";

    /// The search found nothing anywhere in the VÖBB network, so there was no branch
    /// facet to apply and the branch in `--at` is not what came back empty.
    ///
    /// The distinction this tag exists for is a piece of advice: with hits in the network
    /// and none at the branch, naming more libraries is the way out; with none in the
    /// network it is guaranteed to fail again, and the words are what has to change.
    /// voebb.de is the only engine that can tell the two apart — it states the
    /// network-wide count beside the facet, while the KOBV side filters upstream and
    /// never sees an unrestricted total.
    pub const VOEBB_NO_HITS_IN_NETWORK: &str = "voebb_no_hits_in_network";

    /// A branch of a KOBV institution was answered from the **copies** of the records
    /// that were fetched, not by a filter the catalogue applied.
    ///
    /// The house is filtered upstream, so the records are complete for the house; which
    /// branch holds a copy is only ever said by the availability answer, and that is
    /// asked for one page at a time. So the block's total is `null`, the page can come
    /// back shorter than `--limit`, and nothing here proves that the branch does *not*
    /// hold an edition further down the result. A VÖBB branch is not this case — its
    /// house facet filters upstream and its block is complete.
    pub const BRANCH_FROM_COPIES: &str = "branch_from_copies";

    /// A branch of a KOBV institution could not be applied at all, because
    /// `--no-availability` means no copies were fetched and the copies are the only place
    /// the branch is named. The block is the whole house's.
    pub const BRANCH_NEEDS_COPIES: &str = "branch_needs_copies";

    /// One location's search was refused because the requested window begins past that
    /// location's **own** last result, so its block is empty and the other locations
    /// answered normally.
    ///
    /// Every location in `--at` is its own search with its own result set, and they end at
    /// different depths: measured 2026-09-07, `--at TU,HU --page 30 --limit 50` needs
    /// record 1451, which HU's 2005 hits reach and TU's 1106 do not. Until this tag
    /// existed, TU's refusal was the whole invocation's and the user got neither block —
    /// the failure of one location sold as the failure of the run (see
    /// [`crate::error::Blast`]).
    ///
    /// It says nothing about what the location holds. The count that came back beside the
    /// refusal is not to be believed either — a rejected SRU envelope may state
    /// `numberOfRecords` and have it be wrong — so such a block states no total, which is
    /// the second reason the tag is needed: without it, "no total, no records" is
    /// indistinguishable from a location the engine never reached.
    ///
    /// When **every** location answers this way there is nothing left for the refusal to be
    /// smaller than: the run ends with the catalogue's own message and exit 5, and no note
    /// is written at all.
    pub const LOCATION_PAST_THE_LAST_RESULT: &str = "location_past_the_last_result";

    /// One location's window lies deeper than its catalogue can be paged, so **that**
    /// location was not searched while the others were.
    ///
    /// voebb.de has no offset: position 220 is the tenth sequential page of a session
    /// replayed in order, and beyond it blibs refuses before opening one at all. Until
    /// 2026-09-09 that refusal was the whole invocation's — `--at HU,AGB --page 20
    /// --limit 20` was exit 2 with no block whatsoever, although HU reaches 400 records
    /// without effort. One location's ceiling deciding for every other is the same
    /// unevenness as [`LOCATION_PAST_THE_LAST_RESULT`], one step earlier: there the
    /// catalogue refuses, here blibs does on its behalf.
    ///
    /// **Not the same tag, and not the same sentence.** That one says a result set ran
    /// out, which is something the catalogue answered; this one says nobody asked. A
    /// block carrying it may well hold hundreds of hits at a shallower page.
    ///
    /// When **every** location is one of these the run is a usage error again
    /// ([`crate::error::UsageError::WindowTooDeep`], exit 2, before any request) — there
    /// is nothing the empty block could stand beside.
    pub const LOCATION_WINDOW_TOO_DEEP: &str = "location_window_too_deep";

    /// A location in `--at` is answered by the *other* catalogue than the record that was
    /// shown. The two are never matched against each other — a KOBV record states no
    /// branch and a voebb record no institution — so the location could not narrow this
    /// record and was ignored. Never a statement that the record is not held there.
    pub const LOCATION_OTHER_CATALOGUE: &str = "location_other_catalogue";

    /// The record is a serial, and the availability service answers one status for the
    /// *title* — no volume, no year, no shelfmark. Which volumes are actually held cannot
    /// be determined from it.
    ///
    /// **Not** set where the record states its run itself. voebb.de writes one into a
    /// `Bestand` row that reaches the output as
    /// [`crate::model::Holding::holdings_statement`] (`Bestand in ZLB: 1994/95,1 -
    /// 1998/99,17(22.Apr.) … Signatur: A 80 ZC 181`), and a note saying the years cannot
    /// be determined would then be printed directly above them. The tag means "nothing
    /// answered this", and it has to keep meaning only that.
    pub const SERIAL_VOLUMES_UNKNOWN: &str = "serial_volumes_unknown";

    /// A copy is on loan and carries no return date: due dates and holds live behind a
    /// patron login, which this tool never passes.
    ///
    /// Since round 3 this is a statement about the copies that are **out and dateless**,
    /// not about the catalogue: voebb.de does state return dates
    /// ([`crate::model::Item::due_date`]), and a record all of whose out copies carry one
    /// gets no note. A record with both kinds still does — `Verloren` and `Nicht im Regal`
    /// are out with nothing to say, and they are the reason the note is per record and the
    /// date is per copy.
    pub const LOAN_WITHOUT_DUE_DATE: &str = "loan_without_due_date";

    /// A key in `--at` is claimed by more than one entry of the library list, and the
    /// answer is about the **first** of them — the one the resolution order reaches.
    ///
    /// The quietest limitation this tool has. Nothing about such an answer looks wrong:
    /// the search runs, it succeeds, and it reports about a library the user never named,
    /// under a key that library's own detail view prints. There is no field that carries
    /// the discrepancy — `at[].given` holds the typed word and `at[].key` the resolved
    /// one, and a reader who does not already suspect a collision has no reason to compare
    /// them — so the tag is the only place an agent can find out.
    pub const LOCATION_KEY_AMBIGUOUS: &str = "location_key_ambiguous";
}

/// The query, echoed back so a result can be reproduced without the shell history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueryEcho {
    /// The terms as the user typed them.
    pub terms: String,
    /// The assembled PQF query, when a PQF engine ran.
    pub pqf: Option<String>,
}

/// The complete result of one invocation — and the JSON document.
///
/// Record-centric, not location-centric: a record appears **once** in `records` even when
/// it is displayed under three location blocks. The grouping humans see is derived from
/// `at[]` and `holdings[].isil`, so there is one schema, not two.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    /// What was searched.
    pub query: QueryEcho,
    /// Total hits of the KOBV search. `None` when only `voebb` ran.
    pub total: Option<u64>,
    /// How many records are displayed.
    pub shown: usize,
    /// The page that was displayed.
    pub page: Page,
    /// How many records a full page holds — `--limit`.
    ///
    /// Additive, and it exists so that the range of a page can be *computed* rather than
    /// guessed: the first record of this page is `(page - 1) * limit + 1`, and a short
    /// last page must not make the reading aid restart from a smaller stride.
    pub limit: usize,
    /// How the displayed records were ordered.
    pub sort: SortSpec,
    /// What the client-side filters could see.
    pub window: WindowInfo,
    /// Which engines ran.
    pub engines: Vec<Engine>,
    /// Per-location totals.
    pub at: Vec<AtBlock>,
    /// Whether availability was fetched.
    pub availability: AvailabilityMode,
    /// Limitations that are not errors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The records.
    pub records: Vec<Record>,
}

/// One entry of `show`'s `at[]`: a location of `--at` and this record's status **there**.
///
/// The same five identifying members [`AtBlock`] carries, so a reader who can read one
/// document can read the other; what replaces the hit count is the traffic light, because
/// `show` already knows which record is meant and the only open question is where it
/// stands.
///
/// Assembled by [`crate::select::show_at`] rather than here: which copies belong to a
/// branch is a selection question, and a second answer to it in `model` is exactly how
/// the human output and the JSON came to disagree in the first place.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShowAt {
    /// The library's canonical key — [`Location::key`].
    pub key: String,
    /// The word the user put in `--at` — [`Location::given`].
    pub given: String,
    /// The canonical ISIL.
    pub isil: Isil,
    /// The branch id, when the location is a branch.
    pub branch: Option<String>,
    /// The engine that answers for this location.
    pub engine: Engine,
    /// The record's traffic light **at this location**: summarised over the copies that
    /// stand there, and [`crate::model::Status::Unknown`] — never [`Status::Unavailable`] — when the
    /// location holds none, because nothing was said about it.
    pub status: Status,
}

/// The complete result of one `show` — and the JSON document.
///
/// A hull around the record rather than the bare record, because `show` has the same two
/// ambiguities [`SearchResult`] has and had nowhere to resolve them: without
/// [`Self::availability`] an empty `items[]` cannot be told from a `--no-availability`
/// run, and without [`Self::notes`] every limitation the search document states would be
/// silently dropped on the way through `show`. The members are the ones `search` uses and
/// carry the same meaning, so an agent reads both documents the same way.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShowResult {
    /// The record, or `null` when the catalogue has no such record. Even then the
    /// document is complete: `availability` still says whether copies were asked for, and
    /// a note may still say why `--at` did not apply.
    pub record: Option<Record>,
    /// One entry per `--at` location, with **that location's** traffic light.
    ///
    /// The counterpart of [`SearchResult::at`], and the field the obvious pipeline should
    /// read: `at[] | select(.given == "AGB") | .status` answers "is it in at the AGB",
    /// which is the question `--at` asked. `holdings[].summary` cannot answer it — it is
    /// what the catalogue said about the whole *house*, and a VÖBB holding is the whole
    /// network — so reading the answer off a holding got it exactly backwards for a copy
    /// on loan at one branch and in at three others.
    ///
    /// Absent when `--at` was not given, like every other optional member: no location
    /// was named, so there is no per-location answer to state.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<ShowAt>,
    /// Whether availability was **asked for**.
    ///
    /// What it rules out is an empty `items[]` reading as "we did not ask". It is not a
    /// promise in the other direction: voebb.de states the copies on the record page
    /// itself, so a `skipped` lookup there still comes back with them — there is no
    /// cheaper page to ask for and throwing them away would answer less for the same
    /// request.
    pub availability: AvailabilityMode,
    /// Limitations that are not errors, exactly as in [`SearchResult::notes`] — same
    /// vocabulary, same `skip_serializing_if`, so the two documents cannot grow two
    /// styles of the same list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

impl ShowResult {
    /// Assemble the document, deriving the notes the record and the request imply.
    ///
    /// The derivation lives here rather than in a renderer so that the human output and
    /// the JSON cannot state different limitations for one record: both read
    /// [`Self::notes`], and `kind` is what an agent branches on.
    ///
    /// `engine` is the record id's engine — the catalogue that answered — and `locations`
    /// is `--at` as the user wrote it, which may name a location the *other* catalogue
    /// answers for.
    ///
    /// [`Self::at`] is **not** derived here. It needs the branch narrowing, which lives
    /// in `select`, and `cli` fills it with [`crate::select::show_at`] exactly as it
    /// fills [`SearchResult::at`] on the search path — one place per document, and no
    /// second copy of the narrowing rule inside `model`.
    pub fn new(
        record: Option<Record>,
        engine: Engine,
        locations: &[Location],
        availability: AvailabilityMode,
    ) -> Self {
        let mut notes = Vec::new();
        notes.extend(ambiguous_key_notes(locations));
        notes.extend(other_catalogue_note(record.as_ref(), engine, locations));
        notes.extend(kobv_branch_note(locations, availability));
        if let Some(record) = &record {
            notes.extend(record_notes(record));
        }
        Self {
            record,
            at: Vec::new(),
            availability,
            notes: Note::merged(notes),
        }
    }

    /// Put the engine's notes in front of the derived ones, folding duplicates.
    ///
    /// In front because the engine's notes say what the *page* could not state — an
    /// e-lending title has no item table at all — and that has to be read before the
    /// sentences about a serial's volumes or a copy's due date, which assume a list of
    /// copies exists.
    ///
    /// A method rather than a splice at the call site so that [`Self::notes`] is folded
    /// after every way it can be filled: [`Self::new`] folds what it derives, this folds
    /// what the engine adds, and `search` folds its own list through the same
    /// [`Note::merged`]. One idea of what a duplicate note is, for both documents.
    pub fn prepend_notes(&mut self, notes: Vec<Note>) {
        let mut all = notes;
        all.append(&mut self.notes);
        self.notes = Note::merged(all);
    }
}

/// The note for a `--at` entry that is a branch of a KOBV institution.
///
/// Here the branch narrows *which copies of this one record* are shown, and that is
/// complete — the availability answer lists every copy the institution holds. What is not
/// complete is the silence: a copy whose location cell carries no `bibids=` link names no
/// branch, so "no copies at this branch" can also mean "the link was missing". The note
/// says which question was answered; without copies at all it says that instead.
fn kobv_branch_note(locations: &[Location], availability: AvailabilityMode) -> Option<Note> {
    let branch = locations
        .iter()
        .find(|location| location.branch.is_some() && location.engine == Engine::Kobv)?;
    Some(match availability {
        AvailabilityMode::Fetched => Note::new(
            note_kinds::BRANCH_FROM_COPIES,
            format!(
                "--at {} narrows the copies of this record, not the record itself — the \
                 branch of a copy is read from the availability answer, and a copy whose \
                 location names no branch is kept rather than hidden.",
                branch.key
            ),
        ),
        AvailabilityMode::Skipped => Note::new(
            note_kinds::BRANCH_NEEDS_COPIES,
            format!(
                "--at {} could not be applied: the branch of a copy is only named in the \
                 availability answer, and --no-availability did not ask for one.",
                branch.key
            ),
        ),
    })
}

/// The note for a `--at` entry the other catalogue answers for.
///
/// `show voebb_SAK… --at HU` and `show almafu_BV… --at AGB` are the two shapes of it. The
/// *engines* are unrelated — the KOBV record does not know the voebb branch, the voebb
/// record does not know the KOBV institution — but that does not mean the location found
/// nothing: a ZLB record reached through the KOBV union index routinely carries the very
/// `DE-609` holding the voebb engine would have shown, so the note fires only when **none
/// of the record's holdings claim the location** — checked with
/// [`crate::select::holding_is_at`], the exact predicate [`crate::select::mark_mine`]
/// uses to set `holding.mine`. Reusing it rather than a bare ISIL comparison is the fix:
/// a second, looser test is exactly how this note came to contradict a `mine: true`
/// sitting next to it. That also settles the harder case a branch of the other catalogue
/// poses — the ISIL alone can match (every VÖBB copy is catalogued under `DE-609`)
/// without proving the copy stands in *that* branch — because `holding_is_at` already
/// carries the branch rule: it credits the branch when a copy names it, and, when no copy
/// names any branch at all, treats that as "not stated" rather than "elsewhere" and
/// credits the location anyway, the same leniency [`crate::select::mark_mine`] shows. So
/// this note and `mine: true` are never in tension: whichever way `holding_is_at` answers
/// for a holding is *also* the answer `mine` carries for it.
///
/// This note must **never** fire for a location a holding of the record claims. When the
/// record itself is unknown (`show` found nothing to check) there are no holdings to
/// consult, so it fires for every location of the other catalogue, exactly as before —
/// there is nothing for it to contradict.
fn other_catalogue_note(
    record: Option<&Record>,
    engine: Engine,
    locations: &[Location],
) -> Option<Note> {
    let elsewhere: Vec<&Location> = locations
        .iter()
        .filter(|location| location.engine != engine)
        .filter(|location| match record {
            Some(record) => !record
                .holdings
                .iter()
                .any(|holding| crate::select::holding_is_at(holding, location)),
            None => true,
        })
        .collect();
    let other = elsewhere.first()?.engine;
    let keys: Vec<&str> = elsewhere
        .iter()
        .map(|location| location.key.as_str())
        .collect();
    let verb = if keys.len() == 1 { "is" } else { "are" };
    Some(Note::new(
        note_kinds::LOCATION_OTHER_CATALOGUE,
        format!(
            "--at {} {verb} answered by the {} catalogue and this record comes from {}; \
             the two are never matched against each other, so the location did not apply \
             here — it says nothing about whether the copy stands there",
            keys.join(", "),
            other.as_str(),
            engine.as_str()
        ),
    ))
}

/// The note for a `--at` key that names more than one library.
///
/// **A note on the result, not a reason for an empty one.** The empty explanation was the
/// obvious home — the case that hurts most is a shared key answering "no results" for a
/// house the user never named — but it is the wrong one twice over. A search that *finds*
/// something is exactly as wrong and much quieter, and it would say nothing at all;
/// and an [`crate::error::EmptyReason`] is a next step for a search that came back empty,
/// while this is a statement about the *question*, which is true whatever the answer was.
/// So it rides with the other limitations, in the footer and in `notes[]`, and it fires
/// for a full page as readily as for an empty one.
///
/// Fired off [`Location::given`] — the word the user actually typed — and off nothing
/// else. `--at ZLB` and `--at AGB` are aliases, `--at SIG00036` is a KOBV id, and none of
/// the three is claimed twice; only the bare code that two entries of the list carry is,
/// and only for it does the answer go somewhere the user could not see coming. The
/// resolution order is not changed by any of this: the first claimant still wins, and this
/// is the sentence saying so.
///
/// One note per ambiguous location, because the message names its key: two of them in one
/// sentence would be unreadable, and `records[]` cannot carry a location.
pub fn ambiguous_key_notes(locations: &[Location]) -> Vec<Note> {
    locations.iter().filter_map(ambiguous_key_note).collect()
}

/// The note for one location, or `None` when its key names exactly one place.
fn ambiguous_key_note(location: &Location) -> Option<Note> {
    let shadowed = crate::libraries::shadowed_by_key(&location.given);
    if shadowed.is_empty() {
        return None;
    }
    let missed: Vec<String> = shadowed
        .into_iter()
        .map(crate::libraries::entry_location)
        .map(|other| format!("{}, reached by --at {}", other.display, other.key))
        .collect();
    // The canonical key of the entry that did answer, but only when it is a different
    // word: for a library the list gives no alias to it *is* the typed code, and
    // "whose own key is DE-…" about the code the reader just typed explains nothing.
    let answered = if location.key.eq_ignore_ascii_case(&location.given) {
        location.display.clone()
    } else {
        format!("{}, whose own key is {}", location.display, location.key)
    };
    Some(Note::new(
        note_kinds::LOCATION_KEY_AMBIGUOUS,
        format!(
            "--at {given} answered for {answered} — more than one entry of the library \
             list carries {given}, and the key reaches the first of them only. Not \
             searched: {}.",
            missed.join("; "),
            given = location.given,
        ),
    ))
}

/// The note for the locations whose window began past their own last result.
///
/// **One note for all of them, naming them in it.** The alternative — a note per location,
/// each naming its own — is the shape [`Note::merged`] cannot fold: it folds on `kind`
/// *and* `message`, so two locations running out at the same moment would print two
/// near-identical paragraphs differing in one word, which is the failure
/// `voebb_online_url_only` was already fixed for once. The same reasoning gives
/// `branch_from_copies` its shape, and it is followed here.
///
/// `keys` are [`Location::key`]s, in the order the user wrote `--at`. Empty means nothing
/// ran out and there is no note to write.
///
/// Called from both levels that can meet this refusal — one location of an engine, and one
/// whole engine of a run — so that the limitation has one wording wherever it is caught.
pub fn past_the_last_result_note(keys: &[&str]) -> Option<Note> {
    let Named { at, subject, block } = Named::of(keys)?;
    Some(Note::new(
        note_kinds::LOCATION_PAST_THE_LAST_RESULT,
        format!(
            "{at}: the window begins past the last result the catalogue has for \
             {subject}, which leaves {block} empty on this page. That says how far the \
             result set reaches, not what is held, and no total is stated — the count \
             beside a refused window is not to be trusted. Lower --page, or ask a location \
             on its own to see how far it goes; the other locations answered normally."
        ),
    ))
}

/// The note for the locations whose window is deeper than their catalogue can be paged.
///
/// The sister of [`past_the_last_result_note`], and deliberately a different sentence:
/// that one reports a result set that ended, this one reports a question nobody asked.
/// Saying "past the last result" here would claim that a branch holds fewer records than
/// the page asked for, which no request ever established — the depth is blibs's own
/// ceiling on a catalogue with no offset, applied before the session is opened.
///
/// `keys` are [`Location::key`]s in `--at` order, `max` the deepest position the
/// catalogue serves, and `position` the one this window needed. Empty `keys` means every
/// location could be paged and there is no note to write; **all** of them refused is not
/// this function's case at all but [`crate::error::UsageError::WindowTooDeep`], because a
/// note needs an answer to stand beside.
pub fn window_too_deep_note(keys: &[&str], position: u32, max: u32) -> Option<Note> {
    let Named { at, subject, block } = Named::of(keys)?;
    Some(Note::new(
        note_kinds::LOCATION_WINDOW_TOO_DEEP,
        format!(
            "{at}: this page needs result {position} and the catalogue answering for \
             {subject} is paged one sequential request at a time, so blibs stops at {max} \
             and asked it nothing — which leaves {block} empty. Nothing was looked up, so \
             this says nothing about what is held there. Lower --page or --limit to reach \
             {subject} again; the other locations answered normally."
        ),
    ))
}

/// The three pieces of grammar a refusal note needs about the locations it names: how to
/// list them, and how to speak about them once and about their blocks once.
///
/// One place, because two notes that number their subjects differently read as two
/// different limitations — and because a note that interpolates a location per sentence
/// stops folding (`Note::merged` folds on `kind` *and* `message`).
struct Named {
    /// `--at TU` or `--at TU, --at HU`.
    at: String,
    /// "that location" or "those locations".
    subject: &'static str,
    /// "its block" or "their blocks".
    block: &'static str,
}

impl Named {
    /// `None` for no locations at all, which is the "no note to write" case.
    fn of(keys: &[&str]) -> Option<Self> {
        match keys {
            [] => None,
            [only] => Some(Self {
                at: format!("--at {only}"),
                subject: "that location",
                block: "its block",
            }),
            several => Some(Self {
                at: format!("--at {}", several.join(", --at ")),
                subject: "those locations",
                block: "their blocks",
            }),
        }
    }
}

/// The two limitations a record itself implies: the run of a serial, and the due date of
/// a copy that is out. Each is stated **once**, not per copy.
///
/// Both are notes about an **absence**, and both grew a case where the absence is no
/// longer there (round 3): voebb.de states a serial's run in prose and a copy's return
/// date beside its status. A note that fires whether or not the answer is present stops
/// being a signal an agent can switch on, and — worse — stands next to the answer
/// contradicting it. So each is asked for the answer first.
fn record_notes(record: &Record) -> Vec<Note> {
    let mut notes = Vec::new();
    // The serial note says which volumes are held "cannot be determined here". Where the
    // record states its run itself — `Holding::holdings_statement`, voebb.de's `Bestand`
    // line — that is simply untrue, and it would be printed directly above the years it
    // denies. Rewording it was the alternative and is the wrong one: what a re-worded note
    // would add ("the run is prose, and names one library") is what the field already
    // shows, and the tag would then mean two opposite things.
    let run_stated = record
        .holdings
        .iter()
        .any(|holding| holding.holdings_statement.is_some());
    if matches!(record.format, Format::Journal | Format::Ejournal) && !run_stated {
        notes.push(Note::new(note_kinds::SERIAL_VOLUMES_UNKNOWN, SERIAL_NOTE));
    }
    // A copy that is out **and** states no date. voebb.de states one for most of them
    // (`Item::due_date`), and the note beside a copy line reading `on loan, due 22 Sep
    // 2026` would deny what the line says.
    //
    // Never for an online resource either: nothing about it is on loan, its copy lines say
    // "currently unavailable" rather than "on loan", and this sentence beside them was the
    // second half of the invented loan (round 2, §1.9).
    let undated = record
        .holdings
        .iter()
        .flat_map(|holding| &holding.items)
        .any(|item| item.status == Status::Unavailable && item.due_date.is_none());
    if undated && !record.is_online_resource() {
        notes.push(Note::new(note_kinds::LOAN_WITHOUT_DUE_DATE, LOAN_NOTE));
    }
    notes
}

/// What a serial's holdings cannot say. The availability service reports one traffic
/// light for the title, with no years, so a green light would otherwise read as a
/// statement about the volume the user is after.
const SERIAL_NOTE: &str = "which volumes are held cannot be determined here — the service reports one status \
     for the title, without years; check the library's own catalogue or the ZDB";

/// What a copy on loan cannot say. Due dates and holds live behind a patron login, and
/// this tool never signs in — so it says that instead of suggesting a date.
///
/// The wording still holds after `Item::due_date` (round 3), because the note is now only
/// ever attached to a record with a copy that is out and states **no** date: for that copy
/// the date really is only in the library's own catalogue. It is a sentence about the
/// copies that have nothing to say, not a claim that no catalogue ever states a date.
const LOAN_NOTE: &str = "a copy on loan carries no due date here — return dates and holds \
     are only in the library's own catalogue, behind a patron login";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, ResourceUrl};
    use crate::model::{
        AuthorKind, Format, Holding, Item, Limit, RecordId, SortKey, SortScope, Status, UrlKind,
    };

    /// Every tag [`note_kinds`] declares, read out of this file's own source.
    ///
    /// **Not a hand-kept list, because the hand-kept one had already gone stale.** It
    /// named 27 of the 29 constants: `branch_from_copies` and `branch_needs_copies` were
    /// never added to it, so the two tags were exempt from the only test that guards the
    /// vocabulary — which is precisely the failure the vocabulary exists to prevent, one
    /// level up. A list that has to be edited whenever a constant is added will be
    /// forgotten again; the source cannot be.
    ///
    /// Returns `(constant name, tag)` pairs, so the test can also hold the two to each
    /// other.
    fn declared_note_kinds() -> Vec<(&'static str, &'static str)> {
        const SOURCE: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/model/search.rs"));
        let module = SOURCE
            .split_once("pub mod note_kinds {")
            .expect("this file declares `pub mod note_kinds`")
            .1;
        // The constants are indented, so the module's own closing brace is the first one
        // in column zero.
        let module = module.split_once("\n}\n").map_or(module, |(body, _)| body);
        let declared = module.matches("pub const ").count();

        let kinds: Vec<(&str, &str)> = module
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("pub const ")?;
                let (name, rest) = rest.split_once(": &str = ")?;
                let tag = rest.trim().strip_prefix('"')?.strip_suffix("\";")?;
                Some((name, tag))
            })
            .collect();

        // A declaration this scan cannot read must fail the test rather than slip past it
        // — that is the whole point of not keeping the list by hand.
        assert_eq!(
            kinds.len(),
            declared,
            "note_kinds declares {declared} constants and this scan read {}; a declaration \
             it cannot parse is one it would silently exempt",
            kinds.len()
        );
        // And a scan that suddenly finds nothing (a renamed module, a reformatted file)
        // has to fail too, instead of passing over an empty list.
        assert!(kinds.len() >= 29, "only {} tags found", kinds.len());
        kinds
    }

    /// The note vocabulary is the agent-facing half of `notes[]`. Two modules spelling
    /// the same limitation two ways is exactly what the constants prevent, so every tag
    /// is checked for duplicates, for the one shape agents can rely on, and against the
    /// name of its own constant — a tag that no longer matches its name is how a second
    /// spelling gets in without anyone reading both lines at once.
    #[test]
    fn note_kinds_are_unique_lowercase_tags() {
        let all = declared_note_kinds();
        for (index, (name, tag)) in all.iter().enumerate() {
            assert!(
                !tag.is_empty()
                    && tag
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{tag:?} is not a snake_case tag"
            );
            assert_eq!(
                *tag,
                name.to_ascii_lowercase(),
                "the constant {name} and its tag have to say the same thing"
            );
            assert!(
                all[index + 1..].iter().all(|(_, other)| other != tag),
                "{tag:?} is used for two different notes"
            );
        }
    }

    /// The two tags the hand-kept list had lost, named here so the regression has a name
    /// and not only a mechanism.
    #[test]
    fn the_branch_tags_are_part_of_the_checked_vocabulary() {
        let all = declared_note_kinds();
        for tag in [
            note_kinds::BRANCH_FROM_COPIES,
            note_kinds::BRANCH_NEEDS_COPIES,
        ] {
            assert!(
                all.iter().any(|(_, declared)| *declared == tag),
                "{tag:?} escaped the vocabulary check"
            );
        }
    }

    /// The word a sort key is written with is one word, not two: `--sort year`, the JSON
    /// `sort.by`, and the message that says a sorted window ran out must all say `year`.
    #[test]
    fn a_sort_key_spells_itself_the_same_way_everywhere() {
        for key in [
            SortKey::Relevance,
            SortKey::Year,
            SortKey::Title,
            SortKey::Author,
            SortKey::Availability,
        ] {
            let json = serde_json::to_value(key).expect("a sort key serialises");
            assert_eq!(json, serde_json::Value::String(key.as_str().to_owned()));
        }
    }

    /// The shell strips the quotes, so whitespace is the only evidence left that the user
    /// asked for a phrase.
    #[test]
    fn whitespace_is_the_only_thing_that_makes_a_phrase() {
        assert_eq!(Term::from_argument("Kafka"), Term::Word("Kafka".to_owned()));
        assert_eq!(
            Term::from_argument("Der Prozess"),
            Term::Phrase("Der Prozess".to_owned())
        );
        assert_eq!(
            Term::from_argument("  Kafka  "),
            Term::Word("Kafka".to_owned())
        );
        assert_eq!(
            Term::from_argument("Der\tProzess"),
            Term::Phrase("Der\tProzess".to_owned())
        );
    }

    #[test]
    fn a_whitespace_only_argument_is_an_empty_term() {
        assert!(Term::from_argument("   ").is_empty());
        assert!(!Term::from_argument("Kafka").is_empty());
        assert_eq!(Term::from_argument("Kafka").text(), "Kafka");
    }

    #[test]
    fn a_query_of_only_whitespace_terms_is_empty() {
        assert!(QuerySpec::default().is_empty());
        assert!(
            QuerySpec {
                terms: vec![Term::from_argument("  ")],
                ..QuerySpec::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn any_single_field_makes_a_query_non_empty() {
        let with = |spec: QuerySpec| assert!(!spec.is_empty());
        with(QuerySpec {
            terms: vec![Term::from_argument("Kafka")],
            ..QuerySpec::default()
        });
        with(QuerySpec {
            title: Some(Term::from_argument("Prozess")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            subject: Some(Term::from_argument("Roman")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            publisher: Some(Term::from_argument("Fischer")),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            author: Some("Kafka, Franz".to_owned()),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            year: Some(1953),
            ..QuerySpec::default()
        });
        with(QuerySpec {
            identifier: Some(Identifier::Isbn("9783596294331".to_owned())),
            ..QuerySpec::default()
        });
    }

    /// The echo must be paste-able: without the quotes, `blibs search "Der Prozess"`
    /// would come back as two and-ed words and find something else.
    #[test]
    fn the_echo_reproduces_the_free_terms_including_their_quotes() {
        let spec = QuerySpec {
            terms: vec![
                Term::from_argument("Kafka"),
                Term::from_argument("Der Prozess"),
            ],
            ..QuerySpec::default()
        };
        assert_eq!(spec.echo(), "Kafka \"Der Prozess\"");
    }

    /// Flags reproduce themselves; folding them into the term echo would produce a string
    /// that does not run.
    #[test]
    fn the_echo_covers_free_terms_only() {
        let spec = QuerySpec {
            author: Some("Kafka".to_owned()),
            year: Some(1953),
            ..QuerySpec::default()
        };
        assert_eq!(spec.echo(), "");
    }

    /// A fully populated result: both engines, two locations, two records, two holdings
    /// with items on the KOBV one, an `order_option` on the voebb one, and a note.
    fn full_result() -> SearchResult {
        SearchResult {
            query: QueryEcho {
                terms: "Kafka \"Der Prozess\"".to_owned(),
                pqf: Some("@and @attr 1=1016 \"Kafka\" @attr 1=1016 \"Der Prozess\"".to_owned()),
            },
            total: Some(774),
            shown: 2,
            page: Page::FIRST,
            limit: usize::from(Limit::DEFAULT.get()),
            sort: SortSpec {
                by: SortKey::Relevance,
                scope: SortScope::Fetched,
            },
            window: WindowInfo {
                fetched: 50,
                after_filter: 2,
                filtered: false,
                undelivered: 0,
                before_available: None,
            },
            engines: vec![Engine::Kobv, Engine::Voebb],
            at: vec![
                AtBlock {
                    key: "HU".to_owned(),
                    given: "HU".to_owned(),
                    isil: Isil::new("DE-11"),
                    branch: None,
                    engine: Engine::Kobv,
                    total: Some(6),
                    records: vec![
                        RecordId::parse("almafu_BV008885798").expect("a prefixed id parses"),
                    ],
                    refused: None,
                },
                AtBlock {
                    key: "AGB".to_owned(),
                    given: "AGB".to_owned(),
                    isil: Isil::new("DE-609"),
                    branch: Some("SIG00036".to_owned()),
                    engine: Engine::Voebb,
                    total: Some(35),
                    records: vec![RecordId::voebb("SAK13776205")],
                    refused: None,
                },
            ],
            availability: AvailabilityMode::Fetched,
            notes: vec![
                // A note about the answer as a whole: the record it would name is the one
                // that never arrived, so there is no id to give and `records` is absent.
                Note::new(
                    note_kinds::RECORD_UNDELIVERED,
                    "record 49 of the SRU response was a diagnostic and was skipped",
                ),
                // And one about a particular record, so the snapshot pins both shapes.
                Note::about(
                    note_kinds::VOEBB_ONLINE_ONLY,
                    "an Onleihe title has no copies on a shelf; its loan state is stated \
                     only in the link to the lending platform",
                    [RecordId::voebb("SAK13776205")],
                ),
            ],
            records: vec![kobv_record(), voebb_record()],
        }
    }

    fn kobv_record() -> Record {
        Record {
            id: RecordId::parse("almafu_BV008885798").expect("a prefixed id parses"),
            title: "Der Prozess".to_owned(),
            subtitle: Some("Roman".to_owned()),
            authors: vec![
                Author {
                    name: "Kafka, Franz".to_owned(),
                    kind: AuthorKind::Person,
                    dates: Some("1883-1924".to_owned()),
                    gnd: Some("118559230".to_owned()),
                    role: Some("author".to_owned()),
                    role_code: Some("aut".to_owned()),
                },
                Author {
                    name: "S. Fischer Verlag".to_owned(),
                    kind: AuthorKind::Corporate,
                    dates: None,
                    gnd: None,
                    role: None,
                    role_code: None,
                },
            ],
            year: Some(1953),
            publisher: Some("S. Fischer".to_owned()),
            place: Some("Frankfurt am Main".to_owned()),
            edition: Some("2. Auflage".to_owned()),
            extent: Some("345 Seiten".to_owned()),
            languages: vec!["ger".to_owned()],
            format: Format::Book,
            online: false,
            isbns: vec!["9783596294331".to_owned()],
            subjects: vec!["Deutsche Literatur".to_owned(), "Roman".to_owned()],
            urls: vec![ResourceUrl {
                url: "https://d-nb.info/930000000/04".to_owned(),
                kind: UrlKind::Toc,
                label: Some("Inhaltsverzeichnis".to_owned()),
            }],
            holdings: vec![
                Holding {
                    isil: Some(Isil::new("DE-11")),
                    alias: Some("HU".to_owned()),
                    library: "Humboldt-Universität zu Berlin, Universitätsbibliothek".to_owned(),
                    short_name: Some("HU Berlin".to_owned()),
                    local_id: Some("BV008885798".to_owned()),
                    mine: true,
                    summary: Status::Available,
                    // Prose holdings: only voebb.de states any.
                    holdings_statement: None,
                    online_access: None,
                    items: vec![
                        Item {
                            location: Some("ZB Grimm-Zentrum, 7. OG / Bereich B".to_owned()),
                            branch: Some("KOB00032".to_owned()),
                            branch_name: Some("Grimm-Zentrum".to_owned()),
                            call_number: Some("96 A 10064".to_owned()),
                            volume: None,
                            status: Status::Available,
                            // A return date: only voebb.de states one.
                            due_date: None,
                            order_option: None,
                        },
                        Item {
                            location: Some("ZB Grimm-Zentrum, Magazin".to_owned()),
                            branch: None,
                            branch_name: None,
                            call_number: Some("96 A 10064+1".to_owned()),
                            volume: Some("1".to_owned()),
                            status: Status::Unavailable,
                            // A return date: only voebb.de states one.
                            due_date: None,
                            order_option: None,
                        },
                    ],
                },
                // An ISIL that is not in the library list: the display name falls back to
                // the bare code and the holding is still shown.
                Holding {
                    isil: Some(Isil::new("DE-Zz999")),
                    alias: None,
                    library: "DE-Zz999".to_owned(),
                    short_name: None,
                    local_id: Some("275177939".to_owned()),
                    mine: false,
                    summary: Status::Reference,
                    // Prose holdings: only voebb.de states any.
                    holdings_statement: None,
                    online_access: None,
                    // The portal wrote one of its placeholders in the location cell:
                    // what is not stated is `null`, never an empty string.
                    items: vec![Item {
                        location: None,
                        branch: None,
                        branch_name: None,
                        call_number: Some("A 1234".to_owned()),
                        volume: None,
                        status: Status::Reference,
                        // A return date: only voebb.de states one.
                        due_date: None,
                        order_option: None,
                    }],
                },
            ],
        }
    }

    fn voebb_record() -> Record {
        Record {
            id: RecordId::voebb("SAK13776205"),
            title: "Der Prozeß".to_owned(),
            subtitle: None,
            authors: vec![Author {
                name: "Kafka, Franz".to_owned(),
                kind: AuthorKind::Person,
                dates: None,
                gnd: None,
                role: None,
                role_code: None,
            }],
            year: Some(2008),
            publisher: None,
            place: None,
            edition: None,
            extent: None,
            languages: vec![],
            format: Format::Book,
            online: false,
            isbns: vec![],
            subjects: vec![],
            urls: vec![],
            holdings: vec![Holding {
                isil: Some(Isil::new("DE-609")),
                alias: Some("AGB".to_owned()),
                library: "Zentral- und Landesbibliothek Berlin, Amerika-Gedenkbibliothek"
                    .to_owned(),
                short_name: Some("AGB".to_owned()),
                local_id: None,
                mine: true,
                summary: Status::Reference,
                // Prose holdings: only voebb.de states any.
                holdings_statement: None,
                online_access: None,
                items: vec![Item {
                    location: Some("AGB Erwachsenenbibliothek".to_owned()),
                    branch: Some("SIG00036".to_owned()),
                    branch_name: Some("Amerika-Gedenkbibliothek".to_owned()),
                    call_number: Some("Kaf 1".to_owned()),
                    volume: None,
                    status: Status::Reference,
                    // A return date: only voebb.de states one.
                    due_date: None,
                    order_option: Some("nicht entleihbar (Freihand) - Präsenzbestand".to_owned()),
                }],
            }],
        }
    }

    /// The JSON document is the agent-facing contract: member **names and order** are
    /// fixed, missing values are `null` or `[]` and never an empty
    /// string. Adding a member is allowed and shows up here as a snapshot diff to be
    /// reviewed; renaming, reordering or removing one is a break.
    #[test]
    fn the_search_document_matches_the_committed_schema_snapshot() {
        let rendered =
            serde_json::to_string_pretty(&full_result()).expect("a SearchResult always serialises");
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/schema/search.json"
        ));
        assert_eq!(
            rendered.trim_end(),
            expected.trim_end(),
            "the JSON schema changed; review the diff before updating \
             tests/fixtures/schema/search.json"
        );
    }

    /// The record members, in the order the contract fixes them. Spelled out separately
    /// from the snapshot so that a reordering names itself instead of showing up as a
    /// wall of diff.
    #[test]
    fn record_members_appear_in_the_documented_order() {
        // `serde_json` is built with `preserve_order`, so the object keeps the order the
        // struct serialised them in.
        let value = serde_json::to_value(kobv_record()).expect("a Record always serialises");
        let object = value.as_object().expect("a Record is a JSON object");
        let members: Vec<&str> = object.keys().map(String::as_str).collect();
        let expected = [
            "id",
            "engine",
            "source",
            "local_id",
            "title",
            "subtitle",
            "authors",
            "year",
            "publisher",
            "place",
            "edition",
            "extent",
            "languages",
            "format",
            "online",
            "isbns",
            "subjects",
            "urls",
            "holdings",
        ];
        assert_eq!(members, expected);
    }

    /// `undelivered` is additive: absent when it is zero, present the moment SRU
    /// announces more records than it delivers.
    #[test]
    fn undelivered_is_reported_only_when_it_happened() {
        let quiet = serde_json::to_string(&WindowInfo {
            fetched: 50,
            after_filter: 2,
            filtered: false,
            undelivered: 0,
            before_available: None,
        })
        .expect("WindowInfo serialises");
        assert_eq!(quiet, r#"{"fetched":50,"after_filter":2,"filtered":false}"#);

        let loud = serde_json::to_string(&WindowInfo {
            fetched: 48,
            after_filter: 2,
            filtered: false,
            undelivered: 2,
            before_available: None,
        })
        .expect("WindowInfo serialises");
        assert_eq!(
            loud,
            r#"{"fetched":48,"after_filter":2,"filtered":false,"undelivered":2}"#
        );
    }

    /// `before_available` is additive too, and its absence is the signal that
    /// `--available` did not run at all — which is why the committed schema snapshot,
    /// taken without the flag, does not change.
    #[test]
    fn the_available_filter_is_reported_only_when_it_ran() {
        let unfiltered = serde_json::to_string(&WindowInfo {
            fetched: 10,
            after_filter: 10,
            filtered: false,
            undelivered: 0,
            before_available: None,
        })
        .expect("WindowInfo serialises");
        assert_eq!(
            unfiltered,
            r#"{"fetched":10,"after_filter":10,"filtered":false}"#
        );

        let filtered = serde_json::to_string(&WindowInfo {
            fetched: 10,
            after_filter: 10,
            filtered: false,
            undelivered: 0,
            before_available: Some(10),
        })
        .expect("WindowInfo serialises");
        assert_eq!(
            filtered,
            r#"{"fetched":10,"after_filter":10,"filtered":false,"before_available":10}"#
        );
    }

    /// §3.5: three `voebb_online_only` notes over two blocks name no record, so a reader
    /// has to guess which of the displayed records each is about — and `message` is prose
    /// an agent is forbidden to parse. `records[]` is that answer, and it is additive: a
    /// note about the whole answer carries none and the member stays out of the document.
    #[test]
    fn a_note_names_the_records_it_is_about_and_only_then() {
        let ids = [RecordId::voebb("SAK1"), RecordId::voebb("SAK2")];
        let note = Note::about(
            note_kinds::VOEBB_ONLINE_ONLY,
            "an Onleihe title",
            ids.clone(),
        );
        let json = serde_json::to_value(&note).expect("a Note always serialises");
        assert_eq!(json["kind"], note_kinds::VOEBB_ONLINE_ONLY);
        // Plain id strings, as a user types them back in — not four members each.
        assert_eq!(
            json["records"],
            serde_json::json!(["voebb_SAK1", "voebb_SAK2"])
        );

        let whole = Note::new(note_kinds::RESULT_ORDER_UNSTABLE, "pages may overlap");
        assert!(whole.records.is_empty());
        let json = serde_json::to_value(&whole).expect("a Note always serialises");
        assert!(json.get("records").is_none(), "{json}");

        // Naming nothing is naming nothing, not an empty list in the document.
        let unnamed = Note::about(note_kinds::VOEBB_ONLINE_ONLY, "an Onleihe title", []);
        assert_eq!(
            unnamed,
            Note::new(note_kinds::VOEBB_ONLINE_ONLY, "an Onleihe title")
        );
    }

    /// Four records that hit one limitation are **one** note naming four records.
    ///
    /// Measured 2026-09-08: `search --author Kafka --at AGB` printed the same
    /// `voebb_online_state_unstated` paragraph four times, and nothing beside it told a
    /// reader which four of the ten displayed lines were meant — although `records[]` had
    /// said so all along.
    #[test]
    fn notes_that_say_the_same_thing_become_one_naming_every_record() {
        let same = |local: &str| {
            Note::about(
                note_kinds::VOEBB_ONLINE_STATE_UNSTATED,
                "an electronic title has no copies on a shelf",
                [RecordId::voebb(local)],
            )
        };
        let merged = Note::merged(vec![
            same("SAK1"),
            same("SAK2"),
            same("SAK3"),
            // The same record twice — two locations can return one edition — is named
            // once, because `records[]` is a set of records and not a tally of notes.
            same("SAK2"),
        ]);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(
            merged[0].records,
            vec![
                RecordId::voebb("SAK1"),
                RecordId::voebb("SAK2"),
                RecordId::voebb("SAK3")
            ],
            "first appearance decides the order: {merged:?}"
        );
    }

    /// One kind is not one statement. Two notes of a kind whose messages differ name
    /// another platform, another quoted wording or another selector, and folding them
    /// would keep one sentence and silently drop the other.
    #[test]
    fn notes_of_one_kind_that_say_different_things_stay_apart() {
        let onleihe = Note::about(
            note_kinds::VOEBB_ONLINE_ONLY,
            "the Link zur Onleihe row states: ausgeliehen",
            [RecordId::voebb("SAK1")],
        );
        let overdrive = Note::about(
            note_kinds::VOEBB_ONLINE_ONLY,
            "the Link zu Overdrive row states: verfügbar",
            [RecordId::voebb("SAK2")],
        );
        let merged = Note::merged(vec![onleihe.clone(), overdrive.clone(), onleihe.clone()]);
        assert_eq!(merged, vec![onleihe, overdrive], "{merged:?}");
    }

    /// The fold reaches both documents, and `show` folds again after the engine's notes
    /// are put in front of the derived ones — otherwise the one place a note can be added
    /// after assembly would be the one place duplicates survive.
    #[test]
    fn a_show_document_folds_the_engines_notes_too() {
        let same = || {
            Note::about(
                note_kinds::VOEBB_ONLINE_ONLY,
                "an electronic title has no copies on a shelf",
                [RecordId::voebb("SAK1")],
            )
        };
        let mut result = ShowResult::new(None, Engine::Voebb, &[], AvailabilityMode::Fetched);
        result.prepend_notes(vec![same(), same()]);
        assert_eq!(result.notes, vec![same()], "{:?}", result.notes);
    }

    /// Empty notes vanish from the document; a non-empty one must never be swallowed.
    #[test]
    fn notes_are_omitted_when_there_are_none() {
        let mut result = full_result();
        result.notes.clear();
        let json = serde_json::to_string(&result).expect("a SearchResult always serialises");
        assert!(!json.contains("\"notes\""));
        assert!(
            serde_json::to_string(&full_result())
                .expect("a SearchResult always serialises")
                .contains("\"notes\"")
        );
    }

    /// The `show` document that carries every member, for the schema snapshot: a record,
    /// a fetched availability and both shapes of note — one the request implies (a VÖBB
    /// branch on a KOBV record) and one the record implies (a copy on loan).
    fn full_show() -> ShowResult {
        let mut result = ShowResult::new(
            Some(kobv_record()),
            Engine::Kobv,
            &[agb()],
            AvailabilityMode::Fetched,
        );
        // Written out rather than taken from `select::show_at`, so that the snapshot pins
        // the *shape* of the member and stays a test of this module. That the status is
        // the right one is `select`'s own test.
        //
        // `--at AGB` is a VÖBB branch and this is a KOBV record, so the location cannot
        // narrow anything and its status is `unknown` — the `location_other_catalogue`
        // note above says so in words.
        result.at = vec![ShowAt {
            key: "AGB".to_owned(),
            given: "AGB".to_owned(),
            isil: Isil::new("DE-609"),
            branch: Some("SIG00036".to_owned()),
            engine: Engine::Voebb,
            status: Status::Unknown,
        }];
        result
    }

    /// `--at AGB`: a VÖBB branch, which only the voebb engine answers for.
    fn agb() -> Location {
        Location {
            key: "AGB".to_owned(),
            given: "AGB".to_owned(),
            isil: Isil::new("DE-609"),
            branch: Some(BranchRef {
                kobvid: "SIG00036".to_owned(),
                name: "Amerika-Gedenkbibliothek".to_owned(),
            }),
            engine: Engine::Voebb,
            display: "Amerika-Gedenkbibliothek".to_owned(),
        }
    }

    /// The `show` document is a contract exactly as the search document is, and it was
    /// not schema-tested at all until it grew a hull of its own. The snapshot is the
    /// review gate: a diff here is a change to what agents parse.
    #[test]
    fn the_show_document_matches_the_committed_schema_snapshot() {
        let rendered =
            serde_json::to_string_pretty(&full_show()).expect("a ShowResult always serialises");
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/schema/show.json"
        ));
        assert_eq!(
            rendered.trim_end(),
            expected.trim_end(),
            "the JSON schema changed; review the diff before updating \
             tests/fixtures/schema/show.json"
        );
    }

    /// The hull is what `show --no-availability` needs: `record: null` still says whether
    /// anything was asked, so an empty answer is not a bare `null` an agent cannot read.
    #[test]
    fn a_show_without_a_record_still_states_whether_availability_was_asked() {
        let result = ShowResult::new(None, Engine::Kobv, &[], AvailabilityMode::Skipped);
        let json = serde_json::to_value(&result).expect("a ShowResult always serialises");
        assert_eq!(json["record"], serde_json::Value::Null);
        assert_eq!(json["availability"], "skipped");
        // Same convention as the search document: an empty list is left out entirely.
        assert!(json.get("notes").is_none(), "{json}");
    }

    /// A branch in `--at` against a record of the other catalogue is a note, never
    /// silence — and never a claim that the branch does not hold it.
    #[test]
    fn a_location_of_the_other_catalogue_is_stated_as_a_note() {
        let result = ShowResult::new(
            Some(kobv_record()),
            Engine::Kobv,
            &[agb()],
            AvailabilityMode::Fetched,
        );
        let note = result
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::LOCATION_OTHER_CATALOGUE)
            .expect("a voebb branch on a kobv record is stated");
        assert!(note.message.contains("--at AGB"), "{}", note.message);
        assert!(note.message.contains("voebb"), "{}", note.message);
    }

    /// The bug this module used to have: a `kobvindex_` record reached through the union
    /// index can carry the very `DE-609` holding a VÖBB branch would show, with a copy
    /// that even names the branch — so `--at AGB` *did* apply, `mine: true` says so, and
    /// the note must stay silent rather than claim the opposite right next to it.
    #[test]
    fn a_kobvindex_record_with_a_de609_holding_gets_no_other_catalogue_note() {
        let mut record = kobv_record();
        record.id = RecordId::parse("kobvindex_ZLB34296964").expect("a prefixed id parses");
        record.holdings.push(Holding {
            isil: Some(Isil::new("DE-609")),
            alias: Some("VOEBB".to_owned()),
            library: "Berlin VÖBB/ZLB".to_owned(),
            short_name: None,
            local_id: Some("ZLB34296964".to_owned()),
            mine: true,
            summary: Status::Available,
            // Prose holdings: only voebb.de states any.
            holdings_statement: None,
            online_access: None,
            items: vec![Item {
                location: None,
                branch: Some("SIG00036".to_owned()),
                branch_name: Some("Amerika-Gedenkbibliothek".to_owned()),
                call_number: Some("112/000 106 782".to_owned()),
                volume: None,
                status: Status::Available,
                // A return date: only voebb.de states one.
                due_date: None,
                order_option: None,
            }],
        });
        let result = ShowResult::new(
            Some(record),
            Engine::Kobv,
            &[agb()],
            AvailabilityMode::Fetched,
        );
        assert!(
            result
                .notes
                .iter()
                .all(|note| note.kind != note_kinds::LOCATION_OTHER_CATALOGUE),
            "{:?}",
            result.notes
        );
    }

    /// The case the note exists for: a `gbv_` record with no `DE-609` holding at all —
    /// nothing here claims AGB, so the note must still fire.
    #[test]
    fn a_gbv_record_without_a_de609_holding_still_gets_the_note() {
        let mut record = kobv_record();
        record.id = RecordId::parse("gbv_123456789").expect("a prefixed id parses");
        let result = ShowResult::new(
            Some(record),
            Engine::Kobv,
            &[agb()],
            AvailabilityMode::Fetched,
        );
        let note = result
            .notes
            .iter()
            .find(|note| note.kind == note_kinds::LOCATION_OTHER_CATALOGUE)
            .expect("no holding claims AGB, so the note is still owed");
        assert!(note.message.contains("--at AGB"), "{}", note.message);
    }

    /// A location of the record's own catalogue is what `--at` is for, and says nothing.
    #[test]
    fn a_location_of_the_records_own_catalogue_says_nothing() {
        let result = ShowResult::new(
            Some(kobv_record()),
            Engine::Kobv,
            &[Location {
                key: "HU".to_owned(),
                given: "HU".to_owned(),
                isil: Isil::new("DE-11"),
                branch: None,
                engine: Engine::Kobv,
                display: "Humboldt-Universität zu Berlin".to_owned(),
            }],
            AvailabilityMode::Fetched,
        );
        assert!(
            result
                .notes
                .iter()
                .all(|note| note.kind != note_kinds::LOCATION_OTHER_CATALOGUE),
            "{:?}",
            result.notes
        );
    }

    /// The two sentences `show` owes the reader now carry a `kind`, so the JSON states
    /// them as well as the terminal does — a serial's volumes and a missing due date.
    #[test]
    fn a_serial_and_a_copy_on_loan_each_state_their_limitation_once() {
        let mut record = kobv_record();
        record.format = Format::Journal;
        let kinds: Vec<&str> =
            ShowResult::new(Some(record), Engine::Kobv, &[], AvailabilityMode::Fetched)
                .notes
                .iter()
                .map(|note| note.kind)
                .collect();
        assert_eq!(
            kinds,
            vec![
                note_kinds::SERIAL_VOLUMES_UNKNOWN,
                note_kinds::LOAN_WITHOUT_DUE_DATE
            ]
        );
    }

    /// The due-date note is about the copies that have nothing to say, and stops being
    /// said the moment they do. voebb.de states return dates (round 3), and a record whose
    /// every out copy carries one would otherwise be told, right beneath `on loan · back
    /// 2026-09-22`, that no due date is known here.
    #[test]
    fn a_copy_that_states_its_return_date_is_not_told_that_none_is_known() {
        let mut record = kobv_record();
        for holding in &mut record.holdings {
            for item in &mut holding.items {
                if item.status == Status::Unavailable {
                    item.due_date = Some("2026-09-22".to_owned());
                }
            }
        }
        let result = ShowResult::new(Some(record), Engine::Kobv, &[], AvailabilityMode::Fetched);
        assert!(
            result
                .notes
                .iter()
                .all(|note| note.kind != note_kinds::LOAN_WITHOUT_DUE_DATE),
            "{:?}",
            result.notes
        );
    }

    /// And it is still said where a copy is out with nothing to say — `Verloren` and
    /// `Nicht im Regal` are `unavailable` and carry no date, so a record can hold both
    /// kinds at once. That is why the date is per copy and the note is per record.
    #[test]
    fn a_record_with_a_dated_and_an_undated_loan_still_owes_the_note() {
        let mut record = kobv_record();
        let out: Vec<&mut Item> = record
            .holdings
            .iter_mut()
            .flat_map(|holding| &mut holding.items)
            .filter(|item| item.status == Status::Unavailable)
            .collect();
        assert_eq!(out.len(), 1, "the fixture has one copy out");
        // One out with a date, one out without.
        let holding = record.holdings.first_mut().expect("a holding");
        holding.items[1].due_date = Some("2026-09-22".to_owned());
        holding.items[0].status = Status::Unavailable;
        let result = ShowResult::new(Some(record), Engine::Kobv, &[], AvailabilityMode::Fetched);
        assert!(
            result
                .notes
                .iter()
                .any(|note| note.kind == note_kinds::LOAN_WITHOUT_DUE_DATE),
            "{:?}",
            result.notes
        );
    }

    /// The serial note says the volumes "cannot be determined here". Where the record
    /// states its run itself — voebb.de's `Bestand` line — that sentence would be printed
    /// directly above the years it denies, so it is not said at all.
    #[test]
    fn a_serial_that_states_its_run_is_not_told_that_it_is_unknown() {
        let mut record = kobv_record();
        record.format = Format::Journal;
        let bare = ShowResult::new(
            Some(record.clone()),
            Engine::Kobv,
            &[],
            AvailabilityMode::Fetched,
        );
        assert!(
            bare.notes
                .iter()
                .any(|note| note.kind == note_kinds::SERIAL_VOLUMES_UNKNOWN),
            "without a statement the limitation stands: {:?}",
            bare.notes
        );

        record.holdings[0].holdings_statement =
            Some("Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.)".to_owned());
        let stated = ShowResult::new(Some(record), Engine::Kobv, &[], AvailabilityMode::Fetched);
        assert!(
            stated
                .notes
                .iter()
                .all(|note| note.kind != note_kinds::SERIAL_VOLUMES_UNKNOWN),
            "{:?}",
            stated.notes
        );
    }

    /// A book whose every copy is in has nothing to add.
    #[test]
    fn a_book_with_no_copy_out_carries_no_notes() {
        let mut record = kobv_record();
        for holding in &mut record.holdings {
            for item in &mut holding.items {
                item.status = Status::Available;
            }
        }
        let result = ShowResult::new(Some(record), Engine::Kobv, &[], AvailabilityMode::Fetched);
        assert!(result.notes.is_empty(), "{:?}", result.notes);
    }

    /// A holding whose ISIL is not in the library list keeps its display name and is
    /// never dropped — `library` is the one member that is never null.
    #[test]
    fn an_unknown_isil_still_renders_a_library_name() {
        let record = kobv_record();
        let unknown = record
            .holdings
            .last()
            .expect("the fixture record has two holdings");
        assert_eq!(unknown.alias, None);
        assert_eq!(unknown.library, "DE-Zz999");
    }

    /// The window and the fetched size are two different numbers; `plan` is the only
    /// place that relates them.
    #[test]
    fn a_search_request_carries_the_planned_window() {
        let request = SearchRequest {
            query: QuerySpec {
                terms: vec![Term::from_argument("Kafka")],
                ..QuerySpec::default()
            },
            locations: vec![Location {
                key: "HU".to_owned(),
                given: "HU".to_owned(),
                isil: Isil::new("DE-11"),
                branch: None,
                engine: Engine::Kobv,
                display: "Humboldt-Universität zu Berlin".to_owned(),
            }],
            window: FetchWindow::plan(Limit::new(10).expect("10 is in range"), Page::FIRST, false),
        };
        assert_eq!(request.window.start, 1);
        assert_eq!(request.window.size.get(), 10);
    }
}
