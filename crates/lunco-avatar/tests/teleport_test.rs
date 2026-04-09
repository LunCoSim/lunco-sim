//! Tests that verify the teleport math produces correct surface positions.
//!
//! Run with: cargo test -p lunco-avatar --test teleport_test -- --nocapture

use bevy::math::DVec3;
use big_space::prelude::*;

use lunco_celestial::CelestialBody;

const MOON_RADIUS: f64 = 1737.0e3;
const MOON_GRID_CELL_SIZE: f64 = 10_000.0;

/// Compute surface position the same way the teleport command does.
fn surface_pos(lat_deg: f64, lon_deg: f64, radius: f64, altitude: f64) -> DVec3 {
    let lat_r = lat_deg.to_radians();
    let lon_r = lon_deg.to_radians();
    DVec3::new(
        lat_r.cos() * lon_r.sin(),
        lat_r.sin(),
        lat_r.cos() * lon_r.cos(),
    ) * (radius + altitude)
}

/// Test: surface position at lat=0,lon=0 with 50m altitude is ~50m above surface.
#[test]
fn test_surface_altitude_50m() {
    let pos = surface_pos(0.0, 0.0, MOON_RADIUS, 50.0);
    let altitude = pos.length() - MOON_RADIUS;
    assert!(
        (altitude - 50.0).abs() < 1.0,
        "Altitude should be ~50m, got {:.2}m", altitude
    );
}

/// Test: surface position at lat=0,lon=0 points in +Z direction.
#[test]
fn test_surface_normal_lat0_lon0() {
    let pos = surface_pos(0.0, 0.0, MOON_RADIUS, 0.0);
    assert!(pos.x.abs() < 1.0);
    assert!(pos.y.abs() < 1.0);
    assert!(pos.z > MOON_RADIUS - 1.0);
}

/// Test: surface normal at north pole points in +Y.
#[test]
fn test_surface_normal_north_pole() {
    let pos = surface_pos(90.0, 0.0, MOON_RADIUS, 0.0);
    assert!(pos.x.abs() < 1.0);
    assert!(pos.y > MOON_RADIUS - 1.0);
    assert!(pos.z.abs() < 1.0);
}

/// Test: surface normal at south pole points in -Y.
#[test]
fn test_surface_normal_south_pole() {
    let pos = surface_pos(-90.0, 0.0, MOON_RADIUS, 0.0);
    assert!(pos.x.abs() < 1.0);
    assert!(pos.y < -(MOON_RADIUS - 1.0));
    assert!(pos.z.abs() < 1.0);
}

/// Test: grid decomposition + reconstruction preserves surface altitude.
/// This is the core invariant: after teleport, the camera's grid-local position
/// (CellCoord + Transform) must reconstruct to the same world position,
/// preserving the ~50m altitude above the body.
#[test]
fn test_grid_decomposition_preserves_altitude() {
    let pos = surface_pos(0.0, 0.0, MOON_RADIUS, 50.0);
    let grid = Grid::new(MOON_GRID_CELL_SIZE as f32, 1.0e30_f32);
    let (cell, local_tf) = grid.translation_to_grid(pos);

    // Reconstruct from cell + local
    let reconstructed = DVec3::new(
        cell.x as f64 * MOON_GRID_CELL_SIZE + local_tf.x as f64,
        cell.y as f64 * MOON_GRID_CELL_SIZE + local_tf.y as f64,
        cell.z as f64 * MOON_GRID_CELL_SIZE + local_tf.z as f64,
    );

    // Must match original position
    assert!(
        (reconstructed - pos).length() < 1.0,
        "Reconstruction mismatch: diff={:.2}", (reconstructed - pos).length()
    );

    // Altitude must be preserved
    let alt = reconstructed.length() - MOON_RADIUS;
    assert!(
        (alt - 50.0).abs() < 1.0,
        "Altitude not preserved: got {:.2}m (cell={:?}, local={:?})", alt, cell, local_tf
    );
}

/// Test: terrain altitude calculation from grid-local position.
/// When the camera is on the Body's Grid and the Body is at the Grid origin,
/// the camera's grid-local position IS the body-relative vector.
#[test]
fn test_terrain_altitude_from_grid_local() {
    let surface_pos = surface_pos(0.0, 0.0, MOON_RADIUS, 50.0);
    // When Body is at Grid origin: body_relative = surface_pos
    let body_relative = surface_pos;
    let altitude = body_relative.length() - MOON_RADIUS;
    assert!(
        (altitude - 50.0).abs() < 1.0,
        "Terrain altitude should be ~50m, got {:.2}m", altitude
    );
}

/// Suppress unused import warning.
#[allow(dead_code)]
fn _use_celestial_body() {
    let _ = std::mem::size_of::<CelestialBody>();
}
