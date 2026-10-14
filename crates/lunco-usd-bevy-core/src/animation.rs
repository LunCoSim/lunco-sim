//! USD animation channel topology.
//!
//! This module identifies authored USD channels carrying time samples.
//! Transform evaluation belongs to the native composed-stage reader shared
//! by initial projection and the production animation adapter.

use openusd::sdf::Path as SdfPath;

use lunco_usd_bevy_stage::UsdRead;
use lunco_usd_bevy_stage::read::attr_has_time_samples;

/// The bound-shader inputs the USD animation adapter can sample.
pub const ANIMATED_SHADER_INPUTS: [&str; 2] = ["inputs:diffuseColor", "inputs:opacity"];

/// True iff a prim carries any channel consumed by the USD animation adapter.
pub fn prim_is_animated<R: UsdRead>(reader: &R, path: &SdfPath) -> Result<bool, String> {
    if reader
        .transform_might_be_time_varying(path)
        .map_err(|error| error.to_string())?
        || attr_has_time_samples(reader, path, "visibility")
        || attr_has_time_samples(reader, path, "primvars:displayColor")
    {
        return Ok(true);
    }
    Ok(reader.bound_shader(path)?.is_some_and(|shader| {
        ANIMATED_SHADER_INPUTS
            .iter()
            .any(|input| attr_has_time_samples(reader, &shader, input))
    }))
}

#[cfg(test)]
mod animation_tests {
    //! Native composed-stage transform and channel topology seams.
    use super::prim_is_animated;
    use bevy::prelude::{Quat, Vec3};
    use lunco_usd_bevy_stage::UsdRead;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_bevy_stage::read::{
        attr_has_time_samples, read_token_at, read_vec3_f64_at, stage_time_codes_per_second,
    };
    use lunco_usd_bevy_stage::{local_transform_at, read_transform_from_usd};
    use openusd::sdf::Path as SdfPath;

    /// Build the native stage used by the production reader.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_compose::recipe::StageRecipe::from_source(
            "t.usda", usda,
        ))
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
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]
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
        assert!(reader.transform_might_be_time_varying(&mover).unwrap());
        assert!(!reader.transform_might_be_time_varying(&stat).unwrap());
        // Per-channel: translate animated, rotateXYZ not.
        assert!(attr_has_time_samples(&reader, &mover, "xformOp:translate"));
        assert!(!attr_has_time_samples(&reader, &mover, "xformOp:rotateXYZ"));
    }

    #[test]
    fn native_time_read_resolves_static_default() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let stat = SdfPath::new("/Static").unwrap();
        // Native value resolution returns the static opinion at any time.
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
    uniform token[] xformOpOrder = ["xformOp:orient"]
}
"#;

    #[test]
    fn orient_channel_slerps_and_is_detected() {
        let __cs = parse(ORIENT_SCENE);
        let reader = __cs.view();
        let spinner = SdfPath::new("/Spinner").unwrap();
        // The quaternion channel marks the prim animated.
        assert!(reader.transform_might_be_time_varying(&spinner).unwrap());
        assert!(prim_is_animated(&reader, &spinner).unwrap());
        // USD (w,x,y,z) = (1,0,0,0) → Bevy identity at the first key.
        let q0 = local_transform_at(&reader, &spinner, 0.0)
            .unwrap()
            .unwrap()
            .rotation;
        assert!(q0.abs_diff_eq(Quat::IDENTITY, 1e-6));
        // Held past the last key → (0,1,0,0) = 180° about X.
        let q_end = local_transform_at(&reader, &spinner, 99.0)
            .unwrap()
            .unwrap()
            .rotation;
        assert!(q_end.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-6));
        // Midway slerps to 90° about X (normalized) — not a component lerp.
        let q_mid = local_transform_at(&reader, &spinner, 5.0)
            .unwrap()
            .unwrap()
            .rotation;
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
        assert!(reader.transform_might_be_time_varying(&hinge).unwrap());
        // Held start = 0° → identity; midway (code 2) = 45° about Z.
        assert!(
            local_transform_at(&reader, &hinge, 0.0)
                .unwrap()
                .unwrap()
                .rotation
                .abs_diff_eq(Quat::IDENTITY, 1e-6)
        );
        let q = local_transform_at(&reader, &hinge, 2.0)
            .unwrap()
            .unwrap()
            .rotation;
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-5));
    }

    #[test]
    fn euler_order_zyx_composes() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // `rotateZYX = (0,0,90)` → 90° about Z (the X and Y angles are zero).
        let q = local_transform_at(&reader, &SdfPath::new("/EulerZYX").unwrap(), 0.0)
            .unwrap()
            .unwrap()
            .rotation;
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
    }

    #[test]
    fn quath_orient_decodes() {
        // Half-precision quaternion orient: USD (w,x,y,z) = (0,1,0,0) → 180° about
        // X. Native Xformable evaluates the ordered half-precision opinion.
        let scene = r#"#usda 1.0
def Xform "HalfSpin"
{
    quath xformOp:orient = (0, 1, 0, 0)
    uniform token[] xformOpOrder = ["xformOp:orient"]
}
"#;
        let __cs = parse(scene);
        let reader = __cs.view();
        let q = local_transform_at(&reader, &SdfPath::new("/HalfSpin").unwrap(), 0.0)
            .unwrap()
            .unwrap()
            .rotation;
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
        let sf = local_transform_at(&reader, &SdfPath::new("/ScaleFirst").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(sf.translation.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
        assert!(sf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
        // `["translate","scale"]` (standard order): scale applied first, then the
        // unscaled translate → (1,0,0). Different result ⇒ op order is honored.
        let tf = local_transform_at(&reader, &SdfPath::new("/TranslateFirst").unwrap(), 0.0)
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
        assert!(
            tf.rotation
                .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5)
        );
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
    }

    #[test]
    fn matrix_transform_decomposes_translation() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // Identity rotation/scale, translation in the USD matrix's last row.
        let tf = local_transform_at(&reader, &SdfPath::new("/Matrixed").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
        // Default-time projection uses the same ordered native matrix.
        let full = read_transform_from_usd(&reader, &SdfPath::new("/Matrixed").unwrap()).unwrap();
        assert!(full.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
    }

    #[test]
    fn prim_is_animated_covers_visibility_and_xform_but_not_static() {
        let __cs = parse(VIS_SCENE);
        let reader = __cs.view();
        assert!(prim_is_animated(&reader, &SdfPath::new("/Blinker").unwrap()).unwrap());
        // `Solid` keyframes nothing — visibility and translate are both defaults.
        assert!(!prim_is_animated(&reader, &SdfPath::new("/Solid").unwrap()).unwrap());
        // The xform-animated `Mover` from SCENE is still caught by the broader gate.
        let __mover = parse(SCENE);
        let mover_reader = __mover.view();
        assert!(prim_is_animated(&mover_reader, &SdfPath::new("/Mover").unwrap()).unwrap());
        assert!(!prim_is_animated(&mover_reader, &SdfPath::new("/Static").unwrap()).unwrap());
    }
}
