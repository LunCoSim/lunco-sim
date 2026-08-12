//! Terrain tile mesh generation and sampling.

use crate::quad_sphere::cube_to_sphere;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_mesh::{Indices, PrimitiveTopology};
use lunco_terrain_core::HeightSource;

/// The exact local DEM footprint in the body's tangent-plane coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobeHandoff {
    /// Unit direction from the body centre to the site's tangent point.
    pub dir: DVec3,
    /// Unit east and north axes at the site, in body-fixed coordinates.
    pub east: DVec3,
    pub north: DVec3,
    /// Radius of the body used to convert angular gnomonic coordinates to metres.
    pub radius_m: f64,
    /// Half side of the DEM square in metres.
    pub half_extent: f64,
    /// Width of the source-driven transition from the DEM to the mean sphere.
    /// This is supplied by the composed terrain source, not baked into the mesh.
    pub blend_m: f64,
}

/// A globe tile's local source-driven handoff. The DEM is clipped out of the
/// globe inside the exact authored square; the source supplies the boundary and
/// collar heights for the surviving globe triangles.
#[derive(Clone, Copy)]
pub struct GlobeSurfacePatch<'a> {
    pub handoff: GlobeHandoff,
    pub source: &'a dyn HeightSource,
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
    patch: Option<GlobeSurfacePatch<'_>>,
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
            let (position, normal) =
                surface_vertex(pos_sphere, radius, tile_center, patch.as_ref());
            positions.push(position);
            normals.push(normal);
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

    if let Some(patch) =
        patch.filter(|patch| dem_square_intersects_tile(&patch.handoff, &directions))
    {
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
            if dirs.iter().all(|&d| patch.handoff.contains(d)) {
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
                let polygon = clip_triangle_to_region(&dirs, &patch.handoff, region);
                if polygon.len() < 3 {
                    continue;
                }
                for k in 1..polygon.len() - 1 {
                    let triangle = [polygon[0], polygon[k], polygon[k + 1]];
                    let first = clipped_positions.len() as u32;
                    for v in triangle {
                        let dir = interpolate_dir(&dirs, v.bary);
                        let uv = interpolate_uv(&base_uv, v.bary);
                        let (position, normal) =
                            surface_vertex(dir, radius, tile_center, Some(&patch));
                        clipped_positions.push(position);
                        clipped_normals.push(normal);
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

impl GlobeHandoff {
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

    pub fn contains(self, direction: DVec3) -> bool {
        let Some([x, z]) = self.coordinates(direction) else {
            return false;
        };
        x.abs() <= self.half_extent && z.abs() <= self.half_extent
    }

    fn collar_distance(self, direction: DVec3) -> Option<f64> {
        let [x, z] = self.coordinates(direction)?;
        Some(
            (x.abs() - self.half_extent)
                .max(0.0)
                .hypot((z.abs() - self.half_extent).max(0.0)),
        )
    }
}

/// Evaluate the source-driven handoff in body-local coordinates.
///
/// `CompositeHeightSource` already performs the one height transition from the
/// retained DEM to the sphere source. The mesh transition below therefore only
/// changes the tangent-plane parameterisation into the exact radial sphere
/// parameterisation. Blending the complete position here as well would blend the
/// elevation twice and create the artificial dark wall this handoff removes.
fn surface_vertex(
    direction: DVec3,
    radius: f64,
    tile_center: DVec3,
    patch: Option<&GlobeSurfacePatch<'_>>,
) -> ([f32; 3], [f32; 3]) {
    let Some(patch) = patch else {
        return (
            (direction * radius - tile_center).as_vec3().into(),
            direction.as_vec3().into(),
        );
    };
    let Some([x, z_body_north]) = patch.handoff.coordinates(direction) else {
        return (
            (direction * radius - tile_center).as_vec3().into(),
            direction.as_vec3().into(),
        );
    };
    let Some(collar_distance) = patch.handoff.collar_distance(direction) else {
        return (
            (direction * radius - tile_center).as_vec3().into(),
            direction.as_vec3().into(),
        );
    };
    if collar_distance > patch.handoff.blend_m {
        return (
            (direction * radius - tile_center).as_vec3().into(),
            direction.as_vec3().into(),
        );
    }

    // Scene +Z is south in the ENU convention, while this globe handoff stores
    // the body-fixed north coordinate. Convert once at the source boundary so
    // the DEM's relief is not mirrored north/south.
    let z_scene = -z_body_north;
    let t = if patch.handoff.blend_m > 0.0 {
        smoothstep(collar_distance / patch.handoff.blend_m)
    } else {
        0.0
    };
    let position = surface_position(
        &patch.handoff,
        patch.source,
        radius,
        x,
        z_body_north,
        z_scene,
        t,
    );
    let epsilon = (radius * 1.0e-6).max(1.0);
    let normal = surface_normal(
        &patch.handoff,
        patch.source,
        radius,
        x,
        z_body_north,
        epsilon,
    );
    (
        (position - tile_center).as_vec3().into(),
        normal.as_vec3().into(),
    )
}

/// Position the composed surface at tangent-plane coordinates `(x, z_north)`.
///
/// The local terrain is a height graph over the site's tangent plane. The
/// sphere source is the same surface expressed through gnomonic coordinates;
/// at the outer collar edge its horizontal coordinates are therefore `x/q` and
/// `z/q`, where `q = sqrt(1 + (x² + z²) / R²)`. Interpolating only those
/// coordinates while taking the height from the composed source makes the
/// outer edge exactly `direction * R` and leaves the authored DEM unchanged.
fn surface_position(
    handoff: &GlobeHandoff,
    source: &dyn HeightSource,
    radius: f64,
    x: f64,
    z_body_north: f64,
    z_scene: f64,
    t: f64,
) -> DVec3 {
    let source_height = source.height_at(x, z_scene);
    let q = (1.0 + (x * x + z_body_north * z_body_north) / (radius * radius)).sqrt();
    let radial_x = x / q;
    let radial_z = z_body_north / q;
    let tangent_x = x.lerp(radial_x, t);
    let tangent_z = z_body_north.lerp(radial_z, t);
    handoff.dir * (radius + source_height) + handoff.east * tangent_x + handoff.north * tangent_z
}

/// Derive the normal from the actual composed position function. This keeps the
/// normal continuous at both collar boundaries, including the DEM's analytic
/// modifiers, instead of approximating it by blending two unrelated normals.
fn surface_normal(
    handoff: &GlobeHandoff,
    source: &dyn HeightSource,
    radius: f64,
    x: f64,
    z_body_north: f64,
    epsilon: f64,
) -> DVec3 {
    let position_at = |x: f64, z_body_north: f64| {
        let direction =
            (handoff.dir + handoff.east * (x / radius) + handoff.north * (z_body_north / radius))
                .normalize();
        let Some(collar_distance) = handoff.collar_distance(direction) else {
            return direction * radius;
        };
        let t = if collar_distance <= handoff.blend_m && handoff.blend_m > 0.0 {
            smoothstep(collar_distance / handoff.blend_m)
        } else {
            1.0
        };
        surface_position(handoff, source, radius, x, z_body_north, -z_body_north, t)
    };
    let east_derivative =
        position_at(x + epsilon, z_body_north) - position_at(x - epsilon, z_body_north);
    let north_derivative =
        position_at(x, z_body_north + epsilon) - position_at(x, z_body_north - epsilon);
    let normal = north_derivative.cross(east_derivative).normalize_or_zero();
    if normal.dot(handoff.dir) < 0.0 {
        -normal
    } else {
        normal
    }
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn interpolate_dir(dirs: &[DVec3; 3], bary: [f64; 3]) -> DVec3 {
    (dirs[0] * bary[0] + dirs[1] * bary[1] + dirs[2] * bary[2]).normalize()
}

/// Conservative tile-level rejection for the exact cutout. A tile that is
/// wholly behind the tangent plane cannot contain the local DEM, and a tile
/// wholly in front whose projected AABB misses the square cannot contain it
/// either. Boundary/ambiguous tiles take the exact per-triangle path below.
fn dem_square_intersects_tile(handoff: &GlobeHandoff, directions: &[DVec3]) -> bool {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    for &direction in directions {
        let Some([x, z]) = handoff.coordinates(direction) else {
            return true;
        };
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    !(max_x < -handoff.half_extent
        || min_x > handoff.half_extent
        || max_z < -handoff.half_extent
        || min_z > handoff.half_extent)
}

fn interpolate_uv(uvs: &[Vec2; 3], bary: [f64; 3]) -> Vec2 {
    Vec2::new(
        (uvs[0].x as f64 * bary[0] + uvs[1].x as f64 * bary[1] + uvs[2].x as f64 * bary[2]) as f32,
        (uvs[0].y as f64 * bary[0] + uvs[1].y as f64 * bary[1] + uvs[2].y as f64 * bary[2]) as f32,
    )
}

fn clip_triangle_to_region(
    dirs: &[DVec3; 3],
    handoff: &GlobeHandoff,
    region: u8,
) -> Vec<ClipVertex> {
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
            (1.0, 0.0, handoff.half_extent),
        ], // x <= -h
        1 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (-1.0, 0.0, handoff.half_extent),
        ], // x >= h
        2 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, 1.0, handoff.half_extent),
            (-1.0, 0.0, -handoff.half_extent),
            (1.0, 0.0, -handoff.half_extent),
        ],
        3 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, -1.0, handoff.half_extent),
            (-1.0, 0.0, -handoff.half_extent),
            (1.0, 0.0, -handoff.half_extent),
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
            let fs = region_value(start, dirs, handoff, region, a, b, c);
            let fe = region_value(end, dirs, handoff, region, a, b, c);
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
    handoff: &GlobeHandoff,
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
        return raw.dot(handoff.dir);
    }
    let denominator = raw.dot(handoff.dir);
    if a == 0.0 && b == 0.0 && c == 0.0 {
        return -denominator;
    }
    // Multiply the gnomonic half-plane by its positive denominator. This keeps
    // the clipping function finite when an edge crosses the tangent horizon;
    // using `INFINITY` for the back endpoint made `inf / inf` produce NaN
    // intersection barycentrics.
    a * raw.dot(handoff.east) * handoff.radius_m
        + b * raw.dot(handoff.north) * handoff.radius_m
        + c * denominator
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    fn handoff(half_extent: f64) -> GlobeHandoff {
        GlobeHandoff {
            dir: DVec3::X,
            east: DVec3::Z,
            north: DVec3::Y,
            radius_m: 100.0,
            half_extent,
            blend_m: half_extent,
        }
    }

    #[derive(Clone, Copy)]
    struct Flat(f64);

    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            self.0
        }
    }

    #[derive(Clone, Copy)]
    struct Sphere(f64);

    impl HeightSource for Sphere {
        fn height_at(&self, x: f64, z: f64) -> f64 {
            self.0 / (1.0 + (x * x + z * z) / self.0.powi(2)).sqrt() - self.0
        }
    }

    #[test]
    fn handoff_uses_the_authored_tangent_square() {
        let c = handoff(10.0);
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
            Some(GlobeSurfacePatch {
                handoff: handoff(1.0e9),
                source: &Flat(0.0),
            }),
        );
        assert!(mesh.indices().is_none_or(|indices| indices.is_empty()));
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
            Some(GlobeSurfacePatch {
                handoff: handoff(1.0),
                source: &Flat(0.0),
            }),
        );
        assert!(mesh.indices().is_some_and(|indices| indices.len() == 24));
    }

    #[test]
    fn clipping_across_the_tangent_horizon_stays_finite() {
        let c = handoff(10.0);
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

    #[test]
    fn source_handoff_meets_site_and_radial_globe_at_collar_edges() {
        let c = GlobeHandoff {
            dir: DVec3::X,
            east: DVec3::Z,
            north: DVec3::Y,
            radius_m: 100.0,
            half_extent: 10.0,
            blend_m: 10.0,
        };
        let source = Flat(-20.0);
        let patch = GlobeSurfacePatch {
            handoff: c,
            source: &source,
        };
        let direction_at = |x: f64| DVec3::new(1.0, 0.0, x / 100.0).normalize();
        let (inner, _) = surface_vertex(direction_at(10.0), 100.0, DVec3::ZERO, Some(&patch));
        let inner = DVec3::new(inner[0] as f64, inner[1] as f64, inner[2] as f64);
        let expected_inner = DVec3::X * 80.0 + DVec3::Z * 10.0;
        assert!((inner - expected_inner).length() < 1.0e-5);

        // The source height is not blended a second time by the mesh
        // parameterisation. At the collar midpoint it remains the authored
        // -20 m source height, rather than becoming -10 m.
        let midpoint_direction = direction_at(15.0);
        let (midpoint, _) = surface_vertex(midpoint_direction, 100.0, DVec3::ZERO, Some(&patch));
        let midpoint = DVec3::new(midpoint[0] as f64, midpoint[1] as f64, midpoint[2] as f64);
        let q = (1.0 + 15.0_f64.powi(2) / 100.0_f64.powi(2)).sqrt();
        let expected_midpoint = DVec3::X * 80.0 + DVec3::Z * (15.0 + (15.0 / q - 15.0) * 0.5);
        assert!((midpoint - expected_midpoint).length() < 1.0e-5);

        let sphere = Sphere(100.0);
        let patch = GlobeSurfacePatch {
            handoff: c,
            source: &sphere,
        };
        let outer_direction = direction_at(20.0);
        let (outer, _) = surface_vertex(outer_direction, 100.0, DVec3::ZERO, Some(&patch));
        let outer = DVec3::new(outer[0] as f64, outer[1] as f64, outer[2] as f64);
        assert!((outer - outer_direction * 100.0).length() < 1.0e-5);
    }

    #[test]
    fn clipped_triangles_keep_outward_winding() {
        let dir = DVec3::new(1.0, 1.0, 1.0).normalize();
        let east = DVec3::new(1.0, -1.0, 0.0).normalize();
        let north = dir.cross(east).normalize();
        let patch = GlobeSurfacePatch {
            handoff: GlobeHandoff {
                dir,
                east,
                north,
                radius_m: 100.0,
                half_extent: 40.0,
                blend_m: 40.0,
            },
            source: &Flat(0.0),
        };

        for face in 0..6 {
            let mesh = create_quadsphere_tile_mesh(
                Entity::PLACEHOLDER,
                face,
                1,
                0,
                0,
                100.0,
                8,
                DVec3::ZERO,
                Some(patch),
            );
            let VertexAttributeValues::Float32x3(positions) = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .expect("globe mesh positions")
            else {
                panic!("globe positions have an unexpected format");
            };
            let VertexAttributeValues::Float32x3(normals) = mesh
                .attribute(Mesh::ATTRIBUTE_NORMAL)
                .expect("globe mesh normals")
            else {
                panic!("globe normals have an unexpected format");
            };
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("globe mesh indices");
            };

            for triangle in indices.chunks_exact(3) {
                let a = DVec3::from_array(positions[triangle[0] as usize].map(f64::from));
                let b = DVec3::from_array(positions[triangle[1] as usize].map(f64::from));
                let c = DVec3::from_array(positions[triangle[2] as usize].map(f64::from));
                let geometric = (b - a).cross(c - a);
                if geometric.length_squared() < 1.0e-12 {
                    continue;
                }
                let supplied = DVec3::from_array(normals[triangle[0] as usize].map(f64::from))
                    + DVec3::from_array(normals[triangle[1] as usize].map(f64::from))
                    + DVec3::from_array(normals[triangle[2] as usize].map(f64::from));
                assert!(
                    geometric.dot(supplied) > 0.0,
                    "face {face} has inward clipped triangle: {:?}",
                    triangle
                );
            }
        }
    }

    #[test]
    fn clipped_mesh_stays_on_the_body_shell_at_the_tangent_horizon() {
        let lat = 25.28_f64.to_radians();
        let lon = 307.60_f64.to_radians();
        let dir = DVec3::new(lat.cos() * lon.cos(), lat.sin(), -lat.cos() * lon.sin());
        let east = DVec3::new(-lon.sin(), 0.0, -lon.cos()).normalize();
        let north = dir.cross(east).normalize();
        let radius = 1_737_400.0;
        let patch = GlobeSurfacePatch {
            handoff: GlobeHandoff {
                dir,
                east,
                north,
                radius_m: radius,
                half_extent: 1_000.0,
                blend_m: 75_000.0,
            },
            source: &Flat(-1_588.0),
        };

        for face in 0..6 {
            let tile_center = cube_to_sphere(face, 0.0, 0.0) * radius;
            let mesh = create_quadsphere_tile_mesh(
                Entity::PLACEHOLDER,
                face,
                0,
                0,
                0,
                radius,
                32,
                tile_center,
                Some(patch),
            );
            let VertexAttributeValues::Float32x3(positions) = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .expect("globe mesh positions")
            else {
                panic!("globe positions have an unexpected format");
            };
            for position in positions {
                let body_position = tile_center + DVec3::from_array(position.map(f64::from));
                assert!(
                    (body_position.length() - radius).abs() < 5_000.0,
                    "face {face} generated a horizon spike at {body_position:?} (r={})",
                    body_position.length()
                );
            }
        }
    }
}
