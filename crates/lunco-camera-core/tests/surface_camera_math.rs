//! Pure invariants for the reusable surface-camera frame conversion.

use bevy::math::{Quat, Vec3};
use lunco_camera_core::math::{surface_camera_angles, surface_camera_rotation};

fn surface_frame(up: Vec3) -> (Vec3, Vec3, Vec3) {
    let frame = Quat::from_rotation_arc(Vec3::Y, up);
    (frame * Vec3::X, frame * Vec3::NEG_Z, up)
}

#[test]
fn surface_rotation_preserves_up_and_has_no_roll() {
    for up in [
        Vec3::Y,
        Vec3::new(0.3, 0.8, 0.5).normalize(),
        Vec3::new(0.6, 0.6, 0.4).normalize(),
    ] {
        let (east, north, up) = surface_frame(up);
        for heading in [0.0, 0.5, 1.5, -1.0] {
            for pitch in [-0.5, 0.0, 0.4] {
                let rotation = surface_camera_rotation(east, north, up, heading, pitch);
                let right = rotation * Vec3::X;
                assert!(right.dot(up).abs() < 1e-5);
                if pitch == 0.0 {
                    assert!((rotation * Vec3::Y - up).length() < 1e-5);
                }
            }
        }
    }
}

#[test]
fn surface_angles_round_trip_through_authored_frame() {
    let (east, north, up) = surface_frame(Vec3::new(0.6, 0.6, 0.4).normalize());
    for heading in [-2.0, -0.5, 0.0, 0.75, 2.4] {
        for pitch in [-0.6, -0.2, 0.0, 0.4] {
            let rotation = surface_camera_rotation(east, north, up, heading, pitch);
            let (decoded_heading, decoded_pitch) = surface_camera_angles(east, north, up, rotation);
            assert!((decoded_heading - heading).abs() < 1e-5);
            assert!((decoded_pitch - pitch).abs() < 1e-5);
        }
    }
}
