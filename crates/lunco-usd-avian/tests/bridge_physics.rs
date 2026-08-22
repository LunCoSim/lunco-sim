//! Headless integration tests for the Phase 5 avian↔big_space bridge.
//!
//! Full `app.update()` frames with manually-advanced time — the production
//! path including big_space propagation/recentring and avian's physics step —
//! with `BigSpacePhysicsBridgePlugin` owning the transform sync (all of
//! avian's f32 sync disabled). The 2026-07-09 island panic
//! (`islands/mod.rs:547` via `update_narrow_phase`) reproduced under the old
//! every-tick static writes; any regression panics these tests.

use avian3d::math::Vector;
use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_core::ActivePhysicsFrame;
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15625,
    )));
    // Plugins registering resources in `Plugin::finish` (avian's diagnostics)
    // never get it called when tests drive `app.update()` directly.
    app.finish();
    app.cleanup();
    app
}

fn make_app_with_usd_collision_hooks() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.init_asset::<Mesh>();
    app.add_plugins((
        big_space::plugin::BigSpaceMinimalPlugins,
        PhysicsPlugins::default().with_collision_hooks::<lunco_usd_avian::UsdCollisionFilter>(),
        BigSpacePhysicsBridgePlugin,
    ));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15625,
    )));
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
    let pos = app
        .world()
        .get::<Position>(body)
        .expect("body has Position");
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
    assert!(
        (pos.y - 0.5).abs() < 0.1,
        "did not settle: Position.y = {}",
        pos.y
    );
    // …while the render-truth Transform stayed cell-local and small.
    let tf = app.world().get::<Transform>(body).unwrap();
    assert!(
        tf.translation.length() < 1200.0,
        "Transform not cell-local: {:?}",
        tf.translation
    );
    assert!(
        (tf.translation.y - 0.5).abs() < 0.1,
        "local y = {}",
        tf.translation.y
    );
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

    let pos = app
        .world()
        .get::<Position>(body)
        .expect("body has Position");
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

    let pos = app
        .world()
        .get::<Position>(body)
        .expect("body has Position");
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
    app.world_mut().spawn(
        FixedJoint::new(chassis, wheel).with_local_anchor1(bevy::math::DVec3::new(2.0, 0.0, 0.0)),
    );

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
    assert!(
        c.x > 400.0,
        "chassis teleport did not reach physics: x = {}",
        c.x
    );
    let after = w - c;
    assert!(
        (after - before).length() < 0.5,
        "wheel not carried with chassis: relative before {before:?}, after {after:?}"
    );
}

#[test]
fn paired_cell_transform_teleport_reaches_physics() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let grid = shell(&mut app);
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            CellCoord::ZERO,
            Transform::from_xyz(10.0, 0.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    step(&mut app, 10);

    // This changes both components, just like a BigSpace re-split, but it is
    // semantically a move to x=2250 m. Change flags cannot distinguish the two.
    // The bridge must compare the represented pose and deliver this to Avian.
    {
        let mut entity = app.world_mut().entity_mut(body);
        entity.insert(CellCoord::new(1, 0, 0));
        entity.get_mut::<Transform>().unwrap().translation.x = 250.0;
    }
    step(&mut app, 2);

    let position = app.world().get::<Position>(body).unwrap().0;
    assert!(
        (position.x - 2250.0).abs() < 1e-6,
        "paired cross-cell teleport was mistaken for a representation re-split: {position:?}"
    );
}

#[test]
fn plain_ancestor_resplit_and_teleport_are_distinguished() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let grid = shell(&mut app);
    let carrier = app
        .world_mut()
        .spawn((
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Transform::from_xyz(10.0, 0.0, 0.0),
            GlobalTransform::default(),
            ChildOf(carrier),
        ))
        .id();
    step(&mut app, 2);
    let initial = app.world().get::<Position>(body).unwrap().0;

    // Different storage representation, identical carrier pose.
    {
        let mut entity = app.world_mut().entity_mut(carrier);
        entity.insert(CellCoord::new(1, 0, 0));
        entity.get_mut::<Transform>().unwrap().translation.x = -EDGE;
    }
    // The first manual update advances Bevy's clock but does not yet admit a
    // FixedUpdate step.  Seed is owned by the enclosing fixed schedule, so
    // observe it after the first admitted physics frame.
    step(&mut app, 2);
    let after_resplit = app.world().get::<Position>(body).unwrap().0;
    assert_eq!(
        after_resplit, initial,
        "representation-only ancestor re-split moved its physical descendant"
    );

    // Same paired component shape, but now the carrier really moved +2000 m.
    {
        let mut entity = app.world_mut().entity_mut(carrier);
        entity.insert(CellCoord::new(2, 0, 0));
        entity.get_mut::<Transform>().unwrap().translation.x = -EDGE;
    }
    step(&mut app, 2);
    let after_teleport = app.world().get::<Position>(body).unwrap().0;
    assert!(
        (after_teleport.x - (initial.x + f64::from(EDGE))).abs() < 1e-6,
        "semantic ancestor teleport did not carry its physical descendant: initial={initial:?}, after={after_teleport:?}"
    );
}

#[test]
fn surface_physics_frame_is_invariant_to_rotating_celestial_parent() {
    let mut app = make_app();
    let grid = shell(&mut app);

    // This is the production shape: a rotating celestial Grid carries a
    // body-fixed PhysicsFrame Grid, and the physical scene is below it.
    let rotating_parent = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();
    let physics_frame = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(rotating_parent),
        ))
        .id();
    app.world_mut()
        .insert_resource(ActivePhysicsFrame(physics_frame));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 2.0, 20.0),
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -1.0, 0.0),
        GlobalTransform::default(),
        ChildOf(physics_frame),
    ));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            Transform::from_xyz(3.0, 5.0, -4.0),
            GlobalTransform::default(),
            ChildOf(physics_frame),
        ))
        .id();

    step(&mut app, 600);
    let initial = app.world().get::<Position>(body).unwrap().0;
    let initial_rotation = app.world().get::<Rotation>(body).unwrap().0;
    let initial_linear_velocity = app.world().get::<LinearVelocity>(body).unwrap().0;
    let initial_angular_velocity = app.world().get::<AngularVelocity>(body).unwrap().0;
    let initial_specific_energy = 0.5 * initial_linear_velocity.length_squared()
        + 0.5 * initial_angular_velocity.length_squared();
    assert!((initial.x - 3.0).abs() < 0.1);
    assert!((initial.z + 4.0).abs() < 0.1);
    assert!((initial.y - 0.5).abs() < 0.1);

    for n in 1..=30 {
        app.world_mut()
            .get_mut::<Transform>(rotating_parent)
            .unwrap()
            .rotation = Quat::from_rotation_y(n as f32 * 0.1);
        step(&mut app, 1);
    }

    let final_pos = app.world().get::<Position>(body).unwrap().0;
    let final_rotation = app.world().get::<Rotation>(body).unwrap().0;
    let final_linear_velocity = app.world().get::<LinearVelocity>(body).unwrap().0;
    let final_angular_velocity = app.world().get::<AngularVelocity>(body).unwrap().0;
    let final_specific_energy = 0.5 * final_linear_velocity.length_squared()
        + 0.5 * final_angular_velocity.length_squared();
    assert!((final_pos - initial).length() < 0.05);
    assert!(
        final_rotation.angle_between(initial_rotation) < 1e-4,
        "rotation above ActivePhysicsFrame leaked into Avian: before={initial_rotation:?}, after={final_rotation:?}"
    );
    assert!(final_linear_velocity.is_finite());
    assert!(final_angular_velocity.is_finite());
    assert!(
        (final_linear_velocity - initial_linear_velocity).length() < 1e-3,
        "rotating celestial parent injected linear velocity: before={initial_linear_velocity:?}, after={final_linear_velocity:?}"
    );
    assert!(
        (final_angular_velocity - initial_angular_velocity).length() < 1e-3,
        "rotating celestial parent injected angular velocity: before={initial_angular_velocity:?}, after={final_angular_velocity:?}"
    );
    assert!(
        final_specific_energy <= initial_specific_energy + 1e-6,
        "rotating celestial parent injected specific kinetic energy: before={initial_specific_energy:e}, after={final_specific_energy:e}"
    );
    assert!((app.world().get::<Transform>(body).unwrap().translation.x - 3.0).abs() < 0.1);

    // A paired CellCoord/Transform re-split is a representation change of the
    // same active-frame pose, not a physical teleport. This is the exact shape
    // emitted by BigSpace recentring.
    {
        let mut entity = app.world_mut().entity_mut(body);
        entity.insert(CellCoord::new(1, 0, 0));
        entity.get_mut::<Transform>().unwrap().translation.x -= EDGE;
    }
    step(&mut app, 1);
    let after_rebranch = app.world().get::<Position>(body).unwrap().0;
    assert!(
        // The dynamic body remains in contact and advances one solver tick;
        // compare below the contact solver's sub-millimetre resting motion,
        // while a misclassified cell move would be 2 km.
        (after_rebranch - final_pos).length() < 1e-3,
        "representation-only BigSpace rebranch changed physics pose: before={final_pos:?}, after={after_rebranch:?}"
    );
}

#[test]
fn active_frame_handoff_transports_velocity_into_the_new_grid() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let root = shell(&mut app);

    // Scene loading seeds bodies while WorldRoot is still the active Avian
    // frame. Celestial placement then promotes the authored site Grid to the
    // active frame. This is the production handoff that used to rewrite only
    // Position/Rotation and leave the velocity components in root axes.
    let site = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::from_rotation(Quat::from_rotation_y(0.7)),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            LinearVelocity(Vector::new(2.0, -0.5, 3.0)),
            AngularVelocity(Vector::new(-0.2, 0.4, 0.1)),
            CellCoord::ZERO,
            Transform::from_xyz(4.0, 8.0, -2.0),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();

    // Seed the body in WorldRoot coordinates before the frame is promoted.
    // The first manual update only initializes the fixed clock accumulator.
    step(&mut app, 2);
    let old_linear = app.world().get::<LinearVelocity>(body).unwrap().0;
    let old_angular = app.world().get::<AngularVelocity>(body).unwrap().0;
    let frame_rotation = app
        .world()
        .get::<Transform>(site)
        .unwrap()
        .rotation
        .as_dquat();

    app.world_mut().insert_resource(ActivePhysicsFrame(site));
    step(&mut app, 1);

    // Position/Rotation and both velocity vectors must now use the site Grid's
    // axes. A rebranch is not a physical impulse and must not alter magnitudes.
    let expected_linear = frame_rotation.inverse() * old_linear;
    let expected_angular = frame_rotation.inverse() * old_angular;
    let actual_linear = app.world().get::<LinearVelocity>(body).unwrap().0;
    let actual_angular = app.world().get::<AngularVelocity>(body).unwrap().0;
    assert!(
        (actual_linear - expected_linear).length() < 1.0e-6,
        "linear velocity was not rebased: old={old_linear:?} expected={expected_linear:?} actual={actual_linear:?}"
    );
    assert!(
        (actual_angular - expected_angular).length() < 1.0e-6,
        "angular velocity was not rebased: old={old_angular:?} expected={expected_angular:?} actual={actual_angular:?}"
    );
    assert!((actual_linear.length() - old_linear.length()).abs() < 1.0e-6);
    assert!((actual_angular.length() - old_angular.length()).abs() < 1.0e-6);
}

#[test]
fn active_frame_handoff_is_transport_on_the_first_physics_read() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let root = shell(&mut app);
    let site = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::from_rotation(Quat::from_rotation_y(0.7)),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            LinearVelocity(Vector::new(2.0, 0.0, 3.0)),
            AngularVelocity(Vector::new(-0.2, 0.0, 0.1)),
            CellCoord::ZERO,
            Transform::from_xyz(4.0, 8.0, -2.0),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();

    // Seed in WorldRoot directly, then promote the site before any physics
    // schedule read. This is the ordering produced while scene activation is
    // held; a prior physics tick is not part of the transport contract.
    app.world_mut().run_schedule(FixedPostUpdate);
    assert!(app
        .world()
        .get::<lunco_physics::PhysicsPoseSeeded>(body)
        .is_some());
    let old_linear = app.world().get::<LinearVelocity>(body).unwrap().0;
    let old_angular = app.world().get::<AngularVelocity>(body).unwrap().0;
    let frame_rotation = app
        .world()
        .get::<Transform>(site)
        .unwrap()
        .rotation
        .as_dquat();
    app.world_mut().insert_resource(ActivePhysicsFrame(site));
    step(&mut app, 2);

    let actual_linear = app.world().get::<LinearVelocity>(body).unwrap().0;
    let actual_angular = app.world().get::<AngularVelocity>(body).unwrap().0;
    assert!(
        (actual_linear - frame_rotation.inverse() * old_linear).length() < 1.0e-6,
        "first-read linear rebase failed: old={old_linear:?} expected={:?} actual={actual_linear:?}",
        frame_rotation.inverse() * old_linear
    );
    assert!(
        (actual_angular - frame_rotation.inverse() * old_angular).length() < 1.0e-6,
        "first-read angular rebase failed: old={old_angular:?} expected={:?} actual={actual_angular:?}",
        frame_rotation.inverse() * old_angular
    );
}

#[test]
fn frame_handoff_seeds_late_bodies_in_the_committed_frame() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let root = shell(&mut app);
    let site_rotation = Quat::from_rotation_y(0.7);
    let site = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::from_rotation(site_rotation),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    let old_linear = Vector::new(2.0, -0.5, 3.0);
    let existing = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            LinearVelocity(old_linear),
            CellCoord::ZERO,
            Transform::from_xyz(4.0, 8.0, -2.0),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();

    // Establish the old-frame state before the site is promoted.
    app.world_mut().run_schedule(FixedPostUpdate);
    app.world_mut().insert_resource(ActivePhysicsFrame(site));

    // This body appears during the frame transaction. It must not be seeded
    // in the new frame by a parallel hold-safe pass and then transported a
    // second time by the canonical READ owner.
    let late_local = DVec3::new(-3.0, 6.0, 2.0);
    let late = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            CellCoord::ZERO,
            Transform::from_translation(late_local.as_vec3()),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();
    app.world_mut().run_schedule(FixedPostUpdate);

    let rebased = app.world().get::<LinearVelocity>(existing).unwrap().0;
    assert!((rebased - site_rotation.inverse().as_dquat() * old_linear).length() < 1.0e-6);
    let late_position = app.world().get::<Position>(late).unwrap().0;
    assert!(
        (late_position - late_local).length() < 1.0e-6,
        "late body was seeded or transported in the wrong frame: expected={late_local:?} actual={late_position:?}"
    );
    assert!(app
        .world()
        .get::<lunco_physics::PhysicsPoseSeeded>(late)
        .is_some());
}

#[test]
fn active_site_frame_seeds_initial_pose_in_site_coordinates() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let root = shell(&mut app);
    let site_rotation = Quat::from_rotation_y(0.7);
    let site_translation = Vec3::new(120.0, -30.0, 75.0);
    let site = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::from_translation(site_translation).with_rotation(site_rotation),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    app.world_mut().insert_resource(ActivePhysicsFrame(site));
    let body_local = DVec3::new(4.0, 8.0, -2.0);
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            CellCoord::ZERO,
            Transform::from_translation(body_local.as_vec3()),
            GlobalTransform::default(),
            ChildOf(site),
        ))
        .id();

    // Exercise the bridge at its authoritative fixed-physics boundary.  This
    // is the first admitted physics frame; no arbitrary frame-count delay is
    // part of the contract.
    app.world_mut().run_schedule(FixedPostUpdate);

    let position = app.world().get::<Position>(body).unwrap().0;
    assert!(
        (position - body_local).length() < 1.0e-6,
        "initial pose was seeded in the celestial frame instead of the active site frame: expected={body_local:?} actual={position:?}"
    );
    assert!(
        app.world()
            .get::<lunco_physics::PhysicsPoseSeeded>(body)
            .is_some(),
        "active-frame pose was written without publishing the authoritative readiness marker"
    );
}

#[test]
fn jointed_surface_assembly_is_invariant_to_rotating_celestial_parent() {
    let mut app = make_app();
    app.insert_resource(Gravity(Vector::ZERO));
    let root_grid = shell(&mut app);
    let rotating_parent = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(root_grid),
        ))
        .id();
    let physics_frame = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(rotating_parent),
        ))
        .id();
    app.world_mut()
        .insert_resource(ActivePhysicsFrame(physics_frame));

    let chassis = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.0, 2.0),
            CellCoord::ZERO,
            Transform::from_xyz(10.0, 20.0, -30.0),
            GlobalTransform::default(),
            ChildOf(physics_frame),
        ))
        .id();
    let offsets = [
        DVec3::new(1.5, -1.0, 1.5),
        DVec3::new(-1.5, -1.0, 1.5),
        DVec3::new(1.5, -1.0, -1.5),
        DVec3::new(-1.5, -1.0, -1.5),
    ];
    let mut legs = Vec::new();
    for offset in offsets {
        let leg = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(0.25, 1.0, 0.25),
                Transform::from_translation(offset.as_vec3()),
                GlobalTransform::default(),
                ChildOf(chassis),
            ))
            .id();
        app.world_mut()
            .spawn(FixedJoint::new(chassis, leg).with_local_anchor1(offset));
        legs.push(leg);
    }

    step(&mut app, 20);
    let initial_chassis = app.world().get::<Position>(chassis).unwrap().0;
    let initial_relative: Vec<DVec3> = legs
        .iter()
        .map(|leg| app.world().get::<Position>(*leg).unwrap().0 - initial_chassis)
        .collect();

    for n in 1..=120 {
        app.world_mut()
            .get_mut::<Transform>(rotating_parent)
            .unwrap()
            .rotation = Quat::from_euler(EulerRot::YXZ, n as f32 * 0.01, n as f32 * 0.003, 0.0);
        step(&mut app, 1);
    }

    let final_chassis = app.world().get::<Position>(chassis).unwrap().0;
    assert!(
        (final_chassis - initial_chassis).length() < 1e-4,
        "celestial parent motion displaced the chassis in its active physics frame"
    );
    for (leg, expected) in legs.iter().zip(initial_relative) {
        let actual = app.world().get::<Position>(*leg).unwrap().0 - final_chassis;
        assert!(
            (actual - expected).length() < 1e-3,
            "celestial parent motion changed a jointed leg offset: expected={expected:?}, actual={actual:?}"
        );
        let linear = app.world().get::<LinearVelocity>(*leg).unwrap().0;
        let angular = app.world().get::<AngularVelocity>(*leg).unwrap().0;
        assert!(linear.is_finite() && linear.length() < 1e-4);
        assert!(angular.is_finite() && angular.length() < 1e-4);
    }
}

#[test]
fn nested_body_strut_contact_preserves_mass_frame() {
    let mut app = make_app_with_usd_collision_hooks();
    app.insert_resource(Gravity(Vector::new(0.0, -1.6248896, 0.0)));
    app.insert_resource(SubstepCount(32));
    let grid = shell(&mut app);

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Friction {
            dynamic_coefficient: 1.0,
            static_coefficient: 1.0,
            combine_rule: CoefficientCombine::Min,
        },
        ActiveCollisionHooks::MODIFY_CONTACTS,
        CellCoord::ZERO,
        Transform::from_xyz(0.0, -0.05, 0.0),
        GlobalTransform::default(),
        ChildOf(grid),
    ));
    let hull = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Mass(4000.0),
            AngularInertia::new(Vec3::new(5905.0, 6250.0, 5905.0)),
            CenterOfMass::ZERO,
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
            Collider::cuboid(2.5, 1.5, 2.5),
            CellCoord::ZERO,
            Transform::from_xyz(0.0, 12.0, 0.0),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    let angle = 20.0_f64.to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let legs = [
        (
            DVec3::new(sin, -cos, 0.0),
            DVec3::new(2.519, 1.388, 0.0),
            Quat::from_rotation_z(angle as f32),
        ),
        (
            DVec3::new(-sin, -cos, 0.0),
            DVec3::new(-2.519, 1.388, 0.0),
            Quat::from_rotation_z(-angle as f32),
        ),
        (
            DVec3::new(0.0, -cos, sin),
            DVec3::new(0.0, 1.388, 2.519),
            Quat::from_rotation_x(-angle as f32),
        ),
        (
            DVec3::new(0.0, -cos, -sin),
            DVec3::new(0.0, 1.388, -2.519),
            Quat::from_rotation_x(angle as f32),
        ),
    ];
    for (axis, mount, rotation) in legs {
        let leg = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Mass(30.0),
                AngularInertia::new(Vec3::new(124.30, 0.0844, 124.30)),
                CenterOfMass::new(0.0, -3.525, 0.0),
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                Transform::from_translation(mount.as_vec3()).with_rotation(rotation),
                GlobalTransform::default(),
                ChildOf(hull),
            ))
            .id();
        let pad = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Mass(10.0),
                AngularInertia::new(Vec3::splat(10.0)),
                NoAutoMass,
                NoAutoAngularInertia,
                Collider::cylinder(0.455, 0.1),
                Friction {
                    dynamic_coefficient: 0.60,
                    static_coefficient: 0.65,
                    combine_rule: CoefficientCombine::Min,
                },
                ActiveCollisionHooks::MODIFY_CONTACTS,
                Transform::from_xyz(0.0, -7.2, 0.0).with_rotation(rotation.inverse()),
                GlobalTransform::default(),
                ChildOf(leg),
            ))
            .id();
        let rotation_d = DQuat::from_xyzw(
            rotation.x as f64,
            rotation.y as f64,
            rotation.z as f64,
            rotation.w as f64,
        );
        let pad_anchor1 = match axis {
            _ if axis.x > 0.0 => DVec3::new(0.017101, -7.153015, 0.0),
            _ if axis.x < 0.0 => DVec3::new(-0.017101, -7.153015, 0.0),
            _ if axis.z > 0.0 => DVec3::new(0.0, -7.153015, 0.017101),
            _ => DVec3::new(0.0, -7.153015, -0.017101),
        };
        app.world_mut().spawn((
            SphericalJoint::new(leg, pad)
                .with_local_anchor1(pad_anchor1)
                .with_local_anchor2(DVec3::new(0.0, 0.05, 0.0)),
            JointCollisionDisabled,
            JointDamping {
                linear: 0.0,
                angular: 2.5,
            },
        ));
        app.world_mut().spawn((
            PrismaticJoint::new(hull, leg)
                .with_local_anchor1(mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_local_basis1(rotation_d)
                .with_local_basis2(DQuat::IDENTITY)
                .with_slider_axis(DVec3::Y)
                .with_limits(-0.8, 0.0)
                .with_motor(
                    LinearMotor::new(MotorModel::ForceBased {
                        stiffness: 1_800_000.0,
                        damping: 90_000.0,
                    })
                    .with_target_position(0.0)
                    .with_max_force(42_000.0),
                ),
            JointCollisionDisabled,
        ));
    }

    for _ in 0..600 {
        app.update();
    }
    let position = app.world().get::<Position>(hull).unwrap().0;
    let velocity = app.world().get::<LinearVelocity>(hull).unwrap().0;
    assert!(
        position.is_finite() && velocity.is_finite() && velocity.length() < 10.0,
        "nested BigSpace body/child-collider contact became unstable: position={position:?} velocity={velocity:?}"
    );
}
