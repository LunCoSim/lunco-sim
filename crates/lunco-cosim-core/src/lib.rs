//! Backend-neutral co-simulation contracts.
//!
//! This package contains the data and control state shared by simulation
//! participants, APIs, networking, and authored-domain adapters. The Avian
//! port tables, physics systems, and full wiring scheduler are supplied by
//! [`lunco-cosim`], while presentation adapters consume the same contracts.

pub mod actuation;
pub mod binding;
pub mod commands;
pub mod component;
pub mod connection;
pub mod contract;
pub mod diagnostics;

pub use actuation::{ForceActuator, TorqueActuator};
pub use binding::{BoundConnection, ConnectionBinding};
pub use component::*;
pub use connection::{
    ControlWriteFence, PortHolds, RealtimeSafe, SimConnection, clear_control_write_fence,
};
pub use contract::*;
pub use diagnostics::{AlgebraicLoopDiagnostic, BrokenConnection, CosimDiagnostics};

/// The fixed port name exposed by a generic scalar port endpoint.
pub const PORT_NAME: &str = "value";
