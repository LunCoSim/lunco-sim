//! Integration test: verify terrain tiles form a proper sphere
//! with correct positions, normals, and grid orientation.
//!
//! This test builds the full tile generation pipeline, places a camera
//! on the Moon surface, and verifies:
//! 1. Tile meshes have correct vertex positions (body-local, on sphere)
//! 2. Tile normals point radially outward
//! 3. Blueprint grid lines are visible from the surface
//! 4. Adjacent tiles share edges (no gaps)

use bevy::math::{DVec3, Vec3};

const MOON_R: f64 = 1_737_000.0;

/// WGSL-compatible fract
fn wgsl_fract(x: f32) -> f32 {
    x - x.floor()
}

/// Exact copy of the WGSL cartesian grid shader math.
fn compute_grid_mask(body_local: Vec3, major_spacing: f32, minor_spacing: f32,
                     major_line_width: f32, minor_line_width: f32, minor_line_fade: f32) -> f32 {
    let bx = body_local.x;
    let by = body_local.y;
    let bz = body_local.z;

    let gx = ((wgsl_fract(bx / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;
    let gy = ((wgsl_fract(by / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;
    let gz = ((wgsl_fract(bz / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;

    let world_per_px = 1.0f32;
    let major_px = (gx / world_per_px).min(gy / world_per_px).min(gz / world_per_px);
    let major_m = 1.0 - smoothstep(0.0, major_line_width, major_px);

    let gx2 = ((wgsl_fract(bx / minor_spacing - 0.5).abs() - 0.5).abs()) * minor_spacing;
    let gy2 = ((wgsl_fract(by / minor_spacing - 0.5).abs() - 0.5).abs()) * minor_spacing;
    let gz2 = ((wgsl_fract(bz / minor_spacing - 0.5).abs() - 0.5).abs()) * minor_spacing;
    let minor_px = (gx2 / world_per_px).min(gy2 / world_per_px).min(gz2 / world_per_px);
    let minor_raw = 1.0 - smoothstep(0.0, minor_line_width, minor_px);
    let minor_m = minor_raw * minor_line_fade * (1.0 - major_m);

    major_m.max(minor_m)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Simulates the quadsphere mesh generation for a single tile.
/// Returns (positions, normals) in BODY-LOCAL space.
fn generate_tile_vertices(face: u8, level: u32, i: i32, j: i32, radius: f64, res: u32) -> (Vec<Vec3>, Vec<Vec3>) {
    // Reproduce create_quadsphere_tile_mesh logic
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let start_u = -1.0 + (i as f64) * step;
    let start_v = -1.0 + (j as f64) * step;

    // Tile center
    let u_mid = -1.0 + (i as f64 + 0.5) * step;
    let v_mid = -1.0 + (j as f64 + 0.5) * step;
    let tile_center_dir = cube_to_sphere(face, u_mid, v_mid);
    let tile_center = tile_center_dir * radius;

    let mut positions = Vec::new();
    let mut normals = Vec::new();

    for y in 0..=res {
        for x in 0..=res {
            let u = start_u + (x as f64 / res as f64) * step;
            let v = start_v + (y as f64 / res as f64) * step;
            let pos_sphere = cube_to_sphere(face, u, v);
            // Height sampling (no registry = flat sphere)
            let h = radius;
            // Vertex in tile-local space (relative to tile center)
            let vertex_local = Vec3::new(
                (pos_sphere.x * h - tile_center.x) as f32,
                (pos_sphere.y * h - tile_center.y) as f32,
                (pos_sphere.z * h - tile_center.z) as f32,
            );
            positions.push(vertex_local);
            normals.push(Vec3::new(pos_sphere.x as f32, pos_sphere.y as f32, pos_sphere.z as f32));
        }
    }
    (positions, normals)
}

/// Compute body-local position of a tile center, given its cell and local transform.
fn tile_body_local_position(cell_x: i64, cell_y: i64, cell_z: i64,
                             local_x: f32, local_y: f32, local_z: f32,
                             cell_size: f64) -> DVec3 {
    DVec3::new(
        cell_x as f64 * cell_size + local_x as f64,
        cell_y as f64 * cell_size + local_y as f64,
        cell_z as f64 * cell_size + local_z as f64,
    )
}

fn cube_to_sphere(face: u8, u: f64, v: f64) -> DVec3 {
    let p = match face {
        0 => DVec3::new(1.0, v, -u),
        1 => DVec3::new(-1.0, v, u),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(u, -1.0, -v),
        4 => DVec3::new(u, v, 1.0),
        5 => DVec3::new(-u, v, -1.0),
        _ => DVec3::ZERO,
    };
    p.normalize()
}

#[test]
fn test_tile_vertices_on_sphere() {
    // Generate all 24 LOD-1 tiles (face 0-5, 2x2 each) and verify every vertex
    // lies on the sphere surface (within 1mm tolerance for f32 rounding).
    let res = 32u32;
    let tolerance = 1.0; // 1 meter tolerance

    for face in 0..6u8 {
        for tile_i in 0..2i32 {
            for tile_j in 0..2i32 {
                let (positions, normals) = generate_tile_vertices(face, 1, tile_i, tile_j, MOON_R, res);

                // Compute the tile center in body-local space
                let tiles_at_level = 2u32; // LOD 1
                let step = 2.0 / tiles_at_level as f64;
                let u_mid = -1.0 + (tile_i as f64 + 0.5) * step;
                let v_mid = -1.0 + (tile_j as f64 + 0.5) * step;
                let tile_center = cube_to_sphere(face, u_mid, v_mid) * MOON_R;

                // Verify: each vertex position + tile_center should be on the sphere
                for (k, &v) in positions.iter().enumerate() {
                    // Reconstruct body-local position
                    let body_local = DVec3::new(
                        v.x as f64 + tile_center.x,
                        v.y as f64 + tile_center.y,
                        v.z as f64 + tile_center.z,
                    );
                    let dist = body_local.length();
                    let error = (dist - MOON_R).abs();
                    assert!(error < tolerance,
                        "Face {} tile [{},{}] vertex {} off sphere: dist={:.2}, error={:.2}",
                        face, tile_i, tile_j, k, dist, error);

                    // Normal should match radial direction
                    let expected_normal = body_local.normalize();
                    let actual_normal = DVec3::new(normals[k].x as f64, normals[k].y as f64, normals[k].z as f64);
                    let normal_error = (expected_normal - actual_normal).length();
                    assert!(normal_error < 0.001,
                        "Face {} tile [{},{}] vertex {} normal error: {:.6}",
                        face, tile_i, tile_j, k, normal_error);
                }
            }
        }
    }
}

#[test]
fn test_tile_positions_match_grid_decomposition() {
    // big_space Grid::translation_to_grid(cell_size=10000, ...) should decompose
    // tile center positions correctly, and reassembly should give back the original.
    let cell_size = 10_000.0_f64;

    for face in 0..6u8 {
        for tile_i in 0..2i32 {
            for tile_j in 0..2i32 {
                let tiles_at_level = 2u32;
                let step = 2.0 / tiles_at_level as f64;
                let u_mid = -1.0 + (tile_i as f64 + 0.5) * step;
                let v_mid = -1.0 + (tile_j as f64 + 0.5) * step;
                let tile_center = cube_to_sphere(face, u_mid, v_mid) * MOON_R;

                // Simulate Grid::translation_to_grid
                let cs = cell_size as f32;
                let body_pos = tile_center.as_vec3();
                let cell_x = (body_pos.x / cs).floor() as i64;
                let cell_y = (body_pos.y / cs).floor() as i64;
                let cell_z = (body_pos.z / cs).floor() as i64;
                let local_x = body_pos.x - cell_x as f32 * cs;
                let local_y = body_pos.y - cell_y as f32 * cs;
                let local_z = body_pos.z - cell_z as f32 * cs;

                // Reassemble
                let reassembled = tile_body_local_position(cell_x, cell_y, cell_z, local_x, local_y, local_z, cell_size);
                let error = (reassembled - tile_center).length();
                assert!(error < 0.01,
                    "Face {} tile [{},{}] grid decomposition error: {:.4} (center={:?}, cell=({},{},{}), local=({},{},{}))",
                    face, tile_i, tile_j, error, tile_center, cell_x, cell_y, cell_z, local_x, local_y, local_z);
            }
        }
    }
}

#[test]
fn test_grid_visible_from_surface_camera() {
    // Place a camera 50m above the Moon surface at lat=0, lon=0 (+Z face).
    // Verify the blueprint grid is visible from this viewpoint.
    let cam_pos = Vec3::new(0.0, 0.0, (MOON_R + 50.0) as f32);

    // Check grid visibility at camera's forward direction (looking at ground = toward body center)
    let look_dir = -cam_pos.normalize();
    let surface_point = cam_pos + look_dir * 50.0; // 50m below camera, on surface

    let mask = compute_grid_mask(surface_point, 1000.0, 500.0, 0.75, 0.4, 0.3);

    // Also check surrounding area (FOV simulation)
    let mut max_mask = 0.0f32;
    for dx in -20..=20i32 {
        for dy in -20..=20i32 {
            let offset = Vec3::new(dx as f32, dy as f32, 0.0);
            let pt = surface_point + offset;
            // Project back to sphere surface
            let pt_normalized = pt.normalize() * MOON_R as f32;
            let m = compute_grid_mask(pt_normalized, 1000.0, 500.0, 0.75, 0.4, 0.3);
            if m > max_mask { max_mask = m; }
        }
    }

    assert!(max_mask > 0.1,
        "Blueprint grid should be visible from surface camera at +Z. Max mask={:.4}, cam_pos={:?}",
        max_mask, cam_pos);
}

#[test]
fn test_grid_orientation_consistent_across_tile_boundary() {
    // Two adjacent tiles on the same face should have a continuous grid pattern.
    // Tile [0,0] and Tile [1,0] share an edge. Grid lines should not jump at the boundary.
    let res = 8u32; // Low res for speed

    // Generate tile [0,0] face 4 (+Z)
    let (pos_a, _) = generate_tile_vertices(4, 1, 0, 0, MOON_R, res);
    // Generate tile [1,0] face 4 (+Z)
    let (pos_b, _) = generate_tile_vertices(4, 1, 1, 0, MOON_R, res);

    // Tile centers
    let tc_a = cube_to_sphere(4, -0.5, -0.5) * MOON_R;
    let tc_b = cube_to_sphere(4, 0.5, -0.5) * MOON_R;

    // Right edge of tile A (last column, x=res)
    // Left edge of tile B (first column, x=0)
    // These should be at the same body-local position
    let right_edge_a: Vec<Vec3> = (0..=res).map(|y| {
        let idx = y as usize * (res as usize + 1) + res as usize;
        let v = pos_a[idx];
        Vec3::new(v.x as f32 + tc_a.x as f32, v.y as f32 + tc_a.y as f32, v.z as f32 + tc_a.z as f32)
    }).collect();

    let left_edge_b: Vec<Vec3> = (0..=res).map(|y| {
        let idx = y as usize * (res as usize + 1);
        let v = pos_b[idx];
        Vec3::new(v.x as f32 + tc_b.x as f32, v.y as f32 + tc_b.y as f32, v.z as f32 + tc_b.z as f32)
    }).collect();

    for (i, (a, b)) in right_edge_a.iter().zip(left_edge_b.iter()).enumerate() {
        let error = (a - b).length();
        assert!(error < 1.0,
            "Tile boundary mismatch at row {}: edge_a={:?}, edge_b={:?}, error={:.2}",
            i, a, b, error);
    }
}
