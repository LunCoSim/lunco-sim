//! Transport-independent client netcode.
//!
//! This package owns the state and systems that make a client follow and
//! predict an authoritative simulation: interpolation buffers, ownership
//! prediction, rollback, reconciliation, and their diagnostics. It does not
//! know how snapshots arrive. The WebTransport/lightyear adapter in
//! `lunco-networking` produces [`session::IncomingSnapshots`] and composes
//! [`prediction::NetcodePredictionPlugin`] when networking is enabled.
//!
//! Keeping this boundary separate is important for incremental builds. Changes
//! to the transport protocol do not rebuild prediction, and a headless build
//! that only uses the address/configuration surface does not compile Avian
//! prediction systems.

pub mod prediction;
pub mod reconcile;
pub mod session;

pub use reconcile::{ReconcileParams, Reconciliation, reconcile_decision};
