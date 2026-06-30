//! Documents the load-bearing fact behind the click-routing fix: two *global*
//! observers watching the same event BOTH run for a single trigger. That's why
//! selection (`on_scene_click_select`) and possession (`avatar_raycast_possession`)
//! must partition by keyboard modifier — Shift+click selects, plain click
//! possesses — rather than relying on one swallowing the click from the other.

use bevy::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static A_RAN: AtomicUsize = AtomicUsize::new(0);
static B_RAN: AtomicUsize = AtomicUsize::new(0);

#[derive(EntityEvent)]
struct MyClick {
    entity: Entity,
}

fn observer_a(_on: On<MyClick>) {
    A_RAN.fetch_add(1, Ordering::SeqCst);
}

fn observer_b(_on: On<MyClick>) {
    B_RAN.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn both_global_observers_run_for_one_event() {
    let mut app = App::new();
    app.add_observer(observer_a);
    app.add_observer(observer_b);
    let entity = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(MyClick { entity });

    assert_eq!(A_RAN.load(Ordering::SeqCst), 1, "observer A must run");
    assert_eq!(B_RAN.load(Ordering::SeqCst), 1, "observer B must run for the SAME event");
}

#[test]
fn test_detach_joint_command() {
    let mut app = App::new();
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_observer(lunco_sandbox_edit::commands::on_detach_joint);
    app.register_type::<lunco_sandbox_edit::commands::DetachJoint>();

    let joint_entity = app.world_mut().spawn_empty().id();
    assert!(app.world().get_entity(joint_entity).is_some());

    app.world_mut().trigger(lunco_sandbox_edit::commands::DetachJoint {
        target: joint_entity,
    });

    // Flush commands to execute the observer
    app.world_mut().flush();

    assert!(app.world().get_entity(joint_entity).is_none(), "Joint entity must be despawned by DetachJoint command");
}

