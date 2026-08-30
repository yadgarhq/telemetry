//! One wrapper, all three signals.
//!
//! Written because the alternative is a hand-rolled timing-and-emit block in
//! every RPC of every service, and eleven copies of a thing is eleven chances to
//! forget the span, mislabel the outcome, or transpose two scope fields. It is
//! also what makes `observe-coverage` checkable: a handler either goes through
//! here or it does not.

use std::future::Future;
use std::time::Instant;

use crate::estimator::Class;
use crate::metrics;
use crate::pb::yadgar::telemetry::v1::Kind;
use crate::record;

/// The scope fields a record needs.
///
/// Captured before a request is consumed. A struct rather than four loose
/// strings because they are always used together and transposing two produces
/// telemetry that is wrong in a way no test catches.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub request_id: String,
    pub instance_id: String,
    pub user_id: String,
    pub project_id: String,
}

/// What the handler produced, for the fields only it can know.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// The gRPC status name. A bounded value — it reaches a metric label.
    pub status: &'static str,
    /// What was returned, rendered as text — used for the WORD count only.
    /// Empty on an error path.
    pub payload: String,

    /// The exact encoded size on the wire.
    ///
    /// Set it. Without it, bytes are counted from `payload`, which is a Debug or
    /// display rendering and measures the wrong thing — right order of
    /// magnitude, wrong in detail. A prost message knows its own
    /// `encoded_len()`, so the exact number is available without a gateway.
    pub encoded_bytes: Option<u64>,
    pub class: Class,
    pub rows: u32,
    pub suppressed: bool,
    pub bytes_suppressed: u64,
}

/// A call in progress.
///
/// Started before the work, finished after — so the duration covers the handler
/// and nothing else, and an early return cannot silently skip the record: a
/// dropped `Call` emits nothing, which `observe-coverage` is what catches.
pub struct Call {
    service: &'static str,
    tool: &'static str,
    kind: Kind,
    scope: Scope,
    started: Instant,
    span: tracing::Span,
    /// Set by `finish`, checked by `Drop`. Without it a handler that returns
    /// early — every `?` on an error path — would emit nothing at all, so
    /// failures would be the one outcome the telemetry could not see.
    finished: bool,
}

impl Call {
    /// Open a span and start the clock.
    ///
    /// The span carries `request_id` so a trace and its events correlate without
    /// a second identifier — and NOT `user_id`, which belongs on the event.
    pub fn start(service: &'static str, tool: &'static str, kind: Kind, scope: Scope) -> Self {
        let span = tracing::info_span!(
            "rpc",
            service = service,
            tool = tool,
            request_id = %scope.request_id,
        );
        Self {
            service,
            tool,
            kind,
            scope,
            started: Instant::now(),
            span,
            finished: false,
        }
    }

    /// The span, for a handler that wants to attach its own fields or enter it
    /// around an inner await.
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }

    /// Emit the event and the metrics.
    ///
    /// **Never blocks and never fails the call** (D25). This runs on `recall`,
    /// the only latency-critical path; a record that cannot be written is lost
    /// and the response still goes out. That is the exact opposite of D69's
    /// capability probe, which must always fail boot — the two rules point
    /// opposite ways on purpose.
    pub fn finish(mut self, outcome: Outcome) {
        self.finished = true;
        self.emit(outcome);
    }

    fn emit(&self, outcome: Outcome) {
        let elapsed = self.started.elapsed();

        let mut builder = record::Builder::new(self.service, self.tool, self.kind)
            .scope(
                &self.scope.request_id,
                &self.scope.instance_id,
                &self.scope.user_id,
                &self.scope.project_id,
            )
            .outcome(outcome.status)
            .duration(elapsed)
            .rows_returned(outcome.rows)
            .dedup(outcome.suppressed, outcome.bytes_suppressed);

        if !outcome.payload.is_empty() {
            builder = builder.payload(&outcome.payload, outcome.class);
        }
        // The encoded length wins over the rendered one: words come from the
        // text, bytes from the wire. Two features, two sources, each measuring
        // what it actually is.
        if let Some(bytes) = outcome.encoded_bytes {
            builder = builder.encoded_bytes(bytes);
        }

        let built = builder.build();
        record::emit(&built);

        metrics::record(
            self.service,
            self.tool,
            outcome.status,
            elapsed.as_secs_f64(),
            built.bytes_returned,
        );
        if outcome.bytes_suppressed > 0 {
            metrics::suppressed(self.service, self.tool, outcome.bytes_suppressed);
        }
    }

    /// Run a handler body, classify its outcome, and emit exactly once.
    ///
    /// **This is what makes an error path classified rather than `UNRECORDED`.**
    /// A handler using `?` returns early and drops the `Call`, which records that
    /// it happened but not what it was — failures counted, not classified. Here
    /// the body's `Result` is inspected before the record is written.
    ///
    /// Generic over the error rather than taking a `tonic::Status`, so this crate
    /// stays transport-agnostic: the caller supplies the mapping to a bounded
    /// label, which is the only thing a metric can safely carry.
    pub async fn run<T, E, F>(
        self,
        body: F,
        describe: impl FnOnce(&T) -> Outcome,
        classify: impl FnOnce(&E) -> &'static str,
    ) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let result = body.await;
        match &result {
            Ok(value) => self.finish(describe(value)),
            Err(e) => self.fail(classify(e)),
        }
        result
    }

    /// Finish an error path.
    ///
    /// A separate entry point so a failing handler cannot accidentally report
    /// `OK` by reusing a default — the outcome is the one field a metric label
    /// depends on being right.
    pub fn fail(self, status: &'static str) {
        self.finish(Outcome {
            status,
            ..Default::default()
        });
    }
}

/// A call that ended without `finish` is still recorded.
///
/// **This is what makes coverage structural rather than a habit.** Every `?` on
/// an error path drops the `Call`, and without this the one outcome telemetry
/// could not see would be failure — the outcome most worth seeing.
///
/// It reports `UNRECORDED` rather than guessing a status, so a handler that
/// genuinely forgot to call `finish` is visible in the data as a distinct value
/// rather than silently counted as an error, and `observe-coverage` has something
/// to point at.
impl Drop for Call {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.emit(Outcome {
            status: "UNRECORDED",
            ..Default::default()
        });
    }
}
