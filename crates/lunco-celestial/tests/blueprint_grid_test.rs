//! Tests for the blueprint grid shader math.
//!
//! IMPORTANT: WGSL's `fract(x)` = `x - floor(x)` (always positive).
//! Rust's `x.fract()` = `x - x.trunc()` (can be negative).
//! This test uses the WGSL-correct formula.

/// WGSL-compatible fract: always returns value in [0, 1)
fn wgsl_fract(x: f32) -> f32 {
    x - x.floor()
}

/// Simulates the WGSL cartesian grid computation exactly as in the shader.
fn compute_grid_mask(body_local: [f32; 3], major_spacing: f32, minor_spacing: f32,
                     major_line_width: f32, minor_line_width: f32, minor_line_fade: f32) -> f32 {
    let bx = body_local[0];
    let by = body_local[1];
    let bz = body_local[2];

    // 3D grid: distance to nearest grid plane along each axis
    let gx = ((wgsl_fract(bx / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;
    let gy = ((wgsl_fract(by / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;
    let gz = ((wgsl_fract(bz / major_spacing - 0.5).abs() - 0.5).abs()) * major_spacing;

    // fwidth approximation (~1 world unit per pixel at close range)
    let world_per_px = 1.0f32;

    let major_px = (gx / world_per_px).min(gy / world_per_px).min(gz / world_per_px);
    let major_m = 1.0 - smoothstep(0.0, major_line_width, major_px);

    // Minor grid
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

const MOON_R: f32 = 1_737_000.0;

#[test]
fn test_grid_visible_at_face_centers() {
    let face_centers = [
        [MOON_R, 0.0, 0.0], [-MOON_R, 0.0, 0.0],
        [0.0, MOON_R, 0.0], [0.0, -MOON_R, 0.0],
        [0.0, 0.0, MOON_R], [0.0, 0.0, -MOON_R],
    ];
    for (i, pos) in face_centers.iter().enumerate() {
        let mask = compute_grid_mask(*pos, 1000.0, 500.0, 0.75, 0.4, 0.3);
        assert!(mask > 0.5,
            "Face center {} {:?} should be on a grid line, mask={:.4}", i, pos, mask);
    }
}

#[test]
fn test_grid_visible_at_surface_points() {
    let test_points = [
        (0.0f64, 0.0, "equator/prime"),
        (0.0, 90.0, "equator/90E"),
        (45.0, 0.0, "45N/prime"),
        (-45.0, 45.0, "45S/45E"),
        (89.0, 0.0, "near north pole"),
        (-89.0, 180.0, "near south pole"),
    ];

    for (lat, lon, desc) in &test_points {
        let lat_r = (*lat as f64).to_radians();
        let lon_r = (*lon as f64).to_radians();
        let pos = [
            (lat_r.cos() * lon_r.sin() * MOON_R as f64) as f32,
            (lat_r.sin() * MOON_R as f64) as f32,
            (lat_r.cos() * lon_r.cos() * MOON_R as f64) as f32,
        ];

        // Check a small neighborhood (simulates pixel-level variation)
        let mut max_mask = 0.0f32;
        for dx in -5..=5i32 {
            for dy in -5..=5i32 {
                let shifted = [pos[0] + dx as f32, pos[1] + dy as f32, pos[2]];
                let m = compute_grid_mask(shifted, 1000.0, 500.0, 0.75, 0.4, 0.3);
                if m > max_mask { max_mask = m; }
            }
        }

        assert!(max_mask > 0.1,
            "Grid should be visible near {} pos={:?} max_mask={:.4}",
            desc, pos, max_mask);
    }
}

#[test]
fn test_grid_has_gaps_between_lines() {
    let pos = [250.0, 250.0, 250.0];
    let mask = compute_grid_mask(pos, 1000.0, 500.0, 0.75, 0.4, 0.3);
    assert!(mask < 0.5, "Should have gaps between grid lines, got {}", mask);
}

#[test]
fn test_grid_periodic() {
    let base = [1_737_000.0, 100.0, 100.0];
    let shifted = [1_738_000.0, 100.0, 100.0];
    let m1 = compute_grid_mask(base, 1000.0, 500.0, 0.75, 0.4, 0.3);
    let m2 = compute_grid_mask(shifted, 1000.0, 500.0, 0.75, 0.4, 0.3);
    assert!((m1 - m2).abs() < 0.01,
        "Grid should repeat every 1000m: before={:.4}, after={:.4}", m1, m2);
}
