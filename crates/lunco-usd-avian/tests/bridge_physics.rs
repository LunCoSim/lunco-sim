//! Headless integration test for the Phase 5 avian↔big_space bridge.
//!
//! Steps avian physics with `BigSpacePhysicsBridgePlugin` enabled (avian's f32
//! transform sync disabled) over a small grid + static ground + dynamic body,
//! asserting no panic and that the dynamic body is simulated (Position moves).
//! This reproduces the GUI's physics-step path without the 5-minute binary
//! rebuild, so bridge regressions (schedule ambiguity, island panics) surface
//! in seconds.

use bevy::prelude::*;
use avian3d::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
use lunco_usd_avian::BigSpacePhysicsBridgePlugin;

mod support;

fn step_app(app: &mut App, steps: usize) {
    for _ in 0..steps {
        let _ = app.world_mut().run_schedule(FixedUpdate);
    }
}

#[test]
#[ignore = "Phase 5 bridge not yet registered (avian solver/island coupling unresolved); \
            kept as a reproducer for future avian-native integration work"]
fn bridge_steps_physics_without_panic() {
    // `headless_physics_app` supplies MinimalPlugins + AssetPlugin + Mesh +
    // PhysicsPlugins (NO TransformPlugin — big_space drives its own propagation).
    let mut app = support::headless_physics_app();
    app.add_plugins(BigSpacePhysicsBridgePlugin);
    // big_space needs a BigSpace root + grid + floating origin for the bridge's
    // world_pose walks.
    let grid = Grid::new(2000.0, 0.0);
    let root = app
        .world_mut()
        .spawn((big_space::prelude::BigSpace::default(), Transform::default(), GlobalTransform::default()))
        .id();
    let _grid_e = app
        .world_mut()
        .spawn((grid, CellCoord::ZERO, Transform::default(), GlobalTransform::default(), ChildOf(root)))
        .id();
    // A floating origin so big_space propagation has a reference.
    app.world_mut().spawn((
        CellCoord::ZERO,
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
        ChildOf(root),
    ));

    // Static ground + dynamic box, both cell-aware (the bridge syncs them).
    app.world_mut().spawn((
        RigidBody::Static,
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        Collider::cuboid(10.0, 1.0, 10.0),
        ChildOf(root),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 5.0, 0.0),
            GlobalTransform::default(),
            Collider::cuboid(1.0, 1.0, 1.0),
            ChildOf(root),
        ))
        .id();

    // First Update so schedules configure, then step fixed.
    let _ = app.world_mut().run_schedule(Update);
    step_app(&mut app, 60);

    // The dynamic body should have been simulated (Position is present and the
    // box fell under gravity). Position.0.y should be below the spawn 5.0.
    let pos = app.world().get::<Position>(body).expect("body has Position");
    assert!(pos.y < 5.0, "dynamic body did not fall: Position.y = {}", pos.y);
}
