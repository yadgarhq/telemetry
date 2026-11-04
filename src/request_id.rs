//! D67's join key, bounded the same way on every hop.
//!
//! `request_id` crosses hops, and the records on each side must carry the SAME
//! string or a roll-up stops joining them. A caller's id is read before anything
//! validates it, so each hop bounds it. This is that bound, written once: `iam`
//! and `iam-db` each held a hand-synced copy, and a copy that drifts by one
//! character cuts one over-long id two ways (ledger 1255).
//!
//! **Cap, never refuse** (D25). A correlation id must not fail a call.

use std::borrow::Cow;

/// How many of a caller's characters a request id may put on a record.
///
/// A cut value is at most `CAP + CUT_MARKER.len()` characters, the marker
/// included.
pub const CAP: usize = 256;

/// What marks a cut. **ASCII, and that is load-bearing** (ledger 1248): the
/// capped id is forwarded in a header, and a reader that calls `to_str()` on it
/// refuses any non-ASCII byte. A `…` here was accepted into metadata and read
/// back as `""`.
pub const CUT_MARKER: &str = "...";

/// `s` cut to [`CAP`] characters, with [`CUT_MARKER`] appended if it was cut.
///
/// The cut is on a character boundary, never inside a code point. An id of
/// [`CAP`] characters or fewer is returned borrowed and unchanged. Capping a
/// capped value changes nothing, so two hops that both cap still agree.
pub fn capped(s: &str) -> Cow<'_, str> {
    match s.char_indices().nth(CAP) {
        Some((at, _)) => format!("{}{CUT_MARKER}", &s[..at]).into(),
        None => s.into(),
    }
}
