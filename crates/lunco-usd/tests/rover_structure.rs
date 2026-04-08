/// Tests that verify USD rover files match the procedural rover definition.
/// ALL tests load REAL files from disk — no inline USD strings.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use big_space::prelude::CellCoord;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use lunco_mobility::{WheelRaycast, DifferentialDrive, AckermannSteer};
use avian3d::prelude::*;
use lunco_fsw::FlightSoftware;
use lunco_core::{Vessel, RoverVessel};
use lunco_usd_composer::UsdComposer;
use openusd::usda::TextReader;
use openusd::sdf::Path as SdfPath;
use std::sync::Arc;
use std::path::Path;

fn compose_and_load(file_path: &Path, prim_path: &str) -> App {
    let raw = std::fs::read_to_string(file_path)
        .unwrap_or_else(|e| panic!("Missing file: {}\n{}", file_path.display(), e));
    let mut parser = openusd::usda::parser::Parser::new(&raw);
    let data = parser.parse()
        .unwrap_or_else(|e| panic!("Invalid USD: {}\n{}", file_path.display(), e));
    let reader = TextReader::from_data(data);
    let base = Path::new("../../assets/");
    let composed = UsdComposer::flatten(&reader, base)
        .unwrap_or_else(|e| panic!("Composition failed for {}:\n{}", file_path.display(), e));

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let handle = stages.add(UsdStageAsset { reader: Arc::new(composed) });

    app.world_mut().spawn((
        Name::new("TestRover"),
        UsdPrimPath { stage_handle: handle, path: prim_path.to_string() },
        Transform::from_translation(Vec3::new(-15.0, 5.0, -10.0)),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));

    for _ in 0..10 { app.update(); }
    app.world_mut().flush();
    app
}

/// Verify that ALL rover files loaded through the real pipeline produce
/// the exact same component structure as the original procedural spawn.
#[test]
fn test_all_rover_files_match_procedural() {
    let files = [
        ("vessels/rovers/skid_rover.usda", "/SkidRover"),
        ("vessels/rovers/ackermann_rover.usda", "/AckermannRover"),
    ];

    for (file, prim) in &files {
        let label = format!("{file}");
        let mut app = compose_and_load(&Path::new("../../assets/").join(file), prim);

        // Find rover
        let mut q = app.world_mut().query_filtered::<Entity, (With<Vessel>, With<RoverVessel>)>();
        let rover = q.iter(app.world()).next()
            .unwrap_or_else(|| panic!("{label}: No Vessel+RoverVessel entity"));

        // Physics
        let rb = app.world().get::<RigidBody>(rover).expect(&format!("{label}: missing RigidBody"));
        assert_eq!(*rb, RigidBody::Dynamic, "{label}: RigidBody must be Dynamic");

        let mass = app.world().get::<Mass>(rover).expect(&format!("{label}: missing Mass"));
        assert!((mass.0 - 1000.0).abs() < 1.0, "{label}: Mass ~1000, got {}", mass.0);

        let ld = app.world().get::<LinearDamping>(rover).expect(&format!("{label}: missing LinearDamping"));
        assert!((ld.0 - 0.5).abs() < 0.1, "{label}: LinearDamping ~0.5");

        let ad = app.world().get::<AngularDamping>(rover).expect(&format!("{label}: missing AngularDamping"));
        assert!((ad.0 - 2.0).abs() < 0.1, "{label}: AngularDamping ~2.0");

        // Collider
        let col = app.world().get::<Collider>(rover).expect(&format!("{label}: missing Collider"));
        let c = col.shape().as_cuboid().expect(&format!("{label}: Collider must be cuboid"));
        assert!((c.half_extents.x - 1.0).abs() < 0.1, "{label}: hx ~1.0");
        assert!((c.half_extents.y - 0.15).abs() < 0.05, "{label}: hy ~0.15");
        assert!((c.half_extents.z - 1.75).abs() < 0.1, "{label}: hz ~1.75");

        // Visual
        assert!(app.world().get::<Mesh3d>(rover).is_some(), "{label}: missing Mesh3d (body invisible!)");
        assert!(app.world().get::<MeshMaterial3d<StandardMaterial>>(rover).is_some(),
            "{label}: missing MeshMaterial3d (body invisible!)");

        // Steering: Skid has DifferentialDrive, Ackermann has AckermannSteer
        if file.contains("ackermann") {
            let ack = app.world().get::<AckermannSteer>(rover).expect(&format!("{label}: missing AckermannSteer"));
            assert_eq!(ack.drive_left_port, "drive_left", "{label}: wrong drive_left_port");
            assert_eq!(ack.drive_right_port, "drive_right", "{label}: wrong drive_right_port");
            assert_eq!(ack.steer_port, "steering", "{label}: wrong steer_port");
        } else {
            let diff = app.world().get::<DifferentialDrive>(rover).expect(&format!("{label}: missing DifferentialDrive"));
            assert_eq!(diff.left_port, "drive_left", "{label}: wrong left_port");
            assert_eq!(diff.right_port, "drive_right", "{label}: wrong right_port");
        }

        // FlightSoftware
        let fsw = app.world().get::<FlightSoftware>(rover).expect(&format!("{label}: missing FlightSoftware"));
        assert!(fsw.port_map.contains_key("drive_left"), "{label}: FSW missing drive_left");
        assert!(fsw.port_map.contains_key("drive_right"), "{label}: FSW missing drive_right");
        assert!(fsw.port_map.contains_key("steering"), "{label}: FSW missing steering");
        assert!(fsw.port_map.contains_key("brake"), "{label}: FSW missing brake");

        // Wheels
        let children = app.world().get::<Children>(rover).expect(&format!("{label}: missing Children"));
        let mut wheels: Vec<(Entity, String)> = Vec::new();
        for child in children.iter() {
            if let Some(name) = app.world().get::<Name>(child) {
                if name.as_str().contains("Wheel") {
                    wheels.push((child, name.as_str().to_string()));
                }
            }
        }

        assert_eq!(wheels.len(), 4, "{label}: must have 4 wheel children, got {}", wheels.len());

        let expected_rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let expected_positions = [
            ("Wheel_FL", Vec3::new(-1.0, -0.15, 1.225)),
            ("Wheel_FR", Vec3::new(1.0, -0.15, 1.225)),
            ("Wheel_RL", Vec3::new(-1.0, -0.15, -1.225)),
            ("Wheel_RR", Vec3::new(1.0, -0.15, -1.225)),
        ];

        for (w_name, exp_pos) in &expected_positions {
            let found = wheels.iter()
                .find(|(_, n)| n.contains(w_name))
                .unwrap_or_else(|| panic!("{label}: missing {w_name}"));
            let w_ent = found.0;

            let wheel = app.world().get::<WheelRaycast>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing WheelRaycast"));
            assert!((wheel.wheel_radius - 0.4).abs() < 0.01, "{label}: {w_name} radius ~0.4");
            assert!((wheel.rest_length - 0.7).abs() < 0.01, "{label}: {w_name} rest ~0.7");
            assert!((wheel.spring_k - 15000.0).abs() < 100.0, "{label}: {w_name} spring_k ~15000");
            assert!((wheel.damping_c - 3000.0).abs() < 100.0, "{label}: {w_name} damping_c ~3000");

            assert!(app.world().get::<RigidBody>(w_ent).is_none(),
                "{label}: {w_name} must NOT have RigidBody");
            assert!(app.world().get::<Collider>(w_ent).is_none(),
                "{label}: {w_name} must NOT have Collider");

            // Wheel entity (physics) should have identity rotation for correct raycasting.
            // The visual rotation is on a child entity.
            let wt = app.world().get::<Transform>(w_ent)
                .expect(&format!("{label}: {w_name} missing Transform"));
            assert!((wt.translation.x - exp_pos.x).abs() < 0.01, "{label}: {w_name} x ~{}", exp_pos.x);
            assert!((wt.translation.y - exp_pos.y).abs() < 0.01, "{label}: {w_name} y ~{}", exp_pos.y);
            assert!((wt.translation.z - exp_pos.z).abs() < 0.01, "{label}: {w_name} z ~{}", exp_pos.z);

            // Physics entity must have identity rotation (rays go down, not sideways)
            assert!(wt.rotation.angle_between(Quat::IDENTITY).abs() < 0.01,
                "{label}: {w_name} physics entity must have identity rotation, got {:?}", wt.rotation);

            // Visual child must have 90° Z rotation (wheel orientation)
            let children = app.world().get::<bevy::prelude::Children>(w_ent);
            let found_visual = children.map(|c| c.iter().any(|child_ent| {
                app.world().get::<Name>(child_ent).map(|n| n.as_str().contains("visual")).unwrap_or(false)
                    && app.world().get::<Transform>(child_ent).map(|t| {
                        let angle_diff = t.rotation.angle_between(expected_rot);
                        angle_diff.abs() < 0.01
                    }).unwrap_or(false)
            })).unwrap_or(false);
            assert!(found_visual,
                "{label}: {w_name} must have visual child with 90° Z rotation");
        }
    }
}
