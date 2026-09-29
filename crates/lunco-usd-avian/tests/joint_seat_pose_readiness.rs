//! Regression coverage for authored joint seating and the bridge's transform
//! ownership contract.
//!
//! `build_usd_physics_joints` seats USD-authored joints from each body's world
//! anchor, `p + r * localPos`. The bridge must publish the authored body pose at
//! the joint-preparation slot, including while readiness holds physics.

use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_spatial::ActivePhysicsFrame;
use lunco_usd_avian_core::BigSpacePhysicsBridgePlugin;

const EDGE: f32 = 2000.0;

/// The authored height of the test body, matching the shape of the scene that
/// motivated this (`episode_01_recording.usda` puts its lander at y = 70).
const AUTHORED_Y: f32 = 70.0;

/// First `Position` observed at the pre-tick joint-preparation slot.
#[derive(Resource, Default)]
struct SeenInJointPreparation(Option<DVec3>);

#[derive(Component)]
struct Probe;

fn record_joint_slot(mut seen: ResMut<SeenInJointPreparation>, q: Query<&Position, With<Probe>>) {
    if seen.0.is_none() {
        if let Ok(p) = q.single() {
            seen.0 = Some(p.0);
        }
    }
}

fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.init_asset::<Mesh>();
    // No bevy `TransformPlugin` — big_space forbids it and brings its own
    // propagation. This is the production shape.
    app.add_plugins((
        big_space::plugin::BigSpaceMinimalPlugins,
        PhysicsPlugins::default(),
        BigSpacePhysicsBridgePlugin,
    ));
    app.init_resource::<SeenInJointPreparation>();

    app.add_systems(Update, record_joint_slot);

    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15625,
    )));
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .pause();
    app.finish();
    app.cleanup();
    app
}

/// Spawn the production world shell and one dynamic body at `AUTHORED_Y`.
fn spawn_scene(app: &mut App) {
    let root = app
        .world_mut()
        .spawn((
            BigSpace::default(),
            Grid::new(EDGE, 100.0),
            GlobalTransform::default(),
        ))
        .id();
    app.world_mut().insert_resource(ActivePhysicsFrame(root));
    let grid = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    app.world_mut().spawn((
        CellCoord::ZERO,
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
        ChildOf(grid),
    ));
    app.world_mut().spawn((
        Probe,
        RigidBody::Dynamic,
        Transform::from_xyz(0.0, AUTHORED_Y, 0.0),
        GlobalTransform::default(),
        CellCoord::ZERO,
        ChildOf(grid),
    ));
}

#[test]
fn lifecycle_joint_preparation_reads_pose_without_a_physics_tick() {
    let mut app = make_app();
    spawn_scene(&mut app);
    app.update();

    let observed = app
        .world()
        .resource::<SeenInJointPreparation>()
        .0
        .expect("Update joint preparation must observe the bridge-seeded body pose");
    assert!(
        (observed.y - AUTHORED_Y as f64).abs() < 1e-6,
        "joint-seating slot must read the authored pose, got {observed:?} \
         (expected y = {AUTHORED_Y})"
    );
    assert_eq!(
        app.world().resource::<Time<Fixed>>().elapsed(),
        Duration::ZERO,
        "lifecycle pose preparation does not need a fixed physics cycle"
    );
}

/// The premise of the whole fix, asserted directly: avian's own
/// `transform_to_position` is DISABLED in this app, so ordering against
/// `PhysicsTransformSystems::TransformToPosition` is vacuous. This is the fact
/// that made the obvious fix fail, and it is invisible at the call site.
#[test]
fn avian_transform_to_position_is_disabled_by_the_bridge() {
    let app = make_app();
    let cfg = app
        .world()
        .resource::<avian3d::physics_transform::PhysicsTransformConfig>();
    assert!(
        !cfg.transform_to_position,
        "the bridge owns Position initialisation; if avian's sync is back on, the \
         two writers will fight over Position every tick"
    );
    assert!(!cfg.position_to_transform);
    assert!(!cfg.propagate_before_physics);
}
