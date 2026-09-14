//! Backend-neutral co-simulation contracts.
//!
//! This package contains the data and control state shared by simulation
//! participants, APIs, networking, and authored-domain adapters. It deliberately
//! has no Avian or renderer dependency. Avian-specific port tables, physics
//! systems, and the full wiring scheduler remain in [`lunco-cosim`].

pub mod actuation;
pub mod component;
pub mod connection;
pub mod contract;
pub mod diagnostics;

pub use actuation::{ForceActuator, TorqueActuator};
pub use component::*;
pub use connection::{
    clear_control_write_fence, ControlWriteFence, PortHolds, RealtimeSafe, SimConnection,
};
pub use contract::*;
pub use diagnostics::{AlgebraicLoopDiagnostic, BrokenConnection, CosimDiagnostics};

/// The fixed port name exposed by a generic scalar [`lunco_core::Port`].
pub const PORT_NAME: &str = "value";
