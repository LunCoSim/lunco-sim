//! Tests for the surface_camera_system rotation math.
//!
//! The surface camera builds rotation from scratch every frame using:
//!   1. Pick a reference direction (world Y unless parallel to up)
//!   2. east = up × ref_dir (normalized)
//!   3. north = east × up
//!   4. heading_q rotates north around up by heading angle
//!   5. forward = heading_q(north)
//!   6. right = forward × up
//!   7. base_rot = mat3(right, up, -forward)
//!   8. pitch_q rotates around right by pitch angle
//!   9. final = pitch_q × base_rot

use bevy::math::{Quat, Vec3, DVec3};

fn get_up(rot: Quat) -> Vec3 {
    rot.mul_vec3(Vec3::Y)
}

fn get_right(rot: Quat) -> Vec3 {
    rot.mul_vec3(Vec3::X)
}

fn surface_rot(heading: f32, pitch: f32, up_v: Vec3) -> Quat {
    let ref_dir = if up_v.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
    let east = up_v.cross(ref_dir).normalize();
    let north = east.cross(up_v).normalize();

    let heading_q = Quat::from_axis_angle(up_v, heading);
    let forward = heading_q.mul_vec3(north);
    let right = forward.cross(up_v).normalize();

    let base_rot = Quat::from_mat3(&bevy::prelude::Mat3::from_cols(right, up_v, -forward));
    let pitch_q = Quat::from_axis_angle(right, pitch);
    (pitch_q * base_rot).normalize()
}

fn has_no_roll(rot: Quat, up_v: Vec3) -> bool {
    // "No roll" means the right vector lies in the tangent plane (perpendicular to up).
    get_right(rot).dot(up_v).abs() < 1e-5
}

#[test]
fn test_surface_camera_zero_heading_zero_pitch() {
    let up_v = Vec3::Y;
    let rot = surface_rot(0.0, 0.0, up_v);
    assert!(has_no_roll(rot, up_v), "Zero heading/pitch should have zero roll");
    assert!((get_up(rot) - up_v).length() < 1e-5, "Up should match surface normal");
}

#[test]
fn test_surface_camera_yaw_preserves_zero_roll() {
    let up_v = Vec3::Y;
    for &h in &[0.0, 0.5, 1.0, 2.0, -1.0, std::f32::consts::PI] {
        let rot = surface_rot(h, 0.0, up_v);
        assert!(has_no_roll(rot, up_v), "Yaw of {} should preserve zero roll. dot={:.8}", h, get_right(rot).dot(up_v));
    }
}

#[test]
fn test_surface_camera_pitch_preserves_zero_roll() {
    let up_v = Vec3::Y;
    for &h in &[0.0, 1.0, 2.0] {
        for &p in &[0.0, 0.3, -0.5, 1.0] {
            let rot = surface_rot(h, p, up_v);
            assert!(has_no_roll(rot, up_v), "heading={}, pitch={} should preserve zero roll. dot={:.8}", h, p, get_right(rot).dot(up_v));
        }
    }
}

#[test]
fn test_surface_camera_tilted_surface_zero_roll() {
    // Test with a tilted surface normal (not Y)
    let up_v = DVec3::new(0.3, 0.8, 0.5).normalize().as_vec3();
    for &h in &[0.0, 0.5, 1.5] {
        for &p in &[0.0, 0.2, -0.3] {
            let rot = surface_rot(h, p, up_v);
            assert!(has_no_roll(rot, up_v), "Tilted surface h={}, p={} should preserve zero roll. dot={:.8}", h, p, get_right(rot).dot(up_v));
        }
    }
}

#[test]
fn test_surface_camera_combined_heading_pitch_tilted() {
    // Thorough test: many heading/pitch combos on a tilted surface
    let up_v = DVec3::new(0.6, 0.6, 0.4).normalize().as_vec3();
    for i in 0..20 {
        let h = (i as f32) * 0.35;
        let p = (i as f32) * 0.12 - 0.6;
        let rot = surface_rot(h, p, up_v);
        assert!(has_no_roll(rot, up_v), "h={:.2}, p={:.2} on tilted surface: dot={:.10}", h, p, get_right(rot).dot(up_v));
    }
}

#[test]
fn test_surface_camera_consistent_across_frames() {
    // Simulate multiple frames: each frame recomputes from scratch
    let up_v = Vec3::Y;
    let mut prev_rot: Option<Quat> = None;

    for frame in 0..50 {
        let h = (frame as f32) * 0.07;
        let p = (frame as f32) * 0.02 - 0.5;
        let rot = surface_rot(h, p, up_v);

        assert!(has_no_roll(rot, up_v), "Frame {}: zero roll violated", frame);

        // Smooth transition between frames (no sudden jumps).
        if let Some(prev) = prev_rot {
            let angle = prev.angle_between(rot);
            assert!(angle < 0.1, "Frame {}: sudden jump of {:.4} rad from previous frame", frame, angle);
        }
        prev_rot = Some(rot);
    }
}

#[test]
fn test_surface_camera_pole_fallback() {
    // At the pole (up = Y), the ref_dir should be Z
    let up_v = Vec3::Y;
    let rot = surface_rot(0.0, 0.0, up_v);
    assert!(has_no_roll(rot, up_v), "Pole: zero roll");
    assert!((get_up(rot) - up_v).length() < 1e-5, "Pole: up matches");
}
