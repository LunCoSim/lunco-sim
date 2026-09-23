//! Projection of authored control and generic-program behavior onto live USD owners.
//!
//! The visual projector only creates scene entities and appearance intent. This
//! module consumes the completed projection and installs the generic runtime
//! surfaces that authored USD exposes: `Controls` becomes a typed
//! [`ControlBinding`] plus [`InputPorts`], while `LunCoProgramAPI` is resolved by
//! the shared program runtime. Keeping this work at the runtime boundary means
//! visual-only consumers do not compile or install control behavior.

use crate::program_runtime::refresh_program_owner_with_network_members;
use bevy::asset::Assets;
use bevy::prelude::{Add, Commands, Component, Entity, On, Query, With, Without, World};
use lunco_camera_core::{CameraFollow, parse_camera_follow};
use lunco_control_core::ControlBinding;
use lunco_port_core::InputPorts;
use lunco_usd_bevy_scene::{UsdPreviewOnly, UsdPrimPath, UsdSceneProjected};
use lunco_usd_bevy_stage::{
    UsdInstanceProjection, UsdRead, UsdStageAsset, canonical::CanonicalStages,
};
use openusd::sdf::Path as SdfPath;
use std::collections::{HashMap, HashSet};

/// A control surface prepared from one composed USD owner.
struct AuthoredControlSurface {
    binding: ControlBinding,
    inputs: InputPorts,
    follow: Option<CameraFollow>,
}

/// Marks projected USD owners whose authored runtime surfaces have not yet
/// been resolved. The marker is published by the `UsdSceneProjected` observer
/// so steady-state frames do not poll the complete projected scene.
#[derive(Component)]
pub(crate) struct AuthoredRuntimeProjectionPending;

/// Queue one newly projected authored owner for the shared runtime resolver.
pub(crate) fn queue_authored_runtime_projection(
    trigger: On<Add, UsdSceneProjected>,
    owners: Query<(), (With<UsdPrimPath>, Without<UsdPreviewOnly>)>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if owners.get(entity).is_ok() {
        commands
            .entity(entity)
            .try_insert(AuthoredRuntimeProjectionPending);
    }
}

/// Keep the exclusive resolver dormant when no projected owner is pending.
pub(crate) fn has_pending_authored_runtime_projection(
    pending: Query<(), With<AuthoredRuntimeProjectionPending>>,
) -> bool {
    !pending.is_empty()
}

/// Read the composed `Controls` scope belonging to `owner`.
fn read_control_surface<R: UsdRead>(reader: &R, owner: &SdfPath) -> Option<AuthoredControlSurface> {
    let controls = reader
        .children(owner)
        .into_iter()
        .find(|child| child.name() == Some("Controls"))?;
    let entries: Vec<(String, String, f64)> = reader
        .children(&controls)
        .into_iter()
        .filter_map(|binding| {
            let intent = binding.name()?.to_string();
            let port = reader.scalar::<String>(&binding, "lunco:port")?;
            let factor = reader.real(&binding, "lunco:factor")?;
            Some((intent, port, factor))
        })
        .collect();
    let binding = ControlBinding::from_intent_entries(&entries)?;
    let inputs = InputPorts::with_defaults(binding.ports().map(|port| {
        let value = reader.real(owner, &format!("inputs:{port}")).unwrap_or(0.0);
        (port.to_string(), value)
    }));
    let follow = reader
        .text(&controls, "lunco:cameraFollow")
        .and_then(|token| parse_camera_follow(&token));
    Some(AuthoredControlSurface {
        binding,
        inputs,
        follow,
    })
}

/// Project authored runtime behavior for owners queued by the
/// `UsdSceneProjected` add observer. The pending marker limits this exclusive
/// pass to new projections, and visual-only preview entities are excluded.
pub(crate) fn project_authored_runtime_components(world: &mut World) {
    let owners: Vec<_> = world
        .query_filtered::<(Entity, &UsdPrimPath), (
            With<AuthoredRuntimeProjectionPending>,
            Without<UsdPreviewOnly>,
        )>()
        .iter(world)
        .map(|(entity, path)| (entity, path.stage_handle.id(), path.path.clone()))
        .collect();
    let mut network_members_by_stage: HashMap<_, HashSet<String>> = HashMap::new();
    for (_, stage_id, _) in &owners {
        network_members_by_stage
            .entry(*stage_id)
            .or_insert_with(|| {
                let Some(stage_asset) = world
                    .get_resource::<Assets<UsdStageAsset>>()
                    .and_then(|assets| assets.get(*stage_id))
                else {
                    bevy::log::warn!("[usd] stage asset {stage_id:?} is unavailable while projecting authored programs");
                    return HashSet::new();
                };
                let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                    bevy::log::warn!("[usd] canonical stage reader is unavailable while projecting authored programs");
                    return HashSet::new();
                };
                let (reader, _) = stages.reader_for(*stage_id, stage_asset);
                lunco_usd_bevy_core::program::modelica_network_member_paths(&reader)
            });
    }

    for (entity, stage_id, owner_path) in owners {
        if let Ok(mut owner) = world.get_entity_mut(entity) {
            owner.remove::<AuthoredRuntimeProjectionPending>();
        }
        let surface = {
            let Some(stage_asset) = world
                .get_resource::<Assets<UsdStageAsset>>()
                .and_then(|assets| assets.get(stage_id))
            else {
                bevy::log::warn!(
                    "[usd] stage asset {stage_id:?} is unavailable for projected owner {owner_path}"
                );
                continue;
            };
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                bevy::log::warn!(
                    "[usd] canonical stage reader is unavailable for projected owner {owner_path}"
                );
                continue;
            };
            let Ok(owner) = SdfPath::new(&owner_path) else {
                continue;
            };
            let instance = world.get::<UsdInstanceProjection>(entity);
            let (reader, _) = stages.reader_for_entity(stage_id, stage_asset, instance);
            read_control_surface(&reader, &owner)
        };

        let Ok(mut owner) = world.get_entity_mut(entity) else {
            continue;
        };
        owner
            .remove::<ControlBinding>()
            .remove::<InputPorts>()
            .remove::<CameraFollow>();
        if let Some(surface) = surface {
            owner.insert((surface.binding, surface.inputs));
            if let Some(follow) = surface.follow {
                owner.insert(follow);
            }
        }
        drop(owner);

        // Generic executable programs use the same resolver for initial
        // projection and live structural/source edits. This is the only owner
        // path, so a visual refresh cannot create a second program attachment.
        let Some(network_members) = network_members_by_stage.get(&stage_id) else {
            continue;
        };
        refresh_program_owner_with_network_members(world, stage_id, entity, network_members);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::App;

    #[test]
    fn projected_owner_additions_queue_only_live_owners() {
        let mut app = App::new();
        app.add_observer(queue_authored_runtime_projection);

        let live_owner = app
            .world_mut()
            .spawn((UsdPrimPath::default(), UsdSceneProjected))
            .id();
        let preview_owner = app
            .world_mut()
            .spawn((UsdPrimPath::default(), UsdPreviewOnly, UsdSceneProjected))
            .id();
        app.world_mut().flush();

        assert!(
            app.world()
                .get::<AuthoredRuntimeProjectionPending>(live_owner)
                .is_some()
        );
        assert!(
            app.world()
                .get::<AuthoredRuntimeProjectionPending>(preview_owner)
                .is_none()
        );
    }
}
