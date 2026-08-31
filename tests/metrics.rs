//! The bounded half of D67 — the half a dashboard queries by NAME.
//!
//! Two things were unpinned here. The metric name strings are an interface to
//! something outside this repository: rename one and every panel goes blank
//! while the code stays correct and the suite stays green. And the bytes
//! counter is fed `built.bytes_returned`, not the payload's length — the whole
//! reason `encoded_bytes` exists, and nothing checked that the metric got the
//! same number the record did.
//!
//! `DebuggingRecorder` is installed thread-locally, the same shape as this
//! crate's sink override and `tracing`'s default subscriber.

mod common;

use std::collections::BTreeMap;

use common::{capture, scope};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use yadgar_telemetry::metrics::{BYTES, BYTES_SUPPRESSED, CALLS, DURATION};
use yadgar_telemetry::observe::{Call, Outcome};
use yadgar_telemetry::pb::yadgar::telemetry::v1::Kind;

/// One emitted metric: its name, its labels, and its value.
#[derive(Debug)]
struct Emitted {
    labels: BTreeMap<String, String>,
    value: DebugValue,
}

impl Emitted {
    fn label_names(&self) -> Vec<&str> {
        self.labels.keys().map(String::as_str).collect()
    }

    fn counter(&self) -> u64 {
        match self.value {
            DebugValue::Counter(n) => n,
            ref other => panic!("expected a counter, got {other:?}"),
        }
    }

    fn histogram(&self) -> Vec<f64> {
        match self.value {
            DebugValue::Histogram(ref values) => values.iter().map(|v| **v).collect(),
            ref other => panic!("expected a histogram, got {other:?}"),
        }
    }
}

/// Run `f` against a recorder and collect what it emitted, keyed by name.
fn emitted_by(f: impl FnOnce()) -> BTreeMap<String, Emitted> {
    let recorder = DebuggingRecorder::new();
    let snapshotter: Snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, f);

    let mut by_name = BTreeMap::new();
    for (composite, _unit, _description, value) in snapshotter.snapshot().into_vec() {
        let key = composite.key();
        let labels = key
            .labels()
            .map(|l| (l.key().to_string(), l.value().to_string()))
            .collect();
        by_name.insert(key.name().to_string(), Emitted { labels, value });
    }
    // A metrics-util built against another `metrics` major links a SECOND
    // facade: everything compiles, nothing is captured, and every assertion
    // below would pass vacuously against an empty map.
    assert!(
        !by_name.is_empty(),
        "the recorder captured nothing — check for a duplicate `metrics` crate"
    );
    by_name
}

/// One finished call, with a rendering far longer than the wire form.
fn one_call() -> BTreeMap<String, Emitted> {
    emitted_by(|| {
        let (_records, _guard) = capture();
        Call::start("svc", "Tool", Kind::Read, scope()).finish(Outcome {
            status: "OK",
            payload: "a much longer rendered debug string than the wire form".into(),
            encoded_bytes: Some(12),
            ..Default::default()
        });
    })
}

/// The names are an interface to Grafana, not an implementation detail. A
/// rename is a silent outage of every panel that queries them.
#[test]
fn the_metric_names_are_the_strings_dashboards_query() {
    assert_eq!(CALLS, "yadgar_calls_total");
    assert_eq!(DURATION, "yadgar_call_duration_seconds");
    assert_eq!(BYTES, "yadgar_bytes_returned_total");
    assert_eq!(BYTES_SUPPRESSED, "yadgar_bytes_suppressed_total");
}

/// …and the constants are what a call actually emits under.
#[test]
fn a_call_emits_under_exactly_those_names() {
    let emitted = one_call();
    let mut names: Vec<&str> = emitted.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "yadgar_bytes_returned_total",
            "yadgar_call_duration_seconds",
            "yadgar_calls_total",
        ],
        "a call with nothing suppressed emits these three and no more"
    );
}

/// **The cardinality rule, as an assertion.** `tool`, `service` and `outcome`
/// are bounded; `user_id`, `instance_id` and `project_id` are not, and one of
/// them here is one time series per user per tool.
///
/// Catches: adding any unbounded label to `metrics::record`.
#[test]
fn only_bounded_labels_reach_a_metric() {
    let emitted = one_call();

    assert_eq!(
        emitted[CALLS].label_names(),
        vec!["outcome", "service", "tool"],
        "the outcome is a bounded enum, so it may be a label"
    );
    assert_eq!(emitted[DURATION].label_names(), vec!["service", "tool"]);
    assert_eq!(emitted[BYTES].label_names(), vec!["service", "tool"]);

    for name in [CALLS, DURATION, BYTES] {
        for unbounded in ["user_id", "instance_id", "project_id", "request_id"] {
            assert!(
                !emitted[name].labels.contains_key(unbounded),
                "{unbounded} on {name} is one series per value — the mistake \
                 this module exists to prevent"
            );
        }
    }
    assert_eq!(emitted[CALLS].labels["outcome"], "OK");
    assert_eq!(emitted[CALLS].labels["service"], "svc");
    assert_eq!(emitted[CALLS].labels["tool"], "Tool");
}

/// **The bytes counter takes the RECORD's byte count, not the payload's.**
/// `observe::Call::emit` passes `built.bytes_returned`, so the encoded override
/// reaches the metric as well as the event — otherwise the D49 budget and the
/// aggregate disagree, and only one of them is measuring the wire.
///
/// Catches: passing `outcome.payload.len()` instead of `built.bytes_returned`.
#[test]
fn the_bytes_counter_takes_the_encoded_size_not_the_rendering() {
    let emitted = one_call();
    assert_eq!(
        emitted[BYTES].counter(),
        12,
        "the wire's 12 bytes, not the 54-byte rendering the estimate read"
    );
    assert_eq!(emitted[CALLS].counter(), 1, "one call is one increment");
}

/// The duration histogram is in SECONDS — the unit its name promises, and a
/// different one from the record's microseconds.
#[test]
fn the_duration_histogram_is_in_seconds() {
    let emitted = one_call();
    let observations = emitted[DURATION].histogram();
    assert_eq!(observations.len(), 1);
    assert!(
        observations[0] >= 0.0 && observations[0] < 1.0,
        "a trivial call is a fraction of a second, got {}",
        observations[0]
    );
}

/// D49's saving is reported only when there is one. A counter emitted at zero
/// on every call is a series that says nothing and costs the same as one that
/// does.
#[test]
fn nothing_suppressed_emits_no_suppression_counter() {
    assert!(!one_call().contains_key(BYTES_SUPPRESSED));
}

#[test]
fn what_dedup_saved_is_reported_under_its_own_name() {
    let emitted = emitted_by(|| {
        let (_records, _guard) = capture();
        Call::start("svc", "Tool", Kind::Read, scope()).finish(Outcome {
            status: "OK",
            suppressed: true,
            bytes_suppressed: 4_096,
            ..Default::default()
        });
    });

    assert_eq!(emitted[BYTES_SUPPRESSED].counter(), 4_096);
    assert_eq!(
        emitted[BYTES_SUPPRESSED].label_names(),
        vec!["service", "tool"],
        "meaningful only beside BYTES, so it carries the same labels"
    );
}

/// An error path is counted under ITS status, which is what makes a dashboard
/// grouping by outcome say anything at all.
#[test]
fn the_outcome_label_is_the_status_the_call_reported() {
    let emitted = emitted_by(|| {
        let (_records, _guard) = capture();
        Call::start("svc", "Tool", Kind::Read, scope()).fail("NOT_FOUND");
    });
    assert_eq!(emitted[CALLS].labels["outcome"], "NOT_FOUND");
}

/// A dropped call reaches the metrics too, under the value that says so.
#[test]
fn a_dropped_call_is_counted_as_unrecorded() {
    let emitted = emitted_by(|| {
        let (_records, _guard) = capture();
        drop(Call::start("svc", "Tool", Kind::Read, scope()));
    });
    assert_eq!(emitted[CALLS].labels["outcome"], "UNRECORDED");
}
