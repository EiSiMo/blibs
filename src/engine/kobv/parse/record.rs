//! MARC to [`Record`].
//!
//! One function per output field, each independently testable against a fixture. Which
//! MARC field fills which output field is settled in `plan/marc-mapping.md` and is not
//! re-derived here.
//!
//! The `924` rules are the ones with teeth: subfields are always `$a$b$c$d`, `$d` is
//! ignored, `$b` is `ISIL;LocalId`, an ISIL may occur several times and is deduplicated,
//! and a record with no `924` at all — 4.9 % of them — yields an **empty** holdings list
//! that means "not stated in this record", never "held nowhere". `852` also occurs in SRU
//! records and is deliberately not used.

use crate::error::Error;
use crate::model::{AvailabilityId, Holding, Record};

use super::marc::MarcRecord;

/// Build a [`Record`] from a MARC record.
///
/// Never panics on a malformed record: a missing title is an error naming the field, and
/// every optional field degrades to `None`.
pub fn from_marc(_marc: &MarcRecord) -> Result<Record, Error> {
    todo!("phase 2: record parse")
}

/// Assemble the availability key from the holdings.
///
/// `ISIL;LocalId` pairs joined by commas and terminated by one, exactly as the portal
/// writes it into `data-availability-id`. `None` when the record has no holdings — there
/// is then nothing to ask about, and the call is skipped rather than sent empty.
pub fn availability_id(_holdings: &[Holding]) -> Option<AvailabilityId> {
    todo!("phase 2: record parse")
}
