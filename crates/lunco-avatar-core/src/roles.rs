//! ECS role markers for local and replicated avatars.

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;

/// Marker component for an embodiment in the simulation. Ownership and input
/// eligibility are separate qualifiers: [`LocalAvatar`] marks this process's
/// interactive embodiment and [`RemoteAvatar`] marks another session's
/// replicated embodiment.
#[derive(Component)]
pub struct Avatar;

/// **The one local interactive embodiment, when a client has one.**
///
/// At most one entity in a process may carry this marker, and that invariant is
/// enforced here. A headless/API or mission-control process can have no local
/// avatar at all. Other sessions' embodiments belong to [`RemoteAvatar`] and
/// never acquire this marker.
///
/// # Why the invariant is a component hook
///
/// The hook runs on every insert, whatever the spawning path, so a new spawner
/// cannot forget the single-claim rule. The newest claimant wins and the
/// previous holder loses both markers, so it stops being the local interactive
/// embodiment rather than lingering as a second one.
#[derive(Component, Clone, Copy, Debug, Default)]
#[component(on_insert = local_avatar_claimed, on_remove = local_avatar_released)]
pub struct LocalAvatar;

/// Another session's replicated avatar, keyed by the session that owns it.
/// Never this process's local embodiment: inserting it drops [`LocalAvatar`],
/// so the two roles can never describe one entity.
#[derive(Component, Clone, Copy, Debug)]
#[component(on_insert = remote_avatar_claimed)]
pub struct RemoteAvatar {
    /// The session this avatar belongs to. Not this process's.
    pub session: u64,
}

/// The one entity currently holding [`LocalAvatar`], or `None` when no local
/// interactive embodiment exists.
///
/// This is a derived lookup index, not a second ownership model. The component
/// hook is authoritative; it maintains this slot so consumers can resolve the
/// local entity without scanning or selecting by ECS entity order. Read it (or
/// query `Single<_, With<LocalAvatar>>`) — never write it.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TheLocalAvatar(pub Option<Entity>);

/// Installs the avatar role hooks and their derived local-avatar lookup.
pub struct AvatarCorePlugin;

impl Plugin for AvatarCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TheLocalAvatar>();
    }
}

/// `LocalAvatar` was inserted: this entity becomes the sole local avatar, and
/// any previous holder stops being one.
fn local_avatar_claimed(mut world: DeferredWorld, ctx: HookContext) {
    let entity = ctx.entity;
    // A remote avatar cannot also be the local one. Whichever order they arrive
    // in, the entity ends up with exactly one role — see `remote_avatar_claimed`.
    if world.get::<RemoteAvatar>(entity).is_some() {
        world.commands().entity(entity).remove::<LocalAvatar>();
        return;
    }
    let prior = world
        .get_resource::<TheLocalAvatar>()
        .and_then(|slot| slot.0)
        .filter(|prior| *prior != entity);
    if let Some(prior) = prior {
        // The prior holder may already be despawned by the same flush that
        // spawned this one (the usual scene-reload shape).
        if let Ok(mut prior_entity) = world.commands().get_entity(prior) {
            prior_entity.remove::<(LocalAvatar, Avatar)>();
        }
    }
    if let Some(mut slot) = world.get_resource_mut::<TheLocalAvatar>() {
        slot.0 = Some(entity);
    }
}

/// `LocalAvatar` was removed or the entity despawned: clear the slot if it
/// named this entity, so nothing reads a stale avatar.
fn local_avatar_released(mut world: DeferredWorld, ctx: HookContext) {
    let entity = ctx.entity;
    if let Some(mut slot) = world.get_resource_mut::<TheLocalAvatar>() {
        if slot.0 == Some(entity) {
            slot.0 = None;
        }
    }
}

/// A remote avatar can never be the local one.
fn remote_avatar_claimed(mut world: DeferredWorld, ctx: HookContext) {
    if world.get::<LocalAvatar>(ctx.entity).is_some() {
        world.commands().entity(ctx.entity).remove::<LocalAvatar>();
    }
}
