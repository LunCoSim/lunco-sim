//! Backend-neutral typed control commands.
//!
//! These commands describe the public control surface. The Avian-backed
//! orchestration crate installs the observers that apply them to a live port
//! registry, while controllers, networking, APIs, and authored scripts can
//! depend on these contracts without depending on that implementation.

use bevy::prelude::*;
use lunco_core::Command;

/// Internal lifecycle request to apply an endpoint safe state immediately.
/// This is an owner event rather than a user command; authored external
/// releases use [`ReleaseControl`] and enter the session input queue.
#[derive(Event, Clone, Copy, Debug)]
pub struct ControlSafeStop {
    /// Endpoint whose control surface must be neutralized.
    pub target: Entity,
}

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
    /// Stable producer identity for API, actorless Rhai, and direct typed
    /// inputs that are admitted to the session's next fixed tick. Twin Rhai
    /// uses its stable actor identity and omits this field. Internal
    /// fixed-step producers also omit it because their source inputs are
    /// captured at their owning boundary.
    #[serde(default)]
    #[reflect(default)]
    pub producer_id: Option<u64>,
}

/// Release one manual input-port intent and hand that port back to its wiring.
#[Command]
pub struct ReleasePort {
    /// The entity whose hold is released.
    #[authz_target]
    pub target: Entity,
    /// Input-port name.
    pub name: String,
    /// Stable producer identity for API, actorless Rhai, and direct typed
    /// releases admitted to the session's next fixed tick. Internal releases
    /// omit this field because their cause is owned by another fixed-step or
    /// lifecycle boundary.
    #[serde(default)]
    #[reflect(default)]
    pub producer_id: Option<u64>,
}

/// Release the complete control intent for an endpoint and apply its safe state.
#[Command]
pub struct ReleaseControl {
    /// The endpoint whose complete control intent is released.
    #[authz_target]
    pub target: Entity,
    /// Stable producer identity for API, actorless Rhai, and direct typed
    /// releases admitted to the session's next fixed tick. Twin Rhai uses its
    /// stable actor identity and omits this field. Internal lifecycle releases
    /// omit it because their cause is owned by another boundary.
    #[serde(default)]
    #[reflect(default)]
    pub producer_id: Option<u64>,
}
