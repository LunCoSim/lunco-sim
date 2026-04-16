//! Apply simulation forces to Avian.
//!
//! For kinematic balloons, we integrate velocity from Modelica netForce and
//! write it to `LinearVelocity`. Avian's `integrate_positions` then advances
//! `Position` from velocity each step. We intentionally do NOT write Position
//! directly — doing so would block `transform_to_position` from picking up
//! gizmo drags that update Transform.

use bevy::prelude::*;
use avian3d::prelude::{RigidBody, LinearVelocity};

use crate::{SimComponent, AvianSim};

/// Custom velocity component for kinematic balloons.
///
/// KinematicPositionBased bodies don't have LinearVelocity in Avian,
/// so we track it ourselves and integrate to Position directly.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct BalloonVelocity(pub Vec3);

/// System sets for applying forces.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Apply SimComponent outputs as Avian forces.
    ApplyForces,
}

/// Applies balloon physics from Modelica outputs to Avian LinearVelocity.
///
/// - Reads netForce from `AvianSim::inputs` (delivered by propagate_wires)
/// - Integrates acceleration: `dv = F/m * dt`
/// - Writes total velocity to `LinearVelocity`, which Avian's `integrate_positions`
///   then uses to advance `Position`
///
/// We intentionally don't write `Position` directly — that would trigger change
/// detection on Position every frame and block `transform_to_position` from
/// picking up external Transform changes (gizmo drags, etc.).
pub fn apply_sim_forces(
    q_components: Query<&SimComponent>,
    mut q_kinematic: Query<Entity, With<RigidBody>>,
    mut q_balloon_vel: Query<&mut BalloonVelocity>,
    mut q_avian: Query<&mut AvianSim>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    time: Res<Time<Fixed>>,
) {
    let dt = time.delta_secs_f64();

    for entity in &mut q_kinematic {
        // Only apply balloon physics if this entity has a Balloon SimComponent
        let is_balloon = q_components
            .get(entity)
            .map(|c| c.model_name == "Balloon")
            .unwrap_or(false);
        if !is_balloon {
            continue;
        }

        let mut avian = match q_avian.get_mut(entity) {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Take inputs (clears them for next frame)
        let forces = avian.take_inputs();
        let net_force_y = forces[1]; // index 1 is force_y

        // Get mass from SimComponent parameters (default 2.0 if missing)
        let mass = q_components.get(entity)
            .ok()
            .and_then(|c| c.parameters.get("mass").copied())
            .unwrap_or(2.0);

        // Read current velocity from LinearVelocity so Avian contact impulses
        // (from the previous step's constraint solve) are carried into the
        // next force integration.
        let mut vel_y = q_lin_vel.get(entity).map(|v| v.0.y).unwrap_or(0.0);

        // v_new = v + F/m * dt
        let accel_y = net_force_y / mass;
        vel_y += accel_y * dt;

        // Clamp to prevent runaway
        vel_y = vel_y.clamp(-100.0, 100.0);

        // Mirror into BalloonVelocity for any external consumers / diagnostics
        if let Ok(mut bv) = q_balloon_vel.get_mut(entity) {
            bv.0.y = vel_y as f32;
        }

        // Write to LinearVelocity so Avian's integrate_positions advances Position.
        // Also write to AvianSim.outputs for same-frame consumers before
        // read_avian_outputs runs.
        if let Ok(mut lin_vel) = q_lin_vel.get_mut(entity) {
            lin_vel.0.y = vel_y;
        }
        avian.outputs.insert("velocity_y".into(), vel_y);

        debug!(
            "apply_sim_forces entity={:?} net_force_y={:.4} accel_y={:.4} vel_y={:.4} mass={:.2}",
            entity, net_force_y, accel_y, vel_y, mass
        );
    }
}