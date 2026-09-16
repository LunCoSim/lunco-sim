//! One user, one avatar — proven against the avatar role mechanism.

use bevy::prelude::*;
use lunco_avatar_core::roles::{Avatar, LocalAvatar, RemoteAvatar, TheLocalAvatar};

fn world() -> World {
    let mut world = World::new();
    world.init_resource::<TheLocalAvatar>();
    world
}

#[test]
fn the_newest_claimant_is_the_only_avatar() {
    let mut world = world();

    let first = world.spawn((Avatar, LocalAvatar)).id();
    assert_eq!(world.resource::<TheLocalAvatar>().0, Some(first));

    let second = world.spawn((Avatar, LocalAvatar)).id();
    world.flush();

    assert_eq!(world.resource::<TheLocalAvatar>().0, Some(second));
    assert!(world.get::<LocalAvatar>(first).is_none());
    assert!(world.get::<Avatar>(first).is_none());

    let live: Vec<Entity> = world
        .query_filtered::<Entity, With<LocalAvatar>>()
        .iter(&world)
        .collect();
    assert_eq!(live, vec![second]);
}

#[test]
fn a_third_claim_still_leaves_exactly_one() {
    let mut world = world();
    let mut last = Entity::PLACEHOLDER;
    for _ in 0..5 {
        last = world.spawn((Avatar, LocalAvatar)).id();
        world.flush();
    }
    let live: Vec<Entity> = world
        .query_filtered::<Entity, With<LocalAvatar>>()
        .iter(&world)
        .collect();
    assert_eq!(live, vec![last]);
    assert_eq!(world.resource::<TheLocalAvatar>().0, Some(last));
}

#[test]
fn losing_the_avatar_clears_the_slot() {
    let mut world = world();
    let avatar = world.spawn((Avatar, LocalAvatar)).id();
    world.despawn(avatar);
    world.flush();
    assert_eq!(world.resource::<TheLocalAvatar>().0, None);
}

#[test]
fn a_remote_avatar_is_never_the_local_one() {
    let mut world = world();

    let remote = world.spawn((Avatar, RemoteAvatar { session: 7 })).id();
    world.entity_mut(remote).insert(LocalAvatar);
    world.flush();
    assert!(world.get::<LocalAvatar>(remote).is_none());
    assert_eq!(world.resource::<TheLocalAvatar>().0, None);

    let local = world.spawn((Avatar, LocalAvatar)).id();
    world.entity_mut(local).insert(RemoteAvatar { session: 9 });
    world.flush();
    assert!(world.get::<LocalAvatar>(local).is_none());
}
