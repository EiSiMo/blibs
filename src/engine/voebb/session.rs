//! The aDISWeb session, as a value.
//!
//! Every request to voebb.de carries the complete form state of the page it came from:
//! the action URL, nine hidden fields, and a `requestCount` that must be incremented on
//! every step. Getting any of them wrong produces a `/noaccess` page with status 200 —
//! which is why the state is a value that is threaded through the client rather than
//! something reassembled at each call site.

/// The form state of one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The form's action URL for the next request.
    pub action: String,
    /// The hidden fields, in document order, name and value.
    pub hidden: Vec<(String, String)>,
    /// The step counter. Incremented for every request; a stale value is rejected.
    pub request_count: u32,
}

impl Session {
    /// Read the session out of a page.
    ///
    /// A missing hidden field is an error naming it — carrying on with eight of nine
    /// would produce a `/noaccess` page two requests later, where the cause is no longer
    /// visible.
    pub fn from_html(_html: &str) -> Result<Self, crate::error::Error> {
        todo!("phase 5: voebb session")
    }

    /// The form fields for the next request, with `requestCount` advanced.
    pub fn next_form(&self) -> Vec<(String, String)> {
        todo!("phase 5: voebb session")
    }
}
