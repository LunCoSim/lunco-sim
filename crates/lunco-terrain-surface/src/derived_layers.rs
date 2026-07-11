//! P3b-runtime: bake the DEM-derived data layers into GPU textures and publish
//! them for every terrain render path.
//!
//! The pure math is [`lunco_terrain_core::derive`]; this is its Bevy half. For
//! each terrain that carries a retained height field ([`DemHeightField`]) we
//! bake — **off the main thread** — two mipped RGBA8 textures:
//!
//! - `surface_map` (binding 6/7): R=roughness G=AO B=rockDensity A=hazard, and
//! - `normal_map`  (binding 8/9): the DEM-derived meso normal, with the
//!   relief-correlated **albedo scalar in alpha**,
//!
//! and publish them as a [`TerrainDerivedMaps`] component. Consumers:
//!
//! - the **static-mesh path** ([`apply_derived_layers`]) binds them onto the
//!   terrain's own `ShaderMaterial` (`terrain_layered.wgsl` slots) and raises
//!   `weight_rough`/`weight_ao`/`weight_normal`;
//! - the **streamed-tile path** (`stream_viz`) binds them onto every LOD-tile
//!   geomorph material — this is what carries crater rims / AO / tonal variation
//!   at distances where tile geometry and the procedural FBM have LOD'd away.
//!
//! Render-gated by data, not `cfg`: the bake only starts when `Assets<Image>`
//! exists, so the headless server (no render assets) never bakes — it needs only
//! the collider. The maps are pure functions of the height field, so two peers
//! that *do* render derive byte-identical textures with nothing to transfer.
//!
//! Live edits: a brush/reseed swaps the `DemHeightField` Arc →
//! [`mark_derived_stale`] drops the published maps and, after a short quiescence
//! debounce (so a stroke burst coalesces into one bake), the whole chain re-runs.
//!
//! Flow: [`mark_derived_stale`] → [`start_derived_bakes`] (one async task per
//! terrain) → [`finish_derived_bakes`] (upload as `Image`s + publish
//! [`TerrainDerivedMaps`]) → [`apply_derived_layers`] (static-mesh bind; the
//! material asset is created asynchronously by the USD shader path, so binding
//! retries).

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};

use lunco_materials::{ParamValue, ShaderMaterial};
use lunco_terrain_core::{
    albedo_map, ao_map, hazard_from_slope, normal_map, pack_normal_rgba8, pack_surface_rgba8,
    roughness_from_slope, slope_map, Square,
};

use crate::oracle::SurfaceOracle;
use crate::stream_viz::DemHeightField;

/// Texels per side of each baked data layer. 1024² over the moonbase ±4 km
/// window ≈ 8 m/texel — enough for the ≥15 m crater population that defines the
/// far-field look; the streamed tiles' geometry + procedural FBM own everything
/// finer. The bake (`res² · ao_dirs · ao_steps` height samples) stays a few
/// seconds off-thread and is content-address cached, so it runs once per surface.
const LAYER_RES: usize = 1024;
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

/// One-shot marker: this terrain's derived layers are bound onto its own
/// static-mesh `ShaderMaterial`. Stops re-scanning. Streamed tiles don't use
/// this — they read [`TerrainDerivedMaps`] directly.
#[derive(Component)]
pub struct DerivedLayersBuilt;

/// The published derived maps for a terrain — GPU handles every terrain render
/// path binds from. `surface` packs R=roughness G=AO B=rockDensity A=hazard;
/// `normal` packs the meso normal in RGB and the albedo scalar in A. Removed
/// (and re-baked) when the surface changes.
#[derive(Component, Clone)]
pub struct TerrainDerivedMaps {
    pub surface: Handle<Image>,
    pub normal: Handle<Image>,
    /// Texels per side — lets consumers reason about map vs geometry frequency.
    pub res: usize,
}

/// The in-flight off-thread bake for a terrain's derived layers, plus the
/// identity (Arc pointer) of the oracle it was started against — a re-compose
/// mid-bake makes the result stale, and [`finish_derived_bakes`] discards it.
#[derive(Component)]
struct DerivedBakeTask(Task<DerivedMaps>, usize);

/// Debounce marker: the surface changed at `since`; wait for a short quiescent
/// window before re-baking so a burst of brush strokes coalesces into one bake.
#[derive(Component)]
struct DerivedMapsStale {
    since: f64,
}

/// Seconds of surface quiescence before a re-bake starts.
const REBAKE_DEBOUNCE_SECS: f64 = 0.75;

/// Baked RGBA8 buffers + their square resolution, ready to upload as `Image`s.
struct DerivedMaps {
    res: usize,
    surface_rgba: Vec<u8>,
    normal_rgba: Vec<u8>,
}

/// Set by `finish_dem_restamp` alongside the collider ring's dirty region:
/// whether the surface change that swapped the `DemHeightField` was a BOUNDED
/// edit (a brush stroke / placed crater) or whole-terrain (spec change, reseed,
/// load). Consumed by [`mark_derived_stale`].
#[derive(Component)]
pub struct DerivedDirtyRegion {
    pub bounded: bool,
}

/// A surface re-compose swapped the `DemHeightField` Arc: arm the re-bake
/// debounce. For a BOUNDED edit (see [`DerivedDirtyRegion`]) the published maps
/// stay live while the fresh bake runs — they are correct everywhere except the
/// edit's footprint, and dropping them popped the whole far field to the
/// procedural fallback for the entire bake. A whole-terrain change still drops
/// them (globally wrong maps are worse than the fallback).
fn mark_derived_stale(
    mut commands: Commands,
    time: Res<Time>,
    changed: Query<
        (Entity, Option<&DerivedDirtyRegion>),
        (Changed<DemHeightField>, With<TerrainDerivedMaps>),
    >,
) {
    for (entity, region) in &changed {
        let bounded = region.is_some_and(|r| r.bounded);
        let mut e = commands.entity(entity);
        if !bounded {
            e.try_remove::<(TerrainDerivedMaps, DerivedLayersBuilt)>();
        }
        e.try_remove::<DerivedDirtyRegion>()
            .try_insert(DerivedMapsStale { since: time.elapsed_secs_f64() });
    }
}

/// Kick one off-thread bake per terrain that either has no published maps yet
/// or has maps marked stale by a bounded edit (kept live while the bake runs —
/// see [`mark_derived_stale`]), respecting the edit debounce. Gated on
/// `Assets<Image>` existing so the headless server never bakes.
fn start_derived_bakes(
    mut commands: Commands,
    images: Option<Res<Assets<Image>>>,
    time: Res<Time>,
    q: Query<
        (Entity, &DemHeightField, Option<&DerivedMapsStale>, Has<TerrainDerivedMaps>),
        Without<DerivedBakeTask>,
    >,
) {
    if images.is_none() {
        return; // headless: no render assets → no point baking visual layers.
    }
    let now = time.elapsed_secs_f64();
    for (entity, hf, stale, has_maps) in &q {
        if has_maps && stale.is_none() {
            continue; // published and current — nothing to do.
        }
        if let Some(stale) = stale {
            if now - stale.since < REBAKE_DEBOUNCE_SECS {
                continue; // edits still landing — wait for quiescence.
            }
        }
        let oracle: Arc<SurfaceOracle> = hf.0.clone();
        let oracle_ptr = Arc::as_ptr(&hf.0) as usize;
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Off-thread body → own Tracy zone (per-system spans don't reach here).
            let _span = bevy::log::info_span!("terrain_derived_maps_bake").entered();
            #[cfg(not(target_arch = "wasm32"))]
            {
                bake_or_load(&oracle)
            }
            #[cfg(target_arch = "wasm32")]
            {
                bake_or_load_web(&oracle).await
            }
        });
        // Despawn-safe: a load-time / edit re-instantiation can despawn this
        // terrain between queue time and apply — `try_insert` no-ops on a stale
        // entity instead of panicking the command buffer.
        commands
            .entity(entity)
            .try_remove::<DerivedMapsStale>()
            .try_insert(DerivedBakeTask(task, oracle_ptr));
    }
}

/// Bump when the bake math or packed layout changes, so stale cache entries are
/// simply never matched (content-addressed → no explicit invalidation).
/// v2: maps sample the composed `SurfaceOracle` (analytic craters/edits included)
/// and the key folds the oracle's modifier `content_key`.
/// v3: crater profile band-limited + continuous at reach (same crater
/// `content_key`, different sampled surface).
/// v4: albedo scalar packed into normal-map alpha; LAYER_RES 512 → 1024.
/// v5: tone (albedo) derived with a 3-texel stencil on a 6-texel-limited source
/// (1-texel stencil at the 2-texel band edge returned per-texel checker → the
/// mid-field texel mosaic); AO marched at half res and bilinear-expanded.
const CACHE_FORMAT_VERSION: u64 = 5;

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
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn bake_or_load(oracle: &SurfaceOracle) -> DerivedMaps {
    lunco_precompute::bake_or_load(&DerivedBake { oracle }, &lunco_assets::cache_dir())
}

/// Wasm counterpart of [`bake_or_load`]: `lunco_precompute`'s sync fs tier is
/// native-only (no-op on wasm), so the derived-maps cache reads/writes the async
/// OPFS blob store at this — already-async — bake seam instead. Same namespace
/// and key as native; the two maps pack into ONE blob ([`encode_derived_blob`]).
#[cfg(target_arch = "wasm32")]
async fn bake_or_load_web(oracle: &SurfaceOracle) -> DerivedMaps {
    use lunco_precompute::Bake;
    let bake = DerivedBake { oracle };
    let key_hex = lunco_precompute::key_hex(bake.key());
    if let Some(blob) = lunco_storage::opfs_blob::read(DerivedBake::NAMESPACE, &key_hex).await {
        if let Some(maps) = decode_derived_blob(&blob) {
            return maps;
        }
    }
    let maps = bake.bake();
    // Best-effort write-through; a failure only costs a rebake next load.
    let blob = encode_derived_blob(&maps);
    wasm_bindgen_futures::spawn_local(async move {
        lunco_storage::opfs_blob::write(DerivedBake::NAMESPACE, &key_hex, &blob).await;
    });
    maps
}

/// Single-blob OPFS layout for [`DerivedMaps`]: `[res: u32 LE][surface][normal]`
/// — both maps are `res²·4` RGBA8 bytes, so the lengths derive from `res` and
/// need no framing.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn encode_derived_blob(maps: &DerivedMaps) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(4 + maps.surface_rgba.len() + maps.normal_rgba.len());
    out.extend_from_slice(&(maps.res as u32).to_le_bytes());
    out.extend_from_slice(&maps.surface_rgba);
    out.extend_from_slice(&maps.normal_rgba);
    out
}

/// Decode [`encode_derived_blob`]'s layout, validating sizes — `None` (a cache
/// miss → rebake) on a truncated or foreign blob.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn decode_derived_blob(bytes: &[u8]) -> Option<DerivedMaps> {
    let res = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let len = res.checked_mul(res)?.checked_mul(4)?;
    let body = bytes.get(4..)?;
    if body.len() != len.checked_mul(2)? {
        return None;
    }
    Some(DerivedMaps {
        res,
        surface_rgba: body[..len].to_vec(),
        normal_rgba: body[len..].to_vec(),
    })
}

/// Pure bake (runs on the task pool): sample the derived layers off the composed
/// surface (analytic craters included → their slopes/AO land in the maps) and
/// pack them.
fn bake_derived(oracle: &SurfaceOracle) -> DerivedMaps {
    let half = oracle.half_extent() as f64;
    let region = Square { center: [0.0, 0.0], half };
    let res = LAYER_RES;
    let texel = 2.0 * half / res as f64;
    // Gate over-zoom synthesis at the map's texel size (the map is far coarser
    // than the synthetic detail — skip it, don't alias it).
    let limited = oracle.detail_limited(2.0 * texel);

    let normals = normal_map(&limited, &region, res);
    let slope = slope_map(&limited, &region, res);
    // AO is smooth by construction (a horizon integral over AO_RADIUS_FRAC of
    // the extent) — bake the hemisphere march at HALF res (¼ the cost; this was
    // the whole cold-bake wait) and bilinear-expand to pack resolution.
    let ao_res = (res / 2).max(1);
    let ao_limited = oracle.detail_limited(2.0 * (2.0 * half / ao_res as f64));
    let ao_small =
        ao_map(&ao_limited, &region, ao_res, half * AO_RADIUS_FRAC, AO_DIRS, AO_STEPS);
    let ao = lunco_terrain_core::upsample_bilinear(&ao_small, ao_res, res);
    // Tone: 3-texel curvature stencil on a source limited at 2× the stencil.
    // The old 1-texel stencil on the 2-texel-limited source sat exactly AT
    // Nyquist → per-texel checker noise → the hard texel mosaic at mid range.
    const TONE_STENCIL_TEXELS: f64 = 3.0;
    let tone_limited = oracle.detail_limited(2.0 * TONE_STENCIL_TEXELS * texel);
    let albedo = albedo_map(&tone_limited, &region, res, TONE_STENCIL_TEXELS);

    let roughness: Vec<f32> =
        slope.iter().map(|&s| roughness_from_slope(s, ROUGH_BASE, ROUGH_STEEP_RAD)).collect();
    let hazard: Vec<f32> =
        slope.iter().map(|&s| hazard_from_slope(s, SAFE_RAD, CLIFF_RAD)).collect();

    DerivedMaps {
        res,
        surface_rgba: pack_surface_rgba8(&roughness, &ao, &[], &hazard),
        normal_rgba: pack_normal_rgba8(&normals, &albedo),
    }
}

/// Upload finished bakes as linear RGBA8 textures and publish the handles as
/// [`TerrainDerivedMaps`]. Needs `Assets<Image>` (present whenever a bake was
/// started). A result baked against a superseded oracle is discarded — the
/// stale marker chain re-kicks a fresh bake against the current surface.
fn finish_derived_bakes(
    mut commands: Commands,
    time: Res<Time>,
    mut tasks: Query<(Entity, &mut DerivedBakeTask, &DemHeightField)>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let Some(mut images) = images else { return };
    for (entity, mut task, hf) in &mut tasks {
        let Some(maps) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        if task.1 != Arc::as_ptr(&hf.0) as usize {
            // Baked against a surface that no longer exists → drop, and RE-ARM
            // the stale marker (already past debounce): with the old maps kept
            // live through a bounded edit, `TerrainDerivedMaps` is still present,
            // so its absence can no longer be the re-kick signal.
            commands
                .entity(entity)
                .try_remove::<DerivedBakeTask>()
                .try_insert(DerivedMapsStale {
                    since: time.elapsed_secs_f64() - REBAKE_DEBOUNCE_SECS,
                });
            continue;
        }
        let res = maps.res;
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
            .try_insert(TerrainDerivedMaps { surface, normal, res });
    }
}

/// Bind the baked layers onto the terrain's own static-mesh material once it
/// exists (created async by the USD shader path → retry until ready), then mark
/// the terrain done. Streamed tiles bind separately in `stream_viz`.
fn apply_derived_layers(
    mut commands: Commands,
    q: Query<(Entity, &TerrainDerivedMaps, &MeshMaterial3d<ShaderMaterial>), Without<DerivedLayersBuilt>>,
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
        commands.entity(entity).try_insert(DerivedLayersBuilt);
        info!("[terrain-layers] bound DEM-derived surface+normal layers ({}²)", handles.res);
    }
}

/// Build the full RGBA8 box-filtered mip chain for a square `res²` texture.
/// Returns the concatenated level data (level 0 first) and the level count.
/// Mips matter here: these maps are sampled out to the horizon, and without
/// them distant texels shimmer and alias under the raking lunar sun.
fn mip_chain_rgba8(base: Vec<u8>, res: usize) -> (Vec<u8>, u32) {
    let mut all = base;
    let mut levels = 1u32;
    let mut prev_res = res;
    let mut prev_start = 0usize;
    while prev_res > 1 {
        let next_res = prev_res / 2;
        let prev = all[prev_start..prev_start + prev_res * prev_res * 4].to_vec();
        let mut next = Vec::with_capacity(next_res * next_res * 4);
        for y in 0..next_res {
            for x in 0..next_res {
                for c in 0..4 {
                    let i = |px: usize, py: usize| prev[(py * prev_res + px) * 4 + c] as u32;
                    let sum = i(2 * x, 2 * y)
                        + i(2 * x + 1, 2 * y)
                        + i(2 * x, 2 * y + 1)
                        + i(2 * x + 1, 2 * y + 1);
                    next.push(((sum + 2) / 4) as u8);
                }
            }
        }
        prev_start = all.len();
        prev_res = next_res;
        all.extend_from_slice(&next);
        levels += 1;
    }
    (all, levels)
}

/// A linear (non-sRGB) RGBA8 data texture with a full mip chain and
/// trilinear/anisotropic filtering — these carry roughness/AO/hazard scalars,
/// an encoded normal, and the albedo scalar, and are sampled out to the horizon.
fn data_texture(res: usize, rgba: Vec<u8>) -> Image {
    use bevy::image::ImageSamplerDescriptor;
    let (data, mip_levels) = mip_chain_rgba8(rgba, res);
    // `new_uninit` + manual data: `Image::new` debug-asserts data == base level,
    // but ours carries the whole mip chain.
    let mut image = Image::new_uninit(
        Extent3d { width: res as u32, height: res as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_levels;
    // Anisotropy keeps grazing-angle terrain (most of the screen) sharp instead
    // of mip-smeared. wgpu requires all-linear filters with anisotropy > 1.
    // WebGL2's anisotropy support is extension-dependent → stay isotropic there.
    #[cfg(not(target_arch = "wasm32"))]
    let anisotropy_clamp = 8;
    #[cfg(target_arch = "wasm32")]
    let anisotropy_clamp = 1;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        min_filter: bevy::image::ImageFilterMode::Linear,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        anisotropy_clamp,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// Register the derived-layer bake/bind systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (mark_derived_stale, start_derived_bakes, finish_derived_bakes, apply_derived_layers)
            .chain()
            // The `.after` inserts the sync point that makes `finish_dem_restamp`'s
            // deferred `DerivedDirtyRegion` insert visible in the same frame as its
            // (immediate) `DemHeightField` swap — unordered, `mark_derived_stale`
            // could see the swap without the bounded flag and needlessly drop the
            // published maps (the far-field pop this flag exists to prevent).
            .after(crate::terrain::finish_dem_restamp),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_blob_round_trips() {
        let maps = DerivedMaps {
            res: 2,
            surface_rgba: (0u8..16).collect(),
            normal_rgba: (100u8..116).collect(),
        };
        let blob = encode_derived_blob(&maps);
        let back = decode_derived_blob(&blob).expect("decodes");
        assert_eq!(back.res, maps.res);
        assert_eq!(back.surface_rgba, maps.surface_rgba);
        assert_eq!(back.normal_rgba, maps.normal_rgba);
    }

    #[test]
    fn derived_blob_rejects_truncation() {
        let maps = DerivedMaps {
            res: 2,
            surface_rgba: vec![0; 16],
            normal_rgba: vec![0; 16],
        };
        let blob = encode_derived_blob(&maps);
        assert!(decode_derived_blob(&blob[..blob.len() - 1]).is_none());
        assert!(decode_derived_blob(&blob[..3]).is_none());
    }
}
