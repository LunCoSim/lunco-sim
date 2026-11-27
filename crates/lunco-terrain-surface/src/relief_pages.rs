//! World-aligned normal clipmap. Each view has a toroidal GPU atlas of
//! independently cached, mipmapped chunks. Movement changes only the slots
//! that enter the view window; zoom selects a different sampling level.

use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_core::{AppSceneExt, SceneResource};
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority,
    async_work::ExternalWorkPermit,
};
use lunco_materials::{ParamValue, Rgba8MipMode, ShaderLook, TextureLayer, rgba8_mip_chain};
use lunco_render::data_texture::{
    DataTexturePatch, DataTextureTexels, DataTextureUpdates, Rgba8MipRegion,
};
use lunco_terrain_core::{
    HeightSource, Square, derive::bounded_normal_page_rows, pack_normal_rgba8,
};

use crate::{
    band::SurfaceBand,
    derived_layers::data_texture,
    oracle::{DemHeightField, SurfaceOracle},
    stream_viz::{LodTileOf, LodTiles, TerrainDetailDemands, TerrainDiagnosticTile, set_param},
};

// Eight chunks across permits a one-cell shift to reuse seven eighths of
// the texture and gives each High-quality chunk 256² normal postings.
const CHUNKS: i64 = 8;
const MAX_ATLAS_SIDE: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSpec {
    start: [i64; 2],
    chunk_span: f64,
    resolution: usize,
    source_key: u64,
}

impl PageSpec {
    fn region(self) -> Square {
        Square {
            center: self
                .start
                .map(|v| (v + CHUNKS / 2) as f64 * self.chunk_span),
            half: self.chunk_span * CHUNKS as f64 * 0.5,
        }
    }
    fn texel(self) -> f64 {
        self.chunk_span / self.chunk_resolution() as f64
    }
    fn chunk_resolution(self) -> usize {
        self.resolution / CHUNKS as usize
    }
    fn mip_count(self) -> u32 {
        self.chunk_resolution().ilog2() + 1
    }
    fn same_lattice(self, other: Self) -> bool {
        self.source_key == other.source_key
            && self.resolution == other.resolution
            && self.chunk_span == other.chunk_span
    }
    fn retains_eye(self, eye: DVec3) -> bool {
        let center = self.region().center;
        (center[0] - eye.x).abs().max((center[1] - eye.z).abs()) <= self.chunk_span
    }
    fn contains(self, coord: [i64; 2]) -> bool {
        (0..2)
            .all(|axis| coord[axis] >= self.start[axis] && coord[axis] < self.start[axis] + CHUNKS)
    }
    fn coords(self) -> impl Iterator<Item = [i64; 2]> {
        (self.start[1]..self.start[1] + CHUNKS)
            .flat_map(move |z| (self.start[0]..self.start[0] + CHUNKS).map(move |x| [x, z]))
    }
    fn key(self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(self.source_key);
        h.write_u64(self.chunk_span.to_bits());
        h.write_u64(self.resolution as u64);
        for v in self.start {
            h.write_u64(v as u64);
        }
        h.finish()
    }
    fn chunk(self, coord: [i64; 2]) -> ChunkSpec {
        ChunkSpec {
            region: Square {
                center: coord.map(|c| (c as f64 + 0.5) * self.chunk_span),
                half: self.chunk_span * 0.5,
            },
            resolution: self.chunk_resolution(),
            source_key: self.source_key,
        }
    }
}

#[derive(Clone, Copy)]
struct ChunkSpec {
    region: Square,
    resolution: usize,
    source_key: u64,
}
impl ChunkSpec {
    fn texel(self) -> f64 {
        self.region.side() / self.resolution as f64
    }
    fn key(self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(1); // normal sampling and packed chunk format
        h.write_u64(self.source_key);
        for v in self.region.center {
            h.write_u64(v.to_bits());
        }
        h.write_u64(self.region.half.to_bits());
        h.write_u64(self.resolution as u64);
        h.finish()
    }
}

/// Choose a texel level from the projection footprint. Near ground the window
/// is independent of heading; a distant view only requests a finer normal field
/// if its estimated ground intersection resolves more than the site raster.
fn page_spec(
    source: &SurfaceOracle,
    eye: DVec3,
    forward: DVec3,
    radius: f64,
    focal_px: f64,
    res: usize,
) -> Option<(PageSpec, DVec3)> {
    let extent = f64::from(source.half_extent());
    if !radius.is_finite() || radius <= 0.0 || !focal_px.is_finite() || focal_px <= 0.0 {
        return None;
    }
    let chunk_span = 2.0_f64.powf((radius * 0.5).log2().round());
    let side = chunk_span * CHUNKS as f64;
    if !side.is_finite() || side >= 2.0 * extent {
        return None;
    }
    let clearance = (eye.y
        - source.height_at(eye.x.clamp(-extent, extent), eye.z.clamp(-extent, extent)))
    .abs();
    let (focus, camera_distance) =
        if clearance <= radius && eye.x.abs() <= extent && eye.z.abs() <= extent {
            (eye, 0.0)
        } else {
            if forward.y >= -1e-6 {
                return None;
            }
            let distance = clearance / -forward.y;
            let focus = eye + forward * distance;
            if !focus.is_finite() || focus.x.abs() > extent || focus.z.abs() > extent {
                return None;
            }
            (focus, distance)
        };
    let capped = res.min(MAX_ATLAS_SIDE).max(1);
    let pixel_res = (2.0 * focal_px).ceil().clamp(1.0, MAX_ATLAS_SIDE as f64) as usize;
    let mut resolution = pixel_res.next_power_of_two().min(1usize << capped.ilog2());
    // Distant sampling is limited by actual pixel footprint, not a hard
    // altitude threshold. Telephoto views can still request a finer level.
    while resolution > CHUNKS as usize
        && side / resolution as f64 * 2.0 <= camera_distance / focal_px
    {
        resolution /= 2;
    }
    if resolution < CHUNKS as usize || side / resolution as f64 >= 2.0 * extent / res as f64 {
        return None;
    }
    let center = [focus.x / chunk_span, focus.z / chunk_span].map(f64::round);
    if center
        .iter()
        .any(|v| !v.is_finite() || v.abs() >= (i64::MAX / 2) as f64)
    {
        return None;
    }
    let spec = PageSpec {
        start: center.map(|v| v as i64 - CHUNKS / 2),
        chunk_span,
        resolution,
        source_key: source.local_surface_key(),
    };
    let region = spec.region();
    if region.center.iter().any(|v| v.abs() > f32::MAX as f64)
        || region.half > f32::MAX as f64
        || !(spec.texel() as f32 > 0.0)
    {
        return None;
    }
    Some((spec, focus))
}

struct PageBake<'a> {
    source: &'a SurfaceOracle,
    spec: ChunkSpec,
    cancelled: &'a AtomicBool,
}

impl PageBake<'_> {
    fn rows(&self, rows: std::ops::Range<usize>, source: &SurfaceOracle) -> Option<Vec<u8>> {
        let normals = bounded_normal_page_rows(
            source,
            &self.spec.region,
            self.spec.resolution,
            rows,
            f64::from(self.source.half_extent()),
            || self.cancelled.load(Ordering::Relaxed),
        )?;
        Some(pack_normal_rgba8(&normals, &[]))
    }
    fn filtered(&self) -> SurfaceOracle {
        let extent = f64::from(self.source.half_extent());
        // Boundary samples clamp to the measured crop. The scoped oracle
        // must include those clamped coordinates even for entirely exterior chunks.
        let mut region = self.spec.region;
        region.center = region.center.map(|v| v.clamp(-extent, extent));
        SurfaceBand::visual(self.spec.texel()).limited_region(
            self.source,
            region,
            2.0 * self.spec.texel(),
        )
    }
}

impl lunco_precompute::Bake for PageBake<'_> {
    type Output = Option<Vec<u8>>;
    const NAMESPACE: &'static str = "terrain/relief-chunks";
    fn key(&self) -> u64 {
        self.spec.key()
    }
    fn bake(&self) -> Self::Output {
        let filtered = self.filtered();
        let mut bytes = Vec::with_capacity(self.spec.resolution * self.spec.resolution * 4);
        for row in (0..self.spec.resolution).step_by(32) {
            bytes.extend(self.rows(row..(row + 32).min(self.spec.resolution), &filtered)?);
        }
        Some(bytes)
    }
    fn store(dir: &std::path::Path, out: &Self::Output) -> lunco_precompute::StorageResult<()> {
        if let Some(bytes) = out {
            lunco_precompute::store_blob(dir, "normal.bin", bytes)?;
        }
        Ok(())
    }
    fn load(&self, dir: &std::path::Path) -> Option<Self::Output> {
        let bytes = lunco_precompute::load_blob(dir, "normal.bin")?;
        (bytes.len() == self.spec.resolution * self.spec.resolution * 4).then_some(Some(bytes))
    }
}

async fn bake_chunk(
    source: &SurfaceOracle,
    spec: ChunkSpec,
    cancelled: &AtomicBool,
) -> Option<(Vec<u8>, u32)> {
    let bake = PageBake {
        source,
        spec,
        cancelled,
    };
    #[cfg(not(target_arch = "wasm32"))]
    let bytes = lunco_precompute::bake_or_load(&bake, &lunco_assets_core::cache_dir())?;
    #[cfg(target_arch = "wasm32")]
    let bytes = {
        use lunco_precompute::Bake;
        let key = lunco_precompute::key_hex(bake.key());
        if let Some(bytes) = lunco_storage::opfs_blob::read(PageBake::NAMESPACE, &key)
            .await
            .filter(|bytes| bytes.len() == spec.resolution * spec.resolution * 4)
        {
            bytes
        } else {
            let filtered = bake.filtered();
            let mut bytes = Vec::with_capacity(spec.resolution * spec.resolution * 4);
            for row in (0..spec.resolution).step_by(4) {
                bytes.extend(bake.rows(row..(row + 4).min(spec.resolution), &filtered)?);
                future::yield_now().await;
            }
            lunco_storage::opfs_blob::write(PageBake::NAMESPACE, &key, &bytes).await;
            bytes
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    rgba8_mip_chain(
        bytes,
        spec.resolution,
        spec.resolution,
        Rgba8MipMode::Normal,
    )
}

enum AtlasUpload {
    Initial((Vec<u8>, u32)),
    Incremental(Vec<Rgba8MipRegion>),
}
struct AtlasResult {
    upload: AtlasUpload,
    reused: usize,
    updated: usize,
    bytes: usize,
}

fn mip_offset(side: usize, level: u32) -> usize {
    (0..level).map(|i| (side >> i).pow(2) * 4).sum()
}

fn write_region(data: &mut [u8], side: usize, region: &Rgba8MipRegion) -> bool {
    let mip_side = side >> region.level;
    let offset = mip_offset(side, region.level);
    if !region.valid(UVec2::splat(side as u32), side.ilog2() + 1)
        || offset + mip_side * mip_side * 4 > data.len()
    {
        return false;
    }
    let stride = region.size.x as usize * 4;
    for row in 0..region.size.y as usize {
        let dest =
            offset + ((region.origin.y as usize + row) * mip_side + region.origin.x as usize) * 4;
        data[dest..dest + stride].copy_from_slice(&region.bytes[row * stride..(row + 1) * stride]);
    }
    true
}

async fn build_atlas(
    source: Arc<SurfaceOracle>,
    spec: PageSpec,
    previous: Option<PageSpec>,
    cancelled: Arc<AtomicBool>,
) -> Option<AtlasResult> {
    let previous = previous.filter(|old| old.same_lattice(spec));
    let mut initial = previous
        .is_none()
        .then(|| vec![0; mip_offset(spec.resolution, spec.mip_count())]);
    let mut patches = Vec::new();
    let mut reused = 0;
    let mut bytes = 0;
    for coord in spec.coords() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        if previous.is_some_and(|old| old.contains(coord)) {
            reused += 1;
            continue;
        }
        let chunk = spec.chunk(coord);
        let (data, levels) = bake_chunk(&source, chunk, &cancelled).await?;
        if levels != spec.mip_count() {
            return None;
        }
        for level in 0..levels {
            let side = chunk.resolution >> level;
            let offset = mip_offset(chunk.resolution, level);
            let slot = coord.map(|v| v.rem_euclid(CHUNKS) as u32);
            let region = Rgba8MipRegion {
                level,
                origin: UVec2::new(slot[0], slot[1]) * side as u32,
                size: UVec2::splat(side as u32),
                bytes: data[offset..offset + side * side * 4].to_vec(),
            };
            bytes += region.bytes.len();
            if let Some(atlas) = &mut initial {
                if !write_region(atlas, spec.resolution, &region) {
                    return None;
                }
            } else {
                patches.push(region);
            }
        }
    }
    let upload = if let Some(data) = initial {
        AtlasUpload::Initial((data, spec.mip_count()))
    } else {
        AtlasUpload::Incremental(patches)
    };
    Some(AtlasResult {
        upload,
        reused,
        updated: (CHUNKS * CHUNKS) as usize - reused,
        bytes,
    })
}

struct PendingPage {
    spec: PageSpec,
    source: Arc<SurfaceOracle>,
    cancelled: Arc<AtomicBool>,
    task: Task<(Option<AtlasResult>, ExternalWorkPermit)>,
}
impl Drop for PendingPage {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

struct PublishedPage {
    spec: PageSpec,
    image: Handle<Image>,
    reused: usize,
    updated: usize,
    bytes: usize,
    upload_sequence: Option<u64>,
}
#[derive(Default)]
struct ViewPage {
    desired: Option<PageSpec>,
    pending: Option<PendingPage>,
    current: Option<PublishedPage>,
}

/// One retained image per active terrain/view, one globally in-flight replacement.
/// Dropping the scene cancels its work and releases all image leases.
#[derive(Resource, Default)]
pub(crate) struct TerrainReliefPages {
    views: HashMap<(Entity, Entity), ViewPage>,
    dirty: HashSet<(Entity, Entity)>,
}
impl SceneResource for TerrainReliefPages {}

pub(crate) struct ReliefStatus {
    pub current: bool,
    pub page: Option<([f64; 2], f64, f64)>,
    pub reused: usize,
    pub updated: usize,
    pub bytes: usize,
    pub atlas: Option<AssetId<Image>>,
}
impl TerrainReliefPages {
    pub(crate) fn status(&self, terrain: Entity, camera: Entity) -> Option<ReliefStatus> {
        let state = self.views.get(&(terrain, camera))?;
        let current = state.current.as_ref();
        Some(ReliefStatus {
            current: current
                .is_some_and(|p| Some(p.spec) == state.desired && p.upload_sequence.is_none()),
            page: current.map(|page| {
                (
                    page.spec.region().center,
                    page.spec.region().half,
                    page.spec.texel(),
                )
            }),
            reused: current.map_or(0, |p| p.reused),
            updated: current.map_or(0, |p| p.updated),
            bytes: current.map_or(0, |p| p.bytes),
            atlas: current.map(|p| p.image.id()),
        })
    }
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.views.len(),
            self.views
                .values()
                .filter(|v| {
                    v.current
                        .as_ref()
                        .is_some_and(|p| Some(p.spec) == v.desired && p.upload_sequence.is_none())
                })
                .count(),
        )
    }
}

fn update_pages(
    mut pages: ResMut<TerrainReliefPages>,
    images: Option<ResMut<Assets<Image>>>,
    mut uploads: ResMut<DataTextureUpdates>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    admission: Res<AsyncWorkAdmission>,
    demands: Res<TerrainDetailDemands>,
    terrains: Query<(Entity, &DemHeightField, &LodTiles)>,
) {
    let Some(mut images) = images else {
        return;
    };
    let Ok(profile) = quality.validated_profile() else {
        return;
    };
    let mut requests = Vec::new();
    for (terrain, field, tiles) in &terrains {
        for (camera, eye, forward, radius, focal_px) in demands.relief_foci(terrain) {
            let Some((mut spec, focus)) = page_spec(
                &field.0,
                eye,
                forward,
                radius,
                focal_px,
                profile.terrain_derived_map_resolution,
            ) else {
                continue;
            };
            // Preserve the world-aligned lattice through motion within its inner
            // retention area, instead of rebaking at every cell crossing.
            if let Some(previous) = pages
                .views
                .get(&(terrain, camera))
                .and_then(|state| state.desired)
                && previous.same_lattice(spec)
                && previous.retains_eye(focus)
            {
                spec = previous;
            }
            let ready = tiles
                .view_status(camera)
                .is_some_and(|view| view.initial_near_cover_ready);
            requests.push(((terrain, camera), spec, field.0.clone(), ready));
        }
    }
    requests.sort_unstable_by_key(|(key, _, _, _)| *key);
    let wanted: HashSet<_> = requests.iter().map(|(key, _, _, _)| *key).collect();
    let pages = pages.bypass_change_detection();
    pages.views.retain(|key, state| {
        if wanted.contains(key) {
            true
        } else {
            if let Some(page) = &state.current {
                uploads.retire(page.image.id());
            }
            pages.dirty.insert(*key);
            false
        }
    });
    for (key, spec, source, _) in &requests {
        let state = pages.views.entry(*key).or_default();
        state.desired = Some(*spec);
        if let Some(page) = state.current.as_mut() {
            if page.upload_sequence.is_some_and(|seq| {
                uploads
                    .uploaded
                    .get(&page.image.id())
                    .is_some_and(|s| s.sequence >= seq)
            }) {
                page.upload_sequence = None;
            }
        }
        if state
            .current
            .as_ref()
            .is_some_and(|page| page.spec.source_key != spec.source_key)
        {
            if let Some(page) = state.current.take() {
                uploads.retire(page.image.id());
            }
            pages.dirty.insert(*key);
        }
        if state.pending.as_ref().is_some_and(|p| {
            !p.spec.same_lattice(*spec)
                || (0..2).any(|axis| p.spec.start[axis].abs_diff(spec.start[axis]) > 1)
                || !Arc::ptr_eq(&p.source, source)
        }) {
            state.pending = None;
        }
        if let Some(pending) = &mut state.pending {
            if let Some((data, _permit)) = future::block_on(future::poll_once(&mut pending.task)) {
                let finished = pending.spec;
                state.pending = None;
                if let Some(result) = data {
                    let (image, upload_sequence) = match result.upload {
                        AtlasUpload::Initial(data) => {
                            if let Some(old) = state.current.take() {
                                uploads.retire(old.image.id());
                            }
                            (
                                images.add(data_texture(
                                    finished.resolution,
                                    data,
                                    profile.terrain_derived_texture_anisotropy,
                                )),
                                None,
                            )
                        }
                        AtlasUpload::Incremental(regions) => {
                            let Some(current) = state.current.as_ref() else {
                                continue;
                            };
                            let image = current.image.clone();
                            // Maintain the CPU shadow for normal asset re-preparation,
                            // but do not trigger a full texture allocation on a patch.
                            let Some(asset) = images.get_mut_untracked(image.id()) else {
                                continue;
                            };
                            let Some(data) = asset.data.as_mut() else {
                                continue;
                            };
                            if regions
                                .iter()
                                .any(|r| !write_region(data, finished.resolution, r))
                            {
                                error!("terrain relief patch violates the retained atlas layout");
                                continue;
                            }
                            let sequence = uploads.reserve_sequence();
                            uploads.submit(DataTexturePatch {
                                image: image.clone(),
                                sequence,
                                size: UVec2::splat(finished.resolution as u32),
                                texels: DataTextureTexels::Rgba8MipRegions(regions),
                            });
                            (image, Some(sequence))
                        }
                    };
                    state.current = Some(PublishedPage {
                        spec: finished,
                        image,
                        reused: result.reused,
                        updated: result.updated,
                        bytes: result.bytes,
                        upload_sequence,
                    });
                    pages.dirty.insert(*key);
                }
            }
        }
    }
    if pages.views.values().any(|state| state.pending.is_some()) {
        return;
    }
    requests.sort_by_key(|(key, _, _, _)| {
        (
            pages
                .views
                .get(key)
                .is_some_and(|state| state.current.is_some()),
            *key,
        )
    });
    for (key, spec, source, ready) in requests {
        if !ready {
            continue;
        }
        let state = pages.views.get_mut(&key).expect("admitted view");
        if state.current.as_ref().is_some_and(|page| page.spec == spec) {
            continue;
        }
        let identity = ((key.0.to_bits() as u128) << 64) | key.1.to_bits() as u128;
        let work_key = AsyncWorkKey::new(
            AsyncWorkKind::VisualizationPreparation,
            0,
            identity,
            spec.key(),
            0,
        );
        let Ok(permit) = admission.admit_external(AsyncWorkPriority::Background, work_key) else {
            continue;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let previous = state.current.as_ref().map(|page| page.spec);
        let task = AsyncComputeTaskPool::get().spawn({
            let source = source.clone();
            let cancelled = cancelled.clone();
            async move { (build_atlas(source, spec, previous, cancelled).await, permit) }
        });
        state.pending = Some(PendingPage {
            spec,
            source,
            cancelled,
            task,
        });
        break;
    }
}

type TileLook<'a> = (
    &'a LodTileOf,
    &'a lunco_render::camera::SceneViewGeometry,
    &'a mut ShaderLook,
);

fn bind_page(
    pages: &TerrainReliefPages,
    source: &LodTileOf,
    view: &lunco_render::camera::SceneViewGeometry,
    mut look: Mut<ShaderLook>,
) {
    let page = pages
        .views
        .get(&(source.0, view.camera))
        .and_then(|state| state.current.as_ref());
    let texture = page.map(|page| page.image.clone().into());
    if look.textures.get(&TextureLayer::TerrainRelief) != texture.as_ref() {
        if let Some(texture) = texture {
            look.textures.insert(TextureLayer::TerrainRelief, texture);
        } else {
            look.textures.remove(&TextureLayer::TerrainRelief);
        }
    }
    let region = page.map_or([0.0; 4], |page| {
        [
            page.spec.region().center[0] as f32,
            page.spec.region().center[1] as f32,
            page.spec.region().half as f32,
            page.spec.texel() as f32,
        ]
    });
    if look.values().get("relief_page") != Some(&ParamValue::Vec4(region)) {
        set_param(&mut look, "relief_page", ParamValue::Vec4(region));
    }
    let phase = page.map_or([0.0; 2], |page| {
        page.spec
            .start
            .map(|v| v.rem_euclid(CHUNKS) as f32 / CHUNKS as f32)
    });
    if look.values().get("relief_phase") != Some(&ParamValue::Vec2(phase)) {
        set_param(&mut look, "relief_phase", ParamValue::Vec2(phase));
    }
}

fn bind_pages(
    mut pages: ResMut<TerrainReliefPages>,
    mut tiles: ParamSet<(
        Query<TileLook, Without<TerrainDiagnosticTile>>,
        Query<TileLook, (Without<TerrainDiagnosticTile>, Changed<ShaderLook>)>,
    )>,
) {
    let pages = pages.bypass_change_detection();
    if !pages.dirty.is_empty() {
        for (source, view, look) in &mut tiles.p0() {
            if pages.dirty.contains(&(source.0, view.camera)) {
                bind_page(pages, source, view, look);
            }
        }
    }
    for (source, view, look) in &mut tiles.p1() {
        bind_page(pages, source, view, look);
    }
    pages.dirty.clear();
}

pub(crate) fn register(app: &mut App) {
    app.init_scene_resource::<TerrainReliefPages>();
    app.add_systems(
        lunco_core::SceneTeardown,
        |pages: Res<TerrainReliefPages>, mut uploads: ResMut<DataTextureUpdates>| {
            for state in pages.views.values() {
                if let Some(page) = &state.current {
                    uploads.retire(page.image.id());
                }
            }
        },
    );
    app.add_systems(
        Update,
        (update_pages, bind_pages)
            .chain()
            .after(crate::stream_viz::update_lod_tiles)
            .after(crate::stream_viz::bind_terrain_maps_to_materials)
            .before(lunco_materials::ShaderLookApplySet)
            .in_set(lunco_core::RuntimeCycleSet::Visualization),
    );
}
