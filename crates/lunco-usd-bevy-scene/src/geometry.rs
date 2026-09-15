use bevy::prelude::{error, Quat, Vec3};
use lunco_usd_bevy_core::read::UsdReadObject;
use lunco_usd_bevy_core::stage_convention;
use openusd::schemas::geom::tokens;
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
    let points = reader.points3(path, tokens::A_POINTS);
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

#[cfg(test)]
mod primitive_attribute_tests {
    use super::{read_shape_dims, ShapeDims};
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use openusd::sdf::Path as SdfPath;

    fn parse(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_document::recipe::StageRecipe::from_source(
            "primitive.usda",
            source,
        ))
        .expect("build primitive stage")
    }

    #[test]
    fn omitted_dimensions_use_usd_defaults_but_invalid_authored_values_are_rejected() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "World"
{
    def Sphere "Default" {}
    def Cylinder "Negative"
    {
        double radius = -1
        double height = 2
    }
    def Cylinder "WrongType"
    {
        string radius = "not a number"
        double height = 2
    }
    def Cylinder "BadAxis"
    {
        double radius = 1
        double height = 2
        token axis = "Q"
    }
}
"#,
        );
        let reader = stage.view();
        assert_eq!(
            read_shape_dims(&reader, &SdfPath::new("/World/Default").unwrap(), "Sphere"),
            Some(ShapeDims::Sphere { radius: 1.0 })
        );
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/Negative").unwrap(),
            "Cylinder"
        )
        .is_none());
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/WrongType").unwrap(),
            "Cylinder"
        )
        .is_none());
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/BadAxis").unwrap(),
            "Cylinder"
        )
        .is_none());
    }
}

#[cfg(test)]
mod indexed_mesh_tests {
    //! Native USD mesh topology tests for the render-free scene contract.
    use super::read_usd_mesh_indexed;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use openusd::sdf::Path as SdfPath;

    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_document::recipe::StageRecipe::from_source(
            "t.usda", usda,
        ))
        .expect("build canonical stage")
    }

    /// The collider decode keeps the raw points (4) and fan-triangulates the
    /// quad into two index triples — the form `Collider::trimesh` consumes.
    #[test]
    fn indexed_decode_keeps_points_and_fans_quad() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n}\n",
        );
        let reader = __cs.view();
        let (verts, tris) =
            read_usd_mesh_indexed(&reader, &SdfPath::new("/Quad").unwrap()).expect("indexed mesh");
        assert_eq!(verts.len(), 4, "raw points kept (shared verts)");
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]], "fan (0,k,k+1)");
    }

    /// The collider decode rejects malformed topology the same as the render
    /// path, so no bad trimesh reaches the physics engine.
    #[test]
    fn indexed_decode_rejects_bad_topology() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
        let reader = __cs.view();
        assert!(read_usd_mesh_indexed(&reader, &SdfPath::new("/Bad").unwrap()).is_none());
    }
}

#[cfg(test)]
mod stage_metrics_import_tests {
    //! **P7** — the importer honours the stage's `metersPerUnit` / `upAxis`
    //! (`docs/architecture/41-axes-and-units.md`: "convert once, at the
    //! importer"). These tests pin the authored Z-up/centimetre input contract
    //! and its canonical SI Y-up output.
    use super::{read_shape_dims, read_usd_mesh_indexed, usd_axis_to_quat, ShapeDims};
    use bevy::prelude::{Quat, Vec3};
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_bevy_core::units::{StageMetrics, UpAxis};
    use lunco_usd_bevy_core::{local_transform_at, stage_convention};
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_document::recipe::StageRecipe::from_source(
            "t.usda", usda,
        ))
        .expect("build canonical stage")
    }

    /// An Isaac-Sim-flavoured stage: Z-up, centimetres. `/Tower` sits 3 m up the
    /// stage's up-axis (+Z = 300 cm) and 1 m along +X; it is a Z-axial cylinder
    /// (upright in a Z-up world) of radius 0.5 m / height 2 m, authored in cm.
    const ZUP_CM: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 0.01
    upAxis = "Z"
)

def Xform "World"
{
    def Cylinder "Tower"
    {
        double3 xformOp:translate = (100, 0, 300)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        token axis = "Z"
        double radius = 50
        double height = 200
    }

    def Mesh "Slab"
    {
        point3f[] points = [(0, 0, 100), (100, 0, 100), (0, 100, 100)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
    }
}
"#;

    /// The same scene in our canonical metrics (Y-up, metres) — the control.
    const YUP_M: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 1
)

def Xform "World"
{
    def Cylinder "Tower"
    {
        double3 xformOp:translate = (1, 3, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        token axis = "Y"
        double radius = 0.5
        double height = 2
    }
}
"#;

    #[test]
    fn reads_stage_metrics() {
        let m = StageMetrics::from_reader(&parse(ZUP_CM).view()).expect("valid stage metrics");
        assert_eq!(m.up_axis, UpAxis::Z);
        assert_eq!(m.meters_per_unit, 0.01);
        assert!(!m.is_canonical());

        // Unauthored ⇒ the USD defaults, which are our canonical frame.
        let m = StageMetrics::from_reader(&parse(YUP_M).view()).expect("valid stage metrics");
        assert_eq!(m.up_axis, UpAxis::Y);
        assert_eq!(m.meters_per_unit, 1.0);
        assert!(
            m.is_canonical(),
            "a Y-up metre stage must convert to the identity"
        );
    }

    /// The Z-up centimetre stage imports **upright and at true scale**, with its
    /// declared up-axis converted to canonical Y-up coordinates.
    #[test]
    fn zup_centimetre_stage_imports_upright_and_metre_scaled() {
        let __cs = parse(ZUP_CM);
        let reader = __cs.view();
        let tower = SdfPath::new("/World/Tower").unwrap();

        let tf = local_transform_at(&reader, &tower, 0.0)
            .expect("transform stack is valid")
            .expect("prim authors an xform");
        // (100, 0, 300) cm, Z-up  →  (1, 3, 0) m, Y-up: the stage's +Z (up) is now
        // canonical +Y (up); +X is untouched; the metre scale is 1/100.
        assert!(
            tf.translation.abs_diff_eq(Vec3::new(1.0, 3.0, 0.0), 1e-5),
            "expected (1, 3, 0) m Y-up, got {:?}",
            tf.translation
        );

        // Dimensions convert to metres — the collider and the mesh both read this.
        match read_shape_dims(&reader, &tower, "Cylinder") {
            Some(ShapeDims::Cylinder { radius, height }) => {
                assert!((radius - 0.5).abs() < 1e-9, "radius {radius} m");
                assert!((height - 2.0).abs() < 1e-9, "height {height} m");
            }
            other => panic!("expected Cylinder dims, got {other:?}"),
        }

        // Mesh points convert as points: (0,0,100)cm Z-up → (0,1,0)m Y-up, and
        // (0,100,100) → (0, 1, -1).
        let (points, tris) =
            read_usd_mesh_indexed(&reader, &SdfPath::new("/World/Slab").unwrap()).expect("mesh");
        assert_eq!(tris.len(), 1);
        assert!(Vec3::from_array(points[0]).abs_diff_eq(Vec3::new(0.0, 1.0, 0.0), 1e-5));
        assert!(Vec3::from_array(points[1]).abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5));
        assert!(Vec3::from_array(points[2]).abs_diff_eq(Vec3::new(0.0, 1.0, -1.0), 1e-5));

        // The `axis` token is a STAGE-frame axis: a Z-axial cylinder stands up in a
        // Z-up world, so after conversion it must stand up along canonical +Y —
        // i.e. the composed geometry rotation maps the primitive's own +Y to +Y.
        let conv = stage_convention(&reader).expect("valid stage convention");
        let q = conv.orient(usd_axis_to_quat("Z").unwrap_or(Quat::IDENTITY));
        assert!(
            (q * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-5),
            "a Z-axial cylinder on a Z-up stage must end up axial with canonical up, got {:?}",
            q * Vec3::Y
        );
    }

    /// The Z-up/cm stage and its hand-written canonical twin import to the SAME
    /// pose and dimensions — the round-trip guard doc 41 §"three holes" asks for.
    #[test]
    fn zup_cm_stage_matches_its_canonical_twin() {
        let __zup = parse(ZUP_CM);
        let __yup = parse(YUP_M);
        let zup = __zup.view();
        let yup = __yup.view();
        let tower = SdfPath::new("/World/Tower").unwrap();

        let a = local_transform_at(&zup, &tower, 0.0).unwrap().unwrap();
        let b = local_transform_at(&yup, &tower, 0.0).unwrap().unwrap();
        assert!(a.translation.abs_diff_eq(b.translation, 1e-5));

        assert_eq!(
            read_shape_dims(&zup, &tower, "Cylinder"),
            read_shape_dims(&yup, &tower, "Cylinder"),
        );
    }

    /// A canonical stage is bit-for-bit unaffected — every asset we ship takes
    /// this path, so the conversion cannot regress existing content.
    #[test]
    fn canonical_stage_is_untouched() {
        let __cs = parse(YUP_M);
        let reader = __cs.view();
        let tower = SdfPath::new("/World/Tower").unwrap();
        assert!(stage_convention(&reader)
            .expect("valid stage convention")
            .is_identity());
        let tf = local_transform_at(&reader, &tower, 0.0).unwrap().unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 3.0, 0.0), 1e-6));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-6));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-6));
    }

    /// An unsupported declaration must not import silently-wrong: the stage is
    /// rejected instead of being replaced with the canonical frame.
    #[test]
    fn unsupported_declarations_are_rejected() {
        let bogus = parse(
            "#usda 1.0\n(\n    upAxis = \"X\"\n    metersPerUnit = 0\n)\ndef Xform \"W\"\n{\n}\n",
        );
        let error = StageMetrics::from_reader(&bogus.view()).expect_err("malformed metadata");
        assert!(matches!(
            error,
            lunco_usd_document::units::StageMetricsError::InvalidUpAxis(_)
        ));
        assert!(stage_convention(&bogus.view()).is_err());
        assert!(matches!(
            StageMetrics::from_stage(bogus.stage()),
            Err(lunco_usd_document::units::StageMetricsError::InvalidUpAxis(
                _
            ))
        ));

        let malformed_units = parse(
            "#usda 1.0\n(\n    upAxis = \"Y\"\n    metersPerUnit = 0\n)\ndef Xform \"W\"\n{\n}\n",
        );
        assert!(matches!(
            StageMetrics::from_reader(&malformed_units.view()),
            Err(lunco_usd_document::units::StageMetricsError::InvalidMetersPerUnit(_))
        ));
        assert!(matches!(
            StageMetrics::from_stage(malformed_units.stage()),
            Err(lunco_usd_document::units::StageMetricsError::InvalidMetersPerUnit(_))
        ));
    }
}
