//! Regression coverage for the single live-joint detach lifecycle.
//!
//! A command must not despawn an Avian joint directly.  The bridge retires the
//! joint graph edge, removes the native constraint and only then despawns the
//! disposable entity.  This test intentionally detaches an admitted fixed joint
//! and continues stepping: the absence of an island-counter panic is part of the
//! contract, as is the fact that the bodies are no longer coupled afterwards.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

use lunco_physics::PhysicsJointDetachRequested;
use lunco_usd_avian::{attach_joint, fixed_joint, JointAttachPlugin};

#[derive(Resource, Clone, Copy)]
struct JointIds {
    body0: Entity,
    body1: Entity,
    joint: Entity,
}

fn spawn_scene(mut commands: Commands) {
    let body0 = commands
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::from_xyz(0.0, 3.0, 0.0),
        ))
        .id();
    let body1 = commands
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::from_xyz(0.0, 1.5, 0.0),
        ))
        .id();
    let joint = commands.spawn_empty().id();
    attach_joint(
        &mut commands,
        joint,
        body0,
        body1,
        fixed_joint(body0, body1),
    );
    commands.insert_resource(JointIds {
        body0,
        body1,
        joint,
    });
}

fn request_detach(ids: Res<JointIds>, mut commands: Commands, mut updates: Local<u32>) {
    *updates += 1;
    if *updates == 30 {
        commands
            .entity(ids.joint)
            .insert(PhysicsJointDetachRequested);
    }
}

fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        PhysicsPlugins::default(),
        JointAttachPlugin,
    ));
    app.init_asset::<Mesh>();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        17,
    )));
    app.add_systems(Startup, spawn_scene);
    app.add_systems(Update, request_detach);
    app.finish();
    app.cleanup();
    app
}

#[test]
fn admitted_joint_detaches_without_solver_island_corruption() {
    let mut app = make_app();
    for _ in 0..36 {
        app.update();
    }

    let ids = *app.world().resource::<JointIds>();
    assert!(
        !app.world().entities().contains(ids.joint),
        "the lifecycle owner must despawn the joint only after graph retirement"
    );

    let before = app
        .world()
        .get::<Position>(ids.body1)
        .expect("body1 keeps a physics pose")
        .0;
    for _ in 0..60 {
        app.update();
    }
    let after = app
        .world()
        .get::<Position>(ids.body1)
        .expect("body1 keeps a physics pose")
        .0;
    assert!(after.is_finite(), "released body diverged: {after:?}");
    assert!(
        (after - before).length() > 1.0e-4,
        "released body did not resume independent integration"
    );
    assert!(app.world().entities().contains(ids.body0));
}
