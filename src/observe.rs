//! One wrapper, all three signals.
//!
//! Written because the alternative is a hand-rolled timing-and-emit block in
//! every RPC of every service, and eleven copies of a thing is eleven chances to
//! forget the span, mislabel the outcome, or transpose two scope fields. It is
//! also what makes `observe-coverage` checkable: a handler either goes through
//! here or it does not.

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
    /// What was returned, for measurement. Empty on an error path.
    pub payload: String,
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
    pub fn finish(self, outcome: Outcome) {
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
