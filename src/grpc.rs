//! The gRPC-shaped bits, shared so they are written once.
//!
//! Behind the `grpc` feature: a consolidation job or a hook emitting records has
//! no business pulling a gRPC stack, and the rest of this crate is deliberately
//! transport-agnostic.

use tonic::Status;

/// A gRPC status as a bounded metric label.
///
/// **Bounded is the whole point.** The status CODE is an enum with a fixed range,
/// so it is safe as a label; the status MESSAGE is caller-influenced text and
/// would be unbounded cardinality — the exact mistake D67 draws its boundary to
/// prevent. This function is the boundary, in one place.
///
/// Shared rather than written per service for the reason D19 gives about the
/// cache library and D51 about tag resolution: ~61 copies of a mapping is ~61
/// chances for two services to disagree about what an outcome is called, and a
/// dashboard that groups by outcome would then be quietly wrong.
///
/// Unlisted codes collapse to `UNKNOWN` rather than being passed through, so a
/// future tonic release cannot widen the label set without this file changing.
pub fn status_name(status: &Status) -> &'static str {
    use tonic::Code::*;
    match status.code() {
        Ok => "OK",
        Cancelled => "CANCELLED",
        InvalidArgument => "INVALID_ARGUMENT",
        DeadlineExceeded => "DEADLINE_EXCEEDED",
        NotFound => "NOT_FOUND",
        AlreadyExists => "ALREADY_EXISTS",
        PermissionDenied => "PERMISSION_DENIED",
        ResourceExhausted => "RESOURCE_EXHAUSTED",
        FailedPrecondition => "FAILED_PRECONDITION",
        Aborted => "ABORTED",
        Unimplemented => "UNIMPLEMENTED",
        Internal => "INTERNAL",
        Unavailable => "UNAVAILABLE",
        Unauthenticated => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}
