//! `status_name`, the mapping ~61 services share.
//!
//! It had no tests at all. A wrong label is not a crash and not a wrong
//! response — it is a dashboard that groups by outcome and quietly reports the
//! wrong thing, in every service at once, which is precisely the failure the
//! function was centralised to prevent.

#![cfg(feature = "grpc")]

use std::collections::BTreeSet;

use tonic::{Code, Status};
use yadgar_telemetry::grpc::status_name;

/// Every code the function names, and the label it must produce.
///
/// Written out rather than derived from the implementation: a table that reads
/// the code under test agrees with it by construction and asserts nothing.
///
/// Catches: any single arm being rewired, `NotFound => "OK"` included.
const LISTED: &[(Code, &str)] = &[
    (Code::Ok, "OK"),
    (Code::Cancelled, "CANCELLED"),
    (Code::InvalidArgument, "INVALID_ARGUMENT"),
    (Code::DeadlineExceeded, "DEADLINE_EXCEEDED"),
    (Code::NotFound, "NOT_FOUND"),
    (Code::AlreadyExists, "ALREADY_EXISTS"),
    (Code::PermissionDenied, "PERMISSION_DENIED"),
    (Code::ResourceExhausted, "RESOURCE_EXHAUSTED"),
    (Code::FailedPrecondition, "FAILED_PRECONDITION"),
    (Code::Aborted, "ABORTED"),
    (Code::Unimplemented, "UNIMPLEMENTED"),
    (Code::Internal, "INTERNAL"),
    (Code::Unavailable, "UNAVAILABLE"),
    (Code::Unauthenticated, "UNAUTHENTICATED"),
];

/// Codes the function does NOT list. `Unknown` is the obvious one; `OutOfRange`
/// and `DataLoss` are real gRPC codes this mapping deliberately does not name,
/// which is what makes the collapse claim testable rather than tautological.
const UNLISTED: &[Code] = &[Code::Unknown, Code::OutOfRange, Code::DataLoss];

#[test]
fn every_listed_code_maps_to_its_own_name() {
    for &(code, expected) in LISTED {
        assert_eq!(
            status_name(&Status::new(code, "")),
            expected,
            "{code:?} must report {expected}"
        );
    }
}

/// Two codes sharing a label make a dashboard sum unlike outcomes together, and
/// a per-code equality check alone would not notice a label used twice.
#[test]
fn no_two_listed_codes_share_a_label() {
    let names: BTreeSet<&str> = LISTED.iter().map(|(_, name)| *name).collect();
    assert_eq!(
        names.len(),
        LISTED.len(),
        "each listed code needs a label of its own"
    );
    assert!(
        !names.contains("UNKNOWN"),
        "UNKNOWN is the fallback; a listed code reaching it would be invisible"
    );
}

/// The documented claim: unlisted codes COLLAPSE rather than pass through, so a
/// future tonic release cannot widen the label set without this file changing.
///
/// Catches: a fallthrough that forwards the code's own description instead.
#[test]
fn an_unlisted_code_collapses_to_unknown() {
    for &code in UNLISTED {
        assert_eq!(
            status_name(&Status::new(code, "")),
            "UNKNOWN",
            "{code:?} is not named by the mapping and must collapse"
        );
    }
}

/// **Bounded is the whole point.** The code is an enum; the MESSAGE is
/// caller-influenced text, and letting it reach a label is unbounded
/// cardinality — one time series per distinct error string.
#[test]
fn the_caller_influenced_message_never_reaches_the_label() {
    let label = status_name(&Status::not_found(
        "no such project: <anything a caller typed>",
    ));
    assert_eq!(label, "NOT_FOUND");

    let a = status_name(&Status::internal("one message"));
    let b = status_name(&Status::internal("an entirely different message"));
    assert_eq!(a, b, "the same code is the same label whatever it says");
}

/// The vocabulary is gRPC's own spelling. A label that drifted to `NotFound`
/// would split one outcome across two series at whichever release changed it.
#[test]
fn the_label_set_is_a_fixed_vocabulary() {
    let mut names: Vec<&str> = LISTED.iter().map(|(_, name)| *name).collect();
    names.push("UNKNOWN");
    for name in names {
        assert!(
            name.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
            "{name} is not spelled as a gRPC status name"
        );
    }
}
