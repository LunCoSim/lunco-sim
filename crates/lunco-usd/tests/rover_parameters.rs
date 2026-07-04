//! Tests for USD parameter parsing.
//!
//! Verifies that all visual, physics, and electrical parameters
//! are correctly extracted from rover USD files after composition.

use lunco_usd_bevy::{StageView, UsdRead};
use openusd::sdf::Path as SdfPath;
use openusd::usd::Stage;
use std::path::PathBuf;

/// Load the rover USD file with all references resolved, composed through the
/// real openusd PCP engine (`Stage::open` + `ar::DefaultResolver`) into a live
/// `Stage` — the canonical composed representation a `StageView` reads.
fn load_rover() -> Stage {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let asset_root = manifest_dir.parent().unwrap().parent().unwrap();
    let usd_path = asset_root.join("assets/vessels/rovers/rucheyok/rucheyok.usda");

    lunco_usd_bevy::compose_file_to_stage(&usd_path)
        .unwrap_or_else(|e| panic!("Failed to compose {usd_path:?}: {e}"))
}

#[test]
fn test_chassis_dimensions() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok/Chassis").unwrap();

    assert!(view.has_prim(&path), "Chassis prim should exist");

    // Dimensions are authored spec-compliantly as `size` (unit) + `xformOp:scale`;
    // the scale components carry the full extents (width, height, depth).
    let scale: [f64; 3] = view
        .value(&path, "xformOp:scale")
        .expect("Chassis should have 'xformOp:scale'");
    assert!((scale[0] - 15.0).abs() < 0.01, "Chassis width (scale.x) should be 15.0, got {}", scale[0]);
    assert!((scale[1] - 4.0).abs() < 0.01, "Chassis height (scale.y) should be 4.0, got {}", scale[1]);
    assert!((scale[2] - 20.0).abs() < 0.01, "Chassis depth (scale.z) should be 20.0, got {}", scale[2]);
}

#[test]
fn test_chassis_physics() {
    let stage = load_rover();
    let view = StageView::new(&stage);

    // Mass lives on the rigid-body root (Rucheyok), not the Chassis collider
    // child. Authored as `float`, so read f32 and widen.
    let root = SdfPath::new("/Rucheyok").unwrap();
    let mass: f64 = view
        .value::<f64>(&root, "physics:mass")
        .or_else(|| view.value::<f32>(&root, "physics:mass").map(|m| m as f64))
        .expect("Rucheyok root should have physics:mass");
    assert!((mass - 800.0).abs() < 1.0, "Rover mass should be 800.0, got {mass}");

    // Chassis is the collider child.
    assert!(
        view.has_prim(&SdfPath::new("/Rucheyok/Chassis").unwrap()),
        "Chassis collider prim should exist"
    );
}

#[test]
fn test_solar_panel_dimensions() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok/SolarPanel").unwrap();

    assert!(view.has_prim(&path), "SolarPanel prim should exist");

    // Dimensions via `xformOp:scale` (see test_chassis_dimensions).
    let scale: [f64; 3] = view
        .value(&path, "xformOp:scale")
        .expect("SolarPanel should have 'xformOp:scale'");
    assert!((scale[0] - 12.0).abs() < 0.01, "SolarPanel width should be 12.0, got {}", scale[0]);
    assert!((scale[1] - 0.2).abs() < 0.01, "SolarPanel height should be 0.2, got {}", scale[1]);
    assert!((scale[2] - 6.0).abs() < 0.01, "SolarPanel depth should be 6.0, got {}", scale[2]);
}

#[test]
fn test_solar_panel_position() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok/SolarPanel").unwrap();

    // `double3` composes to `Value::Vec3d` — read as a fixed `[f64; 3]`, the
    // variant the strict `TryFrom<Value>` decode matches (a `Vec<f64>` would
    // only match a `double[]` array).
    let translate: [f64; 3] = view
        .value(&path, "xformOp:translate")
        .expect("SolarPanel should have translate");
    assert!((translate[0] - 0.0).abs() < 0.01, "SolarPanel X should be 0");
    assert!((translate[1] - 4.5).abs() < 0.01, "SolarPanel Y should be 4.5");
    assert!((translate[2] - 0.0).abs() < 0.01, "SolarPanel Z should be 0");
}

#[test]
fn test_battery_dimensions() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok/Battery").unwrap();

    assert!(view.has_prim(&path), "Battery prim should exist");

    // Dimensions via `xformOp:scale` (see test_chassis_dimensions).
    let scale: [f64; 3] = view
        .value(&path, "xformOp:scale")
        .expect("Battery should have 'xformOp:scale'");
    assert!((scale[0] - 4.0).abs() < 0.01, "Battery width should be 4.0, got {}", scale[0]);
    assert!((scale[1] - 0.8).abs() < 0.01, "Battery height should be 0.8, got {}", scale[1]);
    assert!((scale[2] - 6.0).abs() < 0.01, "Battery depth should be 6.0, got {}", scale[2]);
}

#[test]
fn test_wheel_positions() {
    let stage = load_rover();
    let view = StageView::new(&stage);

    struct WheelExpect {
        path: &'static str,
        x: f64,
        z: f64,
        index: i64,
    }

    let wheels = [
        WheelExpect { path: "/Rucheyok/Wheel_FL", x: -8.5, z: -10.5, index: 0 },
        WheelExpect { path: "/Rucheyok/Wheel_FR", x: 8.5, z: -10.5, index: 1 },
        WheelExpect { path: "/Rucheyok/Wheel_RL", x: -8.5, z: 10.5, index: 2 },
        WheelExpect { path: "/Rucheyok/Wheel_RR", x: 8.5, z: 10.5, index: 3 },
    ];

    for wheel in wheels {
        let path = SdfPath::new(wheel.path).unwrap();
        assert!(view.has_prim(&path), "Wheel {path} should exist");

        let translate: [f64; 3] = view
            .value(&path, "xformOp:translate")
            .unwrap_or_else(|| panic!("Wheel {path} should have translate"));
        assert!(
            (translate[0] - wheel.x).abs() < 0.01,
            "Wheel {path} X should be {}, got {}",
            wheel.x,
            translate[0]
        );
        assert!(
            (translate[1] - 0.0).abs() < 0.01,
            "Wheel {path} Y should be 0"
        );
        assert!(
            (translate[2] - wheel.z).abs() < 0.01,
            "Wheel {path} Z should be {}, got {}",
            wheel.z,
            translate[2]
        );

        let idx: i64 = view
            .value(&path, "physxVehicleWheel:index")
            .unwrap_or_else(|| {
                let i: i32 = view.value(&path, "physxVehicleWheel:index")
                    .unwrap_or_else(|| panic!("Wheel {path} should have index"));
                i as i64
            });
        assert_eq!(idx, wheel.index, "Wheel {path} index should be {}", wheel.index);
    }
}

#[test]
fn test_wheel_physics() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok/Wheel_FL").unwrap();

    let rigid_body: bool = view
        .value(&path, "physics:rigidBodyEnabled")
        .expect("Wheel should have rigidBodyEnabled");
    assert!(rigid_body, "Wheel should be rigid body enabled");

    let mass: f64 = view
        .value(&path, "physics:mass")
        .unwrap_or_else(|| {
            let m: f32 = view.value(&path, "physics:mass")
                .expect("Wheel should have mass");
            m as f64
        });
    assert!((mass - 25.0).abs() < 1.0, "Wheel mass should be 25.0, got {mass}");

    let radius: f64 = view
        .value(&path, "physxVehicleWheel:radius")
        .unwrap_or_else(|| {
            let r: f32 = view.value(&path, "physxVehicleWheel:radius")
                .expect("Wheel should have radius");
            r as f64
        });
    assert!((radius - 2.0).abs() < 0.01, "Wheel radius should be 2.0, got {radius}");

    let spring_k: f64 = view
        .value(&path, "physxVehicleSuspension:springStiffness")
        .unwrap_or_else(|| {
            let k: f32 = view.value(&path, "physxVehicleSuspension:springStiffness")
                .expect("Wheel should have springStiffness");
            k as f64
        });
    assert!(
        (spring_k - 15000.0).abs() < 1.0,
        "Wheel spring stiffness should be 15000.0, got {spring_k}"
    );

    let damping: f64 = view
        .value(&path, "physxVehicleSuspension:springDamping")
        .unwrap_or_else(|| {
            let d: f32 = view.value(&path, "physxVehicleSuspension:springDamping")
                .expect("Wheel should have springDamping");
            d as f64
        });
    assert!(
        (damping - 5000.0).abs() < 1.0,
        "Wheel damping should be 5000.0, got {damping}"
    );
}

#[test]
fn test_prim_children() {
    let stage = load_rover();
    let view = StageView::new(&stage);
    let path = SdfPath::new("/Rucheyok").unwrap();

    let children = view.prim_children(&path);
    // 8 children: Chassis, SolarPanel, Battery, 4 Wheels + the `Controls` scope
    // composed in from the inherited `_RoverControl` profile (control_profiles.usda).
    assert_eq!(children.len(), 8,
        "Rucheyok should have 8 children (Chassis, SolarPanel, Battery, 4 Wheels, Controls)");

    let child_names: Vec<&str> = children.iter().map(|p| p.as_str()).collect();
    assert!(child_names.contains(&"/Rucheyok/Chassis"));
    assert!(child_names.contains(&"/Rucheyok/SolarPanel"));
    assert!(child_names.contains(&"/Rucheyok/Battery"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_FL"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_FR"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_RL"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_RR"));
    assert!(child_names.contains(&"/Rucheyok/Controls"), "Controls scope from _RoverControl");
}

#[test]
fn test_all_prims_have_color() {
    let stage = load_rover();
    let view = StageView::new(&stage);

    let prims_with_color = [
        "/Rucheyok/Chassis",
        "/Rucheyok/SolarPanel",
        "/Rucheyok/Battery",
        "/Rucheyok/Wheel_FL",
        "/Rucheyok/Wheel_FR",
        "/Rucheyok/Wheel_RL",
        "/Rucheyok/Wheel_RR",
    ];

    for prim_path in prims_with_color {
        let path = SdfPath::new(prim_path).unwrap();
        // `color3f` composes to a scalar `Value::Vec3f` → read as `[f32; 3]`.
        let _color: [f32; 3] = view
            .value(&path, "primvars:displayColor")
            .unwrap_or_else(|| panic!("Prim {prim_path} should have displayColor"));
    }
}

#[test]
fn test_eps_relationships() {
    let stage = load_rover();
    let view = StageView::new(&stage);

    // Solar panel connects to EPS bus
    assert!(
        !view.rel_targets(&SdfPath::new("/Rucheyok/SolarPanel").unwrap(), "lunco:epsBus").is_empty(),
        "SolarPanel should have epsBus relationship"
    );

    // Battery connects to EPS bus
    assert!(
        !view.rel_targets(&SdfPath::new("/Rucheyok/Battery").unwrap(), "lunco:epsBus").is_empty(),
        "Battery should have epsBus relationship"
    );
}

#[test]
fn test_component_eps_fields() {
    // The rover instance wires its power components onto the EPS bus via
    // `rel lunco:epsBus` (authored on the rover, not the reusable components),
    // so the relationships must survive composition onto the instance prims.
    let stage = load_rover();
    let view = StageView::new(&stage);
    for p in [
        "/Rucheyok/SolarPanel",
        "/Rucheyok/Battery",
        "/Rucheyok/Wheel_FL",
    ] {
        assert!(
            !view.rel_targets(&SdfPath::new(p).unwrap(), "lunco:epsBus").is_empty(),
            "{p} epsBus relationship should exist after composition"
        );
    }
}
