//! A worker pool over borrowed data.
//!
//! `std::thread::scope` rather than an async runtime or a thread-pool crate: the tool
//! makes at most a few dozen requests per invocation, they are all I/O-bound, and a
//! scoped pool can borrow `&dyn Fetch` without an `Arc`. The argument is in
//! `plan/client.md`.

use crate::error::Error;

/// Map `f` over `items` on up to [`crate::http::limit::MAX_IN_FLIGHT_PER_HOST`] threads,
/// preserving input order.
///
/// The first error wins and the remaining work is abandoned: one failed availability
/// call means the answer is incomplete, and an incomplete answer that looks complete is
/// the failure mode this crate exists to avoid.
pub fn scope_map<T, U, F>(_items: Vec<T>, _f: F) -> Result<Vec<U>, Error>
where
    T: Send,
    U: Send,
    F: Fn(T) -> Result<U, Error> + Sync,
{
    todo!("phase 2: http")
}
