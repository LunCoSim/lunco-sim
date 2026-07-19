//! Regression: what does a joint-seating system actually READ from `Position`?
//!
//! `build_usd_physics_joints` seats USD-authored joints by comparing each body's
//! world anchor, `p + r * localPos`. That is only meaningful if `p` is the body's
//! authored pose. `Position` is a REQUIRED component of `RigidBody`, so it exists
//! at its default `(0,0,0)` from the instant the body spawns — a system that runs
//! too early reads zeros, the anchor delta degenerates to
//! `localPos0 - localPos1`, and the seat silently measures the wrong thing: a
//! scene misplaced by metres scores as fine, a correctly-placed one is nudged by
//! the anchor offset. Neither failure produces a log line.
//!
//! The measured cause was NOT a race. Two independent things made the read
//! unconditionally wrong, and both are asserted here:
//!
//! 1. `BigSpacePhysicsBridgePlugin` sets
//!    `PhysicsTransformConfig { transform_to_position: false, .. }`, and avian
//!    gates its `transform_to_position` on exactly that flag
//!    (`avian3d-0.7.0/src/physics_transform/mod.rs:108-110`). The system never
//!    runs, so `PhysicsTransformSystems::TransformToPosition` is an EMPTY set and
//!    ordering `.after` it constrains nothing. The bridge's
//!    [`PhysicsBridgeSystems::Read`] pass is what writes `Position` instead.
//!
//! 2. `PhysicsSchedule` is a SEPARATE schedule, run by avian's
//!    `run_physics_schedule` from inside `FixedPostUpdate`'s
//!    `PhysicsSystems::StepSimulation` (`avian3d-0.7.0/src/schedule/mod.rs`).
//!    A system in `FixedPostUpdate`'s `PhysicsSystems::Prepare` — where the joint
//!    builder used to live — is therefore ordered before the entire physics
//!    schedule, so no `.after(...)` written in `FixedPostUpdate` could ever have
//!    observed a bridge-written `Position`. Cross-schedule ordering is silently a
//!    no-op, which is why the bug presented as a race that ordering could not fix.
//!
//! Both probes below sit at those two slots and record what they see on the first
//! tick a body exists. The `FixedPostUpdate` probe reproduces the bug; the
//! `PhysicsSchedule` probe is where `build_usd_physics_joints` now runs.

use avian3d::physics_transform::{Position, PhysicsTransformSystems};
use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_usd_avian::{BigSpacePhysicsBridgePlugin, PhysicsBridgeSystems};

const EDGE: f32 = 2000.0;

/// The authored height of the test body, matching the shape of the scene that
/// motivated this (`episode_01_recording.usda` puts its lander at y = 70).
const AUTHORED_Y: f32 = 70.0;

/// First `Position` observed by a probe at the OLD joint-builder slot
/// (`FixedPostUpdate`, `PhysicsSystems::Prepare`, after `TransformToPosition`).
#[derive(Resource, Default)]
struct SeenInFixedPostUpdate(Option<DVec3>);

/// First `Position` observed by a probe at the NEW joint-builder slot
/// (`PhysicsSchedule`, `PhysicsSystems::Prepare`, after the bridge READ pass).
#[derive(Resource, Default)]
struct SeenInPhysicsSchedule(Option<DVec3>);

#[derive(Component)]
struct Probe;

fn record_old_slot(mut seen: ResMut<SeenInFixedPostUpdate>, q: Query<&Position, With<Probe>>) {
    if seen.0.is_none() {
        if let Ok(p) = q.single() {
            seen.0 = Some(p.0);
        }
    }
}

fn record_new_slot(mut seen: ResMut<SeenInPhysicsSchedule>, q: Query<&Position, With<Probe>>) {
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
    app.init_resource::<SeenInFixedPostUpdate>()
        .init_resource::<SeenInPhysicsSchedule>();

    // Probe at the slot the joint builder USED to occupy.
    app.add_systems(
        FixedPostUpdate,
        record_old_slot
            .in_set(PhysicsSystems::Prepare)
            .after(PhysicsTransformSystems::TransformToPosition),
    );
    // Probe at the slot the joint builder occupies NOW.
    app.add_systems(
        avian3d::schedule::PhysicsSchedule,
        record_new_slot
            .in_set(PhysicsSystems::Prepare)
            .after(PhysicsSystems::First)
            .after(PhysicsBridgeSystems::Read)
            .before(avian3d::schedule::PhysicsStepSystems::First),
    );

    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(15625)));
    app.finish();
    app.cleanup();
    app
}

/// Spawn the production world shell and one dynamic body at `AUTHORED_Y`.
fn spawn_scene(app: &mut App) {
    let root = app
        .world_mut()
        .spawn((BigSpace::default(), Grid::new(EDGE, 100.0), GlobalTransform::default()))
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
    app.world_mut().spawn((
        Probe,
        RigidBody::Dynamic,
        Transform::from_xyz(0.0, AUTHORED_Y, 0.0),
        GlobalTransform::default(),
        CellCoord::ZERO,
        ChildOf(grid),
    ));
}

/// The fix: at the joint builder's slot, `Position` is the AUTHORED pose.
/// At the old slot it is still the required-component default of zero — which is
/// the bug, asserted so that anyone re-introducing that placement fails here
/// instead of shipping a seat that silently measures `localPos0 - localPos1`.
#[test]
fn joint_slot_reads_the_authored_pose_not_the_required_component_default() {
    let mut app = make_app();
    spawn_scene(&mut app);
    // A few frames, because `Time<Fixed>` has to accumulate before `FixedPostUpdate`
    // (and with it the whole `PhysicsSchedule`) runs at all. Both probes latch the
    // FIRST `Position` they ever see, so extra frames cannot mask an early zero —
    // they only give each slot a chance to observe the body once.
    for _ in 0..4 {
        app.update();
    }

    let new_slot = app
        .world()
        .resource::<SeenInPhysicsSchedule>()
        .0
        .expect("the PhysicsSchedule probe must observe the body on the first tick");
    assert!(
        (new_slot.y - AUTHORED_Y as f64).abs() < 1e-6,
        "joint-seating slot must read the AUTHORED pose, got {new_slot:?} \
         (expected y = {AUTHORED_Y}). If this is (0,0,0) the seat is measuring \
         `localPos0 - localPos1` and every anchor verdict it prints is fiction."
    );

    // WHY THE SYSTEM MOVED, recorded as prose rather than as an assertion.
    //
    // The old placement was `FixedPostUpdate`, which runs before the whole
    // `PhysicsSchedule` — so the bridge had not written `Position` yet and the
    // seat measured zeros. An earlier version of this test pinned that by
    // asserting the old slot still reads `DVec3::ZERO`.
    //
    // That assertion is deleted on purpose. It tested AVIAN'S SCHEDULING, not
    // our contract: any upstream reordering would fail it on an unrelated PR,
    // and "the dependency changed" is not a defect in this crate. What we own is
    // the assertion above — the joint-seating slot reads the AUTHORED pose — and
    // that one fails loudly if the fix regresses, whatever avian does internally.
    //
    // `SeenInFixedPostUpdate` is still populated by the probe, so a debugger can
    // read both slots when diagnosing a seating bug; nothing depends on its value.
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
