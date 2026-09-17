//! ECS role markers for local and replicated embodiments.

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;

/// Marker component for an operator embodiment in the simulation. Ownership
/// and input eligibility are separate qualifiers: [`LocalEmbodiment`] marks
/// this process's interactive embodiment and [`RemoteEmbodiment`] marks
/// another session's replicated embodiment.
#[derive(Component)]
pub struct Embodiment;

/// **The one local interactive embodiment, when a client has one.**
///
/// At most one entity in a process may carry this marker, and that invariant is
/// enforced here. A headless/API or mission-control process can have no local
/// embodiment at all. Other sessions' embodiments belong to [`RemoteEmbodiment`] and
/// never acquire this marker.
///
/// # Why the invariant is a component hook
///
/// The hook runs on every insert, whatever the spawning path, so a new spawner
/// cannot forget the single-claim rule. The newest claimant wins and the
/// previous holder loses both markers, so it stops being the local interactive
/// embodiment rather than lingering as a second one.
#[derive(Component, Clone, Copy, Debug, Default)]
#[component(on_insert = local_embodiment_claimed, on_remove = local_embodiment_released)]
pub struct LocalEmbodiment;

/// Another session's replicated embodiment, keyed by the session that owns it.
/// Never this process's local embodiment: inserting it drops [`LocalEmbodiment`],
/// so the two roles can never describe one entity.
#[derive(Component, Clone, Copy, Debug)]
#[component(on_insert = remote_embodiment_claimed)]
pub struct RemoteEmbodiment {
    /// The session this embodiment belongs to. Not this process's.
    pub session: u64,
}

/// The one entity currently holding [`LocalEmbodiment`], or `None` when no local
/// interactive embodiment exists.
///
/// This is a derived lookup index, not a second ownership model. The component
/// hook is authoritative; it maintains this slot so consumers can resolve the
/// local entity without scanning or selecting by ECS entity order. Read it (or
/// query `Single<_, With<LocalEmbodiment>>`) — never write it.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TheLocalEmbodiment(pub Option<Entity>);

/// Resolve an explicitly requested embodiment or the process-local one.
///
/// An omitted entity is an instruction to use the authoritative local role;
/// it is not permission to select an arbitrary entity by ECS order.
pub fn resolve_requested_or_local(
    requested: Option<Entity>,
    local: Option<&TheLocalEmbodiment>,
) -> Result<Entity, String> {
    match requested {
        Some(entity) => Ok(entity),
        None => local
            .and_then(|slot| slot.0)
            .ok_or_else(|| "no authoritative local embodiment is available".to_string()),
    }
}

/// Installs the embodiment role hooks and their derived local-embodiment lookup.
pub struct EmbodimentCorePlugin;

impl Plugin for EmbodimentCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TheLocalEmbodiment>();
    }
}

/// `LocalEmbodiment` was inserted: this entity becomes the sole local embodiment, and
/// any previous holder stops being one.
fn local_embodiment_claimed(mut world: DeferredWorld, ctx: HookContext) {
    let entity = ctx.entity;
    // A remote embodiment cannot also be the local one. Whichever order they
    // arrive in, the entity ends up with exactly one role.
    if world.get::<RemoteEmbodiment>(entity).is_some() {
        world.commands().entity(entity).remove::<LocalEmbodiment>();
        return;
    }
    let prior = world
        .get_resource::<TheLocalEmbodiment>()
        .and_then(|slot| slot.0)
        .filter(|prior| *prior != entity);
    if let Some(prior) = prior {
        // The prior holder may already be despawned by the same flush that
        // spawned this one (the usual scene-reload shape).
        if let Ok(mut prior_entity) = world.commands().get_entity(prior) {
            prior_entity.remove::<(LocalEmbodiment, Embodiment)>();
        }
    }
    if let Some(mut slot) = world.get_resource_mut::<TheLocalEmbodiment>() {
        slot.0 = Some(entity);
    }
}

/// `LocalEmbodiment` was removed or the entity despawned: clear the slot if it
/// named this entity, so nothing reads a stale embodiment.
fn local_embodiment_released(mut world: DeferredWorld, ctx: HookContext) {
    let entity = ctx.entity;
    if let Some(mut slot) = world.get_resource_mut::<TheLocalEmbodiment>() {
        if slot.0 == Some(entity) {
            slot.0 = None;
        }
    }
}

/// A remote embodiment can never be the local one.
fn remote_embodiment_claimed(mut world: DeferredWorld, ctx: HookContext) {
    if world.get::<LocalEmbodiment>(ctx.entity).is_some() {
        world
            .commands()
            .entity(ctx.entity)
            .remove::<LocalEmbodiment>();
    }
}
