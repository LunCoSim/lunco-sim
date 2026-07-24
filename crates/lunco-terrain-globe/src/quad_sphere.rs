//! QuadSphere math — cube-to-sphere projection and LOD subdivision.

use bevy::math::DVec3;

/// Projects a point on a cube face to the unit sphere.
///
/// `face` is 0..5 representing +X, -X, +Y, -Y, +Z, -Z faces.
/// `u` and `v` are in the range [-1, 1] within the face.
pub fn cube_to_sphere(face: u8, u: f64, v: f64) -> DVec3 {
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

/// Compute u,v tile center coordinates from face/level/i/j for LOD tiles.
pub fn tile_center_uv(_face: u8, level: u32, i: i32, j: i32) -> (f64, f64) {
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let u_mid = -1.0 + (i as f64 + 0.5) * step;
    let v_mid = -1.0 + (j as f64 + 0.5) * step;
    (u_mid, v_mid)
}

/// Subdivide a quad sphere face into tiles based on camera distance.
///
/// Recursively subdivides until `max_lod` or the tile is far enough from the
/// camera. `resident` is the currently-streamed leaf set: the split threshold
/// carries a ±5% dead band around it (a resident leaf must come clearly
/// inside to split; a split node stays split until the camera is clearly
/// outside). Without it a camera parked exactly on a threshold — the focus
/// command snaps to precisely 3.0 radii — flaps the leaf set every frame,
/// despawning/respawning tiles with fresh mesh assets (visible as the planet
/// flickering in and out frame by frame).
pub fn subdivide_face(
    desired: &mut std::collections::HashSet<crate::TileCoord>,
    resident: &std::collections::HashSet<crate::TileCoord>,
    body_ent: bevy::prelude::Entity,
    face: u8,
    level: u32,
    i: i32,
    j: i32,
    camera_body_local: DVec3,
    body_radius: f64,
    max_lod: u32,
    lod_distance_factor: f64,
) {
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let u = -1.0 + (i as f64 + 0.5) * step;
    let v = -1.0 + (j as f64 + 0.5) * step;
    let tile_center_sphere = cube_to_sphere(face, u, v);
    let tile_center_local = tile_center_sphere * body_radius;
    let dist = camera_body_local.distance(tile_center_local);
    let tile_size = (body_radius * std::f64::consts::PI * 0.5) / tiles_at_level as f64;

    let is_resident_leaf = resident.contains(&crate::TileCoord {
        body: body_ent,
        face,
        level,
        i,
        j,
    });
    let threshold = tile_size * lod_distance_factor * if is_resident_leaf { 0.95 } else { 1.05 };
    if level < max_lod && dist < threshold {
        for di in 0..2 {
            for dj in 0..2 {
                subdivide_face(
                    desired,
                    resident,
                    body_ent,
                    face,
                    level + 1,
                    i * 2 + di,
                    j * 2 + dj,
                    camera_body_local,
                    body_radius,
                    max_lod,
                    lod_distance_factor,
                );
            }
        }
    } else {
        desired.insert(crate::TileCoord {
            body: body_ent,
            face,
            level,
            i,
            j,
        });
    }
}
