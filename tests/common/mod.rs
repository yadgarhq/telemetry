//! What the suite needs to OBSERVE, rather than merely run, the thing under
//! test.
//!
//! Every helper here exists because a signal this crate emits was previously
//! unreadable from a test, which is why seven tests asserted nothing: the
//! records went to stdout, the span's fields were never visited, and the
//! metrics went into a facade with no recorder. One capture per signal.

// Each test binary compiles this module whole and uses the part it needs, so a
// helper unused by one of them is expected rather than dead.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use yadgar_telemetry::observe::Scope;
use yadgar_telemetry::record::{Sink, SinkGuard};

/// Four values that are all DIFFERENT, so a transposition shows up.
///
/// The reason `Scope` is a struct at all (`src/observe.rs:19-21`): four loose
/// strings of the same type transpose silently. With `"u"` for every field they
/// would transpose silently here too.
pub fn scope() -> Scope {
    Scope {
        request_id: "req-1".into(),
        instance_id: "inst-2".into(),
        user_id: "user-3".into(),
        project_id: "proj-4".into(),
    }
}

/// A sink that keeps what was written.
#[derive(Debug, Default)]
pub struct Capture {
    lines: Mutex<Vec<String>>,
}

impl Sink for Capture {
    fn write_record(&self, line: &str) -> io::Result<()> {
        self.lines
            .lock()
            .expect("capture poisoned")
            .push(line.to_string());
        Ok(())
    }
}

impl Capture {
    /// Every record written while the guard was held, in order.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("capture poisoned").clone()
    }

    /// The one record written. Panics on none or several, which is the point
    /// for "records ONCE".
    pub fn only(&self) -> serde_json::Value {
        let lines = self.lines();
        assert_eq!(
            lines.len(),
            1,
            "expected exactly one record, got: {lines:?}"
        );
        serde_json::from_str(&lines[0]).expect("a record must be valid JSON")
    }

    /// Every record, parsed.
    pub fn records(&self) -> Vec<serde_json::Value> {
        self.lines()
            .iter()
            .map(|l| serde_json::from_str(l).expect("a record must be valid JSON"))
            .collect()
    }
}

/// Capture records for as long as the guard lives.
///
/// **Bind the guard before constructing the `Call`.** A `Call` emits from
/// `Drop`, so a guard declared afterwards is dropped FIRST and the record goes
/// to the real stdout — an empty capture and a baffling failure.
pub fn capture() -> (Arc<Capture>, SinkGuard) {
    let capture = Arc::new(Capture::default());
    let guard = yadgar_telemetry::record::set_sink(capture.clone());
    (capture, guard)
}

/// A sink that always fails, standing in for a closed stdout.
#[derive(Debug, Default)]
pub struct FailingSink;

impl Sink for FailingSink {
    fn write_record(&self, _line: &str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "Broken pipe"))
    }
}

// ---------------------------------------------------------------------------
// The span half.

/// One span, as the subscriber saw it created.
#[derive(Debug, Clone)]
pub struct RecordedSpan {
    pub name: &'static str,
    pub fields: BTreeMap<String, String>,
}

impl RecordedSpan {
    /// The field NAMES, sorted. Asserting on the exact set is what catches a
    /// field being ADDED — `user_id` on the span is the case
    /// `src/observe.rs:73-74` forbids, and an absence-only assertion would also
    /// pass if the span body were deleted entirely.
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.keys().map(String::as_str).collect()
    }
}

/// A `tracing::Subscriber` that records the fields of every span opened.
///
/// Hand-rolled rather than pulled from `tracing-subscriber`: this needs exactly
/// one thing — the attributes at `new_span` — and a formatting-and-filtering
/// stack is a dependency and a licence decision (O10) for no extra coverage.
#[derive(Debug, Default)]
pub struct SpanRecorder {
    spans: Mutex<Vec<RecordedSpan>>,
    next_id: AtomicU64,
}

impl SpanRecorder {
    pub fn spans(&self) -> Vec<RecordedSpan> {
        self.spans.lock().expect("recorder poisoned").clone()
    }

    /// The one span opened. Panics on none or several.
    pub fn only(&self) -> RecordedSpan {
        let spans = self.spans();
        assert_eq!(spans.len(), 1, "expected exactly one span, got: {spans:?}");
        spans.into_iter().next().expect("length was just checked")
    }
}

/// Collects field values as strings, whichever visitor arm `tracing` picks:
/// `service = service` on a `&str` arrives via `record_str`, `request_id = %..`
/// via `record_debug` on a `format_args!`.
struct FieldCollector(BTreeMap<String, String>);

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl Subscriber for SpanRecorder {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let mut collector = FieldCollector(BTreeMap::new());
        attrs.record(&mut collector);
        self.spans
            .lock()
            .expect("recorder poisoned")
            .push(RecordedSpan {
                name: attrs.metadata().name(),
                fields: collector.0,
            });
        // Span ids must be non-zero.
        Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, _event: &Event<'_>) {}
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

/// Keep the `rpc` callsite dispatching for the life of the test binary.
///
/// `tracing` caches CALLSITE INTEREST globally, and a callsite first reached
/// while no dispatcher is registered caches `Interest::never` — after which the
/// macro short-circuits and no thread-local subscriber is consulted for it
/// again. Tests run in parallel threads, so which one reaches `Call::start`
/// first is a race: without this, the span tests failed about one run in
/// twelve with an empty recorder.
///
/// Registering one dispatcher permanently is the fix, not a retry: registration
/// rebuilds the interest cache, and a dispatcher that never goes away holds the
/// callsite at `sometimes`, which means `enabled` is asked per call — of
/// whichever subscriber the calling thread has.
fn register_a_permanent_dispatcher() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Arc::new(SpanRecorder::default()));
    });
}

/// Record every span opened on THIS thread for as long as the guard lives.
pub fn record_spans() -> (Arc<SpanRecorder>, tracing::subscriber::DefaultGuard) {
    register_a_permanent_dispatcher();
    let recorder = Arc::new(SpanRecorder::default());
    let guard = tracing::subscriber::set_default(recorder.clone());
    (recorder, guard)
}
