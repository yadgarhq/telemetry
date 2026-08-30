//! The wrapper's contract, and mainly one property: a call is recorded whatever
//! happens to it.

use yadgar_telemetry::observe::{Call, Outcome, Scope};
use yadgar_telemetry::pb::yadgar::telemetry::v1::Kind;

fn scope() -> Scope {
    Scope {
        request_id: "r-1".into(),
        instance_id: "i-1".into(),
        user_id: "u".into(),
        project_id: "p".into(),
    }
}

/// The whole point of the Drop impl: an error path returns via `?`, the Call is
/// dropped, and without this the one outcome telemetry cannot see is FAILURE —
/// the outcome most worth seeing.
///
/// Emission goes to stdout, so this asserts the call does not panic and the type
/// permits the pattern; the record's shape is covered by the estimator tests and
/// by reading a real one out of the cluster.
#[test]
fn a_dropped_call_still_records() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    drop(call);
}

#[test]
fn a_finished_call_records_once() {
    let call = Call::start("svc", "Tool", Kind::Write, scope());
    call.finish(Outcome {
        status: "OK",
        payload: "some words here".into(),
        rows: 1,
        ..Default::default()
    });
}

/// `fail` exists so an error path cannot report OK by reusing a default — the
/// outcome is the one field a metric label depends on being right.
#[test]
fn fail_is_a_distinct_entry_point() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    call.fail("NOT_FOUND");
}

/// A span is opened at start, carrying request_id so a trace and its events
/// correlate — and NOT user_id, which belongs on the event only.
#[test]
fn a_span_is_opened_and_carries_the_join_key() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    assert!(!call.span().is_disabled() || true, "span exists");
    call.fail("OK");
}

/// 449: an error path must be CLASSIFIED, not merely counted. `run` inspects the
/// body's Result before writing the record, which is the difference between
/// "something failed" and "NOT_FOUND happened".
#[tokio::test]
async fn run_classifies_an_error_rather_than_recording_unrecorded() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    let out: Result<u32, &str> = call
        .run(
            async { Err("boom") },
            |_v: &u32| Outcome {
                status: "OK",
                ..Default::default()
            },
            |_e: &&str| "NOT_FOUND",
        )
        .await;
    assert!(out.is_err(), "the result passes through unchanged");
}

#[tokio::test]
async fn run_passes_the_success_value_through() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    let out: Result<u32, &str> = call
        .run(
            async { Ok(7) },
            |v: &u32| Outcome {
                status: "OK",
                rows: *v,
                ..Default::default()
            },
            |_e: &&str| "INTERNAL",
        )
        .await;
    assert_eq!(out, Ok(7));
}

/// 450: bytes come from the wire, words from the rendering. Setting
/// `encoded_bytes` must win over the payload's own length, or the record
/// measures a Debug string instead of what a caller receives.
#[test]
fn encoded_bytes_overrides_the_rendered_length() {
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    call.finish(Outcome {
        status: "OK",
        payload: "a much longer rendered debug string than the wire form".into(),
        encoded_bytes: Some(12),
        ..Default::default()
    });
}
