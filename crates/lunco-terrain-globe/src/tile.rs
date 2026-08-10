//! Terrain tile mesh generation and sampling.

use crate::quad_sphere::cube_to_sphere;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_mesh::{Indices, PrimitiveTopology};

/// A square cutout in the body's tangent-plane coordinates. It is supplied by
/// the celestial integration when an authored local DEM owns this footprint.
/// Globe tiles are clipped against the square at triangle boundaries; no
/// arbitrary shell sink or enlarged cone is needed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobeCutout {
    /// Unit direction from the body centre to the site's tangent point.
    pub dir: DVec3,
    /// Unit east and north axes at the site, in body-fixed coordinates.
    pub east: DVec3,
    pub north: DVec3,
    /// Radius of the body used to convert angular gnomonic coordinates to metres.
    pub radius_m: f64,
    /// Half side of the DEM square in metres.
    pub half_extent: f64,
}

/// Generate a mesh for a single QuadSphere tile.
pub fn create_quadsphere_tile_mesh(
    _body_ent: Entity,
    face: u8,
    level: u32,
    i: i32,
    j: i32,
    radius: f64,
    res: u32,
    tile_center: DVec3,
    cutout: Option<GlobeCutout>,
) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut uvs = Vec::new();
    let mut directions = Vec::new();
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let start_u = -1.0 + (i as f64) * step;
    let start_v = -1.0 + (j as f64) * step;

    for y in 0..=res {
        for x in 0..=res {
            let u = start_u + (x as f64 / res as f64) * step;
            let v = start_v + (y as f64 / res as f64) * step;
            let pos_sphere = cube_to_sphere(face, u, v);
            positions.push((pos_sphere * radius - tile_center).as_vec3());
            normals.push(pos_sphere.as_vec3());
            directions.push(pos_sphere);

            // Equirectangular UV mapping
            let mut u_raw = (-pos_sphere.z).atan2(pos_sphere.x);
            let center_u = start_u + step * 0.5;
            let center_v = start_v + step * 0.5;
            let tile_center_dir = cube_to_sphere(face, center_u, center_v);
            let ref_lon = (-tile_center_dir.z).atan2(tile_center_dir.x);
            if (u_raw - ref_lon) > std::f64::consts::PI {
                u_raw -= 2.0 * std::f64::consts::PI;
            } else if (u_raw - ref_lon) < -std::f64::consts::PI {
                u_raw += 2.0 * std::f64::consts::PI;
            }

            let u_tex = (u_raw + std::f64::consts::PI) / (2.0 * std::f64::consts::PI);
            // `pos_sphere` is normalised, but rounding can nudge `y` a hair past ±1
            // → `asin` = NaN → NaN UVs at the face corners. Clamp to the valid domain.
            let v_tex = (pos_sphere.y.clamp(-1.0, 1.0).asin() + (std::f64::consts::PI / 2.0))
                / std::f64::consts::PI;
            uvs.push(Vec2::new(u_tex as f32, 1.0 - v_tex as f32));
        }
    }

    for y in 0..res {
        for x in 0..res {
            let i0 = y * (res + 1) + x;
            let i1 = i0 + 1;
            let i2 = (y + 1) * (res + 1) + x;
            let i3 = i2 + 1;

            // CCW for sides, CW for Top/Bottom
            if face == 2 || face == 3 {
                indices.push(i0);
                indices.push(i2);
                indices.push(i1);
                indices.push(i1);
                indices.push(i2);
                indices.push(i3);
            } else {
                indices.push(i0);
                indices.push(i1);
                indices.push(i2);
                indices.push(i1);
                indices.push(i3);
                indices.push(i2);
            }
        }
    }

    if let Some(cutout) = cutout.filter(|cutout| cutout_intersects_tile(cutout, &directions)) {
        let original_indices = std::mem::take(&mut indices);
        let original_directions = directions;
        let original_uvs = uvs;
        let mut clipped_positions = Vec::new();
        let mut clipped_normals = Vec::new();
        let mut clipped_uvs = Vec::new();
        let mut clipped_indices = Vec::new();
        for tri in original_indices.chunks_exact(3) {
            let dirs = [
                original_directions[tri[0] as usize],
                original_directions[tri[1] as usize],
                original_directions[tri[2] as usize],
            ];
            if dirs.iter().all(|&d| cutout.contains(d)) {
                continue;
            }
            let base_uv = [
                original_uvs[tri[0] as usize],
                original_uvs[tri[1] as usize],
                original_uvs[tri[2] as usize],
            ];
            // The outside of an axis-aligned square is four disjoint convex
            // regions: left, right, bottom-between-sides, and top-between-sides.
            // The back hemisphere is a fifth disjoint region because gnomonic
            // coordinates are defined only in front of the tangent plane.
            // Clipping the source triangle to each region removes only the DEM
            // square without dropping the far side of the body or overlapping
            // corner polygons.
            for region in 0..5u8 {
                let polygon = clip_triangle_to_region(&dirs, &cutout, region);
                if polygon.len() < 3 {
                    continue;
                }
                for k in 1..polygon.len() - 1 {
                    let triangle = [polygon[0], polygon[k], polygon[k + 1]];
                    let first = clipped_positions.len() as u32;
                    for v in triangle {
                        let dir = interpolate_dir(&dirs, v.bary);
                        let uv = interpolate_uv(&base_uv, v.bary);
                        clipped_positions.push((dir * radius - tile_center).as_vec3());
                        clipped_normals.push(dir.as_vec3());
                        clipped_uvs.push(uv);
                    }
                    clipped_indices.extend_from_slice(&[first, first + 1, first + 2]);
                }
            }
        }
        positions = clipped_positions;
        normals = clipped_normals;
        uvs = clipped_uvs;
        indices = clipped_indices;
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[derive(Clone, Copy)]
struct ClipVertex {
    bary: [f64; 3],
}

impl GlobeCutout {
    fn coordinates(self, direction: DVec3) -> Option<[f64; 2]> {
        let denominator = direction.dot(self.dir);
        if denominator <= 0.0 {
            return None;
        }
        Some([
            direction.dot(self.east) / denominator * self.radius_m,
            direction.dot(self.north) / denominator * self.radius_m,
        ])
    }

    fn contains(self, direction: DVec3) -> bool {
        let Some([x, z]) = self.coordinates(direction) else {
            return false;
        };
        x.abs() <= self.half_extent && z.abs() <= self.half_extent
    }
}

fn interpolate_dir(dirs: &[DVec3; 3], bary: [f64; 3]) -> DVec3 {
    (dirs[0] * bary[0] + dirs[1] * bary[1] + dirs[2] * bary[2]).normalize()
}

/// Conservative tile-level rejection for the exact cutout. A tile that is
/// wholly behind the tangent plane cannot contain the local DEM, and a tile
/// wholly in front whose projected AABB misses the square cannot contain it
/// either. Boundary/ambiguous tiles take the exact per-triangle path below.
fn cutout_intersects_tile(cutout: &GlobeCutout, directions: &[DVec3]) -> bool {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    for &direction in directions {
        let Some([x, z]) = cutout.coordinates(direction) else {
            return true;
        };
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    !(max_x < -cutout.half_extent
        || min_x > cutout.half_extent
        || max_z < -cutout.half_extent
        || min_z > cutout.half_extent)
}

fn interpolate_uv(uvs: &[Vec2; 3], bary: [f64; 3]) -> Vec2 {
    Vec2::new(
        (uvs[0].x as f64 * bary[0] + uvs[1].x as f64 * bary[1] + uvs[2].x as f64 * bary[2]) as f32,
        (uvs[0].y as f64 * bary[0] + uvs[1].y as f64 * bary[1] + uvs[2].y as f64 * bary[2]) as f32,
    )
}

fn clip_triangle_to_region(dirs: &[DVec3; 3], cutout: &GlobeCutout, region: u8) -> Vec<ClipVertex> {
    let mut polygon = vec![
        ClipVertex {
            bary: [1.0, 0.0, 0.0],
        },
        ClipVertex {
            bary: [0.0, 1.0, 0.0],
        },
        ClipVertex {
            bary: [0.0, 0.0, 1.0],
        },
    ];
    let cuts: &[(f64, f64, f64)] = match region {
        0 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (1.0, 0.0, cutout.half_extent),
        ], // x <= -h
        1 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (-1.0, 0.0, cutout.half_extent),
        ], // x >= h
        2 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, 1.0, cutout.half_extent),
            (-1.0, 0.0, -cutout.half_extent),
            (1.0, 0.0, -cutout.half_extent),
        ],
        3 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, -1.0, cutout.half_extent),
            (-1.0, 0.0, -cutout.half_extent),
            (1.0, 0.0, -cutout.half_extent),
        ],
        4 => &[(0.0, 0.0, 0.0)], // denominator <= 0: the back hemisphere
        _ => &[],
    };
    for &(a, b, c) in cuts {
        let mut next = Vec::new();
        for pair in polygon.windows(2).chain(std::iter::once(
            &[polygon[polygon.len() - 1], polygon[0]][..],
        )) {
            let start = pair[0];
            let end = pair[1];
            let fs = region_value(start, dirs, cutout, region, a, b, c);
            let fe = region_value(end, dirs, cutout, region, a, b, c);
            let start_inside = fs <= 0.0;
            let end_inside = fe <= 0.0;
            if start_inside != end_inside {
                let t = fs / (fs - fe);
                next.push(ClipVertex {
                    bary: std::array::from_fn(|i| {
                        start.bary[i] + (end.bary[i] - start.bary[i]) * t
                    }),
                });
            }
            if end_inside {
                next.push(end);
            }
        }
        polygon = next;
        if polygon.is_empty() {
            break;
        }
    }
    polygon
}

fn region_value(
    vertex: ClipVertex,
    dirs: &[DVec3; 3],
    cutout: &GlobeCutout,
    region: u8,
    a: f64,
    b: f64,
    c: f64,
) -> f64 {
    // Keep the unnormalised barycentric direction for the clipping predicate.
    // Every boundary is homogeneous in the direction vector, so this is linear
    // along an edge and its intersection parameter is exact. Normalising here
    // would make the predicate nonlinear in barycentrics and place the cutout
    // boundary at the wrong point near the horizon.
    let raw = dirs[0] * vertex.bary[0] + dirs[1] * vertex.bary[1] + dirs[2] * vertex.bary[2];
    if region == 4 {
        return raw.dot(cutout.dir);
    }
    let denominator = raw.dot(cutout.dir);
    if a == 0.0 && b == 0.0 && c == 0.0 {
        return -denominator;
    }
    // Multiply the gnomonic half-plane by its positive denominator. This keeps
    // the clipping function finite when an edge crosses the tangent horizon;
    // using `INFINITY` for the back endpoint made `inf / inf` produce NaN
    // intersection barycentrics.
    a * raw.dot(cutout.east) * cutout.radius_m
        + b * raw.dot(cutout.north) * cutout.radius_m
        + c * denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cutout(half_extent: f64) -> GlobeCutout {
        GlobeCutout {
            dir: DVec3::X,
            east: DVec3::Z,
            north: DVec3::Y,
            radius_m: 100.0,
            half_extent,
        }
    }

    #[test]
    fn cutout_uses_the_authored_tangent_square() {
        let c = cutout(10.0);
        assert!(c.contains(DVec3::new(1.0, 0.05, 0.05).normalize()));
        assert!(!c.contains(DVec3::new(1.0, 0.2, 0.0).normalize()));
        assert!(!c.contains(DVec3::new(1.0, 0.0, -0.2).normalize()));
    }

    #[test]
    fn a_tile_wholly_inside_the_cutout_has_no_globe_triangles() {
        let mesh = create_quadsphere_tile_mesh(
            Entity::PLACEHOLDER,
            0,
            0,
            0,
            0,
            100.0,
            2,
            DVec3::ZERO,
            Some(cutout(1.0e9)),
        );
        assert!(mesh.indices().is_none_or(|indices| indices.len() == 0));
    }

    #[test]
    fn far_side_tiles_remain_when_a_front_cutout_is_present() {
        let mesh = create_quadsphere_tile_mesh(
            Entity::PLACEHOLDER,
            1,
            0,
            0,
            0,
            100.0,
            2,
            DVec3::ZERO,
            Some(cutout(1.0)),
        );
        assert!(mesh.indices().is_some_and(|indices| indices.len() == 24));
    }

    #[test]
    fn clipping_across_the_tangent_horizon_stays_finite() {
        let c = cutout(10.0);
        let dirs = [
            DVec3::X,
            DVec3::new(-1.0, 0.1, 0.0).normalize(),
            DVec3::new(1.0, 0.2, 0.0).normalize(),
        ];
        let mut vertices = 0;
        for region in 0..5 {
            for vertex in clip_triangle_to_region(&dirs, &c, region) {
                assert!(vertex.bary.iter().all(|value| value.is_finite()));
                let direction = interpolate_dir(&dirs, vertex.bary);
                assert!(direction.is_finite());
                if let Some([x, z]) = c.coordinates(direction) {
                    let strictly_inside =
                        x.abs() < c.half_extent - 1.0e-9 && z.abs() < c.half_extent - 1.0e-9;
                    assert!(
                        !strictly_inside,
                        "region {region} clipped vertex remained in cutout: {x}, {z}, bary={:?}",
                        vertex.bary
                    );
                }
                vertices += 1;
            }
        }
        assert!(vertices > 0);
    }
}
