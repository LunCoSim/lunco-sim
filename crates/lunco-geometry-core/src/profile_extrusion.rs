//! Convex polygon extrusion with explicit right-handed, outward face topology.

use bevy_math::{DVec2, DVec3};

/// A supported 2D profile plane in LunCoSim's right-handed Y-up frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfilePlane {
    /// Profile coordinates are X/Y; extrusion is along +Z.
    XY,
    /// Profile coordinates are X/(-Z); extrusion is along +Y.
    XZ,
    /// Profile coordinates are Y/Z; extrusion is along +X.
    YZ,
}

/// A closed, indexed polygon extrusion in native f64 coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileMeshData {
    points: Vec<DVec3>,
    face_vertex_counts: Vec<i32>,
    face_vertex_indices: Vec<i32>,
    face_varying_normals: Vec<DVec3>,
}

impl ProfileMeshData {
    /// Mesh vertices in the caller's local component frame.
    pub fn points(&self) -> &[DVec3] {
        &self.points
    }

    /// Number of vertices in each face, in authored face order.
    pub fn face_vertex_counts(&self) -> &[i32] {
        &self.face_vertex_counts
    }

    /// Indexed face topology with counter-clockwise outward winding.
    pub fn face_vertex_indices(&self) -> &[i32] {
        &self.face_vertex_indices
    }

    /// One outward unit normal for every face-vertex corner.
    pub fn face_varying_normals(&self) -> &[DVec3] {
        &self.face_varying_normals
    }

    /// Number of unique profile vertices on either end cap.
    pub fn vertex_count(&self) -> usize {
        self.points.len() / 2
    }

    /// Number of polygon faces including end caps.
    pub fn face_count(&self) -> usize {
        self.face_vertex_counts.len()
    }
}

/// Why an authored profile cannot produce a closed manifold prism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileExtrusionError {
    /// A simple polygon needs at least three points.
    TooFewPoints,
    /// Profile coordinates or extrusion length are not finite.
    NonFinite,
    /// Adjacent profile points do not define an edge.
    RepeatedAdjacentPoint,
    /// The perimeter is collinear or has negligible area.
    DegenerateArea,
    /// The profile crosses itself or changes turn direction.
    NotStrictlyConvex,
    /// The requested extrusion length is not positive.
    NonPositiveLength,
}

impl std::fmt::Display for ProfileExtrusionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::TooFewPoints => "profile requires at least three points",
            Self::NonFinite => "profile coordinates and extrusion length must be finite",
            Self::RepeatedAdjacentPoint => "profile has a repeated adjacent point",
            Self::DegenerateArea => "profile area is degenerate",
            Self::NotStrictlyConvex => "profile must be a simple, strictly convex polygon",
            Self::NonPositiveLength => "extrusion length must be positive",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ProfileExtrusionError {}

/// Build a closed prism from a simple, strictly convex 2D profile.
///
/// The profile may be supplied clockwise or counter-clockwise; the output is
/// normalized to one right-handed convention. Dimensions remain `f64`; any
/// eventual USD schema lowering belongs to the consuming authoring adapter.
pub fn extrude_profile(
    profile: &[DVec2],
    length: f64,
    plane: ProfilePlane,
) -> Result<ProfileMeshData, ProfileExtrusionError> {
    if profile.len() < 3 {
        return Err(ProfileExtrusionError::TooFewPoints);
    }
    if !length.is_finite() || profile.iter().any(|point| !point.is_finite()) {
        return Err(ProfileExtrusionError::NonFinite);
    }
    if length <= 0.0 {
        return Err(ProfileExtrusionError::NonPositiveLength);
    }

    let mut polygon = profile.to_vec();
    let extent = polygon
        .iter()
        .fold(DVec2::ZERO, |acc, point| acc.max(point.abs()))
        .max_element();
    if !extent.is_finite() || extent == 0.0 {
        return Err(ProfileExtrusionError::DegenerateArea);
    }
    let edge_epsilon_squared = extent * extent * 1.0e-24;
    for index in 0..polygon.len() {
        let next = (index + 1) % polygon.len();
        if polygon[index].distance_squared(polygon[next]) <= edge_epsilon_squared {
            return Err(ProfileExtrusionError::RepeatedAdjacentPoint);
        }
    }

    let area_twice = signed_area_twice(&polygon);
    let area_epsilon = extent * extent * 1.0e-12;
    if !area_twice.is_finite() || area_twice.abs() <= area_epsilon {
        return Err(ProfileExtrusionError::DegenerateArea);
    }
    if area_twice < 0.0 {
        polygon.reverse();
    }
    ensure_strictly_convex(&polygon, area_epsilon)?;

    let vertex_count = polygon.len();
    let vertex_count_i32 =
        i32::try_from(vertex_count).map_err(|_| ProfileExtrusionError::TooFewPoints)?;
    let mut points = Vec::with_capacity(vertex_count * 2);
    let mut face_vertex_counts = Vec::with_capacity(vertex_count + 2);
    let mut face_vertex_indices = Vec::with_capacity((vertex_count + 2) * vertex_count);
    let mut face_varying_normals = Vec::with_capacity((vertex_count + 2) * vertex_count);
    let half_length = length * 0.5;
    let normal = plane_normal(plane);

    for distance in [-half_length, half_length] {
        for point in &polygon {
            points.push(map_profile(plane, *point, distance));
        }
    }

    face_vertex_counts.push(vertex_count_i32);
    for index in (0..vertex_count).rev() {
        face_vertex_indices
            .push(i32::try_from(index).map_err(|_| ProfileExtrusionError::TooFewPoints)?);
    }
    face_varying_normals.extend(std::iter::repeat_n(-normal, vertex_count));

    face_vertex_counts.push(vertex_count_i32);
    for index in 0..vertex_count {
        face_vertex_indices.push(
            i32::try_from(vertex_count + index).map_err(|_| ProfileExtrusionError::TooFewPoints)?,
        );
    }
    face_varying_normals.extend(std::iter::repeat_n(normal, vertex_count));

    for index in 0..vertex_count {
        let next = (index + 1) % vertex_count;
        face_vertex_counts.push(4);
        face_vertex_indices.extend_from_slice(&[
            i32::try_from(index).map_err(|_| ProfileExtrusionError::TooFewPoints)?,
            i32::try_from(next).map_err(|_| ProfileExtrusionError::TooFewPoints)?,
            i32::try_from(vertex_count + next).map_err(|_| ProfileExtrusionError::TooFewPoints)?,
            i32::try_from(vertex_count + index).map_err(|_| ProfileExtrusionError::TooFewPoints)?,
        ]);
        let edge = points[next] - points[index];
        let side_normal = edge.cross(normal).normalize_or_zero();
        if side_normal == DVec3::ZERO || !side_normal.is_finite() {
            return Err(ProfileExtrusionError::DegenerateArea);
        }
        face_varying_normals.extend(std::iter::repeat_n(side_normal, 4));
    }

    Ok(ProfileMeshData {
        points,
        face_vertex_counts,
        face_vertex_indices,
        face_varying_normals,
    })
}

fn signed_area_twice(polygon: &[DVec2]) -> f64 {
    let origin = polygon[0];
    (1..polygon.len() - 1)
        .map(|index| (polygon[index] - origin).perp_dot(polygon[index + 1] - origin))
        .sum()
}

fn ensure_strictly_convex(polygon: &[DVec2], epsilon: f64) -> Result<(), ProfileExtrusionError> {
    for index in 0..polygon.len() {
        let a = polygon[index];
        let b = polygon[(index + 1) % polygon.len()];
        let c = polygon[(index + 2) % polygon.len()];
        if (b - a).perp_dot(c - b) <= epsilon {
            return Err(ProfileExtrusionError::NotStrictlyConvex);
        }
    }

    for first in 0..polygon.len() {
        let first_next = (first + 1) % polygon.len();
        for second in (first + 1)..polygon.len() {
            let second_next = (second + 1) % polygon.len();
            if first == second || first_next == second || second_next == first {
                continue;
            }
            if segments_intersect(
                polygon[first],
                polygon[first_next],
                polygon[second],
                polygon[second_next],
                epsilon,
            ) {
                return Err(ProfileExtrusionError::NotStrictlyConvex);
            }
        }
    }
    Ok(())
}

fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2, epsilon: f64) -> bool {
    let ab_c = (b - a).perp_dot(c - a);
    let ab_d = (b - a).perp_dot(d - a);
    let cd_a = (d - c).perp_dot(a - c);
    let cd_b = (d - c).perp_dot(b - c);
    ab_c * ab_d < -epsilon * epsilon && cd_a * cd_b < -epsilon * epsilon
}

fn plane_normal(plane: ProfilePlane) -> DVec3 {
    match plane {
        ProfilePlane::XY => DVec3::Z,
        ProfilePlane::XZ => DVec3::Y,
        ProfilePlane::YZ => DVec3::X,
    }
}

fn map_profile(plane: ProfilePlane, point: DVec2, distance: f64) -> DVec3 {
    match plane {
        ProfilePlane::XY => DVec3::new(point.x, point.y, distance),
        // X and -Z form a right-handed basis whose normal is +Y.
        ProfilePlane::XZ => DVec3::new(point.x, distance, -point.y),
        ProfilePlane::YZ => DVec3::new(distance, point.x, point.y),
    }
}
