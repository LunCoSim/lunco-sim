//! End-to-end: a runtime-spawned body's RENDER follows its PHYSICS over time.
//!
//! Drives the real avian solver (with `interpolate_all`, as production does) over
//! a real big_space world root, and asserts the spawned body's `Transform` tracked
//! its `Position` after ~2 s — the observable behaviour, not a component shape.
//!
//! SCOPE, honestly: this does NOT reproduce the intermittent "spawned body's mesh
//! freezes at its spawn pose while physics climbs" symptom seen in the running
//! app, so treat it as a smoke test for the sync path rather than a guard for
//! that specific freeze. The anchoring shape itself is pinned in
//! `catalog::spawn_anchor_tests`.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use lunco_scene_commands::catalog::{spawn_usd_entry, SpawnAnchor, SpawnSource, SpawnableEntry};
use lunco_usd_bevy::{UsdInstanceRoot, UsdStageAsset};
use std::time::Duration;

#[derive(Resource)]
struct Args {
    entry: SpawnableEntry,
    scene_root: Entity,
}

const SPAWN_Y: f32 = 2.0;
/// Constant climb rate (gravity is off), so the expected trajectory is unambiguous.
const RISE_M_PER_S: f64 = 10.0;

fn spawn_once(mut commands: Commands, assets: Res<AssetServer>, args: Res<Args>) {
    spawn_usd_entry(
        &mut commands,
        &assets,
        &args.entry,
        Vec3::new(0.0, SPAWN_Y, 0.0),
        Quat::IDENTITY,
        SpawnAnchor::scene_root(args.scene_root),
    );
}

fn balloon_entry() -> SpawnableEntry {
    SpawnableEntry {
        id: "modelica_balloon".into(),
        display_name: "Modelica Balloon".into(),
        category: "Vessels".into(),
        source: SpawnSource::UsdFile("vessels/balloons/modelica_balloon.usda".into()),
        spawn_lift: 0.0,
        default_transform: Transform::default(),
    }
}

#[test]
fn a_spawned_bodys_render_follows_its_physics_position_over_time() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::mesh::MeshPlugin,
        // Real big_space propagation, so the body inherits its frame the way it
        // does in the app rather than through a stand-in hierarchy.
        big_space::prelude::BigSpaceDefaultPlugins,
        // `interpolate_all` matches production: avian owns every body's Transform.
        PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
    ))
    .init_asset::<UsdStageAsset>()
    // Deterministic stepping — each `update()` advances exactly one frame, so the
    // solver really runs (a wall-clock app would step physics ~never in a test).
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
    .insert_resource(Gravity::ZERO)
    .insert_resource(Time::<Fixed>::from_hz(60.0));

    // The PRODUCTION world root (BigSpace + WorldGrid + FloatingOrigin), built by
    // the same call the app uses — not a hand-rolled stand-in.
    let grid = lunco_core::ensure_world_root(app.world_mut());

    // Production anchoring shape: the scene-root is the ONE grid-direct anchor
    // (its own cell); everything under it is a plain child that inherits the frame.
    let scene_root = app
        .world_mut()
        .spawn((
            Name::new("Scene:test"),
            big_space::prelude::CellCoord::default(),
            lunco_core::GridAnchor,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    app.insert_resource(Args {
        entry: balloon_entry(),
        scene_root,
    });
    app.add_systems(Startup, spawn_once);
    app.finish();
    app.update();

    let root = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<UsdInstanceRoot>>();
        q.iter(world).next().expect("spawn produced a root entity")
    };

    // What the USD loader makes of a physics prim: a dynamic rigid body.
    app.world_mut().entity_mut(root).insert((
        RigidBody::Dynamic,
        Collider::sphere(1.0),
        Mass(4.5),
        LinearVelocity(bevy::math::DVec3::new(0.0, RISE_M_PER_S, 0.0)),
    ));

    for _ in 0..120 {
        app.update(); // ~2 s of simulation
    }

    let world = app.world();
    let pos_y = world.get::<Position>(root).expect("physics position").0.y;
    let tf_y = world.get::<Transform>(root).expect("render transform").translation.y as f64;

    // Guards a vacuously-green step loop: if the solver never ran there would be
    // nothing for the render to follow, and the real assertion below would pass
    // trivially with both values still sitting at the spawn height.
    assert!(
        pos_y > SPAWN_Y as f64 + 5.0,
        "physics must actually have advanced (position_y={pos_y}); \
         otherwise this test proves nothing"
    );

    // The behaviour that matters: the render transform tracks the physics
    // position. Tolerance covers one interpolation step of lag (RISE/60 m).
    assert!(
        (tf_y - pos_y).abs() < 0.5,
        "a spawned body's render transform must follow its physics position; \
         got transform_y={tf_y} vs position_y={pos_y} (a frozen render means \
         avian's Position→Transform writeback is not reaching the spawned body)"
    );
}
