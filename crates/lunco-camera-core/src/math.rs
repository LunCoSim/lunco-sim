//! Pure camera math shared by camera runtimes.
//!
//! These functions operate on camera contracts and engine math types only.
//! They do not select an avatar, read a scene, or apply a product-specific
//! input policy; callers provide those decisions at their own boundary.

use bevy::math::{Mat3, Quat, Vec3};
use bevy::prelude::Transform;

const ZOOM_FACTOR_MIN: f64 = 0.75;
const ZOOM_FACTOR_MAX: f64 = 1.25;

/// Resolve the shared camera decay rate from an authored base rate and
/// per-camera damping.
#[inline]
pub fn camera_decay_rate(rate: f32, damping: f32) -> f32 {
    rate * (1.0 - damping)
}

/// Return the frame-rate-independent interpolation alpha for camera motion.
#[inline]
pub fn camera_decay_alpha(rate: f32, damping: f32, dt: f32) -> f64 {
    f64::from(1.0 - (-camera_decay_rate(rate, damping) * dt).exp())
}

/// Resolve a camera arm length, easing only when an obstacle is present.
#[inline]
pub fn resolve_camera_arm_length(
    current_len: f64,
    target_len: f64,
    obstacle_present: bool,
    position_rate: f32,
    damping: f32,
    dt: f32,
) -> f64 {
    if !obstacle_present || current_len < 1e-3 {
        return target_len;
    }

    let alpha = camera_decay_alpha(position_rate, damping, dt);
    current_len + (target_len - current_len) * alpha
}

/// Convert normalized scroll input into a bounded multiplicative zoom factor.
#[inline]
pub fn zoom_factor(scroll_delta: f32, sensitivity: f32) -> f64 {
    (-f64::from(scroll_delta) * f64::from(sensitivity) * 0.01)
        .exp()
        .clamp(ZOOM_FACTOR_MIN, ZOOM_FACTOR_MAX)
}

/// Apply an accumulated scroll delta to a camera arm and consume the delta.
pub fn apply_scroll_zoom(
    distance: &mut f64,
    scroll_delta: &mut f32,
    sensitivity: f32,
    min_distance: f64,
    max_distance: f64,
) {
    if *scroll_delta != 0.0 {
        *distance =
            (*distance * zoom_factor(*scroll_delta, sensitivity)).clamp(min_distance, max_distance);
        *scroll_delta = 0.0;
    }
}

/// Build a surface-relative camera orientation from an ENU frame.
pub fn surface_camera_rotation(
    east: Vec3,
    north: Vec3,
    up: Vec3,
    heading: f32,
    pitch: f32,
) -> Quat {
    let (_, north, up) = orthonormal_surface_axes(east, north, up);
    let heading_q = Quat::from_axis_angle(up, heading);
    let forward = heading_q.mul_vec3(north);
    let right = forward.cross(up).normalize();
    let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    let pitch_q = Quat::from_axis_angle(right, pitch);
    (pitch_q * base_rot).normalize()
}

/// Compose a camera-relative movement vector using an explicit vertical axis.
pub fn camera_move_direction(
    transform: &Transform,
    forward: f32,
    side: f32,
    elevation: f32,
    up_direction: Vec3,
) -> Vec3 {
    let up_direction = up_direction.normalize_or_zero();
    let up_direction = if up_direction == Vec3::ZERO {
        Vec3::Y
    } else {
        up_direction
    };
    let direction =
        *transform.forward() * forward + *transform.right() * side + up_direction * elevation;
    let length_squared = direction.length_squared();
    if length_squared > 1.0 {
        direction / length_squared.sqrt()
    } else {
        direction
    }
}

/// Decompose a camera rotation into surface-frame heading and pitch.
pub fn surface_camera_angles(east: Vec3, north: Vec3, up: Vec3, rotation: Quat) -> (f32, f32) {
    let (east, north, up) = orthonormal_surface_axes(east, north, up);
    let forward = rotation * Vec3::NEG_Z;
    let pitch = forward.dot(up).clamp(-1.0, 1.0).asin();
    let tangent_forward = (forward - up * forward.dot(up)).normalize_or(north);
    let heading = (-tangent_forward.dot(east)).atan2(tangent_forward.dot(north));
    (heading, pitch)
}

fn orthonormal_surface_axes(east: Vec3, north: Vec3, up: Vec3) -> (Vec3, Vec3, Vec3) {
    let east = east.normalize_or(Vec3::X);
    let up = up.normalize_or(Vec3::Y);
    let north = up.cross(east).normalize_or(north.normalize_or(Vec3::NEG_Z));
    (east, north, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_zoom_limits_one_frame_to_a_safe_factor() {
        let mut distance = 100.0;
        let mut delta = -10_000.0;
        apply_scroll_zoom(&mut distance, &mut delta, 5.0, 1.0, 1_000.0);
        assert_eq!(distance, 125.0);

        let mut distance = 100.0;
        let mut delta = 10_000.0;
        apply_scroll_zoom(&mut distance, &mut delta, 5.0, 1.0, 1_000.0);
        assert_eq!(distance, 75.0);
        assert_eq!(delta, 0.0);
    }

    #[test]
    fn clear_follow_ray_keeps_requested_distance_when_target_moves() {
        let final_len = resolve_camera_arm_length(47.0, 50.0, false, 30.0, 0.1, 1.0 / 60.0);
        assert_eq!(final_len, 50.0);
    }

    #[test]
    fn obstructed_follow_ray_eases_arm_length() {
        let final_len = resolve_camera_arm_length(50.0, 20.0, true, 30.0, 0.1, 1.0 / 60.0);
        assert!(final_len < 50.0);
        assert!(final_len > 20.0);
    }

    #[test]
    fn pitched_movement_keeps_forward_and_vertical_components() {
        let transform =
            Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4));
        let direction = camera_move_direction(&transform, 1.0, 0.0, -1.0, Vec3::Y);

        assert!(direction.z < -0.3);
        assert!(direction.y < -0.5);
        assert!(direction.length() <= 1.0 + 1e-6);
    }
}
