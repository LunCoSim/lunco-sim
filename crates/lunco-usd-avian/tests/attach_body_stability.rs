//! Regression: the physics an Object Builder new-attach produces must step without
//! crashing the solver.
//!
//! The new-attach (doc 48 §3.1) references a component in as a `Dynamic` rigid body
//! fixed-jointed to its host, added to a LIVE world. This guards the Avian contract
//! that shape of body has to satisfy: a jointed dynamic body — including the
//! degenerate case of one with a `Mass` but NO collider (nothing for Avian to derive
//! inertia from) — steps stably and stays finite rather than sending the solver to
//! NaN or a panic. Pure Avian, so it isolates the physics from the USD read layer.
//!
//! (An earlier hypothesis that a colliderless dynamic body panics Avian turned out
//! to be a test-harness artifact — a headless app driving `update()` without
//! `AssetPlugin` trips a "Message not initialized" panic in Avian's collider cache,
//! which is unrelated to the body. `support::headless_physics_app` encodes the fix;
//! with the app built correctly, Avian tolerates a colliderless dynamic body, and
//! these tests lock that in.)

use avian3d::prelude::*;
use bevy::prelude::*;

mod support;

/// Build a headless Avian app (with `Transform` propagation) and step it `n` fixed
/// steps. The `AssetPlugin`/`Mesh` wiring physics stepping needs lives in
/// [`support::headless_physics_app`].
fn step_app(build: impl FnOnce(&mut World) -> Entity, n: usize) -> (App, Entity) {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    let tracked = build(app.world_mut());
    // Driving `update()` by hand (not `run()`) still needs the plugins' deferred
    // `finish()`/`cleanup()` (Avian registers types/messages there).
    app.finish();
    app.cleanup();
    for _ in 0..n {
        app.update();
    }
    (app, tracked)
}

/// The exact shape the new-attach produces: a `Dynamic` body with a `Mass` but NO
/// collider, fixed-jointed to a kinematic host. It must survive stepping with a
/// finite pose — the solver must not diverge on the zero inertia tensor.
#[test]
fn colliderless_dynamic_body_jointed_to_host_stays_finite() {
    let (app, body) = step_app(
        |world| {
            let host = world
                .spawn((RigidBody::Kinematic, Transform::from_xyz(0.0, 5.0, 0.0)))
                .id();
            let body = world
                .spawn((
                    RigidBody::Dynamic,
                    Mass(1.0),
                    Transform::from_xyz(0.0, 4.0, 0.0),
                ))
                .id();
            world.spawn(
                FixedJoint::new(host, body)
                    .with_local_anchor1(bevy::math::DVec3::new(0.0, -1.0, 0.0)),
            );
            body
        },
        60,
    );

    let pos = app
        .world()
        .get::<Position>(body)
        .expect("body keeps a Position");
    assert!(
        pos.0.is_finite(),
        "jointed colliderless dynamic body must stay finite, got {:?}",
        pos.0
    );
}

/// The same colliderless body under gravity with no joint — no constraint to mask a
/// degeneracy, so it independently proves Avian tolerates a shapeless dynamic body.
#[test]
fn colliderless_dynamic_body_under_gravity_stays_finite() {
    let (app, body) = step_app(
        |world| {
            world
                .spawn((
                    RigidBody::Dynamic,
                    Mass(2.0),
                    Transform::from_xyz(0.0, 10.0, 0.0),
                ))
                .id()
        },
        60,
    );
    let pos = app
        .world()
        .get::<Position>(body)
        .expect("body keeps a Position");
    assert!(
        pos.0.is_finite(),
        "colliderless dynamic body went non-finite: {:?}",
        pos.0
    );
}
