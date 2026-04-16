//! Collider synchronization from simulation outputs.
//!
//! Watches [`SimComponent`] outputs for `volume` and updates
//! the entity's [`Collider`] to a sphere with the corresponding radius.

use bevy::prelude::*;
use avian3d::prelude::{Collider, RigidBody};

use crate::SimComponent;

/// Updates [`Collider`] from simulation output values.
///
/// Currently supports:
/// - `volume` → `Collider::sphere(cbrt(3*V/(4π)))`
///
/// ## Execution
///
/// Runs in [`Update`] (not FixedUpdate) because collider changes are
/// expensive and don't need per-physics-step precision.
pub fn sync_collider(
    q_components: Query<(Entity, &SimComponent), With<RigidBody>>,
    mut q_colliders: Query<&mut Collider>,
) {
    for (entity, comp) in &q_components {
        // Check for volume output → update sphere collider radius
        if let Some(&volume) = comp.outputs.get("volume") {
            if volume > 0.0 {
                let radius = ((3.0 * volume) / (4.0 * std::f64::consts::PI)).cbrt();
                if let Ok(mut collider) = q_colliders.get_mut(entity) {
                    *collider = Collider::sphere(radius);
                }
            }
        }
    }
}
