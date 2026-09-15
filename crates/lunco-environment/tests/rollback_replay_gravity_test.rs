//! **Weight must survive a rollback replay.**
//!
//! The shipped app sets avian's own `Gravity::ZERO` (`lunco-luncosim`); gravity
//! reaches a rigid body through the standard `ConstantLinearAcceleration`
//! projection of the cached `LocalGravity` field.
//!
//! Client rollback does not re-run `FixedUpdate`. `replay_one_tick`
//! (`lunco-networking`) runs `lunco_core::RollbackReplay` and then steps
//! `PhysicsSchedule`. The standard acceleration component is consumed directly
//! by Avian's integrator in that schedule, so a replayed tick solves with the
//! same gravity field as the live tick. With gravity absent from that schedule
//! the rover is re-simulated
//! WEIGHTLESS: no weight, no normal force, no wheel traction, and the replayed
//! trajectory diverges from the host's on the one body rollback exists to keep in
//! sync.
//!
//! This drives the REAL schedules in the REAL order `replay_one_tick` uses —
//! calling the projection as a bare function would assert nothing about the
//! registration and replay boundary.
//!
//! A hand-built probe with acceleration inserted directly into `PhysicsSchedule`
//! would miss the projection boundary. This test first runs the shipped
//! `FixedUpdate` projection, then exercises the actual replay schedule.

use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;

use lunco_environment::{EnvironmentPlugin, Gravity, LocalGravity};

/// One replayed tick, mirroring `lunco-networking`'s `replay_one_tick`: run the
/// actuation chain, then advance the physics clocks and step the solver.
fn replay_one_tick(world: &mut World, dt: std::time::Duration) {
    world.run_schedule(lunco_core::RollbackReplay);

    world.resource_mut::<Time<Physics>>().advance_by(dt);
    let SubstepCount(substeps) = *world.resource::<SubstepCount>();
    world
        .resource_mut::<Time<Substeps>>()
        .advance_by(dt.div_f64(substeps as f64));
    *world.resource_mut::<Time>() = world.resource::<Time<Physics>>().as_generic();
    world.run_schedule(PhysicsSchedule);
}

#[test]
fn rollback_replay_applies_local_gravity() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(PhysicsPlugins::default())
        // Exactly the shipped configuration: avian contributes NO gravity of its
        // own, so anything the body feels must come from `RollbackReplay`.
        .insert_resource(avian3d::prelude::Gravity::ZERO)
        .insert_resource(Gravity::flat(1.62, DVec3::NEG_Y))
        .add_plugins(EnvironmentPlugin);
    app.init_schedule(lunco_core::RollbackReplay);
    app.finish();
    app.cleanup();

    // A free body carrying the `LocalGravity` the live tick already cached —
    // which is precisely what a replay starts from (`compute_local_gravity` is
    // deliberately not mirrored into the replay schedule).
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Mass(10.0),
            LocalGravity(DVec3::new(0.0, -1.62, 0.0)),
            Transform::default(),
        ))
        .id();

    // The live fixed boundary projects the cached field onto Avian's standard
    // persistent acceleration component. Rollback then consumes that component
    // without re-running the live environment schedule.
    app.world_mut().run_schedule(FixedUpdate);
    assert!(app
        .world()
        .get::<ConstantLinearAcceleration>(body)
        .is_some());

    let dt = std::time::Duration::from_secs_f64(1.0 / 60.0);
    for _ in 0..10 {
        replay_one_tick(app.world_mut(), dt);
    }

    let vy = app
        .world()
        .get::<LinearVelocity>(body)
        .expect("rigid body keeps a LinearVelocity")
        .y;

    assert!(
        vy < -0.1,
        "a replayed tick must solve WITH the body's weight — got vy = {vy}. \
         The cached local field must be projected onto Avian's standard \
         acceleration component before rollback; otherwise the client \
         re-simulates a weightless rover and prediction diverges from the host."
    );
}
