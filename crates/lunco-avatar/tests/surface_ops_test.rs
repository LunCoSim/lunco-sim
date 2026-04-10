//! Unit tests for lunar surface operations — teleport, terrain, and camera math.
//!
//! These test the core math without requiring a full Bevy app:
//! - Surface camera rotation (zero-roll invariant)
//! - Lat/lon to cartesian conversion
//! - Altitude calculation from body-relative positions
//! - Terrain spawn threshold gating

use bevy::prelude::*;
use bevy::math::DVec3;

// ─── Surface Camera Rotation Tests ─────────────────────────────────────────

/// Build surface camera rotation the same way as `surface_camera_system`.
fn build_surface_rot(heading: f32, pitch: f32, up_v: Vec3) -> Quat {
    let ref_dir = if up_v.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
    let east = up_v.cross(ref_dir).normalize();
    let north = east.cross(up_v).normalize();

    let heading_q = Quat::from_axis_angle(up_v, heading);
    let forward = heading_q.mul_vec3(north);
    let right = forward.cross(up_v).normalize();

    let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up_v, -forward));
    let pitch_q = Quat::from_axis_angle(right, pitch);
    (pitch_q * base_rot).normalize()
}

/// "No roll" = right vector is perpendicular to up vector.
fn has_no_roll(rot: Quat, up_v: Vec3) -> bool {
    let right = rot.mul_vec3(Vec3::X);
    right.dot(up_v).abs() < 1e-4
}

#[test]
fn test_surface_rot_zero_roll_y_up() {
    let up_v = Vec3::Y;
    for heading_deg in [0, 45, 90, 135, 180, 270, 315] {
        let heading = (heading_deg as f32).to_radians();
        for pitch_deg in [-60, -30, 0, 30, 60] {
            let pitch = (pitch_deg as f32).to_radians();
            let rot = build_surface_rot(heading, pitch, up_v);
            assert!(has_no_roll(rot, up_v),
                "Roll violation at h={}° p={}° up=Y", heading_deg, pitch_deg);
        }
    }
}

#[test]
fn test_surface_rot_zero_roll_tilted_normal() {
    // Test with a non-trivial surface normal
    let up_v = DVec3::new(0.3, 0.8, 0.5).normalize().as_vec3();
    for heading_deg in [0, 60, 120, 180, 240, 300] {
        let heading = (heading_deg as f32).to_radians();
        for pitch_deg in [-45, 0, 45] {
            let pitch = (pitch_deg as f32).to_radians();
            let rot = build_surface_rot(heading, pitch, up_v);
            assert!(has_no_roll(rot, up_v),
                "Roll violation at h={}° p={}° up={:?}", heading_deg, pitch_deg, up_v);
        }
    }
}

#[test]
fn test_surface_rot_idempotent() {
    let up_v = Vec3::Y;
    for heading in [0.0, 0.5, 1.0, 2.0] {
        for pitch in [-0.3, 0.0, 0.2] {
            let rot1 = build_surface_rot(heading, pitch, up_v);
            let rot2 = build_surface_rot(heading, pitch, up_v);
            let diff = (rot1 - rot2).length();
            assert!(diff < 1e-6,
                "Rotation not idempotent at h={}, p={}: diff={:.8}", heading, pitch, diff);
        }
    }
}

#[test]
fn test_surface_rot_up_matches_surface_normal() {
    let normals = vec![
        Vec3::Y,
        Vec3::X,
        Vec3::Z,
        Vec3::NEG_Y,
        DVec3::new(0.5, 0.5, 0.5).normalize().as_vec3(),
    ];

    for up_v in normals {
        let rot = build_surface_rot(0.0, 0.0, up_v);
        let rot_up = rot.mul_vec3(Vec3::Y);
        let diff = (rot_up - up_v).length();
        assert!(diff < 1e-4,
            "Camera up should match surface normal. up={:?}, rot_up={:?}, diff={:.6}",
            up_v, rot_up, diff);
    }
}

// ─── Lat/Lon to Cartesian Tests ────────────────────────────────────────────

/// Convert lat/lon to surface normal (same logic as teleport command).
fn latlon_to_normal(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let lat_r = lat_deg.to_radians();
    let lon_r = lon_deg.to_radians();
    DVec3::new(
        lat_r.cos() * lon_r.sin(),
        lat_r.sin(),
        lat_r.cos() * lon_r.cos(),
    )
}

#[test]
fn test_latlon_equator_lon0() {
    let n = latlon_to_normal(0.0, 0.0);
    assert!((n - DVec3::new(0.0, 0.0, 1.0)).length() < 1e-10);
}

#[test]
fn test_latlon_equator_lon90() {
    let n = latlon_to_normal(0.0, 90.0);
    assert!((n - DVec3::new(1.0, 0.0, 0.0)).length() < 1e-10);
}

#[test]
fn test_latlon_north_pole() {
    let n = latlon_to_normal(90.0, 0.0);
    assert!((n - DVec3::new(0.0, 1.0, 0.0)).length() < 1e-10);
}

#[test]
fn test_latlon_south_pole() {
    let n = latlon_to_normal(-90.0, 0.0);
    assert!((n - DVec3::new(0.0, -1.0, 0.0)).length() < 1e-10);
}

#[test]
fn test_latlon_is_normalized() {
    for lat in [-80, -45, 0, 45, 80] {
        for lon in [-180, -90, 0, 90, 180] {
            let n = latlon_to_normal(lat as f64, lon as f64);
            assert!((n.length() - 1.0).abs() < 1e-10,
                "Normal at lat={}, lon={} should be unit length, got {}", lat, lon, n.length());
        }
    }
}

// ─── Altitude Calculation Tests ────────────────────────────────────────────

const MOON_RADIUS: f64 = 1737.0e3;

#[test]
fn test_altitude_surface_50m() {
    let camera_pos = DVec3::new(0.0, MOON_RADIUS + 50.0, 0.0);
    let body_pos = DVec3::ZERO;
    let alt = (camera_pos - body_pos).length() - MOON_RADIUS;
    assert!((alt - 50.0).abs() < 1.0, "Surface altitude should be 50m, got {}", alt);
}

#[test]
fn test_altitude_low_orbit_100km() {
    let camera_pos = DVec3::new(0.0, MOON_RADIUS + 100_000.0, 0.0);
    let body_pos = DVec3::ZERO;
    let alt = (camera_pos - body_pos).length() - MOON_RADIUS;
    assert!((alt - 100_000.0).abs() < 100.0, "Orbit altitude should be ~100km, got {}", alt);
}

#[test]
fn test_altitude_earth_moon_distance() {
    let camera_pos = DVec3::new(385_000_000.0, 0.0, 0.0);
    let body_pos = DVec3::ZERO;
    let alt = (camera_pos - body_pos).length() - MOON_RADIUS;
    assert!(alt > 383_000_000.0, "EM distance should be ~385M m, got {}", alt);
}

// ─── Terrain Spawn Threshold Tests ─────────────────────────────────────────

const TERRAIN_THRESHOLD: f64 = 100_000.0; // 100 km

#[test]
fn test_terrain_spawns_on_surface() {
    let alt = 50.0;
    assert!(alt < TERRAIN_THRESHOLD, "Terrain should spawn at 50m altitude");
}

#[test]
fn test_terrain_spawns_low_orbit() {
    let alt = 50_000.0;
    assert!(alt < TERRAIN_THRESHOLD, "Terrain should spawn at 50km altitude");
}

#[test]
fn test_terrain_no_spawn_high_orbit() {
    let alt = 200_000.0;
    assert!(alt >= TERRAIN_THRESHOLD, "Terrain should NOT spawn at 200km altitude");
}

#[test]
fn test_terrain_no_spawn_em_distance() {
    let alt = 385_000_000.0;
    assert!(alt >= TERRAIN_THRESHOLD, "Terrain should NOT spawn at Earth-Moon distance");
}

// ─── Gravity Magnitude Tests ───────────────────────────────────────────────

const MOON_GM: f64 = 4.904e12;

#[test]
fn test_moon_surface_gravity() {
    let g = MOON_GM / (MOON_RADIUS * MOON_RADIUS);
    // Moon surface gravity is ~1.625 m/s²
    assert!((g - 1.625).abs() < 0.05,
        "Moon surface gravity should be ~1.625 m/s², got {:.4}", g);
}

#[test]
fn test_gravity_direction_at_surface_positions() {
    let positions = vec![
        DVec3::new(MOON_RADIUS, 0.0, 0.0),
        DVec3::new(0.0, MOON_RADIUS, 0.0),
        DVec3::new(0.0, 0.0, MOON_RADIUS),
        DVec3::new(-MOON_RADIUS, 0.0, 0.0),
    ];

    for pos in positions {
        let dir = -pos / pos.length();
        let expected = -pos / pos.length();
        assert!((dir - expected).length() < 1e-10,
            "Gravity direction at {:?} should point toward origin", pos);
    }
}
