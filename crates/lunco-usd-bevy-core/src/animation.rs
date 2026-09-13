//! USD animation topology and rotation decoding.
//!
//! This module is render-free. It answers which authored USD channels carry
//! time samples and decodes the standard transform values used by both the
//! visual animation adapter and the initial visual projection.

use bevy::prelude::{EulerRot, Mat4, Quat, Transform, Vec3};
use openusd::sdf::{Path as SdfPath, Value};

use crate::read::{attr_has_time_samples, read_vec3_f64_at, stage_time_codes_per_second};
use crate::{resolve_bound_shader, UsdRead, UsdReadObject};

/// The USD rotation xform ops, in sampler precedence: quaternion `orient`, the
/// six Euler-order triples, then the single-axis scalars.
pub const ROTATION_OPS: [&str; 10] = [
    "xformOp:orient",
    "xformOp:rotateXYZ",
    "xformOp:rotateXZY",
    "xformOp:rotateYXZ",
    "xformOp:rotateYZX",
    "xformOp:rotateZXY",
    "xformOp:rotateZYX",
    "xformOp:rotateX",
    "xformOp:rotateY",
    "xformOp:rotateZ",
];

/// The xform ops the USD animation adapter can sample.
pub const ANIMATED_XFORM_OPS: [&str; 3] =
    ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"];

/// The bound-shader inputs the USD animation adapter can sample.
pub const ANIMATED_SHADER_INPUTS: [&str; 2] = ["inputs:diffuseColor", "inputs:opacity"];

/// True iff any xform channel on `path` carries time samples.
pub fn prim_has_xform_time_samples<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    attr_has_time_samples(reader, path, "xformOp:translate")
        || attr_has_time_samples(reader, path, "xformOp:scale")
        || attr_has_time_samples(reader, path, "xformOp:transform")
        || prim_rotation_animated(reader, path)
}

/// True iff a prim carries any channel consumed by the USD animation adapter.
pub fn prim_is_animated<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    if prim_has_xform_time_samples(reader, path)
        || attr_has_time_samples(reader, path, "visibility")
        || attr_has_time_samples(reader, path, "primvars:displayColor")
    {
        return true;
    }
    resolve_bound_shader(reader, path).is_some_and(|shader| {
        ANIMATED_SHADER_INPUTS
            .iter()
            .any(|input| attr_has_time_samples(reader, &shader, input))
    })
}

/// The authored time-code span `(first, last)` of one sampled attribute.
fn attr_sample_span(reader: &dyn UsdReadObject, path: &SdfPath, attr: &str) -> Option<(f64, f64)> {
    let times = reader.time_sample_times(path, attr);
    Some((*times.first()?, *times.last()?))
}

/// The authored animation span `(start, end)` in seconds for one prim.
pub fn animated_time_range(reader: &dyn UsdReadObject, path: &SdfPath) -> Option<(f64, f64)> {
    let mut spans = Vec::new();
    for op in ANIMATED_XFORM_OPS {
        spans.extend(attr_sample_span(reader, path, op));
    }
    for op in ROTATION_OPS {
        spans.extend(attr_sample_span(reader, path, op));
    }
    spans.extend(attr_sample_span(reader, path, "visibility"));
    spans.extend(attr_sample_span(reader, path, "primvars:displayColor"));
    if let Some(shader) = resolve_bound_shader(reader, path) {
        for input in ANIMATED_SHADER_INPUTS {
            spans.extend(attr_sample_span(reader, &shader, input));
        }
    }
    let lo = spans
        .iter()
        .map(|span| span.0)
        .fold(f64::INFINITY, f64::min);
    let hi = spans
        .iter()
        .map(|span| span.1)
        .fold(f64::NEG_INFINITY, f64::max);
    if hi < lo {
        return None;
    }
    let tcps = stage_time_codes_per_second(reader);
    Some((lo / tcps, hi / tcps))
}

/// Sample a vec3 channel only when it carries authored time samples.
pub fn sample_animated_vec3(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<[f64; 3]> {
    if !attr_has_time_samples(reader, path, attr) {
        return None;
    }
    read_vec3_f64_at(reader, path, attr, time)
}

fn euler_op_to_quat(op: &str, deg: Vec3) -> Option<Quat> {
    let (x, y, z) = (deg.x.to_radians(), deg.y.to_radians(), deg.z.to_radians());
    let q = match op {
        "xformOp:rotateXYZ" => Quat::from_euler(EulerRot::XYZEx, x, y, z),
        "xformOp:rotateXZY" => Quat::from_euler(EulerRot::XZYEx, x, z, y),
        "xformOp:rotateYXZ" => Quat::from_euler(EulerRot::YXZEx, y, x, z),
        "xformOp:rotateYZX" => Quat::from_euler(EulerRot::YZXEx, y, z, x),
        "xformOp:rotateZXY" => Quat::from_euler(EulerRot::ZXYEx, z, x, y),
        "xformOp:rotateZYX" => Quat::from_euler(EulerRot::ZYXEx, z, y, x),
        _ => return None,
    };
    Some(q)
}

fn quat_from_value(value: &Value) -> Option<Quat> {
    match value {
        Value::Quatf(q) => Some(Quat::from_xyzw(q.x, q.y, q.z, q.w)),
        Value::Quatd(q) => Some(Quat::from_xyzw(
            q.x as f32, q.y as f32, q.z as f32, q.w as f32,
        )),
        Value::Quath(q) => Some(Quat::from_xyzw(
            q.x.to_f32(),
            q.y.to_f32(),
            q.z.to_f32(),
            q.w.to_f32(),
        )),
        _ => None,
    }
}

fn read_scalar_f32_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<f32> {
    match reader.attr_value_at(path, attr, time)? {
        Value::Float(value) => Some(value),
        Value::Double(value) => Some(value as f32),
        Value::Int(value) => Some(value as f32),
        Value::Int64(value) => Some(value as f32),
        _ => None,
    }
}

/// Compose a prim's local rotation from its authored USD rotation xform op.
pub fn local_rotation_at(reader: &dyn UsdReadObject, path: &SdfPath, time: f64) -> Option<Quat> {
    if let Some(rotation) = reader
        .attr_value_at(path, "xformOp:orient", time)
        .and_then(|value| quat_from_value(&value))
    {
        return Some(rotation);
    }
    for op in &ROTATION_OPS[1..7] {
        if let Some(value) = read_vec3_f64_at(reader, path, op, time) {
            return euler_op_to_quat(
                op,
                Vec3::new(value[0] as f32, value[1] as f32, value[2] as f32),
            );
        }
    }
    let mut rotation = Quat::IDENTITY;
    let mut found = false;
    for (op, axis) in [
        ("xformOp:rotateX", Vec3::X),
        ("xformOp:rotateY", Vec3::Y),
        ("xformOp:rotateZ", Vec3::Z),
    ] {
        if let Some(angle) = read_scalar_f32_at(reader, path, op, time) {
            rotation = Quat::from_axis_angle(axis, angle.to_radians()) * rotation;
            found = true;
        }
    }
    found.then_some(rotation)
}

/// Read a time-sampled `xformOp:transform` matrix into a Bevy transform.
pub fn read_matrix_transform_at(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Option<Transform> {
    match reader.attr_value_at(path, "xformOp:transform", time)? {
        Value::Matrix4d(matrix) => {
            let cols: [f32; 16] = std::array::from_fn(|i| matrix.0[i] as f32);
            Some(Transform::from_matrix(Mat4::from_cols_array(&cols)))
        }
        _ => None,
    }
}

fn prim_rotation_animated(reader: &impl UsdRead, path: &SdfPath) -> bool {
    ROTATION_OPS
        .iter()
        .any(|op| attr_has_time_samples(reader, path, op))
}

#[cfg(test)]
mod animation_tests {
    //! The USD animation sampler read path: `timeSamples` detection, time-aware
    //! vec3 evaluation, and per-channel "animated only" sampling (doc 19).
    use super::{
        animated_time_range, local_rotation_at, prim_has_xform_time_samples, prim_is_animated,
        read_matrix_transform_at, sample_animated_vec3,
    };
    use crate::canonical::CanonicalStage;
    use crate::read::{
        attr_has_time_samples, read_token_at, read_vec3_f64_at, stage_time_codes_per_second,
    };
    use crate::{compose_xform_order_at, local_transform_at, read_transform_from_usd};
    use bevy::prelude::{Quat, Vec3};
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_core::StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// translate is keyframed (animated); rotateXYZ has only a default (static);
    /// scale is absent.
    const SCENE: &str = r#"#usda 1.0

def Xform "Mover"
{
    double3 xformOp:translate.timeSamples = {
        0: (0, 0, 0),
        2: (20, 0, 0),
    }
    double3 xformOp:rotateXYZ = (0, 90, 0)
}

def Xform "Static"
{
    double3 xformOp:translate = (5, 0, 0)
}
"#;

    #[test]
    fn detects_animated_prims_by_xform_time_samples() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let mover = SdfPath::new("/Mover").unwrap();
        let stat = SdfPath::new("/Static").unwrap();
        assert!(prim_has_xform_time_samples(&reader, &mover));
        assert!(!prim_has_xform_time_samples(&reader, &stat));
        // Per-channel: translate animated, rotateXYZ not.
        assert!(attr_has_time_samples(&reader, &mover, "xformOp:translate"));
        assert!(!attr_has_time_samples(&reader, &mover, "xformOp:rotateXYZ"));
    }

    #[test]
    fn samples_animated_channel_and_leaves_static_untouched() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let mover = SdfPath::new("/Mover").unwrap();

        // Animated translate interpolates linearly: t=1.0 → halfway (10,0,0).
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 1.0),
            Some([10.0, 0.0, 0.0])
        );
        // On a key.
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 2.0),
            Some([20.0, 0.0, 0.0])
        );
        // Held past the last key (USD semantics).
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 99.0),
            Some([20.0, 0.0, 0.0])
        );
        // rotateXYZ has only a default → the sampler must NOT touch it (None),
        // so its instantiated pose is preserved.
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:rotateXYZ", 1.0),
            None
        );
    }

    #[test]
    fn read_vec3_f64_at_falls_back_to_default_for_static() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let stat = SdfPath::new("/Static").unwrap();
        // The raw time-aware reader returns the default at any time (value
        // resolution), even though `sample_animated_vec3` gates it out.
        assert_eq!(
            read_vec3_f64_at(&reader, &stat, "xformOp:translate", 7.0),
            Some([5.0, 0.0, 0.0])
        );
    }

    #[test]
    fn time_codes_per_second_defaults_to_24_when_unauthored() {
        // A stage that authors no `timeCodesPerSecond` reads back the USD-spec
        // fallback of 24, so the sampler's seconds→time-code map is well-defined
        // even for content that never set it.
        let __cs = parse(SCENE);
        let reader = __cs.view();
        assert_eq!(stage_time_codes_per_second(&reader), 24.0);
    }

    /// Visibility is keyframed; a second prim is fully static.
    const VIS_SCENE: &str = r#"#usda 1.0

def Xform "Blinker"
{
    token visibility.timeSamples = {
        0: "inherited",
        5: "invisible",
    }
}

def Xform "Solid"
{
    token visibility = "inherited"
    double3 xformOp:translate = (1, 2, 3)
}
"#;

    #[test]
    fn read_token_at_holds_visibility_keyframes() {
        let __cs = parse(VIS_SCENE);
        let reader = __cs.view();
        let blinker = SdfPath::new("/Blinker").unwrap();
        // On the first key.
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 0.0).as_deref(),
            Some("inherited")
        );
        // Between keys → held lower (tokens never interpolate).
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 2.0).as_deref(),
            Some("inherited")
        );
        // Past the last key → held last.
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 9.0).as_deref(),
            Some("invisible")
        );
        // A static-visibility prim has no samples → None (sampler leaves it).
        let solid = SdfPath::new("/Solid").unwrap();
        assert_eq!(read_token_at(&reader, &solid, "visibility", 1.0), None);
    }

    const ORIENT_SCENE: &str = r#"#usda 1.0

def Xform "Spinner"
{
    quatf xformOp:orient.timeSamples = {
        0: (1, 0, 0, 0),
        10: (0, 1, 0, 0),
    }
}
"#;

    #[test]
    fn orient_channel_slerps_and_is_detected() {
        let __cs = parse(ORIENT_SCENE);
        let reader = __cs.view();
        let spinner = SdfPath::new("/Spinner").unwrap();
        // The quaternion channel marks the prim animated.
        assert!(prim_has_xform_time_samples(&reader, &spinner));
        assert!(prim_is_animated(&reader, &spinner));
        // USD (w,x,y,z) = (1,0,0,0) → Bevy identity at the first key.
        let q0 = local_rotation_at(&reader, &spinner, 0.0).unwrap();
        assert!(q0.abs_diff_eq(Quat::IDENTITY, 1e-6));
        // Held past the last key → (0,1,0,0) = 180° about X.
        let q_end = local_rotation_at(&reader, &spinner, 99.0).unwrap();
        assert!(q_end.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-6));
        // Midway slerps to 90° about X (normalized) — not a component lerp.
        let q_mid = local_rotation_at(&reader, &spinner, 5.0).unwrap();
        assert!(q_mid.is_normalized());
        assert!(q_mid.abs_diff_eq(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), 1e-5));
    }

    const ROTATION_OPS_SCENE: &str = r#"#usda 1.0

(
    metersPerUnit = 1
)

def Xform "HingeZ"
{
    float xformOp:rotateZ.timeSamples = {
        0: 0.0,
        4: 90.0,
    }
    uniform token[] xformOpOrder = ["xformOp:rotateZ"]
}

def Xform "EulerZYX"
{
    float3 xformOp:rotateZYX = (0, 0, 90)
    uniform token[] xformOpOrder = ["xformOp:rotateZYX"]
}

def Xform "Matrixed"
{
    matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (3, 4, 5, 1) )
    uniform token[] xformOpOrder = ["xformOp:transform"]
}
"#;

    #[test]
    fn single_axis_rotation_is_detected_and_composed() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        let hinge = SdfPath::new("/HingeZ").unwrap();
        // A single-axis `rotateZ` time-sample marks the prim animated.
        assert!(prim_has_xform_time_samples(&reader, &hinge));
        // Held start = 0° → identity; midway (code 2) = 45° about Z.
        assert!(local_rotation_at(&reader, &hinge, 0.0)
            .unwrap()
            .abs_diff_eq(Quat::IDENTITY, 1e-6));
        let q = local_rotation_at(&reader, &hinge, 2.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-5));
    }

    #[test]
    fn euler_order_zyx_composes() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // `rotateZYX = (0,0,90)` → 90° about Z (the X and Y angles are zero).
        let q = local_rotation_at(&reader, &SdfPath::new("/EulerZYX").unwrap(), 0.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
    }

    #[test]
    fn quath_orient_decodes() {
        // Half-precision quaternion orient: USD (w,x,y,z) = (0,1,0,0) → 180° about
        // X. Proves the `quath` arm (via `f16::to_f32`) decodes.
        let scene = r#"#usda 1.0
def Xform "HalfSpin"
{
    quath xformOp:orient = (0, 1, 0, 0)
}
"#;
        let __cs = parse(scene);
        let reader = __cs.view();
        let q = local_rotation_at(&reader, &SdfPath::new("/HalfSpin").unwrap(), 0.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-3));
    }

    const ORDER_SCENE: &str = r#"#usda 1.0
(
    metersPerUnit = 1
)

def Xform "ScaleFirst"
{
    double3 xformOp:translate = (1, 0, 0)
    double3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:scale", "xformOp:translate"]
}

def Xform "TranslateFirst"
{
    double3 xformOp:translate = (1, 0, 0)
    double3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
}

def Xform "Std"
{
    double3 xformOp:translate = (5, 6, 7)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]
}
"#;

    #[test]
    fn xform_op_order_is_honored() {
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        // `["scale","translate"]`: translate is the LAST op → applied first to the
        // geometry, then `scale` (first op) scales it → translation (2,0,0).
        let sf = compose_xform_order_at(&reader, &SdfPath::new("/ScaleFirst").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(sf.translation.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
        assert!(sf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
        // `["translate","scale"]` (standard order): scale applied first, then the
        // unscaled translate → (1,0,0). Different result ⇒ op order is honored.
        let tf = compose_xform_order_at(&reader, &SdfPath::new("/TranslateFirst").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn shared_transform_reader_preserves_usd_scale() {
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        let tf =
            read_transform_from_usd(&reader, &SdfPath::new("/TranslateFirst").unwrap()).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn xform_op_order_standard_composes_as_expected() {
        // Standard-order content (`["translate","rotateXYZ"]`) composes its
        // authored translation and rotation without a parallel decoder.
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        let tf = local_transform_at(&reader, &SdfPath::new("/Std").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(5.0, 6.0, 7.0), 1e-5));
        assert!(tf
            .rotation
            .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
    }

    #[test]
    fn matrix_transform_decomposes_translation() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // Identity rotation/scale, translation in the USD matrix's last row.
        let tf =
            read_matrix_transform_at(&reader, &SdfPath::new("/Matrixed").unwrap(), 0.0).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
        // And `read_transform_from_usd` prefers the matrix.
        let full = read_transform_from_usd(&reader, &SdfPath::new("/Matrixed").unwrap()).unwrap();
        assert!(full.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
    }

    #[test]
    fn animated_time_range_spans_keys_in_seconds() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        // `/Mover` translate is keyed at codes 0 and 2; default tcps = 24, so the
        // span in seconds is [0, 2/24].
        let (lo, hi) = animated_time_range(&reader, &SdfPath::new("/Mover").unwrap()).unwrap();
        assert!(lo.abs() < 1e-9);
        assert!((hi - 2.0 / 24.0).abs() < 1e-9);
        // A static prim keyframes nothing → no range.
        assert!(animated_time_range(&reader, &SdfPath::new("/Static").unwrap()).is_none());
    }

    #[test]
    fn prim_is_animated_covers_visibility_and_xform_but_not_static() {
        let __cs = parse(VIS_SCENE);
        let reader = __cs.view();
        assert!(prim_is_animated(
            &reader,
            &SdfPath::new("/Blinker").unwrap()
        ));
        // `Solid` keyframes nothing — visibility and translate are both defaults.
        assert!(!prim_is_animated(&reader, &SdfPath::new("/Solid").unwrap()));
        // The xform-animated `Mover` from SCENE is still caught by the broader gate.
        let __mover = parse(SCENE);
        let mover_reader = __mover.view();
        assert!(prim_is_animated(
            &mover_reader,
            &SdfPath::new("/Mover").unwrap()
        ));
        assert!(!prim_is_animated(
            &mover_reader,
            &SdfPath::new("/Static").unwrap()
        ));
    }
}
