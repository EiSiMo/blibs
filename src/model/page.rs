//! Paging: how many records the user asked for, and how many are actually fetched.
//!
//! These are three different numbers and conflating them has already cost a debugging
//! session. [`Limit`] is what the user wants to *see*. [`SruPageSize`] is what SRU can be
//! asked for — it caps at 50 and truncates **silently** above that, which is why it is a
//! type and not a raw integer. [`FetchWindow`] is the window that is actually requested,
//! which is larger than the limit when a client-side filter will thin it out.

use std::num::NonZeroU32;

use crate::error::UsageError;

/// The largest window SRU will serve. Above this it truncates without saying so.
pub const MAX_SRU_PAGE_SIZE: u8 = 50;

/// How many records to display, 1..=50.
///
/// Validated in `cli` and **never clamped**: a clamped `--limit 200` would look like it
/// worked and quietly show 50.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct Limit(u8);

impl Limit {
    /// The default when `--limit` is not given.
    pub const DEFAULT: Limit = Limit(10);

    /// Validate a user-supplied limit.
    pub fn new(_value: u32) -> Result<Self, UsageError> {
        todo!("phase 1: model")
    }

    /// The value.
    pub fn get(self) -> u8 {
        self.0
    }
}

impl Default for Limit {
    fn default() -> Self {
        Limit::DEFAULT
    }
}

/// A 1-based page number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct Page(NonZeroU32);

impl Page {
    /// The first page.
    pub const FIRST: Page = Page(NonZeroU32::MIN);

    /// Validate a user-supplied page number.
    pub fn new(_value: u32) -> Result<Self, UsageError> {
        todo!("phase 1: model")
    }

    /// The value.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl Default for Page {
    fn default() -> Self {
        Page::FIRST
    }
}

/// The `maximumRecords` value for one SRU request.
///
/// The **only** way to reach that parameter. Unlike [`Limit`] this one *does* clamp, at
/// [`MAX_SRU_PAGE_SIZE`] — because here clamping matches what the service does anyway,
/// and the type makes that fact impossible to forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SruPageSize(u8);

impl SruPageSize {
    /// A page size of zero — the counting request, which returns `numberOfRecords` and no
    /// records at all.
    pub const COUNT_ONLY: SruPageSize = SruPageSize(0);

    /// Clamp a desired size into what SRU will actually serve.
    pub fn new(_desired: u32) -> Self {
        todo!("phase 1: model")
    }

    /// The value for `maximumRecords`.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// The record window one search request asks for.
///
/// `start` is the 1-based `startRecord`; `size` is `maximumRecords`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchWindow {
    /// 1-based position of the first record to fetch.
    pub start: u32,
    /// How many records to fetch.
    pub size: SruPageSize,
}

impl FetchWindow {
    /// Work out the window for a given limit and page.
    ///
    /// Without a client-side filter the window is exactly the limit: the user sees what
    /// was fetched, and the tool does not pay for records it will throw away. With
    /// `--format` or `--language` the window is widened to [`MAX_SRU_PAGE_SIZE`], because
    /// those filters see only the fetched records and a window of 10 would report "no
    /// hits" for a book that is on record 11. The output must always say which of the two
    /// happened; it may never imply that a filtered result is complete.
    pub fn plan(_limit: Limit, _page: Page, _filtered: bool) -> Self {
        todo!("phase 1: model")
    }
}
