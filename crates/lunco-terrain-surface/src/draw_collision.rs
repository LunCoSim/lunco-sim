//! Camera-volume admission against the terrain geometry submitted for its views.
//!
//! This is a presentation query: mesh f32 positions and the admitted render
//! transform are widened once through the existing active-grid conversion. The
//! source oracle remains the physical terrain authority. Unprepared or changing
//! geometry returns unavailable, allowing the camera owner to request refinement
//! without moving through a coarse stand-in or sliding on its approximation.

use avian3d::parry::{
    math::Pose,
    query::{ShapeCastOptions, cast_shapes, contact},
    shape::{Ball, ConvexPolyhedron},
};
use bevy::{
    ecs::system::SystemParam,
    math::DVec3,
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
};
use big_space::prelude::Grid;
use lunco_materials::{
    ATTRIBUTE_MORPH_EDGE, ATTRIBUTE_MORPH_TARGET, ParamValue, ShaderLook, ShaderLookReady,
};
use lunco_spatial::{
    ActivePhysicsFrame,
    coords::{
        GridPos, GridRot, RenderCoordinateCertificate, RenderPos, RenderPrecisionQuery,
        grid_absolute_pose_to_render, render_to_grid_absolute,
    },
};

use crate::{
    oracle::DemHeightField,
    stream_viz::{LodTiles, TerrainDetailDemands, TerrainDrawBounds},
    surface_query::{TerrainPoseInPhysicsFrame, terrain_up_translation},
};

/// Result of testing a camera sphere against every consumed output of its source.
#[derive(Debug)]
pub enum DrawTerrainSweep {
    Clear,
    Invalid {
        terrain: Option<Entity>,
        reason: String,
    },
    Refused {
        terrain: Entity,
        target: GridPos,
        reason: String,
    },
    /// A fixed full-detail hit admits a shorter straight segment; coarse
    /// contacts require preparation before any requested motion is published.
    Hit {
        terrain: Entity,
        time: f64,
        point: GridPos,
        normal: DVec3,
        refinement_required: bool,
    },
    /// The finite prism contact depth lower-bounds penetration of the complete solid.
    InitialOverlap {
        terrain: Entity,
        depth_m: f64,
        normal: DVec3,
    },
    Unavailable {
        terrain: Option<Entity>,
        reason: String,
    },
}

/// Retain the earliest contact across all consumed outputs. Equal-time
/// uncertainty takes precedence over an otherwise admissible fine contact.
#[derive(Clone, Copy)]
struct DrawContact {
    terrain: Entity,
    time: f64,
    point: GridPos,
    normal: DVec3,
    refinement_required: bool,
    refinement_refused: bool,
}

impl DrawContact {
    fn record(self, best: &mut Option<Self>) {
        if best.is_none_or(|previous| {
            self.time < previous.time
                || (self.time == previous.time
                    && (self.refinement_required, self.refinement_refused)
                        > (previous.refinement_required, previous.refinement_refused))
        }) {
            *best = Some(self);
        }
    }
}

/// Read the exact retained draw mesh without borrowing camera `Transform`s.
#[derive(SystemParam)]
pub struct DrawTerrainQuery<'w, 's> {
    terrains: Query<
        'w,
        's,
        (
            Entity,
            &'static LodTiles,
            &'static TerrainPoseInPhysicsFrame,
            &'static DemHeightField,
        ),
    >,
    tiles: Query<
        'w,
        's,
        (
            &'static Mesh3d,
            &'static ShaderLook,
            &'static GlobalTransform,
            Option<&'static TerrainDrawBounds>,
            Has<ShaderLookReady>,
            &'static ChildOf,
        ),
    >,
    globals: Query<'w, 's, &'static GlobalTransform>,
    precision: RenderPrecisionQuery<'w, 's>,
    grids: Query<'w, 's, &'static Grid>,
    meshes: Option<Res<'w, Assets<Mesh>>>,
    frame: Option<Res<'w, ActivePhysicsFrame>>,
    demands: Option<Res<'w, TerrainDetailDemands>>,
}

fn unavailable(terrain: Option<Entity>, message: impl Into<String>) -> DrawTerrainSweep {
    DrawTerrainSweep::Unavailable {
        terrain,
        reason: message.into(),
    }
}

fn invalid(terrain: Option<Entity>, message: impl Into<String>) -> DrawTerrainSweep {
    DrawTerrainSweep::Invalid {
        terrain,
        reason: message.into(),
    }
}

fn scalar(look: &ShaderLook, name: &str) -> Option<f32> {
    match look.values().get(name)? {
        ParamValue::F32(value) => Some(*value),
        _ => None,
    }
}

/// The shader clamps smoothstep to exact endpoint values. Interior builtin
/// approximation is enclosed by its full codomain, so CPU admission never
/// assumes an unverified GPU polynomial implementation or contraction rule.
fn shader_morph_interval(
    minimum: f64,
    maximum: f64,
    error: f64,
    start: f64,
    end: f64,
    edge: f64,
) -> (f64, f64) {
    if end <= start {
        return (edge, edge);
    }
    let lower: f64 = if minimum - error >= end { 1.0 } else { 0.0 };
    let upper: f64 = if maximum + error <= start { 0.0 } else { 1.0 };
    (lower.max(edge), upper.max(edge))
}

fn segment_distance(point: DVec3, start: DVec3, end: DVec3) -> f64 {
    let delta = end - start;
    let t = if delta.length_squared() > 0.0 {
        ((point - start).dot(delta) / delta.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    point.distance(start + delta * t)
}

fn overlaps_xz(a: DVec3, b: DVec3, low: DVec3, high: DVec3, radius: f64) -> bool {
    a.x.min(b.x) - radius <= high.x
        && a.x.max(b.x) + radius >= low.x
        && a.z.min(b.z) - radius <= high.z
        && a.z.max(b.z) + radius >= low.z
}

fn overlaps_bounds(a: DVec3, b: DVec3, low: DVec3, high: DVec3, radius: f64) -> bool {
    overlaps_xz(a, b, low, high, radius) && a.y.min(b.y) - radius <= high.y
}

/// A separating slab certifies the entire solid fine/parent convex hull clear of the
/// swept sphere. Face-normal candidates retain slope correlation discarded by
/// an axis-aligned box; absence of separation remains uncertainty, never a hit.
fn hull_separates_sphere(
    points: &[DVec3],
    start: DVec3,
    end: DVec3,
    radius: f64,
    inverse_transpose: bevy::math::DMat4,
) -> bool {
    for a in 0..points.len().saturating_sub(2) {
        for b in a + 1..points.len() - 1 {
            for c in b + 1..points.len() {
                let axis = (points[b] - points[a]).cross(points[c] - points[a]);
                let length = axis.length();
                if !length.is_finite() || length <= f64::EPSILON {
                    continue;
                }
                let axis = axis / length;
                let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
                for point in points {
                    let value = point.dot(axis);
                    low = low.min(value);
                    high = high.max(value);
                }
                let from = start.dot(axis);
                let to = end.dot(axis);
                let radius = radius * inverse_transpose.transform_vector3(axis).length();
                let pad = 64.0
                    * f64::EPSILON
                    * low
                        .abs()
                        .max(high.abs())
                        .max(from.abs())
                        .max(to.abs())
                        .max(radius)
                        .max(1.0);
                if from.min(to) - radius > high + pad || from.max(to) + radius < low - pad {
                    return true;
                }
            }
        }
    }
    false
}

/// Restrict regular cells to the swept tube using the source sweep's conservative
/// time slabs. Boundary-remeshed tiles retain their explicit edge/corner indices.
fn candidate_triangles(
    mesh: &Mesh,
    coord: lunco_terrain_core::QuadCoord,
    resolution: usize,
    half: f64,
    start: DVec3,
    end: DVec3,
    radius: f64,
    remaining: &mut usize,
) -> Result<Vec<[usize; 3]>, String> {
    let indices = mesh
        .indices()
        .ok_or("presented terrain mesh indices are unavailable")?;
    if indices.len() % 3 != 0 || resolution < 3 {
        return Err("presented terrain mesh topology is invalid".into());
    }
    let last = (1u32 << coord.depth) - 1;
    let left = usize::from(coord.x == 0);
    let right = usize::from(coord.x == last);
    let top = usize::from(coord.z == 0);
    let bottom = usize::from(coord.z == last);
    let step = 2.0 * half / (resolution - 1) as f64;
    let cell = |value: f64| (((value + half) / step).floor() as usize).min(resolution - 2);
    let lo = start.min(end) - DVec3::splat(radius);
    let hi = start.max(end) + DVec3::splat(radius);
    let (x0, x1, z0, z1) = (cell(lo.x), cell(hi.x), cell(lo.z), cell(hi.z));
    let touches_boundary =
        !(x0 >= left && x1 < resolution - 1 - right && z0 >= top && z1 < resolution - 1 - bottom);
    let at = |index: usize| -> Option<usize> {
        match indices {
            Indices::U16(values) => values.get(index).map(|value| usize::from(*value)),
            Indices::U32(values) => values.get(index).map(|value| *value as usize),
        }
    };
    let mut result = Vec::new();
    let delta = end - start;
    for z in z0.max(top)..=z1.min(resolution - 2 - bottom) {
        *remaining = remaining
            .checked_sub(1)
            .ok_or("presented terrain swept-row budget exhausted")?;
        let row = [
            -half,
            -half + z as f64 * step,
            half,
            -half + (z + 1) as f64 * step,
        ];
        let Some(clipped) = crate::surface_query::clip_to_swept_tube(start, delta, row, radius)
        else {
            continue;
        };
        let from = cell(clipped[0]).max(x0).max(left);
        let to = cell(clipped[2]).min(x1).min(resolution - 2 - right);
        for x in from..=to {
            let rect = [
                -half + x as f64 * step,
                row[1],
                -half + (x + 1) as f64 * step,
                row[3],
            ];
            if crate::surface_query::clip_to_swept_tube(start, delta, rect, radius).is_none() {
                continue;
            }
            *remaining = remaining
                .checked_sub(2)
                .ok_or("presented terrain swept-cell budget exhausted")?;
            let offset = ((z - top) * (resolution - 1 - left - right) + x - left) * 6;
            for triangle in 0..2 {
                let offset = offset + triangle * 3;
                let (Some(a), Some(b), Some(c)) = (at(offset), at(offset + 1), at(offset + 2))
                else {
                    return Err("presented terrain cell indices are incomplete".into());
                };
                result.push([a, b, c]);
            }
        }
    }
    if touches_boundary {
        // Boundary remeshing owns one regular interior prefix followed by only
        // the edge/corner triangles; do not rescan every interior cell.
        let regular_count = (resolution - 1 - left - right) * (resolution - 1 - top - bottom) * 6;
        if regular_count > indices.len() {
            return Err("presented terrain boundary prefix is incomplete".into());
        }
        let tail = (indices.len() - regular_count) / 3;
        *remaining = remaining
            .checked_sub(tail)
            .ok_or("presented terrain boundary triangle budget exhausted")?;
        let mut indices = indices.iter().skip(regular_count);
        while let Some(a) = indices.next() {
            let (Some(b), Some(c)) = (indices.next(), indices.next()) else {
                return Err("presented terrain index count is not triangular".into());
            };
            result.push([a, b, c]);
        }
    }
    Ok(result)
}

#[derive(Debug)]
enum PrismSweep {
    Clear,
    Hit {
        time: f64,
        point: DVec3,
        normal: DVec3,
    },
    Overlap {
        depth: f64,
        normal: DVec3,
    },
}

/// Maintained Parry f64 owns contact and sphere/solid-prism CCD.
fn sweep_prism(
    vertices: [DVec3; 6],
    start: DVec3,
    end: DVec3,
    radius: f64,
    up: DVec3,
    tolerance: f64,
) -> Result<PrismSweep, String> {
    if vertices.iter().any(|point| !point.is_finite())
        || !up.is_finite()
        || up.length_squared() == 0.0
        || !tolerance.is_finite()
        || tolerance <= 0.0
    {
        return Err("presented terrain prism exceeds its coordinate range".into());
    }
    // The three corresponding top/bottom pairs have a fixed convex prism
    // topology. Orient each face after the affine transform, including mirrors.
    let centroid = vertices.iter().copied().sum::<DVec3>() / 6.0;
    let mut faces = [
        [0_u32, 1, 2],
        [3, 5, 4],
        [0, 3, 4],
        [0, 4, 1],
        [1, 4, 5],
        [1, 5, 2],
        [2, 5, 3],
        [2, 3, 0],
    ];
    for face in &mut faces {
        let [a, b, c] = (*face).map(|index| vertices[index as usize]);
        let outward = (b - a).cross(c - a).dot(a - centroid);
        if !outward.is_finite() || outward == 0.0 {
            return Err("presented terrain contains a degenerate solid prism face".into());
        }
        if outward < 0.0 {
            face.swap(1, 2);
        }
    }
    let shape = ConvexPolyhedron::from_convex_mesh(vertices.to_vec(), &faces)
        .ok_or("presented terrain contains invalid solid prism topology")?;
    let ball = Ball::new(radius);
    let origin = Pose::from_translation(start);
    if let Some(hit) = contact(&origin, &ball, &Pose::IDENTITY, &shape, 0.0)
        .map_err(|error| format!("presented terrain contact unsupported: {error:?}"))?
    {
        if !hit.dist.is_finite() || !hit.normal2.is_finite() {
            return Err("presented terrain contact is non-finite".into());
        }
        if hit.dist < 0.0 {
            let delta = end - start;
            let products = hit.normal2 * delta;
            let pad = 16.0 * f64::EPSILON * products.abs().element_sum();
            // A separating ray cannot worsen this convex solid's contact.
            // The finite bottom is outside the camera's reachable Y interval;
            // its artificial outward normal cannot authorize soil recovery.
            if delta.length_squared() > 0.0
                && hit.normal2.dot(up) >= 0.0
                && products.element_sum() >= pad
            {
                return Ok(PrismSweep::Clear);
            }
            if -hit.dist <= tolerance {
                return Ok(PrismSweep::Hit {
                    time: 0.0,
                    point: hit.point2,
                    normal: hit.normal2,
                });
            }
            return Ok(PrismSweep::Overlap {
                depth: -hit.dist,
                normal: hit.normal2,
            });
        }
    }
    let hit = cast_shapes(
        &origin,
        end - start,
        &ball,
        &Pose::IDENTITY,
        DVec3::ZERO,
        &shape,
        ShapeCastOptions {
            max_time_of_impact: 1.0,
            stop_at_penetration: false,
            ..default()
        },
    )
    .map_err(|error| format!("presented terrain sweep unsupported: {error:?}"))?;
    Ok(match hit {
        Some(hit) => {
            if !hit.time_of_impact.is_finite()
                || !(0.0..=1.0).contains(&hit.time_of_impact)
                || !hit.witness2.is_finite()
                || !hit.normal2.is_finite()
            {
                return Err("presented terrain sweep is non-finite or outside its interval".into());
            }
            PrismSweep::Hit {
                time: hit.time_of_impact,
                point: hit.witness2,
                normal: hit.normal2,
            }
        }
        None => PrismSweep::Clear,
    })
}

fn solid_bottom(
    points: &[DVec3],
    start: DVec3,
    end: DVec3,
    local_radius: f64,
    tolerance: f64,
) -> Result<f64, String> {
    let low = points
        .iter()
        .map(|point| point.y)
        .fold(start.y.min(end.y), f64::min);
    let bottom = (low - local_radius - tolerance).next_down();
    if !bottom.is_finite() || bottom >= low {
        return Err("presented terrain solid extrusion exceeds finite coordinates".into());
    }
    Ok(bottom)
}

struct TriangleVertices {
    fine: [DVec3; 3],
    parent: [DVec3; 3],
    edge_weights: [f64; 3],
    low: DVec3,
    high: DVec3,
}

fn triangle_vertices(
    positions: &[[f32; 3]],
    targets: &[[f32; 3]],
    edges: &[[f32; 4]],
    stitch: [f32; 4],
    indices: [usize; 3],
) -> Result<TriangleVertices, String> {
    let mut result = TriangleVertices {
        fine: [DVec3::ZERO; 3],
        parent: [DVec3::ZERO; 3],
        edge_weights: [0.0; 3],
        low: DVec3::splat(f64::INFINITY),
        high: DVec3::splat(f64::NEG_INFINITY),
    };
    for (slot, index) in indices.into_iter().enumerate() {
        let (Some(position), Some(target), Some(edge)) =
            (positions.get(index), targets.get(index), edges.get(index))
        else {
            return Err("presented terrain index exceeds its vertex arrays".into());
        };
        let fine = Vec3::from_array(*position).as_dvec3();
        let parent = Vec3::from_array(*target).as_dvec3();
        if !fine.is_finite()
            || !parent.is_finite()
            || edge
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("presented terrain contains invalid vertex data".into());
        }
        result.fine[slot] = fine;
        result.parent[slot] = parent;
        result.low = result.low.min(fine).min(parent);
        result.high = result.high.max(fine).max(parent);
        result.edge_weights[slot] = edge
            .iter()
            .zip(stitch.iter())
            .map(|(a, b)| f64::from(a * b))
            .fold(0.0, f64::max);
    }
    Ok(result)
}

fn morph_mix_error(matrix: bevy::math::DMat4, magnitude: DVec3) -> f64 {
    let unit = f64::from(f32::EPSILON);
    let gamma4 = 4.0 * unit / (1.0 - 4.0 * unit);
    2.0 * gamma4
        * (matrix.x_axis.truncate().abs() * magnitude.x
            + matrix.y_axis.truncate().abs() * magnitude.y
            + matrix.z_axis.truncate().abs() * magnitude.z)
            .length()
}

/// Camera and shader-eye motion use the same normalized time. Each interval
/// encloses only positions the shader can produce during that camera segment.
struct TriangleMotion {
    matrix: bevy::math::DMat4,
    inverse_transpose: bevy::math::DMat4,
    camera_start: DVec3,
    camera_end: DVec3,
    local_start: DVec3,
    local_end: DVec3,
    eye_start: DVec3,
    eye_end: DVec3,
    morph_start: f64,
    morph_end: f64,
    radius: f64,
    clearance: f64,
    render_error: f64,
    distance_error: f64,
    tolerance: f64,
}

enum MorphSweep {
    Clear,
    Hit {
        time: f64,
        point: DVec3,
        normal: DVec3,
        full_fine: bool,
    },
    Overlap {
        depth: f64,
        normal: DVec3,
        full_fine: bool,
    },
    Preparation,
}

impl TriangleMotion {
    fn sweep(
        &self,
        fine: [DVec3; 3],
        parent: [DVec3; 3],
        edges: [f64; 3],
        remaining: &mut usize,
        to_grid: impl Fn(DVec3) -> DVec3,
    ) -> Result<MorphSweep, String> {
        let world = fine.map(|point| self.matrix.transform_point3(point));
        let travel = (self.camera_end - self.camera_start).length();
        if !travel.is_finite() {
            return Err("presented terrain camera interval exceeds finite coordinates".into());
        }
        let mut intervals = vec![(0.0_f64, 1.0_f64)];
        while let Some((lower, upper)) = intervals.pop() {
            *remaining = remaining
                .checked_sub(1)
                .ok_or("presented terrain temporal sweep budget exhausted")?;
            let eye_start = self.eye_start.lerp(self.eye_end, lower);
            let eye_end = self.eye_start.lerp(self.eye_end, upper);
            let local_start = self.local_start.lerp(self.local_end, lower);
            let local_end = self.local_start.lerp(self.local_end, upper);
            let start = self.camera_start.lerp(self.camera_end, lower);
            let end = self.camera_start.lerp(self.camera_end, upper);
            let mut hull = [DVec3::ZERO; 12];
            let mut varying = false;
            let mut full_fine = true;
            for slot in 0..3 {
                let minimum_distance = segment_distance(world[slot], eye_start, eye_end);
                let maximum_distance = world[slot]
                    .distance(eye_start)
                    .max(world[slot].distance(eye_end));
                let distance_error = self.distance_error;
                if !minimum_distance.is_finite()
                    || !maximum_distance.is_finite()
                    || !distance_error.is_finite()
                {
                    return Err(
                        "presented terrain morph interval exceeds finite coordinates".into(),
                    );
                }
                let (minimum, maximum) = shader_morph_interval(
                    minimum_distance,
                    maximum_distance,
                    distance_error,
                    self.morph_start,
                    self.morph_end,
                    edges[slot],
                );
                hull[slot * 2] = fine[slot].lerp(parent[slot], minimum);
                hull[slot * 2 + 1] = fine[slot].lerp(parent[slot], maximum);
                varying |= fine[slot] != parent[slot] && minimum != maximum;
                full_fine &= minimum == 0.0 || fine[slot] == parent[slot];
            }
            let inverse_radius = self
                .inverse_transpose
                .x_axis
                .truncate()
                .length()
                .max(self.inverse_transpose.y_axis.truncate().length())
                .max(self.inverse_transpose.z_axis.truncate().length());
            let local_radius = self.clearance * inverse_radius;
            let bottom = solid_bottom(
                &hull[..6],
                self.local_start,
                self.local_end,
                local_radius,
                self.tolerance,
            )?;
            for slot in 0..6 {
                hull[6 + slot] = DVec3::new(hull[slot].x, bottom, hull[slot].z);
            }
            if hull_separates_sphere(
                &hull,
                local_start,
                local_end,
                self.clearance,
                self.inverse_transpose,
            ) {
                continue;
            }
            if !varying {
                let prism = std::array::from_fn(|index| {
                    let local = if index < 3 {
                        hull[index * 2]
                    } else {
                        hull[6 + (index - 3) * 2]
                    };
                    to_grid(self.matrix.transform_point3(local))
                });
                let up = to_grid(self.matrix.transform_point3(DVec3::Y))
                    - to_grid(self.matrix.transform_point3(DVec3::ZERO));
                match sweep_prism(
                    prism,
                    start,
                    end,
                    self.radius + self.render_error,
                    up,
                    self.tolerance,
                )? {
                    PrismSweep::Clear => continue,
                    PrismSweep::Hit {
                        time,
                        point,
                        normal,
                    } => {
                        return Ok(MorphSweep::Hit {
                            time: lower + (upper - lower) * time,
                            point,
                            normal,
                            full_fine,
                        });
                    }
                    PrismSweep::Overlap { depth, normal } if lower == 0.0 => {
                        return Ok(MorphSweep::Overlap {
                            depth,
                            normal,
                            full_fine,
                        });
                    }
                    PrismSweep::Overlap { depth, normal } => {
                        return Ok(MorphSweep::Hit {
                            time: lower,
                            point: start - normal * (self.radius + self.render_error - depth),
                            normal,
                            full_fine,
                        });
                    }
                }
            }
            if (upper - lower) * travel <= self.tolerance {
                return Ok(MorphSweep::Preparation);
            }
            let middle = (lower + upper) * 0.5;
            if middle == lower || middle == upper {
                return Err("presented terrain morph interval exceeds time precision".into());
            }
            // Earlier intervals certify the prefix before later contacts.
            intervals.push((middle, upper));
            intervals.push((lower, middle));
        }
        Ok(MorphSweep::Clear)
    }
}

/// Certify a stationary camera against one candidate publication using the
/// same shader interval, precision bound and solid-prism CCD as navigation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admits_stationary_mesh(
    mesh: &Mesh,
    coord: lunco_terrain_core::QuadCoord,
    resolution: usize,
    domain_half: f64,
    matrix: bevy::math::DMat4,
    eye: DVec3,
    envelope: &lunco_camera_core::CameraCollisionEnvelope,
    morph_start: f32,
    morph_end: f32,
    stitch: [f32; 4],
    certificate: RenderCoordinateCertificate,
    bounds: &TerrainDrawBounds,
    remaining: &mut usize,
) -> Result<bool, String> {
    if !matrix.is_finite()
        || !eye.is_finite()
        || !domain_half.is_finite()
        || domain_half <= 0.0
        || !envelope.radius_m.is_finite()
        || envelope.radius_m <= 0.0
        || !envelope.skin_m.is_finite()
        || envelope.skin_m <= 0.0
        || !envelope.sweep_tolerance_m.is_finite()
        || envelope.sweep_tolerance_m <= 0.0
        || envelope.sweep_tolerance_m >= envelope.radius_m
        || !morph_start.is_finite()
        || !morph_end.is_finite()
        || stitch
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        || !certificate.error_m.is_finite()
        || certificate.error_m < 0.0
        || !certificate.distance_error_m.is_finite()
        || certificate.distance_error_m < 0.0
        || !certificate.translation_bound_m.is_finite()
        || certificate.translation_bound_m < 0.0
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || !bounds.min.cmple(bounds.max).all()
        || !bounds.parent_displacement.is_finite()
        || !bounds.parent_displacement.cmpge(DVec3::ZERO).all()
    {
        return Err("candidate terrain publication has invalid clearance inputs".into());
    }
    let inverse = matrix.inverse();
    let divisions = 1_u32
        .checked_shl(u32::from(coord.depth))
        .ok_or("candidate terrain publication depth exceeds its lattice range")?;
    let half = domain_half / f64::from(divisions);
    if !inverse.is_finite() {
        return Err("candidate terrain publication transform is singular".into());
    }
    let inverse_transpose = inverse.transpose();
    let inverse_radius = inverse_transpose
        .x_axis
        .truncate()
        .length()
        .max(inverse_transpose.y_axis.truncate().length())
        .max(inverse_transpose.z_axis.truncate().length());
    let local = inverse.transform_point3(eye);
    let magnitude = bounds.min.abs().max(bounds.max.abs());
    let render_error = certificate.error_m + morph_mix_error(matrix, magnitude);
    if !local.is_finite() || !render_error.is_finite() || !inverse_radius.is_finite() {
        return Err("candidate terrain publication exceeds its coordinate range".into());
    }
    let selection_radius = (envelope.radius_m + envelope.skin_m + render_error) * inverse_radius;
    if !selection_radius.is_finite() {
        return Err("candidate terrain publication radius exceeds finite coordinates".into());
    }
    if !overlaps_bounds(local, local, bounds.min, bounds.max, selection_radius) {
        return Ok(true);
    }
    if render_error > envelope.skin_m {
        return Ok(false);
    }
    let positions = mesh
        .try_attribute(Mesh::ATTRIBUTE_POSITION)
        .ok()
        .and_then(|attribute| attribute.as_float3())
        .ok_or("candidate terrain has no position array")?;
    let targets = mesh
        .try_attribute(ATTRIBUTE_MORPH_TARGET)
        .ok()
        .and_then(|attribute| attribute.as_float3())
        .ok_or("candidate terrain has no morph position array")?;
    let Some(VertexAttributeValues::Float32x4(edges)) =
        mesh.try_attribute(ATTRIBUTE_MORPH_EDGE).ok()
    else {
        return Err("candidate terrain has no typed edge masks".into());
    };
    let candidate_radius = selection_radius
        + bounds
            .parent_displacement
            .x
            .max(bounds.parent_displacement.z);
    if !candidate_radius.is_finite() {
        return Err("candidate terrain parent displacement exceeds finite coordinates".into());
    }
    let candidates = candidate_triangles(
        mesh,
        coord,
        resolution,
        half,
        local,
        local,
        candidate_radius,
        remaining,
    )?;
    let motion = TriangleMotion {
        matrix,
        inverse_transpose,
        camera_start: eye,
        camera_end: eye,
        local_start: local,
        local_end: local,
        eye_start: eye,
        eye_end: eye,
        morph_start: f64::from(morph_start),
        morph_end: f64::from(morph_end),
        radius: envelope.radius_m,
        clearance: envelope.radius_m + render_error,
        render_error,
        distance_error: certificate.distance_error_m,
        tolerance: envelope.sweep_tolerance_m,
    };
    for indices in candidates {
        let vertices = triangle_vertices(positions, targets, edges, stitch, indices)?;
        if !overlaps_bounds(local, local, vertices.low, vertices.high, selection_radius) {
            continue;
        }
        if !matches!(
            motion.sweep(
                vertices.fine,
                vertices.parent,
                vertices.edge_weights,
                remaining,
                |point| point
            )?,
            MorphSweep::Clear
        ) {
            return Ok(false);
        }
    }
    Ok(true)
}

impl DrawTerrainQuery<'_, '_> {
    /// Revision of the exact consumed draw context for retrying held navigation.
    /// No visual output returns `None`; an output awaiting its first geometry
    /// contributes a stable zero draw revision until publication advances it.
    pub fn revision(&self, camera: Entity) -> Option<u64> {
        let demands = self.demands.as_deref()?;
        let mut cameras = demands.cameras_for_source(camera).collect::<Vec<_>>();
        if cameras.is_empty() || self.terrains.is_empty() {
            return None;
        }
        cameras.sort_unstable_by_key(|camera| camera.to_bits());
        let mut terrains = self.terrains.iter().collect::<Vec<_>>();
        terrains.sort_unstable_by_key(|(terrain, _, _, _)| terrain.to_bits());
        let mut signature = lunco_precompute::Fnv1a::new();
        for (terrain, tiles, pose, field) in terrains {
            signature.write_u64(terrain.to_bits());
            signature.write_u64(field.0.surface_key());
            signature.write_u64(pose.frame.to_bits());
            for view in cameras.iter().copied() {
                let Some((position, forward)) = demands.terrain_pose_snapshot(view, terrain) else {
                    continue;
                };
                signature.write_u64(view.to_bits());
                signature.write_u64(tiles.draw_revision(view).unwrap_or(0));
                for value in position.to_array().into_iter().chain(forward.to_array()) {
                    signature.write_u64(value.to_bits());
                }
            }
        }
        Some(signature.finish())
    }

    /// Certify one admitted movement in active-physics coordinates.
    /// `work` bounds inspected candidate triangles across all matching views.
    /// Missing CPU data and uncertain eye-dependent morph are preparation waits.
    /// The caller first certifies source and prop collision; this query permits
    /// deliberate source-up departure from an initially overlapping draw surface.
    pub fn sweep(
        &self,
        camera: Entity,
        start: GridPos,
        end: GridPos,
        radius: f64,
        skin: f64,
        tolerance_m: f64,
        work: usize,
    ) -> DrawTerrainSweep {
        if self.terrains.is_empty() {
            return DrawTerrainSweep::Clear;
        }
        let Some(demands) = self.demands.as_deref() else {
            return DrawTerrainSweep::Clear;
        };
        let cameras: Vec<_> = demands.cameras_for_source(camera).collect();
        if cameras.is_empty() {
            return DrawTerrainSweep::Clear;
        }
        if !start.0.is_finite()
            || !end.0.is_finite()
            || !radius.is_finite()
            || radius <= 0.0
            || !skin.is_finite()
            || skin < 0.0
            || !tolerance_m.is_finite()
            || tolerance_m <= 0.0
            || tolerance_m >= radius
            || work == 0
        {
            return invalid(
                None,
                "presented terrain sweep has invalid input or no work budget",
            );
        }
        let Some(frame) = self.frame.as_deref() else {
            return unavailable(None, "presented terrain has no active frame");
        };
        let Ok(grid) = self.grids.get(frame.0) else {
            return unavailable(None, "presented terrain active grid is unavailable");
        };
        let Some(meshes) = self.meshes.as_deref() else {
            return unavailable(None, "presented terrain mesh assets are pending");
        };
        let start_render =
            grid_absolute_pose_to_render(grid, start, GridRot(bevy::math::DQuat::IDENTITY))
                .0
                .0;
        let end_render =
            grid_absolute_pose_to_render(grid, end, GridRot(bevy::math::DQuat::IDENTITY))
                .0
                .0;
        let clearance = radius + skin;
        if !clearance.is_finite() || !start_render.is_finite() || !end_render.is_finite() {
            return invalid(None, "presented terrain sweep exceeds its coordinate range");
        }
        let mut remaining = work;
        let mut best: Option<DrawContact> = None;
        for (terrain, tiles, pose, _field) in &self.terrains {
            for view in cameras.iter().copied() {
                // Output admission owns layer and precision-domain participation.
                // A terrain excluded from this view has no geometry to certify.
                if demands.terrain_pose_snapshot(view, terrain).is_none() {
                    continue;
                }
                if pose.frame != frame.0 {
                    return unavailable(
                        Some(terrain),
                        "presented terrain frame lease is unavailable",
                    );
                }
                // Source and native collision already certify this request.
                // Deliberate source-up movement can leave a drawn starting
                // overlap without crossing a new heightfield from above.
                if terrain_up_translation(pose.rotation.inverse() * (end.0 - start.0), tolerance_m)
                {
                    continue;
                }
                let Some(metrics) = tiles.view_status(view) else {
                    return unavailable(
                        Some(terrain),
                        "presented terrain view metrics are preparing",
                    );
                };
                let max_depth = metrics.effective_max_depth;
                if tiles.draw_revision(view).is_none() {
                    return invalid(
                        Some(terrain),
                        "presented terrain draw revision is exhausted",
                    );
                }
                let preparation_refused = demands.has_preparation(view, terrain)
                    && tiles.preparation_refused(view, demands, terrain);
                let Some(draw) = tiles.draw_tiles(view) else {
                    return unavailable(
                        Some(terrain),
                        format!("presented terrain view {view:?} has no admitted cover"),
                    );
                };
                let Ok(eye) = self.globals.get(view) else {
                    return unavailable(
                        Some(terrain),
                        format!("presented terrain view {view:?} pose is pending"),
                    );
                };
                let eye_start = eye.translation().as_dvec3();
                let eye_end = eye_start + (end_render - start_render);
                let mut has_draw = false;
                for (coord, entity, _, _) in draw {
                    has_draw = true;
                    let Some((resolution, domain_half, current_lattice)) =
                        tiles.draw_lattice(view, coord)
                    else {
                        return unavailable(
                            Some(terrain),
                            "presented terrain lattice metadata is pending",
                        );
                    };
                    if resolution < 3 || !domain_half.is_finite() || domain_half <= 0.0 {
                        return invalid(
                            Some(terrain),
                            "presented terrain lattice metadata is invalid",
                        );
                    }
                    let Ok((handle, look, transform, bounds, ready, parent)) =
                        self.tiles.get(entity)
                    else {
                        return unavailable(
                            Some(terrain),
                            format!("presented terrain tile {entity:?} is pending"),
                        );
                    };
                    if !ready {
                        return unavailable(
                            Some(terrain),
                            format!("presented terrain tile {entity:?} material is preparing"),
                        );
                    }
                    let matrix = transform.to_matrix().as_dmat4();
                    let inverse = matrix.inverse();
                    if !inverse.is_finite() {
                        return invalid(
                            Some(terrain),
                            "presented terrain tile transform is singular",
                        );
                    }
                    let local_start = inverse.transform_point3(start_render);
                    let local_end = inverse.transform_point3(end_render);
                    // Each inverse row bounds the camera sphere along that
                    // local axis, including shear from composed ancestors.
                    let inverse_transpose = inverse.transpose();
                    let inverse_radius = inverse_transpose
                        .x_axis
                        .truncate()
                        .length()
                        .max(inverse_transpose.y_axis.truncate().length())
                        .max(inverse_transpose.z_axis.truncate().length());
                    let local_radius = clearance * inverse_radius;
                    if !local_radius.is_finite() {
                        return invalid(
                            Some(terrain),
                            "presented terrain inverse scale exceeds its coordinate range",
                        );
                    }
                    let Some(bounds) = bounds else {
                        return unavailable(
                            Some(terrain),
                            "presented terrain bounds are preparing",
                        );
                    };
                    if !bounds.min.is_finite()
                        || !bounds.max.is_finite()
                        || !bounds.min.cmple(bounds.max).all()
                        || !bounds.parent_displacement.is_finite()
                        || !bounds.parent_displacement.cmpge(DVec3::ZERO).all()
                    {
                        return invalid(Some(terrain), "presented terrain bounds are invalid");
                    }
                    // Skin is the admitted numerical budget at the f32 GPU
                    // boundary. The retained hull bounds all fixed vertices.
                    let magnitude = bounds.min.abs().max(bounds.max.abs());
                    let certificate = match self.precision.certify(
                        camera,
                        frame.0,
                        parent.parent(),
                        eye_start,
                        eye_end,
                        matrix,
                        magnitude,
                    ) {
                        Ok(certificate) => certificate,
                        Err(reason) => return unavailable(Some(terrain), reason),
                    };
                    // WGSL mix(x,y,m) has a subtraction, two multiplies and
                    // an addition. Endpoint magnitudes bound every interpolation
                    // before the mesh's affine transform.
                    let mix_error = morph_mix_error(matrix, magnitude);
                    let render_error = certificate.error_m + mix_error;
                    if !render_error.is_finite() {
                        return invalid(
                            Some(terrain),
                            "presented terrain f32 precision exceeds its coordinate range",
                        );
                    }
                    if !overlaps_bounds(
                        local_start,
                        local_end,
                        bounds.min,
                        bounds.max,
                        (clearance + render_error) * inverse_radius,
                    ) {
                        continue;
                    }
                    if render_error > skin {
                        if coord.depth < max_depth || !current_lattice {
                            if preparation_refused {
                                return DrawTerrainSweep::Refused{terrain,target:end,reason:"camera cover budget refused the detail needed for camera precision".into()};
                            }
                            return unavailable(
                                Some(terrain),
                                "presented terrain needs a finer patch to meet the camera precision budget",
                            );
                        }
                        return invalid(
                            Some(terrain),
                            format!(
                                "presented terrain f32 precision {render_error:.9} m exceeds camera skin {skin:.9} m ({certificate:?}, mix={mix_error:.9} m)"
                            ),
                        );
                    }
                    let half = domain_half / f64::from(1u32 << coord.depth);
                    let local_radius = (clearance + render_error) * inverse_radius;
                    let candidate_radius = local_radius
                        + bounds
                            .parent_displacement
                            .x
                            .max(bounds.parent_displacement.z);
                    if !candidate_radius.is_finite() {
                        return invalid(
                            Some(terrain),
                            "presented terrain parent displacement exceeds finite coordinates",
                        );
                    }
                    if !overlaps_xz(
                        local_start,
                        local_end,
                        DVec3::splat(-half),
                        DVec3::splat(half),
                        candidate_radius,
                    ) {
                        continue;
                    }
                    let Some(mesh) = meshes.get(&handle.0) else {
                        return unavailable(
                            Some(terrain),
                            "presented terrain mesh is not resident",
                        );
                    };
                    let Some(positions) = mesh
                        .try_attribute(Mesh::ATTRIBUTE_POSITION)
                        .ok()
                        .and_then(|attribute| attribute.as_float3())
                    else {
                        return invalid(
                            Some(terrain),
                            "resident presented terrain mesh has no position array",
                        );
                    };
                    let Some(targets) = mesh
                        .try_attribute(ATTRIBUTE_MORPH_TARGET)
                        .ok()
                        .and_then(|attribute| attribute.as_float3())
                    else {
                        return invalid(
                            Some(terrain),
                            "resident presented terrain mesh has no morph position array",
                        );
                    };
                    let Some(VertexAttributeValues::Float32x4(edges)) =
                        mesh.try_attribute(ATTRIBUTE_MORPH_EDGE).ok()
                    else {
                        return invalid(
                            Some(terrain),
                            "resident presented terrain mesh has no typed edge masks",
                        );
                    };
                    let Some(morph_start) = scalar(look, "morph_start") else {
                        return invalid(
                            Some(terrain),
                            "ready presented terrain material has no typed morph start",
                        );
                    };
                    let Some(morph_end) = scalar(look, "morph_end") else {
                        return invalid(
                            Some(terrain),
                            "ready presented terrain material has no typed morph end",
                        );
                    };
                    let Some(ParamValue::Vec4(stitch)) = look.values().get("stitch_edges") else {
                        return invalid(
                            Some(terrain),
                            "ready presented terrain material has no typed stitch state",
                        );
                    };
                    if stitch
                        .iter()
                        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                        || !morph_start.is_finite()
                        || !morph_end.is_finite()
                    {
                        return invalid(
                            Some(terrain),
                            "presented terrain deformation state is invalid",
                        );
                    }
                    let candidates = match candidate_triangles(
                        mesh,
                        coord,
                        resolution,
                        half,
                        local_start,
                        local_end,
                        candidate_radius,
                        &mut remaining,
                    ) {
                        Ok(candidates) => candidates,
                        Err(error) => return invalid(Some(terrain), error),
                    };
                    for [a, b, c] in candidates {
                        let vertices = match triangle_vertices(
                            positions,
                            targets,
                            edges,
                            *stitch,
                            [a, b, c],
                        ) {
                            Ok(vertices) => vertices,
                            Err(reason) => return invalid(Some(terrain), reason),
                        };
                        if !overlaps_bounds(
                            local_start,
                            local_end,
                            vertices.low,
                            vertices.high,
                            local_radius,
                        ) {
                            continue;
                        }
                        let motion = TriangleMotion {
                            matrix,
                            inverse_transpose,
                            camera_start: start.0,
                            camera_end: end.0,
                            local_start,
                            local_end,
                            eye_start,
                            eye_end,
                            morph_start: f64::from(morph_start),
                            morph_end: f64::from(morph_end),
                            radius,
                            clearance,
                            render_error,
                            distance_error: certificate.distance_error_m,
                            tolerance: tolerance_m,
                        };
                        match motion.sweep(
                            vertices.fine,
                            vertices.parent,
                            vertices.edge_weights,
                            &mut remaining,
                            |point| render_to_grid_absolute(grid, RenderPos(point)).0,
                        ) {
                            Ok(MorphSweep::Clear) => {}
                            Ok(MorphSweep::Preparation) => {
                                if preparation_refused {
                                    return DrawTerrainSweep::Refused{terrain,target:end,reason:"camera cover budget refused a stable near-camera morph patch".into()};
                                }
                                return unavailable(
                                    Some(terrain),
                                    format!(
                                        "presented terrain view {view:?} needs a stable near-camera morph patch"
                                    ),
                                );
                            }
                            Ok(MorphSweep::Overlap {
                                depth,
                                normal,
                                full_fine,
                            }) => {
                                if current_lattice && coord.depth >= max_depth && full_fine {
                                    return invalid(
                                        Some(terrain),
                                        format!(
                                            "requested camera clearance penetrates the finest admitted visual surface by at least {depth:.9} m"
                                        ),
                                    );
                                }
                                if preparation_refused {
                                    return DrawTerrainSweep::Refused {
                                        terrain,
                                        target: start,
                                        reason: format!(
                                            "camera cover budget refused required detail at a presented overlap of at least {depth:.9} m"
                                        ),
                                    };
                                }
                                return DrawTerrainSweep::InitialOverlap {
                                    terrain,
                                    depth_m: depth,
                                    normal,
                                };
                            }
                            Ok(MorphSweep::Hit {
                                time,
                                point,
                                normal,
                                full_fine,
                            }) => {
                                let refinement_required =
                                    !current_lattice || coord.depth < max_depth || !full_fine;
                                DrawContact {
                                    terrain,
                                    time,
                                    point: GridPos(point),
                                    normal,
                                    refinement_required,
                                    refinement_refused: refinement_required && preparation_refused,
                                }
                                .record(&mut best);
                            }
                            Err(error) => return invalid(Some(terrain), error),
                        }
                    }
                }
                if !has_draw {
                    return unavailable(
                        Some(terrain),
                        format!("presented terrain view {view:?} root is preparing"),
                    );
                }
            }
        }
        match best {
            Some(hit) if hit.refinement_refused => DrawTerrainSweep::Refused {
                terrain: hit.terrain,
                target: GridPos(start.0 + (end.0 - start.0) * hit.time),
                reason: format!(
                    "camera cover budget refused required detail at presented sweep fraction {:.9}",
                    hit.time
                ),
            },
            Some(DrawContact {
                terrain,
                time,
                point,
                normal,
                refinement_required,
                ..
            }) => DrawTerrainSweep::Hit {
                terrain,
                time,
                point,
                normal,
                refinement_required,
            },
            None => DrawTerrainSweep::Clear,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_solid_sweep(
        top: [DVec3; 3],
        start: DVec3,
        end: DVec3,
        radius: f64,
    ) -> Result<PrismSweep, String> {
        let bottom = solid_bottom(&top, start, end, radius, 0.0001)?;
        let vertices = std::array::from_fn(|index| {
            if index < 3 {
                top[index]
            } else {
                DVec3::new(top[index - 3].x, bottom, top[index - 3].z)
            }
        });
        sweep_prism(vertices, start, end, radius, DVec3::Y, 0.0001)
    }

    #[test]
    fn initial_error_within_existing_tolerance_is_a_blocking_zero_time_contact() {
        let top = [
            DVec3::new(-1.0, 0.0, -1.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(1.0, 0.0, -1.0),
        ];
        let start = DVec3::Y * (0.025 - 0.000001693191);
        let PrismSweep::Hit { time, normal, .. } =
            test_solid_sweep(top, start, start - DVec3::Y * 0.1, 0.025).unwrap()
        else {
            panic!("micron initial error must remain contact, not a terminal overlap");
        };
        assert_eq!(time, 0.0);
        assert!(normal.dot(DVec3::Y) > 0.999);
        let plane = Dir3::new(normal.as_vec3()).unwrap();
        let projected = avian3d::prelude::MoveAndSlide::project_velocity(Vec3::NEG_Y, &[plane]);
        assert!(projected.dot(plane.as_vec3()) >= -f32::EPSILON);
        assert!(
            matches!(test_solid_sweep(top,start,start,0.025).unwrap(),PrismSweep::Hit{time,..} if time==0.0),
            "stationary publication still rejects a tiny negative contact"
        );
        assert!(matches!(
            test_solid_sweep(top, DVec3::Y * 0.015, DVec3::Y * 0.015, 0.025).unwrap(),
            PrismSweep::Overlap { .. }
        ));
    }

    #[test]
    fn direct_prism_topology_remains_outward_under_reflection_and_shear() {
        let top = [
            DVec3::new(-1.0, 0.0, -1.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(1.0, 0.0, -1.0),
        ];
        let matrix = bevy::math::DMat4::from_cols(
            bevy::math::DVec4::new(-2.0, 0.0, 0.0, 0.0),
            bevy::math::DVec4::new(0.4, 3.0, 0.0, 0.0),
            bevy::math::DVec4::new(0.0, 0.0, 0.5, 0.0),
            bevy::math::DVec4::new(1.0, 2.0, -3.0, 1.0),
        );
        let vertices = std::array::from_fn(|index| {
            matrix.transform_point3(if index < 3 {
                top[index]
            } else {
                DVec3::new(top[index - 3].x, -2.0, top[index - 3].z)
            })
        });
        let point = |y| matrix.transform_point3(DVec3::Y * y);
        let up = matrix.transform_vector3(DVec3::Y);
        assert!(matches!(
            sweep_prism(vertices, point(1.0), point(-1.0), 0.025, up, 0.0001).unwrap(),
            PrismSweep::Hit { .. }
        ));
        assert!(matches!(
            sweep_prism(vertices, point(-0.5), point(-0.5), 0.025, up, 0.0001).unwrap(),
            PrismSweep::Overlap { .. }
        ));
        assert!(matches!(
            sweep_prism(vertices, point(1.0), point(2.0), 0.025, up, 0.0001).unwrap(),
            PrismSweep::Clear
        ));
    }

    #[test]
    fn separating_origin_contact_keeps_other_triangles_blocking() {
        let start = DVec3::Y * 0.05;
        let end = start - DVec3::X * 0.5;
        assert!(matches!(
            test_solid_sweep(slope(), start, end, 0.1).unwrap(),
            PrismSweep::Clear
        ));
        assert!(matches!(
            test_solid_sweep(slope(), start, start, 0.1).unwrap(),
            PrismSweep::Overlap { .. }
        ));
        assert!(matches!(
            test_solid_sweep(slope(), start, start + DVec3::X * 0.5, 0.1).unwrap(),
            PrismSweep::Overlap { .. }
        ));
        let wall = [
            DVec3::new(-0.4, 0.2, -1.0),
            DVec3::new(-0.2, 0.2, 0.0),
            DVec3::new(-0.4, 0.2, 1.0),
        ];
        let PrismSweep::Hit { time, .. } = test_solid_sweep(wall, start, end, 0.1).unwrap() else {
            panic!("a later blocking triangle must remain a contact");
        };
        assert!(time > 0.0 && time < 1.0);
    }

    #[test]
    fn stationary_publication_rejects_a_new_overlap_and_unproved_budget() {
        let make = |height: f32, parent_offset: Vec3| {
            let mut mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            );
            let positions = (0..3)
                .flat_map(|z| (0..3).map(move |x| [x as f32 - 1.0, height, z as f32 - 1.0]))
                .collect::<Vec<_>>();
            let targets = positions
                .iter()
                .map(|point| (Vec3::from_array(*point) + parent_offset).to_array())
                .collect::<Vec<_>>();
            let mut min = DVec3::splat(f64::INFINITY);
            let mut max = DVec3::splat(f64::NEG_INFINITY);
            for point in positions.iter().chain(targets.iter()) {
                let p = Vec3::from_array(*point).as_dvec3();
                min = min.min(p);
                max = max.max(p);
            }
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, targets);
            mesh.insert_attribute(ATTRIBUTE_MORPH_EDGE, vec![[1.0_f32; 4]; 9]);
            mesh.insert_indices(Indices::U32(lunco_obstacle_field::field::grid_indices(3)));
            (
                mesh,
                TerrainDrawBounds {
                    min,
                    max,
                    parent_displacement: parent_offset.abs().as_dvec3(),
                },
            )
        };
        let coord = lunco_terrain_core::QuadCoord {
            depth: 2,
            x: 1,
            z: 1,
        };
        let envelope = lunco_camera_core::CameraCollisionEnvelope {
            radius_m: 0.025,
            skin_m: 0.002,
            sweep_tolerance_m: 0.0001,
        };
        let certificate = RenderCoordinateCertificate {
            error_m: 0.0000001,
            translation_bound_m: 0.0,
            distance_error_m: 0.000001,
        };
        let eye = DVec3::Y * 0.026;
        let (old, bounds) = make(0.0, Vec3::ZERO);
        assert!(
            admits_stationary_mesh(
                &old,
                coord,
                3,
                4.0,
                bevy::math::DMat4::IDENTITY,
                eye,
                &envelope,
                1.0,
                2.0,
                [0.0; 4],
                certificate,
                &bounds,
                &mut 64
            )
            .unwrap()
        );
        assert!(
            admits_stationary_mesh(
                &old,
                coord,
                3,
                4.0,
                bevy::math::DMat4::IDENTITY,
                eye,
                &envelope,
                1.0,
                2.0,
                [0.0; 4],
                certificate,
                &bounds,
                &mut 0
            )
            .is_err()
        );
        let (raised, bounds) = make(0.02, Vec3::ZERO);
        assert!(
            !admits_stationary_mesh(
                &raised,
                coord,
                3,
                4.0,
                bevy::math::DMat4::IDENTITY,
                eye,
                &envelope,
                1.0,
                2.0,
                [0.0; 4],
                certificate,
                &bounds,
                &mut 64
            )
            .unwrap()
        );
        let (overhead, bounds) = make(5.0, Vec3::ZERO);
        assert!(
            !admits_stationary_mesh(
                &overhead,
                coord,
                3,
                4.0,
                bevy::math::DMat4::IDENTITY,
                eye,
                &envelope,
                1.0,
                2.0,
                [0.0; 4],
                certificate,
                &bounds,
                &mut 64
            )
            .unwrap(),
            "a far overhead terrain plane still contains the eye in its solid"
        );
        let (shifted, bounds) = make(0.0, Vec3::new(2.0, 0.02, 0.0));
        assert!(
            !admits_stationary_mesh(
                &shifted,
                coord,
                3,
                4.0,
                bevy::math::DMat4::IDENTITY,
                eye + DVec3::X * 2.0,
                &envelope,
                1.0,
                2.0,
                [1.0; 4],
                certificate,
                &bounds,
                &mut 64
            )
            .unwrap(),
            "parent displacement must not hide off-cell geometry"
        );
    }

    #[test]
    fn child_publication_decodes_its_local_lattice_from_the_domain_extent() {
        let coord = lunco_terrain_core::QuadCoord {
            depth: 3,
            x: 3,
            z: 3,
        };
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        let positions = (0..9)
            .flat_map(|z| {
                (0..9).map(move |x| {
                    let px = x as f32 * 0.25 - 1.0;
                    let pz = z as f32 * 0.25 - 1.0;
                    [px, if px >= 0.5 && pz >= 0.5 { 0.02 } else { 0.0 }, pz]
                })
            })
            .collect::<Vec<_>>();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
        mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, positions);
        mesh.insert_attribute(ATTRIBUTE_MORPH_EDGE, vec![[0.0_f32; 4]; 81]);
        mesh.insert_indices(Indices::U32(lunco_obstacle_field::field::grid_indices(9)));
        let bounds = TerrainDrawBounds {
            min: DVec3::new(-1.0, 0.0, -1.0),
            max: DVec3::new(1.0, 0.02, 1.0),
            parent_displacement: DVec3::ZERO,
        };
        let envelope = lunco_camera_core::CameraCollisionEnvelope {
            radius_m: 0.025,
            skin_m: 0.002,
            sweep_tolerance_m: 0.0001,
        };
        let certificate = RenderCoordinateCertificate {
            error_m: 1e-7,
            translation_bound_m: 0.0,
            distance_error_m: 1e-6,
        };
        assert!(
            !admits_stationary_mesh(
                &mesh,
                coord,
                9,
                8.0,
                bevy::math::DMat4::IDENTITY,
                DVec3::new(0.75, 0.026, 0.75),
                &envelope,
                1e20,
                1e21,
                [0.0; 4],
                certificate,
                &bounds,
                &mut 128
            )
            .unwrap(),
            "an off-centre child feature must be checked with the child's posting spacing"
        );
    }

    fn test_motion(start: DVec3, end: DVec3) -> TriangleMotion {
        TriangleMotion {
            matrix: bevy::math::DMat4::IDENTITY,
            inverse_transpose: bevy::math::DMat4::IDENTITY,
            camera_start: start,
            camera_end: end,
            local_start: start,
            local_end: end,
            eye_start: start,
            eye_end: end,
            morph_start: 1.0,
            morph_end: 2.0,
            radius: 0.025,
            clearance: 0.027,
            render_error: 0.0000001,
            distance_error: 0.0000001,
            tolerance: 0.0001,
        }
    }

    #[test]
    fn shader_morph_interval_certifies_clamped_endpoints_and_encloses_interior() {
        assert_eq!(
            shader_morph_interval(0.2, 0.8, 0.01, 1.0, 2.0, 0.0),
            (0.0, 0.0)
        );
        assert_eq!(
            shader_morph_interval(2.1, 3.0, 0.01, 1.0, 2.0, 0.0),
            (1.0, 1.0)
        );
        assert_eq!(
            shader_morph_interval(1.1, 1.2, 0.01, 1.0, 2.0, 0.0),
            (0.0, 1.0)
        );
        assert_eq!(
            shader_morph_interval(0.8, 1.0, 0.01, 1.0, 2.0, 0.0),
            (0.0, 1.0)
        );
        assert_eq!(
            shader_morph_interval(0.2, 0.8, 0.01, 1.0, 2.0, 1.0),
            (1.0, 1.0)
        );
        assert_eq!(
            shader_morph_interval(1.1, 1.2, 0.01, 1.0, 1.0, 0.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn correlated_morph_departure_does_not_block_clear_fine_ground() {
        let fine = [
            DVec3::new(-0.25, 0.0, -0.25),
            DVec3::new(0.0, 0.0, 0.25),
            DVec3::new(0.25, 0.0, -0.25),
        ];
        let parent = fine.map(|point| point + DVec3::Y * 0.2);
        let motion = test_motion(DVec3::Y * 0.1, DVec3::new(4.0, 0.1, 0.0));
        let hull: [DVec3; 6] = std::array::from_fn(|index| {
            if index % 2 == 0 {
                fine[index / 2]
            } else {
                parent[index / 2]
            }
        });
        assert!(
            !hull_separates_sphere(
                &hull,
                motion.local_start,
                motion.local_end,
                motion.clearance,
                motion.inverse_transpose
            ),
            "the uncorrelated whole-path hull intersects near the starting eye"
        );
        assert!(matches!(
            motion
                .sweep(fine, parent, [0.0; 3], &mut 128, |point| point)
                .unwrap(),
            MorphSweep::Clear
        ));
        assert!(
            motion
                .sweep(fine, parent, [0.0; 3], &mut 1, |point| point)
                .is_err(),
            "an incomplete temporal proof must remain unavailable"
        );
        let crossing = test_motion(DVec3::Y * 0.1, DVec3::NEG_Y * 0.1);
        let MorphSweep::Hit {
            time, full_fine, ..
        } = crossing
            .sweep(fine, parent, [0.0; 3], &mut 128, |point| point)
            .unwrap()
        else {
            panic!("a real fine-ground crossing must remain a contact");
        };
        assert!(full_fine && (time - 0.375).abs() < 0.00001);
    }

    #[test]
    fn diagonal_sweep_checks_tube_cells_within_budget_without_losing_contacts() {
        let resolution = 65;
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        let indices = lunco_obstacle_field::field::grid_indices(resolution);
        mesh.insert_indices(Indices::U32(indices.clone()));
        let coord = lunco_terrain_core::QuadCoord {
            depth: 2,
            x: 1,
            z: 1,
        };
        let start = DVec3::new(-2.0, 0.02, -2.0);
        let end = DVec3::new(2.0, 0.02, 2.0);
        let radius = 0.025;
        let candidates =
            candidate_triangles(&mesh, coord, resolution, 1.0, start, end, radius, &mut 1024)
                .unwrap();
        assert!(candidates.len() < 1024);
        assert!(
            candidate_triangles(&mesh, coord, resolution, 1.0, start, end, radius, &mut 32)
                .is_err()
        );
        let selected: std::collections::BTreeSet<_> = candidates.into_iter().collect();
        let reversed: std::collections::BTreeSet<_> =
            candidate_triangles(&mesh, coord, resolution, 1.0, end, start, radius, &mut 1024)
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(selected, reversed);
        let vertex = |index: usize| {
            DVec3::new(
                -1.0 + (index % resolution) as f64 / 32.0,
                0.0,
                -1.0 + (index / resolution) as f64 / 32.0,
            )
        };
        let mut contacts = 0;
        for triangle in indices.chunks_exact(3) {
            let triangle = [
                triangle[0] as usize,
                triangle[1] as usize,
                triangle[2] as usize,
            ];
            if !matches!(
                test_solid_sweep(triangle.map(vertex), start, end, radius).unwrap(),
                PrismSweep::Clear
            ) {
                assert!(
                    selected.contains(&triangle),
                    "the broad phase omitted a real swept-sphere contact"
                );
                contacts += 1;
            }
        }
        assert!(contacts > 0);
    }

    #[test]
    fn earliest_draw_contact_keeps_fine_prefix_and_prioritizes_equal_time_uncertainty() {
        let fine = DrawContact {
            terrain: Entity::PLACEHOLDER,
            time: 0.4,
            point: GridPos(DVec3::ZERO),
            normal: DVec3::Y,
            refinement_required: false,
            refinement_refused: false,
        };
        let coarse = DrawContact {
            time: 0.8,
            refinement_required: true,
            refinement_refused: true,
            ..fine
        };
        for contacts in [[fine, coarse], [coarse, fine]] {
            let mut best = None;
            for hit in contacts {
                hit.record(&mut best);
            }
            let best = best.unwrap();
            assert_eq!(best.time, 0.4);
            assert!(!best.refinement_required && !best.refinement_refused);
        }
        for contacts in [
            [
                fine,
                DrawContact {
                    time: 0.4,
                    ..coarse
                },
            ],
            [
                DrawContact {
                    time: 0.4,
                    ..coarse
                },
                fine,
            ],
        ] {
            let mut best = None;
            for hit in contacts {
                hit.record(&mut best);
            }
            let best = best.unwrap();
            assert!(best.refinement_required && best.refinement_refused);
        }
    }

    #[test]
    fn deforming_slope_hull_keeps_clear_parallel_camera_motion_available() {
        let fine = slope();
        let mut hull = [DVec3::ZERO; 6];
        for (index, point) in fine.into_iter().enumerate() {
            hull[index * 2] = point;
            hull[index * 2 + 1] = point + DVec3::new(0.1, 0.1, 0.0);
        }
        let start = DVec3::new(-1.0, -0.5, 0.0);
        let end = DVec3::new(1.0, 1.5, 0.0);
        assert!(overlaps_bounds(
            start,
            end,
            DVec3::splat(-4.0),
            DVec3::splat(4.1),
            0.1
        ));
        assert!(hull_separates_sphere(
            &hull,
            start,
            end,
            0.1,
            bevy::math::DMat4::IDENTITY
        ));
        assert!(!hull_separates_sphere(
            &hull,
            DVec3::Y * 2.0,
            DVec3::NEG_Y * 2.0,
            0.1,
            bevy::math::DMat4::IDENTITY
        ));
        assert!(!hull_separates_sphere(
            &hull,
            DVec3::ZERO,
            DVec3::new(0.2, 0.2, 0.0),
            0.1,
            bevy::math::DMat4::IDENTITY
        ));
        let reflected = hull.map(|point| DVec3::new(point.x, -point.y, point.z));
        let inverse_transpose = bevy::math::DMat4::from_cols(
            bevy::math::DVec4::new(1.0, 1.0, 0.0, 0.0),
            bevy::math::DVec4::Y,
            bevy::math::DVec4::Z,
            bevy::math::DVec4::W,
        );
        let center = DVec3::new(1.5 / 2.0_f64.sqrt(), 1.5 / 2.0_f64.sqrt(), 0.0);
        assert!(
            !hull_separates_sphere(&reflected, center, center, 1.0, inverse_transpose),
            "a sheared sphere requires support along the full candidate normal"
        );
    }

    fn slope() -> [DVec3; 3] {
        [
            DVec3::new(-4.0, -4.0, -4.0),
            DVec3::new(0.0, 0.0, 4.0),
            DVec3::new(4.0, 4.0, -4.0),
        ]
    }

    #[test]
    fn sphere_sweep_uses_solid_slope_and_volume() {
        let PrismSweep::Hit { time, normal, .. } =
            test_solid_sweep(slope(), DVec3::Y * 2.0, DVec3::NEG_Y * 2.0, 0.1).unwrap()
        else {
            panic!("descending sphere must hit");
        };
        assert!((time - (2.0 - 0.1 * 2.0_f64.sqrt()) / 4.0).abs() < 1e-6);
        assert!(normal.dot(DVec3::new(-1.0, 1.0, 0.0).normalize()) > 0.999);
        assert!(matches!(
            test_solid_sweep(slope(), DVec3::Y * 2.0, DVec3::Y * 3.0, 0.1).unwrap(),
            PrismSweep::Clear
        ));
    }

    #[test]
    fn solid_sweep_rejects_a_deep_below_surface_start_and_crossing() {
        let top = [
            DVec3::new(-1.0, 0.0, -1.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(1.0, 0.0, -1.0),
        ];
        assert!(matches!(
            test_solid_sweep(top, DVec3::NEG_Y * 5.0, DVec3::NEG_Y * 5.0, 0.025).unwrap(),
            PrismSweep::Overlap { .. }
        ));
        assert!(matches!(
            test_solid_sweep(top, DVec3::Y, DVec3::NEG_Y * 5.0, 0.025).unwrap(),
            PrismSweep::Hit { .. }
        ));
        assert!(matches!(
            test_solid_sweep(top, DVec3::Y, DVec3::Y * 2.0, 0.025).unwrap(),
            PrismSweep::Clear
        ));
    }

    #[test]
    fn solid_sweep_reports_initial_overlap() {
        let PrismSweep::Overlap { depth, .. } =
            test_solid_sweep(slope(), DVec3::Y * 0.05, DVec3::ZERO, 0.1).unwrap()
        else {
            panic!("initial overlap must be explicit");
        };
        assert!((depth - (0.1 - 0.05 / 2.0_f64.sqrt())).abs() < 1e-6);
    }

    #[test]
    fn remeshed_boundary_preserves_interior_candidate_lookup() {
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        let mut indices = Vec::new();
        // A northwest domain tile omits its first cell row and column,
        // retaining the regular interior before its perimeter triangles.
        for z in 1..4u32 {
            for x in 1..4u32 {
                let i = z * 5 + x;
                indices.extend_from_slice(&[i, i + 5, i + 1, i + 1, i + 5, i + 6]);
            }
        }
        indices.extend_from_slice(&[0, 5, 6]);
        mesh.insert_indices(Indices::U32(indices));
        let coord = lunco_terrain_core::QuadCoord {
            depth: 1,
            x: 0,
            z: 0,
        };
        let point = DVec3::new(0.1, 0.0, 0.1);
        assert_eq!(
            candidate_triangles(&mesh, coord, 5, 1.0, point, point, 0.01, &mut 3).unwrap(),
            vec![[12, 17, 13], [13, 17, 18]]
        );
        assert!(candidate_triangles(&mesh, coord, 5, 1.0, point, point, 0.01, &mut 2).is_err());
        let boundary = DVec3::new(-0.9, 0.0, -0.9);
        assert!(
            candidate_triangles(&mesh, coord, 5, 1.0, boundary, boundary, 0.01, &mut 20)
                .unwrap()
                .contains(&[0, 5, 6])
        );
    }
}
