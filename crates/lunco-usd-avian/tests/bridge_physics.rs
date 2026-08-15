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
use bevy::math::DVec3;
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
    step(&mut app, 2);

    // This changes both components, just like a BigSpace re-split, but it is
    // semantically a move to x=2250 m. Change flags cannot distinguish the two.
    // The bridge must compare the represented pose and deliver this to Avian.
    {
        let mut entity = app.world_mut().entity_mut(body);
        entity.insert(CellCoord::new(1, 0, 0));
        entity.get_mut::<Transform>().unwrap().translation.x = 250.0;
    }
    step(&mut app, 1);

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
    step(&mut app, 1);
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
    step(&mut app, 1);
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
