//! Bind authored collision prim observations to the native geometry they own.
//! The two projection edges cover initial admission and body collider recooking.

use avian3d::prelude::{ColliderOf, RigidBody, RigidBodyColliders};
use bevy::prelude::*;
use lunco_physics::ColliderSourceEntity;
use lunco_usd_avian_contracts::UsdColliderSource;
use lunco_usd_bevy_scene::UsdPrimPath;

/// A collision prim is projected after its structural ancestors. Resolve its
/// native shapes after the parent's deferred collider admission has committed.
pub(super) fn bind_source_prim(world: &mut World, source: Entity) {
    let Some(path) = world
        .get::<UsdPrimPath>(source)
        .map(|prim| prim.path.clone())
    else {
        return;
    };
    let mut parents = world.query::<&ChildOf>();
    let body = parents
        .query(world)
        .iter_ancestors(source)
        .find(|entity| world.get::<RigidBody>(*entity).is_some());
    let Some(body) = body else {
        return;
    };
    let colliders: Vec<_> = world
        .get::<RigidBodyColliders>(body)
        .into_iter()
        .flat_map(|colliders| colliders.iter())
        .filter(|collider| {
            world
                .get::<UsdColliderSource>(*collider)
                .is_some_and(|native| native.0 == path)
        })
        .collect();
    for collider in colliders {
        world
            .entity_mut(collider)
            .insert(ColliderSourceEntity(source));
    }
}

/// A recooked native collider can arrive while its authored source survives.
/// Search only this body's hierarchy: duplicate assets in another instance or
/// preview cannot supply the observation owner.
pub(super) fn bind_native_collider(world: &mut World, collider: Entity) {
    let Some(body) = world.get::<ColliderOf>(collider).map(|owner| owner.body) else {
        return;
    };
    let Some(path) = world
        .get::<UsdColliderSource>(collider)
        .map(|source| source.0.clone())
    else {
        return;
    };
    let mut children = world.query::<&Children>();
    let sources: Vec<_> = std::iter::once(body)
        .chain(children.query(world).iter_descendants(body))
        .filter(|entity| {
            world
                .get::<UsdPrimPath>(*entity)
                .is_some_and(|prim| prim.path == path)
        })
        .collect();
    match sources.as_slice() {
        [source] => {
            world
                .entity_mut(collider)
                .insert(ColliderSourceEntity(*source));
        }
        [] => {}
        _ => {
            warn!(?body, %path, "ambiguous authored collider source; contact observations remain unavailable")
        }
    }
}
