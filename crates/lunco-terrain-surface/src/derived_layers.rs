//! P3b-runtime: bake the DEM-derived data layers into GPU textures and bind them
//! onto the terrain's `ShaderMaterial` (`terrain_layered.wgsl` slots).
//!
//! The pure math is [`lunco_terrain_core::derive`]; this is its Bevy half. For
//! each terrain that carries a retained height field ([`DemHeightField`]) and a
//! `ShaderMaterial`, we bake — **off the main thread** — two RGBA8 textures:
//!
//! - `surface_map` (binding 6/7): R=roughness G=AO B=rockDensity A=hazard, and
//! - `normal_map`  (binding 8/9): the DEM-derived meso normal,
//!
//! then bind them and raise `weight_rough`/`weight_ao`/`weight_normal` so the
//! layered shader composites them over the procedural regolith floor. Weights are
//! reflected params, so they stay live-tunable in the Inspector / via
//! `SetObjectProperty`.
//!
//! Render-gated by data, not `cfg`: the bake only starts when `Assets<Image>`
//! exists, so the headless server (no render assets) never bakes — it needs only
//! the collider. The maps are pure functions of the height field, so two peers
//! that *do* render derive byte-identical textures with nothing to transfer.
//!
//! Flow: [`start_derived_bakes`] kicks one async task per terrain →
//! [`finish_derived_bakes`] turns the finished buffers into `Image`s →
//! [`apply_derived_layers`] binds them once the material asset is ready (it is
//! created asynchronously by the USD shader path, so binding retries).

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};

use lunco_materials::{ParamValue, ShaderMaterial};
use lunco_terrain_core::{
    ao_map, hazard_from_slope, normal_map, pack_normal_rgba8, pack_surface_rgba8,
    roughness_from_slope, slope_map, Square,
};

use crate::oracle::SurfaceOracle;
use crate::stream_viz::DemHeightField;

/// Texels per side of each baked data layer. 512² keeps the one-shot bake
/// (`res² · ao_dirs · ao_steps` height samples) well under a second off-thread,
/// while still carrying meso-scale slope/AO/normal the procedural FBM fills under.
const LAYER_RES: usize = 512;
/// AO ray budget per texel (directions × march steps).
const AO_DIRS: usize = 8;
const AO_STEPS: usize = 8;
/// AO ray reach as a fraction of the tile half-extent.
const AO_RADIUS_FRAC: f64 = 0.15;
/// Slope (radians) at which roughness saturates and hazard tops out / starts.
const ROUGH_BASE: f32 = 0.6;
const ROUGH_STEEP_RAD: f32 = 0.6; // ~34°
const SAFE_RAD: f32 = 0.2618; // 15°
const CLIFF_RAD: f32 = 0.5236; // 30°

/// One-shot marker: this terrain's derived layers are bound. Stops re-scanning.
#[derive(Component)]
pub struct DerivedLayersBuilt;

/// The in-flight off-thread bake for a terrain's derived layers.
#[derive(Component)]
struct DerivedBakeTask(Task<DerivedMaps>);

/// Baked RGBA8 buffers + their square resolution, ready to upload as `Image`s.
struct DerivedMaps {
    res: usize,
    surface_rgba: Vec<u8>,
    normal_rgba: Vec<u8>,
}

/// GPU handles produced by [`finish_derived_bakes`], awaiting a ready material.
#[derive(Component)]
struct DerivedLayerHandles {
    surface: Handle<Image>,
    normal: Handle<Image>,
}

/// Kick one off-thread bake per terrain that has a height field + a
/// `ShaderMaterial` and hasn't been baked yet. Gated on `Assets<Image>` existing
/// so the headless server never bakes.
fn start_derived_bakes(
    mut commands: Commands,
    images: Option<Res<Assets<Image>>>,
    q: Query<
        (Entity, &DemHeightField),
        (
            With<MeshMaterial3d<ShaderMaterial>>,
            Without<DerivedBakeTask>,
            Without<DerivedLayersBuilt>,
        ),
    >,
) {
    if images.is_none() {
        return; // headless: no render assets → no point baking visual layers.
    }
    for (entity, hf) in &q {
        let oracle: Arc<SurfaceOracle> = hf.0.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move { bake_or_load(&oracle) });
        // Despawn-safe: a load-time / edit re-instantiation can despawn this
        // terrain between queue time and apply — `try_insert` no-ops on a stale
        // entity instead of panicking the command buffer.
        commands.entity(entity).try_insert(DerivedBakeTask(task));
    }
}

/// Bump when the bake math or packed layout changes, so stale cache entries are
/// simply never matched (content-addressed → no explicit invalidation).
/// v2: maps sample the composed `SurfaceOracle` (analytic craters/edits included)
/// and the key folds the oracle's modifier `content_key`.
/// v3: crater profile band-limited + continuous at reach (same crater
/// `content_key`, different sampled surface).
const CACHE_FORMAT_VERSION: u64 = 3;

/// The derived-layer bake as a [`lunco_precompute::Bake`] — the content-addressed
/// disk cache (Substrate B) owns the load/store/rebake orchestration; this only
/// declares *what* is baked, *how it keys*, and *how it serializes*.
struct DerivedBake<'a> {
    oracle: &'a SurfaceOracle,
}

impl lunco_precompute::Bake for DerivedBake<'_> {
    type Output = DerivedMaps;
    const NAMESPACE: &'static str = "terrain/derived";

    /// Content hash of the base DEM heights + the oracle's analytic-modifier key +
    /// every bake parameter. Word-wise FNV-1a fold (no JSON / no allocation),
    /// version-first so a bake/layout change invalidates old entries.
    fn key(&self) -> u64 {
        let grid = self.oracle.grid();
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(CACHE_FORMAT_VERSION);
        h.write_u64(self.oracle.content_key());
        h.write_u64(grid.res as u64);
        h.write_u64(grid.half_extent.to_bits() as u64);
        h.write_u64(LAYER_RES as u64);
        h.write_u64(AO_DIRS as u64);
        h.write_u64(AO_STEPS as u64);
        h.write_u64(AO_RADIUS_FRAC.to_bits());
        h.write_u64(ROUGH_BASE.to_bits() as u64);
        h.write_u64(ROUGH_STEEP_RAD.to_bits() as u64);
        h.write_u64(SAFE_RAD.to_bits() as u64);
        h.write_u64(CLIFF_RAD.to_bits() as u64);
        for &v in &grid.heights {
            h.write_u64(v.to_bits());
        }
        h.finish()
    }

    fn bake(&self) -> DerivedMaps {
        bake_derived(self.oracle)
    }

    fn store(dir: &std::path::Path, maps: &DerivedMaps) -> lunco_precompute::StorageResult<()> {
        lunco_precompute::store_blob(dir, "surface.bin", &maps.surface_rgba)?;
        lunco_precompute::store_blob(dir, "normal.bin", &maps.normal_rgba)
    }

    /// Load both layer buffers, validating that they are square and the same
    /// size. `None` on any miss/mismatch → the orchestrator rebakes.
    fn load(dir: &std::path::Path) -> Option<DerivedMaps> {
        let surface_rgba = lunco_precompute::load_blob(dir, "surface.bin")?;
        let normal_rgba = lunco_precompute::load_blob(dir, "normal.bin")?;
        let texels = surface_rgba.len() / 4;
        let res = (texels as f64).sqrt() as usize;
        if res * res * 4 != surface_rgba.len() || normal_rgba.len() != surface_rgba.len() {
            return None; // corrupt / partial → rebake
        }
        Some(DerivedMaps { res, surface_rgba, normal_rgba })
    }
}

/// P4: content-addressed cache. Load the derived maps from disk if a bake with
/// the same surface + parameters was already persisted; otherwise bake and write
/// them through. Pure-function bake → byte-identical key across runs and peers, so
/// a second load (or a second peer) skips the expensive AO march. The `cache://`
/// dir is shared with the rest of the asset stack.
fn bake_or_load(oracle: &SurfaceOracle) -> DerivedMaps {
    lunco_precompute::bake_or_load(&DerivedBake { oracle }, &lunco_assets::cache_dir())
}

/// Pure bake (runs on the task pool): sample the derived layers off the composed
/// surface (analytic craters included → their slopes/AO land in the maps) and
/// pack them.
fn bake_derived(oracle: &SurfaceOracle) -> DerivedMaps {
    let half = oracle.half_extent() as f64;
    let region = Square { center: [0.0, 0.0], half };
    let res = LAYER_RES;
    // Gate over-zoom synthesis at the map's texel size (512² over the full
    // extent is far coarser than the synthetic detail — skip it, don't alias it).
    let oracle = &oracle.detail_limited(2.0 * half / res as f64);

    let normals = normal_map(oracle, &region, res);
    let slope = slope_map(oracle, &region, res);
    let ao = ao_map(oracle, &region, res, half * AO_RADIUS_FRAC, AO_DIRS, AO_STEPS);

    let roughness: Vec<f32> =
        slope.iter().map(|&s| roughness_from_slope(s, ROUGH_BASE, ROUGH_STEEP_RAD)).collect();
    let hazard: Vec<f32> =
        slope.iter().map(|&s| hazard_from_slope(s, SAFE_RAD, CLIFF_RAD)).collect();

    DerivedMaps {
        res,
        surface_rgba: pack_surface_rgba8(&roughness, &ao, &[], &hazard),
        normal_rgba: pack_normal_rgba8(&normals),
    }
}

/// Upload finished bakes as linear RGBA8 textures and stash the handles for
/// binding. Needs `Assets<Image>` (present whenever a bake was started).
fn finish_derived_bakes(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut DerivedBakeTask)>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let Some(mut images) = images else { return };
    for (entity, mut task) in &mut tasks {
        let Some(maps) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        let surface = images.add(data_texture(maps.res, maps.surface_rgba));
        let normal = images.add(data_texture(maps.res, maps.normal_rgba));
        // `try_*`: a terrain re-bake / doc-backed scene reload can despawn +
        // re-instantiate this terrain while its derived-layer bake is still in flight,
        // so the entity may be gone by the time these deferred commands apply. No-op
        // silently rather than panicking the whole app (as the sibling terrain systems
        // already do — `scatter_terrain_layers`, `finish_dem_restamp`).
        commands
            .entity(entity)
            .try_remove::<DerivedBakeTask>()
            .try_insert(DerivedLayerHandles { surface, normal });
    }
}

/// Bind the baked layers onto the material once it exists (created async by the
/// USD shader path → retry until ready), then mark the terrain done.
fn apply_derived_layers(
    mut commands: Commands,
    q: Query<(Entity, &DerivedLayerHandles, &MeshMaterial3d<ShaderMaterial>), Without<DerivedLayersBuilt>>,
    materials: Option<ResMut<Assets<ShaderMaterial>>>,
) {
    let Some(mut materials) = materials else { return };
    for (entity, handles, mat3d) in &q {
        let Some(material) = materials.get_mut(&mat3d.0) else { continue };
        // Yield to an authored map: a USD `lunco:terrain:layer:surface/normal:map`
        // (bound elsewhere) takes precedence — only fill a slot still empty, so
        // the derived bake is the fallback, not an override.
        let mut weights: Vec<(&str, ParamValue)> = Vec::new();
        if material.surface_map.is_none() {
            material.surface_map = Some(handles.surface.clone());
            weights.push(("weight_rough", ParamValue::F32(1.0)));
            weights.push(("weight_ao", ParamValue::F32(1.0)));
        }
        if material.normal_map.is_none() {
            material.normal_map = Some(handles.normal.clone());
            weights.push(("weight_normal", ParamValue::F32(1.0)));
        }
        if !weights.is_empty() {
            material.set_many(weights);
        }
        commands
            .entity(entity)
            .try_remove::<DerivedLayerHandles>()
            .try_insert(DerivedLayersBuilt);
        info!("[terrain-layers] bound DEM-derived surface+normal layers ({}²)", LAYER_RES);
    }
}

/// A linear (non-sRGB) RGBA8 data texture with linear filtering — these carry
/// roughness/AO/hazard scalars and an encoded normal, not colour.
fn data_texture(res: usize, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d { width: res as u32, height: res as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// Register the derived-layer bake/bind systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (start_derived_bakes, finish_derived_bakes, apply_derived_layers),
    );
}
