//! Generic typed control-relationship commands.

use bevy::prelude::*;
use lunco_core::Command;

/// Acquire an authored control endpoint for a control producer.
#[Command]
pub struct AcquireControl {
    /// The producer that receives the control relationship. `None` resolves to
    /// the local embodiment on the authoritative application.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub source: Option<Entity>,
    /// The entity exposing the writable control surface.
    pub target: Entity,
    /// Whether the local presentation rig should follow the acquired target.
    #[serde(default = "default_true")]
    #[reflect(default = "default_true")]
    pub bind_camera: bool,
}

fn default_true() -> bool {
    true
}

/// Release the control relationship held by a producer.
#[Command]
pub struct ReleaseControlSource {
    /// The producer releasing its control relationship.
    pub source: Entity,
}
