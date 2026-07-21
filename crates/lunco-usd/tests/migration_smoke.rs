//! Focused validation of the openusd 0.2 → 0.5 migration.
//!
//! Composes the REAL sandbox scene + rover assets through the live
//! `compose_file_to_stage` path (read via a `StageView` over a `CanonicalStage`)
//! and asserts the migration-critical properties at their correct composed paths:
//!   * reference composition (referenced rover geometry appears),
//!   * `over` opinion composition (per-instance colour override),
//!   * relationship targets survive composition,
//!   * `apiSchemas` compose across the reference,
//!   * binary-asset (glTF) `lunco:resolvedAsset` synthesis.
//!
//! Pure-reader (no Bevy `App`), so it is immune to the `init_asset::<Scene>()`
//! harness gap that the older entity-spawning tests hit.

use lunco_usd_bevy::{CanonicalStage, StageView, UsdRead};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

fn assets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
}

/// Compose an asset into a live [`CanonicalStage`] — carries the precomputed
/// binary-arc sites so `resolved_asset` synthesizes glTF URIs off the live stage.
fn compose(rel: &str) -> CanonicalStage {
    let path = assets_root().join(rel);
    let stage = lunco_usd_bevy::compose_file_to_stage(&path)
        .unwrap_or_else(|e| panic!("compose failed for {path:?}: {e}"));
    CanonicalStage::from_stage(stage, path.to_string_lossy().to_string())
}

/// First composed, path-translated target of the relationship at `prop_path`
/// (e.g. `/…/Wheel_FL_Hinge.physics:body1`), as a path string.
fn first_rel_target(view: &StageView<'_>, prop_path: &str) -> Option<String> {
    let (prim_s, name) = prop_path.rsplit_once('.')?;
    let prim = SdfPath::new(prim_s).ok()?;
    view.rel_target(&prim, name).map(|p| p.as_str().to_string())
}

/// Reference composition: a referenced rover surfaces its Chassis + wheels.
#[test]
fn reference_geometry_composes() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    for p in [
        "/SandboxScene/Skid_Raycast_1",
        "/SandboxScene/Skid_Raycast_1/Chassis",
        "/SandboxScene/Skid_Raycast_1/Wheel_FL",
        "/SandboxScene/Skid_Raycast_1/Wheel_RR",
    ] {
        assert!(view.has_prim(&SdfPath::new(p).unwrap()), "missing composed prim {p}");
    }
}

/// `over` opinion composition: the scene's `over "Chassis" { displayColor }`
/// must win over the referenced base colour. Authored on the CHILD Chassis.
#[test]
fn over_color_override_composes() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    // `primvars:displayColor` is ARRAY-valued (`color3f[]`) per UsdGeomGprim.
    let c = lunco_usd_bevy::read_primvar_vec3(
        &view,
        &SdfPath::new("/SandboxScene/Skid_Raycast_1/Chassis").unwrap(),
        "primvars:displayColor",
    )
    .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
    .expect("Skid_Raycast_1/Chassis must have composed displayColor");
    // The scene's `over "Chassis"` authors crimson; the referenced base is
    // (0.25, 0.22, 0.18). Matching the override — well clear of the base — is what
    // proves the opinion composed and won.
    assert!((c[0] - 0.85).abs() < 0.01 && (c[1] - 0.15).abs() < 0.01 && (c[2] - 0.12).abs() < 0.01,
        "override colour must be (0.85,0.15,0.12), got {c:?}");
}

/// apiSchemas compose across the reference: the physical skid rover carries the
/// vehicle + articulation schemas.
#[test]
fn api_schemas_compose() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    let ok = view.has_api_schema(
        &SdfPath::new("/SandboxScene/Skid_Physical_1").unwrap(),
        "PhysxVehicleTankDifferentialAPI",
    );
    assert!(ok, "Skid_Physical_1 must compose PhysxVehicleTankDifferentialAPI");
}

/// Relationship targets survive composition (the physical rover's wheel hinge
/// points at its wheel body).
#[test]
fn joint_relationship_targets_survive() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    let target = first_rel_target(&view, "/SandboxScene/Skid_Physical_1/Wheel_FL_Hinge.physics:body1")
        .expect("Wheel_FL_Hinge must have a physics:body1 target");
    assert_eq!(target, "/SandboxScene/Skid_Physical_1/Wheel_FL", "joint body1 target");
}

/// Standalone rover composition (as the Bevy-pipeline tests do): the root's
/// `children` (what `instantiate_usd_prim` iterates) must include the
/// Chassis + 4 wheels, apiSchemas must compose on the root, and the wheel/
/// chassis physics attributes the avian/sim consumers read must be present.
#[test]
fn standalone_rover_reader_is_complete() {
    let cs = compose("vessels/rovers/skid_rover.usda");
    let view = cs.view();

    // Root children — the exact list `children` returns, which is what
    // `instantiate_usd_prim` iterates in production.
    let kids = view.children(&SdfPath::new("/SkidRover").unwrap());
    let names: Vec<String> = kids.iter().filter_map(|p| p.name().map(str::to_string)).collect();
    for w in ["Chassis", "Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
        assert!(names.iter().any(|n| n == w), "SkidRover children missing {w}; got {names:?}");
    }

    // Vehicle-type detection.
    assert!(
        view.has_api_schema(&SdfPath::new("/SkidRover").unwrap(), "PhysxVehicleTankDifferentialAPI"),
        "SkidRover must compose PhysxVehicleTankDifferentialAPI"
    );

    // Wheel parameters the sim reads. Native USD types are preserved by the
    // adapter, so `double` reads as f64 and `float` as f32 (the sim reads each
    // at its authored type — see lunco_usd_sim::setup_*_wheel).
    let fl = SdfPath::new("/SkidRover/Wheel_FL").unwrap();
    assert_eq!(
        view.value::<f64>(&fl, "radius"),
        Some(0.4),
        "Wheel_FL `double radius` must compose"
    );
    assert_eq!(
        view.value::<f32>(&fl, "physxVehicleSuspension:springStrength"),
        Some(15000.0),
        "Wheel_FL `float springStrength` must compose"
    );
}

/// Drivetrain variantSet — `physical` selection: the joints, motor, and
/// articulation root now live in the ROVER ASSET's `physical` variant, not in
/// the scene. Selecting `drivetrain="physical"` on the instance must bring the
/// per-wheel `PhysicsRevoluteJoint`s in with their asset-local rel-targets
/// path-translated into the instance namespace.
#[test]
fn drivetrain_physical_variant_brings_joints() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    for (rover, _drive) in [
        ("Skid_Physical_1", "PhysxVehicleTankDifferentialAPI"),
        ("Ackermann_Physical_1", "PhysxVehicleAckermannSteeringAPI"),
    ] {
        for w in ["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let hinge = format!("/SandboxScene/{rover}/{w}_Hinge");
            assert!(
                view.has_prim(&SdfPath::new(&hinge).unwrap()),
                "{hinge} must compose from the physical variant"
            );
            let target = first_rel_target(&view, &format!("{hinge}.physics:body1"))
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
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    let y = |path: &str| -> f64 {
        view.value::<[f64; 3]>(&SdfPath::new(path).unwrap(), "xformOp:translate")
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
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    let skid = SdfPath::new("/SandboxScene/Skid_Physical_1").unwrap();
    assert!(
        view.has_api_schema(&skid, "PhysicsArticulationRootAPI"),
        "ArticulationRootAPI must compose from the physical variant"
    );
    assert!(
        view.has_api_schema(&skid, "PhysxVehicleTankDifferentialAPI"),
        "DriveSkidAPI must compose from the base rover across the reference"
    );
}

/// A `raycast` instance must NOT carry joints — the fallback variant is empty,
/// so the joint prims authored only under `physical` are absent.
#[test]
fn drivetrain_raycast_has_no_joints() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    assert!(
        !view.has_prim(&SdfPath::new("/SandboxScene/Skid_Raycast_1/Wheel_FL_Hinge").unwrap()),
        "raycast instance must not have joint prims"
    );
}

/// Binary-asset shim: the Perseverance glTF payload surfaces as a
/// `lunco:resolvedAsset` URI on its Visual prim.
#[test]
fn gltf_resolved_asset_synthesized() {
    let cs = compose("scenes/sandbox/sandbox_scene.usda");
    let view = cs.view();
    let visual = SdfPath::new("/SandboxScene/Perseverance/Visual").unwrap();
    let uri = view
        .resolved_asset(&visual)
        .expect("Perseverance/Visual missing lunco:resolvedAsset");
    assert!(uri.contains("perseverance.glb"), "resolved URI should be the glb, got {uri}");
}
