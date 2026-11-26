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
//! - [`reconcile_streamed_horizon_configuration`]: an exact source or mode
//!   change retires the owned map and cache before a replacement is admitted.
//! - [`wire_tile_shadow_cache`]: mirror the environment's per-terrain
//!   `HorizonShadowCache` into the terrain-surface `TileShadowCache` component,
//!   which owns terrain self-shadow at every streamed-tile distance.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::camera::visibility::RenderLayers;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};

use lunco_core::HorizonShadowTerrain;
use lunco_core_runtime::async_work::ExternalWorkPermit;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_environment::horizon::{HorizonShadowCacheConfig, retire_horizon_map};
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
    task: Task<Option<(HeightField, u128, ExternalWorkPermit)>>,
    source: StreamedHorizonSource,
    cancelled: Arc<AtomicBool>,
}

impl Drop for StreamedHorizonBake {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Exact immutable inputs of one streamed heightfield bake.
#[derive(Clone)]
struct StreamedHorizonSource {
    oracle: Arc<lunco_terrain_surface::SurfaceOracle>,
    resolution: u32,
}

impl StreamedHorizonSource {
    fn matches(&self, oracle: &Arc<lunco_terrain_surface::SurfaceOracle>, resolution: u32) -> bool {
        Arc::ptr_eq(&self.oracle, oracle) && self.resolution == resolution
    }
}

/// The exact image installed by the streamed producer; the static producer
/// cannot retire this image through its unrelated mesh lease.
#[derive(Component)]
struct StreamedHorizonProduct {
    source: StreamedHorizonSource,
    image: AssetId<Image>,
}

/// Debounce follows the latest source edit, even while no map is published.
#[derive(Component)]
pub(crate) struct StreamedHorizonStale {
    source: StreamedHorizonSource,
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
        if has_map || !demands.near_covers_ready(entity, tiles) {
            continue;
        }
        match stale {
            // Debounce: wait for the surface to go quiescent (a drag keeps pushing
            // the deadline out → exactly one coalesced bake).
            Some(stale) if now.duration_since(stale.since).as_secs_f64() < REBAKE_DEBOUNCE_SECS => {
                continue;
            }
            Some(_) => {}
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
        let source = StreamedHorizonSource {
            oracle: oracle.clone(),
            resolution: res,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let start = std::time::Instant::now();
            let half = oracle.half_extent() as f64;
            let size = 2.0 * half;
            let step = size / (res as f64 - 1.0);
            // Band-limit to the grid's own step — sub-texel craterlets can't
            // shadow at this scale and would only alias the horizon march.
            let region = lunco_terrain_surface::Square {
                center: [0.0, 0.0],
                half,
            };
            // A node can land just outside the nominal edge by floating-point
            // rounding. One lattice step bounds the preparation footprint.
            let oracle = lunco_terrain_surface::SurfaceBand::visual(step)
                .limited_region(&oracle, region, step);
            let mut heights = Vec::with_capacity((res * res) as usize);
            for iz in 0..res {
                if cancellation.load(Ordering::Acquire) {
                    return None;
                }
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
            Some((field, start.elapsed().as_millis(), permit))
        });
        commands
            .entity(entity)
            .try_remove::<StreamedHorizonStale>()
            .try_insert(StreamedHorizonBake {
                task,
                source,
                cancelled,
            });
    }
}

/// Install finished oracle bakes through the environment's mesh-less path —
/// tiles already carry DEM-global UVs matching the field's addressing.
pub(crate) fn finish_streamed_horizon_bakes(
    mut commands: Commands,
    images: Option<ResMut<Assets<Image>>>,
    mut q: Query<
        (
            Entity,
            &mut StreamedHorizonBake,
            &DemHeightField,
            &HorizonShadowTerrain,
        ),
        (
            With<TerrainLodViz>,
            Without<Mesh3d>,
            Without<RenderLayers>,
            Without<HorizonMap>,
        ),
    >,
) {
    let Some(mut images) = images else { return };
    for (entity, mut task, hf, cfg) in &mut q {
        if !task.source.matches(&hf.0, cfg.resolution) {
            commands
                .entity(entity)
                .try_remove::<StreamedHorizonBake>()
                .try_insert(StreamedHorizonStale {
                    source: StreamedHorizonSource {
                        oracle: hf.0.clone(),
                        resolution: cfg.resolution,
                    },
                    since: Instant::now(),
                });
            continue;
        }
        let Some(result) = future::block_on(future::poll_once(&mut task.task)) else {
            continue;
        };
        commands.entity(entity).try_remove::<StreamedHorizonBake>();
        let Some((field, millis, _permit)) = result else {
            continue;
        };
        let image =
            install_horizon_map_from_field(&mut commands, &mut images, entity, field, millis);
        commands.entity(entity).try_insert(StreamedHorizonProduct {
            source: task.source.clone(),
            image,
        });
    }
}

/// Reconcile every streamed lease, even after an entity changes producer mode.
/// An edit retires only the owned image and re-arms the debounce against the
/// new immutable source. The next edit follows that lease even without a map.
fn reconcile_streamed_horizon_configuration(
    mut commands: Commands,
    terrains: Query<
        (
            Entity,
            Option<&HorizonShadowTerrain>,
            Option<&DemHeightField>,
            Has<TerrainLodViz>,
            Has<Mesh3d>,
            Has<RenderLayers>,
            Option<&HorizonMap>,
            Option<&StreamedHorizonProduct>,
            Option<&StreamedHorizonBake>,
            Option<&StreamedHorizonStale>,
        ),
        Or<(
            With<StreamedHorizonProduct>,
            With<StreamedHorizonBake>,
            With<StreamedHorizonStale>,
        )>,
    >,
) {
    for (entity, marker, field, streamed, mesh, scoped, map, product, task, stale) in &terrains {
        let Some((config, field)) = marker
            .zip(field)
            .filter(|(marker, _)| marker.resolution >= 2 && streamed && !mesh && !scoped)
        else {
            if let Some(product) = product {
                retire_horizon_map(&mut commands, entity, product.image);
            }
            commands.entity(entity).try_remove::<(
                StreamedHorizonProduct,
                StreamedHorizonBake,
                StreamedHorizonStale,
            )>();
            continue;
        };
        let matches = |source: &StreamedHorizonSource| source.matches(&field.0, config.resolution);
        let mut edited = stale.is_some_and(|stale| !matches(&stale.source));
        if let Some(product) = product {
            let changed = !matches(&product.source);
            if changed || map.is_none_or(|map| map.image.id() != product.image) {
                retire_horizon_map(&mut commands, entity, product.image);
                commands
                    .entity(entity)
                    .try_remove::<StreamedHorizonProduct>();
                edited |= changed;
            }
        }
        if let Some(task) = task {
            let changed = !matches(&task.source);
            if changed || map.is_some() {
                commands.entity(entity).try_remove::<StreamedHorizonBake>();
                edited |= changed;
            }
        }
        if edited {
            commands.entity(entity).try_insert(StreamedHorizonStale {
                source: StreamedHorizonSource {
                    oracle: field.0.clone(),
                    resolution: config.resolution,
                },
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
fn wire_tile_shadow_cache(
    mut commands: Commands,
    cfg: Res<HorizonShadowCacheConfig>,
    terrains: Query<
        (
            Entity,
            Option<&HorizonShadowCache>,
            Option<&TileShadowCache>,
            Has<HorizonShadowTerrain>,
            Has<TerrainLodViz>,
            Has<Mesh3d>,
            Has<RenderLayers>,
            Option<&StreamedHorizonProduct>,
            Option<&HorizonMap>,
        ),
        Or<(With<TerrainLodViz>, With<TileShadowCache>)>,
    >,
) {
    let cache_quality_valid = cfg.quality_is_valid();

    for (entity, cache, wired, enabled, streamed, mesh, scoped, product, map) in &terrains {
        if !streamed {
            commands.entity(entity).try_remove::<TileShadowCache>();
            continue;
        }
        let cache = cache.filter(|_| {
            !mesh
                && !scoped
                && product
                    .zip(map)
                    .is_some_and(|(product, map)| product.image == map.image.id())
        });
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
        let image = Handle::<Image>::default();
        let entity = app
            .world_mut()
            .spawn((
                TerrainLodViz,
                DemHeightField(oracle.clone()),
                HorizonShadowTerrain { resolution: 2 },
                StreamedHorizonProduct {
                    source: StreamedHorizonSource {
                        oracle,
                        resolution: 2,
                    },
                    image: image.id(),
                },
                HorizonMap {
                    field: HeightField::from_grid(
                        2,
                        Vec2::splat(-1.0),
                        Vec2::splat(2.0),
                        Arc::new(vec![0.0; 4]),
                    ),
                    image,
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

    #[test]
    fn streamed_handoff_retires_owned_work_but_preserves_foreign_map() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_systems(Update, reconcile_streamed_horizon_configuration);
        let oracle = Arc::new(lunco_terrain_surface::SurfaceOracle::bare(Arc::new(
            lunco_terrain_surface::HeightGrid::new_flat(2, 1.0),
        )));
        let source = StreamedHorizonSource {
            oracle: oracle.clone(),
            resolution: 2,
        };
        let old_image = Handle::<Image>::default();
        let foreign_image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .reserve_handle();
        let field = || HeightField::from_grid(2, Vec2::ZERO, Vec2::ONE, Arc::new(vec![0.0; 4]));
        let entity = app
            .world_mut()
            .spawn((
                TerrainLodViz,
                DemHeightField(oracle),
                HorizonShadowTerrain { resolution: 2 },
                StreamedHorizonProduct {
                    source: source.clone(),
                    image: old_image.id(),
                },
                HorizonMap {
                    field: field(),
                    image: old_image.clone(),
                },
            ))
            .id();
        // A replacement producer has already published its own image at this
        // entity when the streamed mode is removed.
        app.world_mut()
            .entity_mut(entity)
            .remove::<TerrainLodViz>()
            .insert(HorizonMap {
                field: field(),
                image: foreign_image.clone(),
            });
        app.update();
        assert!(app.world().get::<StreamedHorizonProduct>(entity).is_none());
        assert_eq!(
            app.world().get::<HorizonMap>(entity).unwrap().image,
            foreign_image
        );

        // Conversely, the image actually owned by the retiring producer must
        // disappear when the entity becomes static.
        app.world_mut().entity_mut(entity).insert((
            StreamedHorizonProduct {
                source,
                image: old_image.id(),
            },
            HorizonMap {
                field: field(),
                image: old_image,
            },
        ));
        app.update();
        assert!(app.world().get::<HorizonMap>(entity).is_none());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn streamed_pending_work_is_retired_when_mesh_is_inserted() {
        let pool = bevy::tasks::TaskPool::new();
        let mut app = App::new();
        app.add_systems(Update, reconcile_streamed_horizon_configuration);
        let oracle = Arc::new(lunco_terrain_surface::SurfaceOracle::bare(Arc::new(
            lunco_terrain_surface::HeightGrid::new_flat(2, 1.0),
        )));
        let cancelled = Arc::new(AtomicBool::new(false));
        let entity = app
            .world_mut()
            .spawn((
                TerrainLodViz,
                DemHeightField(oracle.clone()),
                HorizonShadowTerrain { resolution: 2 },
                StreamedHorizonBake {
                    task: pool.spawn(std::future::pending()),
                    source: StreamedHorizonSource {
                        oracle,
                        resolution: 2,
                    },
                    cancelled: cancelled.clone(),
                },
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .insert(Mesh3d(Handle::default()));
        app.update();
        assert!(app.world().get::<StreamedHorizonBake>(entity).is_none());
        assert!(cancelled.load(Ordering::Acquire));
    }
}
