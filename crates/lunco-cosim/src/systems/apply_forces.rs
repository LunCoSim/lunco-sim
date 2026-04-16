//! Routes SimComponent force outputs to Avian as external forces.
//!
//! Reads `AvianSim::inputs["force_y"]` (and `force_x`, `force_z`) — delivered
//! by `propagate_connections` from upstream SimComponents — and applies them
//! as external forces on the Avian `RigidBody`. Avian's own solver handles
//! velocity and position integration.
//!
//! This keeps `apply_sim_forces` purely a force-routing system. It does NOT
//! touch `Position` or `LinearVelocity` — both belong to Avian.

use avian3d::prelude::{Forces, RigidBody, WriteRigidBodyForces};
use bevy::math::DVec3;
use bevy::prelude::*;

use crate::AvianSim;

/// System sets for applying forces.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Apply SimComponent outputs as Avian forces.
    ApplyForces,
}

/// Consumes force inputs from `AvianSim::inputs` and applies them as external
/// forces on the entity's Avian `RigidBody`.
///
/// - `force_x`, `force_y`, `force_z` → `Forces::apply_force`
///
/// `avian.take_inputs()` drains the inputs so force accumulation starts fresh
/// next tick — propagate_connections will re-fill them from upstream outputs.
pub fn apply_sim_forces(
    mut q_avian: Query<(Entity, &mut AvianSim), With<RigidBody>>,
    mut forces: Query<Forces>,
) {
    for (entity, mut avian) in &mut q_avian {
        let [fx, fy, fz] = avian.take_inputs();
        if fx == 0.0 && fy == 0.0 && fz == 0.0 {
            continue;
        }
        if let Ok(mut f) = forces.get_mut(entity) {
            f.apply_force(DVec3::new(fx, fy, fz));
        }
    }
}
