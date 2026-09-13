//! Surface-aligned ribbon mesh data shared by transient and USD annotations.
//!
//! A ribbon is the oriented form of a USD curve: `widths` is normalized to
//! half-widths before reaching this builder and `normals` supplies the surface
//! orientation. The builder is independent of routes, vehicles, and UI, so
//! both authored USD curves and transient contact trails use the same
//! turn-safe geometry.

use bevy_asset::RenderAssetUsages;
use bevy_math::DVec3;
use bevy_mesh::{Indices, Mesh, PrimitiveTopology};

/// One point in a surface-aligned ribbon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonPoint {
    pub position: DVec3,
    pub normal: DVec3,
}

/// Build a surface-separated ribbon with vertices relative to `anchor`.
///
/// `half_widths` accepts one constant half-width or one value per point. A
/// different length is rejected rather than silently applying the wrong
/// interpolation to authored geometry.
pub fn build_ribbon_mesh(
    points: &[RibbonPoint],
    anchor: DVec3,
    half_widths: &[f32],
    surface_clearance: f32,
    closed: bool,
) -> Option<Mesh> {
    if points.len() < 2 || half_widths.is_empty() {
        return None;
    }
    if half_widths.len() != 1 && half_widths.len() != points.len() {
        return None;
    }
    if !surface_clearance.is_finite() || half_widths.iter().any(|w| !w.is_finite() || *w <= 0.0) {
        return None;
    }

    let mut positions = Vec::with_capacity(points.len() * 2);
    let mut normals = Vec::with_capacity(points.len() * 2);
    let mut uvs = Vec::with_capacity(points.len() * 2);
    let mut previous_right = None;
    for index in 0..points.len() {
        let normal = points[index].normal.normalize_or_zero();
        if normal == DVec3::ZERO {
            return None;
        }
        let tangent = ribbon_tangent(points, index, normal);
        if tangent == DVec3::ZERO {
            return None;
        }
        let right = ribbon_right(tangent, normal, previous_right)?;
        previous_right = Some(right);
        let width = half_widths[index.min(half_widths.len() - 1)];
        let base = points[index].position - anchor + normal * surface_clearance as f64;
        positions.push((base - right * width as f64).as_vec3().to_array());
        positions.push((base + right * width as f64).as_vec3().to_array());
        normals.push(normal.as_vec3().to_array());
        normals.push(normal.as_vec3().to_array());
        let v = index as f32;
        uvs.push([0.0, v]);
        uvs.push([1.0, v]);
    }

    let segment_count = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    let mut indices = Vec::with_capacity(segment_count * 6);
    for index in 0..segment_count {
        let start = (index * 2) as u32;
        let next = (((index + 1) % points.len()) * 2) as u32;
        indices.extend_from_slice(&[start, start + 1, next, next, start + 1, next + 1]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

fn ribbon_tangent(points: &[RibbonPoint], index: usize, normal: DVec3) -> DVec3 {
    let current = points[index].position;
    let previous = points[index.saturating_sub(1)].position;
    let next = points[(index + 1).min(points.len() - 1)].position;

    let mut tangent = next - previous;
    tangent -= normal * tangent.dot(normal);
    if tangent.length_squared() < 1.0e-9 {
        tangent = current - previous;
        tangent -= normal * tangent.dot(normal);
    }
    if tangent.length_squared() < 1.0e-9 {
        tangent = next - current;
        tangent -= normal * tangent.dot(normal);
    }
    if tangent.length_squared() < 1.0e-9 {
        let reference = if normal.dot(DVec3::Z).abs() < 0.9 {
            DVec3::Z
        } else {
            DVec3::X
        };
        normal.cross(reference).normalize_or_zero()
    } else {
        tangent.normalize()
    }
}

fn ribbon_right(tangent: DVec3, normal: DVec3, previous: Option<DVec3>) -> Option<DVec3> {
    let mut right = tangent.cross(normal);
    if right.length_squared() < 1.0e-9 {
        right = normal.cross(DVec3::Z);
    }
    let mut right = right.normalize_or_zero();
    if right == DVec3::ZERO {
        return None;
    }
    if previous.is_some_and(|old| right.dot(old) < 0.0) {
        right = -right;
    }
    Some(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_mesh::VertexAttributeValues;

    #[test]
    fn flat_ribbon_uses_full_strip_topology_and_surface_offset() {
        let points = [
            RibbonPoint {
                position: DVec3::ZERO,
                normal: DVec3::Y,
            },
            RibbonPoint {
                position: DVec3::new(2.0, 0.5, 0.0),
                normal: DVec3::Y,
            },
        ];
        let mesh = build_ribbon_mesh(&points, DVec3::ZERO, &[0.2], 0.1, false).expect("ribbon");
        assert_eq!(mesh.count_vertices(), 4);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("ribbon must be indexed")
        };
        assert_eq!(indices.len(), 6);
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("ribbon positions")
        };
        assert!((positions[0][1] - 0.1).abs() < 1.0e-6);
        assert!((positions[0][2] + 0.2).abs() < 1.0e-6);
    }

    #[test]
    fn degenerate_authored_normals_are_rejected() {
        let points = [
            RibbonPoint {
                position: DVec3::ZERO,
                normal: DVec3::ZERO,
            },
            RibbonPoint {
                position: DVec3::Z,
                normal: DVec3::Y,
            },
        ];
        assert!(build_ribbon_mesh(&points, DVec3::ZERO, &[0.2], 0.1, false).is_none());
    }
}
