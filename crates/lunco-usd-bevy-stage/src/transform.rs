use bevy::math::{DMat4, DVec3};
use bevy::prelude::{EulerRot, Mat4, Quat, Transform, Vec3};
use openusd::sdf::{Path as SdfPath, Value};

use crate::read::read_vec3_f64_at;
use crate::read::{UsdRead, UsdReadObject};
use crate::units::stage_convention;
use crate::view::StageView;

/// Convert a USD `rotateXYZ` value authored in degrees into a Bevy quaternion.
///
/// USD's rotation values use degrees and the `XYZ` operation applies fixed
/// axes. Keeping this conversion in the render-free transform substrate gives
/// authoring and visual adapters one canonical rotation convention.
pub fn euler_xyz_deg_to_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::XYZEx,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

/// A USD transform stack was authored but could not be composed safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformReadError {
    pub prim: String,
}

impl std::fmt::Display for TransformReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed authored transform at {}", self.prim)
    }
}

impl std::error::Error for TransformReadError {}

fn malformed_transform(path: &SdfPath) -> TransformReadError {
    TransformReadError {
        prim: path.as_str().to_owned(),
    }
}

/// Compose a reader's local USD transform at `time`.
pub fn compose_xform_order_at<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    reader.local_transform_at(path, time)
}

/// Read a grid-direct USD translation without narrowing its `double3` value.
///
/// High-precision cell splitting is defined for a pure `xformOp:translate`
/// stack. More complex stacks can move the translation through pivots or
/// matrix operations, so a double-valued position in one of those stacks is
/// rejected instead of being silently projected through Bevy's `f32`
/// `Transform`.
pub fn grid_translation_d_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<Option<DVec3>, TransformReadError> {
    if reader.attr_type_name(path, "xformOp:translate").as_deref() != Some("double3") {
        return Ok(None);
    }

    let Some(order) = read_xform_op_order(reader, path) else {
        if !reader.has_authored_attribute(path, "xformOpOrder")
            || authored_empty_xform_op_order(reader, path)
        {
            return Ok(None);
        }
        return Err(malformed_transform(path));
    };
    let translation_is_ordered = order.iter().any(|operation| {
        operation.strip_prefix("!invert!").unwrap_or(operation) == "xformOp:translate"
    });
    if !translation_is_ordered {
        return Ok(None);
    }
    if order.first().map(String::as_str) != Some("xformOp:translate")
        || order.iter().skip(1).any(|operation| {
            let operation = operation.strip_prefix("!invert!").unwrap_or(operation);
            !(operation.starts_with("xformOp:rotate")
                || operation.starts_with("xformOp:orient")
                || operation.starts_with("xformOp:scale"))
        })
    {
        return Err(malformed_transform(path));
    }
    let Some(value) = read_vec3_f64_at(reader, path, "xformOp:translate", time) else {
        return Err(malformed_transform(path));
    };
    let value = DVec3::from_array(value);
    if !value.is_finite() {
        return Err(malformed_transform(path));
    }
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some(convention.point_d(value)))
}

/// Compose a live OpenUSD transform through the shared reader contract.
pub(crate) fn compose_live_xform_order_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    if validated_live_xform_op_order(reader, path)?.is_none() {
        return Ok(None);
    }
    let matrix = local_to_parent_matrix_d_at(reader, path, time)?;
    let cols: [f32; 16] = matrix.to_cols_array().map(|value| value as f32);
    let raw = Transform::from_matrix(Mat4::from_cols_array(&cols));
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some(convention.local_transform(raw)))
}

fn validated_live_xform_op_order(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Option<Vec<String>>, TransformReadError> {
    use openusd_schemas::geom::{Xformable, XformableSchema};
    let xform =
        Xformable::from_prim_unchecked(reader.stage().prim(path).expect("validated USD path"));
    let order = xform
        .xform_op_order()
        .map_err(|_| malformed_transform(path))?;
    Ok(order
        .filter(|order| !order.is_empty())
        .map(|order| order.into_iter().map(|token| token.to_string()).collect()))
}

fn local_to_parent_matrix_d_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<DMat4, TransformReadError> {
    use openusd_schemas::geom::{Xformable, XformableExt};

    let matrix = Xformable::from_prim_unchecked(
        reader
            .stage()
            .prim(path.clone())
            .expect("validated USD path"),
    )
    .local_transformation(openusd::usd::TimeCode::new(time))
    .map_err(|_| malformed_transform(path))?;
    // Gf matrices are row-major for row-vector transforms. Interpreting that
    // storage as Bevy columns transposes into the equivalent column-vector
    // matrix, including moving translation into the final column.
    let matrix = DMat4::from_cols_array(&matrix.0);
    if !matrix.is_finite() {
        return Err(malformed_transform(path));
    }
    Ok(matrix)
}

/// The canonical local transform from the composed reader.
pub fn local_transform_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    reader.local_transform_at(path, time)
}

/// The canonical local transform matrix at one sample time, without reducing
/// the authored OpenUSD `Matrix4d` to Bevy's render-oriented `Transform`.
pub fn local_transform_matrix_d_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<DMat4>, TransformReadError> {
    if validated_live_xform_op_order(reader, path)?.is_none() {
        return Ok(None);
    }
    let matrix = local_to_parent_matrix_d_at(reader, path, time)?;
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some(convention.canonical_local_matrix_d(matrix)))
}

/// Read a prim's canonical local transform at its default time.
pub fn read_transform_from_usd(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
) -> Result<Transform, TransformReadError> {
    match local_transform_at(reader, path, 0.0) {
        Ok(Some(transform)) => Ok(transform),
        Ok(None) => Ok(Transform::IDENTITY),
        Err(error) => Err(error),
    }
}

/// Compose a prim's world transform through the available USD read surface.
///
/// Prepared reference readers may begin below the scene root, so absent outer
/// ancestors are skipped until the first prim in the read surface. Once that
/// surface has begun, a missing interior prim stops composition at the
/// reference boundary rather than inventing a transform across the gap.
pub fn world_transform(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
) -> Result<Transform, TransformReadError> {
    let matrix = world_transform_matrix_d_at(reader, path, 0.0)?;
    // Explicit projection to the render-oriented Transform boundary.
    let columns = matrix.to_cols_array().map(|value| value as f32);
    if columns.iter().any(|value| !value.is_finite()) {
        return Err(malformed_transform(path));
    }
    Ok(Transform::from_matrix(Mat4::from_cols_array(&columns)))
}

/// Compose canonical f64 geometry through the reader's USD transform stack.
/// A reset stack discards all ancestor transforms at that prim. Missing outer
/// ancestors delimit prepared reference readers, as in `world_transform`.
pub fn world_transform_matrix_d_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<DMat4, TransformReadError> {
    if !reader.has_prim(path) {
        return Err(malformed_transform(path));
    }
    let mut chain = Vec::new();
    let mut current = Some(path.clone());
    while let Some(prim) = current {
        if prim.is_abs_root() {
            break;
        }
        current = prim.parent();
        chain.push(prim);
    }
    let mut matrix = DMat4::IDENTITY;
    let mut in_read_surface = false;
    for prim in chain.iter().rev() {
        if !reader.has_prim(prim) {
            if in_read_surface {
                break;
            }
            continue;
        }
        in_read_surface = true;
        if read_xform_op_order(reader, prim)
            .is_some_and(|order| order.iter().any(|op| op == RESET_XFORM_STACK))
        {
            matrix = DMat4::IDENTITY;
        }
        if let Some(local) = reader.local_transform_matrix_d_at(prim, time)? {
            matrix *= local;
        }
    }
    if !matrix.is_finite() {
        return Err(malformed_transform(path));
    }
    Ok(matrix)
}

/// Compose a prim's transform in an authored body's local frame.
pub fn transform_in_body_frame(
    reader: &dyn UsdReadObject,
    body_path: &SdfPath,
    prim_path: &SdfPath,
) -> Option<Transform> {
    let body = world_transform(reader, body_path).ok()?;
    let prim = world_transform(reader, prim_path).ok()?;
    let inv = body.rotation.inverse();
    Some(Transform {
        translation: inv * (prim.translation - body.translation),
        rotation: (inv * prim.rotation).normalize(),
        scale: Vec3::ONE,
    })
}

pub fn read_xform_op_order(reader: &dyn UsdReadObject, path: &SdfPath) -> Option<Vec<String>> {
    let Value::TokenVec(values) = reader.attr_value(path, "xformOpOrder")? else {
        return None;
    };
    let order = values
        .into_iter()
        .map(|token| token.to_string())
        .collect::<Vec<_>>();
    (!order.is_empty()).then_some(order)
}

fn authored_empty_xform_op_order(reader: &dyn UsdReadObject, path: &SdfPath) -> bool {
    match reader.attr_value(path, "xformOpOrder") {
        Some(Value::TokenVec(values)) => values.is_empty(),
        _ => false,
    }
}

pub const RESET_XFORM_STACK: &str = "!resetXformStack!";

#[cfg(test)]
mod high_precision_grid_translation_tests {
    use super::*;
    use lunco_usd_compose::recipe::StageRecipe;

    fn plan(source: &str) -> crate::UsdStageProjectionPlan {
        let recipe = StageRecipe::from_source("high_precision.usda", source);
        crate::UsdStageProjectionPlan::from_recipe(&recipe).expect("compose authored stage")
    }

    #[test]
    fn world_geometry_honors_reset_stack_without_narrowing() {
        let stage = plan(
            r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Parent" {
    double3 xformOp:translate = (1000000000, 0, 0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
    def Xform "Probe" {
        double3 xformOp:translate = (0.125, 0, 0)
        double3 xformOp:scale = (2, 3, 4)
        uniform token[] xformOpOrder = ["!resetXformStack!", "xformOp:translate", "xformOp:scale"]
    }
}"#,
        );
        let path = SdfPath::new("/Parent/Probe").unwrap();
        let matrix = world_transform_matrix_d_at(&stage, &path, 0.0).unwrap();
        assert_eq!(
            matrix.transform_point3(DVec3::ONE),
            DVec3::new(2.125, 3.0, 4.0)
        );
    }

    #[test]
    fn double3_time_samples_keep_precision_for_grid_splitting() {
        let stage = crate::canonical::CanonicalStage::from_recipe(&StageRecipe::from_source(
            "high_precision.usda",
            r#"#usda 1.0
(
    metersPerUnit = 1
    timeCodesPerSecond = 1
)
def Xform "Vehicle"
{
    double3 xformOp:translate.timeSamples = {
        0: (149597870700.125, 0, 0),
        10: (149597870700.375, 0, 0),
    }
    uniform token[] xformOpOrder = ["xformOp:translate"]
}
"#,
        ))
        .expect("native temporal stage");
        let path = SdfPath::new("/Vehicle").expect("valid prim path");

        assert_eq!(
            grid_translation_d_at(&stage.view(), &path, 5.0).expect("valid stack"),
            Some(DVec3::new(149_597_870_700.25, 0.0, 0.0))
        );
    }

    #[test]
    fn unsupported_double3_stack_is_rejected() {
        let stage = plan(
            r#"#usda 1.0
def Xform "Vehicle"
{
    double3 xformOp:translate = (149597870700, 0, 0)
    double3 xformOp:rotateXYZ = (0, 0, 30)
    uniform token[] xformOpOrder = ["xformOp:rotateXYZ", "xformOp:translate"]
}
"#,
        );
        let path = SdfPath::new("/Vehicle").expect("valid prim path");

        assert!(grid_translation_d_at(&stage, &path, 0.0).is_err());
    }

    #[test]
    fn double3_translation_outside_the_ordered_stack_is_ignored() {
        let stage = plan(
            r#"#usda 1.0
def Xform "Camera"
{
    double3 xformOp:translate = (-340, -1907, -340)
    double3 xformOp:rotateXYZ = (116.89, -11.28, 18.27)
    uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
}
"#,
        );
        let path = SdfPath::new("/Camera").expect("valid prim path");

        assert_eq!(
            grid_translation_d_at(&stage, &path, 0.0).expect("valid ordered stack"),
            None,
            "an authored op omitted from xformOpOrder is inert in USD"
        );
    }
}
