//! QuadSphere math — cube-to-sphere projection and LOD subdivision.

use bevy::math::DVec3;

/// A square-site perimeter band that needs a minimum globe-tile detail.
///
/// `radius_m` is a conservative spherical bound around the perimeter band.
/// Intersecting tiles are refined until their nominal arc size is no larger than
/// `max_tile_size_m`. Fixed-detail regions are independent of camera distance
/// and support static transitions outside the camera-driven detail band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodRefinementRegion {
    /// Point on the body sphere at the centre of the refinement region.
    pub center: DVec3,
    /// Conservative spherical bound of the perimeter band, in metres.
    pub radius_m: f64,
    /// Unit east and north axes at the square boundary's centre.
    pub east: DVec3,
    pub north: DVec3,
    /// Radius at which the site's tangent-plane square is defined.
    pub site_radius_m: f64,
    /// Half side of the square site's projected footprint.
    pub half_extent_m: f64,
    /// Width of the boundary band that needs fixed detail.
    pub width_m: f64,
    /// Largest allowed tile arc size where this region intersects a tile.
    pub max_tile_size_m: f64,
    /// Deepest subdivision allowed for the region, independent of the camera.
    pub max_lod: u32,
}

/// Projects a point on a cube face to the unit sphere.
///
/// `face` is 0..5 representing +X, -X, +Y, -Y, +Z, -Z faces.
/// `u` and `v` are in the range [-1, 1] within the face.
pub fn cube_to_sphere(face: u8, u: f64, v: f64) -> DVec3 {
    cube_face_point(face, u, v).normalize()
}

fn cube_face_point(face: u8, u: f64, v: f64) -> DVec3 {
    match face {
        0 => DVec3::new(1.0, v, -u),
        1 => DVec3::new(-1.0, v, u),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(u, -1.0, -v),
        4 => DVec3::new(u, v, 1.0),
        5 => DVec3::new(-u, v, -1.0),
        _ => DVec3::ZERO,
    }
}

fn cube_point_to_face_uv(point: DVec3) -> (u8, f64, f64) {
    let abs = point.abs();
    if abs.x >= abs.y && abs.x >= abs.z {
        if point.x >= 0.0 {
            (0, -point.z / point.x, point.y / point.x)
        } else {
            let scale = -point.x;
            (1, point.z / scale, point.y / scale)
        }
    } else if abs.y >= abs.z {
        if point.y >= 0.0 {
            (2, point.x / point.y, point.z / point.y)
        } else {
            let scale = -point.y;
            (3, point.x / scale, -point.z / scale)
        }
    } else if point.z >= 0.0 {
        (4, point.x / point.z, point.y / point.z)
    } else {
        let scale = -point.z;
        (5, -point.x / scale, point.y / scale)
    }
}

/// Refine a desired cube-sphere cover until adjacent leaves differ by at most
/// one quadtree level, including edges shared by different cube faces.
///
/// The result is a disjoint cover of the same sphere: balancing only replaces a
/// coarse leaf with its four children. Call this after camera and fixed-region
/// selection, then cache the balanced set with the normal globe selection.
pub fn balance_cube_sphere_lod(
    desired: &mut std::collections::HashSet<crate::TileCoord>,
    body: bevy::prelude::Entity,
    max_level: u32,
) {
    loop {
        let mut refine = std::collections::HashSet::new();
        for tile in desired.iter().filter(|tile| tile.body == body).copied() {
            for neighbor in cube_sphere_edge_neighbors(&tile, max_level, desired) {
                if tile.level > neighbor.level + 1 {
                    refine.insert(neighbor);
                }
            }
        }
        if refine.is_empty() {
            break;
        }
        for tile in refine {
            if !desired.remove(&tile) {
                continue;
            }
            let level = tile.level + 1;
            for di in 0..2 {
                for dj in 0..2 {
                    desired.insert(crate::TileCoord {
                        body: tile.body,
                        face: tile.face,
                        level,
                        i: tile.i * 2 + di,
                        j: tile.j * 2 + dj,
                    });
                }
            }
        }
    }
}

fn cube_sphere_edge_neighbors(
    tile: &crate::TileCoord,
    max_level: u32,
    leaves: &std::collections::HashSet<crate::TileCoord>,
) -> Vec<crate::TileCoord> {
    let (center_u, center_v) = tile_center_uv(tile.face, tile.level, tile.i, tile.j);
    let step = 2.0 / f64::from(1_u32 << tile.level);
    let half_step = step * 0.5;
    // Probe a small distance beyond each edge. Across face boundaries, the
    // dominant cube axis then unambiguously identifies the adjoining face.
    let probe = step * 1.0e-6;
    let probes = [
        (center_u - half_step - probe, center_v),
        (center_u + half_step + probe, center_v),
        (center_u, center_v - half_step - probe),
        (center_u, center_v + half_step + probe),
    ];
    let grid_size = 1_i32 << max_level;
    let mut neighbors = Vec::with_capacity(4);
    for (u, v) in probes {
        let point = cube_face_point(tile.face, u, v);
        let (face, neighbor_u, neighbor_v) = cube_point_to_face_uv(point);
        let i = (((neighbor_u + 1.0) * 0.5 * f64::from(grid_size)).floor() as i32)
            .clamp(0, grid_size - 1);
        let j = (((neighbor_v + 1.0) * 0.5 * f64::from(grid_size)).floor() as i32)
            .clamp(0, grid_size - 1);
        let neighbor = (0..=max_level).rev().find_map(|level| {
            let scale = 1_i32 << (max_level - level);
            leaves
                .get(&crate::TileCoord {
                    body: tile.body,
                    face,
                    level,
                    i: i / scale,
                    j: j / scale,
                })
                .copied()
        });
        if let Some(neighbor) = neighbor.filter(|neighbor| *neighbor != *tile) {
            neighbors.push(neighbor);
        }
    }
    neighbors
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
    refinement_regions: &[LodRefinementRegion],
) {
    if level == 0 {
        assert!(
            refinement_regions.iter().all(|region| {
                region.center.is_finite()
                    && region.radius_m.is_finite()
                    && region.radius_m > 0.0
                    && region.east.is_finite()
                    && region.north.is_finite()
                    && region.site_radius_m.is_finite()
                    && region.site_radius_m > 0.0
                    && region.half_extent_m.is_finite()
                    && region.half_extent_m > 0.0
                    && region.width_m.is_finite()
                    && region.width_m >= 0.0
                    && region.max_tile_size_m.is_finite()
                    && region.max_tile_size_m > 0.0
                    && region.max_lod < 31
            }),
            "globe LOD refinement regions must contain finite geometry and positive tile sizes"
        );
    }
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let u = -1.0 + (i as f64 + 0.5) * step;
    let v = -1.0 + (j as f64 + 0.5) * step;
    let tile_center_sphere = cube_to_sphere(face, u, v);
    let tile_center_local = tile_center_sphere * body_radius;
    let dist = camera_body_local.distance(tile_center_local);
    let tile_size = (body_radius * std::f64::consts::PI * 0.5) / tiles_at_level as f64;
    let intersects_refinement_region = refinement_regions.iter().any(|region| {
        if level >= region.max_lod || tile_size <= region.max_tile_size_m {
            return false;
        }
        let outer_chord_radius = if region.radius_m >= std::f64::consts::PI * body_radius {
            2.0 * body_radius
        } else {
            2.0 * body_radius * (region.radius_m / (2.0 * body_radius)).sin()
        };
        let center_distance = tile_center_local.distance(region.center);
        if center_distance > outer_chord_radius + tile_size {
            return false;
        }
        tile_intersects_square_boundary_band(
            face,
            level,
            i,
            j,
            region.center.normalize(),
            region.east,
            region.north,
            region.site_radius_m,
            region.half_extent_m,
            region.width_m,
        )
    });

    let is_resident_leaf = resident.contains(&crate::TileCoord {
        body: body_ent,
        face,
        level,
        i,
        j,
    });
    let threshold = tile_size * lod_distance_factor * if is_resident_leaf { 0.95 } else { 1.05 };
    let refine_for_camera = level < max_lod && dist < threshold;
    if refine_for_camera || intersects_refinement_region {
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
                    refinement_regions,
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

fn tile_intersects_square_boundary_band(
    face: u8,
    level: u32,
    i: i32,
    j: i32,
    center_direction: DVec3,
    east: DVec3,
    north: DVec3,
    site_radius_m: f64,
    half_extent_m: f64,
    width_m: f64,
) -> bool {
    let tiles_at_level = 1_u32 << level;
    let step = 2.0 / f64::from(tiles_at_level);
    let min_u = -1.0 + f64::from(i) * step;
    let min_v = -1.0 + f64::from(j) * step;
    let max_u = min_u + step;
    let max_v = min_v + step;
    let corners = [
        (min_u, min_v),
        (min_u, max_v),
        (max_u, min_v),
        (max_u, max_v),
    ];
    let mut min_east = f64::INFINITY;
    let mut max_east = f64::NEG_INFINITY;
    let mut min_north = f64::INFINITY;
    let mut max_north = f64::NEG_INFINITY;
    for (u, v) in corners {
        let direction = cube_to_sphere(face, u, v);
        let denominator = direction.dot(center_direction);
        if denominator <= 0.0 {
            return true;
        }
        let east_m = direction.dot(east) / denominator * site_radius_m;
        let north_m = direction.dot(north) / denominator * site_radius_m;
        min_east = min_east.min(east_m);
        max_east = max_east.max(east_m);
        min_north = min_north.min(north_m);
        max_north = max_north.max(north_m);
    }

    let outer_extent_m = half_extent_m + width_m;
    let inner_extent_m = (half_extent_m - width_m).max(0.0);
    let overlaps_outer_square = max_east >= -outer_extent_m
        && min_east <= outer_extent_m
        && max_north >= -outer_extent_m
        && min_north <= outer_extent_m;
    let fully_inside_inner_square = min_east > -inner_extent_m
        && max_east < inner_extent_m
        && min_north > -inner_extent_m
        && max_north < inner_extent_m;
    overlaps_outer_square && !fully_inside_inner_square
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use crate::TileCoord;

    #[test]
    fn lod_balance_closes_cube_face_edges_and_preserves_the_sphere_cover() {
        let body = bevy::prelude::Entity::from_bits(1);
        let max_level = 4;
        let mut desired: HashSet<TileCoord> = (0..6)
            .map(|face| TileCoord {
                body,
                face,
                level: 0,
                i: 0,
                j: 0,
            })
            .collect();

        // Refine one complete face deeply to exercise every adjacent cube face.
        for level in 0..max_level {
            let parents: Vec<_> = desired
                .iter()
                .filter(|tile| tile.face == 0 && tile.level == level)
                .copied()
                .collect();
            for parent in parents {
                desired.remove(&parent);
                for di in 0..2 {
                    for dj in 0..2 {
                        desired.insert(TileCoord {
                            body,
                            face: parent.face,
                            level: level + 1,
                            i: parent.i * 2 + di,
                            j: parent.j * 2 + dj,
                        });
                    }
                }
            }
        }

        balance_cube_sphere_lod(&mut desired, body, max_level);

        let full_face_area = 1_u64 << (2 * max_level);
        let covered_area: u64 = desired
            .iter()
            .map(|tile| 1_u64 << (2 * (max_level - tile.level)))
            .sum();
        assert_eq!(covered_area, 6 * full_face_area);
        for tile in &desired {
            for neighbor in cube_sphere_edge_neighbors(tile, max_level, &desired) {
                assert!(
                    tile.level.abs_diff(neighbor.level) <= 1,
                    "unbalanced edge: face {} L{} ({}, {}) touches face {} L{} ({}, {})",
                    tile.face,
                    tile.level,
                    tile.i,
                    tile.j,
                    neighbor.face,
                    neighbor.level,
                    neighbor.i,
                    neighbor.j,
                );
            }
        }
        assert!(desired.iter().any(|tile| tile.face != 0 && tile.level > 0));
    }

    #[test]
    fn handoff_refinement_is_local_and_independent_of_camera_distance() {
        let body = bevy::prelude::Entity::from_bits(1);
        let radius_m = 1_737_400.0;
        let region = LodRefinementRegion {
            center: DVec3::X * radius_m,
            radius_m: 16_000.0,
            east: DVec3::Z,
            north: DVec3::Y,
            site_radius_m: radius_m,
            half_extent_m: 10_000.0,
            width_m: 1_000.0,
            max_tile_size_m: 50_000.0,
            max_lod: 6,
        };
        let mut desired = HashSet::new();
        for face in 0..6u8 {
            subdivide_face(
                &mut desired,
                &HashSet::new(),
                body,
                face,
                0,
                0,
                0,
                DVec3::splat(1.0e12),
                radius_m,
                4,
                2.0,
                &[region],
            );
        }

        assert!(desired.iter().any(|tile| tile.face == 0 && tile.level >= 6));
        assert!(desired.contains(&TileCoord {
            body,
            face: 1,
            level: 0,
            i: 0,
            j: 0,
        }));
    }
}
