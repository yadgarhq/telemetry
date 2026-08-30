//! The bounded half of D67.
//!
//! **Only labels of bounded cardinality appear here.** Cardinality is the product
//! of every label's range, so a per-user label is one time series per user per
//! tool — which is what makes a metrics store fall over. `user_id`,
//! `instance_id`, `project_id` and the exact timestamp live on the wide event
//! instead, and that split is the whole boundary D67 draws.
//!
//! O17 named this exact risk about `caller` and never stated the rule; it is
//! stated here, in the code that would otherwise violate it.

use std::net::SocketAddr;

/// Total calls, by the three bounded dimensions.
pub const CALLS: &str = "yadgar_calls_total";
/// Call duration in seconds.
pub const DURATION: &str = "yadgar_call_duration_seconds";
/// Bytes returned to the caller — the D49 budget, aggregated.
pub const BYTES: &str = "yadgar_bytes_returned_total";
/// Bytes D49's dedup suppressed. Meaningful only beside BYTES.
pub const BYTES_SUPPRESSED: &str = "yadgar_bytes_suppressed_total";

/// Record one call.
///
/// `tool`, `service` and `outcome` are the ONLY labels, and each is bounded: a
/// fixed set of RPC names, a fixed service name, and the gRPC status enum.
/// Adding an unbounded one here is the mistake this module exists to prevent.
pub fn record(
    service: &'static str,
    tool: &'static str,
    outcome: &'static str,
    seconds: f64,
    bytes: u64,
) {
    metrics::counter!(CALLS, "service" => service, "tool" => tool, "outcome" => outcome)
        .increment(1);
    metrics::histogram!(DURATION, "service" => service, "tool" => tool).record(seconds);
    metrics::counter!(BYTES, "service" => service, "tool" => tool).increment(bytes);
}

/// Record what dedup saved (D49).
pub fn suppressed(service: &'static str, tool: &'static str, bytes: u64) {
    metrics::counter!(BYTES_SUPPRESSED, "service" => service, "tool" => tool).increment(bytes);
}

/// Install the Prometheus scrape endpoint.
///
/// Called by the BINARY, never by a library: a library that installs an exporter
/// decides the backend for every service linking it, and the choice belongs to
/// the deployment.
///
/// Failure is returned rather than panicking — a service that cannot export
/// metrics should still serve traffic, which is the same rule as D25's for the
/// event path.
pub fn install_prometheus(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()?;
    tracing::info!(%addr, "metrics endpoint listening");
    Ok(())
}
