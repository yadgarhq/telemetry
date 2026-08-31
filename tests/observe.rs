//! The wrapper's contract, and mainly one property: a call is recorded whatever
//! happens to it — with the RECORD read back, not merely produced.
//!
//! Every test here names the mutation it catches. That is the standard the
//! previous version of this file failed: it ran the code and asserted nothing,
//! so deleting `impl Drop`, making `finish` emit twice, or making `fail` report
//! `OK` all left it green.

mod common;

use std::sync::Arc;

use common::{capture, record_spans, scope, FailingSink};
use yadgar_telemetry::observe::{Call, Outcome, Scope};
use yadgar_telemetry::pb::yadgar::telemetry::v1::Kind;

/// The whole point of the Drop impl: an error path returns via `?`, the Call is
/// dropped, and without this the one outcome telemetry cannot see is FAILURE —
/// the outcome most worth seeing.
///
/// Catches: deleting `impl Drop for Call` entirely. Nothing is emitted and
/// `only()` finds no record.
#[test]
fn a_dropped_call_still_records() {
    let (records, _guard) = capture();

    let call = Call::start("svc", "Tool", Kind::Read, scope());
    drop(call);

    let record = records.only();
    assert_eq!(
        record["outcome"], "UNRECORDED",
        "a dropped Call reports UNRECORDED rather than guessing a status"
    );
    assert_eq!(record["tool"], "Tool");
    assert_eq!(record["request_id"], "req-1");
}

/// ONCE is the assertion. `finish` sets `finished` so `Drop` stays quiet, and
/// without that flag both paths emit — one call, two records, and every metric
/// they feed doubled.
///
/// Catches: `finish` no longer setting `self.finished`. Two records, not one.
#[test]
fn a_finished_call_records_once() {
    let (records, _guard) = capture();

    let call = Call::start("svc", "Tool", Kind::Write, scope());
    call.finish(Outcome {
        status: "OK",
        payload: "some words here".into(),
        rows: 1,
        ..Default::default()
    });

    let lines = records.lines();
    assert_eq!(lines.len(), 1, "one call is one record, got: {lines:?}");
    let record = records.only();
    assert_eq!(record["outcome"], "OK");
    assert_eq!(record["rows_returned"], 1);
}

/// `fail` exists so an error path cannot report OK by reusing a default — the
/// outcome is the one field a metric label depends on being right.
///
/// Catches: `fail` reporting `status: "OK"`. The outcome is read back.
#[test]
fn fail_is_a_distinct_entry_point() {
    let (records, _guard) = capture();

    let call = Call::start("svc", "Tool", Kind::Read, scope());
    call.fail("NOT_FOUND");

    let record = records.only();
    assert_eq!(
        record["outcome"], "NOT_FOUND",
        "the status `fail` was given is the status recorded"
    );
    assert_eq!(
        record["bytes_returned"], 0,
        "an error path returned no payload"
    );
}

/// A span is opened at start, carrying request_id so a trace and its events
/// correlate — and NOT user_id, which belongs on the event only
/// (`src/observe.rs:73-74`).
///
/// Catches: adding `user_id` to the `info_span!`. The field set is asserted
/// EXACTLY, so an added field fails and a deleted span fails too — an
/// absence-only assertion would pass for both.
#[test]
fn a_span_is_opened_and_carries_the_join_key() {
    let (spans, _span_guard) = record_spans();
    let (_records, _sink_guard) = capture();

    let call = Call::start("svc", "Tool", Kind::Read, scope());
    let span = spans.only();
    call.fail("OK");

    assert_eq!(span.name, "rpc");
    assert_eq!(
        span.field_names(),
        vec!["request_id", "service", "tool"],
        "the span carries exactly these three; an unbounded dimension on a span \
         is the same mistake as one on a metric label"
    );
    assert_eq!(
        span.fields["request_id"], "req-1",
        "the join key is what makes a trace and its events correlate"
    );
    assert_eq!(span.fields["service"], "svc");
    assert_eq!(span.fields["tool"], "Tool");
}

/// The event carries what the span must not: user, instance and project are
/// unbounded dimensions, and they have to live somewhere.
#[test]
fn the_unbounded_dimensions_go_on_the_event_not_the_span() {
    let (spans, _span_guard) = record_spans();
    let (records, _sink_guard) = capture();

    Call::start("svc", "Tool", Kind::Read, scope()).fail("INTERNAL");

    let span = spans.only();
    for field in ["user_id", "instance_id", "project_id"] {
        assert!(
            !span.fields.contains_key(field),
            "{field} is unbounded and must not reach a span"
        );
    }

    let record = records.only();
    assert_eq!(record["user_id"], "user-3");
    assert_eq!(record["instance_id"], "inst-2");
    assert_eq!(record["project_id"], "proj-4");
}

/// 449: an error path must be CLASSIFIED, not merely counted. `run` inspects the
/// body's Result before writing the record, which is the difference between
/// "something failed" and "NOT_FOUND happened".
///
/// Catches: replacing the `Err` arm with `Err(_) => {}`. The `Call` is then
/// dropped un-finished and the record reads `UNRECORDED` — the outcome this
/// test's name forbids. `is_err()` alone could not tell the two apart.
#[tokio::test]
async fn run_classifies_an_error_rather_than_recording_unrecorded() {
    let (records, _guard) = capture();

    let out: Result<u32, &str> = Call::start("svc", "Tool", Kind::Read, scope())
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
    let record = records.only();
    assert_eq!(
        record["outcome"], "NOT_FOUND",
        "the classifier's verdict is what reaches the record"
    );
}

#[tokio::test]
async fn run_passes_the_success_value_through() {
    let (records, _guard) = capture();

    let out: Result<u32, &str> = Call::start("svc", "Tool", Kind::Read, scope())
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
    let record = records.only();
    assert_eq!(record["outcome"], "OK");
    assert_eq!(
        record["rows_returned"], 7,
        "`describe` saw the success value"
    );
}

/// 450: bytes come from the wire, words from the rendering. Setting
/// `encoded_bytes` must win over the payload's own length, or the record
/// measures a Debug string instead of what a caller receives.
///
/// Catches: `Builder::encoded_bytes` made a no-op. `bytes_returned` then falls
/// back to the rendering's length instead of the 12 the wire carried.
#[test]
fn encoded_bytes_overrides_the_rendered_length() {
    let (records, _guard) = capture();

    let payload = "a much longer rendered debug string than the wire form";
    let call = Call::start("svc", "Tool", Kind::Read, scope());
    call.finish(Outcome {
        status: "OK",
        payload: payload.into(),
        encoded_bytes: Some(12),
        ..Default::default()
    });

    let record = records.only();
    assert_eq!(
        record["bytes_returned"],
        12,
        "bytes come from the wire, not from a {}-byte Debug rendering",
        payload.len()
    );
    assert_eq!(
        record["words_returned"], 10,
        "words still come from the rendering — the encoded form has none"
    );
}

/// D25, on the path that makes it a process-killer rather than a lost record:
/// `Drop` runs during unwinding, and a panic there aborts. `println!` panics on
/// a closed stdout (`failed printing to stdout: Broken pipe`), so this was live.
///
/// Catches: a sink write error unwrapped or `expect`ed rather than logged.
#[test]
fn a_sink_that_fails_does_not_fail_the_call() {
    let _guard = yadgar_telemetry::record::set_sink(Arc::new(FailingSink));

    // Both paths: the explicit one, and the destructor one.
    Call::start("svc", "Tool", Kind::Read, scope()).fail("INTERNAL");
    drop(Call::start("svc", "Tool", Kind::Read, scope()));
}

/// The override is undone when its guard drops, so one test cannot capture
/// another's records — the reason the sink is thread-scoped rather than global.
#[test]
fn the_sink_override_is_restored_when_its_guard_drops() {
    let outer = {
        let (outer, _guard) = capture();
        {
            let (inner, _guard) = capture();
            Call::start("svc", "Inner", Kind::Read, scope()).fail("OK");
            assert_eq!(inner.lines().len(), 1, "the inner sink took the record");
        }
        Call::start("svc", "Outer", Kind::Read, scope()).fail("OK");
        outer
    };

    let records = outer.records();
    assert_eq!(records.len(), 1, "the outer sink saw only its own record");
    assert_eq!(records[0]["tool"], "Outer");
}

/// An empty payload is not measured: `payload()` is skipped, so no estimate is
/// attached rather than one claiming zero tokens for a thing nobody measured.
#[test]
fn an_error_path_carries_no_estimate() {
    let (records, _guard) = capture();

    Call::start("svc", "Tool", Kind::Read, scope()).finish(Outcome {
        status: "INTERNAL",
        payload: String::new(),
        ..Default::default()
    });

    let record = records.only();
    assert!(
        record.get("tokens_estimated").is_none(),
        "an unmeasured payload has no estimate, not an estimate of zero"
    );
    assert!(record.get("estimator_version").is_none());
}

/// D49's two fields travel together: whether the response was suppressed, and
/// what that saved.
#[test]
fn dedup_records_both_the_flag_and_what_it_saved() {
    let (records, _guard) = capture();

    Call::start("svc", "Tool", Kind::Read, scope()).finish(Outcome {
        status: "OK",
        suppressed: true,
        bytes_suppressed: 4096,
        ..Default::default()
    });

    let record = records.only();
    assert_eq!(record["suppressed_by_dedup"], true);
    assert_eq!(
        record["bytes_suppressed"], 4096,
        "the efficiency number is the saving, not the flag"
    );
}

/// The scope is copied field for field, each to its own column. Four strings of
/// the same type transpose silently, which `src/observe.rs:19-21` names as the
/// reason `Scope` is a struct at all.
#[test]
fn every_scope_field_reaches_its_own_column() {
    let (records, _guard) = capture();

    Call::start(
        "svc",
        "Tool",
        Kind::Read,
        Scope {
            request_id: "R".into(),
            instance_id: "I".into(),
            user_id: "U".into(),
            project_id: "P".into(),
        },
    )
    .fail("OK");

    let record = records.only();
    assert_eq!(record["request_id"], "R");
    assert_eq!(record["instance_id"], "I");
    assert_eq!(record["user_id"], "U");
    assert_eq!(record["project_id"], "P");
}
