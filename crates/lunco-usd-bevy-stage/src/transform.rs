use bevy::math::{DMat4, DVec3};
use bevy::prelude::{EulerRot, Quat, Transform, Vec3};
use lunco_usd_data::units::ConventionTransform;
use openusd::sdf::{Path as SdfPath, Value};

use crate::read::UsdReadObject;
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

/// Split the composed local translation into grid coordinates before any
/// narrowing. Pivots, inverse operations and matrix edits have the same
/// OpenUSD evaluation here as in the authored-pose projector.
pub fn grid_translation_d_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<Option<DVec3>, TransformReadError> {
    let Some(matrix) = reader.local_transform_matrix_d_at(path, time)? else {
        return Ok(None);
    };
    let has_translation =
        read_xform_op_order(reader, path).is_some_and(|order| ordered_translation(&order));
    Ok(has_translation.then(|| matrix.w_axis.truncate()))
}

fn ordered_translation(order: &[String]) -> bool {
    use openusd_schemas::geom::XformOpKind;
    order
        .iter()
        .rev()
        .take_while(|op| op.as_str() != "!resetXformStack!")
        .any(|op| {
            matches!(
                XformOpKind::from_attr_name(op.strip_prefix("!invert!").unwrap_or(op)),
                Some(
                    XformOpKind::Translate
                        | XformOpKind::TranslateX
                        | XformOpKind::TranslateY
                        | XformOpKind::TranslateZ
                        | XformOpKind::Transform
                )
            )
        })
}

/// One native evaluation at the render projection boundary. The optional
/// parent-local translation retains f64 for direct BigSpace grid placement.
pub struct AnimatedTransformSample {
    pub pose: Transform,
    pub grid_translation: Option<DVec3>,
}

/// A native ordered transform stack with retained OpenUSD attribute queries.
/// Its lifetime is bounded by the canonical stage's committed revision.
pub(crate) struct NativeTransformQuery {
    query: openusd_schemas::geom::XformQuery,
    has_order: bool,
    has_translation: bool,
}

impl NativeTransformQuery {
    pub(crate) fn new(reader: &StageView<'_>, path: &SdfPath) -> Result<Self, TransformReadError> {
        use openusd_schemas::geom::{XformQuery, Xformable};
        let order = validated_live_xform_op_order(reader, path)?;
        let prim = reader
            .stage()
            .prim(path)
            .map_err(|_| malformed_transform(path))?;
        let query = XformQuery::new(&Xformable::from_prim_unchecked(prim))
            .map_err(|_| malformed_transform(path))?;
        let has_translation = order.as_deref().is_some_and(ordered_translation);
        Ok(Self {
            query,
            has_order: order.is_some(),
            has_translation,
        })
    }

    fn matrix(&self, path: &SdfPath, time: f64) -> Result<Option<DMat4>, TransformReadError> {
        if !self.has_order {
            return Ok(None);
        }
        let matrix = self
            .query
            .local_transform(openusd::usd::TimeCode::new(time))
            .map_err(|_| malformed_transform(path))?;
        native_matrix_d(matrix, path).map(Some)
    }

    /// Both the render pose and grid translation come from the same evaluation.
    pub(crate) fn sample(
        &self,
        path: &SdfPath,
        time: f64,
        convention: ConventionTransform,
    ) -> Result<Option<AnimatedTransformSample>, TransformReadError> {
        let Some(matrix) = self.matrix(path, time)? else {
            return Ok(None);
        };
        let pose = convention.local_transform(project_transform_matrix_d(matrix, path)?);
        let grid_translation = self.has_translation.then(|| {
            convention
                .canonical_local_matrix_d(matrix)
                .w_axis
                .truncate()
        });
        Ok(Some(AnimatedTransformSample {
            pose,
            grid_translation,
        }))
    }
}

/// Compose a live OpenUSD transform through the shared reader contract.
pub(crate) fn compose_live_xform_order_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    let Some((matrix, convention)) = live_local_matrix_and_convention_at(reader, path, time)?
    else {
        return Ok(None);
    };
    let raw = project_transform_matrix_d(matrix, path)?;
    Ok(Some(convention.local_transform(raw)))
}

/// Transform facts retained by one immutable projection snapshot.
pub(crate) struct PreparedTransform {
    pub local: Option<(Transform, DMat4)>,
    pub resets_stack: bool,
    pub might_vary: bool,
}

/// Resolve the validated native op query once for the snapshot's matrix,
/// reset and variation facts. Inactive prims retain metadata without evaluating
/// a local matrix. Render decomposition and canonical geometry conversion keep
/// their separate precision boundaries.
pub(crate) fn prepare_transform_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
    active: bool,
) -> Result<PreparedTransform, TransformReadError> {
    let query = NativeTransformQuery::new(reader, path)?;
    let local = if active {
        match query.matrix(path, time)? {
            Some(matrix) => {
                let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
                let raw = project_transform_matrix_d(matrix, path)?;
                Some((
                    convention.local_transform(raw),
                    convention.canonical_local_matrix_d(matrix),
                ))
            }
            None => None,
        }
    } else {
        None
    };
    Ok(PreparedTransform {
        local,
        resets_stack: query.query.resets_xform_stack(),
        might_vary: query
            .query
            .transform_might_be_time_varying()
            .map_err(|_| malformed_transform(path))?,
    })
}

fn live_local_matrix_and_convention_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<(DMat4, ConventionTransform)>, TransformReadError> {
    if validated_live_xform_op_order(reader, path)?.is_none() {
        return Ok(None);
    }
    let matrix = local_to_parent_matrix_d_at(reader, path, time)?;
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some((matrix, convention)))
}

pub(crate) fn validated_live_xform_op_order(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Option<Vec<String>>, TransformReadError> {
    use openusd::usd::SchemaBase;
    use openusd_schemas::geom::{Xformable, XformableSchema};
    let prim = reader
        .stage()
        .prim(path)
        .map_err(|_| malformed_transform(path))?;
    if !prim.is_valid().map_err(|_| malformed_transform(path))? {
        return Err(malformed_transform(path));
    }
    let xform = Xformable::from_prim_unchecked(prim);
    let order = xform
        .xform_op_order()
        .map_err(|_| malformed_transform(path))?;
    // An omitted or blocked stack has no authored value. An unreadable
    // authored value cannot be treated as an identity transform.
    if order.is_none()
        && xform
            .prim()
            .attribute("xformOpOrder")
            .has_authored_value()
            .map_err(|_| malformed_transform(path))?
    {
        return Err(malformed_transform(path));
    }
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
    .local_transform(openusd::usd::TimeCode::new(time))
    .map_err(|_| malformed_transform(path))?;
    native_matrix_d(matrix, path)
}

fn native_matrix_d(
    matrix: openusd::gf::Mat4d,
    path: &SdfPath,
) -> Result<DMat4, TransformReadError> {
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
    let Some((matrix, convention)) = live_local_matrix_and_convention_at(reader, path, time)?
    else {
        return Ok(None);
    };
    Ok(Some(convention.canonical_local_matrix_d(matrix)))
}

/// Canonical double-precision parent-local pose in the selected projection.
pub fn local_pose_matrix(
    reader: &dyn UsdReadObject,
    path: &str,
    time: f64,
) -> Result<DMat4, String> {
    let path = SdfPath::new(path).map_err(|e| e.to_string())?;
    if !reader.has_prim(&path) {
        return Err(format!("missing USD prim {path}"));
    }
    reader
        .local_transform_matrix_d_at(&path, time)
        .map(|matrix| matrix.unwrap_or(DMat4::IDENTITY))
        .map_err(|e| e.to_string())
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
    project_transform_matrix_d(matrix, path)
}

/// Decompose the canonical matrix before narrowing at the render boundary.
/// Narrowing matrix columns first turns rotation roundoff into nonuniform
/// scale, which can change an analytic collision shape into a faceted hull.
pub fn project_transform_matrix_d(
    matrix: DMat4,
    path: &SdfPath,
) -> Result<Transform, TransformReadError> {
    if !matrix.is_finite()
        || matrix.x_axis.truncate().length_squared() == 0.0
        || matrix.y_axis.truncate().length_squared() == 0.0
        || matrix.z_axis.truncate().length_squared() == 0.0
    {
        return Err(malformed_transform(path));
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let projected = Transform {
        scale: scale.as_vec3(),
        rotation: rotation.as_quat(),
        translation: translation.as_vec3(),
    };
    if !projected.scale.is_finite()
        || !projected.rotation.is_finite()
        || !projected.translation.is_finite()
    {
        return Err(malformed_transform(path));
    }
    Ok(projected)
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
        if reader.resets_xform_stack(prim)? {
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

/// Compose a canonical physical mount in its body's scale-free frame.
/// Absolute translations remain `f64` through subtraction, so moving
/// an assembly far from the origin cannot change its local geometry.
pub fn transform_in_body_frame(
    reader: &dyn UsdReadObject,
    body_path: &SdfPath,
    prim_path: &SdfPath,
) -> Option<lunco_core::DTransform> {
    let body = world_transform_matrix_d_at(reader, body_path, 0.0).ok()?;
    let prim = world_transform_matrix_d_at(reader, prim_path, 0.0).ok()?;
    if [body, prim].into_iter().any(|matrix| {
        !matrix.row(3).abs_diff_eq(bevy::math::DVec4::W, 1e-12)
            || !matrix.determinant().is_finite()
            || matrix.determinant() == 0.0
    }) {
        return None;
    }
    let (_, body_rotation, body_translation) = body.to_scale_rotation_translation();
    let (_, prim_rotation, prim_translation) = prim.to_scale_rotation_translation();
    let inverse = body_rotation.inverse();
    lunco_core::DTransform::new(
        inverse * (prim_translation - body_translation),
        inverse * prim_rotation,
        DVec3::ONE,
    )
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

#[cfg(test)]
mod high_precision_grid_translation_tests {
    use super::*;
    use lunco_usd_compose::recipe::StageRecipe;

    fn plan(source: &str) -> crate::UsdStageProjectionPlan {
        let recipe = StageRecipe::from_source("high_precision.usda", source);
        crate::UsdStageProjectionPlan::from_recipe(&recipe).expect("compose authored stage")
    }

    #[test]
    fn rotation_projection_preserves_scale_without_snapping_authored_changes() {
        let path = SdfPath::new("/Body").unwrap();
        for i in 0..300 {
            let rotation = bevy::math::DQuat::from_euler(
                EulerRot::XYZ,
                i as f64 * 0.013,
                i as f64 * 0.027,
                i as f64 * 0.043,
            );
            for scale in [DVec3::ONE, DVec3::new(1.0, 1.000001, 2.0)] {
                let matrix = DMat4::from_scale_rotation_translation(scale, rotation, DVec3::ONE);
                let transform = project_transform_matrix_d(matrix, &path).unwrap();
                assert_eq!(transform.scale, scale.as_vec3());
            }
        }
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
    fn physical_mount_reduction_is_invariant_under_large_translation() {
        for offset in [0.0, 100_000_000.0, 149_597_870_700.0] {
            let source = format!(
                r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Body" {{
    double3 xformOp:translate = ({offset}, 0, 0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
    def Xform "Mount" {{
        double3 xformOp:translate = (1, 0.125, -2)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }}
}}"#
            );
            let stage = plan(&source);
            let mount = transform_in_body_frame(
                &stage,
                &SdfPath::new("/Body").unwrap(),
                &SdfPath::new("/Body/Mount").unwrap(),
            )
            .unwrap();
            assert_eq!(mount.translation, DVec3::new(1.0, 0.125, -2.0));
        }
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
    fn retained_animation_query_tracks_committed_stack_and_sample_edits() {
        let stage = crate::canonical::CanonicalStage::from_recipe(&StageRecipe::from_source(
            "sampled.usda",
            r#"#usda 1.0
(metersPerUnit = 0.01, upAxis = "Z")
def Xform "Part" {
    double3 xformOp:translate.timeSamples = {0: (100, 200, 300), 10: (300, 400, 500)}
    double xformOp:rotateZ = 90
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateZ"]
}
"#,
        ))
        .unwrap();
        let path = SdfPath::new("/Part").unwrap();
        let verify = |time| {
            let sample = stage
                .sample_animated_transform(&path, time)
                .unwrap()
                .unwrap();
            let view = stage.view();
            let expected = local_transform_at(&view, &path, time).unwrap().unwrap();
            assert!(
                sample
                    .pose
                    .translation
                    .abs_diff_eq(expected.translation, 1e-6)
            );
            assert!(sample.pose.rotation.abs_diff_eq(expected.rotation, 1e-6));
            assert!(sample.pose.scale.abs_diff_eq(expected.scale, 1e-6));
            assert_eq!(
                sample.grid_translation,
                grid_translation_d_at(&view, &path, time).unwrap()
            );
            sample
        };
        let first = verify(5.0).pose.translation;
        verify(2.0);
        verify(9.0);
        let prim = stage.stage().prim(&path).unwrap();
        prim.attribute("xformOp:translate")
            .set_at(
                openusd::sdf::Value::Vec3d(openusd::gf::vec3d(900.0, 800.0, 700.0)),
                openusd::usd::TimeCode::new(5.0),
            )
            .unwrap();
        let edited = verify(5.0).pose.translation;
        assert_ne!(edited, first);
        prim.attribute("xformOpOrder")
            .set(openusd::sdf::Value::TokenVec(vec![
                "xformOp:rotateZ".into(),
                "xformOp:translate".into(),
            ]))
            .unwrap();
        assert_ne!(verify(5.0).pose.translation, edited);
        prim.attribute("xformOpOrder")
            .set(openusd::sdf::Value::TokenVec(vec![
                "xformOp:rotateZ".into(),
            ]))
            .unwrap();
        assert_eq!(verify(5.0).grid_translation, None);
    }

    #[test]
    fn rotated_double_translation_is_split_in_parent_coordinates() {
        let stage = plan(
            r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Vehicle"
{
    double3 xformOp:translate = (149597870700, 0, 0)
    double3 xformOp:rotateXYZ = (0, 0, 30)
    uniform token[] xformOpOrder = ["xformOp:rotateXYZ", "xformOp:translate"]
}
"#,
        );
        let path = SdfPath::new("/Vehicle").expect("valid prim path");

        let position = grid_translation_d_at(&stage, &path, 0.0).unwrap().unwrap();
        let expected = bevy::math::DQuat::from_rotation_z(30.0_f64.to_radians())
            * DVec3::X
            * 149_597_870_700.0;
        assert!(position.abs_diff_eq(expected, 0.0001));
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
