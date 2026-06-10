//! Focused validation of the openusd 0.2 → 0.5 migration.
//!
//! Composes the REAL sandbox scene + rover assets through the new
//! `compose_native_fs` path and asserts the migration-critical properties at
//! their correct composed paths:
//!   * reference composition (referenced rover geometry appears),
//!   * `over` opinion composition (per-instance colour override),
//!   * relationship targets survive the flatten,
//!   * `apiSchemas` compose across the reference,
//!   * binary-asset (glTF) `lunco:resolvedAsset` synthesis.
//!
//! Pure-reader (no Bevy `App`), so it is immune to the `init_asset::<Scene>()`
//! harness gap that the older entity-spawning tests hit.

use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use openusd::usda::TextReader;
use std::path::PathBuf;

fn assets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
}

fn compose(rel: &str) -> TextReader {
    let path = assets_root().join(rel);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    lunco_usd_bevy::compose_native_fs(&raw, path.parent().unwrap())
        .unwrap_or_else(|| panic!("compose failed for {path:?}"))
}

fn field<'a>(reader: &'a TextReader, path_str: &str, field: &str) -> Option<Value> {
    let p = SdfPath::new(path_str).ok()?;
    reader.try_get(&p, field).ok().flatten().map(|c| c.into_owned())
}

fn first_rel_target(reader: &TextReader, prop_path: &str) -> Option<String> {
    match field(reader, prop_path, "targetPaths")? {
        Value::PathListOp(op) => op
            .explicit_items
            .first()
            .or_else(|| op.prepended_items.first())
            .or_else(|| op.added_items.first())
            .map(|p| p.as_str().to_string()),
        _ => None,
    }
}

/// Reference composition: a referenced rover surfaces its Chassis + wheels.
#[test]
fn reference_geometry_composes() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    for p in [
        "/SandboxScene/Skid_Raycast_1",
        "/SandboxScene/Skid_Raycast_1/Chassis",
        "/SandboxScene/Skid_Raycast_1/Wheel_FL",
        "/SandboxScene/Skid_Raycast_1/Wheel_RR",
    ] {
        assert!(r.has_spec(&SdfPath::new(p).unwrap()), "missing composed prim {p}");
    }
}

/// `over` opinion composition: the scene's `over "Chassis" { displayColor }`
/// must win over the referenced base colour. Authored on the CHILD Chassis.
#[test]
fn over_color_override_composes() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let c = r
        .prim_attribute_value::<[f32; 3]>(
            &SdfPath::new("/SandboxScene/Skid_Raycast_1/Chassis").unwrap(),
            "primvars:displayColor",
        )
        .expect("Skid_Raycast_1/Chassis must have composed displayColor");
    assert!((c[0] - 0.8).abs() < 0.01 && (c[1] - 0.2).abs() < 0.01 && (c[2] - 0.2).abs() < 0.01,
        "override colour must be (0.8,0.2,0.2), got {c:?}");
}

/// apiSchemas compose across the reference: the physical skid rover carries the
/// vehicle + articulation schemas.
#[test]
fn api_schemas_compose() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let ok = lunco_usd_bevy::has_api_schema(
        &r,
        &SdfPath::new("/SandboxScene/Skid_Physical_1").unwrap(),
        "PhysxVehicleDriveSkidAPI",
    );
    assert!(ok, "Skid_Physical_1 must compose PhysxVehicleDriveSkidAPI");
}

/// Relationship targets survive the flatten (the physical rover's wheel hinge
/// points at its wheel body).
#[test]
fn joint_relationship_targets_survive() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let target = first_rel_target(&r, "/SandboxScene/Skid_Physical_1/Wheel_FL_Hinge.physics:body1")
        .expect("Wheel_FL_Hinge must have a physics:body1 target");
    assert_eq!(target, "/SandboxScene/Skid_Physical_1/Wheel_FL", "joint body1 target");
}

/// Standalone rover composition (as the Bevy-pipeline tests do): the root's
/// `prim_children` (what `instantiate_usd_prim` iterates) must include the
/// Chassis + 4 wheels, apiSchemas must compose on the root, and the wheel/
/// chassis physics attributes the avian/sim consumers read must be present.
#[test]
fn standalone_rover_reader_is_complete() {
    let r = compose("vessels/rovers/skid_rover.usda");

    // Root children — the exact list `prim_children` returns.
    let kids = r.prim_children(&SdfPath::new("/SkidRover").unwrap());
    let names: Vec<String> = kids.iter().filter_map(|p| p.name().map(str::to_string)).collect();
    for w in ["Chassis", "Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
        assert!(names.iter().any(|n| n == w), "SkidRover children missing {w}; got {names:?}");
    }

    // Vehicle-type detection.
    assert!(
        lunco_usd_bevy::has_api_schema(&r, &SdfPath::new("/SkidRover").unwrap(), "PhysxVehicleDriveSkidAPI"),
        "SkidRover must compose PhysxVehicleDriveSkidAPI"
    );

    // Wheel parameters the sim reads. Native USD types are preserved by the
    // adapter, so `double` reads as f64 and `float` as f32 (the sim reads each
    // at its authored type — see lunco_usd_sim::setup_*_wheel).
    let fl = SdfPath::new("/SkidRover/Wheel_FL").unwrap();
    assert_eq!(
        r.prim_attribute_value::<f64>(&fl, "radius"),
        Some(0.4),
        "Wheel_FL `double radius` must compose"
    );
    assert_eq!(
        r.prim_attribute_value::<f32>(&fl, "physxVehicleSuspension:springStiffness"),
        Some(15000.0),
        "Wheel_FL `float springStiffness` must compose"
    );
}

/// Drivetrain variantSet — `physical` selection: the joints, motor, and
/// articulation root now live in the ROVER ASSET's `physical` variant, not in
/// the scene. Selecting `drivetrain="physical"` on the instance must bring the
/// per-wheel `PhysicsRevoluteJoint`s in with their asset-local rel-targets
/// path-translated into the instance namespace.
#[test]
fn drivetrain_physical_variant_brings_joints() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    for (rover, _drive) in [
        ("Skid_Physical_1", "PhysxVehicleDriveSkidAPI"),
        ("Ackermann_Physical_1", "PhysxVehicleDrive4WAPI"),
    ] {
        for w in ["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let hinge = format!("/SandboxScene/{rover}/{w}_Hinge");
            assert!(
                r.has_spec(&SdfPath::new(&hinge).unwrap()),
                "{hinge} must compose from the physical variant"
            );
            let target = first_rel_target(&r, &format!("{hinge}.physics:body1"))
                .unwrap_or_else(|| panic!("{hinge} missing physics:body1 target"));
            assert_eq!(
                target,
                format!("/SandboxScene/{rover}/{w}"),
                "asset-local body1 must translate into the instance namespace"
            );
        }
    }
}

/// `physical` variant drops the wheels below the chassis (y = -0.65), while a
/// `raycast` instance keeps them at ride height (y = -0.15). Proves the variant
/// `over` opinions compose (or don't) per selection.
#[test]
fn drivetrain_variant_sets_wheel_height() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let y = |path: &str| -> f64 {
        r.prim_attribute_value::<[f64; 3]>(&SdfPath::new(path).unwrap(), "xformOp:translate")
            .unwrap_or_else(|| panic!("{path} missing xformOp:translate"))[1]
    };
    assert!(
        (y("/SandboxScene/Skid_Physical_1/Wheel_FL") - (-0.65)).abs() < 1e-6,
        "physical variant must drop the wheel to y=-0.65"
    );
    assert!(
        (y("/SandboxScene/Skid_Raycast_1/Wheel_FL") - (-0.15)).abs() < 1e-6,
        "raycast (fallback) variant keeps the wheel at y=-0.15"
    );
}

/// apiSchemas compose from TWO sources at once: the base rover's vehicle drive
/// (via the reference) and the `physical` variant's `PhysicsArticulationRootAPI`
/// — neither is re-listed on the scene instance anymore.
#[test]
fn drivetrain_physical_composes_articulation_and_drive() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let skid = SdfPath::new("/SandboxScene/Skid_Physical_1").unwrap();
    assert!(
        lunco_usd_bevy::has_api_schema(&r, &skid, "PhysicsArticulationRootAPI"),
        "ArticulationRootAPI must compose from the physical variant"
    );
    assert!(
        lunco_usd_bevy::has_api_schema(&r, &skid, "PhysxVehicleDriveSkidAPI"),
        "DriveSkidAPI must compose from the base rover across the reference"
    );
}

/// A `raycast` instance must NOT carry joints — the fallback variant is empty,
/// so the joint prims authored only under `physical` are absent.
#[test]
fn drivetrain_raycast_has_no_joints() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    assert!(
        !r.has_spec(&SdfPath::new("/SandboxScene/Skid_Raycast_1/Wheel_FL_Hinge").unwrap()),
        "raycast instance must not have joint prims"
    );
}

/// Binary-asset shim: the Perseverance glTF payload surfaces as a
/// `lunco:resolvedAsset` URI on its Visual prim.
#[test]
fn gltf_resolved_asset_synthesized() {
    let r = compose("scenes/sandbox/sandbox_scene.usda");
    let visual = SdfPath::new("/SandboxScene/Perseverance/Visual").unwrap();
    let attr = visual.append_property("lunco:resolvedAsset").unwrap();
    let uri = match r.try_get(&attr, "default").ok().flatten().map(|c| c.into_owned()) {
        Some(Value::AssetPath(a)) => a.as_str().to_string(),
        Some(Value::String(s)) | Some(Value::Token(s)) => s,
        other => panic!("Perseverance/Visual missing lunco:resolvedAsset, got {other:?}"),
    };
    assert!(uri.contains("perseverance.glb"), "resolved URI should be the glb, got {uri}");
}
