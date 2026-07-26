//! One user, one avatar — proven against the mechanism, not the callers.
//!
//! Every way an avatar comes into being inserts `LocalAvatar`: a USD scene's
//! `Avatar` prim, the app's fallback free-flight camera, a scene recompose that
//! hands the same prim a fresh entity. The invariant therefore cannot live in
//! any of them; it lives in the component hook, and these tests spawn avatars
//! the crude way — several, directly — because that is what a caller that never
//! heard of the rule looks like.
//!
//! What two live avatars cost, when this regressed: both are `Camera3d`s on the
//! same window, so the viewport visibly flickers between them; input drives both
//! their linked vessels at once; release fires twice.

use bevy::prelude::*;
use lunco_core::{Avatar, LocalAvatar, RemoteAvatar, TheLocalAvatar};

/// A world with the resource the hooks maintain — what `LunCoCorePlugin` installs.
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

    // A second spawner — a different code path, in the real app a different
    // crate — claims the role. The first must stop being an avatar entirely,
    // not merely stop being "the" avatar.
    let second = world.spawn((Avatar, LocalAvatar)).id();
    world.flush();

    assert_eq!(world.resource::<TheLocalAvatar>().0, Some(second));
    assert!(world.get::<LocalAvatar>(first).is_none());
    assert!(
        world.get::<Avatar>(first).is_none(),
        "a demoted avatar keeps no avatar role at all — a leftover `Avatar` is \
         still a second camera to the systems that query it"
    );

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
    assert_eq!(live, vec![last], "five spawns, one avatar");
    assert_eq!(world.resource::<TheLocalAvatar>().0, Some(last));
}

#[test]
fn losing_the_avatar_clears_the_slot() {
    let mut world = world();
    let avatar = world.spawn((Avatar, LocalAvatar)).id();
    world.despawn(avatar);
    world.flush();
    assert_eq!(
        world.resource::<TheLocalAvatar>().0,
        None,
        "a despawned avatar must not be readable as the current one"
    );
}

/// Another user's avatar is a different path, and the types say so: whichever
/// order the two markers arrive in, one entity never holds both.
#[test]
fn a_remote_avatar_is_never_the_local_one() {
    let mut world = world();

    let remote = world.spawn((Avatar, RemoteAvatar { session: 7 })).id();
    world.entity_mut(remote).insert(LocalAvatar);
    world.flush();
    assert!(
        world.get::<LocalAvatar>(remote).is_none(),
        "a remote avatar must not become locally driven"
    );
    assert_eq!(world.resource::<TheLocalAvatar>().0, None);

    // …and the other order: a local avatar handed a remote session stops being
    // local rather than becoming both.
    let local = world.spawn((Avatar, LocalAvatar)).id();
    world.entity_mut(local).insert(RemoteAvatar { session: 9 });
    world.flush();
    assert!(world.get::<LocalAvatar>(local).is_none());
}
