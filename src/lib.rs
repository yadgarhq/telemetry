//! `yadgar-telemetry` — D67's instrumentation seam.
//!
//! Three signals, one emission point, and they are not interchangeable:
//!
//! | | carries | why it cannot be one of the others |
//! | --- | --- | --- |
//! | metric | `tool`, `kind`, `outcome` — BOUNDED | a per-user label is one series per user per tool |
//! | wide event | plus user, instance, project, exact timestamp | unbounded dimensions have to live somewhere |
//! | span | causality across hops | answers *where the time went*, which neither other can |
//!
//! **The cardinality rule is the whole boundary**: an unbounded dimension goes on
//! the event, never on a metric label.
//!
//! **Emission never blocks and never fails a call.** This sits on `recall`, the
//! only latency-critical path (D25). A record that cannot be written is lost and
//! the call still succeeds — telemetry that can fail a request is a new outage
//! source bolted to every path in the system.
//!
//! That is the exact opposite of the capability probe (D69), which must ALWAYS
//! fail boot. The two rules point opposite ways on purpose.

#![forbid(unsafe_code)]

pub mod estimator;
pub mod metrics;
pub mod observe;
pub mod record;

/// Generated from the vendored contract (D16, D70).
pub mod pb {
    pub mod yadgar {
        pub mod telemetry {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/yadgar.telemetry.v1.rs"));
            }
        }
    }
}
