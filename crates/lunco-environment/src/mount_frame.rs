//! Coordinate conversion for mechanisms that track a world-space direction.
//!
//! A tracker command is always expressed in the frame that owns its joint axes.
//! Sending a world bearing to a mount-frame joint happens to work only while the
//! host is level and north-aligned; it is not a coordinate contract.

use bevy::prelude::{GlobalTransform, Vec3};

/// Express a world-space direction in an authored mount's local axes.
///
/// The mount convention is `+X` right, `+Y` up, `-Z` forward.  Rotation is the
/// only relevant transform for a direction, so translation and scale cannot
/// leak into a pointing command.
pub(crate) fn direction_in_mount_frame(world_direction: Vec3, mount: &GlobalTransform) -> Vec3 {
    mount.rotation().inverse() * world_direction
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{Quat, Transform};

    #[test]
    fn full_mount_rotation_not_just_heading_defines_tracker_frame() {
        let world_north = Vec3::NEG_Z;
        let mount = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_x(
            std::f32::consts::FRAC_PI_2,
        )));
        let local = direction_in_mount_frame(world_north, &mount);
        assert!(
            local.abs_diff_eq(Vec3::NEG_Y, 1e-5),
            "pitch must change the mount-local target, got {local:?}"
        );
    }
}
