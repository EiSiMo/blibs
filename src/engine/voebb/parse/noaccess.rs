//! The `/noaccess` check.
//!
//! aDISWeb answers a broken form with **HTTP 200** and a page that says the session is
//! gone. Read as a result list it looks exactly like "no hits" — which is why this runs
//! before every other parser and turns it into
//! [`crate::error::UnexpectedError::VoebbNoAccess`], exit 6.

use crate::error::Error;

/// Fail if this page is the session-lost page.
///
/// `step` names the request that produced it and goes into the message, because "the
/// session was lost" is only actionable if it says where.
pub fn check(_html: &str, _step: &str) -> Result<(), Error> {
    todo!("phase 5: voebb parse")
}
