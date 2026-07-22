//! A ROVER IS THE SAME VEHICLE WHICHEVER WAY ITS WHEELS ARE REALIZED.
//!
//! `drivetrain = raycast | physical` chooses how a wheel is *simulated* — a
//! suspension raycast with an analytic tire, or a rigid body on a revolute joint.
//! It is not licence to change the vehicle. Mass, inertia, damping, wheelbase,
//! track, wheel radius, tire grip and the motor's torque/speed curve are
//! properties of the ROVER, and a variant that quietly alters one of them makes
//! `drivetrain_parity` unwinnable for reasons no amount of tire tuning can reach.
//!
//! This happened: the raycast rover massed 1000 kg and its physical twin 1100 kg,
//! because a raycast wheel is a kinematic proxy whose authored `physics:mass` avian
//! never weighed. Terminal speed goes as `F/(c·m)` under `physxRigidBody:linearDamping`,
//! so a 10% mass error is a 10% speed error that looks exactly like a tire defect.
//! `lunco_mobility::fold_proxy_wheel_mass` now folds the proxy wheels onto the
//! chassis; these tests pin the AUTHORED side of the same contract.
//!
//! The headline test does not hand-list the properties to compare. It composes the
//! rover twice and DIFFS every attribute of every prim the two compositions share,
//! so a newly-introduced divergence fails here by existing — nobody has to have
//! predicted it.

use lunco_usd_bevy::{compose_file_to_stage, CanonicalStage, UsdRead};
use openusd::sdf::Path as SdfPath;

/// The two rover prims in `scenes/tests/drivetrain_parity.usda` — the SAME
/// `skid_rover.usda` composed twice, differing in exactly one authored opinion.
const RAYCAST: &str = "/DrivetrainParity/RoverRaycast";
const PHYSICAL: &str = "/DrivetrainParity/RoverPhysical";

/// Compose the real parity scene — the very file `scene_test` drives.
///
/// NOT a synthetic in-memory wrapper: `StageRecipe::from_source` builds a
/// single-layer stage with no closure, so every `@lunco://…@` reference silently
/// resolves to nothing and a comparison of two empty prims passes while proving
/// nothing. `compose_file_to_stage` anchors the root under the shipped-asset root
/// and walks the real layer closure, which is also what the app does.
fn parity_scene() -> CanonicalStage {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/tests/drivetrain_parity.usda");
    let stage = compose_file_to_stage(&path)
        .unwrap_or_else(|e| panic!("compose {}: {e:?}", path.display()));
    CanonicalStage::from_stage(stage, path.to_string_lossy().to_string())
}

/// A scalar 3-vector attribute, whichever precision it was authored at
/// (`double3` for transforms, `float3` for the inertia tensor).
fn vec3(view: &lunco_usd_bevy::StageView<'_>, prim: &SdfPath, attr: &str) -> Option<[f64; 3]> {
    if let Some(v) = view.scalar::<[f64; 3]>(prim, attr) {
        return Some(v);
    }
    view.scalar::<[f32; 3]>(prim, attr)
        .map(|v| [v[0] as f64, v[1] as f64, v[2] as f64])
}

/// `prim`'s subpath under whichever rover root it belongs to, so the two
/// compositions' prims line up for comparison (`…/RoverRaycast/Wheel_FL` and
/// `…/RoverPhysical/Wheel_FL` both key as `/Wheel_FL`).
fn under(prim: &str, root: &str) -> Option<String> {
    prim.strip_prefix(root).map(str::to_string)
}

/// Every `prim.attr = value` under one rover, keyed by the prim's path RELATIVE
/// to that rover so the two rovers' maps are directly comparable.
///
/// Values are compared by their debug form rather than through typed accessors on
/// purpose: this test must catch a divergence in ANY attribute, including ones
/// nobody thought to write a getter for.
fn attrs(stage: &CanonicalStage, root: &str) -> std::collections::BTreeMap<String, String> {
    let view = stage.view();
    let mut out = std::collections::BTreeMap::new();
    for prim in view.prim_paths() {
        let Some(rel) = under(prim.as_str(), root) else {
            continue;
        };
        for name in view.attr_names(&prim) {
            if let Some(v) = view.attr_value(&prim, &name) {
                out.insert(format!("{rel}.{name}"), format!("{v:?}"));
            }
        }
    }
    out
}

/// Where a wheel sits on the rover — authored ONCE, on the rover.
///
/// It used to be authored per drivetrain, which was the bug: `raycast_drivetrain.usda`
/// placed the wheel prim at the strut top (y = -0.15) and `physical_drivetrain.usda`
/// at the axle (y = -0.65), so the prim meant two different points and the two rovers
/// shared neither a ride height nor a centre-of-mass height. The prim is the AXLE in
/// both realizations now; the raycast wheel derives its strut top from the authored
/// suspension (`lunco_mobility::strut_offset`).
const WHEEL_MOUNT_TRANSLATE: &str = "xformOp:translate";

#[test]
fn the_two_realizations_compose_the_same_vehicle() {
    let stage = parity_scene();
    let (ra, rb) = (attrs(&stage, RAYCAST), attrs(&stage, PHYSICAL));

    // A vacuous pass is the failure mode this whole file exists to avoid: two
    // empty maps compare equal. The rover has wheels, a chassis and motors, so
    // anything under a hundred attributes means composition did not resolve.
    assert!(
        ra.len() > 100 && rb.len() > 100,
        "composition resolved almost nothing (raycast {} attrs, physical {} attrs) — \
         the comparison below would pass without comparing anything",
        ra.len(),
        rb.len()
    );

    // The rover-root `xformOp:translate` is the LANE the scene parks each rover
    // in (-25 vs +25 X), not a property of the vehicle.
    let lane = ".xformOp:translate";

    // Only attributes the two compositions SHARE. The physical variant legitimately
    // adds prims the raycast one has no use for (the articulation root, the per-wheel
    // revolute joints); a prim that exists on one side only is the variant doing its
    // job. A prim on BOTH sides with a different value is the variant exceeding it.
    let mut diffs: Vec<String> = Vec::new();
    for (key, va) in &ra {
        let Some(vb) = rb.get(key) else { continue };
        if va == vb {
            continue;
        }
        // The scene parks the two rovers in different lanes; that is the scene's
        // doing, not the variant's.
        if key == lane {
            continue;
        }
        // The livery `over "Chassis"` is display-only — the parity scene says so
        // itself, and displayColor touches no physics.
        if key.ends_with(".primvars:displayColor") {
            continue;
        }
        diffs.push(format!("  {key}\n      raycast : {va}\n      physical: {vb}"));
    }

    assert!(
        diffs.is_empty(),
        "`drivetrain` changed {} propert{} of the vehicle itself, not just how its \
         wheels are simulated. A variant chooses a REALIZATION; these are the ROVER:\n{}",
        diffs.len(),
        if diffs.len() == 1 { "y" } else { "ies" },
        diffs.join("\n")
    );
}

#[test]
fn the_wheels_sit_at_the_same_place_on_the_vehicle() {
    // A wheel is at a place on the rover. Track (X), wheelbase (Z) AND ride
    // height (Y) are the vehicle's geometry, and none of them may depend on how
    // the wheel is simulated: a wheel mounted lower carries the chassis higher,
    // which moves the centre of mass and changes how the rover turns — a
    // difference no tire parameter can compensate for and none should have to.
    let stage = parity_scene();
    let view = stage.view();

    let mut checked = 0;
    for wheel in ["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
        let pa = vec3(
            &view,
            &SdfPath::new(&format!("{RAYCAST}/{wheel}")).unwrap(),
            WHEEL_MOUNT_TRANSLATE,
        );
        let pb = vec3(
            &view,
            &SdfPath::new(&format!("{PHYSICAL}/{wheel}")).unwrap(),
            WHEEL_MOUNT_TRANSLATE,
        );
        let (Some(pa), Some(pb)) = (pa, pb) else {
            continue;
        };
        assert!(
            (pa[0] - pb[0]).abs() < 1e-6
                && (pa[1] - pb[1]).abs() < 1e-6
                && (pa[2] - pb[2]).abs() < 1e-6,
            "{wheel} sits somewhere else depending on how it is simulated: \
             raycast {pa:?} vs physical {pb:?}"
        );
        checked += 1;
    }

    // A silent zero-comparison run is not a pass — a `find` that resolves nothing
    // looks exactly like a clean run.
    assert_eq!(checked, 4, "expected to compare four wheel mounts, compared {checked}");
}

#[test]
fn the_chassis_masses_the_same_either_way() {
    // The chassis is the same authored body in both compositions. This is the
    // AUTHORED half of the mass contract; the realized half (folding the proxy
    // wheels onto it so the vehicle totals 1100 kg either way) is pinned by
    // `lunco_mobility::proxy_wheel_mass_tests`.
    let stage = parity_scene();
    let view = stage.view();
    let (ra, rp) = (
        SdfPath::new(RAYCAST).unwrap(),
        SdfPath::new(PHYSICAL).unwrap(),
    );

    for attr in [
        "physics:mass",
        "physxRigidBody:linearDamping",
        "physxRigidBody:angularDamping",
    ] {
        let (x, y) = (view.real(&ra, attr), view.real(&rp, attr));
        assert!(x.is_some(), "{attr} not authored — the test is checking nothing");
        assert_eq!(x, y, "{attr} differs between drivetrain variants");
    }

    let (ia_, ib_) = (
        vec3(&view, &ra, "physics:diagonalInertia"),
        vec3(&view, &rp, "physics:diagonalInertia"),
    );
    assert!(ia_.is_some(), "diagonalInertia not authored");
    assert_eq!(ia_, ib_, "diagonalInertia differs between drivetrain variants");
}

#[test]
fn every_wheel_reads_the_same_parameters_in_both_realizations() {
    // ONE `WheelParams` reader serves both kinds. If the two compositions hand it
    // different numbers then "same parameter set, two realizations" is a fiction
    // and parity is unreachable by construction.
    let stage = parity_scene();
    let view = stage.view();

    let mut checked = 0;
    for wheel in ["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
        let pa = lunco_usd_sim::wheel_params::WheelParams::read(
            &view,
            &SdfPath::new(&format!("{RAYCAST}/{wheel}")).unwrap(),
            None,
            None,
        );
        let pb = lunco_usd_sim::wheel_params::WheelParams::read(
            &view,
            &SdfPath::new(&format!("{PHYSICAL}/{wheel}")).unwrap(),
            None,
            None,
        );

        match (pa, pb) {
            (Ok(pa), Ok(pb)) => {
                assert_eq!(pa.radius, pb.radius, "{wheel} radius");
                assert_eq!(pa.mass, pb.mass, "{wheel} mass");
                assert_eq!(pa.moment_of_inertia, pb.moment_of_inertia, "{wheel} moi");
                assert_eq!(pa.peak_torque, pb.peak_torque, "{wheel} peak torque");
                assert_eq!(
                    pa.max_rotation_speed, pb.max_rotation_speed,
                    "{wheel} no-load speed — the ONE top-speed parameter"
                );
                assert_eq!(pa.bearing_damping, pb.bearing_damping, "{wheel} bearing drag");
                assert_eq!(pa.friction_mu, pb.friction_mu, "{wheel} tire friction");
                assert_eq!(pa.slip_stiffness, pb.slip_stiffness, "{wheel} slip stiffness");
                assert_eq!(
                    pa.lateral_stiffness, pb.lateral_stiffness,
                    "{wheel} lateral stiffness"
                );
                assert_eq!(pa.brake_torque_max, pb.brake_torque_max, "{wheel} brake");
                checked += 1;
            }
            (a, b) => panic!("{wheel} did not resolve identically: {a:?} vs {b:?}"),
        }
    }
    assert_eq!(checked, 4, "expected four wheels, compared {checked}");
}
