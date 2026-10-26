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

use metrics_exporter_prometheus::{BuildError, Matcher, PrometheusBuilder};

/// Total calls, by the three bounded dimensions.
pub const CALLS: &str = "yadgar_calls_total";
/// Call duration in seconds, exported as a Prometheus histogram over
/// [`DURATION_BUCKETS`].
pub const DURATION: &str = "yadgar_call_duration_seconds";

/// The bucket upper bounds, in seconds, that [`DURATION`] exports (ADR-0925
/// Q-C2). Explicit rather than the exporter's default, which renders a
/// summary: a summary's quantiles are computed per pod and cannot be
/// aggregated across replicas, and its decaying window reads zero once a tool
/// goes quiet.
///
/// Each edge has a source, measured on kind-yadgar on 2026-10-17 over 15 days
/// (`max_over_time` of the summary's 0.0 and 1.0 quantiles, and `_sum` /
/// `_count`), or read from a configured value:
///
/// - `0.0001`–`0.001`: the fastest calls. `server/discover` measured 16 us
///   minimum; db-tier reads (`GetKeyIdentity`, `ListProjects`) 0.17–0.6 ms
///   typical. Without sub-millisecond edges every db call lands in one bucket.
/// - `0.0025`–`0.1`: everything that is neither floored nor timing out.
///   The slowest unfloored call measured 70 ms (`create_task`, cold).
/// - `0.15` and `0.2`: resolution below the `Login` floor. An Argon2 verify
///   runs 27–136 ms worst case, so its headroom under 250 ms needs edges
///   between `0.1` and `0.25`.
/// - `0.25` and `0.3`: iam's `Login` response floor, `responseFloorMs: 250`
///   in the iam chart, seen from the CALLER. `gateway auth/login` measured
///   0.253–0.258 s; a floored call lands in `(0.25, 0.3]`. iam's OWN `Login`
///   series excludes the floor by construction: its `Call` finishes inside
///   `login_inner` and `hold_until_floor` runs after it (live iam `Login`
///   mean 25 ms). Even at the gateway, a gateway-side refusal (bad request,
///   upstream unavailable) lands below `0.25` with no floor failure, so a
///   `le="0.25"` count is not by itself an alert.
/// - `0.75` and `0.8`: the same pair for `RedeemEnrolment`, `responseFloorMs:
///   750`, with the same caller-side reading.
/// - `0.5`, `1`, `2.5`, `5`, `10`: the slow-but-served range, half-decades.
/// - `25`: the client pool-acquire timeout (ADR-0925, confirmed as shipped).
///   A call that waited the whole acquire lands just above it.
/// - `30`: the client request timeout (ADR-0925 Q-C4). A call cut at the
///   deadline lands in `+Inf`.
///
/// **The `25` and `30` edges do not follow a retuned timeout.** Q-C1 makes
/// dial's request timeout a chart value per service; an operator who moves it
/// moves where timeouts land, not these edges. Same for both floors. Re-read
/// this list when any of those values changes.
pub const DURATION_BUCKETS: &[f64] = &[
    0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3,
    0.5, 0.75, 0.8, 1.0, 2.5, 5.0, 10.0, 25.0, 30.0,
];
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
/// metrics should still serve traffic, which is the same rule as D67's for the
/// event path.
pub fn install_prometheus(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    builder()?.with_http_listener(addr).install()?;
    tracing::info!(%addr, "metrics endpoint listening");
    Ok(())
}

/// The exporter configuration, minus the listener.
///
/// Buckets are scoped to [`DURATION`] by full name, not set globally: the
/// ruling covers latency, and a global setter would hand latency edges to any
/// future histogram of bytes or counts.
fn builder() -> Result<PrometheusBuilder, BuildError> {
    PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Full(DURATION.to_owned()), DURATION_BUCKETS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render what the INSTALLED configuration exports for one recorded call.
    ///
    /// It goes through `builder()`, the same function `install_prometheus`
    /// calls, so a test that builds its own exporter cannot pass while the
    /// binary still ships the summary.
    fn rendered() -> String {
        let recorder = builder().expect("the bucket set is valid").build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || record("svc", "Tool", "OK", 0.0004, 7));
        handle.render()
    }

    #[test]
    fn duration_exports_as_a_histogram_not_a_summary() {
        let out = rendered();
        assert!(
            out.contains(&format!("# TYPE {DURATION} histogram")),
            "DURATION is not a histogram:\n{out}"
        );
        assert!(
            !out.contains("quantile="),
            "a summary quantile leaked:\n{out}"
        );
        assert!(
            out.contains(&format!("{DURATION}_sum{{")),
            "no _sum:\n{out}"
        );
        assert!(
            out.contains(&format!("{DURATION}_count{{")),
            "no _count:\n{out}"
        );
    }

    #[test]
    fn every_declared_edge_and_inf_is_exported() {
        let out = rendered();
        for le in DURATION_BUCKETS
            .iter()
            .map(f64::to_string)
            .chain(["+Inf".into()])
        {
            let needle = format!("le=\"{le}\"");
            assert!(
                out.lines()
                    .any(|l| l.starts_with(&format!("{DURATION}_bucket{{")) && l.contains(&needle)),
                "no bucket {needle}:\n{out}"
            );
        }
    }

    #[test]
    fn the_buckets_are_the_ruled_set() {
        // Pinned, not derived: each edge is a measured or configured number
        // (see DURATION_BUCKETS), and moving one is a decision, not drift.
        assert_eq!(
            DURATION_BUCKETS,
            &[
                0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.15, 0.2,
                0.25, 0.3, 0.5, 0.75, 0.8, 1.0, 2.5, 5.0, 10.0, 25.0, 30.0,
            ]
        );
    }

    #[test]
    fn no_other_histogram_inherits_the_latency_buckets() {
        let recorder = builder().expect("the bucket set is valid").build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            metrics::histogram!("some_other_size_bytes").record(3.0)
        });
        let out = handle.render();
        assert!(
            out.contains("# TYPE some_other_size_bytes summary"),
            "the latency buckets were applied globally:\n{out}"
        );
    }
}
