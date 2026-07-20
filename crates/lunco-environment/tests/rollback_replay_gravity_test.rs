//! **Weight must survive a rollback replay.**
//!
//! The shipped app sets avian's own `Gravity::ZERO` (`lunco-sandbox`); gravity
//! reaches a rigid body ONLY through `apply_gravity_to_rigid_bodies`, which
//! writes it into avian's force accumulator every `FixedUpdate`.
//!
//! Client rollback does not re-run `FixedUpdate`. `replay_one_tick`
//! (`lunco-networking`) runs `lunco_core::RollbackReplay` and then steps
//! `PhysicsSchedule` — and the physics step CLEARS the accumulator, so nothing
//! carries over from the live tick that preceded the correction. A replayed tick
//! therefore solves with exactly the forces `RollbackReplay` produced and nothing
//! else. With gravity absent from that schedule the rover is re-simulated
//! WEIGHTLESS: no weight, no normal force, no wheel traction, and the replayed
//! trajectory diverges from the host's on the one body rollback exists to keep in
//! sync.
//!
//! This drives the REAL schedules in the REAL order `replay_one_tick` uses —
//! calling the system as a bare function would assert nothing about the
//! registration, which is the thing that was missing.
//!
//! Note why the existing probes never caught this: `rollback_rover_probe` /
//! `rollback_probe` insert `avian3d::prelude::Gravity(-9.81)` directly, so their
//! gravity is applied INSIDE `PhysicsSchedule` and is present during replay by
//! construction. They are green on a path the shipped app does not take.

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
         Gravity reaches a body only through `apply_gravity_to_rigid_bodies`, and \
         the physics step clears the force accumulator, so if that system is not \
         registered in `RollbackReplay` the client re-simulates a weightless rover \
         and prediction diverges from the host."
    );
}
