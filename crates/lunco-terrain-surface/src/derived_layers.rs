//! P3b-runtime: bake the DEM-derived data layers into GPU textures and publish
//! them for every terrain render path.
//!
//! The pure math is [`lunco_terrain_core::derive`]; this is its Bevy half. For
//! each terrain that carries a retained height field ([`DemHeightField`]) we
//! bake two mipped RGBA8 textures (on native workers; cooperative CPU/OPFS
//! work on the browser page executor):
//!
//! - `surface_map` (binding 6/7): R=roughness G=AO B=rockDensity A=unused, and
//! - `normal_map`  (binding 8/9): the DEM-local ENU meso normal, with the
//!   relief-correlated **albedo scalar in alpha**,
//!
//! and publish them as a [`TerrainDerivedMaps`] component. Consumers:
//!
//! - the **streamed-tile path** (`stream_viz`) selects them as the `Surface`/`Normal`
//!   texture layers of an LOD tile only when the corresponding USD role is absent.
//!   Geometry is published independently; the current raster is bound through
//!   the material owner when it arrives;
//! - the **static-mesh path** consumes them through the terrain owner's existing
//!   [`ShaderLook`](lunco_materials::ShaderLook) intent, using the same source
//!   reconciler as streamed tiles. No concrete material is created or mutated here;
//!   `lunco-render-bevy` remains only the generic intent binder.
//!
//! Render-gated by data, not `cfg`: the bake only starts when `Assets<Image>`
//! exists, so the headless server (no render assets) never bakes — it needs only
//! the collider. The maps are pure functions of the height field, so two peers
//! that *do* render derive byte-identical textures with nothing to transfer.
//!
//! Live edits: a brush/reseed swaps the `DemHeightField` Arc →
//! [`mark_derived_stale`] retains maps only for a bounded edit from their exact
//! source oracle, then after a short quiescence debounce the whole chain re-runs.
//!
//! Flow: [`mark_derived_stale`] → [`start_derived_bakes`] (one async task per
//! terrain) → [`finish_derived_bakes`] (upload as `Image`s + publish
//! [`TerrainDerivedMaps`]). The two consumers above then bind from the published
//! component.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::{ops::Range, sync::Arc};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::platform::time::Instant;
use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_core_runtime::async_work::ExternalWorkPermit;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_materials::{ParamValue, Rgba8MipMode, ShaderLook, TextureLayer, rgba8_mip_chain};
// `wgpu-types`, not `bevy::render` — these are plain POD texture descriptors
// (`bevy_image` itself takes them from here) and carry no pipeline, no wgpu device,
// no naga. `bevy::render::render_resource` merely re-exports them, and importing it
// would drag the whole GPU stack in. See docs/architecture/render-decoupling.md.
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

use lunco_terrain_core::derive::{AoRayLattice, albedo_map_rows, normal_slope_map_rows};
use lunco_terrain_core::{
    BoundedHeightSource, Square, pack_normal_rgba8, pack_surface_rgba8, roughness_from_slope,
};

use crate::band::SurfaceBand;
use crate::oracle::DemHeightField;
use crate::oracle::SurfaceOracle;
use crate::stream_viz::{LodTiles, TerrainDetailDemands, TerrainLodViz, TileShadowCache};
use crate::surface_change::TerrainSurfaceChange;
use crate::terrain::DemVisualTargetRes;

/// Visual products derived from a ready DEM. Interactive geometry and physics
/// become usable before this work finishes. Numbered recording requires the
/// requested final shading products.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TerrainDerivedStatus {
    /// At least one site raster or consumed-view clipmap has unfinished preparation.
    pub active: bool,
    /// Current shading products with no replacement pending.
    pub ready: usize,
    /// Required site-raster and view-clipmap products in this render host.
    pub total: usize,
    /// Products waiting for final derived shading, including GPU patch acknowledgement.
    pub pending: usize,
    /// Terminal failures for current source/quality requests.
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

/// The published derived maps for a terrain — GPU handles every terrain render
/// path binds from. `surface` packs R=roughness G=AO B=rockDensity A=unused;
/// `normal` packs the DEM-local ENU meso normal in RGB and the albedo scalar in A.
/// The source key decides whether maps can remain live during a bounded re-bake.
#[derive(Component, Clone)]
pub struct TerrainDerivedMaps {
    pub surface: Handle<Image>,
    pub normal: Handle<Image>,
    /// Oracle content the published maps were baked from.
    pub surface_key: u64,
    /// Content identity of the actual published raster and its bake quality.
    pub bake_key: u64,
    /// Texels per side, retained for diagnostics and static-material reporting.
    pub res: usize,
    /// Physical spacing between adjacent level-zero texel centres in terrain
    /// local metres. This is the authoritative material-detail scale; consumers
    /// must not reconstruct it from mesh LOD depth.
    pub texel_size_m: f32,
}

/// The **authored** layer maps for a terrain, read off its bound UsdShade
/// Material network (`inputs:*_map` and their `inputs:weight_*`) — the
/// counterpart to [`TerrainDerivedMaps`], which the engine bakes rather than
/// the author supplying.
///
/// Published as a component for the same reason the derived maps are: BOTH
/// terrain render paths need them. The static and streamed paths consume the
/// same USD layer roles and weights; the streamed path chooses an engine-derived
/// product only for a role that USD did not author.
///
/// The generic USD shader projection already resolves the material network into
/// `ShaderLook`. This component is the terrain-specific view of those same
/// fixed texture roles; it is derived from the look rather than parsed again.
#[derive(Component, Clone, Default, PartialEq)]
pub struct TerrainAuthoredMaps {
    /// `inputs:albedo_map` — the site's authored material albedo. For an
    /// illumination-bearing grayscale orthophoto, the `kind = "albedo"`
    /// asset pipeline must produce this stable map before it is bound here.
    pub albedo: Option<lunco_materials::ShaderTexture>,
    /// Regional reference-geometry reflectance with explicit alpha coverage.
    pub regional_albedo: Option<lunco_materials::ShaderTexture>,
    /// `inputs:mineral_map` — a classification/analysis drape, composited
    /// UNLIT so it stays readable in shadow (doc 18 §4).
    pub mineral: Option<lunco_materials::ShaderTexture>,
    /// `inputs:weight_albedo`; 0 = pure procedural.
    pub weight_albedo: f32,
    /// `inputs:weight_mineral`; 0 = no drape.
    pub weight_mineral: f32,
    /// `inputs:surface_map` — packed roughness/AO/hazard data.
    pub surface: Option<lunco_materials::ShaderTexture>,
    /// `inputs:weight_rough`; controls the authored surface roughness channel.
    pub weight_rough: f32,
    /// `inputs:weight_ao`; controls the authored surface AO channel.
    pub weight_ao: f32,
    /// `inputs:normal_map` — a DEM-local ENU normal layer.
    pub normal: Option<lunco_materials::ShaderTexture>,
    /// `inputs:weight_normal`; 0 = no authored normal contribution.
    pub weight_normal: f32,
}

impl TerrainAuthoredMaps {
    /// Project the authored terrain roles already present in a generic `ShaderLook`.
    ///
    /// USD owns the material-network read. This conversion only gives the terrain
    /// stream a typed view of the existing texture handles and weights, so static
    /// and streamed terrain use the same source without a second USD reader. The
    /// engine-derived surface and normal layers are excluded by their engine-owned
    /// enable flags; otherwise a reconciliation pass would mistake its own output
    /// for newly authored USD input. The published derived handles are also used
    /// to cover the one-frame ordering where the map component arrives before the
    /// first runtime enable flags are written onto the look.
    pub(crate) fn from_shader_look(
        look: &ShaderLook,
        derived: Option<&TerrainDerivedMaps>,
    ) -> Self {
        let weight = |name: &str, map_present: bool| match look.values().get(name) {
            Some(ParamValue::F32(value)) => *value,
            _ if map_present => 1.0,
            _ => 0.0,
        };
        let albedo = look.textures.get(&TextureLayer::Albedo).cloned();
        let regional_albedo = look.textures.get(&TextureLayer::RegionalAlbedo).cloned();
        let mineral = look.textures.get(&TextureLayer::Mineral).cloned();
        let engine_layer_on = |name: &str| {
            matches!(
                look.values().get(name),
                Some(ParamValue::F32(value)) if *value > 0.5
            )
        };
        let derived_surface = engine_layer_on("derived_surface_on")
            || derived.is_some_and(|maps| {
                look.textures
                    .get(&TextureLayer::Surface)
                    .and_then(lunco_materials::ShaderTexture::image)
                    == Some(&maps.surface)
            });
        let derived_normal = engine_layer_on("derived_normal_on")
            || derived.is_some_and(|maps| {
                look.textures
                    .get(&TextureLayer::Normal)
                    .and_then(lunco_materials::ShaderTexture::image)
                    == Some(&maps.normal)
            });
        let surface = (!derived_surface)
            .then(|| look.textures.get(&TextureLayer::Surface).cloned())
            .flatten();
        let normal = (!derived_normal)
            .then(|| look.textures.get(&TextureLayer::Normal).cloned())
            .flatten();
        Self {
            weight_albedo: weight("weight_albedo", albedo.is_some()),
            weight_mineral: weight("weight_mineral", mineral.is_some()),
            weight_rough: weight("weight_rough", surface.is_some()),
            weight_ao: weight("weight_ao", surface.is_some()),
            weight_normal: weight("weight_normal", normal.is_some()),
            albedo,
            regional_albedo,
            mineral,
            surface,
            normal,
        }
    }

    /// Whether USD's surface map is an active material source. A zero-weight
    /// authored map is intentionally inactive, so the engine-derived product
    /// remains available for that role.
    pub fn has_active_surface(&self) -> bool {
        self.surface.is_some() && (self.weight_rough > 0.0 || self.weight_ao > 0.0)
    }

    /// Whether USD's normal map is an active material source.
    pub fn has_active_normal(&self) -> bool {
        self.normal.is_some() && self.weight_normal > 0.0
    }
}

/// The in-flight off-thread bake for a terrain's derived layers, plus the
/// exact oracle lease and requested quality identity. Recomposition or quality
/// changes retire the request; running results cannot publish after retirement.
#[derive(Component)]
struct DerivedBakeTask {
    source: Arc<SurfaceOracle>,
    requested_key: u64,
    #[cfg(not(target_arch = "wasm32"))]
    native: NativeDerivedTask,
    #[cfg(target_arch = "wasm32")]
    task: Task<(DerivedMipped, ExternalWorkPermit)>,
}

/// Byte reservation for incomplete row staging, distinct from worker permits.
/// A request releases each row's worker permit on receipt; otherwise a raster
/// larger than the worker width could not finish. Total staging is bounded by
/// the layout of one raster at the currently validated maximum quality.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct DerivedRasterAssemblyBudget(Arc<AtomicUsize>);

#[cfg(not(target_arch = "wasm32"))]
impl lunco_core::SceneResource for DerivedRasterAssemblyBudget {
    fn clear_scene(&mut self) {
        // Scene teardown drops all owner components. Cancelled workers retain
        // only their allocation reservations until their row exits and memory
        // is dropped; preserve that accounting across the scene boundary.
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct DerivedRasterReservation {
    used: Arc<AtomicUsize>,
    bytes: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for DerivedRasterReservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn raster_staging_bytes(res: usize) -> usize {
    let texels = res * res;
    let reduced = reduced_raster_res(res);
    let f32_bytes = std::mem::size_of::<f32>();
    let packed_texel = 2 * 4; // two RGBA8 maps
    let row_bytes = texels * (std::mem::size_of::<[f32; 3]>() + f32_bytes)
        + reduced * reduced * (2 * f32_bytes);
    let mut mip_dim = res;
    let mut mip_bytes = 0;
    loop {
        mip_bytes += mip_dim * mip_dim * packed_texel;
        if mip_dim == 1 {
            break;
        }
        mip_dim = (mip_dim / 2).max(1);
    }
    // Conservative sum: staged rows plus ordered concatenation, transient
    // slope and expanded AO/tone arrays, packed base levels, and full mip
    // chains. Some allocations do not overlap, but none is left unaccounted.
    2 * row_bytes + texels * (3 * f32_bytes + packed_texel) + mip_bytes
}

#[cfg(not(target_arch = "wasm32"))]
impl DerivedRasterAssemblyBudget {
    fn reserve(&self, res: usize, max_res: usize) -> Option<Arc<DerivedRasterReservation>> {
        let bytes = raster_staging_bytes(res);
        let limit = raster_staging_bytes(max_res);
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= limit)
            })
            .ok()?;
        Some(Arc::new(DerivedRasterReservation {
            used: self.0.clone(),
            bytes,
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct DerivedTransport<T> {
    _request: lunco_core_runtime::async_work::QueuedWorkRequest,
    result: Mutex<
        std::sync::mpsc::Receiver<(
            Option<T>,
            ExternalWorkPermit,
            Option<Arc<DerivedRasterReservation>>,
        )>,
    >,
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + 'static> DerivedTransport<T> {
    fn submit(
        admission: &AsyncWorkAdmission,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        allocation: Option<Arc<DerivedRasterReservation>>,
        job: impl FnOnce() -> Option<T> + Send + 'static,
    ) -> Option<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let request = admission
            .submit_transport(priority, key, move |permit| {
                let _ = sender.send((job(), permit, allocation));
            })
            .ok()?;
        Some(Self {
            _request: request,
            result: Mutex::new(receiver),
        })
    }

    fn poll(&mut self) -> Result<Option<T>, ()> {
        match self
            .result
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv()
        {
            Ok((Some(result), _permit, _allocation)) => Ok(Some(result)),
            Ok((None, _, _)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(()),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum DerivedCacheProbe {
    Hit(DerivedMipped),
    Miss {
        profile: lunco_render::RenderQualityProfile,
        directory: std::path::PathBuf,
    },
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeDerivedRows {
    plan: Arc<DerivedRasterPlan>,
    directory: std::path::PathBuf,
    next: usize,
    pending: Vec<(usize, DerivedTransport<DerivedRasterRows>)>,
    completed: Vec<Option<DerivedRasterRows>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct DerivedFinalizePayload {
    res: usize,
    directory: std::path::PathBuf,
    rows: Vec<DerivedRasterRows>,
}

#[cfg(not(target_arch = "wasm32"))]
enum NativeDerivedStage {
    Probe(DerivedTransport<DerivedCacheProbe>),
    Reserve {
        profile: lunco_render::RenderQualityProfile,
        directory: std::path::PathBuf,
    },
    Prepare {
        directory: std::path::PathBuf,
        job: DerivedTransport<DerivedRasterPlan>,
    },
    Rows(NativeDerivedRows),
    // Admission may refuse a submission while workers are busy. The payload
    // remains owned here until an accepted worker takes it exactly once.
    Assemble(Arc<Mutex<Option<DerivedFinalizePayload>>>),
    Finish(DerivedTransport<DerivedMipped>),
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeDerivedTask {
    priority: AsyncWorkPriority,
    cancelled: Arc<AtomicBool>,
    stage: NativeDerivedStage,
    // Dropped after the stage's payloads and requests; running workers own a
    // clone until their actual allocations have gone away.
    assembly: Option<Arc<DerivedRasterReservation>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for NativeDerivedTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
const DERIVED_PROBE_OPERATION: u64 = u64::MAX - 1;
#[cfg(not(target_arch = "wasm32"))]
const DERIVED_PREPARE_OPERATION: u64 = u64::MAX - 4;
#[cfg(not(target_arch = "wasm32"))]
const DERIVED_FINALIZE_OPERATION: u64 = u64::MAX - 5;
#[cfg(not(target_arch = "wasm32"))]
const DERIVED_ROW_OPERATION_BASE: u64 = u64::MAX - 1024;
#[cfg(not(target_arch = "wasm32"))]
const REDUCED_ROWS_PER_JOB: usize = 64;

#[cfg(not(target_arch = "wasm32"))]
fn derived_work_key(entity: Entity, requested_key: u64, operation: u64) -> AsyncWorkKey {
    AsyncWorkKey::new(
        AsyncWorkKind::VisualizationPreparation,
        0,
        entity.to_bits() as u128,
        requested_key,
        operation,
    )
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeDerivedTask {
    fn start(
        admission: &AsyncWorkAdmission,
        priority: AsyncWorkPriority,
        entity: Entity,
        requested_key: u64,
        source: Arc<SurfaceOracle>,
        profile: lunco_render::RenderQualityProfile,
        preview: bool,
    ) -> Option<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let probe = DerivedTransport::submit(
            admission,
            priority,
            derived_work_key(entity, requested_key, DERIVED_PROBE_OPERATION),
            None,
            move || probe_native_cache(&source, profile, preview, &cancellation),
        )?;
        Some(Self {
            priority,
            cancelled,
            stage: NativeDerivedStage::Probe(probe),
            assembly: None,
        })
    }

    /// Advance only owner bookkeeping on Update. Cache I/O, scope construction,
    /// every row range, ordered assembly, and mipping run as admitted workers.
    fn advance(
        &mut self,
        admission: &AsyncWorkAdmission,
        budget: &DerivedRasterAssemblyBudget,
        entity: Entity,
        requested_key: u64,
        source: &Arc<SurfaceOracle>,
        max_res: usize,
    ) -> Result<Option<DerivedMipped>, ()> {
        match &mut self.stage {
            NativeDerivedStage::Probe(job) => match job.poll()? {
                Some(DerivedCacheProbe::Hit(maps)) => return Ok(Some(maps)),
                Some(DerivedCacheProbe::Miss { profile, directory }) => {
                    self.stage = NativeDerivedStage::Reserve { profile, directory };
                }
                None => {}
            },
            NativeDerivedStage::Reserve { profile, directory } => {
                let Some(reservation) =
                    budget.reserve(profile.terrain_derived_map_resolution, max_res)
                else {
                    return Ok(None);
                };
                let allocation = reservation.clone();
                let source = source.clone();
                let profile = *profile;
                let cancellation = self.cancelled.clone();
                let Some(job) = DerivedTransport::submit(
                    admission,
                    self.priority,
                    derived_work_key(entity, requested_key, DERIVED_PREPARE_OPERATION),
                    Some(allocation),
                    move || {
                        if cancellation.load(Ordering::Acquire) {
                            return None;
                        }
                        Some(DerivedRasterPlan::new(&source, profile))
                    },
                ) else {
                    return Ok(None);
                };
                self.assembly = Some(reservation);
                self.stage = NativeDerivedStage::Prepare {
                    directory: directory.clone(),
                    job,
                };
            }
            NativeDerivedStage::Prepare { directory, job } => {
                if let Some(plan) = job.poll()? {
                    let chunk_count =
                        reduced_raster_res(plan.profile.terrain_derived_map_resolution)
                            .div_ceil(REDUCED_ROWS_PER_JOB);
                    let completed = std::iter::repeat_with(|| None).take(chunk_count).collect();
                    self.stage = NativeDerivedStage::Rows(NativeDerivedRows {
                        plan: Arc::new(plan),
                        directory: directory.clone(),
                        next: 0,
                        pending: Vec::new(),
                        completed,
                    });
                }
            }
            NativeDerivedStage::Rows(rows) => {
                // Completed worker slots are released as ranges move into
                // the bounded assembly reservation. Arrival order never
                // decides image row order.
                let mut i = 0;
                while i < rows.pending.len() {
                    if let Some(data) = rows.pending[i].1.poll()? {
                        let (index, _) = rows.pending.swap_remove(i);
                        rows.completed[index] = Some(data);
                    } else {
                        i += 1;
                    }
                }
                if rows.next == rows.completed.len() && rows.pending.is_empty() {
                    let payload = DerivedFinalizePayload {
                        res: rows.plan.profile.terrain_derived_map_resolution,
                        directory: rows.directory.clone(),
                        rows: rows
                            .completed
                            .iter_mut()
                            .map(Option::take)
                            .collect::<Option<Vec<_>>>()
                            .ok_or(())?,
                    };
                    self.stage = NativeDerivedStage::Assemble(Arc::new(Mutex::new(Some(payload))));
                    return Ok(None);
                }
                while rows.next < rows.completed.len() {
                    let index = rows.next;
                    let start = index * REDUCED_ROWS_PER_JOB;
                    let end = (start + REDUCED_ROWS_PER_JOB).min(reduced_raster_res(
                        rows.plan.profile.terrain_derived_map_resolution,
                    ));
                    let plan = rows.plan.clone();
                    let cancellation = self.cancelled.clone();
                    let allocation = self.assembly.as_ref().ok_or(())?.clone();
                    let operation = DERIVED_ROW_OPERATION_BASE + index as u64;
                    let Some(job) = DerivedTransport::submit(
                        admission,
                        self.priority,
                        derived_work_key(entity, requested_key, operation),
                        Some(allocation),
                        move || {
                            let _span =
                                bevy::log::info_span!("terrain_derived_maps_bake_rows").entered();
                            plan.sample_rows(start..end, || cancellation.load(Ordering::Acquire))
                        },
                    ) else {
                        break;
                    };
                    rows.pending.push((index, job));
                    rows.next += 1;
                }
            }
            NativeDerivedStage::Assemble(payload) => {
                let payload = payload.clone();
                let allocation = self.assembly.as_ref().ok_or(())?.clone();
                let cancellation = self.cancelled.clone();
                let Some(job) = DerivedTransport::submit(
                    admission,
                    self.priority,
                    derived_work_key(entity, requested_key, DERIVED_FINALIZE_OPERATION),
                    Some(allocation),
                    move || {
                        use lunco_precompute::Bake;
                        let payload = payload
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()?;
                        if cancellation.load(Ordering::Acquire) {
                            return None;
                        }
                        let _span =
                            bevy::log::info_span!("terrain_derived_maps_assemble").entered();
                        let maps = assemble_derived_rows(payload.res, payload.rows, || {
                            cancellation.load(Ordering::Acquire)
                        })?;
                        if cancellation.load(Ordering::Acquire) {
                            return None;
                        }
                        // The same content-addressed format; failure to persist
                        // is only a cache miss on a later load.
                        let _ = DerivedBake::store(&payload.directory, &maps);
                        if cancellation.load(Ordering::Acquire) {
                            return None;
                        }
                        let maps = mip_maps(maps);
                        (!cancellation.load(Ordering::Acquire)).then_some(maps)
                    },
                ) else {
                    return Ok(None);
                };
                self.stage = NativeDerivedStage::Finish(job);
            }
            NativeDerivedStage::Finish(job) => return job.poll(),
        }
        Ok(None)
    }
}

/// A terminal failure for one exact requested source/quality lease.
#[derive(Component)]
struct DerivedMapsFailed {
    source: Arc<SurfaceOracle>,
    requested_key: u64,
}

/// Debounce marker: the surface changed at `since`; wait for a short quiescent
/// window before re-baking so a burst of brush strokes coalesces into one bake.
#[derive(Component)]
struct DerivedMapsStale {
    since: Instant,
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
    quality: Res<lunco_render::RenderingQualitySettings>,
    mut signature: ResMut<DerivedQualitySignature>,
    terrains: Query<Entity, (With<DemHeightField>, With<TerrainDerivedMaps>)>,
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
        commands.entity(entity).try_insert(DerivedMapsStale {
            since: Instant::now(),
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
/// level_count)` per [`lunco_materials::rgba8_mip_chain`]). Built INSIDE the
/// async bake body —
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
        surface: rgba8_mip_chain(surface_rgba, res, res, Rgba8MipMode::Linear)
            .expect("derived surface map dimensions must match its RGBA8 buffer"),
        normal: rgba8_mip_chain(normal_rgba, res, res, Rgba8MipMode::Normal)
            .expect("derived normal map dimensions must match its RGBA8 buffer"),
    }
}

/// A surface re-compose swapped the `DemHeightField` Arc: arm the re-bake
/// debounce. For a bounded change in the shared [`TerrainSurfaceChange`] the
/// published maps stay live while the fresh bake runs — they are correct
/// everywhere except the edit's footprint. A whole-terrain or unrecognized
/// change drops them because globally stale maps are worse than the fallback.
fn mark_derived_stale(
    mut commands: Commands,
    changed: Query<
        (
            Entity,
            &DemHeightField,
            Option<&TerrainSurfaceChange>,
            Option<&TerrainDerivedMaps>,
        ),
        Changed<DemHeightField>,
    >,
) {
    for (entity, height_field, surface_change, maps) in &changed {
        let Some(maps) = maps else {
            continue;
        };
        let bounded = surface_change.is_some_and(|change| {
            change.is_bounded_from(maps.surface_key, height_field.0.surface_key())
        });
        let mut e = commands.entity(entity);
        if !bounded {
            e.try_remove::<TerrainDerivedMaps>();
        }
        e.try_insert(DerivedMapsStale {
            since: Instant::now(),
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
    quality: Res<lunco_render::RenderingQualitySettings>,
    admission: Res<AsyncWorkAdmission>,
    demands: Res<TerrainDetailDemands>,
    q: Query<
        (
            Entity,
            &DemHeightField,
            Option<&DerivedMapsStale>,
            Option<&TerrainDerivedMaps>,
            Option<&DemVisualTargetRes>,
            Has<TerrainLodViz>,
            Option<&LodTiles>,
            Option<&DerivedMapsFailed>,
        ),
        Without<DerivedBakeTask>,
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
    let now = Instant::now();
    for (entity, hf, stale, maps, target_res, streamed, tiles, failed) in &q {
        if let Some(stale) = stale {
            if now.duration_since(stale.since).as_secs_f64() < REBAKE_DEBOUNCE_SECS {
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
        if let Some(maps) = maps.filter(|maps| maps.bake_key == derived_key(&oracle, bake_profile))
        {
            if maps.surface_key != oracle.surface_key() {
                let mut current = maps.clone();
                current.surface_key = oracle.surface_key();
                commands.entity(entity).try_insert(current);
            }
            if stale.is_some() {
                commands.entity(entity).try_remove::<DerivedMapsStale>();
            }
            continue;
        }
        // Keep the initial shading product cheap. Full-quality refinement
        // competes only after the currently consumed near-camera covers land.
        if streamed
            && maps.is_some()
            && stale.is_none()
            && !demands.near_covers_ready(entity, tiles)
        {
            continue;
        }
        let requested_key = derived_key(&oracle, bake_profile);
        if failed.is_some_and(|failure| {
            Arc::ptr_eq(&failure.source, &oracle) && failure.requested_key == requested_key
        }) {
            continue;
        }
        #[cfg(target_arch = "wasm32")]
        let key = AsyncWorkKey::new(
            AsyncWorkKind::VisualizationPreparation,
            0,
            entity.to_bits() as u128,
            requested_key,
            u64::MAX - 1,
        );
        let priority = if maps.is_none() {
            AsyncWorkPriority::Interactive
        } else {
            AsyncWorkPriority::Background
        };
        #[cfg(not(target_arch = "wasm32"))]
        let task = {
            let preview = streamed && maps.is_none();
            let Some(native) = NativeDerivedTask::start(
                &admission,
                priority,
                entity,
                requested_key,
                oracle.clone(),
                bake_profile,
                preview,
            ) else {
                continue;
            };
            DerivedBakeTask {
                source: oracle,
                requested_key,
                native,
            }
        };
        #[cfg(target_arch = "wasm32")]
        let task = {
            let Ok(permit) = admission.admit_external(priority, key) else {
                continue;
            };
            let source = oracle.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                let maps = bake_or_load_web(&source, bake_profile).await;
                (mip_maps(maps), permit)
            });
            DerivedBakeTask {
                source: oracle,
                requested_key,
                task,
            }
        };
        commands
            .entity(entity)
            .try_remove::<(DerivedMapsStale, DerivedMapsFailed)>()
            .try_insert(task);
    }
}

/// Resolve the derived-map resolution from the two authoritative owners:
/// rendering quality supplies the upper bound, while a static terrain's
/// authored visual target and source grid supply the available visual detail.
/// Streamed terrain keeps the quality resolution because its geometry refines
/// independently of the static `targetRes` product.
pub(crate) fn profile_for_terrain(
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

/// The content key versions the current sampling math and packed raster layout.
const CACHE_FORMAT_VERSION: u64 = 10;

/// The derived-layer cache identity and serialization contract. Native miss
/// handling admits row jobs explicitly; the serial web consumer uses the same
/// bake kernels and cache identity.
struct DerivedBake<'a> {
    oracle: &'a SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
}

impl lunco_precompute::Bake for DerivedBake<'_> {
    type Output = DerivedMaps;
    const NAMESPACE: &'static str = "terrain/derived";

    /// The maps sample only the local footprint, so changing an exterior
    /// regional tier does not invalidate this bake. The oracle retains the
    /// local surface identity separately from its complete domain identity.
    fn key(&self) -> u64 {
        let grid = self.oracle.grid();
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(CACHE_FORMAT_VERSION);
        h.write_u64(self.oracle.local_surface_key());
        h.write_u64(grid.res as u64);
        h.write_u64(grid.half_extent.to_bits() as u64);
        let (res, directions, steps, radius, roughness, saturation) =
            derived_quality_signature(self.profile);
        h.write_u64(res as u64);
        h.write_u64(directions as u64);
        h.write_u64(steps as u64);
        h.write_u64(radius);
        h.write_u64(roughness as u64);
        h.write_u64(saturation as u64);
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
    fn load(&self, dir: &std::path::Path) -> Option<DerivedMaps> {
        let surface_rgba = lunco_precompute::load_blob(dir, "surface.bin")?;
        let normal_rgba = lunco_precompute::load_blob(dir, "normal.bin")?;
        derived_maps_from_buffers(
            self.profile.terrain_derived_map_resolution,
            surface_rgba,
            normal_rgba,
        )
    }
}

fn derived_maps_from_buffers(
    res: usize,
    surface_rgba: Vec<u8>,
    normal_rgba: Vec<u8>,
) -> Option<DerivedMaps> {
    let len = res.checked_mul(res)?.checked_mul(4)?;
    if res == 0 || surface_rgba.len() != len || normal_rgba.len() != len {
        return None;
    }
    Some(DerivedMaps {
        res,
        surface_rgba,
        normal_rgba,
    })
}

pub(crate) fn derived_key(
    oracle: &SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
) -> u64 {
    use lunco_precompute::Bake;
    DerivedBake { oracle, profile }.key()
}

/// Cache probe on an admitted worker, retaining the same key/layout and the
/// warm-final-first startup selection. A miss becomes immutable row work; the
/// shared Bake implementation owns both load and store, rather than duplicating
/// the cache format in the scheduler.
#[cfg(not(target_arch = "wasm32"))]
fn probe_native_cache(
    oracle: &SurfaceOracle,
    mut profile: lunco_render::RenderQualityProfile,
    preview: bool,
    cancelled: &AtomicBool,
) -> Option<DerivedCacheProbe> {
    use lunco_precompute::Bake;
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    const PREVIEW_RESOLUTION: usize = 256;
    let root = lunco_assets_core::cache_dir();
    let entry = |profile| {
        let bake = DerivedBake { oracle, profile };
        let directory = lunco_precompute::entry_dir(&root, DerivedBake::NAMESPACE, bake.key());
        (directory, bake)
    };
    let cached = |maps| {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let maps = mip_maps(maps);
        (!cancelled.load(Ordering::Acquire)).then_some(DerivedCacheProbe::Hit(maps))
    };
    if preview && profile.terrain_derived_map_resolution > PREVIEW_RESOLUTION {
        let (directory, bake) = entry(profile);
        if let Some(maps) = bake.load(&directory) {
            return cached(maps);
        }
        profile.terrain_derived_map_resolution = PREVIEW_RESOLUTION;
    }
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let (directory, bake) = entry(profile);
    if let Some(maps) = bake.load(&directory) {
        return cached(maps);
    }
    (!cancelled.load(Ordering::Acquire)).then_some(DerivedCacheProbe::Miss { profile, directory })
}

/// Web cache consumer: `lunco_precompute`'s sync fs tier is
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
        if let Some(maps) = decode_derived_blob(&blob)
            .filter(|maps| maps.res == profile.terrain_derived_map_resolution)
        {
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
#[cfg(any(target_arch = "wasm32", test))]
fn encode_derived_blob(maps: &DerivedMaps) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + maps.surface_rgba.len() + maps.normal_rgba.len());
    out.extend_from_slice(&(maps.res as u32).to_le_bytes());
    out.extend_from_slice(&maps.surface_rgba);
    out.extend_from_slice(&maps.normal_rgba);
    out
}

/// Decode [`encode_derived_blob`]'s layout, validating sizes — `None` (a cache
/// miss → rebake) on a truncated or foreign blob.
#[cfg(any(target_arch = "wasm32", test))]
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

/// Immutable preparation shared by every independently admitted row range.
/// Each source is filtered once over the full original sampling footprint so
/// partitioning cannot change crater placement or stencil values at chunk edges.
struct DerivedRasterPlan {
    profile: lunco_render::RenderQualityProfile,
    region: Square,
    normal_source: SurfaceOracle,
    ao_source: SurfaceOracle,
    tone_source: SurfaceOracle,
    rays: AoRayLattice,
}

const TONE_STENCIL_TEXELS: f64 = 3.0;

struct DerivedRasterRows {
    normals: Vec<[f32; 3]>,
    roughness: Vec<f32>,
    ao: Vec<f32>,
    tone: Vec<f32>,
}

impl DerivedRasterPlan {
    fn new(oracle: &SurfaceOracle, profile: lunco_render::RenderQualityProfile) -> Self {
        let half = oracle.half_extent() as f64;
        let region = Square {
            center: [0.0, 0.0],
            half,
        };
        let res = profile.terrain_derived_map_resolution;
        let texel = raster_texel_size_m(half, res);
        let reduced_texel = raster_texel_size_m(half, reduced_raster_res(res));
        let ao_radius = half * profile.terrain_derived_ao_radius_fraction;
        // Match the existing normal, half-resolution AO, and wider tone bands.
        let normal_source = SurfaceBand::visual(texel).limited_region(oracle, region, 2.0 * texel);
        let ao_source = SurfaceBand::visual(reduced_texel).limited_region(
            oracle,
            region,
            ao_radius + 2.0 * reduced_texel,
        );
        let tone_source = SurfaceBand::visual_wavelength(2.0 * TONE_STENCIL_TEXELS * texel)
            .limited_region(oracle, region, 2.0 * TONE_STENCIL_TEXELS * texel);
        let rays = AoRayLattice::new(
            ao_radius,
            profile.terrain_derived_ao_directions,
            profile.terrain_derived_ao_steps,
        );
        Self {
            profile,
            region,
            normal_source,
            ao_source,
            tone_source,
            rays,
        }
    }

    /// Reduced-resolution row indices choose the corresponding full-resolution
    /// interval, including odd static-map resolutions, with no overlap or gaps.
    fn sample_rows(
        &self,
        reduced_rows: Range<usize>,
        cancelled: impl Fn() -> bool,
    ) -> Option<DerivedRasterRows> {
        let res = self.profile.terrain_derived_map_resolution;
        let reduced = reduced_raster_res(res);
        let half = self.region.half;
        let full_rows = (reduced_rows.start * res / reduced)..(reduced_rows.end * res / reduced);
        let bounded = BoundedHeightSource::new(&self.normal_source, half);
        let (normals, slopes) =
            normal_slope_map_rows(&bounded, &self.region, res, full_rows, &cancelled)?;
        let roughness = slopes
            .into_iter()
            .map(|s| {
                roughness_from_slope(
                    s,
                    self.profile.terrain_derived_roughness_base,
                    self.profile.terrain_derived_roughness_saturation_radians,
                )
            })
            .collect();
        let ao_bounded = BoundedHeightSource::new(&self.ao_source, half);
        let ao = self.rays.map_rows(
            &ao_bounded,
            &self.region,
            reduced,
            half,
            reduced_rows.clone(),
            &cancelled,
        )?;
        let tone_bounded = BoundedHeightSource::new(&self.tone_source, half);
        let tone = albedo_map_rows(
            &tone_bounded,
            &self.region,
            reduced,
            TONE_STENCIL_TEXELS * 0.5,
            half,
            reduced_rows,
            &cancelled,
        )?;
        Some(DerivedRasterRows {
            normals,
            roughness,
            ao,
            tone,
        })
    }
}

fn reduced_raster_res(res: usize) -> usize {
    (res / 2).max(1)
}

/// Concatenate complete ranges in ascending row order before applying the
/// full-raster transfer and mip contract. This runs on a worker, never Update.
fn assemble_derived_rows(
    res: usize,
    rows: Vec<DerivedRasterRows>,
    cancelled: impl Fn() -> bool,
) -> Option<DerivedMaps> {
    if cancelled() {
        return None;
    }
    let reduced = reduced_raster_res(res);
    let mut rows = rows.into_iter();
    let DerivedRasterRows {
        mut normals,
        mut roughness,
        ao: mut ao_small,
        tone: mut tone_small,
    } = rows.next()?;
    // The serial web path already owns one complete range and requires no
    // second full-size allocation merely to concatenate it.
    normals.reserve_exact(res * res - normals.len());
    roughness.reserve_exact(res * res - roughness.len());
    ao_small.reserve_exact(reduced * reduced - ao_small.len());
    tone_small.reserve_exact(reduced * reduced - tone_small.len());
    for mut row in rows {
        if cancelled() {
            return None;
        }
        normals.append(&mut row.normals);
        roughness.append(&mut row.roughness);
        ao_small.append(&mut row.ao);
        tone_small.append(&mut row.tone);
    }
    assert_eq!(
        normals.len(),
        res * res,
        "normal row coverage must be complete"
    );
    assert_eq!(
        roughness.len(),
        res * res,
        "roughness row coverage must be complete"
    );
    if cancelled() {
        return None;
    }
    let ao = lunco_terrain_core::upsample_bilinear(&ao_small, reduced, res);
    drop(ao_small);
    if cancelled() {
        return None;
    }
    let albedo = lunco_terrain_core::upsample_bilinear(&tone_small, reduced, res);
    drop(tone_small);
    if cancelled() {
        return None;
    }
    let surface_rgba = pack_surface_rgba8(&roughness, &ao, &[]);
    if cancelled() {
        return None;
    }
    let normal_rgba = pack_normal_rgba8(&normals, &albedo);
    (!cancelled()).then_some(DerivedMaps {
        res,
        surface_rgba,
        normal_rgba,
    })
}

/// The serial cache consumer (web) uses the same row kernels and assembly.
fn bake_derived(
    oracle: &SurfaceOracle,
    profile: lunco_render::RenderQualityProfile,
) -> DerivedMaps {
    let plan = DerivedRasterPlan::new(oracle, profile);
    let res = profile.terrain_derived_map_resolution;
    let rows = plan
        .sample_rows(0..reduced_raster_res(res), || false)
        .expect("non-cancellable derived raster cannot be cancelled");
    assemble_derived_rows(res, vec![rows], || false).expect("complete serial raster must assemble")
}

/// Upload finished bakes as linear RGBA8 textures and publish the handles as
/// [`TerrainDerivedMaps`]. Needs `Assets<Image>` (present whenever a bake was
/// started). A result baked against a superseded oracle is discarded — the
/// stale marker chain re-kicks a fresh bake against the current surface.
fn finish_derived_bakes(
    mut commands: Commands,
    quality: Res<lunco_render::RenderingQualitySettings>,
    #[cfg(not(target_arch = "wasm32"))] admission: Res<AsyncWorkAdmission>,
    #[cfg(not(target_arch = "wasm32"))] budget: Res<DerivedRasterAssemblyBudget>,
    mut tasks: Query<(
        Entity,
        &mut DerivedBakeTask,
        &DemHeightField,
        Option<&DemVisualTargetRes>,
        Has<TerrainLodViz>,
    )>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let Some(mut images) = images else { return };
    let Ok(profile) = quality.validated_profile() else {
        // Invalid requested quality cannot admit or retain a previous worker
        // lease. Cancel now rather than stranding its completed result permit.
        for (entity, _, _, _, _) in &mut tasks {
            commands.entity(entity).try_remove::<DerivedBakeTask>();
        }
        return;
    };
    for (entity, mut task, hf, target, streamed) in &mut tasks {
        let requested_profile =
            profile_for_terrain(profile, hf.0.grid().res, target.map(|t| t.0), streamed);
        if !Arc::ptr_eq(&task.source, &hf.0)
            || task.requested_key != derived_key(&hf.0, requested_profile)
        {
            // Retire queued work and fence running results when either immutable input changes.
            commands
                .entity(entity)
                .try_remove::<DerivedBakeTask>()
                .try_insert(DerivedMapsStale {
                    since: Instant::now(),
                });
            continue;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let maps = {
            let source = task.source.clone();
            let requested_key = task.requested_key;
            match task.native.advance(
                &admission,
                &budget,
                entity,
                requested_key,
                &source,
                profile.terrain_derived_map_resolution,
            ) {
                Ok(Some(maps)) => maps,
                Ok(None) => continue,
                Err(()) => {
                    error!(?entity, "terrain derived-map worker did not complete");
                    commands
                        .entity(entity)
                        .try_remove::<DerivedBakeTask>()
                        .try_insert(DerivedMapsFailed {
                            source: task.source.clone(),
                            requested_key: task.requested_key,
                        });
                    continue;
                }
            }
        };
        #[cfg(target_arch = "wasm32")]
        let Some((maps, _permit)) = future::block_on(future::poll_once(&mut task.task)) else {
            continue;
        };
        let mut actual_profile = requested_profile;
        actual_profile.terrain_derived_map_resolution = maps.res;
        let bake_key = derived_key(&hf.0, actual_profile);
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
                surface_key: hf.0.surface_key(),
                bake_key,
                res,
                texel_size_m,
            });
    }
}

/// A surviving entity can lose its DEM during authored reconciliation. Its
/// queued request, unconsumed result and published rasters are still owned by
/// the removed source, not by the entity's remaining authored material.
fn retire_derived_without_source(
    mut commands: Commands,
    entities: Query<
        Entity,
        (
            Without<DemHeightField>,
            Or<(
                With<DerivedBakeTask>,
                With<DerivedMapsFailed>,
                With<DerivedMapsStale>,
                With<TerrainDerivedMaps>,
            )>,
        ),
    >,
) {
    for entity in &entities {
        commands.entity(entity).try_remove::<(
            DerivedBakeTask,
            DerivedMapsFailed,
            DerivedMapsStale,
            TerrainDerivedMaps,
        )>();
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
            With<DerivedMapsFailed>,
            With<DerivedMapsStale>,
            With<TerrainDerivedMaps>,
            With<TerrainAuthoredMaps>,
        )>,
    >,
    mut status: ResMut<TerrainDerivedStatus>,
) {
    for entity in &entities {
        commands.entity(entity).try_remove::<(
            DerivedBakeTask,
            DerivedMapsFailed,
            DerivedMapsStale,
            TerrainDerivedMaps,
            TerrainAuthoredMaps,
        )>();
    }
    *status = TerrainDerivedStatus::default();
}

/// Publish the optional visual lifecycle. This is the terrain crate's
/// authoritative state; the application only mirrors it onto the existing
/// status bus.
fn update_derived_status(
    images: Option<Res<Assets<Image>>>,
    relief: Option<Res<crate::relief_pages::TerrainReliefPages>>,
    horizon_config: Option<Res<lunco_environment::HorizonShadowCacheConfig>>,
    render_sun: Option<Res<lunco_environment::SunRenderState>>,
    sun: lunco_environment::SunQuery,
    quality: Res<lunco_render::RenderingQualitySettings>,
    terrains: Query<
        (
            Entity,
            &DemHeightField,
            Option<&TerrainDerivedMaps>,
            Option<&DerivedBakeTask>,
            Option<&DerivedMapsFailed>,
            Has<DerivedMapsStale>,
            Option<&DemVisualTargetRes>,
            Has<TerrainLodViz>,
            Option<&lunco_core::HorizonShadowTerrain>,
            Has<bevy::camera::visibility::RenderLayers>,
            Option<&lunco_environment::HorizonMap>,
            Option<&GlobalTransform>,
            Option<&lunco_environment::HorizonShadowCache>,
            Option<&TileShadowCache>,
        ),
        With<DemHeightField>,
    >,
    mut status: ResMut<TerrainDerivedStatus>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if images.is_none() {
        status.set_if_neq(TerrainDerivedStatus::default());
        return;
    }

    let mut ready = 0;
    let mut total = 0;
    let mut pending = 0;
    let mut failed = 0;
    let mut findings = Vec::new();
    let Ok(profile) = quality.validated_profile() else {
        return;
    };
    let sun_sample = lunco_environment::pick_sun(&sun).and_then(|(_, radius, _)| {
        render_sun
            .as_ref()?
            .direction_to_sun_world
            .map(|direction| (direction, radius))
    });
    for (
        entity,
        field,
        maps,
        task,
        failure,
        stale,
        target,
        streamed,
        horizon,
        render_layers,
        horizon_ready,
        transform,
        shadow,
        wired_shadow,
    ) in &terrains
    {
        let horizon_enabled = horizon.is_some_and(|cfg| cfg.resolution >= 2)
            && !render_layers
            && !(streamed && cfg!(target_arch = "wasm32"));
        let local_sun = sun_sample
            .zip(transform)
            .map(|((direction, radius), transform)| {
                (
                    transform
                        .affine()
                        .inverse()
                        .transform_vector3(direction)
                        .normalize_or_zero(),
                    radius,
                )
            });
        let cache_required = horizon_enabled
            && horizon_config.as_ref().zip(local_sun).is_some_and(
                |(config, (direction, radius))| config.requires_sample(direction, radius),
            );
        let shadow_ready = !cache_required
            || shadow.is_some_and(|cache| {
                horizon_config.as_ref().zip(local_sun).is_some_and(
                    |(config, (direction, radius))| {
                        cache.matches_quality(config, radius)
                            && cache.is_valid_for_sun(direction, config.sun_threshold_deg)
                    },
                ) && (!streamed
                    || wired_shadow
                        .is_some_and(|wired| wired.on > 0.0 && wired.image == cache.image))
            });
        let height_ready = !horizon_enabled
            || (streamed && !cache_required)
            || horizon
                .zip(horizon_ready)
                .is_some_and(|(cfg, map)| map.field.resolution() == cfg.resolution);
        let has_task = task.is_some();
        total += 1;
        let desired =
            profile_for_terrain(profile, field.0.grid().res, target.map(|t| t.0), streamed);
        let requested_key = derived_key(&field.0, desired);
        if failure.is_some_and(|failure| {
            Arc::ptr_eq(&failure.source, &field.0) && failure.requested_key == requested_key
        }) {
            failed += 1;
            findings.push(lunco_core::RuntimeDiagnostic {
                producer: "terrain-visual-preparation".into(),
                code: "derived-bake-failed".into(),
                severity: lunco_core::DiagnosticSeverity::Error,
                subject: format!("{entity:?}"),
                message:
                    "terrain derived raster preparation failed for the current source and quality"
                        .into(),
            });
            continue;
        }
        let complete = maps.is_some_and(|maps| maps.bake_key == requested_key)
            && !has_task
            && !stale
            && height_ready
            && shadow_ready;
        ready += usize::from(complete);
        pending += usize::from(!complete);
    }

    if let Some(relief) = relief {
        let (requested, complete) = relief.counts();
        total += requested;
        ready += complete;
        pending += requested - complete;
    }
    status.set_if_neq(TerrainDerivedStatus {
        active: pending > 0,
        ready,
        total,
        pending,
        failed,
    });
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        if diagnostics
            .findings
            .iter()
            .filter(|finding| finding.producer == "terrain-visual-preparation")
            .ne(findings.iter())
        {
            diagnostics.replace_producer("terrain-visual-preparation", findings);
        }
    }
}

/// A linear (non-sRGB) RGBA8 data texture with a full mip chain and
/// trilinear/anisotropic filtering — these carry the roughness/AO scalars,
/// an encoded normal, and the albedo scalar, and are sampled out to the horizon.
/// Takes the PREBUILT chain (`(data, level_count)` from
/// [`lunco_materials::rgba8_mip_chain`],
/// run in the bake task) — this only wraps it in an `Image`.
pub(crate) fn data_texture(
    res: usize,
    (data, mip_levels): (Vec<u8>, u32),
    anisotropy_clamp: u16,
) -> Image {
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
/// [`crate::plugin::TerrainSurfaceVisualizationPlugin`].
pub(crate) fn register(app: &mut App) {
    #[cfg(not(target_arch = "wasm32"))]
    lunco_core::AppSceneExt::init_scene_resource::<DerivedRasterAssemblyBudget>(app);
    app.init_resource::<DerivedQualitySignature>()
        .init_resource::<TerrainDerivedStatus>()
        .add_systems(
            lunco_core::SceneTeardown,
            cancel_derived_bakes_on_scene_teardown,
        );
    app.add_systems(
        Update,
        (
            retire_derived_without_source,
            mark_derived_stale_on_quality_change
                .run_if(resource_changed::<lunco_render::RenderingQualitySettings>),
            mark_derived_stale,
            start_derived_bakes,
            finish_derived_bakes,
            update_derived_status,
        )
            .chain()
            // The `.after` inserts the sync point that makes `finish_dem_restamp`'s
            // deferred `TerrainSurfaceChange` visible in the same frame as its
            // (immediate) `DemHeightField` swap — unordered, `mark_derived_stale`
            // could otherwise treat a bounded edit as a full-surface change.
            .after(crate::terrain::finish_dem_restamp),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_precompute::Bake;

    fn terrain_profile() -> lunco_render::RenderQualityProfile {
        lunco_render::RenderQualityProfile {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            directional_cascades: 2,
            shadow_budget_bytes: 1024 * 1024 * 1024,
            horizon_shadow_cache_sun_threshold_deg: 0.2,
            horizon_march_steps: 24,
            horizon_cache_samples_per_axis: 1,
            shadow_first_cascade_far_bound: 20.0,
            shadow_maximum_distance: 600.0,
            render_failure_quiet_period_secs: 0.5,
            render_failure_give_up_after_secs: 5.0,
            editor_preview_illuminance: 128_000.0,
            local_light_default_range: 20.0,
            dome_cubemap_face_size: 512,
            primitive_sphere_longitudes: 24,
            primitive_sphere_latitudes: 16,
            primitive_radial_segments: 32,
            primitive_capsule_longitudes: 16,
            primitive_capsule_latitudes: 8,
            terrain_mesh_cache_bytes: 256 * 1024 * 1024,
            terrain_derived_map_resolution: 1024,
            terrain_derived_ao_directions: 4,
            terrain_derived_ao_steps: 4,
            terrain_derived_ao_radius_fraction: 0.1,
            terrain_derived_roughness_base: 0.6,
            terrain_derived_roughness_saturation_radians: 0.6,
            terrain_derived_texture_anisotropy: 1,
            terrain_rock_max_instances: 2_000,
            terrain_rock_mesh_buckets: 3,
            terrain_rock_mesh_cube_count: 2,
            terrain_rock_lod_fade_distance: 300.0,
            terrain_lod_tile_resolution: 33,
            terrain_lod_cinematic_resolution: 1025,
            terrain_lod_pixel_error: 4.0,
            terrain_lod_max_depth: 6,
            terrain_lod_probe_resolution: 5,
            terrain_lod_bakes_per_frame: 8,
            terrain_lod_max_inflight_bakes: 16,
            terrain_lod_tile_budget: 256,
            terrain_lod_cover_edits_per_frame: 16,
            terrain_lod_hysteresis_ratio: 1.2,
            nurbs_surface_samples_per_control_span: 3,
            nurbs_surface_minimum_subdivisions: 6,
            nurbs_surface_maximum_subdivisions: 64,
            nurbs_trim_curve_samples: 12,
            nurbs_trim_minimum_subdivisions: 8,
            nurbs_trim_maximum_subdivisions: 48,
            curve_samples_per_segment: 4,
            curve_radial_segments: 6,
            ..Default::default()
        }
    }

    #[test]
    fn native_cache_rejects_wrong_requested_resolution_and_empty_maps() {
        assert!(derived_maps_from_buffers(0, vec![], vec![]).is_none());
        assert!(derived_maps_from_buffers(4, vec![0; 16], vec![0; 16]).is_none());
        assert!(derived_maps_from_buffers(2, vec![0; 16], vec![0; 15]).is_none());
        assert_eq!(
            derived_maps_from_buffers(2, vec![0; 16], vec![0; 16])
                .unwrap()
                .res,
            2
        );
    }

    #[test]
    fn desired_bake_identity_includes_quality_at_the_same_resolution() {
        let oracle = SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(2, 1.0),
        ));
        let profile = terrain_profile();
        let changed = lunco_render::RenderQualityProfile {
            terrain_derived_ao_steps: profile.terrain_derived_ao_steps + 1,
            ..profile
        };
        assert_ne!(derived_key(&oracle, profile), derived_key(&oracle, changed));
    }

    #[test]
    fn baker_and_renderer_share_exact_texel_spacing() {
        assert_eq!(raster_texel_size_m(4_096.0, 1_024), 8.0);
    }

    #[test]
    fn static_derived_resolution_is_bounded_by_its_visual_product() {
        let profile = terrain_profile();
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
        let profile = terrain_profile();

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
    fn local_derived_maps_reuse_cache_across_regional_changes() {
        use lunco_geotiff::{GeoTransform, LunarFrame};
        use lunco_obstacle_field::field::HeightGrid;
        use lunco_terrain_bake::regional::RegisteredRegionalSurface;

        let local = Arc::new(HeightGrid::new_flat(3, 10.0));
        let geo = |half, res| {
            GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, 0.0, 0.0)
                .with_frame(LunarFrame::MoonMe)
        };
        let make = |height| {
            let mut outer = HeightGrid::new_flat(5, 40.0);
            outer.heights.fill(height);
            SurfaceOracle::registered(
                Arc::new(
                    RegisteredRegionalSurface::new(
                        local.clone(),
                        geo(10.0, 3),
                        Arc::new(outer),
                        geo(40.0, 5),
                        5.0,
                        0.0,
                    )
                    .unwrap(),
                ),
                Vec::new(),
            )
        };
        let first = make(0.0);
        let second = make(1.0);
        assert_ne!(first.surface_key(), second.surface_key());
        assert_eq!(first.local_surface_key(), second.local_surface_key());
        let profile = terrain_profile();
        assert_eq!(
            DerivedBake {
                oracle: &first,
                profile
            }
            .key(),
            DerivedBake {
                oracle: &second,
                profile
            }
            .key()
        );
        let mut changed_local = (*local).clone();
        changed_local.heights[4] = 1.0;
        let edited = SurfaceOracle::registered(
            Arc::new(
                RegisteredRegionalSurface::new(
                    Arc::new(changed_local),
                    geo(10.0, 3),
                    Arc::new(HeightGrid::new_flat(5, 40.0)),
                    geo(40.0, 5),
                    5.0,
                    0.0,
                )
                .unwrap(),
            ),
            Vec::new(),
        );
        assert_ne!(
            DerivedBake {
                oracle: &first,
                profile
            }
            .key(),
            DerivedBake {
                oracle: &edited,
                profile
            }
            .key()
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
    fn terminal_current_raster_failure_is_not_indefinite_pending() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<lunco_render::RenderingQualitySettings>()
            .init_resource::<TerrainDerivedStatus>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_systems(Update, update_derived_status);
        app.world_mut()
            .resource_mut::<lunco_render::RenderingQualitySettings>()
            .apply_profile(terrain_profile());
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(crate::HeightGrid::new_flat(
            2, 1.0,
        ))));
        let profile = app
            .world()
            .resource::<lunco_render::RenderingQualitySettings>()
            .validated_profile()
            .unwrap();
        let desired = profile_for_terrain(profile, 2, None, false);
        app.world_mut().spawn((
            DemHeightField(oracle.clone()),
            DerivedMapsFailed {
                requested_key: derived_key(&oracle, desired),
                source: oracle,
            },
        ));
        app.update();
        let status = app.world().resource::<TerrainDerivedStatus>();
        assert_eq!(status.failed, 1);
        assert_eq!(status.pending, 0);
        assert!(!status.active);
        assert_eq!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .len(),
            1
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn source_removal_releases_queued_work_and_unconsumed_raster_permit() {
        let mut app = App::new();
        app.init_resource::<AsyncWorkAdmission>()
            .init_resource::<DerivedRasterAssemblyBudget>()
            .add_systems(Update, retire_derived_without_source);
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(crate::HeightGrid::new_flat(
            2, 1.0,
        ))));
        let (request, permit) = {
            let admission = app.world().resource::<AsyncWorkAdmission>();
            let key = |operation| {
                AsyncWorkKey::new(AsyncWorkKind::VisualizationPreparation, 0, 1, 1, operation)
            };
            (
                admission
                    .submit_transport(AsyncWorkPriority::Background, key(1), |_| {})
                    .unwrap(),
                admission
                    .admit_external(AsyncWorkPriority::Interactive, key(2))
                    .unwrap(),
            )
        };
        let reservation = app
            .world()
            .resource::<DerivedRasterAssemblyBudget>()
            .reserve(1, 1)
            .unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        assert!(
            sender
                .send((
                    Some(DerivedMipped {
                        res: 1,
                        surface: (vec![0; 4], 1),
                        normal: (vec![0; 4], 1)
                    }),
                    permit,
                    Some(reservation.clone()),
                ))
                .is_ok()
        );
        let entity = app
            .world_mut()
            .spawn((
                DemHeightField(oracle.clone()),
                DerivedBakeTask {
                    source: oracle.clone(),
                    requested_key: 1,
                    native: NativeDerivedTask {
                        priority: AsyncWorkPriority::Background,
                        cancelled: cancellation.clone(),
                        stage: NativeDerivedStage::Finish(DerivedTransport {
                            _request: request,
                            result: Mutex::new(receiver),
                        }),
                        assembly: Some(reservation),
                    },
                },
                DerivedMapsFailed {
                    source: oracle,
                    requested_key: 1,
                },
                DerivedMapsStale {
                    since: Instant::now(),
                },
                TerrainDerivedMaps {
                    surface: Handle::default(),
                    normal: Handle::default(),
                    surface_key: 1,
                    bake_key: 1,
                    res: 1,
                    texel_size_m: 1.0,
                },
                TerrainAuthoredMaps::default(),
            ))
            .id();
        let snapshot = app.world().resource::<AsyncWorkAdmission>().snapshot();
        assert_eq!(snapshot.queued.iter().sum::<usize>(), 1);
        assert_eq!(snapshot.in_flight.iter().sum::<usize>(), 1);
        app.world_mut()
            .entity_mut(entity)
            .remove::<DemHeightField>();
        app.update();
        let snapshot = app.world().resource::<AsyncWorkAdmission>().snapshot();
        assert_eq!(snapshot.queued.iter().sum::<usize>(), 0);
        assert_eq!(snapshot.in_flight.iter().sum::<usize>(), 0);
        assert!(cancellation.load(Ordering::Acquire));
        assert_eq!(
            app.world()
                .resource::<DerivedRasterAssemblyBudget>()
                .0
                .load(Ordering::Acquire),
            0
        );
        assert!(app.world().get::<DerivedBakeTask>(entity).is_none());
        assert!(app.world().get::<DerivedMapsFailed>(entity).is_none());
        assert!(app.world().get::<DerivedMapsStale>(entity).is_none());
        assert!(app.world().get::<TerrainDerivedMaps>(entity).is_none());
        assert!(app.world().get::<TerrainAuthoredMaps>(entity).is_some());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn retired_raster_keeps_staging_reserved_until_worker_releases_it() {
        use lunco_core::SceneResource;
        let mut budget = DerivedRasterAssemblyBudget::default();
        let owner = budget.reserve(64, 64).unwrap();
        let worker = owner.clone();
        assert!(budget.reserve(1, 64).is_none());
        drop(owner);
        budget.clear_scene();
        assert!(budget.reserve(64, 64).is_none());
        drop(worker);
        assert!(budget.reserve(64, 64).is_some());
    }

    #[test]
    fn partitioned_derived_raster_matches_serial_packed_pixels() {
        let mut grid = crate::HeightGrid::new_flat(9, 20.0);
        for (index, height) in grid.heights.iter_mut().enumerate() {
            *height = ((index as f64 * 0.37).sin() * 3.0) + 0.1 * index as f64;
        }
        let oracle = SurfaceOracle::bare(Arc::new(grid));
        for res in [1, 7, 16, 31] {
            let mut profile = terrain_profile();
            profile.terrain_derived_map_resolution = res;
            let serial = bake_derived(&oracle, profile);
            let plan = DerivedRasterPlan::new(&oracle, profile);
            let reduced = reduced_raster_res(res);
            let rows = (0..reduced)
                .map(|row| plan.sample_rows(row..row + 1, || false).unwrap())
                .collect();
            let partitioned = assemble_derived_rows(res, rows, || false).unwrap();
            assert_eq!(partitioned.res, serial.res);
            assert_eq!(partitioned.surface_rgba, serial.surface_rgba);
            assert_eq!(partitioned.normal_rgba, serial.normal_rgba);
        }
    }

    #[test]
    fn scene_teardown_removes_derived_state_and_resets_status() {
        let mut app = App::new();
        app.init_resource::<TerrainDerivedStatus>().add_systems(
            lunco_core::SceneTeardown,
            cancel_derived_bakes_on_scene_teardown,
        );
        let entity = app.world_mut().spawn_empty().id();
        *app.world_mut().resource_mut::<TerrainDerivedStatus>() = TerrainDerivedStatus {
            active: true,
            ready: 0,
            total: 1,
            pending: 1,
            failed: 0,
        };

        lunco_core::run_scene_teardown(app.world_mut());

        assert!(app.world().get_entity(entity).is_ok());
        assert_eq!(
            *app.world().resource::<TerrainDerivedStatus>(),
            TerrainDerivedStatus::default()
        );
    }
}
