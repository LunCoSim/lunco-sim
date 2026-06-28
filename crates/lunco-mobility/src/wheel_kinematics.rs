//! # Wheel kinematics — frame-safe hub pose / velocity / roll-rate
//!
//! luncosim runs avian3d physics under `big_space` floating-origin, so **two
//! position frames coexist and must never be mixed in one arithmetic
//! expression**:
//!
//! - **Render frame** (`GlobalTransform::translation()`): origin-rebased — the
//!   world is periodically re-centred on the camera's grid cell.
//! - **Avian cell-local frame** (`Position.0` / `Rotation.0`, ==
//!   `Forces::position()/rotation()`): the physics solver's own coordinates,
//!   relative to the body's grid cell, *not* the render origin.
//!
//! Near the origin the two coincide, so a frame-mix bug is invisible in local
//! testing and only appears once a rover drives ~km away (the CQ-201 bug class).
//! `AngularVelocity` is frame-**orientation** independent (big_space only
//! *translates* the origin, never rotates), so angular velocity is safe in
//! either frame — **only positions / lever-arms carry the frame.**
//!
//! **The invariant:** a lever arm `hub − chassis_centre` must have **both**
//! terms in the **same** frame. These helpers operate entirely in the avian
//! cell-local frame by reconstructing the hub from the chassis body pose plus
//! the wheel's chassis-local transform — never from `GlobalTransform`.

use bevy::math::{DQuat, DVec3};

/// World pose of a wheel hub in the **avian cell-local frame**, reconstructed
/// from the chassis body pose and the wheel's chassis-local transform.
///
/// `chassis_pos` / `chassis_rot` are avian `Position.0` / `Rotation.0`
/// (== `Forces::position()/rotation()`); `wheel_local_*` is the wheel entity's
/// `Transform` relative to the chassis. **Never feed this `GlobalTransform`** —
/// that mixes the render frame in and reintroduces CQ-201.
#[inline]
pub fn wheel_hub_pose(
    chassis_pos: DVec3,
    chassis_rot: DQuat,
    wheel_local_pos: DVec3,
    wheel_local_rot: DQuat,
) -> (DVec3, DQuat) {
    (
        chassis_pos + chassis_rot * wheel_local_pos,
        chassis_rot * wheel_local_rot,
    )
}

/// Linear velocity of the hub: `v + ω × r`, where `r = hub_pos − chassis_pos`
/// is the lever arm.
///
/// **Both `hub_pos` and `chassis_pos` MUST be in the same (avian) frame** — this
/// is the CQ-201 invariant. `chassis_ang` is frame-safe (see module docs).
#[inline]
pub fn wheel_hub_velocity(
    chassis_lin: DVec3,
    chassis_ang: DVec3,
    hub_pos: DVec3,
    chassis_pos: DVec3,
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
        let rot = DQuat::from_rotation_z(FRAC_PI_2);
        let local = DVec3::new(1.0, 0.0, 0.0);
        let (near, _) = wheel_hub_pose(DVec3::ZERO, rot, local, DQuat::IDENTITY);
        let far_centre = DVec3::new(1_000_000.0, 0.0, 0.0);
        let (far, _) = wheel_hub_pose(far_centre, rot, local, DQuat::IDENTITY);
        approx(near - DVec3::ZERO, far - far_centre);
    }

    #[test]
    fn hub_velocity_adds_rotational_term() {
        // Chassis spinning about +Z at 1 rad/s, hub 1 m out along +X → the hub
        // moves at 1 m/s along +Y (ω × r), plus any chassis linear velocity.
        let lin = DVec3::new(2.0, 0.0, 0.0);
        let ang = DVec3::Z;
        let hub = DVec3::new(1.0, 0.0, 0.0);
        let v = wheel_hub_velocity(lin, ang, hub, DVec3::ZERO);
        approx(v, DVec3::new(2.0, 1.0, 0.0));
    }

    #[test]
    fn roll_rate_is_v_long_over_radius() {
        let forward = DVec3::NEG_Z;
        let hub_vel = DVec3::new(0.0, 0.0, -4.0); // 4 m/s forward
        assert!((wheel_roll_rate(hub_vel, forward, 2.0) - 2.0).abs() < 1e-9);
        // Radius is floored to avoid div-by-zero.
        assert!(wheel_roll_rate(hub_vel, forward, 0.0).is_finite());
    }
}
