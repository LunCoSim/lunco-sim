//! Closed tapered rectangular beams between two typed 3D datums.
//!
//! This is intentionally a small CAD-like primitive rather than a Griffin
//! special case.  A caller supplies the two centreline datums, the transverse
//! width axis, and the two end widths.  The kernel owns topology, winding, and
//! unit normals so authored scripts never have to hand-build mesh arrays.

use bevy_math::DVec3;

/// A closed, indexed tapered rectangular beam.
#[derive(Clone, Debug, PartialEq)]
pub struct TaperedBeamMeshData {
    points: Vec<DVec3>,
    face_vertex_counts: Vec<i32>,
    face_vertex_indices: Vec<i32>,
    face_varying_normals: Vec<DVec3>,
}

impl TaperedBeamMeshData {
    /// Mesh vertices in the caller's local frame.
    pub fn points(&self) -> &[DVec3] {
        &self.points
    }

    /// Number of vertices in each face.
    pub fn face_vertex_counts(&self) -> &[i32] {
        &self.face_vertex_counts
    }

    /// Right-handed, outward-wound face indices.
    pub fn face_vertex_indices(&self) -> &[i32] {
        &self.face_vertex_indices
    }

    /// One outward unit normal for every face corner.
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

/// Why a typed tapered beam cannot be generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaperedBeamError {
    NonFinite,
    CoincidentEndpoints,
    InvalidWidthAxis,
    ParallelWidthAxis,
    NonPositiveWidth,
    NonPositiveThickness,
}

impl std::fmt::Display for TaperedBeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::NonFinite => "beam datums and dimensions must be finite",
            Self::CoincidentEndpoints => "beam endpoints must not coincide",
            Self::InvalidWidthAxis => "beam width axis must have nonzero length",
            Self::ParallelWidthAxis => "beam width axis must not be parallel to its centreline",
            Self::NonPositiveWidth => "beam end half-widths must be positive",
            Self::NonPositiveThickness => "beam thickness must be positive",
        };
        f.write_str(message)
    }
}

impl std::error::Error for TaperedBeamError {}

/// Build a closed rectangular beam whose width may taper from one end to the
/// other.  `width_axis` is normalized internally; thickness is measured along
/// the plane normal derived from the centreline and width axis.
pub fn tapered_beam(
    start: DVec3,
    end: DVec3,
    width_axis: DVec3,
    start_half_width: f64,
    end_half_width: f64,
    thickness: f64,
) -> Result<TaperedBeamMeshData, TaperedBeamError> {
    if !start.is_finite()
        || !end.is_finite()
        || !width_axis.is_finite()
        || !start_half_width.is_finite()
        || !end_half_width.is_finite()
        || !thickness.is_finite()
    {
        return Err(TaperedBeamError::NonFinite);
    }
    let centreline = end - start;
    if centreline.length_squared() <= f64::EPSILON * f64::EPSILON {
        return Err(TaperedBeamError::CoincidentEndpoints);
    }
    if width_axis.length_squared() <= f64::EPSILON * f64::EPSILON {
        return Err(TaperedBeamError::InvalidWidthAxis);
    }
    if start_half_width <= 0.0 || end_half_width <= 0.0 {
        return Err(TaperedBeamError::NonPositiveWidth);
    }
    if thickness <= 0.0 {
        return Err(TaperedBeamError::NonPositiveThickness);
    }

    let width = width_axis.normalize();
    let depth = centreline.cross(width);
    if depth.length_squared() <= f64::EPSILON * f64::EPSILON {
        return Err(TaperedBeamError::ParallelWidthAxis);
    }
    let depth = depth.normalize();
    let half_thickness = thickness * 0.5;

    // Local basis is centreline, width, depth: centreline × width = depth.
    // The loops below are ordered so every face has outward right-handed
    // winding, independent of the direction of the supplied datums.
    let start_points = [
        start - depth * half_thickness - width * start_half_width,
        start - depth * half_thickness + width * start_half_width,
        start + depth * half_thickness + width * start_half_width,
        start + depth * half_thickness - width * start_half_width,
    ];
    let end_points = [
        end - depth * half_thickness - width * end_half_width,
        end - depth * half_thickness + width * end_half_width,
        end + depth * half_thickness + width * end_half_width,
        end + depth * half_thickness - width * end_half_width,
    ];
    let mut points = Vec::with_capacity(8);
    points.extend(start_points);
    points.extend(end_points);

    let faces = [
        [0_usize, 3, 2, 1], // start cap
        [4, 5, 6, 7],       // end cap
        [0, 4, 7, 3],       // -width
        [1, 2, 6, 5],       // +width
        [0, 1, 5, 4],       // -depth
        [3, 7, 6, 2],       // +depth
    ];
    let mut face_vertex_indices = Vec::with_capacity(faces.len() * 4);
    let mut face_varying_normals = Vec::with_capacity(faces.len() * 4);
    for face in faces {
        let a = points[face[0]];
        let b = points[face[1]];
        let c = points[face[2]];
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal == DVec3::ZERO || !normal.is_finite() {
            return Err(TaperedBeamError::NonFinite);
        }
        face_vertex_indices.extend(face.map(|index| index as i32));
        face_varying_normals.extend([normal; 4]);
    }

    Ok(TaperedBeamMeshData {
        points,
        face_vertex_counts: vec![4; faces.len()],
        face_vertex_indices,
        face_varying_normals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_outward_unit_normals_for_a_tapered_beam() {
        let mesh = tapered_beam(
            DVec3::ZERO,
            DVec3::new(0.0, 2.0, 0.0),
            DVec3::Z,
            0.3,
            0.2,
            0.1,
        )
        .expect("valid beam");
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.face_count(), 6);
        assert!(
            mesh.face_varying_normals()
                .iter()
                .all(|normal| (normal.length() - 1.0).abs() < 1.0e-12)
        );

        let mut signed_volume = 0.0;
        for face in mesh.face_vertex_indices().chunks_exact(4) {
            let a = mesh.points()[face[0] as usize];
            let b = mesh.points()[face[1] as usize];
            let c = mesh.points()[face[2] as usize];
            let d = mesh.points()[face[3] as usize];
            signed_volume += a.dot(b.cross(c)) / 6.0;
            signed_volume += a.dot(c.cross(d)) / 6.0;
        }
        assert!(signed_volume > 0.0);
    }
}
