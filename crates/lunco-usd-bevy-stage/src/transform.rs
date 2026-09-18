use bevy::prelude::{EulerRot, Mat4, Quat, Transform, Vec3};
use openusd::sdf::{Path as SdfPath, Value};

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

/// Compose a live OpenUSD transform through the shared reader contract.
pub(crate) fn compose_live_xform_order_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    use openusd::schemas::geom::Xformable as _;
    let Some(order) = read_xform_op_order(reader, path) else {
        return if UsdReadObject::has_authored_attribute(reader, path, "xformOpOrder")
            && !authored_empty_xform_op_order(reader, path)
        {
            Err(malformed_transform(path))
        } else {
            Ok(None)
        };
    };
    if !valid_xform_op_order(reader, path, &order) {
        return Err(malformed_transform(path));
    }
    let matrix = XformablePrim(reader.stage().prim(path.clone()))
        .local_to_parent_transform(time)
        .map_err(|_| malformed_transform(path))?;
    let cols: [f32; 16] = std::array::from_fn(|i| matrix.0[i] as f32);
    let raw = Transform::from_matrix(Mat4::from_cols_array(&cols));
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some(convention.local_transform(raw)))
}

/// The canonical local transform from the composed reader.
pub fn local_transform_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    reader.local_transform_at(path, time)
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
    if !reader.has_prim(path) {
        return Err(TransformReadError {
            prim: path.to_string(),
        });
    }
    let mut chain = Vec::new();
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            break;
        }
        chain.push(p);
        cur = chain.last().and_then(SdfPath::parent);
    }
    let mut acc = Transform::IDENTITY;
    let mut in_read_surface = false;
    for prim in chain.iter().rev() {
        if !reader.has_prim(prim) {
            if in_read_surface {
                break;
            }
            continue;
        }
        in_read_surface = true;
        if let Some(local) = reader.local_transform_at(prim, 0.0)? {
            acc = acc.mul_transform(local);
        }
    }
    Ok(acc)
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

/// Resolve inherited USD Imageable visibility and purpose on a live stage.
pub(crate) fn stage_prim_is_invisible_or_guide(reader: &StageView<'_>, path: &SdfPath) -> bool {
    use openusd::schemas::geom::Imageable as _;
    let imageable = XformablePrim(reader.stage().prim(path.clone()));
    imageable
        .compute_visibility()
        .map(|value| value == openusd::schemas::geom::Visibility::Invisible)
        .unwrap_or(false)
        || imageable
            .compute_purpose()
            .map(|value| value == openusd::schemas::geom::Purpose::Guide)
            .unwrap_or(false)
}

pub fn read_xform_op_order(reader: &dyn UsdReadObject, path: &SdfPath) -> Option<Vec<String>> {
    let order: Vec<String> = match reader.attr_value(path, "xformOpOrder")? {
        Value::TokenVec(values) => values.iter().map(ToString::to_string).collect(),
        Value::StringVec(values) => values,
        Value::TokenListOp(op) => op
            .flatten()
            .into_iter()
            .map(|token| token.to_string())
            .collect(),
        Value::StringListOp(op) => op.flatten(),
        _ => return None,
    };
    (!order.is_empty()).then_some(order)
}

fn is_valid_xform_op_token(op: &str, index: usize) -> bool {
    let (inverted, base) = match op.strip_prefix("!invert!") {
        Some(base) => (true, base),
        None => (false, op),
    };
    if base == RESET_XFORM_STACK {
        return !inverted && index == 0;
    }
    if inverted && base.starts_with('!') {
        return false;
    }
    [
        "xformOp:translate",
        "xformOp:scale",
        "xformOp:transform",
        "xformOp:orient",
        "xformOp:rotateX",
        "xformOp:rotateY",
        "xformOp:rotateZ",
        "xformOp:rotateXYZ",
        "xformOp:rotateXZY",
        "xformOp:rotateYXZ",
        "xformOp:rotateYZX",
        "xformOp:rotateZXY",
        "xformOp:rotateZYX",
    ]
    .iter()
    .any(|kind| base == *kind || base.strip_prefix(kind).is_some_and(|s| s.starts_with(':')))
}

fn valid_xform_op_order<R: UsdRead>(reader: &R, path: &SdfPath, order: &[String]) -> bool {
    order.iter().enumerate().all(|(index, op)| {
        if !is_valid_xform_op_token(op, index) {
            return false;
        }
        let base = op.strip_prefix("!invert!").unwrap_or(op);
        base == RESET_XFORM_STACK || reader.has_authored_attribute(path, base)
    })
}

fn authored_empty_xform_op_order(reader: &StageView<'_>, path: &SdfPath) -> bool {
    match UsdReadObject::attr_value(reader, path, "xformOpOrder") {
        Some(Value::TokenVec(values)) => values.is_empty(),
        Some(Value::StringVec(values)) => values.is_empty(),
        Some(Value::TokenListOp(op)) => op.flatten().is_empty(),
        Some(Value::StringListOp(op)) => op.flatten().is_empty(),
        _ => false,
    }
}

struct XformablePrim(openusd::usd::Prim);

impl openusd::usd::SchemaBase for XformablePrim {
    const KIND: openusd::usd::SchemaKind = openusd::usd::SchemaKind::AbstractTyped;

    fn prim(&self) -> &openusd::usd::Prim {
        &self.0
    }
}

impl openusd::schemas::geom::Imageable for XformablePrim {}
impl openusd::schemas::geom::Xformable for XformablePrim {}

pub const RESET_XFORM_STACK: &str = "!resetXformStack!";
