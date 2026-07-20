/// Tests that verify USD rover files match the procedural rover definition.
/// ALL tests load REAL files from disk — no inline USD strings.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use big_space::prelude::CellCoord;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use lunco_mobility::{WheelRaycast, Suspension};
use lunco_mobility::kernels::DriveMix;
use avian3d::prelude::*;
use lunco_core::ActuatorPorts;

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
use std::path::Path;

/// Build the live canonical stage for a rover `.usda` (which references
/// `wheel.usda` / drivetrain sublayers) and publish it into `CanonicalStages`
/// keyed by a fresh `UsdStageAsset` handle. File-with-external-refs scenes can't
/// use `StageRecipe::from_source` (a lone in-memory layer won't resolve the
/// refs), so we compose the full closure via `compose_file_to_stage` and insert
/// the wrapped stage directly — the same door the live-doc projection uses.
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

fn compose_and_load(file_path: &Path, prim_path: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.init_asset::<bevy::shader::Shader>();
    // No GPU in this harness, so a wheel's render-only `ShaderMaterial` never
    // arrives — mark the app headless so sim builds wheel PHYSICS without waiting
    // (the faithful `--no-ui` server stand-in; the visual mesh child still builds
    // via the visual extractor). Without this the wheels deadlock and never gain
    // `WheelRaycast` within the test's few no-time-advance frames.
    app.insert_resource(NoRenderVisuals);
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, file_path);

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

/// HEADLESS-PARITY GUARD — regression test for the wheel-shader deadlock.
///
/// The `--no-ui` server never adds `LuncoRenderPlugin`, so **no material of any
/// kind is ever bound** — `lunco-render-bevy` is the only crate that names one.
/// The wheel PHYSICS must still build, or the authoritative server can never
/// simulate or replicate a drivable rover: every wheel would deadlock
/// `Without<UsdSimProcessed>` forever. That is what once shipped to the server.
///
/// This app reproduces the server's shape: **no render plugin, so no material
/// binder** + `NoRenderVisuals` inserted.
///
/// NOTE (post render-decoupling): what `process_usd_sim_prims` waits on is now
/// `Mesh3d` / `PbrLook` / `ShaderLook` — all render-FREE intent, all authored
/// headless — so the original deadlock is structurally unreachable rather than
/// merely fixed. This test is kept as the guard that it stays that way: if
/// anyone re-couples the physics build to a GPU-side material, it fails here.
#[test]
fn headless_server_builds_wheel_physics_without_shader_material() {
    let file = Path::new("../../assets/vessels/rovers/skid_rover.usda");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.init_asset::<bevy::shader::Shader>();
    // DELIBERATELY no `LuncoRenderPlugin` — that is the ONLY thing that binds a
    // material, so its absence is exactly what makes this app a faithful stand-in
    // for the `--no-ui` server.
    app.insert_resource(NoRenderVisuals); // the fix under test
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, file);
    app.world_mut().spawn((
        Name::new("HeadlessRover"),
        UsdPrimPath { stage_handle: handle, path: "/SkidRover".to_string() },
        Transform::default(),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));

    for _ in 0..10 {
        app.update();
    }
    app.world_mut().flush();

    let mut q = app.world_mut().query::<&WheelRaycast>();
    let n = q.iter(app.world()).count();
    assert_eq!(
        n, 4,
        "headless server (no ShaderMaterial) must still build 4 WheelRaycast wheels; \
         got {n} — wheels deadlocked waiting on a render-only material the server never produces"
    );
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
        let mut q = app.world_mut().query_filtered::<Entity, With<ActuatorPorts>>();
        let rover = q.iter(app.world()).next()
            .unwrap_or_else(|| panic!("{label}: No ActuatorPorts (rover root) entity"));

        // Physics
        let rb = app.world().get::<RigidBody>(rover).expect(&format!("{label}: missing RigidBody"));
        assert_eq!(*rb, RigidBody::Dynamic, "{label}: RigidBody must be Dynamic");

        let mass = app.world().get::<Mass>(rover).expect(&format!("{label}: missing Mass"));
        assert!((mass.0 - 1000.0).abs() < 1.0, "{label}: Mass ~1000, got {}", mass.0);

        let ld = app.world().get::<LinearDamping>(rover).expect(&format!("{label}: missing LinearDamping"));
        assert!((ld.0 - 0.5).abs() < 0.1, "{label}: LinearDamping ~0.5");

        let ad = app.world().get::<AngularDamping>(rover).expect(&format!("{label}: missing AngularDamping"));
        assert!((ad.0 - 2.0).abs() < 0.1, "{label}: AngularDamping ~2.0");

        // Collider (compound-of-one cuboid built from the Chassis child)
        let col = app.world().get::<Collider>(rover).expect(&format!("{label}: missing Collider"));
        let he = cuboid_half_extents(col);
        assert!((he[0] - 1.0).abs() < 0.1, "{label}: hx ~1.0, got {}", he[0]);
        assert!((he[1] - 0.15).abs() < 0.05, "{label}: hy ~0.15, got {}", he[1]);
        assert!((he[2] - 1.75).abs() < 0.1, "{label}: hz ~1.75, got {}", he[2]);

        // Visual — body mesh + material live on the Chassis child.
        let chassis = chassis_child(&app, rover, &label);
        assert!(app.world().get::<Mesh3d>(chassis).is_some(), "{label}: Chassis missing Mesh3d (body invisible!)");
        // Appearance INTENT, not a bound material: `LuncoRenderPlugin` (absent in a
        // headless test) is what turns `PbrLook` into `MeshMaterial3d`.
        // See docs/architecture/render-decoupling.md.
        assert!(app.world().get::<lunco_render::PbrLook>(chassis).is_some(),
            "{label}: Chassis missing PbrLook (body would be invisible!)");

        // Steering allocation: every rover carries a `DriveMix` naming a kernel.
        let mix = app.world().get::<DriveMix>(rover).expect(&format!("{label}: missing DriveMix"));
        if file.contains("ackermann") {
            assert_eq!(mix.kernel, "linear", "{label}: ackermann should use the linear kernel");
            assert!(mix.entries.iter().any(|e| e.port == "steering"), "{label}: missing steering term");
            assert!(mix.entries.iter().any(|e| e.port == "drive_left"), "{label}: missing drive_left term");
        } else {
            assert_eq!(mix.kernel, "skid", "{label}: skid rover should use the skid kernel");
            assert_eq!(mix.ports, vec!["drive_left".to_string(), "drive_right".to_string()], "{label}: wrong skid ports");
        }

        // Actuator ports
        let actuators = app.world().get::<ActuatorPorts>(rover).expect(&format!("{label}: missing ActuatorPorts"));
        assert!(actuators.ports.contains_key("drive_left"), "{label}: actuators missing drive_left");
        assert!(actuators.ports.contains_key("drive_right"), "{label}: actuators missing drive_right");
        assert!(actuators.ports.contains_key("steering"), "{label}: actuators missing steering");
        assert!(actuators.ports.contains_key("brake"), "{label}: actuators missing brake");

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

        // Wheels author `axis = "X"`, so the visual child carries the
        // cylinder-axis rotation `from_rotation_arc(Y, X)` (= −90° about Z;
        // the live log shows `Quat(0,0,-0.707,0.707)`).
        let expected_rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X);
        let expected_positions = [
            ("Wheel_FL", Vec3::new(-1.0, -0.15, -1.225)),
            ("Wheel_FR", Vec3::new(1.0, -0.15, -1.225)),
            ("Wheel_RL", Vec3::new(-1.0, -0.15, 1.225)),
            ("Wheel_RR", Vec3::new(1.0, -0.15, 1.225)),
        ];

        for (w_name, exp_pos) in &expected_positions {
            let found = wheels.iter()
                .find(|(_, n)| n.contains(w_name))
                .unwrap_or_else(|| panic!("{label}: missing {w_name}"));
            let w_ent = found.0;

            let wheel = app.world().get::<WheelRaycast>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing WheelRaycast"));
            assert!((wheel.wheel_radius - 0.4).abs() < 0.01, "{label}: {w_name} radius ~0.4");
            let susp = app.world().get::<Suspension>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing Suspension"));
            assert!((susp.rest_length - 0.7).abs() < 0.01, "{label}: {w_name} rest ~0.7");
            assert!((susp.spring_k - 15000.0).abs() < 100.0, "{label}: {w_name} spring_k ~15000");
            assert!((susp.damping_c - 3000.0).abs() < 100.0, "{label}: {w_name} damping_c ~3000");

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
