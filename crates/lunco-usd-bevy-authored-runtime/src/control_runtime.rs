//! Projection of authored control and generic-program behavior onto live USD owners.
//!
//! The visual projector only creates scene entities and appearance intent. This
//! module consumes the completed projection and installs the generic runtime
//! surfaces that authored USD exposes: `Controls` becomes a typed
//! [`ControlBinding`] plus [`InputPorts`], while `LunCoProgramAPI` is resolved by
//! the shared program runtime. Keeping this work at the runtime boundary means
//! visual-only consumers do not compile or install control behavior.

use crate::program_runtime::refresh_program_owner;
use bevy::prelude::{Added, Entity, Without, World};
use lunco_camera_core::{CameraFollow, parse_camera_follow};
use lunco_control_core::ControlBinding;
use lunco_port_core::InputPorts;
use lunco_usd_bevy_core::{UsdRead, canonical::CanonicalStages};
use lunco_usd_bevy_scene::{UsdPreviewOnly, UsdPrimPath, UsdSceneProjected};
use openusd::sdf::Path as SdfPath;

/// A control surface prepared from one composed USD owner.
struct AuthoredControlSurface {
    binding: ControlBinding,
    inputs: InputPorts,
    follow: Option<CameraFollow>,
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

/// Project authored runtime behavior after the visual projector has admitted a
/// USD prim. The `Added<UsdSceneProjected>` fence makes this event-driven: idle
/// scenes do not rescan their hierarchy, and visual-only preview entities are
/// never given executable behavior.
pub(crate) fn project_authored_runtime_components(world: &mut World) {
    let owners: Vec<_> = world
        .query_filtered::<(Entity, &UsdPrimPath), (Added<UsdSceneProjected>, Without<UsdPreviewOnly>)>()
        .iter(world)
        .map(|(entity, path)| (entity, path.stage_handle.id(), path.path.clone()))
        .collect();

    for (entity, stage_id, owner_path) in owners {
        let surface = {
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                continue;
            };
            let Some(stage) = stages.get(stage_id) else {
                continue;
            };
            let Ok(owner) = SdfPath::new(&owner_path) else {
                continue;
            };
            read_control_surface(&stage.view(), &owner)
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
        refresh_program_owner(world, stage_id, entity);
    }
}
