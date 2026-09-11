use bevy_app::{Plugin, PreUpdate};
use bevy_camera::Camera;
use bevy_ecs::{
    message::MessageWriter,
    prelude::With,
    schedule::IntoScheduleConfigs,
    system::{Query, Res},
};
use bevy_picking::{
    PickingSystems,
    backend::{HitData, PointerHits},
    pointer::{PointerId, PointerLocation},
};

use crate::{GizmoCamera, GizmoOptions, GizmoStorage, map_cursor_to_gizmo_viewport};

// Keep gizmo handles above the scene camera's ordinary hit layer and below the
// next camera layer. The workbench's offscreen-preview capture is a fractional
// layer above the preview camera; the handle must remain interactable through
// that capture while the egui host still masks the gizmo over real chrome.
const GIZMO_PICK_ORDER_BIAS: f32 = 0.75;

fn gizmo_pick_order(camera_order: isize) -> f32 {
    camera_order as f32 + GIZMO_PICK_ORDER_BIAS
}

pub struct TransformGizmoPickingPlugin;

impl Plugin for TransformGizmoPickingPlugin {
    fn build(&self, app: &mut bevy_app::App) {
        app.add_systems(PreUpdate, update_hits.in_set(PickingSystems::Backend));
    }
}

fn update_hits(
    storage: Res<GizmoStorage>,
    options: Res<GizmoOptions>,
    mut output: MessageWriter<PointerHits>,
    pointers: Query<(&PointerId, &PointerLocation)>,
    cameras: Query<&Camera, With<GizmoCamera>>,
) {
    let mut camera = None;
    for candidate in &cameras {
        if !candidate.is_active {
            continue;
        }
        if camera.replace(candidate).is_some() {
            bevy_log::warn!("Only one camera with a GizmoCamera component is supported.");
            return;
        }
    }
    let Some(camera) = camera else {
        return;
    };
    let Some(viewport) = camera.logical_viewport_rect() else {
        return;
    };
    let gizmos = storage
        .entity_gizmo_map
        .iter()
        .filter_map(|(entity, uuid)| storage.gizmos.get(uuid).map(|gizmo| (*entity, gizmo)))
        .collect::<Vec<_>>();

    for (pointer_id, pointer_location) in &pointers {
        let Some(location) = &pointer_location.location else {
            continue;
        };
        let Some(cursor_pos) =
            map_cursor_to_gizmo_viewport(location.position, viewport, options.viewport_rect)
        else {
            continue;
        };
        let hits = gizmos
            .iter()
            .filter(|(_entity, gizmo)| gizmo.pick_preview((cursor_pos.x, cursor_pos.y)))
            .map(|(entity, _gizmo)| (*entity, HitData::new(*entity, 0.0, None, None)))
            .collect::<Vec<_>>();

        output.write(PointerHits::new(
            *pointer_id,
            hits,
            gizmo_pick_order(camera.order),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::gizmo_pick_order;

    #[test]
    fn gizmo_hit_layer_sits_above_preview_capture() {
        assert_eq!(gizmo_pick_order(1), 1.75);
    }
}
