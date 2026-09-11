use bevy::prelude::{error, Quat, Vec3};
use lunco_usd_bevy_core::read::UsdReadObject;
use lunco_usd_bevy_core::stage_convention;
use openusd::sdf::Path as SdfPath;
use openusd::sdf::Value;

/// Canonical dimensions of a USD primitive shape, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeDims {
    Cube { size: f64 },
    Sphere { radius: f64 },
    Cylinder { radius: f64, height: f64 },
    Cone { radius: f64, height: f64 },
    Capsule { radius: f64, height: f64 },
    Plane { width: f64, length: f64 },
}

/// Canonical topology of a native USD mesh.
///
/// Points have already been converted to the engine's canonical up-axis and
/// metres. Face counts and indices remain in authored topology order so a
/// renderer can preserve per-corner attributes while physics can use the
/// indexed view below.
#[derive(Debug, Clone, PartialEq)]
pub struct UsdMeshTopology {
    pub points: Vec<[f32; 3]>,
    pub face_vertex_counts: Vec<i32>,
    pub face_vertex_indices: Vec<i32>,
}

/// Canonical `UsdGeom` axis token to quaternion for Y-axial Bevy primitives.
/// Returns `None` for the already-aligned Y axis and unsupported tokens.
pub fn usd_axis_to_quat(axis: &str) -> Option<Quat> {
    match axis {
        "X" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
        "Z" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
        _ => None,
    }
}

/// Read the standard `UsdGeom` axis token for a primitive.
pub fn read_primitive_axis(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<String> {
    if !matches!(type_name, "Cylinder" | "Cone" | "Capsule" | "Plane") {
        return Some("Z".to_owned());
    }
    match reader.text(path, "axis") {
        Some(axis) if matches!(axis.as_str(), "X" | "Y" | "Z") => Some(axis),
        Some(axis) => {
            error!(
                "[usd-scene] {} has invalid {} axis token `{axis}`; expected X, Y, or Z",
                path.as_str(),
                type_name
            );
            None
        }
        None if reader.has_authored_attribute(path, "axis")
            || !reader.connections(path, "axis").is_empty() =>
        {
            error!(
                "[usd-scene] {} has an authored {} axis with an unsupported value type",
                path.as_str(),
                type_name
            );
            None
        }
        None => Some("Z".to_owned()),
    }
}

/// Read a primitive's USD dimensions and convert them to canonical metres.
pub fn read_shape_dims(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<ShapeDims> {
    let dims = read_shape_dims_raw(reader, path, type_name)?;
    let convention = stage_convention(reader).ok()?;
    if convention.is_identity() {
        return Some(dims);
    }
    let length = |value: f64| convention.length(value);
    Some(match dims {
        ShapeDims::Cube { size } => ShapeDims::Cube { size: length(size) },
        ShapeDims::Sphere { radius } => ShapeDims::Sphere {
            radius: length(radius),
        },
        ShapeDims::Cylinder { radius, height } => ShapeDims::Cylinder {
            radius: length(radius),
            height: length(height),
        },
        ShapeDims::Cone { radius, height } => ShapeDims::Cone {
            radius: length(radius),
            height: length(height),
        },
        ShapeDims::Capsule { radius, height } => ShapeDims::Capsule {
            radius: length(radius),
            height: length(height),
        },
        ShapeDims::Plane {
            width,
            length: depth,
        } => ShapeDims::Plane {
            width: length(width),
            length: length(depth),
        },
    })
}

fn read_shape_dims_raw(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<ShapeDims> {
    read_primitive_axis(reader, path, type_name)?;
    let dims = match type_name {
        "Cube" => ShapeDims::Cube {
            size: read_shape_dimension(reader, path, "size", 2.0)?,
        },
        "Sphere" => ShapeDims::Sphere {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
        },
        "Cylinder" => ShapeDims::Cylinder {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
            height: read_shape_dimension(reader, path, "height", 2.0)?,
        },
        "Cone" => ShapeDims::Cone {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
            height: read_shape_dimension(reader, path, "height", 2.0)?,
        },
        "Capsule" => ShapeDims::Capsule {
            radius: read_shape_dimension(reader, path, "radius", 0.5)?,
            height: read_shape_dimension(reader, path, "height", 1.0)?,
        },
        "Plane" => ShapeDims::Plane {
            width: read_shape_dimension(reader, path, "width", 2.0)?,
            length: read_shape_dimension(reader, path, "length", 2.0)?,
        },
        _ => return None,
    };
    Some(dims)
}

fn read_shape_dimension(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    name: &str,
    schema_default: f64,
) -> Option<f64> {
    let authored =
        reader.has_authored_attribute(path, name) || !reader.connections(path, name).is_empty();
    match reader.real(path, name) {
        Some(value) if value.is_finite() && value > 0.0 => Some(value),
        Some(value) => {
            error!(
                "[usd-scene] {} has invalid primitive {} = {value}; expected a finite positive value",
                path.as_str(),
                name
            );
            None
        }
        None if authored => {
            error!(
                "[usd-scene] {} has authored primitive {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None => Some(schema_default),
    }
}

/// Read native USD mesh topology after applying the stage axis/unit convention.
pub fn read_usd_mesh_topology(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
) -> Option<UsdMeshTopology> {
    let points = read_usd_mesh_points(reader, path)?;
    let face_vertex_counts = read_int_array(reader, path, "faceVertexCounts")?;
    let face_vertex_indices = read_int_array(reader, path, "faceVertexIndices")?;
    if points.is_empty() || face_vertex_counts.is_empty() || face_vertex_indices.is_empty() {
        return None;
    }
    Some(UsdMeshTopology {
        points,
        face_vertex_counts,
        face_vertex_indices,
    })
}

/// Read a native USD mesh in the indexed form consumed by physics trimeshes.
pub fn read_usd_mesh_indexed(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    let topology = read_usd_mesh_topology(reader, path)?;
    let n_points = topology.points.len() as u32;
    let n_corners = topology.face_vertex_indices.len();
    let mut triangles = Vec::new();
    let mut base = 0usize;

    for &face_count in &topology.face_vertex_counts {
        let count = usize::try_from(face_count).ok()?;
        if base + count > n_corners {
            return None;
        }
        for k in 1..count.saturating_sub(1) {
            let tri = [
                u32::try_from(topology.face_vertex_indices[base]).ok()?,
                u32::try_from(topology.face_vertex_indices[base + k]).ok()?,
                u32::try_from(topology.face_vertex_indices[base + k + 1]).ok()?,
            ];
            if tri.iter().any(|index| *index >= n_points) {
                return None;
            }
            triangles.push(tri);
        }
        base += count;
    }
    if triangles.is_empty() {
        return None;
    }
    Some((topology.points, triangles))
}

pub fn read_usd_mesh_points(reader: &dyn UsdReadObject, path: &SdfPath) -> Option<Vec<[f32; 3]>> {
    let points = reader.points3(path, "points");
    if points.is_empty()
        || points
            .iter()
            .any(|point| !Vec3::from_array(*point).is_finite())
    {
        if points
            .iter()
            .any(|point| !Vec3::from_array(*point).is_finite())
        {
            error!(
                "[usd-scene] {} has non-finite mesh points; refusing geometry projection",
                path.as_str()
            );
        }
        return None;
    }
    let convention = stage_convention(reader).ok()?;
    if convention.is_identity() {
        return Some(points);
    }
    let points: Vec<[f32; 3]> = points
        .into_iter()
        .map(|point| convention.point(Vec3::from_array(point)).to_array())
        .collect();
    if points
        .iter()
        .any(|point| !Vec3::from_array(*point).is_finite())
    {
        error!(
            "[usd-scene] {} mesh points became non-finite after stage conversion; refusing geometry projection",
            path.as_str()
        );
        return None;
    }
    Some(points)
}

fn read_int_array(reader: &dyn UsdReadObject, path: &SdfPath, attr: &str) -> Option<Vec<i32>> {
    match reader.attr_value(path, attr)? {
        Value::IntVec(values) => Some(values),
        Value::Int64Vec(values) => values
            .into_iter()
            .map(|value| i32::try_from(value).ok())
            .collect(),
        _ => None,
    }
}
