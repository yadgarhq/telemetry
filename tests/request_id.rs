//! The request-id cap: D67's join key, bounded identically on every hop.
//!
//! Ported from the two hand-synced copies this crate replaces: `iam`'s
//! `REQUEST_ID_CAP`/`capped` and `iam-db`'s `ACTOR_RECORD_CAP`/`capped`
//! (ledger 1255). A cut that differs by one character, or by the marker, makes
//! two hops' `CallRecord`s stop joining.

use std::borrow::Cow;

use yadgar_telemetry::request_id::{capped, CAP, CUT_MARKER};

/// The numbers both hops and the join depend on.
///
/// Catches: a changed `CAP`, a wrong `CUT_MARKER`, a non-ASCII marker.
#[test]
fn the_bound_and_the_marker_are_the_ones_both_hops_used() {
    assert_eq!(CAP, 256);
    assert_eq!(CUT_MARKER, "...");
}

/// Exactly `CAP` characters is not cut, not marked, and not copied.
///
/// Catches: an off-by-one in `CAP` that cuts at 256 (`nth(CAP - 1)`).
#[test]
fn an_id_of_exactly_the_cap_is_untouched() {
    let id = "a".repeat(CAP);
    let out = capped(&id);
    assert_eq!(out, id);
    assert!(
        matches!(out, Cow::Borrowed(_)),
        "an uncut id must not allocate"
    );
}

/// One character over the cap is cut back to the cap and marked.
///
/// Catches: an off-by-one that lets 257 through (`nth(CAP + 1)`), dropping the
/// marker, and a wrong marker.
#[test]
fn an_id_one_over_the_cap_is_cut_to_the_cap_plus_the_marker() {
    let id = "a".repeat(CAP + 1);
    assert_eq!(capped(&id), format!("{}...", "a".repeat(CAP)));
}

/// A short or empty id passes through unchanged.
#[test]
fn a_short_or_empty_id_is_untouched() {
    assert_eq!(capped(""), "");
    assert_eq!(capped("0192e5a0-uuid"), "0192e5a0-uuid");
}

/// The bound is in CHARACTERS: a multi-byte id is cut on a char boundary,
/// never inside a code point (which would panic on the slice).
///
/// A 3-byte character puts byte 256 inside the 86th character.
///
/// Catches: cutting at byte offset `CAP` instead of char index `CAP`.
#[test]
fn a_multibyte_id_is_cut_on_a_char_boundary() {
    let id = "€".repeat(CAP + 10);
    let out = capped(&id);
    assert_eq!(out, format!("{}...", "€".repeat(CAP)));
    assert_eq!(out.chars().count(), CAP + CUT_MARKER.len());
}

/// A multi-byte id of exactly `CAP` characters is far over `CAP` bytes and
/// must still be left alone.
///
/// Catches: comparing `s.len()` (bytes) to `CAP`.
#[test]
fn a_multibyte_id_of_exactly_the_cap_in_characters_is_untouched() {
    let id = "€".repeat(CAP);
    assert_eq!(capped(&id), id);
}

/// A cut ASCII id is ASCII and every byte is a legal header-value byte, so it
/// survives metadata. `iam-db` reads the header with `to_str()`, which refuses
/// any non-ASCII byte; a `…` marker was once accepted into metadata and read
/// back as `""` (ledger 1248).
///
/// Catches: a non-ASCII `CUT_MARKER`.
#[test]
fn the_cut_value_of_an_ascii_id_is_a_valid_header_value() {
    let cut = capped(&"z".repeat(1000)).into_owned();
    assert!(cut.is_ascii(), "the marker must be ASCII: {cut}");
    assert!(cut.starts_with(&"z".repeat(CAP)));
    assert!(
        cut.bytes().all(|b| (0x20..0x7f).contains(&b)),
        "not a valid header value: {cut}"
    );
}

/// Capping a capped value changes nothing, so a hop that re-caps what an
/// earlier hop already cut still records the identical string.
///
/// Catches: a cut that is not a fixpoint (counting the marker toward the cap,
/// or cutting at `CAP - 1`).
#[test]
fn capping_a_capped_id_changes_nothing() {
    let once = capped(&"z".repeat(1000)).into_owned();
    assert_eq!(capped(&once), once);
}
