//! # Wheel kinematics — frame-safe hub pose / velocity / roll-rate
//!
//! luncosim runs avian3d physics under `big_space` floating-origin, so two
//! position frames coexist: the origin-rebased **render frame**
//! (`GlobalTransform::translation()`) and the **grid-absolute** frame avian
//! `Position.0` / `Rotation.0` carry. Near the origin they coincide, so a
//! frame-mix bug is invisible in local testing and only appears once a rover
//! drives ~km away (the CQ-201 bug class). The [`GridPos`] parameter types
//! below make that mix a compile error: a render-frame `GlobalTransform`
//! translation has no `GridPos` and cannot be fed in.
//!
//! `AngularVelocity` is frame-**orientation** independent (big_space only
//! *translates* the origin, never rotates), so angular velocity is safe in
//! either frame — only positions / lever-arms carry the frame, and
//! `GridPos − GridPos` is the one legal way to build a lever arm.

use bevy::math::{DQuat, DVec3};
use lunco_core::coords::{GridPos, GridRot, VehicleFrame};

/// World pose of a wheel hub in the grid-absolute physics frame, reconstructed
/// from the chassis body pose and the wheel's chassis-local transform.
///
/// `chassis_pos` / `chassis_rot` wrap avian `Position.0` / `Rotation.0`
/// (== `Forces::position()/rotation()`); `wheel_local_*` is the wheel entity's
/// `Transform` relative to the chassis (body-local, so bare).
#[inline]
pub fn wheel_hub_pose(
    chassis_pos: GridPos,
    chassis_rot: GridRot,
    wheel_local_pos: DVec3,
    wheel_local_rot: DQuat,
) -> (GridPos, GridRot) {
    (
        chassis_pos + chassis_rot.0 * wheel_local_pos,
        GridRot(chassis_rot.0 * wheel_local_rot),
    )
}

/// Return the wheel's authored traction axes in Avian's grid-absolute physics
/// frame.  `chassis_rot` is the rigid body's attitude and `wheel_local_rot` is
/// the wheel's chassis-local (including steering) attitude.  The render tree is
/// deliberately not an input: BigSpace may rebase that tree without changing
/// the physical orientation of the body.
#[inline]
pub fn wheel_heading(chassis_rot: GridRot, wheel_local_rot: DQuat) -> (DVec3, DVec3) {
    let wheel_rot = chassis_rot.0 * wheel_local_rot;
    let wheel_frame = GridRot(wheel_rot);
    (
        VehicleFrame::forward(wheel_frame),
        VehicleFrame::right(wheel_frame),
    )
}

/// Linear velocity of the hub: `v + ω × r`, where `r = hub_pos − chassis_pos`
/// is the lever arm — both ends typed grid-absolute, so the CQ-201 invariant
/// (same frame on both terms) holds by construction. `chassis_ang` is
/// frame-safe (see module docs).
#[inline]
pub fn wheel_hub_velocity(
    chassis_lin: DVec3,
    chassis_ang: DVec3,
    hub_pos: GridPos,
    chassis_pos: GridPos,
) -> DVec3 {
    chassis_lin + chassis_ang.cross(hub_pos - chassis_pos)
}

/// Free-rolling axle rate ω (rad/s) for a wheel rolling on the ground at the
/// given hub velocity.
///
/// **Sign convention:** ω is `v_long / r` where `v_long = hub_vel · forward`
/// and `forward` is the wheel's forward travel axis (`wheel_rot · −Z`). Positive
/// ω therefore corresponds to forward travel. The mapping from ω to a *visual
/// mesh rotation* is the caller's job and depends on each wheel system's mesh
/// base/axle choice (e.g. the proxy `PhysicalWheel` applies a `ROLL_SIGN = −1`
/// against this convention to match its `axis_rot · Y` axle). Keep the visual
/// sign at the call site; do not bake it in here.
#[inline]
pub fn wheel_roll_rate(hub_vel: DVec3, forward: DVec3, radius: f64) -> f64 {
    hub_vel.dot(forward) / radius.max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::FRAC_PI_2;

    fn approx(a: DVec3, b: DVec3) {
        assert!((a - b).length() < 1e-9, "{a:?} != {b:?}");
    }

    #[test]
    fn hub_pose_is_translation_invariant_lever() {
        // The lever arm (hub − chassis) must be identical near origin and far
        // from it — this is the property the CQ-201 bug violated.
        let rot = GridRot(DQuat::from_rotation_z(FRAC_PI_2));
        let local = DVec3::new(1.0, 0.0, 0.0);
        let (near, _) = wheel_hub_pose(GridPos(DVec3::ZERO), rot, local, DQuat::IDENTITY);
        let far_centre = GridPos(DVec3::new(1_000_000.0, 0.0, 0.0));
        let (far, _) = wheel_hub_pose(far_centre, rot, local, DQuat::IDENTITY);
        approx(near - GridPos(DVec3::ZERO), far - far_centre);
    }

    #[test]
    fn hub_velocity_adds_rotational_term() {
        // Chassis spinning about +Z at 1 rad/s, hub 1 m out along +X → the hub
        // moves at 1 m/s along +Y (ω × r), plus any chassis linear velocity.
        let lin = DVec3::new(2.0, 0.0, 0.0);
        let ang = DVec3::Z;
        let hub = GridPos(DVec3::new(1.0, 0.0, 0.0));
        let v = wheel_hub_velocity(lin, ang, hub, GridPos(DVec3::ZERO));
        approx(v, DVec3::new(2.0, 1.0, 0.0));
    }

    #[test]
    fn roll_rate_is_v_long_over_radius() {
        let forward = VehicleFrame::FORWARD_LOCAL;
        let hub_vel = DVec3::new(0.0, 0.0, -4.0); // 4 m/s forward
        assert!((wheel_roll_rate(hub_vel, forward, 2.0) - 2.0).abs() < 1e-9);
        // Radius is floored to avoid div-by-zero.
        assert!(wheel_roll_rate(hub_vel, forward, 0.0).is_finite());
    }

    #[test]
    fn wheel_heading_uses_physics_attitude_not_render_frame() {
        // A site grid may rotate the physics frame relative to the renderer.
        // A wheel whose authored forward is -Z must therefore be transformed
        // by the Avian chassis attitude; using a rebased GlobalTransform would
        // incorrectly leave it in the renderer's -Z direction.
        let site_rotation = DQuat::from_rotation_y(core::f64::consts::FRAC_PI_2);
        let chassis_rot = GridRot(site_rotation);
        let (forward, right) = wheel_heading(chassis_rot, DQuat::IDENTITY);
        approx(forward, DVec3::NEG_X);
        approx(right, VehicleFrame::right(chassis_rot));
    }
}
