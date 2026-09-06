//! End-to-end tests of the voebb.de engine against saved fixtures.
//!
//! Filled in phase 5.4. What has to be proven here, from `plan/voebb.md`:
//!
//! - `requestCount` increases with every step of the session;
//! - a `/noaccess` page is reported as a lost session (exit 6) and never as "no hits";
//! - the branch facet narrows the result, and the documented fallback (client-side sieve
//!   plus a note) is taken when it cannot be applied;
//! - the detail page is fetched without a session.

mod common;
