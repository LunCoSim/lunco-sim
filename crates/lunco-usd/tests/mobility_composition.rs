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
        get("physxVehicleSuspension:restLength"),
        get("physxVehicleSuspension:springStiffness"),
        get("physxVehicleSuspension:springDamping"),
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

/// A wheel's grip and tread come from its `tire` variant, not from the hub.
#[test]
fn a_wheel_composes_its_tire() {
    let cs = compose("vessels/rovers/skid_rover.usda");
    let view = cs.view();
    let fl = SdfPath::new("/SkidRover/Wheel_FL").unwrap();

    // Grip — `regolith` is the wheel's default tire.
    assert_eq!(
        view.real(&fl, "lunco:frictionCoefficient"),
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
    let material = view
        .rel_target(&fl, "material:binding")
        .expect("Wheel_FL must compose its tire's material binding");
    let surface = view
        .connection_source(&material, "outputs:surface")
        .expect("the bound Material must connect its surface to a Shader");
    let (shader, _) = surface
        .rsplit_once('.')
        .expect("outputs:surface.connect must name a Shader property");
    let shader = SdfPath::new(shader).unwrap();
    assert_eq!(
        view.asset(&shader, "info:wgsl:sourceAsset").as_deref(),
        Some("shaders/wheel.wgsl"),
        "Wheel_FL must compose its tire's tread shader"
    );
}
