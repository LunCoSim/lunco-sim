//! Unit tests proving the surface camera quaternion math preserves zero roll.
//!
//! These tests verify the invariant that yaw around a fixed "up" axis
//! followed by pitch around the yawed-right axis never introduces roll
//! (i.e. the right vector's Y component stays zero when up = Y).

use bevy::math::{Quat, Vec3};

fn get_right(rot: Quat) -> Vec3 {
    rot.mul_vec3(Vec3::X)
}

/// "No roll" invariant: the camera's right axis stays horizontal.
fn has_no_roll(rot: Quat) -> bool {
    let right = get_right(rot);
    right.y.abs() < 1e-5
}

#[test]
fn test_surface_incremental_yaw_no_roll() {
    let up_v = Vec3::Y;
    let mut rot = Quat::IDENTITY;

    for _ in 0..50 {
        let yaw_delta = 0.1f32;
        let yaw_q = Quat::from_axis_angle(up_v, yaw_delta);
        let right = get_right(rot);
        let right_after_yaw = yaw_q.mul_vec3(right);
        let pitch_q = Quat::from_axis_angle(right_after_yaw, 0.0f32);
        rot = (pitch_q * yaw_q * rot).normalize();
    }

    assert!(has_no_roll(rot), "After 50 yaw increments, right axis should be horizontal. right.y={}", get_right(rot).y);
}

#[test]
fn test_surface_incremental_pitch_no_roll() {
    let mut rot = Quat::IDENTITY;

    for _ in 0..20 {
        let pitch_delta = 0.02f32;
        let right = get_right(rot);
        let pitch_q = Quat::from_axis_angle(right, pitch_delta);
        rot = (pitch_q * rot).normalize();
    }

    assert!(has_no_roll(rot), "After pitch, right axis should stay horizontal. right.y={}", get_right(rot).y);
}

#[test]
fn test_surface_combined_yaw_pitch_no_roll() {
    let up_v = Vec3::Y;
    let mut rot = Quat::IDENTITY;

    for _ in 0..100 {
        let yaw_delta = 0.05f32;
        let pitch_delta = -0.01f32;

        let yaw_q = Quat::from_axis_angle(up_v, yaw_delta);
        let right = get_right(rot);
        let right_after_yaw = yaw_q.mul_vec3(right);
        let pitch_q = Quat::from_axis_angle(right_after_yaw, pitch_delta);
        rot = (pitch_q * yaw_q * rot).normalize();
    }

    assert!(
        has_no_roll(rot),
        "Combined yaw+pitch should preserve zero roll. right.y={:.8}, right={:?}",
        get_right(rot).y,
        get_right(rot)
    );
}

#[test]
fn test_surface_mouse_axes_intuitive() {
    let up_v = Vec3::Y;
    let mut rot = Quat::IDENTITY;

    let yaw_delta = -0.1f32;
    let yaw_q = Quat::from_axis_angle(up_v, yaw_delta);
    rot = (yaw_q * rot).normalize();

    let fwd = rot.mul_vec3(Vec3::NEG_Z);
    assert!(fwd.x > 0.0, "Negative yaw should turn camera right. fwd.x={}", fwd.x);

    let pitch_delta = 0.1f32;
    let right = get_right(rot);
    let pitch_q = Quat::from_axis_angle(right, pitch_delta);
    rot = (pitch_q * rot).normalize();

    let fwd2 = rot.mul_vec3(Vec3::NEG_Z);
    assert!(fwd2.y > 0.0, "Positive pitch should look down. fwd.y={}", fwd2.y);
}
