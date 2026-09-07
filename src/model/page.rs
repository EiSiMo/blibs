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
    pub fn new(value: u32) -> Result<Self, UsageError> {
        match u8::try_from(value) {
            Ok(small) if (1..=MAX_SRU_PAGE_SIZE).contains(&small) => Ok(Limit(small)),
            _ => Err(UsageError::LimitOutOfRange { value }),
        }
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
    pub fn new(value: u32) -> Result<Self, UsageError> {
        NonZeroU32::new(value)
            .map(Page)
            .ok_or(UsageError::PageOutOfRange { value })
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
    /// Clamp a desired size into what SRU will actually serve.
    ///
    /// Clamping is correct here and only here: the service truncates at
    /// [`MAX_SRU_PAGE_SIZE`] anyway and says nothing about it, so the type does the
    /// truncation openly instead of leaving a lie in the request. [`Limit`], which is a
    /// promise to the user rather than a request parameter, is never clamped.
    pub fn new(desired: u32) -> Self {
        match u8::try_from(desired) {
            Ok(size) if size <= MAX_SRU_PAGE_SIZE => SruPageSize(size),
            _ => SruPageSize(MAX_SRU_PAGE_SIZE),
        }
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
    pub fn plan(limit: Limit, page: Page, filtered: bool) -> Self {
        let stride = if filtered {
            MAX_SRU_PAGE_SIZE
        } else {
            limit.get()
        };
        // Saturating throughout: `--page 4294967295` is a legal page number, and an
        // overflow here would silently wrap round to page one.
        let start = page
            .get()
            .saturating_sub(1)
            .saturating_mul(u32::from(stride))
            .saturating_add(1);
        Self {
            start,
            size: SruPageSize::new(u32::from(stride)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_inside_the_range_is_taken_as_given() {
        assert_eq!(Limit::new(1).expect("1 is in range").get(), 1);
        assert_eq!(Limit::new(10).expect("10 is in range").get(), 10);
        assert_eq!(Limit::new(50).expect("50 is in range").get(), 50);
        assert_eq!(Limit::default(), Limit::DEFAULT);
    }

    /// Never clamped: a clamped `--limit 200` would look like it worked.
    #[test]
    fn a_limit_outside_the_range_is_rejected_not_clamped() {
        for value in [0, 51, 200, u32::MAX] {
            let error = Limit::new(value).expect_err("outside 1..=50");
            assert!(
                matches!(error, UsageError::LimitOutOfRange { value: v } if v == value),
                "{value} should be reported verbatim"
            );
        }
    }

    #[test]
    fn pages_are_one_based() {
        assert_eq!(Page::new(1).expect("1 is a page").get(), 1);
        assert_eq!(Page::new(7).expect("7 is a page").get(), 7);
        assert_eq!(Page::default(), Page::FIRST);
        assert!(matches!(
            Page::new(0).expect_err("there is no page zero"),
            UsageError::PageOutOfRange { value: 0 }
        ));
    }

    /// The other half of the pair: here clamping is the honest thing, because the service
    /// truncates at 50 without saying so.
    #[test]
    fn the_sru_page_size_clamps_at_fifty() {
        assert_eq!(SruPageSize::new(0).get(), 0);
        assert_eq!(SruPageSize::new(10).get(), 10);
        assert_eq!(SruPageSize::new(50).get(), 50);
        assert_eq!(SruPageSize::new(51).get(), MAX_SRU_PAGE_SIZE);
        assert_eq!(SruPageSize::new(1000).get(), MAX_SRU_PAGE_SIZE);
        assert_eq!(SruPageSize::new(u32::MAX).get(), MAX_SRU_PAGE_SIZE);
    }

    #[test]
    fn an_unfiltered_window_is_exactly_the_limit() {
        let limit = Limit::new(20).expect("20 is in range");
        let window = FetchWindow::plan(limit, Page::FIRST, false);
        assert_eq!(window.start, 1);
        assert_eq!(window.size.get(), 20);
    }

    #[test]
    fn an_unfiltered_window_steps_by_the_limit() {
        let limit = Limit::new(20).expect("20 is in range");
        let page = Page::new(3).expect("3 is a page");
        let window = FetchWindow::plan(limit, page, false);
        assert_eq!(window.start, 41);
        assert_eq!(window.size.get(), 20);
    }

    /// With `--format`/`--language` the filter only ever sees the fetched records, so the
    /// window is widened to the largest one SRU serves — and the pages step by 50 too.
    #[test]
    fn a_filtered_window_is_widened_to_the_maximum() {
        let limit = Limit::new(10).expect("10 is in range");
        let first = FetchWindow::plan(limit, Page::FIRST, true);
        assert_eq!(first.start, 1);
        assert_eq!(first.size.get(), MAX_SRU_PAGE_SIZE);

        let page = Page::new(2).expect("2 is a page");
        let second = FetchWindow::plan(limit, page, true);
        assert_eq!(second.start, 51);
        assert_eq!(second.size.get(), MAX_SRU_PAGE_SIZE);
    }

    /// A legal but absurd page number must not wrap round to page one.
    #[test]
    fn a_huge_page_saturates_instead_of_wrapping() {
        let limit = Limit::new(50).expect("50 is in range");
        let page = Page::new(u32::MAX).expect("u32::MAX is a page");
        let window = FetchWindow::plan(limit, page, false);
        assert_eq!(window.start, u32::MAX);
    }
}
