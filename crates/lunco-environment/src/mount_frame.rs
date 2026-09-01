//! Coordinate conversion for mechanisms that track a world-space direction.
//!
//! A tracker command is always expressed in the frame that owns its joint axes.
//! Sending a world bearing to a mount-frame joint happens to work only while the
//! host is level and north-aligned; it is not a coordinate contract.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::Vec3;

/// Express a direction in a mount's local axes while retaining f64 precision
/// through the BigSpace pose boundary. The final conversion to `Vec3` is the
/// authored mechanism port boundary, not a coordinate-composition boundary.
pub(crate) fn direction_in_mount_rotation(world_direction: DVec3, mount_rotation: DQuat) -> Vec3 {
    (mount_rotation.inverse() * world_direction).as_vec3()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_mount_rotation_not_just_heading_defines_tracker_frame() {
        let world_north = Vec3::NEG_Z;
        let mount = DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2);
        let local = direction_in_mount_rotation(world_north.as_dvec3(), mount);
        assert!(
            local.abs_diff_eq(Vec3::NEG_Y, 1e-5),
            "pitch must change the mount-local target, got {local:?}"
        );
    }
}
