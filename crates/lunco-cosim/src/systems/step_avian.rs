//! Avian output-reading system.
//!
//! Reads Avian physics state into [`AvianSim::outputs`] after each physics step.
//! Avian itself is stepped by its own `PhysicsPlugins` in `FixedPostUpdate` —
//! we do not manually advance `Time<Physics>` or run `PhysicsSchedule` here,
//! which would cause double-stepping on top of Avian's built-in pipeline.

use bevy::prelude::*;
use avian3d::prelude::*;

use crate::AvianSim;

/// Reads Avian state into [`AvianSim::outputs`].
///
/// Runs in [`FixedPostUpdate`] after [`PhysicsSystems::Writeback`], so Position
/// and LinearVelocity reflect the physics step that just completed.
///
/// ## Output Mapping
///
/// - `position_x`, `position_y`, `position_z` → from [`Position`]
/// - `velocity_x`, `velocity_y`, `velocity_z` → from [`LinearVelocity`] (Dynamic only)
/// - `height` → alias for `position_y`
pub fn read_avian_outputs(
    mut q_avian: Query<(Entity, &mut AvianSim, Option<&Position>, Option<&LinearVelocity>)>,
) {
    for (_entity, mut avian, position, linear_velocity) in &mut q_avian {
        avian.read_state(position, linear_velocity);
    }
}
