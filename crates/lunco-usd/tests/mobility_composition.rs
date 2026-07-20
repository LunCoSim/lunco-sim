//! A wheel gets its spring from a suspension component and its grip from a tire, and
//! both arrive by composition.
//!
//! The numbers a rover drives on are not authored on its wheels — they come from
//! `components/mobility/suspensions/*.usda` and `components/mobility/tires/*.usda`,
//! which compose onto each wheel prim through a reference arc. That is the whole point
//! (retune the ride in one file; re-shoe a rover with one variant), and it is also the
//! whole risk: point a wheel at the wrong arc and nothing complains — the rover just
//! drives differently. So the composed values are asserted here, per rover.

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

fn compose(rel: &str) -> CanonicalStage {
    let path = assets_root().join(rel);
    let stage = lunco_usd_bevy::compose_file_to_stage(&path)
        .unwrap_or_else(|e| panic!("compose failed for {path:?}: {e}"));
    CanonicalStage::from_stage(stage, path.to_string_lossy().to_string())
}

/// The spring a wheel actually rides on, as composed — authored `float` per
/// `PhysxVehicleSuspensionAPI`.
fn spring(view: &StageView<'_>, wheel: &str) -> (f32, f32, f32) {
    let p = SdfPath::new(wheel).unwrap_or_else(|_| panic!("bad path {wheel}"));
    let get = |name: &str| -> f32 {
        view.real_f32(&p, name)
            .unwrap_or_else(|| panic!("{wheel} has no {name} — its suspension arc is missing"))
    };
    (
        get("lunco:suspension:restLength"),
        get("physxVehicleSuspension:springStrength"),
        get("physxVehicleSuspension:springDamperRate"),
    )
}

#[test]
fn each_rover_composes_the_suspension_it_asked_for() {
    // rest, stiffness, damping — per components/mobility/suspensions/*.usda
    const STANDARD: (f32, f32, f32) = (0.7, 15000.0, 3000.0);
    const ROCKER: (f32, f32, f32) = (0.5, 12000.0, 2500.0);
    const RIGID: (f32, f32, f32) = (0.0, 15000.0, 5000.0);

    for (asset, wheel, want) in [
        (
            "vessels/rovers/skid_rover.usda",
            "/SkidRover/Wheel_FL",
            STANDARD,
        ),
        (
            "vessels/rovers/ackermann_rover.usda",
            "/AckermannRover/Wheel_FL",
            STANDARD,
        ),
        (
            "vessels/rovers/six_wheel_rover.usda",
            "/SixWheelRover/Wheel_L0",
            STANDARD,
        ),
        (
            "vessels/rovers/six_wheel_independent.usda",
            "/SixWheelIndependent/Wheel_L0",
            STANDARD,
        ),
        (
            "vessels/rovers/rocker_bogie.usda",
            "/RockerBogie/RockerL/Wheel_FL",
            ROCKER,
        ),
        (
            "vessels/rovers/rucheyok/rucheyok.usda",
            "/Rucheyok/Wheel_FL",
            RIGID,
        ),
    ] {
        let cs = compose(asset);
        let view = cs.view();
        assert_eq!(
            spring(&view, wheel),
            want,
            "{asset}: {wheel} composed the wrong suspension"
        );
    }
}

/// The APIs arrive by composition too — and the loader spawns nothing without them.
///
/// `apiSchemas` is applied ONCE per component (`wheel.usda`'s `Wheel`, each
/// `suspensions/*.usda`'s `Suspension`) and reaches all 30 wheels through the
/// reference arcs, because `apiSchemas` is a list-op and composes. A rover authors
/// values, never schemas.
///
/// This is load-bearing, not decorative: `process_usd_sim_prim_read` detects a wheel
/// by `has_api_schema("PhysxVehicleWheelAPI")`, so an arc that stopped delivering the
/// API would mean a rover with no wheels at all — and the composed *values* asserted
/// above would still be perfectly correct.
#[test]
fn every_rover_wheel_composes_its_applied_schemas() {
    for (asset, wheel) in [
        ("vessels/rovers/skid_rover.usda", "/SkidRover/Wheel_FL"),
        ("vessels/rovers/ackermann_rover.usda", "/AckermannRover/Wheel_FL"),
        ("vessels/rovers/six_wheel_rover.usda", "/SixWheelRover/Wheel_L0"),
        ("vessels/rovers/six_wheel_independent.usda", "/SixWheelIndependent/Wheel_L0"),
        ("vessels/rovers/rocker_bogie.usda", "/RockerBogie/RockerL/Wheel_FL"),
        ("vessels/rovers/rucheyok/rucheyok.usda", "/Rucheyok/Wheel_FL"),
    ] {
        let cs = compose(asset);
        let view = cs.view();
        let p = SdfPath::new(wheel).unwrap();
        for api in [
            // from wheel.usda</Wheel>
            "PhysxVehicleWheelAPI",
            "LunCoWheelAPI",
            // from suspensions/*.usda</Suspension>
            "PhysxVehicleSuspensionAPI",
            "LunCoSuspensionAPI",
        ] {
            assert!(
                view.has_api_schema(&p, api),
                "{asset}: {wheel} must compose {api} — without it the loader does \
                 not see a wheel here at all"
            );
        }
    }
}

/// A strut's moving visuals declare themselves; the casing does not.
///
/// The piston and spring apply `LunCoSuspensionVisualAPI` and carry a role token;
/// the casing never moves, so it applies nothing and stays plain geometry. The
/// loader gates on the API, so a role token without the API animates nothing.
#[test]
fn suspension_visuals_declare_their_role() {
    let cs = compose("vessels/rovers/skid_rover.usda");
    let view = cs.view();

    for (prim, role) in [
        ("/SkidRover/Wheel_FL/SuspensionPiston", "piston"),
        ("/SkidRover/Wheel_FL/SuspensionSpring", "spring"),
    ] {
        let p = SdfPath::new(prim).unwrap();
        assert!(
            view.has_api_schema(&p, "LunCoSuspensionVisualAPI"),
            "{prim} must apply LunCoSuspensionVisualAPI"
        );
        assert_eq!(
            view.text(&p, "lunco:suspensionVisual:role").as_deref(),
            Some(role),
            "{prim} must declare role {role:?}"
        );
    }

    let casing = SdfPath::new("/SkidRover/Wheel_FL/SuspensionCasing").unwrap();
    assert!(
        !view.has_api_schema(&casing, "LunCoSuspensionVisualAPI"),
        "the casing does not move — it must not claim a visual role"
    );
}

/// A wheel's grip and tread come from its `tire` variant, not from the hub.
#[test]
fn a_wheel_composes_its_tire() {
    let cs = compose("vessels/rovers/skid_rover.usda");
    let view = cs.view();
    let fl = SdfPath::new("/SkidRover/Wheel_FL").unwrap();

    // Grip — `regolith` is the wheel's default tire.
    assert_eq!(
        view.real(&fl, "lunco:tire:frictionCoefficient"),
        Some(0.8),
        "Wheel_FL must compose its tire's friction"
    );
    assert_eq!(
        view.real_f32(&fl, "physxVehicleTire:longitudinalStiffness"),
        Some(8000.0),
        "Wheel_FL must compose its tire's contact stiffness"
    );

    // Tread — the look the tire brings, bound the way USD binds a shader: the wheel
    // gets a `material:binding`, the `Material` names the `Shader` its surface comes
    // from, and that `Shader`'s WGSL source is the tread. The Material is authored as a
    // child of the tire's `over`, so the reference arc path-translates the whole chain
    // onto the wheel — binding included.
    // Resolved through the PRODUCTION resolver, not a hand-rolled walk of the two
    // hops: a test that re-implements the traversal only proves the test's own
    // traversal works, and would keep passing if the loader's diverged.
    let shader = lunco_usd_bevy::resolve_bound_shader(&view, &fl)
        .expect("Wheel_FL must compose its tire's material binding through to its Shader");
    assert_eq!(
        view.asset(&shader, "info:wgsl:sourceAsset").as_deref(),
        Some("lunco://shaders/wheel.wgsl"),
        "Wheel_FL must compose its tire's tread shader, with the shipped-asset scheme intact"
    );
}

/// P1 pin for the UNIFIED wheel parameter model: every wheel of every rover
/// satisfies the ONE strict reader both wheel kinds spawn from
/// (`lunco_usd_sim::wheel_params::WheelParams::read`). This is the
/// asset-contract half of the strictness bargain — the loader refuses a wheel
/// missing any required attr, so the library must compose the complete set
/// onto every wheel, and a broken arc fails HERE naming the exact attrs
/// instead of as a silent no-spawn in the app.
#[test]
fn every_rover_wheel_satisfies_the_unified_param_reader() {
    let rovers = [
        "vessels/rovers/skid_rover.usda",
        "vessels/rovers/ackermann_rover.usda",
        "vessels/rovers/six_wheel_rover.usda",
        "vessels/rovers/six_wheel_independent.usda",
        "vessels/rovers/rocker_bogie.usda",
        "vessels/rovers/rucheyok/rucheyok.usda",
    ];
    for rover in rovers {
        let stage = compose(rover);
        let view = stage.view();
        let mut wheels = 0;
        for p in view.prim_paths() {
            if !view.has_api_schema(&p, "PhysxVehicleWheelAPI") {
                continue;
            }
            wheels += 1;
            let params = lunco_usd_sim::wheel_params::WheelParams::read(&view, &p, None, None)
                .unwrap_or_else(|missing| {
                    panic!("{rover}: wheel {} is missing {:?}", p.as_str(), missing)
                });
            assert!(params.radius > 0.0, "{rover}: {} radius", p.as_str());
            assert!(params.mass > 0.0, "{rover}: {} mass", p.as_str());
            assert!(params.peak_torque > 0.0, "{rover}: {} peakTorque", p.as_str());
            assert!(params.friction_mu > 0.0, "{rover}: {} tire μ", p.as_str());
            // Default-variant rovers are raycast: no wheel functions without
            // composed suspension compliance.
            assert!(
                params.suspension.is_some(),
                "{rover}: {} composes no suspension — raycast spawn would refuse it",
                p.as_str()
            );
        }
        assert!(wheels > 0, "{rover}: no PhysxVehicleWheelAPI wheels composed");
    }
}

/// The live-resync CLAIM must be prim-scoped: `physics:mass` on a WHEEL routes
/// to the in-place wheel resync, while the same attr on the CHASSIS keeps the
/// generic refresh path (avian mass overrides rebuild there). Wheel-only
/// namespaces are claimed anywhere they appear.
#[test]
fn wheel_resync_claims_are_prim_scoped() {
    use lunco_usd_sim::wheel_params::claims_edit;
    let stage = compose("vessels/rovers/skid_rover.usda");
    let view = stage.view();
    let wheel = SdfPath::new("/SkidRover/Wheel_FL").unwrap();
    let chassis = SdfPath::new("/SkidRover/Chassis").unwrap();
    let root = SdfPath::new("/SkidRover").unwrap();

    assert!(claims_edit(&view, &wheel, "physics:mass"));
    assert!(!claims_edit(&view, &chassis, "physics:mass"));
    assert!(claims_edit(&view, &wheel, "lunco:wheel:driveDamping"));
    assert!(claims_edit(&view, &wheel, "physxVehicleEngine:maxRotationSpeed"));
    assert!(claims_edit(&view, &wheel, "physxVehicleEngine:peakTorque"));
    assert!(claims_edit(&view, &wheel, "physxVehicleSuspension:springStrength"));
    assert!(claims_edit(&view, &root, "lunco:driveKernel"));
    assert!(claims_edit(&view, &root, "physxVehicleAckermannSteering:maxSteerAngle"));
    assert!(!claims_edit(&view, &chassis, "primvars:displayColor"));
    assert!(!claims_edit(&view, &root, "lunco:spawnable"));
}
