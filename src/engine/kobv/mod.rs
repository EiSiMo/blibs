//! The KOBV union catalogue: 46.7 million records over ~88 institutions.
//!
//! Two services, one engine. The SRU endpoint answers the search — always as PQF via
//! `x-pquery`, never as CQL, because PQF is the only way to reach Bib-1 attribute `1044`
//! (ISIL) and thus to filter by holdings *upstream*. The portal's availability service
//! answers "is it in right now", one call per record.
//!
//! The traps this engine has to survive are listed in `CLAUDE.md`; the two that shape the
//! code most are that **HTTP 200 means nothing here** — diagnostics, silent truncation
//! and phantom records all arrive with status 200 — and that availability **must not be
//! batched**, because the response is keyed by ISIL and two records' keys collide.

pub mod client;
pub mod parse;
pub mod pqf;

use crate::error::{Error, UnexpectedError};
use crate::http::{Fetch, scope_map};
use crate::libraries;
use crate::model::{
    AtBlock, AvailabilityId, AvailabilityMode, Catalog, Engine, EngineSearch, FetchWindow, Holding,
    Isil, Location, Note, QuerySpec, Record, RecordId, SearchRequest, SruPageSize,
};

use client::KobvClient;
use parse::sru::{RecordPayload, SruResponse};
use parse::{availability, record, sru};

/// What names the document an error message is about.
const CONTEXT: &str = "the SRU response";

/// The KOBV engine.
pub struct Kobv<'f> {
    client: KobvClient<'f>,
}

impl<'f> Kobv<'f> {
    /// Build the engine over a transport.
    pub fn new(fetch: &'f dyn Fetch) -> Self {
        Self {
            client: KobvClient::new(fetch),
        }
    }

    /// The transport this engine was built with.
    pub fn fetch(&self) -> &'f dyn Fetch {
        self.client.fetch()
    }

    /// One `at[]` entry per location, in the order the user gave them.
    ///
    /// The totals are the point of the entries: "HU Berlin · 230 results" has to be the
    /// number of HU records, not the size of the joint search. Without `want_totals` the
    /// locations are still listed, with `total: None` — a location the user named must
    /// appear even when its count was not paid for, because a missing block cannot be
    /// told apart from a forgotten one.
    ///
    /// `records` is the membership of the block, and here it is simply which of the
    /// fetched records carry a `924` for that ISIL — the same question the renderer used
    /// to ask of every holding. Stating it costs nothing and is what lets the grouping be
    /// read off the JSON for both engines alike.
    fn at_blocks(
        &self,
        request: &SearchRequest,
        records: &[Record],
    ) -> Result<Vec<AtBlock>, Error> {
        if request.locations.is_empty() {
            return Ok(Vec::new());
        }
        let totals = if request.want_totals {
            self.totals(&request.query, &request.locations)?
        } else {
            vec![None; request.locations.len()]
        };
        Ok(request
            .locations
            .iter()
            .zip(totals)
            .map(|(location, total)| AtBlock {
                key: location.key.clone(),
                isil: location.isil.clone(),
                branch: None,
                engine: Engine::Kobv,
                total,
                records: held_at(records, &location.isil),
            })
            .collect())
    }

    /// One counting request per location, up to six at a time.
    ///
    /// A failed count fails the search: a heading that silently omits one location's
    /// number, or shows a stale one, is exactly the kind of half-answer this tool must
    /// not give.
    fn totals(&self, spec: &QuerySpec, locations: &[Location]) -> Result<Vec<Option<u64>>, Error> {
        let isils: Vec<Isil> = locations
            .iter()
            .map(|location| location.isil.clone())
            .collect();
        let counts = scope_map(isils, |isil| {
            let query = pqf::count_for(spec, &isil)?;
            self.client.count(&query)
        })?;
        Ok(counts.into_iter().map(Some).collect())
    }
}

impl Catalog for Kobv<'_> {
    fn engine(&self) -> Engine {
        Engine::Kobv
    }

    /// One search request, plus one counting request per `--at` location — the counts
    /// among themselves concurrently, after the search. The counting requests use
    /// `maximumRecords=0` and exist so that `at[].total` is the location's real hit count
    /// and not the size of the window.
    ///
    /// `--at` is part of the query, not a sieve over the answer: the ISILs go out as
    /// `@or @attr 1=1044 …`, so `total` and the paging both apply to the filtered result.
    fn search(&self, request: &SearchRequest) -> Result<EngineSearch, Error> {
        let query = pqf::search(&request.query, &filter_isils(&request.locations))?;
        let response = self.client.search(&query, request.window)?;
        let records = records(&response)?;
        Ok(EngineSearch {
            engine: Engine::Kobv,
            total: Some(response.number_of_records),
            fetched: records.len(),
            undelivered: undelivered(&response),
            at: self.at_blocks(request, &records)?,
            records,
            query_echo: Some(query.as_str().to_owned()),
            notes: sru::record_notes(&response),
        })
    }

    /// One availability call per record, concurrently, capped at six. Records with no
    /// MARC `924` are skipped rather than asked about — there is no key to ask with.
    ///
    /// A failed call fails the whole fill: the copies of the remaining records would be
    /// missing from an answer that still looked complete, and "not on the shelf" and "not
    /// asked" are indistinguishable to whoever reads it.
    fn fill_availability(&self, records: &mut [Record]) -> Result<Vec<Note>, Error> {
        let asked: Vec<(usize, AvailabilityId)> = records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| Some((index, record::availability_id(&record.holdings)?)))
            .collect();
        let answers = scope_map(asked, |(index, id)| {
            Ok((index, self.client.availability(&id)?))
        })?;

        let mut notes = Vec::new();
        for (index, answer) in answers {
            // The indices were taken from this very slice a moment ago, so this cannot
            // miss; `get_mut` says so without a panicking path through network data.
            let Some(record) = records.get_mut(index) else {
                continue;
            };
            notes.extend(availability::merge(&answer, &mut record.holdings));
            name_libraries(&mut record.holdings);
        }
        Ok(notes)
    }

    /// Look one record up by its id, and — unless availability was waived — ask for its
    /// copies.
    ///
    /// `Ok(None)` means the catalogue has no such record, which is exit 1 and not an
    /// error. It is returned only when the service delivered nothing *and* announced
    /// nothing: an announced record that arrives as a surrogate diagnostic is a failure
    /// of the response, not an absent record, and saying "no such record" for it would be
    /// a wrong answer rather than a missing one.
    fn show(&self, id: &RecordId, mode: AvailabilityMode) -> Result<Option<Record>, Error> {
        let query = pqf::record_lookup(id);
        let window = FetchWindow {
            start: 1,
            size: SruPageSize::new(1),
        };
        let response = self.client.search(&query, window)?;
        if response.records.is_empty() {
            return if response.number_of_records == 0 {
                Ok(None)
            } else {
                Err(missing(&format!(
                    "the record {id}, which the response counted but did not deliver"
                )))
            };
        }
        let Some(mut record) = records(&response)?.into_iter().next() else {
            return Err(missing(&format!("a readable MARC record for {id}")));
        };
        if mode == AvailabilityMode::Fetched {
            self.fill_availability(std::slice::from_mut(&mut record))?;
        }
        Ok(Some(record))
    }
}

/// The ids of the records that state a holding of this ISIL.
///
/// A record with no `924` at all belongs to no location — 4.9 % have none, and that is
/// "not stated in this record", never "held everywhere".
fn held_at(records: &[Record], isil: &Isil) -> Vec<RecordId> {
    records
        .iter()
        .filter(|record| {
            record
                .holdings
                .iter()
                .any(|holding| holding.isil.as_ref() == Some(isil))
        })
        .map(|record| record.id.clone())
        .collect()
}

/// The ISILs of the locations this engine answers for, deduplicated, in the user's order.
///
/// Deduplicated because `@or @attr 1=1044 DE-11 @attr 1=1044 DE-11` is a longer way of
/// writing the same query, and the query is echoed into the JSON where a doubled clause
/// would read as a mistake. The spelling is the canonical one from the library list, put
/// there by `libraries::resolve`: the attribute is case-sensitive and `de-11` matches
/// nothing, silently.
fn filter_isils(locations: &[Location]) -> Vec<Isil> {
    let mut isils: Vec<Isil> = Vec::with_capacity(locations.len());
    for location in locations {
        if !isils.contains(&location.isil) {
            isils.push(location.isil.clone());
        }
    }
    isils
}

/// The delivered MARC records, converted and named.
///
/// A record that is not MARC is not an error here — it is counted by [`undelivered`] and
/// explained by [`sru::record_notes`]. A MARC record that cannot be converted *is* one:
/// the only conversion failure is a missing identity, and a record without an id can
/// neither be shown again nor asked about.
fn records(response: &SruResponse) -> Result<Vec<Record>, Error> {
    response
        .records
        .iter()
        .filter_map(|delivered| match &delivered.payload {
            RecordPayload::Marc(marc) => Some(marc),
            RecordPayload::Diagnostic(_) | RecordPayload::Unknown { .. } => None,
        })
        .map(|marc| {
            let mut record = record::from_marc(marc)?;
            name_libraries(&mut record.holdings);
            Ok(record)
        })
        .collect()
}

/// How many slots of the result list arrived empty — a surrogate diagnostic or a schema
/// this tool cannot read. Never the shortfall against `maximumRecords`, which is normal.
fn undelivered(response: &SruResponse) -> usize {
    response
        .records
        .iter()
        .filter(|delivered| !matches!(delivered.payload, RecordPayload::Marc(_)))
        .count()
}

/// Add the library list's names to every holding that carries an ISIL.
///
/// This is the *only* thing the list contributes: nothing branches on a specific ISIL, and
/// a code the list does not know keeps the bare ISIL that `parse` put there — an
/// institution that joined the network yesterday must still have its holding shown.
///
/// `library` is the full official name, `short_name` the short one; that split is fixed by
/// the JSON contract in `plan/cli.md`, where `library` reads "Humboldt-Universität zu
/// Berlin, Universitätsbibliothek" next to a `short_name` of "HU Berlin".
fn name_libraries(holdings: &mut [Holding]) {
    for holding in holdings {
        libraries::name_holding(holding);
    }
}

/// A structure the catalogue must have delivered and did not.
fn missing(what: &str) -> Error {
    UnexpectedError::MissingElement {
        what: what.to_owned(),
        context: CONTEXT.to_owned(),
    }
    .into()
}
