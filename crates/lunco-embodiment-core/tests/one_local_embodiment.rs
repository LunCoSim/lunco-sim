//! One user, one local embodiment — proven against the role mechanism.

use bevy::prelude::*;
use lunco_embodiment_core::roles::{
    Embodiment, LocalEmbodiment, RemoteEmbodiment, TheLocalEmbodiment,
};

fn world() -> World {
    let mut world = World::new();
    world.init_resource::<TheLocalEmbodiment>();
    world
}

#[test]
fn the_newest_claimant_is_the_only_local_embodiment() {
    let mut world = world();

    let first = world.spawn((Embodiment, LocalEmbodiment)).id();
    assert_eq!(world.resource::<TheLocalEmbodiment>().0, Some(first));

    let second = world.spawn((Embodiment, LocalEmbodiment)).id();
    world.flush();

    assert_eq!(world.resource::<TheLocalEmbodiment>().0, Some(second));
    assert!(world.get::<LocalEmbodiment>(first).is_none());
    assert!(world.get::<Embodiment>(first).is_none());

    let live: Vec<Entity> = world
        .query_filtered::<Entity, With<LocalEmbodiment>>()
        .iter(&world)
        .collect();
    assert_eq!(live, vec![second]);
}

#[test]
fn a_third_claim_still_leaves_exactly_one() {
    let mut world = world();
    let mut last = Entity::PLACEHOLDER;
    for _ in 0..5 {
        last = world.spawn((Embodiment, LocalEmbodiment)).id();
        world.flush();
    }
    let live: Vec<Entity> = world
        .query_filtered::<Entity, With<LocalEmbodiment>>()
        .iter(&world)
        .collect();
    assert_eq!(live, vec![last]);
    assert_eq!(world.resource::<TheLocalEmbodiment>().0, Some(last));
}

#[test]
fn losing_the_local_embodiment_clears_the_slot() {
    let mut world = world();
    let embodiment = world.spawn((Embodiment, LocalEmbodiment)).id();
    world.despawn(embodiment);
    world.flush();
    assert_eq!(world.resource::<TheLocalEmbodiment>().0, None);
}

#[test]
fn a_remote_embodiment_is_never_the_local_one() {
    let mut world = world();

    let remote = world
        .spawn((Embodiment, RemoteEmbodiment { session: 7 }))
        .id();
    world.entity_mut(remote).insert(LocalEmbodiment);
    world.flush();
    assert!(world.get::<LocalEmbodiment>(remote).is_none());
    assert_eq!(world.resource::<TheLocalEmbodiment>().0, None);

    let local = world.spawn((Embodiment, LocalEmbodiment)).id();
    world
        .entity_mut(local)
        .insert(RemoteEmbodiment { session: 9 });
    world.flush();
    assert!(world.get::<LocalEmbodiment>(local).is_none());
}
