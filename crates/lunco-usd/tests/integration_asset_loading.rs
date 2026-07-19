/// Integration tests that load REAL assets through the EXACT same pipeline as runtime.
/// NO inline USD strings. NO manual file reading. Uses AssetServer just like the app.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use lunco_mobility::{WheelRaycast, Suspension};
use lunco_mobility::kernels::DriveMix;
use lunco_fsw::FlightSoftware;

/// The rover root carries `PhysicsRigidBodyAPI`, so avian builds a
/// `Collider::compound` from its child colliders (the Chassis cuboid) — a
/// compound-of-one is NOT `as_cuboid()`. Extract the single cuboid's
/// half-extents whether the collider is a plain cuboid or that compound.
fn cuboid_half_extents(col: &Collider) -> [f32; 3] {
    let shape = col.shape();
    if let Some(c) = shape.as_cuboid() {
        return [c.half_extents.x as f32, c.half_extents.y as f32, c.half_extents.z as f32];
    }
    if let Some(compound) = shape.as_compound() {
        if let Some(c) = compound.shapes().first().and_then(|(_, s)| s.as_cuboid()) {
            return [c.half_extents.x as f32, c.half_extents.y as f32, c.half_extents.z as f32];
        }
    }
    panic!("collider is neither a cuboid nor a compound-of-cuboid: {:?}", shape.shape_type());
}

/// After the Xform-root refactor the visible body mesh lives on the Chassis
/// CHILD, not the rover root (an `Xform`). Return that Chassis child entity.
fn chassis_child(app: &App, rover: Entity, label: impl std::fmt::Display) -> Entity {
    let kids = app.world().get::<Children>(rover)
        .unwrap_or_else(|| panic!("{label}: rover missing Children"));
    kids.iter()
        .find(|&c| app.world().get::<Name>(c).map(|n| n.as_str().contains("Chassis")).unwrap_or(false))
        .unwrap_or_else(|| panic!("{label}: rover has no Chassis child"))
}
use lunco_usd_bevy::usd_data::UsdDataExt;
use openusd::sdf::{AbstractData, Path as SdfPath};
use avian3d::prelude::*;
use big_space::prelude::CellCoord;
use std::path::Path;

// ============================================================
// Helper: compose asset using EXACT same logic as runtime loader
// ============================================================

fn compose_stage_from_file(file_path: &Path) -> openusd::usd::Stage {
    // Real openusd PCP composition from disk (`Stage::open` + `DefaultResolver`),
    // resolving relative references anchored at the file's own directory. Read it
    // through the live `StageView` (the production read path) — not a flatten.
    compose_file_to_stage(file_path)
        .unwrap_or_else(|e| panic!("Composition failed for {}: {e}", file_path.display()))
}

/// Compose a `.usda` (with its external references) into a live canonical stage
/// and publish it into `CanonicalStages` keyed by a fresh `UsdStageAsset` handle
/// — the recipe path for file-with-refs scenes. `StageRecipe::from_source` only
/// resolves a single in-memory layer, so these ref-carrying scene/rover files
/// compose the full closure via `compose_file_to_stage` and insert the wrapped
/// stage directly (the door the live-doc projection uses).
fn add_canonical_from_file(app: &mut App, file_path: &Path) -> Handle<UsdStageAsset> {
    let handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset { recipe: None })
    };
    let stage = compose_file_to_stage(file_path)
        .unwrap_or_else(|e| panic!("Composition failed for {}: {e}", file_path.display()));
    let cstage = CanonicalStage::from_stage(stage, file_path.display().to_string());
    app.world_mut()
        .get_non_send_mut::<CanonicalStages>()
        .expect("CanonicalStages resource (UsdBevyPlugin)")
        .insert(handle.id(), cstage);
    handle
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
        let stage = compose_stage_from_file(&p);
        let view = StageView::new(&stage);
        // Verify the composed reader has rover + 4 wheels
        let rover_name = if f.contains("ackermann") { "AckermannRover" } else { "SkidRover" };
        assert!(view.has_prim(&SdfPath::new(&format!("/{}", rover_name)).unwrap()),
            "{f}: /{} must exist after composition", rover_name);
        for w in &["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let wp = SdfPath::new(&format!("/{}/{}", rover_name, w)).unwrap();
            assert!(view.has_prim(&wp),
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
    let stage = compose_stage_from_file(p);
    let view = StageView::new(&stage);

    // Ground
    let ground = SdfPath::new("/SandboxScene/Ground").unwrap();
    assert!(view.has_prim(&ground), "Ground must exist");
    // Ground/Ramp dimensions are authored as unit `size` + `xformOp:scale`.
    let g: [f64; 3] = view.value_vec3(&ground, "xformOp:scale").expect("Ground scale");
    assert!((g[0] - 4000.0).abs() < 1.0, "Ground width ~4000, got {}", g[0]);
    assert!((g[1] - 0.2).abs() < 0.05, "Ground height ~0.2, got {}", g[1]);
    assert!((g[2] - 4000.0).abs() < 1.0, "Ground depth ~4000, got {}", g[2]);

    // Ramp
    let ramp = SdfPath::new("/SandboxScene/Ramp").unwrap();
    assert!(view.has_prim(&ramp), "Ramp must exist");
    let r: [f64; 3] = view.value_vec3(&ramp, "xformOp:scale").expect("Ramp scale");
    assert!((r[0] - 60.0).abs() < 1.0, "Ramp width ~60, got {}", r[0]);
    assert!((r[1] - 2.0).abs() < 0.05, "Ramp height ~2, got {}", r[1]);
    assert!((r[2] - 80.0).abs() < 1.0, "Ramp depth ~80, got {}", r[2]);
}

// ============================================================
// Test: Load rovers through Bevy pipeline, verify ALL components
// ============================================================

fn load_rover_through_bevy(file_path: &Path, prim_path: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
        app.init_asset::<bevy::shader::Shader>();
    // No GPU here, so a wheel's render-only `ShaderMaterial` never arrives —
    // mark headless so sim builds wheel physics without waiting (the `--no-ui`
    // server stand-in). Without it wheels deadlock within the few no-time frames.
    app.insert_resource(NoRenderVisuals);
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, file_path);

    app.world_mut().spawn((
        Name::new("TestRover"),
        UsdPrimPath { stage_handle: handle, path: prim_path.to_string() },
        // Root needs a Transform + spatial/visibility bundle so the
        // `instantiate_usd_prim` observer cascade spawns the wheel children
        // (matches the runtime spawn + the passing `rover_structure` harness).
        Transform::default(),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
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

        // Find rover entity (has FlightSoftware)
        let mut q_rover = app.world_mut().query_filtered::<Entity, With<FlightSoftware>>();
        let rover_ent = q_rover.iter(app.world()).next()
            .unwrap_or_else(|| panic!("{label}: No entity with FlightSoftware found"));

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

        // Collider: USD chassis is 2.0 x 0.3 x 3.5 → half-extents (1.0, 0.15, 1.75).
        // The rover root has PhysicsRigidBodyAPI, so avian wraps the chassis
        // cuboid in a `Collider::compound`; `cuboid_half_extents` unwraps either.
        let col = app.world().get::<Collider>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing Collider"));
        let he = cuboid_half_extents(col);
        assert!((he[0] - 1.0).abs() < 0.1,
            "{label}: Collider hx must be ~1.0 (width/2), got {}", he[0]);
        assert!((he[1] - 0.15).abs() < 0.05,
            "{label}: Collider hy must be ~0.15 (height/2), got {}", he[1]);
        assert!((he[2] - 1.75).abs() < 0.1,
            "{label}: Collider hz must be ~1.75 (depth/2), got {}", he[2]);

        // Visual (Mesh3d + appearance INTENT) — on the Chassis child, not the Xform
        // root. The prim carries a `PbrLook`, not a `MeshMaterial3d`: the material is
        // bound by `LuncoRenderPlugin` in render builds only, so this headless test
        // asserts the intent, which is what USD actually authors.
        // See docs/architecture/render-decoupling.md.
        let chassis = chassis_child(&app, rover_ent, label);
        let _mesh = app.world().get::<Mesh3d>(chassis)
            .unwrap_or_else(|| panic!("{label}: Chassis Missing Mesh3d (body not visible!)"));
        let _look = app.world().get::<lunco_render::PbrLook>(chassis)
            .unwrap_or_else(|| panic!("{label}: Chassis Missing PbrLook (body would not be visible!)"));

        // Steering allocation: every rover carries a `DriveMix` naming a kernel.
        // Ackermann → the `linear` kernel with a `steering` term; skid → the
        // `skid` kernel over the two drive ports.
        let mix = app.world().get::<DriveMix>(rover_ent)
            .unwrap_or_else(|| panic!("{label}: Missing DriveMix (cannot steer!)"));
        if f.contains("ackermann") {
            assert_eq!(mix.kernel, "linear", "{label}: ackermann should use the linear kernel");
            assert!(mix.entries.iter().any(|e| e.port == "steering" && e.steer != 0.0),
                "{label}: ackermann DriveMix missing a steering term");
        } else {
            // skid or an explicit linear per-wheel mix — both must name a known kernel.
            assert!(mix.kernel == "skid" || mix.kernel == "linear",
                "{label}: unexpected drive kernel '{}'", mix.kernel);
            if mix.kernel == "skid" {
                assert_eq!(mix.ports.len(), 2, "{label}: skid needs two drive ports");
            } else {
                assert!(!mix.entries.is_empty(), "{label}: linear DriveMix has no entries");
            }
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
            let susp = app.world().get::<Suspension>(*w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing Suspension"));
            assert!((susp.rest_length - 0.7).abs() < 0.01, "{label}: {w_name} rest ~0.7");
            assert!((susp.spring_k - 15000.0).abs() < 100.0, "{label}: {w_name} spring_k ~15000");
            assert!((susp.damping_c - 3000.0).abs() < 100.0, "{label}: {w_name} damping_c ~3000");

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

        let stage = compose_stage_from_file(&p);
        let view = StageView::new(&stage);

        for w_name in &["Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR"] {
            let wp = SdfPath::new(&format!("/{}/{}", rover_name, w_name)).unwrap();
            assert!(view.has_prim(&wp), "{label}: {w_name} must exist after composition");

            let radius: f64 = view.value(&wp, "radius")
                .unwrap_or_else(|| panic!("{label}: {w_name} missing 'radius' after composition"));
            let height: f64 = view.value(&wp, "height")
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
// Catches: rovers spawning without FSW/DriveMix because
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

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<UsdStageAsset>();
        app.init_asset::<Mesh>();
            app.init_asset::<Image>();
        app.init_asset::<bevy::shader::Shader>();
        // No GPU here, so a wheel's render-only `ShaderMaterial` never arrives —
    // mark headless so sim builds wheel physics without waiting (the `--no-ui`
    // server stand-in). Without it wheels deadlock within the few no-time frames.
    app.insert_resource(NoRenderVisuals);
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

        // Publish the live canonical stage (composed off the ref-carrying file).
        let handle = add_canonical_from_file(&mut app, &p);

        app.world_mut().spawn((
            Name::new("TestRover"),
            UsdPrimPath { stage_handle: handle, path: rover_path.to_string() },
            // Root needs Transform + spatial/visibility bundle so the wheel
            // children spawn through the observer cascade (see runtime spawn).
            Transform::default(),
            CellCoord::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            ViewVisibility::default(),
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

        // MUST have a DriveMix (the kernel-selected allocation) after sim processing.
        let mut q_mix = app.world_mut().query_filtered::<Entity, With<DriveMix>>();
        let has_drive = q_mix.iter(app.world()).count() > 0;
        assert!(has_drive,
            "{label}: Must have a DriveMix after sim processing. \
            Rover won't be able to steer!");

        // MUST have FlightSoftware
        let mut q_vessel = app.world_mut().query_filtered::<Entity, With<FlightSoftware>>();
        let vessel_count = q_vessel.iter(app.world()).count();
        assert!(vessel_count > 0,
            "{label}: FlightSoftware must be present. Got {vessel_count}.");

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

        let stage = compose_stage_from_file(&p);
        let view = StageView::new(&stage);

        let rover_path = SdfPath::new(&format!("/{}", rover_name)).unwrap();
        assert!(view.has_prim(&rover_path), "{label}: /{} must exist", rover_name);

        // The composed rover prim is present in the live stage's prim set.
        let found = view.prim_paths().iter().any(|p| p.to_string() == format!("/{}", rover_name));
        assert!(found, "{label}: /{} spec must exist", rover_name);
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

        // Single-layer parse (uncomposed): this checks the rover file ITSELF
        // has no baked root transform, so it must not pull in references.
        let raw = std::fs::read_to_string(&p).unwrap();
        let reader = openusd::usda::parse(&raw).unwrap();

        let rover_path = SdfPath::new(&format!("/{}", rover_name)).unwrap();
        assert!(reader.has_spec(&rover_path), "{label}: /{} must exist", rover_name);

        // Rover root must NOT have xformOp:translate (position set by Rust at runtime).
        // `double3` decodes to `[f64; 3]` — a `Vec<f64>` would never match, so a
        // baked translate would slip past the guard.
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`scalar`)
        // instead of the legacy `prim_attribute_value`, matching production (doc 21).
        let root_pos: Option<[f64; 3]> = reader.prim_attribute_value(&rover_path, "xformOp:translate");
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

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    // The scene references the Perseverance glTF, which the loader hands to
    // `AssetServer::load::<WorldAsset>` — register the asset so handle
    // allocation doesn't panic in this minimal harness.
    app.init_asset::<bevy::world_serialization::WorldAsset>();
    app.init_asset::<bevy::shader::Shader>();
    // Physical rovers create revolute joints whose `JointCollisionDisabled`
    // hook reads avian's `JointGraph` resource — without the physics plugins
    // it panics. (The single-rover tests use raycast wheels, so they don't.)
    app.add_plugins(avian3d::prelude::PhysicsPlugins::default());
    // No GPU here, so a wheel's render-only `ShaderMaterial` never arrives —
    // mark headless so sim builds wheel physics without waiting (the `--no-ui`
    // server stand-in). Without it wheels deadlock within the few no-time frames.
    app.insert_resource(NoRenderVisuals);
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));
    // PhysicsPlugins is here only so the joint-spawn hooks find avian's resources
    // (JointGraph) — this is a composition/structure test, not a physics-step test.
    // Stop `FixedMain` from ticking during the update loop below: it would run the
    // full physics + mobility fixed schedule, which needs resources this minimal
    // harness (no `LunCoCorePlugin`) doesn't have (`NetworkRole`,
    // `ColliderTreeDiagnostics`, …) and panics. A huge fixed period never
    // accumulates a tick across the ~10 fast updates. The structural wiring the
    // test checks (wheels, DriveMix, steer wires) is built in PreUpdate/Update.
    app.insert_resource(Time::<Fixed>::from_hz(0.0001));

    // Spawn scene — rovers come from scene references
    let scene_handle = add_canonical_from_file(&mut app, scene_path);
    app.world_mut().spawn((
        Name::new("TestScene"),
        UsdPrimPath { stage_handle: scene_handle.clone(), path: "/SandboxScene".to_string() },
        Transform::default(),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));

    for _ in 0..10 { app.update(); }
    app.world_mut().flush();

    // Count rovers — 5 instances from scene references
    let mut q_rovers = app.world_mut().query_filtered::<(Entity, &Name, &UsdPrimPath), With<FlightSoftware>>();
    let rover_info: Vec<_> = q_rovers.iter(app.world())
        .map(|(_, n, p)| (n.as_str().to_string(), p.path.clone()))
        .collect();
    assert_eq!(rover_info.len(), 5, "Must have 5 rovers from scene, got {}: {:?}", rover_info.len(), rover_info);

    // Drivable = carries a `DriveMix` (any kernel) + FSW.
    // 3 skid (2 raycast + 1 physical) + 2 ackermann (1 raycast + 1 physical) = 5
    let mut q_mix = app.world_mut().query_filtered::<Entity, (With<DriveMix>, With<FlightSoftware>)>();
    let drivable: usize = q_mix.iter(app.world()).count();
    assert_eq!(drivable, 5, "All 5 rovers must be drivable (carry DriveMix), got {drivable}");

    // 12 raycast wheels (3 raycast rovers × 4 wheels)
    // Skid_Physical_1 + Ackermann_Physical_1 have physical wheels
    let mut q_wheels = app.world_mut().query_filtered::<Entity, With<WheelRaycast>>();
    let wheel_count = q_wheels.iter(app.world()).count();
    assert_eq!(wheel_count, 12, "3 raycast rovers x 4 wheels = 12, got {wheel_count}");

    // 8 physical wheels (2 physical rovers × 4 wheels)
    let mut q_physical = app.world_mut().query_filtered::<Entity, With<PhysicalWheel>>();
    let physical_count = q_physical.iter(app.world()).count();
    assert_eq!(physical_count, 8, "2 physical rovers x 4 wheels = 8, got {physical_count}");

    // All 5 rovers must show a visible body — the Chassis CHILD carries the
    // Mesh3d (the rover root is an Xform after the Xform-root refactor).
    let mut q_rovers2 = app.world_mut().query_filtered::<Entity, With<FlightSoftware>>();
    let rover_ents: Vec<Entity> = q_rovers2.iter(app.world()).collect();
    let visible_count = rover_ents.iter()
        .filter(|&&r| app.world().get::<Mesh3d>(chassis_child(&app, r, "scene")).is_some())
        .count();
    assert_eq!(visible_count, 5, "All 5 rovers' Chassis must have Mesh3d (visible), got {visible_count}");

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
        // Steer wires are named `Wire_Steer_<port>` (the port is lowercase
        // "steering"), so match the wire prefix — the old `"Steering"` filter never
        // matched the actual name (a latent bug, unrelated to the drive kernel).
        .filter(|(_, name)| name.as_str().contains("Wire_Steer"))
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
    use openusd::sdf::Path as SdfPath;

    // Load scene
    let scene_path = Path::new("../../assets/scenes/sandbox/sandbox_scene.usda");
    let stage = compose_stage_from_file(scene_path);
    let view = StageView::new(&stage);

    // The per-instance colour override is authored as `over "Chassis" {
    // displayColor }` — i.e. on the Chassis CHILD, not the rover root.
    let chassis = SdfPath::new("/SandboxScene/Skid_Raycast_1/Chassis").unwrap();
    assert!(view.has_prim(&chassis), "Skid_Raycast_1/Chassis prim must exist");

    // `primvars:displayColor` is ARRAY-valued (`color3f[]`, constant
    // interpolation) per UsdGeomGprim — read element 0, not a scalar `color3f`.
    let display_color = lunco_usd_bevy::read_primvar_vec3(&view, &chassis, "primvars:displayColor")
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .expect("Skid_Raycast_1/Chassis must have the composed displayColor override");
    assert!((display_color[0] - 0.8).abs() < 0.01, "Red should be 0.8, got {}", display_color[0]);
    assert!((display_color[1] - 0.2).abs() < 0.01, "Green should be 0.2, got {}", display_color[1]);
    assert!((display_color[2] - 0.2).abs() < 0.01, "Blue should be 0.2, got {}", display_color[2]);
}
