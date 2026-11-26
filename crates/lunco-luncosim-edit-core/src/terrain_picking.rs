//! Analytic picking backend for streamed DEM terrain.
//!
//! Streamed CDLOD meshes deliberately keep only `RENDER_WORLD` vertex data: the
//! terrain surface query and the collider ring are the authoritative CPU-side
//! geometry. Bevy's mesh backend therefore cannot produce a pointer hit over open
//! terrain. This backend feeds that existing analytic surface into the normal
//! `PointerClick` pipeline, so waypoint placement, terrain tools, and every other
//! scene-click observer retain one input path.

use bevy::math::Dir3;
use bevy::picking::backend::ray::{RayId, RayMap};
use bevy::picking::backend::{HitData, PointerHits};
use bevy::prelude::*;
use lunco_render::SceneCamera;
use lunco_spatial::coords::GridPos;
use lunco_terrain_surface::{GridSurfaceQuery, SurfaceHit, TerrainRaySurfaceKey};
use lunco_viewport_core::SceneViewport;
use std::collections::HashMap;

#[derive(Resource, Default)]
pub(crate) struct TerrainPickCache {
    surface: Option<TerrainRaySurfaceKey>,
    rays: HashMap<RayId, ((GridPos, Dir3), Option<SurfaceHit>)>,
}
impl lunco_core::SceneResource for TerrainPickCache {}

/// Report the analytic terrain under each scene-camera pointer ray.
///
/// The mesh backend remains responsible for props and vehicles. Its hits are
/// coalesced with these terrain hits by `bevy_picking`, so a rover or marker in
/// front of the ground still wins by depth and open DEM ground still produces a
/// normal `PointerClick` target. The egui backend has the higher camera order
/// and suppresses these hits over chrome.
#[allow(private_interfaces)]
pub fn emit_terrain_hits(
    ray_map: Res<RayMap>,
    viewport: Res<SceneViewport>,
    scene_cameras: Query<
        (&Camera, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<SceneCamera>),
    >,
    surface: GridSurfaceQuery,
    mut output: MessageWriter<PointerHits>,
    mut cache: ResMut<TerrainPickCache>,
) {
    let Some(active_camera) = viewport.exposed_camera() else {
        cache.rays.clear();
        cache.surface = None;
        return;
    };
    let next_surface = surface.ray_surface_key();
    if next_surface.is_none() || next_surface != cache.surface {
        cache.rays.clear();
        cache.surface = next_surface;
    }
    cache
        .rays
        .retain(|id, _| id.camera == active_camera && ray_map.map.contains_key(id));
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
        let render_origin = lunco_spatial::coords::RenderPos(ray.origin.as_dvec3());
        let grid_ray = cache
            .surface
            .as_ref()
            .and_then(|_| surface.ray_to_grid(render_origin, ray.direction));
        let result = if let Some(grid_ray) = grid_ray {
            if let Some((_, hit)) = cache
                .rays
                .get(&ray_id)
                .filter(|(previous, _)| *previous == grid_ray)
            {
                Ok(*hit)
            } else {
                let result = surface.raycast(grid_ray.0, grid_ray.1, f64::INFINITY);
                if let Ok(hit) = &result {
                    cache.rays.insert(ray_id, (grid_ray, *hit));
                }
                result
            }
        } else {
            surface.raycast_render(render_origin, ray.direction, f64::INFINITY)
        };
        let hit = match result {
            Ok(Some(hit)) => hit,
            Ok(None) => continue,
            Err(error) => {
                warn_once!("terrain picking could not certify cursor ray: {error}");
                continue;
            }
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
                HitData::new_with_extra(
                    ray_id.camera,
                    depth,
                    Some(render_point.0.as_vec3()),
                    None,
                    hit,
                ),
            )],
            camera.order as f32,
        ));
    }
}
