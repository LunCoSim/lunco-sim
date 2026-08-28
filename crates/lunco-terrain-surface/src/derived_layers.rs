//! P3b-runtime: bake the DEM-derived data layers into GPU textures and publish
//! them for every terrain render path.
//!
//! The pure math is [`lunco_terrain_core::derive`]; this is its Bevy half. For
//! each terrain that carries a retained height field ([`DemHeightField`]) we
//! bake — **off the main thread** — two mipped RGBA8 textures:
//!
//! - `surface_map` (binding 6/7): R=roughness G=AO B=rockDensity A=unused, and
//! - `normal_map`  (binding 8/9): the DEM-local ENU meso normal, with the
//!   relief-correlated **albedo scalar in alpha**,
//!
//! and publish them as a [`TerrainDerivedMaps`] component. Consumers:
//!
//! - the **streamed-tile path** (`stream_viz`) sets them as the `Surface`/`Normal`
//!   texture layers of every LOD tile's `ShaderLook` — this is what carries crater
//!   rims / AO / tonal variation at distances where tile geometry and the procedural
//!   FBM have LOD'd away;
//! - the **static-mesh path** binds them onto the terrain's own `ShaderMaterial`
//!   (`terrain_layered.wgsl` slots). That semantic material is created by
//!   `lunco-render-bevy`, so there is no intent component to restate — filling its
//!   slots means naming `MeshMaterial3d`, and that lives in `lunco-render-bevy`
//!   (`terrain_maps.rs`), the one crate allowed to. It is why this crate can publish
//!   the maps without linking `bevy_pbr`.
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
//! [`TerrainDerivedMaps`]). The two consumers above then bind from the published
//! component.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};
// `wgpu-types`, not `bevy::render` — these are plain POD texture descriptors
// (`bevy_image` itself takes them from here) and carry no pipeline, no wgpu device,
// no naga. `bevy::render::render_resource` merely re-exports them, and importing it
// would drag the whole GPU stack in. See docs/architecture/render-decoupling.md.
use web_time::Instant;
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

use lunco_terrain_core::{
    albedo_map, ao_map, normal_slope_maps, pack_normal_rgba8, pack_surface_rgba8,
    roughness_from_slope, BoundedHeightSource, Square,
};

use crate::band::SurfaceBand;
use crate::oracle::SurfaceOracle;
use crate::stream_viz::{DemHeightField, TerrainLodViz};
use crate::terrain::{DemVisualTargetRes, TERRAIN_WORK_MAX_SECS};

/// Telemetry event emitted when an optional visual bake exceeds its liveness
/// bound. The terrain itself remains a valid scene product; this event reports
/// only the unavailable refinement.
pub const TERRAIN_DERIVED_FAILED: &str = "TERRAIN_DERIVED_FAILED";

/// Optional visual products derived from a ready DEM. The terrain surface and
/// physics become usable before this work finishes; this resource only reports
/// the non-blocking visual refinement owned by this module.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TerrainDerivedStatus {
    /// At least one terrain is waiting for or running a derived-map bake.
    pub active: bool,
    /// Terrains with a current derived-map product and no replacement pending.
    pub ready: usize,
    /// Terrains that can publish a derived-map product in the current render host.
    pub total: usize,
    /// Bakes that are queued or running.
    pub pending: usize,
    /// Terrains whose optional visual product was cancelled after the watchdog.
    pub failed: usize,
}

/// Physical spacing of texel-centred cells over a `[-half_extent, +half_extent]`
/// square. This is the one definition used by both the baker's filters and the
/// renderer metadata, so their frequency boundary cannot drift.
fn raster_texel_size_m(half_extent: f64, res: usize) -> f64 {
    assert!(res > 0, "derived terrain map cannot have zero resolution");
    let texel_size_m = 2.0 * half_extent / res as f64;
    assert!(
        texel_size_m.is_finite() && texel_size_m > 0.0,
        "derived terrain map requires a finite positive physical texel size"
    );
    texel_size_m
}

/// One-shot marker: this terrain's derived layers are bound onto its own
/// static-mesh `ShaderMaterial`. Stops re-scanning. Streamed tiles don't use
/// this — they read [`TerrainDerivedMaps`] directly.
#[derive(Component)]
pub struct DerivedLayersBuilt;

/// The published derived maps for a terrain — GPU handles every terrain render
/// path binds from. `surface` packs R=roughness G=AO B=rockDensity A=unused;
/// `normal` packs the DEM-local ENU meso normal in RGB and the albedo scalar in A. Removed
/// (and re-baked) when the surface changes.
#[derive(Component, Clone)]
pub struct TerrainDerivedMaps {
    pub surface: Handle<Image>,
    pub normal: Handle<Image>,
    /// Texels per side, retained for diagnostics and static-material reporting.
    pub res: usize,
    /// Physical spacing between adjacent level-zero texel centres in terrain
    /// local metres. This is the authoritative material-detail scale; consumers
    /// must not reconstruct it from mesh LOD depth.
    pub texel_size_m: f32,
}

/// The **authored** layer maps for a terrain, read off its bound UsdShade
/// Material network (`inputs:albedo_map` / `inputs:mineral_map` and their
/// `inputs:weight_*`) — the counterpart to [`TerrainDerivedMaps`], which the
/// engine bakes rather than the author supplying.
///
/// Published as a component for the same reason the derived maps are: BOTH
/// terrain render paths need them. Before this existed only the static-mesh
/// path bound authored rasters, so a site with `lodViz = true` — what every
/// real DEM site uses — could bake a true NAC orthophoto, wire it correctly
/// through the network, and still render pure procedural regolith. The map was
/// authored, resolved, loaded, and never sampled.
///
/// The reader (`bind_terrain_layers`) lives in the editor crate because it
/// needs the composed stage; it publishes here so the streaming path can
/// consume the result without depending on the editor.
#[derive(Component, Clone, Default)]
pub struct TerrainAuthoredMaps {
    /// `inputs:albedo_map` — the site's real colour mosaic.
    pub albedo: Option<Handle<Image>>,
    /// `inputs:mineral_map` — a classification/analysis drape, composited
    /// UNLIT so it stays readable in shadow (doc 18 §4).
    pub mineral: Option<Handle<Image>>,
    /// `inputs:weight_albedo`; 0 = pure procedural.
    pub weight_albedo: f32,
    /// `inputs:weight_mineral`; 0 = no drape.
    pub weight_mineral: f32,
}

/// The in-flight off-thread bake for a terrain's derived layers, plus the
/// identity (Arc pointer) of the oracle it was started against — a re-compose
/// mid-bake makes the result stale, and [`finish_derived_bakes`] discards it.
/// The task's start time is local to this entity; a late queued terrain must not
/// inherit another terrain's watchdog age.
#[derive(Component)]
struct DerivedBakeTask(Task<DerivedMipped>, usize, Instant);

/// A terminal state for the optional visual product. The procedural terrain
/// material remains a valid authored semantic default, so a failed derived bake
/// must be reported and stopped rather than restarted forever.
#[derive(Component)]
struct DerivedMapsBuildFailed;

/// Debounce marker: the surface changed at `since`; wait for a short quiescent
/// window before re-baking so a burst of brush strokes coalesces into one bake.
#[derive(Component)]
struct DerivedMapsStale {
    since: f64,
}

/// The subset of Graphics settings that changes the derived-map bake. Keeping
/// this signature separate from the whole quality resource prevents a shadow or
/// camera edit from needlessly rebaking every terrain's large textures.
#[derive(Resource, Default)]
struct DerivedQualitySignature(Option<(usize, usize, usize, u64, u32, u32)>);

fn derived_quality_signature(
    profile: lunco_render::RenderQualityProfile,
) -> (usize, usize, usize, u64, u32, u32) {
    (
        profile.terrain_derived_map_resolution,
        profile.terrain_derived_ao_directions,
        profile.terrain_derived_ao_steps,
        profile.terrain_derived_ao_radius_fraction.to_bits(),
        profile.terrain_derived_roughness_base.to_bits(),
        profile
            .terrain_derived_roughness_saturation_radians
            .to_bits(),
    )
}

/// A derived-map quality edit invalidates the published textures without
/// removing them. The old maps remain visible until the replacement finishes,
/// while the changed signature makes the cache key and bake output authoritative.
fn mark_derived_stale_on_quality_change(
    mut commands: Commands,
    time: Res<Time>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    mut signature: ResMut<DerivedQualitySignature>,
    terrains: Query<Entity, With<DemHeightField>>,
) {
    let Ok(profile) = quality.validated_profile() else {
        return;
    };
    let next = derived_quality_signature(profile);
    if signature.0 == Some(next) {
        return;
    }
    signature.0 = Some(next);
    for entity in &terrains {
        commands
            .entity(entity)
            .try_remove::<DerivedMapsBuildFailed>()
            .try_insert(DerivedMapsStale {
                since: time.elapsed_secs_f64(),
            });
    }
}

/// Seconds of surface quiescence before a re-bake starts.
const REBAKE_DEBOUNCE_SECS: f64 = 0.75;

/// Baked RGBA8 buffers + their square resolution, ready to upload as `Image`s.
/// Base level only — this is the cache/blob format; the mip chains are derived
/// from it inside the bake task ([`DerivedMipped`]).
struct DerivedMaps {
    res: usize,
    surface_rgba: Vec<u8>,
    normal_rgba: Vec<u8>,
}

/// [`DerivedMaps`] with the full mip chain prebuilt for each map (`(data,
/// level_count)` per [`mip_chain_rgba8`]). Built INSIDE the async bake body —
/// mipping the RGBA8 maps on the main thread after the off-thread bake was a
/// per-publish `Update` spike — so [`finish_derived_bakes`] only wraps the
/// buffers into `Image`s.
struct DerivedMipped {
    res: usize,
    surface: (Vec<u8>, u32),
    normal: (Vec<u8>, u32),
}

/// Fold the base-level maps into their uploaded form: one box-filtered mip
/// chain per map. Runs on the task pool (both the fresh-bake and cache-hit
/// paths mip here).
fn mip_maps(maps: DerivedMaps) -> DerivedMipped {
    let DerivedMaps {
        res,
        surface_rgba,
        normal_rgba,
    } = maps;
    DerivedMipped {
        res,
        surface: mip_chain_rgba8(surface_rgba, res),
        normal: mip_chain_rgba8(normal_rgba, res),
    }
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
        (
            Entity,
            Option<&DerivedDirtyRegion>,
            Has<TerrainDerivedMaps>,
            Has<DerivedMapsBuildFailed>,
        ),
        Changed<DemHeightField>,
    >,
) {
    for (entity, region, has_maps, has_failed) in &changed {
        if has_failed {
            commands
                .entity(entity)
                .try_remove::<DerivedMapsBuildFailed>();
        }
        if !has_maps {
            continue;
        }
        let bounded = region.is_some_and(|r| r.bounded);
        let mut e = commands.entity(entity);
        if !bounded {
            e.try_remove::<(TerrainDerivedMaps, DerivedLayersBuilt)>();
        }
        e.try_remove::<DerivedDirtyRegion>()
            .try_insert(DerivedMapsStale {
                since: time.elapsed_secs_f64(),
            });
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
    quality: Res<lunco_render::RenderingQualitySettings>,
    q: Query<
        (
            Entity,
            &DemHeightField,
            Option<&DerivedMapsStale>,
            Has<TerrainDerivedMaps>,
            Option<&DemVisualTargetRes>,
            Has<TerrainLodViz>,
        ),
        (Without<DerivedBakeTask>, Without<DerivedMapsBuildFailed>),
    >,
) {
    if images.is_none() {
        return; // headless: no render assets → no point baking visual layers.
    }
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "[terrain] invalid Graphics derived-map quality; retaining current maps: {reason}"
            );
            return;
        }
    };
    let now = time.elapsed_secs_f64();
    for (entity, hf, stale, has_maps, target_res, streamed) in &q {
        if has_maps && stale.is_none() {
            continue; // published and current — nothing to do.
        }
        if let Some(stale) = stale {
            if now - stale.since < REBAKE_DEBOUNCE_SECS {
                continue; // edits still landing — wait for quiescence.
            }
        }
        let oracle: Arc<SurfaceOracle> = hf.0.clone();
        let bake_profile = profile_for_terrain(
            profile,
            oracle.grid().res,
            target_res.map(|target| target.0),
            streamed,
        );
        let oracle_ptr = Arc::as_ptr(&hf.0) as usize;
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Off-thread body → own Tracy zone (per-system spans don't reach here).
            let _span = bevy::log::info_span!("terrain_derived_maps_bake").entered();
            #[cfg(not(target_arch = "wasm32"))]
            let maps = bake_or_load(&oracle, bake_profile);
            #[cfg(target_arch = "wasm32")]
            let maps = bake_or_load_web(&oracle, bake_profile).await;
            // Mip HERE, still off-thread — the chain build is real work and
            // must not run on the main thread at publish.
            mip_maps(maps)
        });
        // Despawn-safe: a load-time / edit re-instantiation can despawn this
        // terrain between queue time and apply — `try_insert` no-ops on a stale
        // entity instead of panicking the command buffer.
        commands
            .entity(entity)
            .try_remove::<DerivedMapsStale>()
            .try_insert(DerivedBakeTask(task, oracle_ptr, Instant::now()));
    }
}

/// Resolve the derived-map resolution from the two authoritative owners:
/// rendering quality supplies the upper bound, while a static terrain's
/// authored visual target and source grid supply the available visual detail.
/// Streamed terrain keeps the quality resolution because its geometry refines
/// independently of the static `targetRes` product.
fn profile_for_terrain(
    mut profile: lunco_render::RenderQualityProfile,
    source_res: usize,
    visual_target_res: Option<usize>,
    streamed: bool,
) -> lunco_render::RenderQualityProfile {
    if !streamed {
        let visual_res = visual_target_res
            .filter(|&res| res > 0)
            .unwrap_or(source_res)
            .min(source_res)
            .max(1);
        profile.terrain_derived_map_resolution = profile
            .terrain_derived_map_resolution
            .min(visual_res)
            .max(1);
    }
    profile
}

/// Bump when the bake math or packed layout changes, so stale cache entries are
/// simply never matched (content-addressed → no explicit invalidation).
/// v2: maps sample the composed `SurfaceOracle` (analytic craters/edits included)
/// and the key folds the oracle's modifier `content_key`.
/// v3: crater profile band-limited + continuous at reach (same crater
/// `content_key`, different sampled surface).
/// v4: albedo scalar packed into normal-map alpha; the map resolution became a
/// Graphics setting after this version.
/// v5: tone (albedo) derived with a 3-texel stencil on a 6-texel-limited source
/// (1-texel stencil at the 2-texel band edge returned per-texel checker → the
/// mid-field texel mosaic); AO marched at half res and bilinear-expanded.
/// v6: surface-map alpha is no longer a baked slope hazard (nothing sampled it;
/// hazard is a live per-pixel view off the `overlay_*` uniforms). The frozen
/// safe/cliff angles left the key with it.
/// v7: tone (albedo) marched at half res and bilinear-expanded — its source is
/// band-limited to 6 texels, so full-res sampling was resolving detail the
/// source cannot contain, at ~10 oracle evaluations per texel.
/// v8: derived-map resolution and AO sample counts became Graphics settings.
/// v9: AO radius and roughness transfer parameters became Graphics settings.
const CACHE_FORMAT_VERSION: u64 = 9;

/// The derived-layer bake as a [`lunco_precompute::Bake`] — the content-addressed
/// disk cache (Substrate B) owns the load/store/rebake orchestration; this only
/// declares *what* is baked, *how it keys*, and *how it serializes*.
struct DerivedBake<'a> {
    oracle: &'a SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
}

impl lunco_precompute::Bake for DerivedBake<'_> {
    type Output = DerivedMaps;
    const NAMESPACE: &'static str = "terrain/derived";

    /// Content hash of the canonical composed-surface identity plus every bake
    /// parameter. The oracle already owns the base-grid identity, so this stays
    /// O(1) even for a multi-million-sample DEM.
    fn key(&self) -> u64 {
        let grid = self.oracle.grid();
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(CACHE_FORMAT_VERSION);
        h.write_u64(self.oracle.surface_key());
        h.write_u64(grid.res as u64);
        h.write_u64(grid.half_extent.to_bits() as u64);
        h.write_u64(self.profile.terrain_derived_map_resolution as u64);
        h.write_u64(self.profile.terrain_derived_ao_directions as u64);
        h.write_u64(self.profile.terrain_derived_ao_steps as u64);
        h.write_u64(self.profile.terrain_derived_ao_radius_fraction.to_bits());
        h.write_u64(self.profile.terrain_derived_roughness_base.to_bits() as u64);
        h.write_u64(
            self.profile
                .terrain_derived_roughness_saturation_radians
                .to_bits() as u64,
        );
        h.finish()
    }

    fn bake(&self) -> DerivedMaps {
        bake_derived(self.oracle, self.profile)
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
        Some(DerivedMaps {
            res,
            surface_rgba,
            normal_rgba,
        })
    }
}

/// P4: content-addressed cache. Load the derived maps from disk if a bake with
/// the same surface + parameters was already persisted; otherwise bake and write
/// them through. Pure-function bake → byte-identical key across runs and peers, so
/// a second load (or a second peer) skips the expensive AO march. The machine-global
/// cache directory is shared with the rest of the asset stack.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn bake_or_load(
    oracle: &SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
) -> DerivedMaps {
    lunco_precompute::bake_or_load(&DerivedBake { oracle, profile }, &lunco_assets::cache_dir())
}

/// Wasm counterpart of [`bake_or_load`]: `lunco_precompute`'s sync fs tier is
/// native-only (no-op on wasm), so the derived-maps cache reads/writes the async
/// OPFS blob store at this — already-async — bake seam instead. Same namespace
/// and key as native; the two maps pack into ONE blob ([`encode_derived_blob`]).
#[cfg(target_arch = "wasm32")]
async fn bake_or_load_web(
    oracle: &SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
) -> DerivedMaps {
    use lunco_precompute::Bake;
    let bake = DerivedBake { oracle, profile };
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
    let mut out = Vec::with_capacity(4 + maps.surface_rgba.len() + maps.normal_rgba.len());
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
    if res == 0 {
        return None;
    }
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
fn bake_derived(
    oracle: &SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
) -> DerivedMaps {
    let half = oracle.half_extent() as f64;
    let region = Square {
        center: [0.0, 0.0],
        half,
    };
    let res = profile.terrain_derived_map_resolution;
    let texel = raster_texel_size_m(half, res);
    // Gate over-zoom synthesis at the map's texel size via the shared filter
    // policy (the map is far coarser than the synthetic detail — skip it, don't
    // alias it).
    // Region-scoped (the whole DEM square, plus a texel of stencil slack): the
    // map is one bake over a known box, so crater fields gather their placements
    // once here — and at this texel size the gate drops everything below ~2
    // texels outright instead of rejecting it 1024² times.
    let limited = SurfaceBand::visual(texel).limited_region(oracle, region, 2.0 * texel);

    // One derive pass: slope is `acos(n.y)` of the very normal just computed,
    // so the fused kernel halves the oracle samples per texel.
    let bounded = BoundedHeightSource::new(&limited, half);
    let (normals, slope) = normal_slope_maps(&bounded, &region, res);
    // AO is smooth by construction (a horizon integral over the configured
    // terrain-derived AO radius fraction of
    // the extent) — bake the hemisphere march at HALF res (¼ the cost; this was
    // the whole cold-bake wait) and bilinear-expand to pack resolution.
    let ao_res = (res / 2).max(1);
    let ao_texel = raster_texel_size_m(half, ao_res);
    // The AO march walks horizon rays out to the configured fraction of `half` from each
    // texel, so the scope must grow by that reach — a ray leaving the box would
    // otherwise sample a view the region prune never promised.
    let ao_limited = SurfaceBand::visual(ao_texel).limited_region(
        oracle,
        region,
        half * profile.terrain_derived_ao_radius_fraction + 2.0 * ao_texel,
    );
    let ao_bounded = BoundedHeightSource::new(&ao_limited, half);
    let ao_small = ao_map(
        &ao_bounded,
        &region,
        ao_res,
        half * profile.terrain_derived_ao_radius_fraction,
        profile.terrain_derived_ao_directions,
        profile.terrain_derived_ao_steps,
        half,
    );
    let ao = lunco_terrain_core::upsample_bilinear(&ao_small, ao_res, res);
    // Tone: 3-texel curvature stencil on a source limited at 2× the stencil.
    // The old 1-texel stencil on the 2-texel-limited source sat exactly AT
    // Nyquist → per-texel checker noise → the hard texel mosaic at mid range.
    const TONE_STENCIL_TEXELS: f64 = 3.0;
    // A stencil wider than the texel — a custom Nyquist multiple, not the plain
    // `visual(texel)` band. Named inline rather than given a constructor, since
    // the stencil width is a property of this one bake.
    let tone_limited = SurfaceBand {
        min_wavelength: 2.0 * TONE_STENCIL_TEXELS * texel,
    }
    .limited_region(oracle, region, 2.0 * TONE_STENCIL_TEXELS * texel);
    // Marched at HALF res and bilinear-expanded, like AO — and for a stronger
    // reason: the tone source is band-limited to `2·3·texel`, i.e. it carries
    // nothing finer than 6 texels, so a 2-texel sampling pitch is still three
    // times finer than its Nyquist. Sampling it at 1024² was resolving detail
    // the source provably does not contain, at ~10 oracle evaluations per texel
    // (a 5-tap Laplacian plus the slope probe) — the most expensive pass in the
    // whole bake. The stencil is halved IN TEXELS so its width IN METRES is
    // unchanged: same tone, a quarter of the samples.
    let tone_res = (res / 2).max(1);
    let tone_bounded = BoundedHeightSource::new(&tone_limited, half);
    let albedo_small = albedo_map(
        &tone_bounded,
        &region,
        tone_res,
        TONE_STENCIL_TEXELS * 0.5,
        half,
    );
    let albedo = lunco_terrain_core::upsample_bilinear(&albedo_small, tone_res, res);

    let roughness: Vec<f32> = slope
        .iter()
        .map(|&s| {
            roughness_from_slope(
                s,
                profile.terrain_derived_roughness_base,
                profile.terrain_derived_roughness_saturation_radians,
            )
        })
        .collect();

    DerivedMaps {
        res,
        surface_rgba: pack_surface_rgba8(&roughness, &ao, &[]),
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
    quality: Res<lunco_render::RenderingQualitySettings>,
    mut tasks: Query<(Entity, &mut DerivedBakeTask, &DemHeightField)>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let Some(mut images) = images else { return };
    let Ok(profile) = quality.validated_profile() else {
        return;
    };
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
        // Derived rasters are texel-centred cells over the full terrain square:
        // adjacent centres are exactly `width / res` apart (derive::texel_world).
        let texel_size_m = raster_texel_size_m(hf.0.half_extent() as f64, res) as f32;
        let surface = images.add(data_texture(
            maps.res,
            maps.surface,
            profile.terrain_derived_texture_anisotropy,
        ));
        let normal = images.add(data_texture(
            maps.res,
            maps.normal,
            profile.terrain_derived_texture_anisotropy,
        ));
        // `try_*`: a terrain re-bake / doc-backed scene reload can despawn +
        // re-instantiate this terrain while its derived-layer bake is still in flight,
        // so the entity may be gone by the time these deferred commands apply. No-op
        // silently rather than panicking the whole app (as the sibling terrain systems
        // already do — `scatter_terrain_layers`, `finish_dem_restamp`).
        commands
            .entity(entity)
            .try_remove::<DerivedBakeTask>()
            .try_insert(TerrainDerivedMaps {
                surface,
                normal,
                res,
                texel_size_m,
            });
    }
}

/// Cancel an optional visual bake before the outgoing scene is despawned. The
/// task handle is scene-owned just like the DEM task; leaving it in the compute
/// pool would let the previous Twin consume capacity while the replacement is
/// loading. The published maps are also scene-owned and must not be inherited.
fn cancel_derived_bakes_on_scene_teardown(
    mut commands: Commands,
    entities: Query<
        Entity,
        Or<(
            With<DerivedBakeTask>,
            With<DerivedMapsStale>,
            With<DerivedDirtyRegion>,
            With<TerrainDerivedMaps>,
            With<TerrainAuthoredMaps>,
            With<DerivedLayersBuilt>,
            With<DerivedMapsBuildFailed>,
        )>,
    >,
    mut status: ResMut<TerrainDerivedStatus>,
) {
    for entity in &entities {
        commands.entity(entity).try_remove::<(
            DerivedBakeTask,
            DerivedMapsStale,
            DerivedDirtyRegion,
            TerrainDerivedMaps,
            TerrainAuthoredMaps,
            DerivedLayersBuilt,
            DerivedMapsBuildFailed,
        )>();
    }
    *status = TerrainDerivedStatus::default();
}

/// Publish the optional visual lifecycle and cancel a bake that has stopped
/// making forward progress. This is the terrain crate's authoritative state;
/// the application only mirrors it onto the existing status bus.
fn update_derived_status(
    mut commands: Commands,
    images: Option<Res<Assets<Image>>>,
    terrains: Query<
        (
            Entity,
            Has<TerrainDerivedMaps>,
            Option<&DerivedBakeTask>,
            Has<DerivedMapsStale>,
            Has<DerivedMapsBuildFailed>,
        ),
        With<DemHeightField>,
    >,
    mut status: ResMut<TerrainDerivedStatus>,
) {
    if images.is_none() {
        *status = TerrainDerivedStatus::default();
        return;
    }

    let mut ready = 0;
    let mut total = 0;
    let mut pending = 0;
    let mut failed = 0;
    for (_, has_maps, task, stale, build_failed) in &terrains {
        let has_task = task.is_some();
        total += 1;
        ready += usize::from(has_maps && !has_task && !stale && !build_failed);
        failed += usize::from(build_failed);
        pending += usize::from(has_task || stale);
    }

    if pending == 0 {
        *status = TerrainDerivedStatus {
            active: false,
            ready,
            total,
            pending,
            failed,
        };
        return;
    }

    let mut cancelled = 0;
    for (entity, _, task, _, build_failed) in &terrains {
        let Some(task) = task else {
            continue;
        };
        if Instant::now()
            .saturating_duration_since(task.2)
            .as_secs_f32()
            <= TERRAIN_WORK_MAX_SECS
        {
            continue;
        }
        commands
            .entity(entity)
            .try_remove::<(DerivedBakeTask, DerivedMapsStale)>();
        if !build_failed {
            commands.entity(entity).try_insert(DerivedMapsBuildFailed);
            cancelled += 1;
        }
    }
    if cancelled > 0 {
        commands.trigger(lunco_core::TelemetryEvent {
            name: TERRAIN_DERIVED_FAILED.to_owned(),
            source: 0,
            severity: lunco_core::Severity::Warning,
            data: lunco_core::TelemetryValue::String(format!(
                "{cancelled} terrain visual bake(s) exceeded {TERRAIN_WORK_MAX_SECS:.0}s and were cancelled"
            )),
            timestamp: 0.0,
        });
    }

    let remaining_pending = pending.saturating_sub(cancelled);
    *status = TerrainDerivedStatus {
        active: remaining_pending > 0,
        ready,
        total,
        pending: remaining_pending,
        failed: failed + cancelled,
    };
}

/// Build the full RGBA8 box-filtered mip chain for a square `res²` texture.
/// Returns the concatenated level data (level 0 first) and the level count.
/// Mips matter here: these maps are sampled out to the horizon, and without
/// them distant texels shimmer and alias under the raking lunar sun.
fn mip_chain_rgba8(base: Vec<u8>, res: usize) -> (Vec<u8>, u32) {
    // Size the whole chain up front so each level is written IN PLACE into the
    // one buffer (a disjoint `split_at_mut` window) — the old grow-as-you-go
    // loop `to_vec()`d every source level just to appease the borrow checker.
    let mut total = 0usize;
    let mut levels = 0u32;
    let mut r = res;
    loop {
        total += r * r * 4;
        levels += 1;
        if r <= 1 {
            break;
        }
        r /= 2;
    }
    let mut all = base;
    all.resize(total, 0);
    let mut prev_res = res;
    let mut prev_start = 0usize;
    while prev_res > 1 {
        let next_res = prev_res / 2;
        let next_start = prev_start + prev_res * prev_res * 4;
        // Read the previous level, write the next — disjoint halves of `all`.
        let (head, tail) = all.split_at_mut(next_start);
        let prev = &head[prev_start..];
        let next = &mut tail[..next_res * next_res * 4];
        for y in 0..next_res {
            for x in 0..next_res {
                for c in 0..4 {
                    let i = |px: usize, py: usize| prev[(py * prev_res + px) * 4 + c] as u32;
                    let sum = i(2 * x, 2 * y)
                        + i(2 * x + 1, 2 * y)
                        + i(2 * x, 2 * y + 1)
                        + i(2 * x + 1, 2 * y + 1);
                    next[(y * next_res + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        prev_start = next_start;
        prev_res = next_res;
    }
    (all, levels)
}

/// A linear (non-sRGB) RGBA8 data texture with a full mip chain and
/// trilinear/anisotropic filtering — these carry the roughness/AO scalars,
/// an encoded normal, and the albedo scalar, and are sampled out to the horizon.
/// Takes the PREBUILT chain (`(data, level_count)` from [`mip_chain_rgba8`],
/// run in the bake task) — this only wraps it in an `Image`.
fn data_texture(res: usize, (data, mip_levels): (Vec<u8>, u32), anisotropy_clamp: u16) -> Image {
    use bevy::image::ImageSamplerDescriptor;
    // `new_uninit` + manual data: `Image::new` debug-asserts data == base level,
    // but ours carries the whole mip chain.
    let mut image = Image::new_uninit(
        Extent3d {
            width: res as u32,
            height: res as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_levels;
    // Anisotropy keeps grazing-angle terrain (most of the screen) sharp instead
    // of mip-smeared. The requested value is an explicit Graphics setting; it is
    // never replaced with a platform-specific lower quality here.
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
    app.init_resource::<DerivedQualitySignature>()
        .init_resource::<TerrainDerivedStatus>()
        .add_systems(
            lunco_core::SceneTeardown,
            cancel_derived_bakes_on_scene_teardown,
        );
    app.add_systems(
        Update,
        // The static-mesh bind (`lunco-render-bevy`'s `apply_derived_layers`) is no
        // longer in this chain: it names a material, so it lives on the render side.
        // It retries until the async USD material exists, so it needs no ordering.
        (
            mark_derived_stale_on_quality_change
                .run_if(resource_changed::<lunco_render::RenderingQualitySettings>),
            mark_derived_stale,
            start_derived_bakes,
            finish_derived_bakes,
            update_derived_status,
        )
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
    use lunco_precompute::Bake;

    #[test]
    fn baker_and_renderer_share_exact_texel_spacing() {
        assert_eq!(raster_texel_size_m(4_096.0, 1_024), 8.0);
    }

    #[test]
    fn static_derived_resolution_is_bounded_by_its_visual_product() {
        let profile = lunco_render::RenderingQuality::Balanced.profile();
        assert_eq!(
            profile_for_terrain(profile, 3_200, Some(32), false).terrain_derived_map_resolution,
            32
        );
        assert_eq!(
            profile_for_terrain(profile, 3_200, Some(32), true).terrain_derived_map_resolution,
            profile.terrain_derived_map_resolution
        );
        assert_eq!(
            profile_for_terrain(profile, 32, Some(0), false).terrain_derived_map_resolution,
            32
        );
    }

    #[test]
    fn derived_key_uses_the_oracles_canonical_surface_identity() {
        let first = Arc::new(lunco_obstacle_field::field::HeightGrid {
            res: 2,
            half_extent: 1.0,
            heights: vec![0.0; 4],
        });
        let second = Arc::new(lunco_obstacle_field::field::HeightGrid {
            res: 2,
            half_extent: 1.0,
            heights: vec![1.0; 4],
        });
        let first_oracle = SurfaceOracle::new_with_base_key(first, Vec::new(), 42);
        let second_oracle = SurfaceOracle::new_with_base_key(second, Vec::new(), 42);
        let profile = lunco_render::RenderingQuality::Balanced.profile();

        assert_eq!(
            DerivedBake {
                oracle: &first_oracle,
                profile,
            }
            .key(),
            DerivedBake {
                oracle: &second_oracle,
                profile,
            }
            .key(),
            "the bake key follows the oracle identity, not a second DEM hash"
        );
    }

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

    #[test]
    fn derived_blob_rejects_zero_resolution() {
        assert!(decode_derived_blob(&0_u32.to_le_bytes()).is_none());
    }

    #[test]
    fn scene_teardown_removes_derived_state_and_resets_status() {
        let mut app = App::new();
        app.init_resource::<TerrainDerivedStatus>().add_systems(
            lunco_core::SceneTeardown,
            cancel_derived_bakes_on_scene_teardown,
        );
        let entity = app.world_mut().spawn(DerivedMapsBuildFailed).id();
        *app.world_mut().resource_mut::<TerrainDerivedStatus>() = TerrainDerivedStatus {
            active: true,
            ready: 0,
            total: 1,
            pending: 1,
            failed: 0,
        };

        lunco_core::run_scene_teardown(app.world_mut());

        assert!(app
            .world()
            .get_entity(entity)
            .is_ok_and(|entity| !entity.contains::<DerivedMapsBuildFailed>()));
        assert_eq!(
            *app.world().resource::<TerrainDerivedStatus>(),
            TerrainDerivedStatus::default()
        );
    }
}
