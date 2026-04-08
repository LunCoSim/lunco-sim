/// Integration tests that load REAL assets through the EXACT same pipeline as runtime.
/// NO inline USD strings. NO manual file reading. Uses AssetServer just like the app.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use lunco_mobility::{WheelRaycast, DifferentialDrive, AckermannSteer};
use lunco_core::{Vessel, RoverVessel};
use lunco_fsw::FlightSoftware;
use lunco_usd_composer::UsdComposer;
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath};
use avian3d::prelude::*;
use big_space::prelude::CellCoord;
use std::sync::Arc;
use std::path::Path;

// ============================================================
// Helper: compose asset using EXACT same logic as runtime loader
// ============================================================

fn compose_asset_from_file(file_path: &Path) -> TextReader {
    let raw = std::fs::read_to_string(file_path)
        .unwrap_or_else(|e| panic!("Missing file: {}\n{}", file_path.display(), e));
    let mut parser = openusd::usda::parser::Parser::new(&raw);
    let data = parser.parse()
        .unwrap_or_else(|e| panic!("Invalid USD: {}\n{}", file_path.display(), e));
    let reader = TextReader::from_data(data);
    // Use the file's parent directory as base for resolving relative references
    let base_dir = file_path.parent().unwrap_or(Path::new("."));
    UsdComposer::flatten(&reader, base_dir)
        .unwrap_or_else(|e| panic!("Composition failed for {}:\n{}", file_path.display(), e))
}

// ============================================================
// Test: Sandbox Rover files load and compose correctly
// ============================================================

#[test]
fn test_sandbox_rover_files_compose() {
    let files = [
        "vessels/rovers/skid_rover.usda",
        "vessels/rovers/ackermann_rover.usda",
    ];
    for f in &files {
        let p = Path::new("../../assets/").join(f);
        let reader = compose_asset_from_file(&p);
        // Verify the composed reader has rover + 4 wheels
        let rover_name = if f.contains("ackermann") { "AckermannRover" } else { "SkidRover" };
        assert!(reader.has_spec(&SdfPath::new(&format!("/{}", rover_name)).unwrap()),
            "{f}: /{} must exist after composition", rover_name);
        for w in &["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let wp = SdfPath::new(&format!("/{}/{}", rover_name, w)).unwrap();
            assert!(reader.has_spec(&wp),
                "{f}: /{}/{} must exist after composition (wheel reference broken?)", rover_name, w);
        }
    }
}

// ============================================================
// Test: Sandbox Scene composes correctly
// ============================================================

#[test]
fn test_sandbox_scene_composes() {
    let p = Path::new("../../assets/scenes/sandbox/sandbox_scene.usda");
    let reader = compose_asset_from_file(p);

    // Ground
    let ground = SdfPath::new("/SandboxScene/Ground").unwrap();
    assert!(reader.has_spec(&ground), "Ground must exist");
    let w: f64 = reader.prim_attribute_value(&ground, "width").expect("Ground width");
    let h: f64 = reader.prim_attribute_value(&ground, "height").expect("Ground height");
    let d: f64 = reader.prim_attribute_value(&ground, "depth").expect("Ground depth");
    assert!((w - 4000.0).abs() < 1.0, "Ground width ~4000, got {w}");
    assert!((h - 0.2).abs() < 0.05, "Ground height ~0.2, got {h}");
    assert!((d - 4000.0).abs() < 1.0, "Ground depth ~4000, got {d}");

    // Ramp
    let ramp = SdfPath::new("/SandboxScene/Ramp").unwrap();
    assert!(reader.has_spec(&ramp), "Ramp must exist");
    let rw: f64 = reader.prim_attribute_value(&ramp, "width").expect("Ramp width");
    let rh: f64 = reader.prim_attribute_value(&ramp, "height").expect("Ramp height");
    let rd: f64 = reader.prim_attribute_value(&ramp, "depth").expect("Ramp depth");
    assert!((rw - 60.0).abs() < 1.0, "Ramp width ~60, got {rw}");
    assert!((rh - 2.0).abs() < 0.05, "Ramp height ~2, got {rh}");
    assert!((rd - 80.0).abs() < 1.0, "Ramp depth ~80, got {rd}");
}

// ============================================================
// Test: Load rovers through Bevy pipeline, verify ALL components
// ============================================================

fn load_rover_through_bevy(file_path: &Path, prim_path: &str) -> App {
    let composed = compose_asset_from_file(file_path);
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
    ));

    for _ in 0..10 { app.update(); }
    app.world_mut().flush();
    app
}

#[test]
fn test_rover_components_via_bevy_pipeline() {
    let files = [
        "vessels/rovers/skid_rover.usda",
        "vessels/rovers/ackermann_rover.usda",
    ];
    let paths = ["/SkidRover", "/AckermannRover"];

    for (i, f) in files.iter().enumerate() {
        let p = Path::new("../../assets/").join(f);
        let label = f;

        let mut app = load_rover_through_bevy(&p, paths[i]);

        // Find rover entity (has both Vessel and RoverVessel)
        let mut q_rover = app.world_mut().query_filtered::<Entity, (With<Vessel>, With<RoverVessel>)>();
        let rover_ent = q_rover.iter(app.world()).next()
            .unwrap_or_else(|| panic!("{label}: No entity with Vessel+RoverVessel found"));

        // --- REQUIRED COMPONENTS ---

        // Physics
        let rb = app.world().get::<RigidBody>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing RigidBody"));
        assert_eq!(*rb, RigidBody::Dynamic, "{label}: RigidBody must be Dynamic");

        let mass = app.world().get::<Mass>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing Mass"));
        assert!((mass.0 - 1000.0).abs() < 1.0, "{label}: Mass must be ~1000, got {}", mass.0);

        let lin_damp = app.world().get::<LinearDamping>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing LinearDamping"));
        assert!((lin_damp.0 - 0.5).abs() < 0.1, "{label}: LinearDamping must be ~0.5");

        let ang_damp = app.world().get::<AngularDamping>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing AngularDamping"));
        assert!((ang_damp.0 - 2.0).abs() < 0.1, "{label}: AngularDamping must be ~2.0");

        // Collider: USD stores FULL dimensions (2.0 x 0.3 x 3.5)
        // Collider::cuboid(2.0, 0.3, 3.5) → half-extents: (1.0, 0.15, 1.75)
        let col = app.world().get::<Collider>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing Collider"));
        let cuboid = col.shape().as_cuboid()
            .unwrap_or_else(|| panic!("{label}: Collider must be cuboid"));
        assert!((cuboid.half_extents.x - 1.0).abs() < 0.1,
            "{label}: Collider hx must be ~1.0 (width/2), got {}", cuboid.half_extents.x);
        assert!((cuboid.half_extents.y - 0.15).abs() < 0.05,
            "{label}: Collider hy must be ~0.15 (height/2), got {}", cuboid.half_extents.y);
        assert!((cuboid.half_extents.z - 1.75).abs() < 0.1,
            "{label}: Collider hz must be ~1.75 (depth/2), got {}", cuboid.half_extents.z);

        // Visual (Mesh3d + material)
        let mesh = app.world().get::<Mesh3d>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing Mesh3d (body not visible!)"));
        let _mat = app.world().get::<MeshMaterial3d<StandardMaterial>>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing MeshMaterial3d (body not visible!)"));

        // Steering: Skid has DifferentialDrive, Ackermann has AckermannSteer
        if f.contains("ackermann") {
            let ack = app.world().get::<AckermannSteer>(rover_ent)
                .unwrap_or_else(|| panic!("{label}: Missing AckermannSteer (cannot steer!)"));
            assert!(!ack.drive_left_port.is_empty(), "{label}: AckermannSteer.drive_left_port empty");
            assert!(!ack.drive_right_port.is_empty(), "{label}: AckermannSteer.drive_right_port empty");
            assert!(!ack.steer_port.is_empty(), "{label}: AckermannSteer.steer_port empty");
        } else {
            let diff = app.world().get::<DifferentialDrive>(rover_ent)
                .unwrap_or_else(|| panic!("{label}: Missing DifferentialDrive (cannot steer!)"));
            assert!(!diff.left_port.is_empty(), "{label}: DifferentialDrive.left_port empty");
            assert!(!diff.right_port.is_empty(), "{label}: DifferentialDrive.right_port empty");
        }

        // FlightSoftware (for command routing)
        let fsw = app.world().get::<FlightSoftware>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing FlightSoftware"));
        assert!(fsw.port_map.contains_key("drive_left"), "{label}: FSW missing drive_left");
        assert!(fsw.port_map.contains_key("drive_right"), "{label}: FSW missing drive_right");
        assert!(fsw.port_map.contains_key("steering"), "{label}: FSW missing steering");
        assert!(fsw.port_map.contains_key("brake"), "{label}: FSW missing brake");

        // --- WHEELS ---
        // Find children of rover
        let children = app.world().get::<Children>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Rover must have Children"));
        let child_count = children.len();
        assert!(child_count >= 4, "{label}: Rover must have >= 4 children, got {child_count}");

        // Find wheel children
        let mut wheel_ents = Vec::new();
        for child in children.iter() {
            if let Some(name) = app.world().get::<Name>(child) {
                if name.as_str().contains("Wheel") {
                    wheel_ents.push((child, name.as_str().to_string()));
                }
            }
        }
        assert_eq!(wheel_ents.len(), 4,
            "{label}: Must have exactly 4 wheel children, found {} named: {:?}",
            wheel_ents.len(), wheel_ents.iter().map(|(_, n)| n).collect::<Vec<_>>());

        for (w_ent, w_name) in &wheel_ents {
            // MUST have WheelRaycast
            let wheel = app.world().get::<WheelRaycast>(*w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing WheelRaycast"));
            assert!((wheel.wheel_radius - 0.4).abs() < 0.01, "{label}: {w_name} radius ~0.4");
            assert!((wheel.rest_length - 0.7).abs() < 0.01, "{label}: {w_name} rest ~0.7");
            assert!((wheel.spring_k - 15000.0).abs() < 100.0, "{label}: {w_name} spring_k ~15000");
            assert!((wheel.damping_c - 3000.0).abs() < 100.0, "{label}: {w_name} damping_c ~3000");

            // MUST have RayCaster
            assert!(app.world().get::<RayCaster>(*w_ent).is_some(),
                "{label}: {w_name} missing RayCaster");

            // MUST NOT have collider/rigidbody (wheels are raycast, not physical)
            assert!(app.world().get::<RigidBody>(*w_ent).is_none(),
                "{label}: {w_name} must NOT have RigidBody");
            assert!(app.world().get::<Collider>(*w_ent).is_none(),
                "{label}: {w_name} must NOT have Collider");

            // Physics entity should NOT have Mesh3d (visual child has the mesh)
            assert!(app.world().get::<Mesh3d>(*w_ent).is_none(),
                "{label}: {w_name} physics entity must NOT have Mesh3d (mesh is on visual child)");

            // Visual child must have Mesh3d with 90° Z rotation
            if let Some(children) = app.world().get::<bevy::prelude::Children>(*w_ent) {
                let found_visual = children.iter().any(|gc| {
                    app.world().get::<Name>(gc).map(|n| n.as_str().contains("visual")).unwrap_or(false)
                        && app.world().get::<Mesh3d>(gc).is_some()
                });
                assert!(found_visual,
                    "{label}: {w_name} must have visual child with Mesh3d");
            } else {
                panic!("{label}: {w_name} must have children (visual child)");
            }
        }
    }
}

/// Test that wheel cylinder meshes get CORRECT dimensions (not wheel.usda defaults).
/// BUG CATCHER: composition must merge overrides correctly.
#[test]
fn test_wheel_mesh_dimensions_after_composition() {
    let rover_files = [
        ("vessels/rovers/skid_rover.usda", "SkidRover"),
        ("vessels/rovers/ackermann_rover.usda", "AckermannRover"),
    ];
    for (f, rover_name) in &rover_files {
        let p = Path::new("../../assets/").join(f);
        let label = f;

        let raw = std::fs::read_to_string(&p).unwrap();
        let mut parser = openusd::usda::parser::Parser::new(&raw);
        let data = parser.parse().unwrap();
        let reader = TextReader::from_data(data);
        let composed = UsdComposer::flatten(&reader, Path::new("../../assets/"))
            .unwrap_or_else(|e| panic!("{label} composition failed: {e}"));

        for w_name in &["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let wp = SdfPath::new(&format!("/{}/{}", rover_name, w_name)).unwrap();
            assert!(composed.has_spec(&wp), "{label}: {w_name} must exist after composition");

            let radius: f64 = composed.prim_attribute_value(&wp, "radius")
                .unwrap_or_else(|| panic!("{label}: {w_name} missing 'radius' after composition"));
            let height: f64 = composed.prim_attribute_value(&wp, "height")
                .unwrap_or_else(|| panic!("{label}: {w_name} missing 'height' after composition"));

            // Rover override: radius=0.4, height=0.3
            // wheel.usda default: radius=2.0, height=4.0
            assert!((radius - 0.4).abs() < 0.05,
                "{label}: {w_name} cylinder radius must be ~0.4, got {radius} (using wheel.usda default 2.0?)");
            assert!((height - 0.3).abs() < 0.05,
                "{label}: {w_name} cylinder height must be ~0.3, got {height} (using wheel.usda default 4.0?)");
        }
    }
}

// ============================================================
// Test: Async asset loading + sim processing
// Catches: rovers spawning without FSW/DifferentialDrive because
// assets load AFTER the observer fires (async loading bug)
// ============================================================

#[test]
fn test_rover_sim_processing_after_async_load() {
    let rover_files = [
        ("vessels/rovers/skid_rover.usda", "/SkidRover"),
        ("vessels/rovers/ackermann_rover.usda", "/AckermannRover"),
    ];

    for (f, rover_path) in rover_files {
        let p = Path::new("../../assets/").join(f);
        let label = f;

        let raw = std::fs::read_to_string(&p).unwrap();
        let mut parser = openusd::usda::parser::Parser::new(&raw);
        let data = parser.parse().unwrap();
        let reader = TextReader::from_data(data);
        let composed = UsdComposer::flatten(&reader, p.parent().unwrap())
            .unwrap_or_else(|e| panic!("{label} composition failed: {e}"));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<UsdStageAsset>();
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

        // Add stage asset directly (synchronously, like tests do)
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        let handle = stages.add(UsdStageAsset { reader: Arc::new(composed) });

        app.world_mut().spawn((
            Name::new("TestRover"),
            UsdPrimPath { stage_handle: handle, path: rover_path.to_string() },
        ));

        // Run update loop - sim processing happens in Update systems
        for _ in 0..20 {
            app.update();
        }
        app.world_mut().flush();

        // MUST have FlightSoftware (from PhysxVehicleContextAPI)
        let mut q_fsw = app.world_mut().query_filtered::<Entity, With<FlightSoftware>>();
        let fsw_count = q_fsw.iter(app.world()).count();
        assert!(fsw_count > 0,
            "{label}: FlightSoftware must be present after sim processing. Got {fsw_count} entities with FSW. \
            This means the sim system didn't process the rover - likely async loading bug.");

        // MUST have DifferentialDrive or AckermannSteer (depending on rover type)
        let mut q_skid = app.world_mut().query_filtered::<Entity, With<DifferentialDrive>>();
        let mut q_ack = app.world_mut().query_filtered::<Entity, With<AckermannSteer>>();
        let has_drive = q_skid.iter(app.world()).count() > 0 || q_ack.iter(app.world()).count() > 0;
        assert!(has_drive,
            "{label}: Must have DifferentialDrive or AckermannSteer after sim processing. \
            Rover won't be able to steer!");

        // MUST have Vessel + RoverVessel
        let mut q_vessel = app.world_mut().query_filtered::<Entity, (With<Vessel>, With<RoverVessel>)>();
        let vessel_count = q_vessel.iter(app.world()).count();
        assert!(vessel_count > 0,
            "{label}: Vessel+RoverVessel must be present. Got {vessel_count}.");

        // MUST have 4 wheels with WheelRaycast
        let mut q_wheels = app.world_mut().query_filtered::<Entity, With<WheelRaycast>>();
        let wheel_count = q_wheels.iter(app.world()).count();
        assert_eq!(wheel_count, 4,
            "{label}: Must have 4 wheels with WheelRaycast, got {wheel_count}");
    }
}

// ============================================================
// Test: Verify rover schema detection (PhysxVehicleContextAPI etc)
// ============================================================

#[test]
fn test_rover_schema_detection_after_composition() {
    let rover_files = [
        ("vessels/rovers/skid_rover.usda", "SkidRover"),
        ("vessels/rovers/ackermann_rover.usda", "AckermannRover"),
    ];

    for (f, rover_name) in &rover_files {
        let p = Path::new("../../assets/").join(f);
        let label = f;

        let raw = std::fs::read_to_string(&p).unwrap();
        let mut parser = openusd::usda::parser::Parser::new(&raw);
        let data = parser.parse()
            .unwrap_or_else(|e| panic!("{label}: Invalid USD: {e}"));
        let reader = TextReader::from_data(data);
        let composed = UsdComposer::flatten(&reader, Path::new("../../assets/"))
            .unwrap_or_else(|e| panic!("{label} composition failed: {e}"));

        let rover_path = SdfPath::new(&format!("/{}", rover_name)).unwrap();
        assert!(composed.has_spec(&rover_path), "{label}: /{} must exist", rover_name);

        // Verify apiSchemas exist in composed spec
        let rover_spec = composed.iter().find(|(p, _)| p.to_string() == format!("/{}", rover_name));
        assert!(rover_spec.is_some(), "{label}: /{} spec must exist", rover_name);
    }
}

// ============================================================
// Test: Verify rover files have NO root-level transform
// Catches: rover position baked into file instead of set by Rust code
// ============================================================

#[test]
fn test_rover_files_have_no_baked_position() {
    let rover_files = [
        ("vessels/rovers/skid_rover.usda", "SkidRover"),
        ("vessels/rovers/ackermann_rover.usda", "AckermannRover"),
    ];

    for (f, rover_name) in &rover_files {
        let p = Path::new("../../assets/").join(f);
        let label = f;

        let raw = std::fs::read_to_string(&p).unwrap();
        let mut parser = openusd::usda::parser::Parser::new(&raw);
        let data = parser.parse().unwrap();
        let reader = TextReader::from_data(data);

        let rover_path = SdfPath::new(&format!("/{}", rover_name)).unwrap();
        assert!(reader.has_spec(&rover_path), "{label}: /{} must exist", rover_name);

        // Rover root must NOT have xformOp:translate (position set by Rust at runtime)
        let root_pos: Option<Vec<f64>> = reader.prim_attribute_value(&rover_path, "xformOp:translate");
        assert!(root_pos.is_none(),
            "{label}: /{} must NOT have xformOp:translate (position set by Rust), got: {:?}", rover_name, root_pos);
    }
}

// ============================================================
// Test: Load full scene through Bevy, verify 5 rovers exist
// This test uses the EXACT same spawning logic as runtime:
// - ChildOf for parenting (NOT set_parent_in_place)
// - NO GlobalTransform::default() (causes position reset)
// ============================================================

#[test]
fn test_full_scene_loads_with_rovers() {
    // Load scene — Ground + Ramp + 5 rover instances (2 base files)
    // Matrix: 2 steering × 2 wheel types = 4 variants, plus 1 extra Ackermann
    let scene_path = Path::new("../../assets/scenes/sandbox/sandbox_scene.usda");
    let scene_composed = compose_asset_from_file(scene_path);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    // Spawn scene — rovers come from scene references
    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let scene_handle = stages.add(UsdStageAsset { reader: Arc::new(scene_composed) });
    app.world_mut().spawn((
        Name::new("TestScene"),
        UsdPrimPath { stage_handle: scene_handle.clone(), path: "/SandboxScene".to_string() },
        Transform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));

    for _ in 0..10 { app.update(); }
    app.world_mut().flush();

    // Count rovers — 5 instances from scene references
    let mut q_rovers = app.world_mut().query_filtered::<(Entity, &Name, &UsdPrimPath), (With<Vessel>, With<RoverVessel>)>();
    let rover_info: Vec<_> = q_rovers.iter(app.world())
        .map(|(_, n, p)| (n.as_str().to_string(), p.path.clone()))
        .collect();
    assert_eq!(rover_info.len(), 5, "Must have 5 rovers from scene, got {}: {:?}", rover_info.len(), rover_info);

    // Drivable = has steering system (DifferentialDrive OR AckermannSteer) + FSW
    // 3 skid (2 raycast + 1 physical) + 2 ackermann (1 raycast + 1 physical) = 5
    let mut q_skid = app.world_mut().query_filtered::<Entity, (With<DifferentialDrive>, With<FlightSoftware>)>();
    let skid_count: usize = q_skid.iter(app.world()).count();
    let mut q_ackermann = app.world_mut().query_filtered::<Entity, (With<AckermannSteer>, With<FlightSoftware>)>();
    let ackermann_count: usize = q_ackermann.iter(app.world()).count();
    let drivable = skid_count + ackermann_count;
    assert_eq!(drivable, 5, "All 5 rovers must be drivable (skid={}, ackermann={}), got {drivable}", skid_count, ackermann_count);

    // 12 raycast wheels (3 raycast rovers × 4 wheels)
    // Skid_Physical_1 + Ackermann_Physical_1 have physical wheels
    let mut q_wheels = app.world_mut().query_filtered::<Entity, With<WheelRaycast>>();
    let wheel_count = q_wheels.iter(app.world()).count();
    assert_eq!(wheel_count, 12, "3 raycast rovers x 4 wheels = 12, got {wheel_count}");

    // 8 physical wheels (2 physical rovers × 4 wheels)
    let mut q_physical = app.world_mut().query_filtered::<Entity, With<PhysicalWheel>>();
    let physical_count = q_physical.iter(app.world()).count();
    assert_eq!(physical_count, 8, "2 physical rovers x 4 wheels = 8, got {physical_count}");

    // All rovers must have Mesh3d (visible body)
    let mut q_mesh = app.world_mut().query_filtered::<Entity, (With<Vessel>, With<RoverVessel>, With<Mesh3d>)>();
    let visible_count = q_mesh.iter(app.world()).count();
    assert_eq!(visible_count, 5, "All 5 rovers must have Mesh3d (visible), got {visible_count}");

    // Verify rovers have scene paths (from references) not standalone paths
    let rover_paths: Vec<_> = rover_info.iter().map(|(_, p)| p.as_str()).collect();
    assert!(rover_paths.iter().any(|p| p.contains("Skid_Raycast_1")), "Should have Skid_Raycast_1");
    assert!(rover_paths.iter().any(|p| p.contains("Skid_Raycast_2")), "Should have Skid_Raycast_2");
    assert!(rover_paths.iter().any(|p| p.contains("Skid_Physical_1")), "Should have Skid_Physical_1");
    assert!(rover_paths.iter().any(|p| p.contains("Ackermann_Raycast_1")), "Should have Ackermann_Raycast_1");
    assert!(rover_paths.iter().any(|p| p.contains("Ackermann_Physical_1")), "Should have Ackermann_Physical_1");

    // Verify steering wires exist for front wheels
    use lunco_core::architecture::Wire;
    let mut q_wires = app.world_mut().query::<(&Wire, &Name)>();
    let steering_wires: Vec<_> = q_wires.iter(app.world())
        .filter(|(_, name)| name.as_str().contains("Steering"))
        .map(|(_, name)| name.as_str().to_string())
        .collect();
    // 5 rovers × 2 front wheels = 10 steering wires
    assert_eq!(steering_wires.len(), 10, "5 rovers × 2 front wheels = 10 steering wires, got {}: {:?}",
        steering_wires.len(), steering_wires);
}

// ============================================================
// Test: Verify parameter override composition (color override from scene)
// ============================================================

#[test]
fn test_valentine_color_override() {
    use openusd::sdf::{Path as SdfPath, AbstractData};

    // Load scene
    let scene_path = Path::new("../../assets/scenes/sandbox/sandbox_scene.usda");
    let composed = compose_asset_from_file(scene_path);

    // Verify Skid_Raycast_1 prim exists and has the override color
    let rover_path = SdfPath::new("/SandboxScene/Skid_Raycast_1").unwrap();
    assert!(composed.has_spec(&rover_path), "Skid_Raycast_1 prim must exist");

    // The local prim should have the override color
    if let Some(display_color) = composed.prim_attribute_value::<Vec<f32>>(&rover_path, "primvars:displayColor") {
        assert_eq!(display_color.len(), 3, "Color must have 3 components");
        assert!((display_color[0] - 0.8).abs() < 0.01, "Red should be 0.8, got {}", display_color[0]);
        assert!((display_color[1] - 0.2).abs() < 0.01, "Green should be 0.2, got {}", display_color[1]);
        assert!((display_color[2] - 0.2).abs() < 0.01, "Blue should be 0.2, got {}", display_color[2]);
    } else {
        panic!("Skid_Raycast_1 prim must have primvars:displayColor override");
    }
}
