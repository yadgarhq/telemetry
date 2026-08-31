//! Building and emitting a [`CallRecord`].

use std::cell::RefCell;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use crate::estimator::{self, Class, Features};
use crate::pb::yadgar::telemetry::v1::{CallRecord, Kind};

/// Build a record for one call at one hop.
///
/// A builder rather than a wide constructor because the fields differ per hop and
/// per kind — a `-db` sets `rows_returned`, a job sets `rows_touched`, only `ask`
/// sets the generation counters. A constructor taking all of them would be
/// twenty arguments of which most are zero.
#[derive(Debug, Clone, Default)]
pub struct Builder {
    record: CallRecord,
}

impl Builder {
    /// Start a record. `service` is which hop this is; `tool` is the operation as
    /// a caller names it.
    pub fn new(service: &str, tool: &str, kind: Kind) -> Self {
        let mut record = CallRecord {
            service: service.to_string(),
            tool: tool.to_string(),
            ..Default::default()
        };
        record.set_kind(kind);
        Self { record }
    }

    /// The scope fields, from `common.v1.Scope`.
    ///
    /// `request_id` is the join key: one logical call fans out across hops, and
    /// without it a roll-up sums one call several times.
    pub fn scope(
        mut self,
        request_id: &str,
        instance_id: &str,
        user_id: &str,
        project_id: &str,
    ) -> Self {
        self.record.request_id = request_id.to_string();
        self.record.instance_id = instance_id.to_string();
        self.record.user_id = user_id.to_string();
        self.record.project_id = project_id.to_string();
        self
    }

    /// The gRPC status name, e.g. "OK".
    pub fn outcome(mut self, outcome: &str) -> Self {
        self.record.outcome = outcome.to_string();
        self
    }

    pub fn duration(mut self, d: Duration) -> Self {
        self.record.duration_us = d.as_micros() as u64;
        self
    }

    /// Measure the payload, and estimate its token cost from the same features.
    ///
    /// Bytes and words are recorded RAW alongside the estimate, so retuning the
    /// estimator never invalidates the measurement underneath — the estimate is
    /// derived and can be recomputed, the measurement cannot.
    pub fn payload(mut self, payload: &str, class: Class) -> Self {
        let f = Features::of(payload);
        let (tokens, version) = estimator::estimate(f, class);
        self.record.bytes_returned = f.bytes;
        self.record.words_returned = f.words;
        self.record.tokens_estimated = Some(tokens);
        self.record.estimator_version = Some(version.to_string());
        self
    }

    /// Override the byte count with the exact encoded size.
    ///
    /// `payload` measures a rendering; this measures the wire. Words stay from
    /// the rendering because the encoded form has none — each feature comes from
    /// the source that actually has it.
    pub fn encoded_bytes(mut self, bytes: u64) -> Self {
        self.record.bytes_returned = bytes;
        self
    }

    pub fn rows_returned(mut self, n: u32) -> Self {
        self.record.rows_returned = n;
        self
    }

    /// D49: whether this response was suppressed as already-delivered, and what
    /// that saved. A large total with a high suppression share is a working
    /// system; the same total with none is not.
    pub fn dedup(mut self, suppressed: bool, bytes_saved: u64) -> Self {
        self.record.suppressed_by_dedup = suppressed;
        self.record.bytes_suppressed = bytes_saved;
        self
    }

    pub fn build(self) -> CallRecord {
        self.record
    }
}

/// Where an emitted record is written.
///
/// **The seam exists so a record can be OBSERVED.** Without it `emit` writes to
/// stdout and nothing — no test, no embedding process — can see what was
/// written, so the only thing a test can assert is that emission did not panic.
/// That is not a behavioural assertion, and a suite full of them stays green
/// through any change to what is emitted.
///
/// Implementations MUST NOT panic. This runs from [`Drop`], and a panic from a
/// destructor while unwinding aborts the process — the loudest possible way to
/// break the rule that telemetry never fails a call (D25). Report the failure by
/// returning it; [`emit`] logs and drops it.
pub trait Sink: Send + Sync {
    /// Write one serialised record. The line carries no trailing newline; a
    /// sink that needs a delimiter adds its own.
    fn write_record(&self, line: &str) -> io::Result<()>;
}

/// The production sink: one JSON object per line on stdout (D67).
///
/// Deliberately NOT `println!`, which PANICS when the write fails — the
/// demonstrated case being a closed stdout, `failed printing to stdout: Broken
/// pipe`. See [`emit`] for why a panic here is the one outcome this path may not
/// have.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdoutSink;

impl Sink for StdoutSink {
    fn write_record(&self, line: &str) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(line.as_bytes())?;
        // The newline is what makes a LineWriter flush, so this call is also
        // where a broken pipe surfaces — as an error, which is the point.
        out.write_all(b"\n")
    }
}

thread_local! {
    /// `None` means [`StdoutSink`]. The default is not stored, so a thread that
    /// never overrides pays one thread-local read and no allocation.
    static OVERRIDE: RefCell<Option<Arc<dyn Sink>>> = const { RefCell::new(None) };
}

/// Send records to `sink` for as long as the returned guard lives.
///
/// **Thread-scoped, deliberately** — the same shape as
/// `tracing::subscriber::set_default` and `metrics::with_local_recorder`, the two
/// other overrides this crate's tests install. Tests in one binary run in
/// parallel threads, and a process-global sink would make each test's capture
/// depend on what the others happened to be doing at the time.
///
/// Install it BEFORE the [`crate::observe::Call`] whose record it should catch:
/// a `Call` emits from `Drop`, so a guard declared after it is dropped first and
/// the record goes to stdout instead.
#[must_use = "the override lasts only as long as the guard"]
pub fn set_sink(sink: Arc<dyn Sink>) -> SinkGuard {
    let previous = OVERRIDE.with(|slot| slot.borrow_mut().replace(sink));
    SinkGuard {
        previous,
        not_send: PhantomData,
    }
}

/// Restores the previous sink. Not `Send`: the override it undoes belongs to the
/// thread that installed it.
pub struct SinkGuard {
    previous: Option<Arc<dyn Sink>>,
    not_send: PhantomData<*const ()>,
}

impl Drop for SinkGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        // `try_with`, because a guard can outlive the thread-local it restores
        // during thread teardown, and `with` panics there — from a destructor,
        // which aborts.
        let _ = OVERRIDE.try_with(|slot| *slot.borrow_mut() = previous);
    }
}

fn write_line(line: &str) -> io::Result<()> {
    let sink = OVERRIDE
        .try_with(|slot| slot.borrow().clone())
        .ok()
        .flatten();
    match sink {
        Some(sink) => sink.write_record(line),
        None => StdoutSink.write_record(line),
    }
}

/// Write the record where the collector will find it.
///
/// **Never blocks, never fails the call.** A serialisation failure is logged and
/// dropped: D25's rule is that this path must not be able to break a request, and
/// a telemetry error that propagates is a new outage source on every path.
///
/// The WRITE failure is dropped for the same reason, and it is the likelier of
/// the two: a closed stdout is an ordinary end for a process in a pipeline, and
/// this is reached from `Call`'s `Drop`. `println!` panics on it, so the rule the
/// module documents held for serialisation only.
///
/// stdout as JSON rather than an RPC, per D67 — the transport is boring on
/// purpose, and it survives yadgar being broken in a way a telemetry service does
/// not.
pub fn emit(record: &CallRecord) {
    let line = match serde_json::to_string(&Wire::from(record)) {
        Ok(line) => line,
        Err(e) => {
            tracing::warn!(error = %e, "dropping a telemetry record it could not serialise");
            return;
        }
    };
    if let Err(e) = write_line(&line) {
        tracing::warn!(error = %e, "dropping a telemetry record it could not write");
    }
}

/// The JSON shape on the wire.
///
/// Written out rather than derived on the generated type: prost does not emit
/// serde derives, and adding them would mean either a codegen flag every consumer
/// must match or a wrapper. The wrapper is explicit and diffable.
#[derive(serde::Serialize)]
struct Wire<'a> {
    request_id: &'a str,
    instance_id: &'a str,
    user_id: &'a str,
    project_id: &'a str,
    tool: &'a str,
    service: &'a str,
    kind: i32,
    outcome: &'a str,
    duration_us: u64,
    bytes_returned: u64,
    words_returned: u32,
    rows_returned: u32,
    suppressed_by_dedup: bool,
    bytes_suppressed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_estimated: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimator_version: Option<&'a str>,
}

impl<'a> From<&'a CallRecord> for Wire<'a> {
    fn from(r: &'a CallRecord) -> Self {
        Self {
            request_id: &r.request_id,
            instance_id: &r.instance_id,
            user_id: &r.user_id,
            project_id: &r.project_id,
            tool: &r.tool,
            service: &r.service,
            kind: r.kind,
            outcome: &r.outcome,
            duration_us: r.duration_us,
            bytes_returned: r.bytes_returned,
            words_returned: r.words_returned,
            rows_returned: r.rows_returned,
            suppressed_by_dedup: r.suppressed_by_dedup,
            bytes_suppressed: r.bytes_suppressed,
            tokens_estimated: r.tokens_estimated,
            estimator_version: r.estimator_version.as_deref(),
        }
    }
}
