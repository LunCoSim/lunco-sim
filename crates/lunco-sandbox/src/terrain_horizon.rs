//! Far-field terrain self-shadow for STREAMED terrains — the app glue between
//! `lunco-environment`'s horizon system and `lunco-terrain-surface`'s LOD tiles.
//!
//! The horizon pipeline (heightfield → R8 sun-visibility cache → material
//! wiring) was built for the static single-mesh terrain: its bake rasterizes
//! `Mesh3d` triangles and its wiring touches the terrain entity's own
//! material. A streamed terrain has neither — its ground truth is the analytic
//! `SurfaceOracle` and its pixels live on per-tile materials. This module
//! bridges both ends, and only this crate can: it is the one place that sees
//! `HorizonShadowCache` (environment) and `TileShadowCache` (terrain-surface)
//! at once.
//!
//! - [`start_streamed_horizon_bakes`] / [`finish_streamed_horizon_bakes`]:
//!   sample the oracle into a `HeightField` off-thread and install the
//!   `HorizonMap` via the mesh-less path — from there the environment's own
//!   shadow-cache systems take over unchanged (sun-threshold re-bakes etc.).
//! - [`mark_streamed_horizon_stale`]: a live edit swaps the oracle → drop the
//!   map + cache and re-bake after a quiescence debounce.
//! - [`wire_tile_shadow_cache`]: mirror the environment's per-terrain
//!   `HorizonShadowCache` (+ the sun's CSM far bound) into the terrain-surface
//!   `TileShadowCache` component, which the tile materials consume.

use std::sync::Arc;

use bevy::camera::visibility::RenderLayers;
use bevy::light::{CascadeShadowConfig, DirectionalLight};
use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};

use lunco_core::HorizonShadowTerrain;
use lunco_environment::{
    install_horizon_map_from_field, HeightField, HorizonMap, HorizonShadowCache,
};
use lunco_terrain_surface::{DemHeightField, HeightSource, TerrainLodViz, TileShadowCache};

/// Seconds of surface quiescence after a live edit before the horizon
/// heightfield re-bakes (mirrors the derived-maps debounce so a brush-stroke
/// burst coalesces into one bake).
const REBAKE_DEBOUNCE_SECS: f64 = 0.75;

/// In-flight off-thread oracle→heightfield bake for a streamed terrain.
#[derive(Component)]
pub(crate) struct StreamedHorizonBake(Task<(HeightField, u128)>);

/// Debounce marker armed by [`mark_streamed_horizon_stale`].
#[derive(Component)]
pub(crate) struct StreamedHorizonStale {
    since: f64,
}

/// Sample the oracle into a horizon `HeightField` for every streamed terrain
/// that opted into horizon shadows and has no map yet. Native-only: the bake is
/// a few million oracle samples on the async pool; the web build's horizon
/// cache is config-disabled anyway.
#[allow(clippy::type_complexity)]
pub(crate) fn start_streamed_horizon_bakes(
    mut commands: Commands,
    time: Res<Time>,
    q: Query<
        (
            Entity,
            &HorizonShadowTerrain,
            &DemHeightField,
            Option<&StreamedHorizonStale>,
            Has<HorizonMap>,
        ),
        // NOT `Without<HorizonMap>`: an edit made WHILE a bake was in flight lands its
        // (pre-edit) map and re-arms `StreamedHorizonStale` — "map present + stale
        // armed" must therefore re-bake, or the far-field sun-visibility cache stays
        // wrong for the rest of the session (see `mark_streamed_horizon_stale`).
        (With<TerrainLodViz>, Without<Mesh3d>, Without<StreamedHorizonBake>, Without<RenderLayers>),
    >,
) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let now = time.elapsed_secs_f64();
    for (entity, cfg, hf, stale, has_map) in &q {
        match stale {
            // Debounce: wait for the surface to go quiescent (a drag keeps pushing
            // the deadline out → exactly one coalesced bake).
            Some(stale) if now - stale.since < REBAKE_DEBOUNCE_SECS => continue,
            Some(_) => {}
            // No map and nothing armed → this is the terrain's FIRST bake.
            // Map present and nothing armed → it is current; nothing to do.
            None if has_map => continue,
            None => {}
        }
        let oracle = hf.0.clone();
        let res = cfg.resolution.max(2);
        info!(
            "[horizon] baking {res}² heightfield for streamed terrain {entity:?} \
             from the surface oracle…"
        );
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let start = std::time::Instant::now();
            let half = oracle.half_extent() as f64;
            let size = 2.0 * half;
            let step = size / (res as f64 - 1.0);
            // Band-limit to the grid's own step — sub-texel craterlets can't
            // shadow at this scale and would only alias the horizon march.
            let oracle = oracle.detail_limited(step);
            let mut heights = Vec::with_capacity((res * res) as usize);
            for iz in 0..res {
                let z = -half + iz as f64 * step;
                for ix in 0..res {
                    let x = -half + ix as f64 * step;
                    heights.push(oracle.height_at(x, z) as f32);
                }
            }
            let field = HeightField::from_grid(
                res,
                Vec2::splat(-half as f32),
                Vec2::splat(size as f32),
                Arc::new(heights),
            );
            (field, start.elapsed().as_millis())
        });
        commands
            .entity(entity)
            .try_remove::<StreamedHorizonStale>()
            .try_insert(StreamedHorizonBake(task));
    }
}

/// Install finished oracle bakes through the environment's mesh-less path —
/// tiles already carry DEM-global UVs matching the field's addressing.
pub(crate) fn finish_streamed_horizon_bakes(
    mut commands: Commands,
    images: Option<ResMut<Assets<Image>>>,
    mut q: Query<(Entity, &mut StreamedHorizonBake)>,
) {
    let Some(mut images) = images else { return };
    for (entity, mut task) in &mut q {
        let Some((field, millis)) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).try_remove::<StreamedHorizonBake>();
        install_horizon_map_from_field(&mut commands, &mut images, entity, field, millis);
    }
}

/// A live edit swapped the surface oracle: the horizon map (and the visibility
/// cache derived from it) now shadow terrain that no longer exists. Drop both
/// and arm the re-bake debounce; [`wire_tile_shadow_cache`] switches the tile
/// sampling off until the fresh cache lands (far pixels briefly revert to
/// CSM-only — subtle at those ranges).
#[allow(clippy::type_complexity)]
pub(crate) fn mark_streamed_horizon_stale(
    mut commands: Commands,
    time: Res<Time>,
    // NOTE: this must NOT filter `With<HorizonMap>`. The first edit removes the map,
    // so a `With<HorizonMap>` filter would stop matching mid-drag and never refresh
    // `since` — the debounce would then fire at edit+0.75 s against a still-editing
    // oracle and re-bake repeatedly per stroke. Match any already-managed terrain
    // (live map OR debounce already armed) and re-arm on every change instead.
    changed: Query<
        (Entity, Has<HorizonMap>, Has<StreamedHorizonStale>, Has<StreamedHorizonBake>),
        (Changed<DemHeightField>, With<TerrainLodViz>, Without<Mesh3d>),
    >,
) {
    let now = time.elapsed_secs_f64();
    for (entity, has_map, is_stale, is_baking) in &changed {
        // An un-shadowed terrain's FIRST bake is handled by the bake system; only
        // (re)arm terrains that already have a horizon map, a pending debounce, or a
        // bake IN FLIGHT.
        //
        // `is_baking` matters: `start_streamed_horizon_bakes` removes the stale marker
        // when it starts, so while the task runs an edited terrain has no map AND no
        // marker — this used to `continue`, the bake then installed a map from the
        // PRE-EDIT oracle snapshot, and nothing ever re-armed. Brush a crater ~0.8 s
        // into a multi-second bake and the far-field sun-visibility cache stayed wrong
        // for the whole session.
        if !has_map && !is_stale && !is_baking {
            continue;
        }
        let mut e = commands.entity(entity);
        if has_map {
            // The live map now shadows terrain that no longer exists — drop it and
            // the cache so tile sampling reverts to CSM until the re-bake lands.
            e.try_remove::<(HorizonMap, HorizonShadowCache)>();
        }
        // (Re)arm the debounce on EVERY edit so a continuous drag keeps pushing the
        // deadline out → exactly one coalesced bake once the surface goes quiescent.
        e.try_insert(StreamedHorizonStale { since: now });
    }
}

/// Mirror each streamed terrain's `HorizonShadowCache` into the
/// terrain-surface `TileShadowCache` the tile materials consume, tagging on
/// the sun's CSM far bound (the shader blends the cache in beyond it). Writes
/// only on actual change — an unconditional insert would trip the tile
/// late-bind system every frame.
#[allow(clippy::type_complexity)]
pub(crate) fn wire_tile_shadow_cache(
    mut commands: Commands,
    terrains: Query<
        (Entity, Option<&HorizonShadowCache>, Option<&TileShadowCache>),
        (With<TerrainLodViz>, With<HorizonShadowTerrain>),
    >,
    sun: Query<
        (&DirectionalLight, Option<&CascadeShadowConfig>, Option<&RenderLayers>),
        With<DirectionalLight>,
    >,
) {
    // The same "one sun" rule as the environment's wiring: brightest
    // directional light not scoped to a preview render layer.
    let csm_far: f32 = sun
        .iter()
        .filter(|(_, _, layers)| layers.is_none())
        .max_by(|a, b| a.0.illuminance.total_cmp(&b.0.illuminance))
        .and_then(|(light, cascades, _)| {
            if !light.shadow_maps_enabled {
                return Some(0.0);
            }
            cascades.and_then(|c| c.bounds.last().copied())
        })
        .unwrap_or(0.0);

    for (entity, cache, wired) in &terrains {
        let (image, on) = match cache {
            Some(c) => (Some(c.image.clone()), 1.0),
            None => (wired.map(|w| w.image.clone()), 0.0),
        };
        let Some(image) = image else { continue }; // never had a cache → nothing to wire
        let dirty = match wired {
            None => true,
            Some(w) => {
                w.image != image || (w.on - on).abs() > 1e-3 || (w.csm_far - csm_far).abs() > 0.5
            }
        };
        if dirty {
            commands.entity(entity).try_insert(TileShadowCache { image, on, csm_far });
        }
    }
}

/// Register the streamed-horizon glue systems (GUI builds only — the bake and
/// wiring are render concerns; the headless server needs neither).
pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (
            mark_streamed_horizon_stale,
            start_streamed_horizon_bakes,
            finish_streamed_horizon_bakes,
            wire_tile_shadow_cache,
        )
            .chain(),
    );
}
