//! Closed solids made by revolving a typed 2D `(radius, height)` profile.
//!
//! Profiles and generated vertices use native `f64` coordinates. The result
//! is ordinary indexed mesh data so authoring adapters can lower it to
//! `UsdGeomMesh` without adding a second parametric schema or storing design
//! values in USD strings.

use bevy_math::{DVec2, DVec3};

/// A closed, indexed surface of revolution around local +Y.
#[derive(Clone, Debug, PartialEq)]
pub struct RevolvedProfileMeshData {
    points: Vec<DVec3>,
    face_vertex_counts: Vec<i32>,
    face_vertex_indices: Vec<i32>,
    face_varying_normals: Vec<DVec3>,
}

impl RevolvedProfileMeshData {
    /// Mesh vertices in the profile's local component frame.
    pub fn points(&self) -> &[DVec3] {
        &self.points
    }

    /// Number of vertices in each face, in authored face order.
    pub fn face_vertex_counts(&self) -> &[i32] {
        &self.face_vertex_counts
    }

    /// Indexed faces with outward, right-handed winding.
    pub fn face_vertex_indices(&self) -> &[i32] {
        &self.face_vertex_indices
    }

    /// One outward flat normal for every face-vertex corner.
    pub fn face_varying_normals(&self) -> &[DVec3] {
        &self.face_varying_normals
    }

    pub fn vertex_count(&self) -> usize {
        self.points.len()
    }

    pub fn face_count(&self) -> usize {
        self.face_vertex_counts.len()
    }
}

/// Why a meridian profile cannot produce a closed revolved mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileRevolutionError {
    TooFewPoints,
    TooManyPoints,
    TooFewSegments,
    TooManySegments,
    NonFinite,
    NegativeRadius,
    RepeatedAdjacentPoint,
    DegenerateArea,
    SelfIntersecting,
    MeshTooLarge,
    DegenerateFace,
}

impl std::fmt::Display for ProfileRevolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::TooFewPoints => "profile requires at least three points",
            Self::TooManyPoints => "profile exceeds the 4096-point limit",
            Self::TooFewSegments => "revolution requires at least eight angular segments",
            Self::TooManySegments => "revolution exceeds the 512-segment limit",
            Self::NonFinite => "profile coordinates must be finite",
            Self::NegativeRadius => "profile radius must be non-negative",
            Self::RepeatedAdjacentPoint => "profile has a repeated adjacent point",
            Self::DegenerateArea => "profile encloses negligible area",
            Self::SelfIntersecting => "profile must be a simple closed polygon",
            Self::MeshTooLarge => "revolved mesh exceeds the supported vertex limit",
            Self::DegenerateFace => "profile generated a degenerate surface face",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ProfileRevolutionError {}

/// Revolve a simple closed `(radius, height)` polygon around local +Y.
///
/// The polygon is implicitly closed, may be authored in either winding, and
/// may touch the axis at one or more profile vertices. Axis edges collapse to
/// fans, so a hemispherical profile can include a real flat circular contact
/// face and a pole without zero-area rings. A minimum of eight angular
/// segments prevents accidental low-resolution visual primitives.
pub fn revolve_profile(
    profile: &[DVec2],
    angular_segments: u16,
) -> Result<RevolvedProfileMeshData, ProfileRevolutionError> {
    const MAX_PROFILE_POINTS: usize = 4096;
    const MAX_ANGULAR_SEGMENTS: u16 = 512;
    const MAX_VERTICES: usize = 250_000;

    if profile.len() < 3 {
        return Err(ProfileRevolutionError::TooFewPoints);
    }
    if profile.len() > MAX_PROFILE_POINTS {
        return Err(ProfileRevolutionError::TooManyPoints);
    }
    if angular_segments < 8 {
        return Err(ProfileRevolutionError::TooFewSegments);
    }
    if angular_segments > MAX_ANGULAR_SEGMENTS {
        return Err(ProfileRevolutionError::TooManySegments);
    }
    if profile.iter().any(|point| !point.is_finite()) {
        return Err(ProfileRevolutionError::NonFinite);
    }
    if profile.iter().any(|point| point.x < 0.0) {
        return Err(ProfileRevolutionError::NegativeRadius);
    }

    let mut meridian = profile.to_vec();
    let extent = meridian
        .iter()
        .fold(DVec2::ZERO, |acc, point| acc.max(point.abs()))
        .max_element();
    if !extent.is_finite() || extent == 0.0 {
        return Err(ProfileRevolutionError::DegenerateArea);
    }
    let epsilon = extent * 1.0e-12;
    let epsilon_squared = epsilon * epsilon;
    for index in 0..meridian.len() {
        let next = (index + 1) % meridian.len();
        if meridian[index].distance_squared(meridian[next]) <= epsilon_squared {
            return Err(ProfileRevolutionError::RepeatedAdjacentPoint);
        }
    }

    let area_twice = signed_area_twice(&meridian);
    if !area_twice.is_finite() || area_twice.abs() <= extent * extent * 1.0e-12 {
        return Err(ProfileRevolutionError::DegenerateArea);
    }
    if area_twice < 0.0 {
        meridian.reverse();
    }
    ensure_simple(&meridian, extent * extent * 1.0e-12)?;

    let segments = usize::from(angular_segments);
    let ring_count = meridian.iter().filter(|point| point.x > 0.0).count();
    let axis_count = meridian.len() - ring_count;
    let vertex_count = ring_count
        .checked_mul(segments)
        .and_then(|count| count.checked_add(axis_count))
        .filter(|count| *count <= MAX_VERTICES && *count <= i32::MAX as usize)
        .ok_or(ProfileRevolutionError::MeshTooLarge)?;

    let mut points = Vec::with_capacity(vertex_count);
    let mut vertex_starts = Vec::with_capacity(meridian.len());
    let tau = std::f64::consts::TAU;
    for point in &meridian {
        let start = points.len();
        if point.x == 0.0 {
            points.push(DVec3::new(0.0, point.y, 0.0));
        } else {
            for segment in 0..segments {
                let angle = tau * segment as f64 / segments as f64;
                points.push(DVec3::new(
                    point.x * angle.cos(),
                    point.y,
                    point.x * angle.sin(),
                ));
            }
        }
        vertex_starts.push(start);
    }

    let mut face_vertex_counts = Vec::with_capacity(meridian.len() * segments);
    let mut face_vertex_indices = Vec::with_capacity(meridian.len() * segments * 4);
    let mut face_varying_normals = Vec::with_capacity(meridian.len() * segments * 4);
    for edge in 0..meridian.len() {
        let next_edge = (edge + 1) % meridian.len();
        let start_is_axis = meridian[edge].x == 0.0;
        let end_is_axis = meridian[next_edge].x == 0.0;
        if start_is_axis && end_is_axis {
            continue;
        }

        for segment in 0..segments {
            let next_segment = (segment + 1) % segments;
            let start = vertex_starts[edge];
            let end = vertex_starts[next_edge];
            let face: Vec<usize> = match (start_is_axis, end_is_axis) {
                (true, false) => vec![start, end + segment, end + next_segment],
                (false, true) => vec![start + segment, end, start + next_segment],
                (false, false) => vec![
                    start + segment,
                    end + segment,
                    end + next_segment,
                    start + next_segment,
                ],
                (true, true) => unreachable!(),
            };
            let a = points[face[0]];
            let b = points[face[1]];
            let c = points[face[2]];
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if normal == DVec3::ZERO || !normal.is_finite() {
                return Err(ProfileRevolutionError::DegenerateFace);
            }
            face_vertex_counts
                .push(i32::try_from(face.len()).map_err(|_| ProfileRevolutionError::MeshTooLarge)?);
            for index in face {
                face_vertex_indices
                    .push(i32::try_from(index).map_err(|_| ProfileRevolutionError::MeshTooLarge)?);
                face_varying_normals.push(normal);
            }
        }
    }

    if face_vertex_counts.is_empty() {
        return Err(ProfileRevolutionError::DegenerateArea);
    }
    Ok(RevolvedProfileMeshData {
        points,
        face_vertex_counts,
        face_vertex_indices,
        face_varying_normals,
    })
}

fn signed_area_twice(profile: &[DVec2]) -> f64 {
    let origin = profile[0];
    (1..profile.len() - 1)
        .map(|index| (profile[index] - origin).perp_dot(profile[index + 1] - origin))
        .sum()
}

fn ensure_simple(profile: &[DVec2], epsilon: f64) -> Result<(), ProfileRevolutionError> {
    for first in 0..profile.len() {
        let first_next = (first + 1) % profile.len();
        for second in first + 1..profile.len() {
            let second_next = (second + 1) % profile.len();
            if first == second || first_next == second || second_next == first {
                continue;
            }
            if segments_intersect(
                profile[first],
                profile[first_next],
                profile[second],
                profile[second_next],
                epsilon,
            ) {
                return Err(ProfileRevolutionError::SelfIntersecting);
            }
        }
    }
    Ok(())
}

fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2, epsilon: f64) -> bool {
    fn orientation(a: DVec2, b: DVec2, c: DVec2) -> f64 {
        (b - a).perp_dot(c - a)
    }
    fn on_segment(a: DVec2, b: DVec2, point: DVec2, epsilon: f64) -> bool {
        point.x >= a.x.min(b.x) - epsilon
            && point.x <= a.x.max(b.x) + epsilon
            && point.y >= a.y.min(b.y) - epsilon
            && point.y <= a.y.max(b.y) + epsilon
    }

    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    if abc.abs() <= epsilon && on_segment(a, b, c, epsilon) {
        return true;
    }
    if abd.abs() <= epsilon && on_segment(a, b, d, epsilon) {
        return true;
    }
    if cda.abs() <= epsilon && on_segment(c, d, a, epsilon) {
        return true;
    }
    if cdb.abs() <= epsilon && on_segment(c, d, b, epsilon) {
        return true;
    }
    (abc > 0.0) != (abd > 0.0) && (cda > 0.0) != (cdb > 0.0)
}
