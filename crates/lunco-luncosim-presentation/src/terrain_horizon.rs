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
//!   `HorizonShadowCache` into the terrain-surface `TileShadowCache` component,
//!   which owns terrain self-shadow at every streamed-tile distance.

use std::sync::Arc;

use bevy::camera::visibility::RenderLayers;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};

use lunco_core::HorizonShadowTerrain;
use lunco_core_runtime::async_work::ExternalWorkPermit;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_environment::horizon::HorizonShadowCacheConfig;
use lunco_environment::{
    HeightField, HorizonMap, HorizonShadowCache, install_horizon_map_from_field,
};
use lunco_terrain_surface::stream_viz::TerrainDetailDemands;
use lunco_terrain_surface::{
    DemHeightField, HeightSource, LodTiles, TerrainLodViz, TileShadowCache,
};

/// Seconds of surface quiescence after a live edit before the horizon
/// heightfield re-bakes (mirrors the derived-maps debounce so a brush-stroke
/// burst coalesces into one bake).
const REBAKE_DEBOUNCE_SECS: f64 = 0.75;

/// In-flight off-thread oracle→heightfield bake for a streamed terrain.
#[derive(Component)]
pub(crate) struct StreamedHorizonBake {
    task: Task<(HeightField, u128, ExternalWorkPermit)>,
    source: Arc<lunco_terrain_surface::SurfaceOracle>,
    resolution: u32,
}

/// Debounce marker armed by [`mark_streamed_horizon_stale`].
#[derive(Component)]
pub(crate) struct StreamedHorizonStale {
    since: Instant,
}

/// Sample the oracle into a horizon `HeightField` for every streamed terrain
/// that opted into horizon shadows and has no map yet. Native-only: the bake is
/// a few million oracle samples on the async pool. The browser currently
/// has no streamed horizon-heightfield producer.
#[allow(clippy::type_complexity)]
pub(crate) fn start_streamed_horizon_bakes(
    mut commands: Commands,
    config: Res<HorizonShadowCacheConfig>,
    demands: Res<TerrainDetailDemands>,
    admission: Res<AsyncWorkAdmission>,
    q: Query<
        (
            Entity,
            &HorizonShadowTerrain,
            &DemHeightField,
            Option<&StreamedHorizonStale>,
            Has<HorizonMap>,
            Option<&LodTiles>,
        ),
        // A current source change retires the queued or running result.
        (
            With<TerrainLodViz>,
            Without<Mesh3d>,
            Without<StreamedHorizonBake>,
            Without<RenderLayers>,
        ),
    >,
) {
    if cfg!(target_arch = "wasm32") || !config.enabled || !config.quality_is_valid() {
        return;
    }
    let now = Instant::now();
    for (entity, cfg, hf, stale, has_map, tiles) in &q {
        if !demands.near_covers_ready(entity, tiles) {
            continue;
        }
        match stale {
            // Debounce: wait for the surface to go quiescent (a drag keeps pushing
            // the deadline out → exactly one coalesced bake).
            Some(stale) if now.duration_since(stale.since).as_secs_f64() < REBAKE_DEBOUNCE_SECS => {
                continue;
            }
            Some(_) => {}
            // Map present and nothing armed → it is current; nothing to do.
            None if has_map => continue,
            // No map and nothing armed → this is the initial visual product. The
            // DEM oracle is already complete, so no live-edit debounce is needed.
            None => {}
        }
        let oracle = hf.0.clone();
        if cfg.resolution < 2 {
            warn!(
                "[horizon] streamed terrain {entity:?} has invalid resolution {}; skipping bake",
                cfg.resolution
            );
            continue;
        }
        let res = cfg.resolution;
        debug!(
            "[horizon] baking {res}² heightfield for streamed terrain {entity:?} \
             from the surface oracle…"
        );
        let key = AsyncWorkKey::new(
            AsyncWorkKind::VisualizationPreparation,
            0,
            entity.to_bits() as u128,
            oracle.surface_key(),
            u64::MAX - 2,
        );
        let Ok(permit) = admission.admit_external(AsyncWorkPriority::Background, key) else {
            continue;
        };
        let source = oracle.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let start = std::time::Instant::now();
            let half = oracle.half_extent() as f64;
            let size = 2.0 * half;
            let step = size / (res as f64 - 1.0);
            // Band-limit to the grid's own step — sub-texel craterlets can't
            // shadow at this scale and would only alias the horizon march.
            let oracle = lunco_terrain_surface::SurfaceBand::visual(step).limited(&oracle);
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
            (field, start.elapsed().as_millis(), permit)
        });
        commands
            .entity(entity)
            .try_remove::<StreamedHorizonStale>()
            .try_insert(StreamedHorizonBake {
                task,
                source,
                resolution: res,
            });
    }
}

/// Install finished oracle bakes through the environment's mesh-less path —
/// tiles already carry DEM-global UVs matching the field's addressing.
pub(crate) fn finish_streamed_horizon_bakes(
    mut commands: Commands,
    images: Option<ResMut<Assets<Image>>>,
    mut q: Query<(
        Entity,
        &mut StreamedHorizonBake,
        &DemHeightField,
        &HorizonShadowTerrain,
    )>,
) {
    let Some(mut images) = images else { return };
    for (entity, mut task, hf, cfg) in &mut q {
        if !Arc::ptr_eq(&task.source, &hf.0) || task.resolution != cfg.resolution {
            commands
                .entity(entity)
                .try_remove::<StreamedHorizonBake>()
                .try_insert(StreamedHorizonStale {
                    since: Instant::now(),
                });
            continue;
        }
        let Some((field, millis, _permit)) = future::block_on(future::poll_once(&mut task.task))
        else {
            continue;
        };
        commands.entity(entity).try_remove::<StreamedHorizonBake>();
        install_horizon_map_from_field(&mut commands, &mut images, entity, field, millis);
    }
}

/// A live edit swapped the surface oracle: the horizon map (and the visibility
/// cache derived from it) now shadow terrain that no longer exists. Drop both
/// and arm the re-bake debounce; [`wire_tile_shadow_cache`] switches terrain
/// self-shadow sampling off until the fresh cache lands. Dynamic-object CSM
/// shadows remain independent and continue to render.
#[allow(clippy::type_complexity)]
pub(crate) fn mark_streamed_horizon_stale(
    mut commands: Commands,
    // Match every managed terrain while a bake or debounce is active.
    // NOTE: this must NOT filter `With<HorizonMap>`. The first edit removes the map,
    // so a `With<HorizonMap>` filter would stop matching mid-drag and never refresh
    // `since` — the debounce would then fire at edit+0.75 s against a still-editing
    // oracle and re-bake repeatedly per stroke. Match any already-managed terrain
    // (live map OR debounce already armed) and re-arm on every change instead.
    changed: Query<
        (
            Entity,
            Has<HorizonMap>,
            Has<StreamedHorizonStale>,
            Has<StreamedHorizonBake>,
        ),
        (
            Changed<DemHeightField>,
            With<TerrainLodViz>,
            Without<Mesh3d>,
        ),
    >,
) {
    let now = Instant::now();
    for (entity, has_map, is_stale, is_baking) in &changed {
        // An un-shadowed terrain's FIRST bake is handled by the bake system; only
        // (re)arm terrains that already have a horizon map, a pending debounce, or a
        // bake IN FLIGHT.
        //
        // The exact in-flight lease is retired by the finish boundary.
        if !has_map && !is_stale && !is_baking {
            continue;
        }
        let mut e = commands.entity(entity);
        if has_map {
            // The live map now shadows terrain that no longer exists — drop it and
            // the cache so stale terrain self-shadow is never displayed while
            // the re-bake runs.
            e.try_remove::<(HorizonMap, HorizonShadowCache)>();
        }
        // (Re)arm the debounce on EVERY edit so a continuous drag keeps pushing the
        // deadline out → exactly one coalesced bake once the surface goes quiescent.
        e.try_insert(StreamedHorizonStale { since: now });
    }
}

/// Retire products and their tasks when the authored configuration no longer
/// owns them. A changed heightfield resolution requires a new current map.
fn reconcile_streamed_horizon_configuration(
    mut commands: Commands,
    terrains: Query<
        (
            Entity,
            Option<&HorizonShadowTerrain>,
            Has<DemHeightField>,
            Has<RenderLayers>,
            Option<&HorizonMap>,
            Option<&StreamedHorizonBake>,
        ),
        (
            With<TerrainLodViz>,
            Or<(
                With<HorizonMap>,
                With<StreamedHorizonBake>,
                With<StreamedHorizonStale>,
            )>,
        ),
    >,
) {
    for (entity, marker, has_source, scoped, map, task) in &terrains {
        let Some(config) = marker.filter(|marker| marker.resolution >= 2 && has_source && !scoped)
        else {
            commands.entity(entity).try_remove::<(
                HorizonMap,
                HorizonShadowCache,
                StreamedHorizonBake,
                StreamedHorizonStale,
            )>();
            continue;
        };
        if map.is_some_and(|map| map.field.resolution() != config.resolution)
            || task.is_some_and(|task| task.resolution != config.resolution)
        {
            commands
                .entity(entity)
                .try_remove::<(HorizonMap, HorizonShadowCache, StreamedHorizonBake)>()
                .try_insert(StreamedHorizonStale {
                    since: Instant::now(),
                });
        }
    }
}

/// Mirror each streamed terrain's `HorizonShadowCache` into the
/// terrain-surface `TileShadowCache` the tile materials consume. Writes only on
/// actual change — an unconditional insert would trip the tile late-bind
/// system every frame.
#[allow(clippy::type_complexity)]
pub(crate) fn wire_tile_shadow_cache(
    mut commands: Commands,
    cfg: Res<HorizonShadowCacheConfig>,
    terrains: Query<
        (
            Entity,
            Option<&HorizonShadowCache>,
            Option<&TileShadowCache>,
            Has<HorizonShadowTerrain>,
        ),
        With<TerrainLodViz>,
    >,
) {
    let cache_quality_valid = cfg.quality_is_valid();

    for (entity, cache, wired, enabled) in &terrains {
        let (image, on) = match cache {
            Some(c) => (
                Some(c.image.clone()),
                f32::from(enabled && cache_quality_valid && cfg.enabled),
            ),
            None => (wired.map(|w| w.image.clone()), 0.0),
        };
        let Some(image) = image else { continue }; // never had a cache → nothing to wire
        let dirty = match wired {
            None => true,
            Some(w) => w.image != image || (w.on - on).abs() > 1e-3,
        };
        if dirty {
            commands
                .entity(entity)
                .try_insert(TileShadowCache { image, on });
        }
    }
}

/// Register the streamed-horizon glue systems (GUI builds only — the bake and
/// wiring are render concerns; the headless server needs neither).
pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (
            reconcile_streamed_horizon_configuration,
            mark_streamed_horizon_stale,
            start_streamed_horizon_bakes,
            finish_streamed_horizon_bakes,
        )
            .chain()
            .before(lunco_terrain_surface::stream_viz::update_lod_tiles)
            .in_set(lunco_core::RuntimeCycleSet::Visualization),
    );
    // Publish cache residency before tile binding. Direction-dependent shader
    // uniforms use the finalized render frame in the renderer's horizon feed.
    app.add_systems(
        PostUpdate,
        wire_tile_shadow_cache
            .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision)
            .after(lunco_environment::finalize_sun_render_state)
            .before(lunco_terrain_surface::TerrainSurfaceSet::RenderShadowBinding)
            .in_set(lunco_core::RuntimeCycleSet::Visualization),
    );
}

#[cfg(test)]
mod configuration_tests {
    use super::*;

    #[test]
    fn changed_or_removed_horizon_configuration_retires_published_map() {
        let mut app = App::new();
        app.add_systems(Update, reconcile_streamed_horizon_configuration);
        let oracle = Arc::new(lunco_terrain_surface::SurfaceOracle::bare(Arc::new(
            lunco_terrain_surface::HeightGrid::new_flat(2, 1.0),
        )));
        let entity = app
            .world_mut()
            .spawn((
                TerrainLodViz,
                DemHeightField(oracle),
                HorizonShadowTerrain { resolution: 2 },
                HorizonMap {
                    field: HeightField::from_grid(
                        2,
                        Vec2::splat(-1.0),
                        Vec2::splat(2.0),
                        Arc::new(vec![0.0; 4]),
                    ),
                    image: Handle::default(),
                },
            ))
            .id();
        app.update();
        assert!(app.world().get::<HorizonMap>(entity).is_some());
        app.world_mut()
            .get_mut::<HorizonShadowTerrain>(entity)
            .unwrap()
            .resolution = 4;
        app.update();
        assert!(app.world().get::<HorizonMap>(entity).is_none());
        assert!(app.world().get::<StreamedHorizonStale>(entity).is_some());
        app.world_mut()
            .entity_mut(entity)
            .remove::<HorizonShadowTerrain>();
        app.update();
        assert!(app.world().get::<StreamedHorizonStale>(entity).is_none());
    }
}
