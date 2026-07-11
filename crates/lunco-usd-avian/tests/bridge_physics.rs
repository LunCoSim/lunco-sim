//! Headless integration tests for the Phase 5 avian↔big_space bridge.
//!
//! Full `app.update()` frames with manually-advanced time — the production
//! path including big_space propagation/recentring and avian's physics step —
//! with `BigSpacePhysicsBridgePlugin` owning the transform sync (all of
//! avian's f32 sync disabled). The 2026-07-09 island panic
//! (`islands/mod.rs:547` via `update_narrow_phase`) reproduced under the old
//! every-tick static writes; any regression panics these tests.

use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_usd_avian::BigSpacePhysicsBridgePlugin;

const EDGE: f32 = 2000.0;

/// Production-shaped world shell: root carries `Grid`+`BigSpace` (the doc-45
/// rule), the world grid is a cell-entity child, the floating origin a child
/// of that.
fn shell(app: &mut App) -> Entity {
    // NO Transform on the root — the canonical production shape
    // (`ensure_world_root`). The bridge's rootless ColliderTransform
    // propagation replaces the avian pass that needed a root Transform —
    // `scaled_child_collider_ground_settles_without_root_transform` proves
    // that scale-carrying colliders survive this shape.
    let root = app
        .world_mut()
        .spawn((
            BigSpace::default(),
            Grid::new(EDGE, 100.0),
            GlobalTransform::default(),
        ))
        .id();
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
    grid
}

fn make_app() -> App {
    let mut app = App::new();
    // AssetPlugin + Mesh: avian's collider-from-mesh backend reads
    // `AssetEvent<Mesh>` messages, which only exist with assets registered.
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.init_asset::<Mesh>();
    // No bevy TransformPlugin — big_space forbids it and brings its own
    // propagation (the production shape).
    app.add_plugins((
        big_space::plugin::BigSpaceMinimalPlugins,
        PhysicsPlugins::default(),
        BigSpacePhysicsBridgePlugin,
    ));
    // Drive real frames at the fixed timestep so FixedUpdate ticks once per
    // update — deterministic, no wall-clock dependency.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(15625)));
    // Plugins registering resources in `Plugin::finish` (avian's diagnostics)
    // never get it called when tests drive `app.update()` directly.
    app.finish();
    app.cleanup();
    app
}

fn step(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn dynamic_body_settles_on_static_ground_at_origin() {
    let mut app = make_app();
    let grid = shell(&mut app);

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 2.0, 20.0),
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        ChildOf(grid),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 5.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    step(&mut app, 600);

    // Fell from 5 m and rests on the ground top (y = 0) at half-height 0.5.
    let pos = app.world().get::<Position>(body).expect("body has Position");
    assert!(
        (pos.y - 0.5).abs() < 0.1,
        "body did not settle on ground: Position.y = {}",
        pos.y
    );
    // Render truth followed: Transform (cell-local) agrees with the solve.
    let tf = app.world().get::<Transform>(body).unwrap();
    assert!(
        (tf.translation.y - 0.5).abs() < 0.1,
        "writeback missing: Transform.y = {}",
        tf.translation.y
    );
}

#[test]
fn physics_works_at_astronomical_offset_with_small_local_transforms() {
    let mut app = make_app();
    let grid = shell(&mut app);

    // A site grid 2e8 m out (cell 100_000 on a 2 km edge) — Moon-range.
    let site = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::new(100_000, 0, 0),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 2.0, 20.0),
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        ChildOf(site),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 5.0, 0.0),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();

    step(&mut app, 600);

    // The solve ran in the absolute frame…
    let pos = app.world().get::<Position>(body).unwrap();
    assert!(
        pos.x > 1.9e8,
        "Position not in the absolute frame: x = {}",
        pos.x
    );
    assert!((pos.y - 0.5).abs() < 0.1, "did not settle: Position.y = {}", pos.y);
    // …while the render-truth Transform stayed cell-local and small.
    let tf = app.world().get::<Transform>(body).unwrap();
    assert!(
        tf.translation.length() < 1200.0,
        "Transform not cell-local: {:?}",
        tf.translation
    );
    assert!((tf.translation.y - 0.5).abs() < 0.1, "local y = {}", tf.translation.y);
}

#[test]
fn dynamic_body_settles_on_child_collider_ground() {
    // The USD loader's shape: the collider is a CHILD entity of the body prim
    // (`ColliderOf` via hierarchy), not a component on the body itself. This
    // is the class the live sandbox ground uses — a regression here is
    // "rovers sink through the ground at damping-terminal velocity".
    let mut app = make_app();
    let grid = shell(&mut app);

    let ground = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            CellCoord::ZERO,
            Transform::from_xyz(0.0, -0.1, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    app.world_mut().spawn((
        Collider::cuboid(20.0, 2.0, 20.0),
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        ChildOf(ground),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 5.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    app.world_mut().spawn((
        Collider::cuboid(1.0, 1.0, 1.0),
        Transform::default(),
        GlobalTransform::default(),
        ChildOf(body),
    ));

    step(&mut app, 600);

    let pos = app.world().get::<Position>(body).expect("body has Position");
    assert!(
        (pos.y - (-0.1 + (-1.0) + 1.0 + 0.5)).abs() < 0.15,
        "body did not settle on child-collider ground: Position.y = {}",
        pos.y
    );
}

#[test]
fn scaled_child_collider_ground_settles_without_root_transform() {
    // The live sandbox Ground is a UNIT cube scaled by `xformOp:scale =
    // (4000, 0.2, 4000)` — its collider's real size arrives via
    // `ColliderTransform` SCALE (`update_collider_scale`'s child branch).
    // avian's own propagation only descends from tree roots WITH a
    // `Transform` (2026-07-11: with a Transform-free root it froze, the
    // collider collapsed to ~1 m, and every rover sank at damping-terminal
    // speed). The bridge's `propagate_collider_transforms_rootless` must
    // keep this working with the canonical root: the box drops OUTSIDE the
    // unit footprint but INSIDE the scaled one — it can only settle if the
    // scale actually propagated.
    let mut app = make_app();
    let grid = shell(&mut app);

    let ground = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            CellCoord::ZERO,
            Transform::from_xyz(0.0, -0.1, 0.0).with_scale(Vec3::new(4000.0, 0.2, 4000.0)),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    app.world_mut().spawn((
        Collider::cuboid(1.0, 1.0, 1.0),
        Transform::default(),
        GlobalTransform::default(),
        ChildOf(ground),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            // x = 25: outside the unit cube's ±0.5, inside the scaled ±2000.
            Transform::from_xyz(25.0, 5.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    step(&mut app, 600);

    let pos = app.world().get::<Position>(body).expect("body has Position");
    assert!(
        pos.y > -1.0,
        "body fell through the scaled ground — the bridge's rootless \
         ColliderTransform propagation is not carrying scale: Position.y = {}",
        pos.y
    );
    assert!(
        (pos.y - 0.5).abs() < 0.2,
        "body did not settle on the scaled ground top: Position.y = {}",
        pos.y
    );
}

#[test]
fn external_teleport_wakes_sleeping_body() {
    let mut app = make_app();
    let grid = shell(&mut app);

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 2.0, 20.0),
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        ChildOf(grid),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 3.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    // Settle long enough to sleep, then teleport up via Transform only (the
    // MoveEntity / journal-replay shape — no direct Position write).
    step(&mut app, 600);
    assert!(
        app.world().get::<Sleeping>(body).is_some(),
        "precondition: body should be asleep after settling"
    );
    {
        let mut tf = app.world_mut().get_mut::<Transform>(body).unwrap();
        tf.translation.y += 10.0;
    }
    step(&mut app, 300);

    // A body left sleeping would hover at 10.5; the wake path drops it back.
    let pos = app.world().get::<Position>(body).unwrap();
    assert!(
        (pos.y - 0.5).abs() < 0.1,
        "teleported sleeping body did not fall back to ground: y = {}",
        pos.y
    );
}

#[test]
fn external_teleport_carries_child_body() {
    let mut app = make_app();
    let grid = shell(&mut app);

    // Chassis (cell-entity body) with a jointed wheel modelled the way the
    // USD loader builds rovers: the wheel is a Dynamic body that is a plain
    // Transform CHILD of the chassis entity, no CellCoord of its own.
    let chassis = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.0, 2.0),
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 10.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    let wheel = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Transform::from_xyz(2.0, 0.0, 0.0),
            GlobalTransform::default(),
            ChildOf(chassis),
        ))
        .id();
    app.world_mut()
        .spawn(FixedJoint::new(chassis, wheel).with_local_anchor1(bevy::math::DVec3::new(2.0, 0.0, 0.0)));

    // Let the pair free-fall a few ticks so the solver owns both.
    step(&mut app, 5);
    let before = app.world().get::<Position>(wheel).unwrap().0
        - app.world().get::<Position>(chassis).unwrap().0;

    // External teleport of the chassis only (a spawn-placement / gizmo /
    // journal-replay shaped write).
    {
        let mut tf = app.world_mut().get_mut::<Transform>(chassis).unwrap();
        tf.translation.x += 500.0;
    }
    step(&mut app, 1);

    let c = app.world().get::<Position>(chassis).unwrap().0;
    let w = app.world().get::<Position>(wheel).unwrap().0;
    assert!(c.x > 400.0, "chassis teleport did not reach physics: x = {}", c.x);
    let after = w - c;
    assert!(
        (after - before).length() < 0.5,
        "wheel not carried with chassis: relative before {before:?}, after {after:?}"
    );
}
