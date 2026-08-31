//! The builder and the wire shape.
//!
//! `build()` returns the `CallRecord`, so every field the builder sets is
//! assertable — and `Wire` is what a collector actually parses, so its field
//! MAPPING is a contract. Transposing `user_id` and `project_id` there produces
//! telemetry that is wrong in a way `src/observe.rs:19-21` admits no test
//! catches. These tests are that test.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{capture, Capture};
use yadgar_telemetry::estimator::{Class, VERSION};
use yadgar_telemetry::pb::yadgar::telemetry::v1::{CallRecord, Kind};
use yadgar_telemetry::record::{self, Builder};

/// Every field `Wire` serialises, in the order it declares them. A collector
/// parses these NAMES; renaming or dropping one is a breaking change to
/// something outside this repository.
const ALWAYS_PRESENT: &[&str] = &[
    "request_id",
    "instance_id",
    "user_id",
    "project_id",
    "tool",
    "service",
    "kind",
    "outcome",
    "duration_us",
    "bytes_returned",
    "words_returned",
    "rows_returned",
    "suppressed_by_dedup",
    "bytes_suppressed",
];

/// Round-trip one record through `emit` and read back what a collector sees.
fn emitted(record: &CallRecord) -> serde_json::Value {
    let (records, _guard) = capture();
    record::emit(record);
    records.only()
}

fn keys(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a record is a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The scope fields are four strings of the same type, which is exactly why
/// transposing two of them is invisible. Four DISTINCT values are what make it
/// visible.
///
/// Catches: swapping `user_id` and `project_id` in `Wire::from`, or in
/// `Builder::scope`.
#[test]
fn each_scope_field_lands_in_its_own_wire_column() {
    let record = Builder::new("the-service", "the-tool", Kind::Read)
        .scope("the-request", "the-instance", "the-user", "the-project")
        .outcome("OK")
        .build();

    let wire = emitted(&record);
    assert_eq!(wire["request_id"], "the-request");
    assert_eq!(wire["instance_id"], "the-instance");
    assert_eq!(wire["user_id"], "the-user");
    assert_eq!(wire["project_id"], "the-project");
    assert_eq!(wire["service"], "the-service");
    assert_eq!(wire["tool"], "the-tool");
}

/// The record a collector parses has exactly these keys. An added one is a
/// schema change nobody downstream agreed to; a missing one is a column that
/// silently becomes null.
#[test]
fn the_wire_carries_exactly_the_always_present_fields() {
    let record = Builder::new("svc", "Tool", Kind::Read)
        .scope("r", "i", "u", "p")
        .outcome("OK")
        .build();

    let mut expected: Vec<String> = ALWAYS_PRESENT.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(keys(&emitted(&record)), expected);
}

/// MICROseconds, as the field name and the proto both say. Milliseconds would
/// be a thousandfold error that every latency dashboard would render as a
/// plausible number.
///
/// Catches: `d.as_micros()` becoming `d.as_millis()`.
#[test]
fn duration_is_recorded_in_microseconds() {
    let record = Builder::new("svc", "Tool", Kind::Read)
        .duration(Duration::from_millis(3))
        .build();
    assert_eq!(
        record.duration_us, 3_000,
        "three milliseconds is three thousand microseconds"
    );

    let record = Builder::new("svc", "Tool", Kind::Read)
        .duration(Duration::from_micros(1_234))
        .build();
    assert_eq!(record.duration_us, 1_234);
    assert_eq!(emitted(&record)["duration_us"], 1_234);
}

/// `payload` measures AND estimates from the same features, and the estimate is
/// never separated from the version that produced it.
///
/// Catches: `payload` dropping the estimate, or attaching one with no version.
#[test]
fn payload_records_the_measurement_and_an_estimate_carrying_its_version() {
    let record = Builder::new("svc", "Tool", Kind::Read)
        .payload("alpha beta gamma", Class::Prose)
        .build();

    assert_eq!(record.bytes_returned, 16, "the raw measurement, kept as-is");
    assert_eq!(record.words_returned, 3);
    assert_eq!(
        record.tokens_estimated,
        Some(4),
        "16 bytes of prose over 4.0, against 3 words times 1.3 — words win"
    );
    assert_eq!(record.estimator_version.as_deref(), Some(VERSION));

    let wire = emitted(&record);
    assert_eq!(wire["tokens_estimated"], 4);
    assert_eq!(wire["estimator_version"], VERSION);
}

/// The two optionals are OMITTED rather than sent as null when nothing measured
/// the payload — an estimate of zero and no estimate at all are different facts.
#[test]
fn an_unmeasured_payload_omits_the_estimate_entirely() {
    let record = Builder::new("svc", "Tool", Kind::Read)
        .outcome("INTERNAL")
        .build();
    let wire = emitted(&record);
    assert!(wire.get("tokens_estimated").is_none());
    assert!(wire.get("estimator_version").is_none());
}

/// `payload` measures a rendering; `encoded_bytes` measures the wire. Bytes
/// come from the wire, words from the rendering — each feature from the source
/// that actually has it.
///
/// Catches: `encoded_bytes` made a no-op, or clobbering the word count too.
#[test]
fn encoded_bytes_replaces_the_byte_count_and_leaves_the_words() {
    let rendered = "a much longer rendered debug string than the wire form";
    let record = Builder::new("svc", "Tool", Kind::Read)
        .payload(rendered, Class::Envelope)
        .encoded_bytes(12)
        .build();

    assert_eq!(
        record.bytes_returned,
        12,
        "the wire's 12, not the rendering's {}",
        rendered.len()
    );
    assert_eq!(
        record.words_returned, 10,
        "words survive: the encoded form has none to offer"
    );
}

/// The counters that are not the payload: rows, and D49's pair.
#[test]
fn rows_and_the_dedup_pair_are_recorded_separately() {
    let record = Builder::new("svc", "Tool", Kind::Read)
        .rows_returned(5)
        .dedup(true, 4_096)
        .build();

    assert_eq!(record.rows_returned, 5);
    assert!(record.suppressed_by_dedup);
    assert_eq!(record.bytes_suppressed, 4_096);

    let wire = emitted(&record);
    assert_eq!(wire["rows_returned"], 5);
    assert_eq!(wire["suppressed_by_dedup"], true);
    assert_eq!(wire["bytes_suppressed"], 4_096);
}

/// `kind` is one of the few fields that may also be a metric label, so it is an
/// enum and it reaches the wire as its numeric discriminant.
#[test]
fn kind_reaches_the_wire_as_its_discriminant() {
    for (kind, discriminant) in [
        (Kind::Read, 1),
        (Kind::Write, 2),
        (Kind::Generate, 3),
        (Kind::Job, 4),
    ] {
        let record = Builder::new("svc", "Tool", kind).build();
        assert_eq!(record.kind(), kind);
        assert_eq!(emitted(&record)["kind"], discriminant);
    }
}

/// D25's rule, at the point it was only half true: serialisation failure was
/// handled, the WRITE was a `println!` that panics on a closed stdout.
///
/// Catches: `emit` propagating or unwrapping a sink error.
#[test]
fn emit_survives_a_sink_that_cannot_write() {
    let _guard = record::set_sink(Arc::new(common::FailingSink));
    record::emit(
        &Builder::new("svc", "Tool", Kind::Read)
            .outcome("OK")
            .build(),
    );
}

/// The default sink is stdout and is restored the moment an override's guard
/// drops — an override that leaked would silently swallow a service's telemetry.
#[test]
fn an_override_lasts_exactly_as_long_as_its_guard() {
    let capture = Arc::new(Capture::default());
    {
        let _guard = record::set_sink(capture.clone());
        record::emit(&Builder::new("svc", "Inside", Kind::Read).build());
    }
    record::emit(&Builder::new("svc", "Outside", Kind::Read).build());

    let records = capture.records();
    assert_eq!(records.len(), 1, "only the record emitted under the guard");
    assert_eq!(records[0]["tool"], "Inside");
}
