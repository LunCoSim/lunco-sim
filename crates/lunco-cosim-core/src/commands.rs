//! Backend-neutral typed control commands.
//!
//! These commands describe the public control surface. The Avian-backed
//! orchestration crate installs the observers that apply them to a live port
//! registry, while controllers, networking, APIs, and authored scripts can
//! depend on these contracts without depending on that implementation.

use bevy::prelude::*;
use lunco_core::Command;

/// Write a batch of named input ports on `target`.
///
/// This is the generic control command: a wheeled rover, a Modelica-flown
/// lander, or any other endpoint is controlled by writing the input names it
/// declares. The receiver applies writes through its authoritative port
/// backend. `seq` and `tick` carry prediction bookkeeping for networked input.
#[Command]
pub struct SetPorts {
    /// The entity whose input ports are written.
    #[authz_target]
    pub target: Entity,
    /// `(port_name, value)` writes to apply this tick.
    pub writes: Vec<(String, f64)>,
    /// Client prediction sequence number, when the command came from a client.
    #[serde(default)]
    #[reflect(default)]
    pub seq: u32,
    /// Simulation tick associated with this command.
    #[serde(default)]
    #[reflect(default)]
    pub tick: u64,
}

/// Release one manual input-port intent and hand that port back to its wiring.
#[Command]
pub struct ReleasePort {
    /// The entity whose hold is released.
    #[authz_target]
    pub target: Entity,
    /// Input-port name.
    pub name: String,
}

/// Release the complete control intent for an endpoint and apply its safe state.
#[Command]
pub struct ReleaseControl {
    /// The endpoint whose complete control intent is released.
    #[authz_target]
    pub target: Entity,
}
