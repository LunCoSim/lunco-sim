//! Analytic picking backend for streamed DEM terrain.
//!
//! Streamed CDLOD meshes deliberately keep only `RENDER_WORLD` vertex data: the
//! terrain surface query and the collider ring are the authoritative CPU-side
//! geometry. Bevy's mesh backend therefore cannot produce a pointer hit over open
//! terrain. This backend feeds that existing analytic surface into the normal
//! `Pointer<Click>` pipeline, so waypoint placement, terrain tools, and every other
//! scene-click observer retain one input path.

use bevy::picking::backend::ray::RayMap;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::prelude::*;
use lunco_core::SceneViewport;
use lunco_render::SceneCamera;
use lunco_terrain_surface::GridSurfaceQuery;

/// Report the analytic terrain under each scene-camera pointer ray.
///
/// The mesh backend remains responsible for props and vehicles. Its hits are
/// coalesced with these terrain hits by `bevy_picking`, so a rover or marker in
/// front of the ground still wins by depth and open DEM ground still produces a
/// normal `Pointer<Click>` target. The egui backend has the higher camera order
/// and suppresses these hits over chrome.
pub fn emit_terrain_hits(
    ray_map: Res<RayMap>,
    viewport: Res<SceneViewport>,
    scene_cameras: Query<
        (&Camera, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<SceneCamera>),
    >,
    surface: GridSurfaceQuery,
    mut output: MessageWriter<PointerHits>,
) {
    let Some(active_camera) = viewport.active_camera else {
        return;
    };
    for (&ray_id, ray) in ray_map.iter() {
        if ray_id.camera != active_camera {
            continue;
        }
        let Ok((camera, target)) = scene_cameras.get(ray_id.camera) else {
            continue;
        };
        if !camera.is_active || !matches!(target, bevy::camera::RenderTarget::Window(_)) {
            continue;
        }
        let Some(hit) = surface.raycast_render(
            lunco_core::coords::RenderPos(ray.origin.as_dvec3()),
            ray.direction,
            f64::INFINITY,
        ) else {
            continue;
        };
        let Some(render_point) = surface.to_render(hit.point) else {
            continue;
        };
        let depth = hit.distance as f32;
        if !depth.is_finite() {
            continue;
        }

        output.write(PointerHits::new(
            ray_id.pointer,
            vec![(
                hit.terrain,
                HitData::new(ray_id.camera, depth, Some(render_point.0.as_vec3()), None),
            )],
            camera.order as f32,
        ));
    }
}
