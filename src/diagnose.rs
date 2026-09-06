//! What an operator reads when something refuses.
//!
//! **Not a fourth signal.** [`crate::metrics`], [`crate::record`] and the span
//! in [`crate::observe`] carry what a running system emits about its own work.
//! This module carries the one thing a process says on the way out, and it is
//! here for the reason `grpc::status_name` is: the estate had written it once
//! per service, and text an operator reads is worth writing once so that five
//! services cannot disagree about it.
//!
//! Deliberately UNGATED, unlike the gRPC half. The signature names
//! `std::error::Error` and nothing else, so putting it behind `grpc` would be a
//! claim the code does not make — and three of the five copies it replaces sat
//! in boot paths that never touch a transport.

/// Flatten an error and everything under it into one sentence.
///
/// # Why this is not `error.to_string()`
///
/// `tonic::transport::Error` displays as the three words "transport error" and
/// keeps what actually went wrong in its source. So a message built from the
/// head of the chain tells an operator that the transport failed and nothing
/// about which file was unreadable or which key did not match its certificate.
/// Losing it is the same class of mistake as printing `Debug` from `main`.
///
/// # Why it is shared
///
/// It was written five times — `iam`, `iam-db`, `task`, `task-db` and
/// `project-db` — under TWO names, `chain` in the first two and `describe` in
/// the other three, with byte-identical bodies apart from local variable names.
/// The two names are the interesting part rather than a detail: a sweep that
/// grepped for either one would have found two copies or three and reported the
/// unit consolidated (ADR-0591). The count comes from what the code DOES.
///
/// # The separator is an interface
///
/// Layers are joined with `": "`, matching all five copies. It is what a human
/// reads in a crash loop, so changing it reformats every one of those services
/// at once; `tests/diagnose.rs` pins the exact text rather than reading this
/// module's own constant back.
///
/// # The argument is a plain `&dyn Error`
///
/// Three of the five copies took `&dyn std::error::Error` and two took
/// `&(dyn std::error::Error + 'static)`. The first is the permissive one — a
/// borrowed error that does not outlive the call is accepted — and it is what
/// is published here, because a shared unit that refuses inputs one of its
/// callers already passes is not the same unit.
///
/// ```
/// use yadgar_telemetry::diagnose::chain;
///
/// #[derive(Debug)]
/// struct Refusal;
///
/// impl std::fmt::Display for Refusal {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.write_str("cannot serve")
///     }
/// }
///
/// impl std::error::Error for Refusal {}
///
/// assert_eq!(chain(&Refusal), "cannot serve");
/// ```
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(current) = source {
        rendered.push_str(": ");
        rendered.push_str(&current.to_string());
        source = current.source();
    }
    rendered
}
