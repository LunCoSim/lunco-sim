//! Backend-neutral access to the live, layered input transaction owner.

use bevy::prelude::*;
use lunco_core::CommandOrigin;

/// Installed by the co-simulation orchestrator. Live participants use this
/// writer so controller/program priority, backend validation and hold lifetime
/// are committed by one owner. Document-only hosts do not install it.
#[derive(Resource, Clone, Copy)]
pub struct LayeredInputWriter(
    pub fn(&mut World, Entity, &[(String, f64)], Option<CommandOrigin>) -> Result<(), String>,
);

impl LayeredInputWriter {
    /// Commit a complete named-input transaction with its original provenance.
    pub fn write(
        self,
        world: &mut World,
        target: Entity,
        writes: &[(String, f64)],
        origin: Option<CommandOrigin>,
    ) -> Result<(), String> {
        (self.0)(world, target, writes, origin)
    }
}
