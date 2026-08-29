use avian3d::prelude::*;
use bevy::asset::AssetPlugin;
/// Tests that verify USD rover files project the canonical mobility structure.
/// ALL tests load REAL files from disk — no inline USD strings.
use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_core::{MobilityRoot, OutputPorts};
use lunco_mobility::kernels::DriveMix;
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_usd_avian::*;
use lunco_usd_bevy::*;
use lunco_usd_sim::*;

/// The rover root carries `PhysicsRigidBodyAPI`, so avian builds a
/// `Collider::compound` from its child colliders. A compound is NOT
/// `as_cuboid()`. Extract the cuboid half-extents whether the collider is plain
/// or compound. A body may
/// have several authored collision children (for example a mounted battery), so
/// callers must select the shape they are asserting rather than assuming the
/// first compound entry is the chassis.
fn cuboid_half_extents(col: &Collider) -> Vec<[f32; 3]> {
    let shape = col.shape();
    if let Some(c) = shape.as_cuboid() {
        return vec![[
            c.half_extents.x as f32,
            c.half_extents.y as f32,
            c.half_extents.z as f32,
        ]];
    }
    if let Some(compound) = shape.as_compound() {
        return compound
            .shapes()
            .iter()
            .filter_map(|(_, shape)| shape.as_cuboid())
            .map(|c| {
                [
                    c.half_extents.x as f32,
                    c.half_extents.y as f32,
                    c.half_extents.z as f32,
                ]
            })
            .collect();
    }
    panic!(
        "collider is neither a cuboid nor a compound-of-cuboid: {:?}",
        shape.shape_type()
    );
}

/// After the Xform-root refactor the visible body mesh lives on the Chassis
/// CHILD, not the rover root (an `Xform`). Return that Chassis child entity.
fn chassis_child(app: &App, rover: Entity, label: impl std::fmt::Display) -> Entity {
    let kids = app
        .world()
        .get::<Children>(rover)
        .unwrap_or_else(|| panic!("{label}: rover missing Children"));
    kids.iter()
        .find(|&c| {
            app.world()
                .get::<Name>(c)
                .map(|n| n.as_str().contains("Chassis"))
                .unwrap_or(false)
        })
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
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, file_path);

    app.world_mut().spawn((
        Name::new("TestRover"),
        UsdPrimPath {
            stage_handle: handle,
            path: prim_path.to_string(),
        },
        Transform::from_translation(Vec3::new(-15.0, 5.0, -10.0)),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));

    for _ in 0..10 {
        app.update();
    }
    app.world_mut().flush();
    app
}

/// Headless simulation projection does not depend on a renderer or a timeout
/// fallback. This exercises the canonical physics realization without visual
/// components present.
///
#[test]
fn headless_server_builds_wheel_physics_without_renderer() {
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
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, file);
    app.world_mut().spawn((
        Name::new("HeadlessRover"),
        UsdPrimPath {
            stage_handle: handle,
            path: "/SkidRover".to_string(),
        },
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
        "headless server must build 4 WheelRaycast wheels; got {n}"
    );
}

/// Verify that ALL rover files loaded through the real pipeline produce the
/// canonical component structure.
#[test]
fn test_all_rover_files_project_canonical_structure() {
    let files = [
        ("vessels/rovers/skid_rover.usda", "/SkidRover", false),
        (
            "vessels/rovers/ackermann_rover.usda",
            "/AckermannRover",
            true,
        ),
    ];

    for (file, prim, ackermann) in &files {
        let label = file.to_string();
        let mut app = compose_and_load(&Path::new("../../assets/").join(file), prim);

        // Find rover
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<MobilityRoot>>();
        let rover = q
            .iter(app.world())
            .next()
            .unwrap_or_else(|| panic!("{label}: No MobilityRoot (rover root) entity"));

        // Physics. Born Kinematic + `ShouldBeDynamic` until joints resolve and
        // readiness promotes — with no terrain in this world the hold is the
        // correct terminal state, so destined-dynamic is the invariant.
        let rb = app
            .world()
            .get::<RigidBody>(rover)
            .unwrap_or_else(|| panic!("{label}: missing RigidBody"));
        assert!(
            *rb == RigidBody::Dynamic
                || app
                    .world()
                    .get::<lunco_usd_avian::ShouldBeDynamic>(rover)
                    .is_some(),
            "{label}: RigidBody must be Dynamic (or Kinematic-held via ShouldBeDynamic)"
        );

        let mass = app
            .world()
            .get::<Mass>(rover)
            .unwrap_or_else(|| panic!("{label}: missing Mass"));
        assert!(
            (mass.0 - 1000.0).abs() < 1.0,
            "{label}: Mass ~1000, got {}",
            mass.0
        );

        let ld = app
            .world()
            .get::<LinearDamping>(rover)
            .unwrap_or_else(|| panic!("{label}: missing LinearDamping"));
        assert!((ld.0 - 0.5).abs() < 0.1, "{label}: LinearDamping ~0.5");

        let ad = app
            .world()
            .get::<AngularDamping>(rover)
            .unwrap_or_else(|| panic!("{label}: missing AngularDamping"));
        assert!((ad.0 - 2.0).abs() < 0.1, "{label}: AngularDamping ~2.0");

        // Collider: the chassis cuboid is one authored member of the body's
        // compound shape; other authored rigid children may contribute peers.
        let col = app
            .world()
            .get::<Collider>(rover)
            .unwrap_or_else(|| panic!("{label}: missing Collider"));
        let he = cuboid_half_extents(col);
        assert!(
            he.iter().any(|[x, y, z]| {
                (x - 1.0).abs() < 0.1 && (y - 0.15).abs() < 0.05 && (z - 1.75).abs() < 0.1
            }),
            "{label}: compound collider is missing the authored chassis cuboid; got {he:?}"
        );

        // Visual — body mesh + material live on the Chassis child.
        let chassis = chassis_child(&app, rover, &label);
        assert!(
            app.world().get::<Mesh3d>(chassis).is_some(),
            "{label}: Chassis missing Mesh3d (body invisible!)"
        );
        // Appearance INTENT, not a bound material: `LuncoRenderPlugin` (absent in a
        // headless test) is what turns `PbrLook` into `MeshMaterial3d`.
        // See docs/architecture/render-decoupling.md.
        assert!(
            app.world().get::<lunco_render::PbrLook>(chassis).is_some(),
            "{label}: Chassis missing PbrLook (body would be invisible!)"
        );

        // Steering allocation: every rover carries a `DriveMix` naming a kernel.
        let mix = app
            .world()
            .get::<DriveMix>(rover)
            .unwrap_or_else(|| panic!("{label}: missing DriveMix"));
        if *ackermann {
            assert_eq!(
                mix.kernel, "linear",
                "{label}: ackermann should use the linear kernel"
            );
            assert!(
                mix.entries.iter().any(|e| e.port == "steering"),
                "{label}: missing steering term"
            );
            assert!(
                mix.entries.iter().any(|e| e.port == "drive_left"),
                "{label}: missing drive_left term"
            );
        } else {
            assert_eq!(
                mix.kernel, "skid",
                "{label}: skid rover should use the skid kernel"
            );
            assert_eq!(
                mix.ports,
                vec!["drive_left".to_string(), "drive_right".to_string()],
                "{label}: wrong skid ports"
            );
        }

        // Actuator ports
        let actuators = app
            .world()
            .get::<OutputPorts>(rover)
            .unwrap_or_else(|| panic!("{label}: missing OutputPorts"));
        assert!(
            actuators.ports.contains_key("drive_left"),
            "{label}: actuators missing drive_left"
        );
        assert!(
            actuators.ports.contains_key("drive_right"),
            "{label}: actuators missing drive_right"
        );
        assert!(
            actuators.ports.contains_key("steering"),
            "{label}: actuators missing steering"
        );
        assert!(
            actuators.ports.contains_key("brake"),
            "{label}: actuators missing brake"
        );

        // Wheels are nested under their authored suspension carrier. Resolve
        // them from the mobility API component, not from Bevy names or tree
        // depth. `WheelRaycast` is the authoritative realization marker.
        let wheels: Vec<Entity> = {
            let mut query = app
                .world_mut()
                .query_filtered::<Entity, With<WheelRaycast>>();
            query.iter(app.world()).collect()
        };

        assert_eq!(
            wheels.len(),
            4,
            "{label}: must have 4 authored raycast wheels, got {}",
            wheels.len()
        );

        // Wheels author `axis = "X"`, so the visual child carries the
        // cylinder-axis rotation `from_rotation_arc(Y, X)` (= −90° about Z;
        // the live log shows `Quat(0,0,-0.707,0.707)`).
        let expected_rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X);
        let expected_positions = [
            ("Suspension_FL/Wheel_FL", Vec3::new(-1.0, -0.65, -1.225)),
            ("Suspension_FR/Wheel_FR", Vec3::new(1.0, -0.65, -1.225)),
            ("Suspension_RL/Wheel_RL", Vec3::new(-1.0, -0.65, 1.225)),
            ("Suspension_RR/Wheel_RR", Vec3::new(1.0, -0.65, 1.225)),
        ];

        for (wheel_index, (relative_path, exp_pos)) in expected_positions.iter().enumerate() {
            let expected_path = format!("{prim}/{relative_path}");
            let w_ent = *wheels
                .iter()
                .find(|&&entity| {
                    app.world()
                        .get::<UsdPrimPath>(entity)
                        .is_some_and(|path| path.path == expected_path)
                })
                .unwrap_or_else(|| {
                    panic!("{label}: no WheelRaycast for authored path {expected_path}")
                });
            let w_name = format!("wheel #{wheel_index}");

            let wheel = app
                .world()
                .get::<WheelRaycast>(w_ent)
                .expect("WheelRaycast query returned an entity without WheelRaycast");
            assert!(
                (wheel.wheel_radius - 0.4).abs() < 0.01,
                "{label}: {w_name} radius ~0.4"
            );
            let susp = app
                .world()
                .get::<Suspension>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing Suspension"));
            assert!(
                (susp.rest_length - 0.7).abs() < 0.01,
                "{label}: {w_name} rest ~0.7"
            );
            assert!(
                (susp.spring_k - 15000.0).abs() < 100.0,
                "{label}: {w_name} spring_k ~15000"
            );
            assert!(
                (susp.damping_c - 3000.0).abs() < 100.0,
                "{label}: {w_name} damping_c ~3000"
            );

            assert!(
                app.world().get::<RigidBody>(w_ent).is_none(),
                "{label}: {w_name} must NOT have RigidBody"
            );
            assert!(
                app.world().get::<Collider>(w_ent).is_none(),
                "{label}: {w_name} must NOT have Collider"
            );

            // Wheel entity (physics) should have identity rotation for correct raycasting.
            // The visual rotation is on a child entity.
            let wt = app
                .world()
                .get::<Transform>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing Transform"));
            assert_eq!(
                wt.translation,
                Vec3::ZERO,
                "{label}: {w_name} wheel prim must use its authored zero local offset"
            );

            let mount = app
                .world()
                .get::<ChildOf>(w_ent)
                .unwrap_or_else(|| panic!("{label}: {w_name} missing authored suspension parent"))
                .parent();
            let mount_tf = app
                .world()
                .get::<Transform>(mount)
                .unwrap_or_else(|| panic!("{label}: {w_name} suspension parent lacks Transform"));
            assert!(
                mount_tf.translation.distance_squared(*exp_pos) < 0.0001,
                "{label}: {w_name} suspension mount {:?}, expected {:?}",
                mount_tf.translation,
                exp_pos
            );

            // Physics entity must have identity rotation (rays go down, not sideways)
            assert!(
                wt.rotation.angle_between(Quat::IDENTITY).abs() < 0.01,
                "{label}: {w_name} physics entity must have identity rotation, got {:?}",
                wt.rotation
            );

            // The mobility API owns the visual link; do not rediscover it by
            // scanning child names.
            let visual = wheel
                .visual_entity
                .unwrap_or_else(|| panic!("{label}: {w_name} has no visual entity link"));
            let visual_tf = app
                .world()
                .get::<Transform>(visual)
                .unwrap_or_else(|| panic!("{label}: {w_name} visual entity lacks Transform"));
            assert!(
                visual_tf.rotation.angle_between(expected_rot).abs() < 0.01,
                "{label}: {w_name} visual rotation is {:?}, expected {:?}",
                visual_tf.rotation,
                expected_rot
            );
        }
    }
}
