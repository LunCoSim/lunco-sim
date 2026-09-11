//! Surface-aligned ribbon geometry shared by transient editor annotations.

use bevy::math::DVec3;
use bevy::prelude::Mesh;

/// One point in a surface-aligned ribbon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RibbonPoint {
    pub position: DVec3,
    pub normal: DVec3,
}

/// Build a surface-separated ribbon with vertices relative to `anchor`.
pub(crate) fn build_ribbon_mesh(
    points: &[RibbonPoint],
    anchor: DVec3,
    half_width: f32,
    surface_clearance: f32,
) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    if points.len() < 2 {
        return None;
    }

    let mut positions = Vec::with_capacity(points.len() * 2);
    let mut normals = Vec::with_capacity(points.len() * 2);
    let mut uvs = Vec::with_capacity(points.len() * 2);
    let mut previous_right = None;
    for index in 0..points.len() {
        let tangent = ribbon_tangent(points, index);
        let right = ribbon_right(tangent, points[index].normal, previous_right);
        previous_right = Some(right);
        let normal = points[index].normal.as_vec3();
        let base = (points[index].position - anchor).as_vec3() + normal * surface_clearance;
        let right = (right * half_width as f64).as_vec3();
        positions.push((base - right).to_array());
        positions.push((base + right).to_array());
        normals.push(normal.to_array());
        normals.push(normal.to_array());
        let v = index as f32;
        uvs.push([0.0, v]);
        uvs.push([1.0, v]);
    }

    let mut indices = Vec::with_capacity((points.len() - 1) * 6);
    for index in 0..points.len() - 1 {
        let start = (index * 2) as u32;
        indices.extend_from_slice(&[start, start + 1, start + 2, start + 2, start + 1, start + 3]);
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

fn ribbon_tangent(points: &[RibbonPoint], index: usize) -> DVec3 {
    let current = points[index].position;
    let normal = points[index].normal;
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

fn ribbon_right(tangent: DVec3, normal: DVec3, previous: Option<DVec3>) -> DVec3 {
    let mut right = tangent.cross(normal);
    if right.length_squared() < 1.0e-9 {
        right = normal.cross(DVec3::Z);
    }
    let mut right = right.normalize();
    if previous.is_some_and(|old| right.dot(old) < 0.0) {
        right = -right;
    }
    right
}
