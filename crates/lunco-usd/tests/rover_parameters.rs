//! Tests for USD parameter parsing.
//!
//! Verifies that all visual, physics, and electrical parameters
//! are correctly extracted from rover USD files.

use openusd::sdf::{AbstractData, Path as SdfPath};
use openusd::usda::TextReader;
use std::path::PathBuf;

/// Load the rover USD file and return the reader.
fn load_rover() -> TextReader {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let asset_path = PathBuf::from(manifest_dir)
        .parent().unwrap()
        .parent().unwrap()
        .join("assets/vessels/rovers/rucheyok/rucheyok.usda");
    
    TextReader::read(&asset_path)
        .unwrap_or_else(|e| panic!("Failed to load {:?}: {}", asset_path, e))
}

#[test]
fn test_chassis_dimensions() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/Chassis").unwrap();

    assert!(reader.has_spec(&path), "Chassis prim should exist");

    let width: f64 = reader
        .prim_attribute_value(&path, "width")
        .expect("Chassis should have 'width' attribute");
    assert!((width - 15.0).abs() < 0.01, "Chassis width should be 15.0, got {width}");

    let height: f64 = reader
        .prim_attribute_value(&path, "height")
        .expect("Chassis should have 'height' attribute");
    assert!((height - 4.0).abs() < 0.01, "Chassis height should be 4.0, got {height}");

    let depth: f64 = reader
        .prim_attribute_value(&path, "depth")
        .expect("Chassis should have 'depth' attribute");
    assert!((depth - 20.0).abs() < 0.01, "Chassis depth should be 20.0, got {depth}");
}

#[test]
fn test_chassis_physics() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/Chassis").unwrap();

    let rigid_body: bool = reader
        .prim_attribute_value(&path, "physics:rigidBodyEnabled")
        .expect("Chassis should have rigidBodyEnabled");
    assert!(rigid_body, "Chassis should be rigid body enabled");

    let mass: f64 = reader
        .prim_attribute_value(&path, "physics:mass")
        .unwrap_or_else(|| {
            // Try as f32 then convert
            let m: f32 = reader.prim_attribute_value(&path, "physics:mass")
                .expect("Chassis should have mass as f32");
            m as f64
        });
    assert!((mass - 800.0).abs() < 1.0, "Chassis mass should be 800.0, got {mass}");
}

#[test]
fn test_solar_panel_dimensions() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/SolarPanel").unwrap();

    assert!(reader.has_spec(&path), "SolarPanel prim should exist");

    let width: f64 = reader
        .prim_attribute_value(&path, "width")
        .expect("SolarPanel should have 'width'");
    assert!((width - 12.0).abs() < 0.01, "SolarPanel width should be 12.0, got {width}");

    let height: f64 = reader
        .prim_attribute_value(&path, "height")
        .expect("SolarPanel should have 'height'");
    assert!((height - 0.2).abs() < 0.01, "SolarPanel height should be 0.2, got {height}");

    let depth: f64 = reader
        .prim_attribute_value(&path, "depth")
        .expect("SolarPanel should have 'depth'");
    assert!((depth - 6.0).abs() < 0.01, "SolarPanel depth should be 6.0, got {depth}");
}

#[test]
fn test_solar_panel_position() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/SolarPanel").unwrap();

    let translate: Vec<f64> = reader
        .prim_attribute_value(&path, "xformOp:translate")
        .expect("SolarPanel should have translate");
    assert_eq!(translate.len(), 3);
    assert!((translate[0] - 0.0).abs() < 0.01, "SolarPanel X should be 0");
    assert!((translate[1] - 4.5).abs() < 0.01, "SolarPanel Y should be 4.5");
    assert!((translate[2] - 0.0).abs() < 0.01, "SolarPanel Z should be 0");
}

#[test]
fn test_battery_dimensions() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/Battery").unwrap();

    assert!(reader.has_spec(&path), "Battery prim should exist");

    let width: f64 = reader
        .prim_attribute_value(&path, "width")
        .expect("Battery should have 'width'");
    assert!((width - 4.0).abs() < 0.01, "Battery width should be 4.0, got {width}");

    let height: f64 = reader
        .prim_attribute_value(&path, "height")
        .expect("Battery should have 'height'");
    assert!((height - 0.8).abs() < 0.01, "Battery height should be 0.8, got {height}");

    let depth: f64 = reader
        .prim_attribute_value(&path, "depth")
        .expect("Battery should have 'depth'");
    assert!((depth - 6.0).abs() < 0.01, "Battery depth should be 6.0, got {depth}");
}

#[test]
fn test_wheel_positions() {
    let reader = load_rover();

    struct WheelExpect {
        path: &'static str,
        x: f64,
        z: f64,
        index: i64,
    }

    let wheels = [
        WheelExpect { path: "/Rucheyok/Wheel_FL", x: -8.5, z: 10.5, index: 0 },
        WheelExpect { path: "/Rucheyok/Wheel_FR", x: 8.5, z: 10.5, index: 1 },
        WheelExpect { path: "/Rucheyok/Wheel_RL", x: -8.5, z: -10.5, index: 2 },
        WheelExpect { path: "/Rucheyok/Wheel_RR", x: 8.5, z: -10.5, index: 3 },
    ];

    for wheel in wheels {
        let path = SdfPath::new(wheel.path).unwrap();
        assert!(reader.has_spec(&path), "Wheel {path} should exist");

        let translate: Vec<f64> = reader
            .prim_attribute_value(&path, "xformOp:translate")
            .unwrap_or_else(|| panic!("Wheel {path} should have translate"));
        assert_eq!(translate.len(), 3);
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

        let idx: i64 = reader
            .prim_attribute_value(&path, "physxVehicleWheel:index")
            .unwrap_or_else(|| {
                let i: i32 = reader.prim_attribute_value(&path, "physxVehicleWheel:index")
                    .unwrap_or_else(|| panic!("Wheel {path} should have index"));
                i as i64
            });
        assert_eq!(idx, wheel.index, "Wheel {path} index should be {}", wheel.index);
    }
}

#[test]
fn test_wheel_physics() {
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok/Wheel_FL").unwrap();

    let rigid_body: bool = reader
        .prim_attribute_value(&path, "physics:rigidBodyEnabled")
        .expect("Wheel should have rigidBodyEnabled");
    assert!(rigid_body, "Wheel should be rigid body enabled");

    let mass: f64 = reader
        .prim_attribute_value(&path, "physics:mass")
        .unwrap_or_else(|| {
            let m: f32 = reader.prim_attribute_value(&path, "physics:mass")
                .expect("Wheel should have mass");
            m as f64
        });
    assert!((mass - 25.0).abs() < 1.0, "Wheel mass should be 25.0, got {mass}");

    let radius: f64 = reader
        .prim_attribute_value(&path, "physxVehicleWheel:radius")
        .unwrap_or_else(|| {
            let r: f32 = reader.prim_attribute_value(&path, "physxVehicleWheel:radius")
                .expect("Wheel should have radius");
            r as f64
        });
    assert!((radius - 2.0).abs() < 0.01, "Wheel radius should be 2.0, got {radius}");

    let spring_k: f64 = reader
        .prim_attribute_value(&path, "physxVehicleSuspension:springStiffness")
        .unwrap_or_else(|| {
            let k: f32 = reader.prim_attribute_value(&path, "physxVehicleSuspension:springStiffness")
                .expect("Wheel should have springStiffness");
            k as f64
        });
    assert!(
        (spring_k - 15000.0).abs() < 1.0,
        "Wheel spring stiffness should be 15000.0, got {spring_k}"
    );

    let damping: f64 = reader
        .prim_attribute_value(&path, "physxVehicleSuspension:springDamping")
        .unwrap_or_else(|| {
            let d: f32 = reader.prim_attribute_value(&path, "physxVehicleSuspension:springDamping")
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
    let reader = load_rover();
    let path = SdfPath::new("/Rucheyok").unwrap();

    let children = reader.prim_children(&path);
    assert_eq!(children.len(), 7, "Rucheyok should have 7 children (Chassis, SolarPanel, Battery, 4 Wheels)");

    let child_names: Vec<&str> = children.iter().map(|p| p.as_str()).collect();
    assert!(child_names.contains(&"/Rucheyok/Chassis"));
    assert!(child_names.contains(&"/Rucheyok/SolarPanel"));
    assert!(child_names.contains(&"/Rucheyok/Battery"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_FL"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_FR"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_RL"));
    assert!(child_names.contains(&"/Rucheyok/Wheel_RR"));
}

#[test]
fn test_all_prims_have_color() {
    let reader = load_rover();

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
        let color: Vec<f32> = reader
            .prim_attribute_value(&path, "primvars:displayColor")
            .unwrap_or_else(|| panic!("Prim {prim_path} should have displayColor"));
        assert_eq!(color.len(), 3, "Color should have 3 components");
    }
}
