//! Heightfield-ray-marched terrain sun shadows — the long-range half of the
//! two-system shadow design.
//!
//! Cascaded shadow maps cannot resolve kilometre-scale terrain shadows: the
//! atlas texels thin out with range, casters outside the cascade footprint
//! vanish, and coarse DEM triangles alias into sawtooth edges. A precomputed
//! horizon-angle map (NASA LOLA style) was tried first and rejected: storing
//! *angles per grid point* low-pass-filters the casting crests, smearing the
//! terminator over tens to hundreds of metres.
//!
//! The approach that actually delivers crisp, realistic shadow lines is the
//! terrain-renderer standard: **ray-march the heightfield itself, per pixel,
//! in the terrain shader** (`assets/shaders/horizon_march.wgsl`). At each
//! march step the occlusion factor is
//! `(ray_height − terrain_height) / (distance · tan(sun_radius))`, so the
//! penumbra is physically scaled — razor-sharp next to the casting crest,
//! softening linearly with caster distance, with the sun's angular size
//! taken from the UsdLux light's `inputs:angle`
//! ([`SunAngularDiameter`](lunco_core::SunAngularDiameter)). Any sun
//! direction works at any time — no relight pass, no latency.
//!
//! ## Pipeline
//!
//! 1. **Bake** (once per terrain, async, ~100 ms): rasterize the terrain
//!    `Mesh3d` into a heightmap over its local XZ bounds. The heightmap is
//!    geometry, not lighting — it never needs re-baking. Terrains opt in
//!    via the [`HorizonShadowTerrain`] marker (USD:
//!    `custom bool lunco:terrain:horizonShadows`).
//! 2. **Material wiring** (idempotent): the heightfield (R32Float texture)
//!    and sun uniforms are written into the terrain's
//!    [`ShaderMaterial`](lunco_materials::ShaderMaterial) — the authored
//!    one (e.g. regolith) if present, else a default `terrain_shadow.wgsl`
//!    is applied, keeping the prim's `displayColor` as albedo. The mesh
//!    gets planar UVs so shaders can address the heightfield.
//!
//!    The terrain stays a **CSM caster**: within the sun's cascade range the
//!    shadow map renders the actual mesh, giving mesh-accurate self-shadow
//!    and contact shadows (and the only terrain shadows on wasm, where the
//!    bake is skipped). The march fades in only beyond ~half the cascade
//!    range (`engine2.w` carries the CSM far bound), so its heightfield-
//!    texel-quantized edges never show up close and near pixels skip the
//!    march entirely.
//! 2b. **Shadow cache** (sun-driven, off-thread on native): the per-pixel
//!     march is expensive — up to 48 steps × 4 heightfield fetches per
//!     fragment. Because the sun moves slowly (a lunar day ≈ 29.5 Earth
//!     days), the march result is cached into an `R8Unorm` visibility
//!     texture ([`HorizonShadowCache`]) baked from the SAME
//!     [`HeightField::sun_visibility`] algorithm, refreshed only when the
//!     sun's terrain-local direction rotates past
//!     [`HorizonShadowCacheConfig::sun_threshold_deg`]. The fragment shader
//!     then does a single `textureSampleLevel` (guarded by the
//!     `shadow_cache_on` uniform) instead of the loop — dropping fragment
//!     cost to near zero. Defaults off on wasm (the streamed-tile path
//!     bypasses the march there, and an inline bake would hitch under a fast
//!     day cycle). See [`start_shadow_cache_bake`] / [`wire_terrain_materials`].
//! 3. **Dynamic shading**: every mesh entity's position is run through the
//!    SAME march on the CPU ([`HeightField::sun_visibility`]) — object and
//!    ground can never disagree. The visibility darkens the entity's
//!    material (`engine.x` for prop `ShaderMaterial`s; `base_color` scale
//!    for `StandardMaterial`s, cloned to a unique handle first so shared
//!    glb materials don't darken together), and a fully shadowed entity
//!    gets [`NotShadowCaster`]. (A `RenderLayers` swap does NOT work for
//!    darkening: a light's layers gate per-mesh *shadow casting*, but
//!    main-pass illumination is applied per view.)
//!
//! ## Limits (v1)
//!
//! - Heights are sampled at the entity's ground cell; a hovering object
//!   uses the ground-level result under it.
//! - Bake is skipped on wasm (`AsyncComputeTaskPool` is the main thread
//!   there).

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::image::ImageSampler;
use bevy::light::{CascadeShadowConfig, NotShadowCaster};
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_core::{HorizonShadowTerrain, SunAngularDiameter};
use lunco_materials::{ParamValue, ShaderMaterial};

/// Tan of the sun's angular radius for a diameter given in degrees.
fn tan_sun_radius(diameter_deg: f32) -> f32 {
    (diameter_deg.to_radians() * 0.5).tan()
}

/// The baked terrain heightfield. Cheap to clone (`Arc`) so bake tasks and
/// CPU queries share it. The GPU sees the same data as an R32Float texture.
#[derive(Clone)]
pub struct HeightField {
    resolution: u32,
    /// Terrain-local XZ bounds the grid covers.
    min: Vec2,
    size: Vec2,
    /// `resolution²` world-space heights, row-major over (x, z).
    heights: Arc<Vec<f32>>,
}

impl HeightField {
    /// Build a heightfield directly from a sampled grid — the mesh-less path
    /// for STREAMED terrains, whose ground truth is an analytic height oracle
    /// rather than a single `Mesh3d` (the caller samples the oracle row-major
    /// over (x, z)). `min`/`size` are the terrain-local XZ bounds the grid
    /// covers, exactly like the mesh-bake path derives from vertex bounds.
    pub fn from_grid(resolution: u32, min: Vec2, size: Vec2, heights: Arc<Vec<f32>>) -> Self {
        debug_assert_eq!(heights.len(), (resolution * resolution) as usize);
        Self { resolution, min, size: size.max(Vec2::splat(f32::EPSILON)), heights }
    }

    /// Bilinear height at grid coords `g` (clamped to the grid).
    fn height_at(&self, g: Vec2) -> f32 {
        let r = self.resolution as usize;
        let gx = g.x.clamp(0.0, (r - 1) as f32);
        let gy = g.y.clamp(0.0, (r - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(r - 1), (y0 + 1).min(r - 1));
        let (fx, fy) = (gx.fract(), gy.fract());
        let h = |x: usize, y: usize| self.heights[y * r + x];
        let top = h(x0, y0) + (h(x1, y0) - h(x0, y0)) * fx;
        let bot = h(x0, y1) + (h(x1, y1) - h(x0, y1)) * fx;
        top + (bot - top) * fy
    }

    /// Sun visibility 0..1 at a terrain-local XZ position — the CPU mirror
    /// of `horizon_march.wgsl::sun_visibility` (keep the two in sync!).
    /// `None` outside the heightfield bounds.
    pub fn sun_visibility(&self, local_xz: Vec2, sun_local: Vec3, tan_sun_r: f32) -> Option<f32> {
        let p0 = local_xz - self.min;
        if p0.x < 0.0 || p0.y < 0.0 || p0.x > self.size.x || p0.y > self.size.y {
            return None;
        }
        if sun_local.y <= 0.0 {
            return Some(0.0);
        }
        let horiz = Vec2::new(sun_local.x, sun_local.z);
        let hl = horiz.length();
        if hl < 1e-4 {
            return Some(1.0);
        }
        let dir = horiz / hl;
        let slope = sun_local.y / hl;
        let to_grid = (self.resolution - 1) as f32 / self.size;
        let h0 = self.height_at(p0 * to_grid) + 0.35;
        let texel = self.size.min_element() / (self.resolution - 1) as f32;
        let max_t = self.size.length() * 1.42;
        let mut vis: f32 = 1.0;
        let mut t = texel;
        for _ in 0..48 {
            let p = p0 + dir * t;
            if p.x < 0.0 || p.y < 0.0 || p.x > self.size.x || p.y > self.size.y {
                break;
            }
            let h = self.height_at(p * to_grid);
            let occ = (h0 + slope * t - h) / (t * tan_sun_r);
            vis = vis.min(occ);
            if vis <= 0.0 {
                return Some(0.0);
            }
            t = t * 1.18 + texel * 0.5;
            if t > max_t {
                break;
            }
        }
        let v = vis.clamp(0.0, 1.0);
        Some(v * v * (3.0 - 2.0 * v))
    }

    /// Side length of the heightfield grid (texels per edge).
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Bakes the sun-visibility cache — a `target_res²` grid of `u8` values
    /// (0..255 ← 0..1 visibility) sampled from [`sun_visibility`] over the
    /// heightfield footprint, for the given terrain-local sun direction.
    ///
    /// This is the CPU side of the **horizon shadow cache**: the result is
    /// uploaded to an `R8Unorm` texture the terrain fragment shader samples
    /// with a single `textureSampleLevel` instead of running the 48-step
    /// ray-march per pixel. `target_res` may differ from the heightfield's
    /// own resolution (a coarser cache on memory-constrained targets); each
    /// cache texel maps to `min + (frac) * size` and marches against the
    /// full-resolution heightfield, so the cache is always geometrically
    /// correct regardless of its own sampling rate.
    ///
    /// Reuses [`sun_visibility`] — the SAME algorithm the shader runs — so the
    /// cache and any live fallback march can never disagree. Off-thread on
    /// native (`AsyncComputeTaskPool`); inline on wasm (see
    /// [`start_shadow_cache_bake`]).
    pub fn bake_visibility_cache(
        &self,
        sun_local: Vec3,
        tan_sun_r: f32,
        target_res: u32,
    ) -> Vec<u8> {
        let res = target_res.max(2);
        let mut bytes = vec![0u8; (res as usize) * (res as usize)];
        let inv = 1.0 / (res - 1) as f32;
        for y in 0..res {
            for x in 0..res {
                let local_xz = self.min + Vec2::new(x as f32, y as f32) * inv * self.size;
                // Outside the heightfield → fully lit (no obstruction possible).
                let vis = self.sun_visibility(local_xz, sun_local, tan_sun_r).unwrap_or(1.0);
                bytes[(y as usize) * (res as usize) + (x as usize)] =
                    (vis * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        bytes
    }
}

/// Baked heightfield living on the terrain entity: CPU copy for entity
/// shading + the GPU texture wired into the terrain material.
#[derive(Component)]
pub struct HorizonMap {
    pub field: HeightField,
    image: Handle<Image>,
}

/// In-flight async bake for a terrain entity.
#[derive(Component)]
pub struct HorizonBakeTask(Task<BakeResult>);

// ─────────────────────────────────────────────────────────────────────────
// 1b. Horizon shadow cache — pre-bake the ray-march into an R8Unorm texture
// ─────────────────────────────────────────────────────────────────────────

/// Runtime-tunable knobs for the **horizon shadow cache** (see module docs §2b).
/// The cache replaces the per-pixel 48-step heightfield ray-march in the
/// terrain fragment shader with a single `textureSampleLevel` of a pre-baked
/// `R8Unorm` visibility texture, refreshed only when the sun's terrain-local
/// direction moves past [`sun_threshold_deg`](Self::sun_threshold_deg). Tune
/// live in the Inspector.
///
/// Defaults **off on wasm**: the streamed LOD tiles (the common web path) bypass
/// the march entirely (`Plain` mode + `NotShadowReceiver`), and the static
/// terrain defaults to the flat shader there too — so the march rarely runs on
/// web, while the inline (main-thread) cache bake would hitch during a
/// day-cycle animation. Native keeps the cache on: the static `regolith`
/// terrain marches per pixel, and the bake runs off-thread.
#[derive(Resource, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct HorizonShadowCacheConfig {
    /// Master switch. `false` → the fragment shader keeps ray-marching per
    /// pixel (the engine writes `shadow_cache_on = 0` and drops the cache
    /// binding). `true` → the cache is baked and sampled instead.
    pub enabled: bool,
    /// Re-bake the cache when the sun's terrain-local direction has rotated
    /// more than this many degrees from the direction it was last baked at.
    /// Small → crisper shadows during a moving sun, more frequent bakes;
    /// large → cheaper, shadows lag slightly behind the sun. A lunar day is
    /// ~29.5 Earth days (~0.5°/h), so 0.05° re-bakes roughly every 6 minutes
    /// of real time at 1× — and far less often when the sun is static.
    pub sun_threshold_deg: f32,
}

impl Default for HorizonShadowCacheConfig {
    fn default() -> Self {
        // Web: the march is bypassed by the streamed-tile path, and an inline
        // bake would hitch under a fast day cycle → cache off. Native: the
        // static regolith terrain marches per pixel → cache on (off-thread).
        #[cfg(target_arch = "wasm32")]
        let enabled = false;
        #[cfg(not(target_arch = "wasm32"))]
        let enabled = true;
        Self { enabled, sun_threshold_deg: 0.05 }
    }
}

/// Cap on the cache texture resolution for the inline wasm bake. The native
/// off-thread bake uses the full heightfield resolution (1:1); wasm bakes on
/// the main thread, so a coarser cache keeps the one-time cost a load hitch,
/// not a stall. Bilinear filtering upsamples it smoothly.
#[cfg(target_arch = "wasm32")]
const WASM_SHADOW_CACHE_MAX_RES: u32 = 256;

/// The baked sun-visibility cache on a terrain entity: an `R8Unorm` texture
/// (0..1 visibility over the heightfield footprint) + the terrain-local sun
/// direction it was baked at. The fragment shader samples this with a single
/// `textureSampleLevel` (guarded by the `shadow_cache_on` uniform) instead of
/// the 48-step ray-march. Refreshed by [`start_shadow_cache_bake`] when the
/// sun moves past the configured threshold.
#[derive(Component)]
pub struct HorizonShadowCache {
    /// The GPU visibility texture (binding 10). Swapped atomically when a
    /// re-bake finishes — the old handle is dropped, the new one takes over.
    pub image: Handle<Image>,
    /// Terrain-local to-sun direction the cache was baked for. The cache is
    /// valid (visually lossless) while the sun stays within `sun_threshold_deg`
    /// of this; beyond it a re-bake is queued.
    last_sun_local: Vec3,
}

/// In-flight async visibility-cache bake for a terrain entity (native only).
#[derive(Component)]
pub struct ShadowCacheBakeTask(Task<ShadowCacheResult>);

struct ShadowCacheResult {
    bytes: Vec<u8>,
    resolution: u32,
    sun_local: Vec3,
    millis: u128,
}

/// Marker: the horizon system inserted [`NotShadowCaster`] on this entity
/// (it sits in terrain shadow, so it cannot block sunlight). Only what we
/// inserted is ever removed — authored `NotShadowCaster`s are left alone.
#[derive(Component)]
pub struct HorizonShadowed;

/// Engine-applied darkening of a `StandardMaterial` entity inside terrain
/// shadow. Records the authored base colour (restored as visibility returns
/// to 1) and the last visibility written, to avoid re-uploading the asset
/// every frame.
#[derive(Component)]
pub struct HorizonShade {
    original: Color,
    last_vis: f32,
    /// The authored shared `StandardMaterial` handle (held strongly here while
    /// the entity is darkened). Restored when the entity returns to full
    /// sunlight, at which point the entity's only strong handle to the unique
    /// darkened clone drops and the clone is freed — so a shadowed prop never
    /// keeps a permanent extra material (CPU-4).
    shared: Handle<StandardMaterial>,
}

struct BakeResult {
    field: HeightField,
    millis: u128,
}

// ─────────────────────────────────────────────────────────────────────────
// 1. Bake — DEM mesh → heightfield
// ─────────────────────────────────────────────────────────────────────────

/// Kicks off an async heightfield bake for every opted-in terrain whose
/// mesh has loaded. Steady-state cost: the query is empty once every
/// terrain carries a `HorizonMap`.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut, unused_variables))]
pub fn start_horizon_bakes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    q: Query<
        (Entity, &HorizonShadowTerrain, &Mesh3d),
        // `Without<RenderLayers>` mirrors `pick_sun`: terrain spawned under the
        // RTT preview `scene_root` carries a RenderLayers and must NOT bake into
        // the main scene (ARC-1 — cross-scene contamination + wasted bake).
        (Without<HorizonMap>, Without<HorizonBakeTask>, Without<RenderLayers>),
    >,
) {
    for (entity, cfg, mesh3d) in &q {
        let Some(mesh) = meshes.get(&mesh3d.0) else { continue }; // not loaded yet
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            warn!("[horizon] terrain {entity:?} mesh has no Float32x3 positions; skipping");
            commands.entity(entity).remove::<HorizonShadowTerrain>();
            continue;
        };
        let positions = positions.clone();
        let indices: Vec<u32> = match mesh.indices() {
            Some(Indices::U32(v)) => v.clone(),
            Some(Indices::U16(v)) => v.iter().map(|&i| i as u32).collect(),
            None => (0..positions.len() as u32).collect(),
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            let resolution = cfg.resolution;
            info!(
                "[horizon] baking {resolution}² heightfield for {entity:?} ({} verts) \
                 in background…",
                positions.len()
            );
            let task = AsyncComputeTaskPool::get()
                .spawn(async move { bake_heightfield(&positions, &indices, resolution) });
            commands.entity(entity).insert(HorizonBakeTask(task));
        }
        #[cfg(target_arch = "wasm32")]
        {
            // No worker threads on wasm (AsyncComputeTaskPool runs on the main
            // thread), so bake INLINE at a reduced resolution: a one-time load
            // cost — never a per-frame stall — so the web build still gets
            // far-field terrain shadows instead of none.
            let resolution = cfg.resolution.min(WASM_HORIZON_MAX_RES);
            info!(
                "[horizon] baking {resolution}² heightfield inline for {entity:?} \
                 ({} verts) on wasm…",
                positions.len()
            );
            let result = bake_heightfield(&positions, &indices, resolution);
            install_horizon_map(
                &mut commands,
                &mut meshes,
                &mut images,
                entity,
                mesh3d,
                result.field,
                result.millis,
            );
        }
    }
}

/// Pure CPU bake: rasterize the DEM triangles into a max-height grid.
fn bake_heightfield(positions: &[[f32; 3]], indices: &[u32], resolution: u32) -> BakeResult {
    let start = std::time::Instant::now();
    let r = resolution as usize;

    let (mut min, mut max) = (Vec2::MAX, Vec2::MIN);
    for p in positions {
        min = min.min(Vec2::new(p[0], p[2]));
        max = max.max(Vec2::new(p[0], p[2]));
    }
    let size = (max - min).max(Vec2::splat(f32::EPSILON));
    let to_grid = (resolution - 1) as f32 / size;

    let mut heights = vec![f32::NEG_INFINITY; r * r];
    for tri in indices.chunks_exact(3) {
        let p: [Vec3; 3] = [
            Vec3::from(positions[tri[0] as usize]),
            Vec3::from(positions[tri[1] as usize]),
            Vec3::from(positions[tri[2] as usize]),
        ];
        let g: [Vec2; 3] = [0, 1, 2].map(|i| (Vec2::new(p[i].x, p[i].z) - min) * to_grid);
        let (lo_x, hi_x) = (
            g.iter().map(|v| v.x).fold(f32::MAX, f32::min).floor().max(0.0) as usize,
            (g.iter().map(|v| v.x).fold(f32::MIN, f32::max).ceil() as usize).min(r - 1),
        );
        let (lo_y, hi_y) = (
            g.iter().map(|v| v.y).fold(f32::MAX, f32::min).floor().max(0.0) as usize,
            (g.iter().map(|v| v.y).fold(f32::MIN, f32::max).ceil() as usize).min(r - 1),
        );
        let d = (g[1] - g[0]).perp_dot(g[2] - g[0]);
        if d.abs() < 1e-12 {
            continue;
        }
        for iy in lo_y..=hi_y {
            for ix in lo_x..=hi_x {
                let q = Vec2::new(ix as f32, iy as f32);
                let w0 = (g[1] - q).perp_dot(g[2] - q) / d;
                let w1 = (g[2] - q).perp_dot(g[0] - q) / d;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    let h = w0 * p[0].y + w1 * p[1].y + w2 * p[2].y;
                    let cell = &mut heights[iy * r + ix];
                    *cell = cell.max(h);
                }
            }
        }
    }
    // Cells no triangle covered: lowest terrain, so they never fabricate
    // obstructions.
    let floor = heights.iter().copied().filter(|h| h.is_finite()).fold(f32::MAX, f32::min);
    for h in &mut heights {
        if !h.is_finite() {
            *h = floor;
        }
    }

    BakeResult {
        field: HeightField { resolution, min, size, heights: Arc::new(heights) },
        millis: start.elapsed().as_millis(),
    }
}

/// Collects finished bakes: installs the `HorizonMap` (CPU field + R32Float
/// GPU texture) and gives the mesh planar UVs (how shaders address the
/// heightfield). The terrain deliberately STAYS a CSM caster — cascades
/// supply the mesh-accurate near-field self-shadow; the march covers the
/// range beyond them. Material wiring happens in [`wire_terrain_materials`].
pub fn finish_horizon_bakes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<(Entity, &mut HorizonBakeTask, &Mesh3d)>,
) {
    use bevy::tasks::futures_lite::future;
    for (entity, mut task, mesh3d) in &mut q {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else { continue };
        install_horizon_map(
            &mut commands,
            &mut meshes,
            &mut images,
            entity,
            mesh3d,
            result.field,
            result.millis,
        );
    }
}

/// Reduced heightfield resolution for the inline wasm bake (vs. the native
/// async bake's full `cfg.resolution`): keeps the one-time synchronous bake
/// short enough to be a load hitch, not a stall.
#[cfg(target_arch = "wasm32")]
const WASM_HORIZON_MAX_RES: u32 = 512;

/// Installs a finished bake on the terrain entity: gives the mesh planar UVs
/// (`(local.xz - field.min) / field.size`, the exact inverse of the shader's
/// `uv * size` heightfield addressing — so `#ifdef VERTEX_UVS_A` lights up and
/// the march samples the right texels), uploads the R32Float heightfield
/// texture, and inserts the `HorizonMap`. Shared by the native async finish
/// and the wasm inline bake so the two paths can never drift.
fn install_horizon_map(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    entity: Entity,
    mesh3d: &Mesh3d,
    field: HeightField,
    millis: u128,
) {
    if let Some(mut mesh) = meshes.get_mut(&mesh3d.0) {
        if let Some(VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        {
            let uvs: Vec<[f32; 2]> = pos
                .iter()
                .map(|p| {
                    let uv = (Vec2::new(p[0], p[2]) - field.min) / field.size;
                    [uv.x, uv.y]
                })
                .collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        }
    }
    install_horizon_map_from_field(commands, images, entity, field, millis);
}

/// Mesh-less install: uploads the R32Float heightfield texture and inserts the
/// `HorizonMap`, WITHOUT touching any mesh UVs. Public for the streamed-terrain
/// path (LOD tiles already carry DEM-global planar UVs matching the field's
/// `min`/`size` addressing) — the shadow-cache bake and entity shading then run
/// off this map exactly as they do for a static-mesh terrain.
pub fn install_horizon_map_from_field(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    entity: Entity,
    field: HeightField,
    millis: u128,
) {
    let bytes: Vec<u8> = field.heights.iter().flat_map(|h| h.to_le_bytes()).collect();
    let image = images.add(Image::new(
        Extent3d { width: field.resolution, height: field.resolution, depth_or_array_layers: 1 },
        TextureDimension::D2,
        bytes,
        TextureFormat::R32Float,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    ));

    info!(
        "[horizon] heightfield installed for {entity:?}: {}² in {} ms — far-field \
         ray-march shadows active (near field stays on CSM)",
        field.resolution, millis
    );
    commands.entity(entity).remove::<HorizonBakeTask>().insert(HorizonMap { field, image });
}

// ─────────────────────────────────────────────────────────────────────────
// 2b. Shadow cache bake — pre-bake the ray-march into an R8Unorm texture
// ─────────────────────────────────────────────────────────────────────────

/// Builds (or replaces) the `R8Unorm` visibility texture from baked bytes.
/// Linear filtering so the fragment shader's `textureSampleLevel` bilinearly
/// interpolates the cache (the visibility is smooth across penumbra, so this
/// is visually lossless and avoids per-texel march edges).
fn make_shadow_cache_image(images: &mut Assets<Image>, bytes: Vec<u8>, res: u32) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d { width: res, height: res, depth_or_array_layers: 1 },
        TextureDimension::D2,
        bytes,
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

/// Kicks off a visibility-cache (re)bake for every horizon terrain whose sun
/// has rotated past the configured threshold since its last bake — the
/// low-frequency update that makes the cache worthwhile (a lunar day is
/// ~29.5 Earth days, so at 0.05° the cache re-bakes roughly every 6 minutes
/// at 1×, and far less when the sun is static).
///
/// Debounced by `Without<ShadowCacheBakeTask>`: while a bake is in flight the
/// sun may keep moving, but no second bake starts — the stale cache stays
/// bound (visually lossless within the threshold) and the next bake fires
/// once the in-flight one lands and the sun is still past threshold. This
/// naturally limits the re-bake rate to the bake duration, so a fast
/// day-cycle animation doesn't queue an unbounded backlog.
///
/// Below-horizon sun is skipped: the march short-circuits to 0 there (its
/// first branch), and `wire_terrain_materials` drops `shadow_cache_on` to 0
/// so the shader falls back to that cheap march instead of sampling a stale
/// above-horizon cache. A fresh bake fires when the sun rises past the
/// threshold again.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut, unused_variables))]
#[allow(clippy::type_complexity)]
pub fn start_shadow_cache_bake(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    cfg: Res<HorizonShadowCacheConfig>,
    sun: Query<
        (
            &GlobalTransform,
            &DirectionalLight,
            Option<&SunAngularDiameter>,
            Option<&CascadeShadowConfig>,
        ),
        Without<RenderLayers>,
    >,
    terrains: Query<
        (Entity, &GlobalTransform, &HorizonMap, Option<&HorizonShadowCache>),
        (Without<RenderLayers>, Without<ShadowCacheBakeTask>),
    >,
) {
    if !cfg.enabled {
        return;
    }
    let Some((sun_gt, tan_r, _csm_far)) = pick_sun(&sun) else { return };
    let to_sun_world: Vec3 = sun_gt.back().into();
    let cos_thresh = cfg.sun_threshold_deg.to_radians().cos();

    for (entity, terrain_gt, map, cache) in &terrains {
        let sun_local = terrain_gt
            .affine()
            .inverse()
            .transform_vector3(to_sun_world)
            .normalize_or_zero();
        // No sun / sun below horizon → the march handles it cheaply; no bake.
        if sun_local.y <= 1e-4 {
            continue;
        }
        // Within the threshold of the last bake → cache still valid.
        if let Some(c) = cache {
            if c.last_sun_local.dot(sun_local) >= cos_thresh {
                continue;
            }
        }

        let target_res = {
            #[cfg(target_arch = "wasm32")]
            {
                map.field.resolution().min(WASM_SHADOW_CACHE_MAX_RES).max(2)
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                map.field.resolution().max(2)
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Off-thread: clone the HeightField (cheap — `Arc<Vec<f32>>`)
            // and march every cache texel on a worker. The frame never blocks.
            let field = map.field.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                let start = std::time::Instant::now();
                let bytes = field.bake_visibility_cache(sun_local, tan_r, target_res);
                ShadowCacheResult { bytes, resolution: target_res, sun_local, millis: start.elapsed().as_millis() }
            });
            commands.entity(entity).insert(ShadowCacheBakeTask(task));
        }
        #[cfg(target_arch = "wasm32")]
        {
            // Inline (no worker threads on wasm): a one-time cost per sun
            // threshold crossing, at a capped resolution. The threshold makes
            // it rare; the cap keeps it a hitch, not a stall.
            let start = std::time::Instant::now();
            let bytes = map.field.bake_visibility_cache(sun_local, tan_r, target_res);
            let millis = start.elapsed().as_millis();
            install_shadow_cache(&mut commands, &mut images, entity, bytes, target_res, sun_local, millis);
        }
    }
}

/// Collects finished off-thread visibility-cache bakes and installs them
/// (native). The cache handle swaps atomically: the old `HorizonShadowCache`
/// image is dropped, the fresh one takes over, and `wire_terrain_materials`
/// rebinds it next frame. While the bake ran the stale cache stayed bound
/// (within the sun threshold → visually lossless).
pub fn finish_shadow_cache_bake(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<(Entity, &mut ShadowCacheBakeTask)>,
) {
    use bevy::tasks::futures_lite::future;
    for (entity, mut task) in &mut q {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else { continue };
        install_shadow_cache(
            &mut commands,
            &mut images,
            entity,
            result.bytes,
            result.resolution,
            result.sun_local,
            result.millis,
        );
    }
}

/// Installs a finished visibility-cache bake on the terrain entity: uploads
/// the `R8Unorm` texture and inserts/replaces [`HorizonShadowCache`]. The
/// previous cache image handle (if any) is dropped here — the material
/// rebinding happens in [`wire_terrain_materials`] next frame. Shared by the
/// native async finish and the wasm inline bake so the two paths can't drift.
fn install_shadow_cache(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    entity: Entity,
    bytes: Vec<u8>,
    resolution: u32,
    sun_local: Vec3,
    millis: u128,
) {
    let image = make_shadow_cache_image(images, bytes, resolution);
    debug!(
        "[horizon] shadow cache baked for {entity:?}: {resolution}² in {millis} ms"
    );
    commands
        .entity(entity)
        .remove::<ShadowCacheBakeTask>()
        .insert(HorizonShadowCache { image, last_sun_local: sun_local });
}

// ─────────────────────────────────────────────────────────────────────────
// 2. Material wiring — heightfield + sun uniforms into the terrain shader
// ─────────────────────────────────────────────────────────────────────────

/// The single sun all horizon systems agree on: the brightest
/// `DirectionalLight` not scoped to another render layer (preview-viewport
/// suns carry `RenderLayers`). Deterministic — query iteration order is
/// not, and systems disagreeing on the sun shows up as flickering
/// half-shaded objects. Returns the transform, tan(angular radius), and the
/// CSM far bound in metres (0 when the sun casts no cascade shadows — the
/// march then covers the whole range).
#[allow(clippy::type_complexity)]
fn pick_sun<'a>(
    sun: &'a Query<
        (
            &GlobalTransform,
            &DirectionalLight,
            Option<&SunAngularDiameter>,
            Option<&CascadeShadowConfig>,
        ),
        Without<RenderLayers>,
    >,
) -> Option<(&'a GlobalTransform, f32, f32)> {
    sun.iter().max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance)).map(
        |(gt, light, ang, csm)| {
            let csm_far = if light.shadow_maps_enabled {
                csm.and_then(|c| c.bounds.last().copied()).unwrap_or(0.0)
            } else {
                0.0
            };
            // A sun with no authored angular size must not yield tan(0)=0
            // (→ div-by-zero in the march). Default to Sol's ~0.53° diameter.
            let diameter_deg = ang.map(|a| a.0).filter(|d| *d > 0.0).unwrap_or(0.53);
            (gt, tan_sun_radius(diameter_deg), csm_far)
        },
    )
}

/// Keeps every horizon terrain's `ShaderMaterial` wired: heightfield
/// texture, static `engine2` (size/resolution), the per-frame sun direction
/// in `engine`, and the **shadow cache** binding + `shadow_cache_on` flag.
/// A terrain with no authored shader gets the default `terrain_shadow.wgsl`
/// (albedo from its `displayColor`). Idempotent and self-healing against
/// later material swaps; steady-state cost is a uniform compare per terrain
/// (writes only when the sun moves or the cache swaps).
#[allow(clippy::type_complexity)]
pub fn wire_terrain_materials(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cfg: Res<HorizonShadowCacheConfig>,
    sun: Query<
        (
            &GlobalTransform,
            &DirectionalLight,
            Option<&SunAngularDiameter>,
            Option<&CascadeShadowConfig>,
        ),
        Without<RenderLayers>,
    >,
    shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    std_mats: Res<Assets<StandardMaterial>>,
    terrains: Query<
        (
            Entity,
            &GlobalTransform,
            &HorizonMap,
            Option<&HorizonShadowCache>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
        ),
        // Skip preview-layer terrain (ARC-1) — mirrors `pick_sun`.
        Without<RenderLayers>,
    >,
    // Hysteresis state for the cache↔march handoff, per terrain (see below).
    mut cache_engaged: Local<std::collections::HashMap<Entity, bool>>,
) {
    let Some(mut shader_mats) = shader_mats else { return };
    let Some((sun_gt, tan_r, csm_far)) = pick_sun(&sun) else { return };
    let to_sun_world: Vec3 = sun_gt.back().into();

    for (entity, terrain_gt, map, shadow_cache, shader_mat, std_mat) in &terrains {
        let sun_local = terrain_gt
            .affine()
            .inverse()
            .transform_vector3(to_sun_world)
            .normalize_or_zero();
        let engine = Vec4::new(sun_local.x, sun_local.y, sun_local.z, tan_r);
        let engine2 = Vec4::new(
            map.field.size.x,
            map.field.size.y,
            map.field.resolution as f32,
            csm_far,
        );

        // Shadow cache binding + the uniform flag that tells the fragment
        // shader to sample it (`1.0`) instead of ray-marching (`0.0`). The
        // handle is bound whenever a cache exists (it stays allocated on the
        // `HorizonShadowCache` component regardless); only the flag toggles —
        // cheap uniform write, no bind-group churn — when the sun dips below
        // the horizon or the cache is disabled. Below-horizon sun falls back
        // to the march, which short-circuits to 0 in its first branch.
        let cache_image: Option<Handle<Image>> = shadow_cache.map(|c| c.image.clone());
        // HYSTERESIS on the cache↔march handoff. A single hard threshold
        // (`y > 1e-4`) flaps when the real sun sits AT the horizon — exactly
        // the polar-site situation — because every f32 ULP step of the light
        // direction or terrain GT crosses it, alternating the ENTIRE terrain
        // between baked-cache shadows and the ray-march's below-horizon
        // short-circuit: "the shadow on the moon oscillates back and forth".
        // Engage above ~0.3° elevation, release below ~0.06° — the band is
        // wider than any per-frame jitter, so the mode changes at most once
        // per real sunrise/sunset.
        let engaged = {
            let prev = cache_engaged.get(&entity).copied().unwrap_or(false);
            let now = if prev { sun_local.y > 1.0e-3 } else { sun_local.y > 5.0e-3 };
            cache_engaged.insert(entity, now);
            now
        };
        let shadow_cache_on: f32 =
            if cfg.enabled && engaged && cache_image.is_some() { 1.0 } else { 0.0 };

        // Named engine uniforms consumed by the terrain shaders (regolith /
        // terrain_shadow declare these in their `Material` struct; the engine
        // packs them at the reflected offsets).
        let sun_dir = ParamValue::Vec3([engine.x, engine.y, engine.z]);
        // World-space to-sun for the BRDF opposition term. The march uses the
        // terrain-LOCAL `sun_dir` (heightfield space); the lunar BRDF runs in
        // world space (world N/V), so it needs the world-space sun. Passing the
        // CPU-picked canonical sun here means the shader never has to guess it
        // from `directional_lights[0]` — robust to the earthshine fill light.
        let sun_dir_world = ParamValue::Vec3([to_sun_world.x, to_sun_world.y, to_sun_world.z]);
        let hf_size = ParamValue::Vec2([engine2.x, engine2.y]);
        let write_engine = |m: &mut ShaderMaterial| {
            // Handle is a cheap Arc bump, but skip even that when unchanged (MAT-3).
            if m.height_map.as_ref() != Some(&map.image) {
                m.height_map = Some(map.image.clone());
            }
            // Shadow cache handle: swap only when the baked image changes
            // (first bind / re-bake finished). Stays bound otherwise.
            if m.shadow_cache != cache_image {
                m.shadow_cache = cache_image.clone();
            }
            // One repack for all engine fields instead of one-per-field (MAT-1).
            m.set_many([
                ("sun_dir", sun_dir),
                ("sun_dir_world", sun_dir_world),
                ("sun_tan_radius", ParamValue::F32(tan_r)),
                ("hf_size", hf_size),
                ("hf_res", ParamValue::F32(engine2.z)),
                ("csm_far", ParamValue::F32(csm_far)),
                ("shadow_cache_on", ParamValue::F32(shadow_cache_on)),
            ]);
        };

        if let Some(handle) = shader_mat {
            // Compare before `get_mut` — a blind `get_mut` re-uploads the
            // asset every frame. Sun direction + heightfield identity + csm
            // bound + cache handle/flag cover everything that changes.
            let needs = shader_mats.get(&handle.0).is_some_and(|m| {
                m.height_map.as_ref() != Some(&map.image)
                    || m.shadow_cache != cache_image
                    || m.get_scalar("shadow_cache_on").is_none_or(|s| (s - shadow_cache_on).abs() > 1e-3)
                    || m.get_vec4("sun_dir")
                        .is_none_or(|v| (v.truncate() - Vec3::new(engine.x, engine.y, engine.z)).length() > 1e-4)
                    || m.get_scalar("csm_far").is_none_or(|c| (c - csm_far).abs() > 1e-3)
            });
            if needs {
                if let Some(mut m) = shader_mats.get_mut(&handle.0) {
                    write_engine(&mut m);
                }
            }
        } else {
            // No authored shader: apply the default ray-march terrain
            // shader, carrying the displayColor albedo over.
            let albedo = std_mat
                .and_then(|h| std_mats.get(&h.0))
                .map(|m| m.base_color)
                .unwrap_or(Color::srgb(0.5, 0.5, 0.5));
            let a = albedo.to_linear();
            let mut material = ShaderMaterial {
                shader: asset_server.load("shaders/terrain_shadow.wgsl"),
                height_map: Some(map.image.clone()),
                shadow_cache: cache_image.clone(),
                ..Default::default()
            };
            material.set("albedo", ParamValue::Vec3([a.red, a.green, a.blue]));
            write_engine(&mut material);
            let handle = shader_mats.add(material);
            info!("[horizon] applied terrain_shadow.wgsl to {entity:?}");
            commands
                .entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(handle));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 3. Dynamic objects — darken by CPU-marched visibility
// ─────────────────────────────────────────────────────────────────────────

/// Minimum sun movement (cosine of angle) before objects are re-evaluated
/// — ~0.1°.
const SUN_EPSILON_COS: f32 = 0.999_998_5;

/// Scales a colour's linear RGB by `q`, keeping alpha.
fn scale_color(c: Color, q: f32) -> Color {
    let l = c.to_linear();
    Color::LinearRgba(LinearRgba::new(l.red * q, l.green * q, l.blue * q, l.alpha))
}

/// Runs every mesh entity's position through the same heightfield march the
/// terrain shader uses and darkens the entity by its sun visibility (see
/// module docs §3). Change-driven: a full pass only when the sun moved;
/// otherwise only entities whose `GlobalTransform` changed.
#[allow(clippy::type_complexity)]
pub fn shade_dynamic_entities(
    mut commands: Commands,
    mut last_sun: Local<Option<Vec3>>,
    mut sweep_timer: Local<Option<Timer>>,
    time: Res<Time>,
    sun: Query<
        (
            &GlobalTransform,
            &DirectionalLight,
            Option<&SunAngularDiameter>,
            Option<&CascadeShadowConfig>,
        ),
        Without<RenderLayers>,
    >,
    terrains: Query<(&GlobalTransform, &HorizonMap), Without<RenderLayers>>,
    mut shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut entities: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            Has<RenderLayers>,
            Has<HorizonShadowed>,
            Has<NotShadowCaster>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&mut HorizonShade>,
            Option<&Name>,
        ),
        (With<Mesh3d>, Without<HorizonMap>, Without<DirectionalLight>),
    >,
) {
    if terrains.is_empty() {
        return;
    }
    let Some((sun_gt, tan_r, _csm_far)) = pick_sun(&sun) else { return };
    let to_sun_world: Vec3 = sun_gt.back().into();

    // Throttle the expensive full sweep — O(entities × terrains × ≤48-step
    // CPU ray-march) — to ~30 Hz so it no longer fires at uncapped render FPS
    // (120–175) every frame the sun animates (day cycle, `SetEnvironmentLight`
    // slider drag). Moving entities still update every frame via the
    // `gt.is_changed()` fast path below; only the sun-moved full pass is gated.
    let timer = sweep_timer
        .get_or_insert_with(|| Timer::from_seconds(1.0 / 30.0, TimerMode::Repeating));
    timer.tick(time.delta());

    let sun_moved = match *last_sun {
        Some(prev) => prev.dot(to_sun_world) <= SUN_EPSILON_COS,
        None => true,
    };
    // Commit to a full sweep only when the throttle fires (or on first run).
    // Until then `last_sun` is NOT advanced, so a sun change arriving between
    // ticks is still picked up at the next tick (≤33 ms later — imperceptible
    // given the 1/32 visibility quantization).
    let do_full = sun_moved && (timer.just_finished() || last_sun.is_none());
    if do_full {
        *last_sun = Some(to_sun_world);
    }

    // Per-terrain loop-invariants — the affine inverse and sun-in-terrain-local
    // depend only on the terrain transform + sun, not the shaded entity — so
    // compute them once here instead of N×M times inside the entity loop (CPU-2;
    // `transform_point3(entity_pos)` stays inside since it is entity-dependent).
    let terrain_cache: Vec<_> = terrains
        .iter()
        .map(|(terrain_gt, map)| {
            let inv = terrain_gt.affine().inverse();
            let sun_local = inv.transform_vector3(to_sun_world).normalize_or_zero();
            (inv, sun_local, map)
        })
        .collect();

    for (entity, gt, has_layers, shadowed, has_nsc, shader_mat, std_mat, shade, name) in
        &mut entities
    {
        if !do_full && !gt.is_changed() {
            continue;
        }
        // Entities scoped to other render layers (preview viewports, viz
        // overlays) live outside the main scene's lighting — leave alone.
        if has_layers {
            continue;
        }

        // Min visibility across all horizon terrains containing the point —
        // the SAME march the terrain pixels run.
        let mut vis: f32 = 1.0;
        for (inv, sun_local, map) in &terrain_cache {
            let local = inv.transform_point3(gt.translation());
            if let Some(v) =
                map.field.sun_visibility(Vec2::new(local.x, local.z), *sun_local, tan_r)
            {
                vis = vis.min(v);
            }
        }
        // Quantized so a slowly drifting sun doesn't re-upload materials
        // every frame.
        let q = (vis * 32.0).round() / 32.0;

        // Prop ShaderMaterials (wheels, panels, balls): the engine channel
        // is multiplied into the shader's lit output.
        if let (Some(handle), Some(mats)) = (shader_mat, shader_mats.as_mut()) {
            let needs = mats
                .get(&handle.0)
                .is_some_and(|m| m.get_scalar("sun_vis").map_or(true, |s| (s - q).abs() > 1e-3));
            if needs {
                if let Some(mut m) = mats.get_mut(&handle.0) {
                    m.set_scalar("sun_vis", q);
                }
            }
        } else if let Some(handle) = std_mat {
            // StandardMaterials (chassis, props): scale the albedo. Cloned
            // to a unique handle on first shading so glb materials shared
            // across instances don't darken together.
            match shade {
                None => {
                    if q < 0.999 {
                        if let Some(mut m) = std_mats.get(&handle.0).cloned() {
                            let original = m.base_color;
                            m.base_color = scale_color(original, q);
                            let unique = std_mats.add(m);
                            debug!("[horizon-dbg] {entity:?} {name:?} vis={q:.2} SHADE-NEW (std)");
                            commands.entity(entity).insert((
                                MeshMaterial3d(unique),
                                HorizonShade { original, last_vis: q, shared: handle.0.clone() },
                            ));
                        }
                    }
                }
                Some(mut state) => {
                    if q >= 0.999 {
                        // Back in full sun: restore the shared authored material.
                        // Overwriting `MeshMaterial3d` drops the entity's only
                        // strong handle to the unique darkened clone, so the
                        // clone is freed rather than kept forever (CPU-4).
                        commands
                            .entity(entity)
                            .insert(MeshMaterial3d(state.shared.clone()))
                            .remove::<HorizonShade>();
                        debug!("[horizon-dbg] {entity:?} {name:?} vis={q:.2} SHADE-CLEAR (std)");
                    } else if (state.last_vis - q).abs() > 1e-3 {
                        if let Some(mut m) = std_mats.get_mut(&handle.0) {
                            m.base_color = scale_color(state.original, q);
                        }
                        debug!("[horizon-dbg] {entity:?} {name:?} vis={q:.2} SHADE-UPDATE (std)");
                        state.last_vis = q;
                    }
                }
            }
        }

        // A body inside terrain shadow receives no sunlight, so it must not
        // throw a CSM shadow onto lit ground at the terminator either.
        // Hysteresis avoids flicker; authored `NotShadowCaster`s are never
        // touched (we only remove what we inserted).
        if !shadowed && !has_nsc && vis < 0.35 {
            commands.entity(entity).insert((NotShadowCaster, HorizonShadowed));
        } else if shadowed && vis > 0.65 {
            commands.entity(entity).remove::<(NotShadowCaster, HorizonShadowed)>();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────

/// Registers the heightfield-shadow pipeline. Added by [`EnvironmentPlugin`]
/// — binaries need no extra wiring; terrains opt in via the
/// [`HorizonShadowTerrain`] marker (stamped by the USD loader).
pub struct HorizonShadowPlugin;

impl Plugin for HorizonShadowPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HorizonShadowTerrain>();
        app.register_type::<HorizonShadowCacheConfig>();
        app.init_resource::<HorizonShadowCacheConfig>();
        // Horizon baking/shading is a render-only feature: every system here
        // needs render-asset stores (`Assets<Mesh>`/`Assets<Image>`/materials).
        // Gate the whole chain on those existing so a headless app (cosim
        // integration tests, server builds) cleanly skips it instead of
        // panicking on a missing `ResMut<Assets<…>>`. The real app always has
        // them, so this is a no-op there.
        app.add_systems(
            Update,
            (
                start_horizon_bakes,
                finish_horizon_bakes,
                start_shadow_cache_bake,
                finish_shadow_cache_bake,
                wire_terrain_materials,
                shade_dynamic_entities,
            )
                .chain()
                .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_field(res: u32, min: Vec2, size: Vec2, heights: Vec<f32>) -> HeightField {
        assert_eq!(heights.len(), (res * res) as usize);
        HeightField { resolution: res, min, size, heights: Arc::new(heights) }
    }

    /// Zenith sun (straight up): every texel is fully lit — the march
    /// short-circuits to 1.0 in its `hl < 1e-4` branch, so the whole cache
    /// bakes to 255.
    #[test]
    fn bake_cache_zenith_all_lit() {
        let field = make_field(4, Vec2::ZERO, Vec2::splat(3.0), vec![0.0; 16]);
        let bytes = field.bake_visibility_cache(Vec3::new(0.0, 1.0, 0.0), 0.0046, 4);
        assert!(bytes.iter().all(|&b| b == 255), "zenith sun → all 255");
    }

    /// Sun below the horizon: every texel is fully shadowed — the march
    /// short-circuits to 0.0 in its `sun_local.y <= 0.0` branch, so the whole
    /// cache bakes to 0.
    #[test]
    fn bake_cache_below_horizon_all_shadow() {
        let field = make_field(4, Vec2::ZERO, Vec2::splat(3.0), vec![0.0; 16]);
        let bytes = field.bake_visibility_cache(Vec3::new(0.0, -1.0, 0.0), 0.0046, 4);
        assert!(bytes.iter().all(|&b| b == 0), "below-horizon sun → all 0");
    }

    /// A flat heightfield never occludes — the march finds no higher terrain
    /// in any direction, so visibility stays 1.0 for any above-horizon sun.
    #[test]
    fn bake_cache_flat_terrain_all_lit() {
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), vec![1.5; 64]);
        let sun = Vec3::new(1.0, 0.3, 0.2).normalize();
        let bytes = field.bake_visibility_cache(sun, 0.0046, 8);
        assert!(bytes.iter().all(|&b| b == 255), "flat terrain → all lit");
    }

    /// A ridge casts a shadow on the side away from the sun: texels beyond the
    /// ridge (between the ridge and the anti-sun direction) bake to 0, while
    /// texels on the sun-facing side stay lit. This is the core guarantee the
    /// cache reproduces the per-pixel march.
    #[test]
    fn bake_cache_ridge_casts_shadow() {
        // 8×8 grid, 7 m extent (1 m texels). A ridge runs along z at x=1.
        let mut heights = vec![0.0f32; 64];
        for y in 0..8 {
            heights[y * 8 + 1] = 5.0;
        }
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), heights);
        // Sun toward -x (low): the march steps toward -x, so texels with x>1
        // see the ridge as an occluder → shadowed; texels with x<=1 face the
        // sun and stay lit.
        let sun = Vec3::new(-1.0, 0.5, 0.0).normalize();
        let bytes = field.bake_visibility_cache(sun, 0.0046, 8);
        // Texel (3, 0) → world (3, 0): beyond the ridge → shadowed.
        assert_eq!(bytes[0 * 8 + 3], 0, "texel beyond the ridge is shadowed");
        // Texel (0, 0) → world (0, 0): sun-facing side → lit.
        assert_eq!(bytes[0 * 8 + 0], 255, "sun-facing texel is lit");
    }

    /// The cache may be baked at a coarser resolution than the heightfield
    /// (the wasm cap): each cache texel still maps to the right world position
    /// and marches against the full-resolution field, so a ridge shadow shows
    /// up at the downscaled rate too.
    #[test]
    fn bake_cache_coarser_resolution_still_shadows() {
        let mut heights = vec![0.0f32; 64];
        for y in 0..8 {
            heights[y * 8 + 1] = 5.0;
        }
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), heights);
        let sun = Vec3::new(-1.0, 0.5, 0.0).normalize();
        // Bake at 4² (half the heightfield resolution).
        let bytes = field.bake_visibility_cache(sun, 0.0046, 4);
        assert_eq!(bytes.len(), 16);
        // Cache texel (3, 0) → world (7, 0): beyond the ridge → shadowed.
        assert_eq!(bytes[0 * 4 + 3], 0, "coarse-cache texel beyond the ridge is shadowed");
        // Cache texel (0, 0) → world (0, 0): sun-facing → lit.
        assert_eq!(bytes[0 * 4 + 0], 255, "coarse-cache sun-facing texel is lit");
    }
}
