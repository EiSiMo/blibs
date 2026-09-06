//! Terminal output: one readable line per hit.
//!
//! Rules that the example output in `plan/cli.md` pins down character for character:
//!
//! - **With `--at`, output is grouped by location** — one block per location, one line
//!   per record, that location's copies beneath it. No flag turns this off. Without
//!   `--at` it is one flat line per hit.
//! - **The legend lists only symbols that actually occur** in this output.
//! - **A record id is never shortened**; titles are.
//! - `no holdings recorded in this record` for a record with no `924` — not an empty
//!   list, and not "held nowhere".
//! - For a serial, a line saying that holdings runs cannot be determined: the service
//!   gives one traffic light for the *title*, with no volume, and a green light would
//!   otherwise read as a statement about the year the user wants.
//! - When a client-side filter was in play, a line saying how large the window was — so
//!   that "nothing found" is never mistaken for "nothing exists".

use std::io::Write;

use crate::error::{EmptyReason, Error};
use crate::libraries::Library;
use crate::model::{Record, SearchResult};
use crate::render::Style;

/// Render a search result.
pub fn search(_out: &mut impl Write, _result: &SearchResult, _style: Style) -> std::io::Result<()> {
    todo!("phase 3: render human")
}

/// Render one record in full.
pub fn show(_out: &mut impl Write, _record: &Record, _style: Style) -> std::io::Result<()> {
    todo!("phase 3: render human")
}

/// Render the library table, or one library in detail.
pub fn libraries(
    _out: &mut impl Write,
    _libraries: &[&Library],
    _style: Style,
) -> std::io::Result<()> {
    todo!("phase 3: render human")
}

/// Say why there is nothing to show. Never a bare "no results": the reason decides what
/// the user should try next.
pub fn empty(_out: &mut impl Write, _reason: &EmptyReason, _style: Style) -> std::io::Result<()> {
    todo!("phase 3: render human")
}

/// Render an error and its hint to stderr.
pub fn error(_out: &mut impl Write, _error: &Error, _style: Style) -> std::io::Result<()> {
    todo!("phase 3: render human")
}
