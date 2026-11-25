//! Atomic runtime participation for admitted simulation and presentation owners.
use bevy::prelude::*;
use lunco_celestial_spatial_core::{LinkOccluder, LinkOcclusion};
use lunco_core::{ActiveCommandId, on_command, register_commands};
use lunco_scene_command_contracts::SetSceneParticipation;
use std::collections::HashSet;

#[on_command(SetSceneParticipation)]
fn on_set_scene_participation(
    trigger: On<SetSceneParticipation>,
    active: Res<ActiveCommandId>,
    reply: lunco_api::executor::CommandReply,
    mut commands: Commands,
) -> lunco_core::CommandOutcome {
    let command = trigger.event().clone();
    let origin = active.origin();
    reply.queue(&mut commands, move |world| {
        let bodies: Vec<_> = command
            .bodies
            .iter()
            .map(|b| (b.target, b.active))
            .collect();
        let colliders: Vec<_> = command
            .colliders
            .iter()
            .map(|c| (c.target, c.enabled))
            .collect();
        let mounted = world
            .get_resource::<lunco_core::SceneMountState>()
            .ok_or("No mounted scene")?;
        for entity in bodies
            .iter()
            .chain(&colliders)
            .map(|(target, _)| *target)
            .chain(command.occluders.iter().map(|occluder| occluder.target))
        {
            let root = lunco_usd_bevy_scene::scene_root_in_world(world, entity)
                .map_err(|error| format!("Cannot resolve scene for {entity:?}: {error:?}"))?;
            if !root.is_some_and(|root| mounted.contains_root(root))
                || lunco_usd_bevy_scene::is_preview_only_entity(world, entity)
            {
                return Err(format!("{entity:?} is outside the mounted scene"));
            }
        }
        let mut seen = HashSet::new();
        for blocker in &command.occluders {
            if !seen.insert(blocker.target) {
                return Err(format!("Repeated radio occluder {:?}", blocker.target));
            }
            if world.get::<LinkOccluder>(blocker.target).is_none()
                || world.get::<LinkOcclusion>(blocker.target).is_none()
            {
                return Err(format!(
                    "{:?} has no admitted radio occluder",
                    blocker.target
                ));
            }
        }
        let physics = lunco_physics::participation::prepare(world, &bodies, &colliders)?;
        if !command.views.is_empty() {
            crate::scene_view::apply_writes(world, origin, command.views)?;
        }
        physics.apply(world);
        for blocker in command.occluders {
            if let Some(mut state) = world.get_mut::<LinkOcclusion>(blocker.target) {
                state.enabled = blocker.enabled;
            }
        }
        Ok(lunco_command_contracts::Ack::default())
    })
}
register_commands!(on_set_scene_participation);

pub(crate) fn install(app: &mut App) {
    use lunco_api::executor::DeferredCommandAppExt;
    register_all_commands(app);
    app.register_deferred_command::<SetSceneParticipation>();
}
