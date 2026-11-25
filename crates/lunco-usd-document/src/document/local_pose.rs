use super::{
    ConventionTransform, LayerId, SdfPath, Stage, StageMetrics, UsdDocument, UsdOp,
    usd_rotate_xyz_degrees,
};
use bevy::math::{DMat4, DQuat, DVec3};
use openusd::sdf::Value;
use openusd_schemas::geom::{Xformable, XformableExt, XformableSchema};

impl UsdDocument {
    /// Plan a pose edit against this document's current authored layers.
    pub fn local_pose_ops(
        &self,
        target: &LayerId,
        path: &str,
        translation: Option<DVec3>,
        rotation: Option<DQuat>,
        scale: Option<DVec3>,
    ) -> Result<Vec<UsdOp>, String> {
        let stage = self.open_composed_stage().map_err(|e| e.to_string())?;
        local_pose_ops(&stage, target, path, translation, rotation, scale)
    }
}

/// Lower absolute parent-local pose channels to a single journal group. Simple
/// TRS stacks retain editable channels. A mixed stack retains its axes, pivots
/// and animation under one parent-space adjustment, evaluated at the same
/// reference time as the live authored-transform projector.
pub fn local_pose_ops(
    stage: &Stage,
    target: &LayerId,
    path: &str,
    translation: Option<DVec3>,
    rotation: Option<DQuat>,
    scale: Option<DVec3>,
) -> Result<Vec<UsdOp>, String> {
    if translation.is_none() && rotation.is_none() && scale.is_none() {
        return Ok(Vec::new());
    }
    let err = |msg: &str| format!("cannot author local pose at {path}: {msg}");
    if translation.is_some_and(|v| !v.is_finite())
        || scale.is_some_and(|v| !v.is_finite() || v.abs().min_element() == 0.0)
        || rotation.is_some_and(|q| !q.is_finite() || !q.is_normalized())
    {
        return Err(err("invalid pose channels"));
    }
    let prim = stage
        .prim(SdfPath::new(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if !prim.is_valid().map_err(|e| e.to_string())? {
        return Err(err("prim is missing"));
    }
    let xform = Xformable::from_prim_unchecked(prim.clone());
    let mut order: Vec<String> = xform
        .xform_op_order()
        .map_err(|e| e.to_string())?
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect();
    let common = [
        "!resetXformStack!",
        "xformOp:translate",
        "xformOp:rotateXYZ",
        "xformOp:scale",
    ];
    let mut expected = common.into_iter();
    let mut simple = order.iter().all(|op| expected.any(|name| name == op));
    for (name, changed) in [
        ("xformOp:translate", translation.is_some()),
        ("xformOp:rotateXYZ", rotation.is_some()),
        ("xformOp:scale", scale.is_some()),
    ] {
        if changed {
            if let Some(kind) = prim
                .attribute(name)
                .type_name()
                .map_err(|e| e.to_string())?
            {
                simple &= kind.as_str() == "double3";
            }
        }
    }
    if simple {
        let mut ops = Vec::new();
        if let Some(value) = translation {
            ops.push(UsdOp::SetTranslate {
                edit_target: target.clone(),
                path: path.into(),
                value: value.to_array(),
            });
        }
        if let Some(value) = rotation {
            ops.push(UsdOp::SetRotate {
                edit_target: target.clone(),
                path: path.into(),
                value: usd_rotate_xyz_degrees(value),
            });
        }
        if let Some(value) = scale {
            ops.push(UsdOp::SetScale {
                edit_target: target.clone(),
                path: path.into(),
                value: value.to_array(),
            });
        }
        return Ok(ops);
    }

    const ADJUSTMENT: &str = "xformOp:transform:poseEdit";
    let first = order
        .iter()
        .rposition(|token| token == "!resetXformStack!")
        .map_or(0, |i| i + 1);
    let existing = order.get(first).is_some_and(|token| token == ADJUSTMENT);
    let count = order
        .iter()
        .filter(|t| t.trim_start_matches("!invert!") == ADJUSTMENT)
        .count();
    let attribute = prim.attribute(ADJUSTMENT);
    if count != usize::from(existing)
        || (!existing && attribute.type_name().map_err(|e| e.to_string())?.is_some())
    {
        return Err(err(
            "poseEdit is already used elsewhere in the authored stack",
        ));
    }
    let time = openusd::usd::TimeCode::new(0.);
    let current =
        DMat4::from_cols_array(&xform.local_transform(time).map_err(|e| e.to_string())?.0);
    if !current.is_finite() || current.determinant() == 0.0 {
        return Err(err("singular or non-finite transform stack"));
    }
    let (s, r, t) = current.to_scale_rotation_translation();
    if !r.is_normalized()
        || !DMat4::from_scale_rotation_translation(s, r, t)
            .abs_diff_eq(current, 1e-9 * s.abs().max_element().max(1.0))
    {
        return Err(err("the composed transform contains shear or perspective"));
    }
    let convention = ConventionTransform::from_stage_metrics(
        &StageMetrics::from_stage(stage).map_err(|e| e.to_string())?,
    );
    let desired = DMat4::from_scale_rotation_translation(
        scale.map_or(s, |v| convention.stage_scale_vec_d(v)),
        rotation.map_or(r, |q| convention.stage_rotation_d(q)),
        translation.map_or(t, |v| convention.stage_point_d(v)),
    );
    let mut adjustment = desired * current.inverse();
    if existing {
        let old = attribute
            .get_at::<openusd::gf::Mat4d>(time)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| err("pose adjustment has no matrix value"))?;
        adjustment *= DMat4::from_cols_array(&old.0);
    }
    if !adjustment.is_finite() {
        return Err(err("pose adjustment exceeds double precision range"));
    }
    let set = |name: &str, kind: &str, value: Value| -> Result<UsdOp, String> {
        Ok(UsdOp::SetAttribute {
            edit_target: target.clone(),
            path: path.into(),
            name: name.into(),
            type_name: kind.into(),
            value: openusd::usda::TextWriter::value_to_string(&value).map_err(|e| e.to_string())?,
        })
    };
    let mut ops = vec![set(
        ADJUSTMENT,
        "matrix4d",
        Value::from(adjustment.to_cols_array()),
    )?];
    if !existing {
        order.insert(first, ADJUSTMENT.into());
        ops.push(set("xformOpOrder", "token[]", Value::token_vec(order))?);
    }
    Ok(ops)
}
