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
//! ## What lives here, and what does not
//!
//! **This module is render-free**: the heightfield, the bakes, the
//! sun-visibility cache and the R32Float/R8Unorm `Image` uploads are all
//! `bevy_mesh` / `bevy_image` / `bevy_light` — no `bevy_pbr`, no wgpu. It runs
//! headless.
//!
//! The half that writes those textures and uniforms INTO a concrete material —
//! `wire_terrain_materials` (terrain `ShaderMaterial`) and
//! `shade_dynamic_entities` (prop `ShaderMaterial` / `StandardMaterial`
//! darkening) — lives in **`lunco-render-bevy::horizon_shade`**. It is a
//! per-frame *uniform feed*, not appearance intent, so `PbrLook`/`ShaderLook`
//! cannot express it; the binder crate (the only one allowed to name a
//! material) hosts it instead — the same move `terrain_maps.rs` made for
//! `lunco-terrain-surface`. See `docs/architecture/render-decoupling.md`.
//!
//! ## Pipeline
//!
//! 1. **Bake** (once per terrain, async, ~100 ms): rasterize the terrain
//!    `Mesh3d` into a heightmap over its local XZ bounds. The heightmap is
//!    geometry, not lighting — it never needs re-baking. Terrains opt in
//!    via the [`HorizonShadowTerrain`] marker (USD:
//!    `custom bool lunco:terrain:horizonShadows`).
//! 2. **Material wiring** (idempotent, RENDER-SIDE): the heightfield
//!    (R32Float texture) and sun uniforms are written into the terrain's
//!    `ShaderMaterial` — the authored one (e.g. regolith) if present, else a
//!    default `terrain_shadow.wgsl` is applied, keeping the prim's
//!    `displayColor` as albedo. The mesh gets planar UVs (done here, in
//!    [`install_horizon_map`]) so shaders can address the heightfield.
//!
//!    The terrain stays a **CSM caster**: within the sun's cascade range the
//!    shadow map renders the actual mesh, giving mesh-accurate self-shadow
//!    and contact shadows (and the only terrain shadows on wasm, where the
//!    bake is skipped). The march fades in only beyond ~half the cascade
//!    range (`csm_far` carries the CSM far bound), so its heightfield-
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
//!     day cycle). See [`start_shadow_cache_bake`].
//! 3. **Dynamic shading** (RENDER-SIDE): every mesh entity's position is run
//!    through the SAME march on the CPU ([`HeightField::sun_visibility`]) —
//!    object and ground can never disagree. The visibility darkens the
//!    entity's material (`sun_vis` for prop `ShaderMaterial`s; `base_color`
//!    scale for `StandardMaterial`s, cloned to a unique handle first so
//!    shared glb materials don't darken together), and a fully shadowed
//!    entity gets `NotShadowCaster`. (A `RenderLayers` swap does NOT work for
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
use bevy::light::CascadeShadowConfig;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_core::{HorizonShadowTerrain, SunAngularDiameter};
// The POD texture descriptors `bevy_image` itself takes from `wgpu-types`.
// `bevy::render::render_resource` only RE-EXPORTS them, and importing THAT would
// drag wgpu + naga into this crate. `wgpu-types` is not `wgpu`.
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

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
            // Penumbra width floored at ONE heightfield texel: the physical width
            // `t * tan_sun_r` collapses far below a texel for near casters at grazing sun
            // (at lunar `tan_sun_r ≈ 0.0046` it is centimetres), quantizing the stored
            // visibility into a hard 0/1 staircase. The floor keeps the gradient
            // resolvable at the sampling rate.
            //
            // ONE texel, not two. The floor is the width of the shadow's soft EDGE, so it
            // also sets how far above the ray an occluder must stand to reach full umbra.
            // At two texels nothing ever got properly dark — a 5 m ridge 6 m away leaves
            // its own shadow only ~82% deep — which is why the deep-umbra tests demanded a
            // hard 0 and did not get one. One texel still spans the ramp across a sample,
            // and the 2×2 supersample in `bake_visibility_cache` does the anti-aliasing the
            // wider floor was over-compensating for.
            let width = (t * tan_sun_r).max(texel);
            // How far the terrain rises ABOVE the ray, measured in penumbra widths — the
            // ONLY thing that may darken a sample. `1.0 - rise` is the visibility it
            // implies: `1` while the ray is clear (`h <= ray`), fading to `0` once the
            // occluder stands a full penumbra width above it.
            //
            // The obvious-looking `(ray - h) / width` is WRONG, and was the bug: it
            // demanded the ray clear the terrain by a WHOLE penumbra width to read fully
            // lit, so once the 2-texel floor dominates (it does almost everywhere — at
            // lunar `tan_sun_r ≈ 0.0046` the physical width is centimetres) even perfectly
            // FLAT ground came back ~32% lit. The floor is a sampling-resolution device
            // for the shadow's soft EDGE; it must never dim ground that has no occluder.
            // Keep in sync with `horizon_march.wgsl`, which marches the same formula.
            let rise = (h - (h0 + slope * t)) / width;
            vis = vis.min(1.0 - rise);
            if vis <= 0.0 {
                return Some(0.0);
            }
            t = t * 1.18 + texel * 0.5;
            if t > max_t {
                break;
            }
        }
        // Linear penumbra — no terminal smoothstep: it steepened the already
        // texel-narrow transition band back into a near-binary edge.
        Some(vis.clamp(0.0, 1.0))
    }

    /// Side length of the heightfield grid (texels per edge).
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Terrain-local XZ extent the grid covers (metres). Read by the render-side
    /// material wiring to pack the `hf_size` shader uniform.
    pub fn size(&self) -> Vec2 {
        self.size
    }

    /// Terrain-local XZ origin of the grid (metres).
    pub fn min(&self) -> Vec2 {
        self.min
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
    ///
    /// [`sun_visibility`]: HeightField::sun_visibility
    pub fn bake_visibility_cache(
        &self,
        sun_local: Vec3,
        tan_sun_r: f32,
        target_res: u32,
    ) -> Vec<u8> {
        let res = target_res.max(2);
        let mut bytes = vec![0u8; (res as usize) * (res as usize)];
        let inv = 1.0 / (res - 1) as f32;
        // 2×2 supersample per cache texel: a single ray per texel aliases
        // along the sun azimuth into elongated streaks; averaging four
        // quarter-texel-offset marches antialiases the stored edge. The bake
        // runs off-thread on native, so the 4× cost never touches a frame.
        const SUB: [Vec2; 4] = [
            Vec2::new(-0.25, -0.25),
            Vec2::new(0.25, -0.25),
            Vec2::new(-0.25, 0.25),
            Vec2::new(0.25, 0.25),
        ];
        let max_g = (res - 1) as f32;
        for y in 0..res {
            for x in 0..res {
                let mut vis = 0.0;
                for s in SUB {
                    // CLAMP the subsample back into the grid. The ±0.25-texel offsets push
                    // the border texels' samples OUTSIDE the heightfield, where
                    // `sun_visibility` returns `None` — and `unwrap_or(1.0)` reads that as
                    // "fully lit". So every edge texel picked up spurious light: a
                    // below-horizon sun baked a lit rim instead of an all-dark cache, and
                    // the shadow behind a ridge leaked at the border. Clamping supersamples
                    // a real edge texel slightly inward, which is exactly right — there is
                    // no terrain out there to average in.
                    let g = (Vec2::new(x as f32, y as f32) + s).clamp(Vec2::ZERO, Vec2::splat(max_g));
                    let local_xz = self.min + g * inv * self.size;
                    // Now always in-bounds; `unwrap_or` is belt-and-braces against fp edge
                    // cases, not a real path.
                    vis += self.sun_visibility(local_xz, sun_local, tan_sun_r).unwrap_or(1.0);
                }
                bytes[(y as usize) * (res as usize) + (x as usize)] =
                    (vis * 0.25 * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        bytes
    }
}

/// Baked heightfield living on the terrain entity: CPU copy for entity
/// shading + the GPU texture the terrain material binds.
///
/// The R32Float `image` is uploaded here (render-free — an `Image` asset is
/// `bevy_image`); it is *bound* into the terrain's `ShaderMaterial` by
/// `lunco-render-bevy::horizon_shade`, the only crate allowed to name a material.
#[derive(Component)]
pub struct HorizonMap {
    pub field: HeightField,
    /// The heightfield as an R32Float texture (shader binding). Public so the
    /// render-side wiring can bind it without owning the bake.
    pub image: Handle<Image>,
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
            commands.entity(entity).try_insert(HorizonBakeTask(task));
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
// `Instant` is `bevy::platform::time::Instant` — portable (std on native,
// web-time on wasm). clippy's `disallowed_methods` ban targets the wasm-panicking
// `std::time::Instant`, but on the *native* target bevy's re-export resolves to
// the same `DefId`, so it fires on correct code. Documented false positive; see
// clippy.toml's Time section.
#[allow(clippy::disallowed_methods)]
fn bake_heightfield(positions: &[[f32; 3]], indices: &[u32], resolution: u32) -> BakeResult {
    let start = Instant::now();
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
/// range beyond them. Material wiring happens render-side
/// (`lunco-render-bevy::horizon_shade::wire_terrain_materials`).
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
    commands.entity(entity).remove::<HorizonBakeTask>().try_insert(HorizonMap { field, image });
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
/// first branch), and the render-side `wire_terrain_materials` drops
/// `shadow_cache_on` to 0 so the shader falls back to that cheap march instead
/// of sampling a stale above-horizon cache. A fresh bake fires when the sun
/// rises past the threshold again.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut, unused_variables))]
// `Instant` is bevy's portable clock (see `bake_heightfield`) — the
// `disallowed_methods` hit is the documented native-only false positive.
#[allow(clippy::type_complexity, clippy::disallowed_methods)]
pub fn start_shadow_cache_bake(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    cfg: Res<HorizonShadowCacheConfig>,
    sun: SunQuery,
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
                let start = Instant::now();
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
            let start = Instant::now();
            let bytes = map.field.bake_visibility_cache(sun_local, tan_r, target_res);
            let millis = start.elapsed().as_millis();
            install_shadow_cache(&mut commands, &mut images, entity, bytes, target_res, sun_local, millis);
        }
    }
}

/// Collects finished off-thread visibility-cache bakes and installs them
/// (native). The cache handle swaps atomically: the old `HorizonShadowCache`
/// image is dropped, the fresh one takes over, and the render-side
/// `wire_terrain_materials` rebinds it next frame. While the bake ran the stale
/// cache stayed bound (within the sun threshold → visually lossless).
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
/// rebinding happens render-side next frame. Shared by the native async finish
/// and the wasm inline bake so the two paths can't drift.
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
        .try_insert(HorizonShadowCache { image, last_sun_local: sun_local });
}

// ─────────────────────────────────────────────────────────────────────────
// 2. The canonical sun — shared by the bakes (here) and the render-side
//    material wiring (`lunco-render-bevy::horizon_shade`)
// ─────────────────────────────────────────────────────────────────────────

/// The sun query every horizon system agrees on. Named so the render-side
/// wiring declares the exact same one and reuses [`pick_sun`] instead of
/// re-implementing the pick — two implementations disagreeing on which light is
/// "the sun" is precisely what shows up as flickering half-shaded objects.
///
/// All four components are render-FREE: `DirectionalLight` /
/// `CascadeShadowConfig` are `bevy_light`, `RenderLayers` is `bevy_camera`.
pub type SunQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        &'static DirectionalLight,
        Option<&'static SunAngularDiameter>,
        Option<&'static CascadeShadowConfig>,
    ),
    Without<RenderLayers>,
>;

/// The single sun all horizon systems agree on: the brightest
/// `DirectionalLight` not scoped to another render layer (preview-viewport
/// suns carry `RenderLayers`). Deterministic — query iteration order is
/// not. Returns the transform, tan(angular radius), and the CSM far bound in
/// metres (0 when the sun casts no cascade shadows — the march then covers the
/// whole range).
pub fn pick_sun<'a>(sun: &'a SunQuery) -> Option<(&'a GlobalTransform, f32, f32)> {
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

// ─────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────

/// Registers the heightfield-shadow **bake** pipeline (render-free). Added by
/// [`EnvironmentPlugin`](crate::EnvironmentPlugin) — binaries need no extra
/// wiring; terrains opt in via the [`HorizonShadowTerrain`] marker (stamped by
/// the USD loader).
///
/// The material half (`wire_terrain_materials` / `shade_dynamic_entities`) is
/// added by `lunco_render_bevy::LuncoRenderPlugin`, and ordered after these
/// systems. Headless simply never adds it: the heightfields and caches are still
/// baked (they are simulation-visible data — a shadow query, a solar-power
/// model), they just never reach a GPU material.
pub struct HorizonShadowPlugin;

impl Plugin for HorizonShadowPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HorizonShadowTerrain>();
        app.register_type::<HorizonShadowCacheConfig>();
        app.init_resource::<HorizonShadowCacheConfig>();
        // Every system here needs the asset stores (`Assets<Mesh>`/`Assets<Image>`).
        // Gate the chain on those existing so an app with no AssetPlugin (cosim
        // integration tests) cleanly skips it instead of panicking on a missing
        // `ResMut<Assets<…>>`. The real app always has them, so this is a no-op there.
        app.add_systems(
            Update,
            (
                start_horizon_bakes,
                finish_horizon_bakes,
                start_shadow_cache_bake,
                finish_shadow_cache_bake,
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
        // 8×8 grid, 7 m extent (1 m texels). A ridge runs along z at x=1..=2.
        //
        // TWO texels wide, deliberately. A one-texel spike is degenerate for ANY
        // point-sampling march: the ray takes geometrically growing steps, so from a start
        // that is not grid-aligned the samples straddle the crest (landing either side of
        // it) and the occluder is stepped straight over. The old single-sample bake only
        // passed because it started exactly on a grid column and a step happened to land on
        // the peak — supersampling, which samples off-grid by design, exposed that. A ridge
        // at least as wide as the march's step cannot be missed, and "a ridge shadows the
        // ground beyond it" is the property actually under test.
        let mut heights = vec![0.0f32; 64];
        for y in 0..8 {
            heights[y * 8 + 1] = 5.0;
            heights[y * 8 + 2] = 5.0;
        }
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), heights);
        // Sun toward -x (low): the march steps toward -x, so texels with x>1
        // see the ridge as an occluder → shadowed; texels with x<=1 face the
        // sun and stay lit.
        let sun = Vec3::new(-1.0, 0.5, 0.0).normalize();
        let bytes = field.bake_visibility_cache(sun, 0.0046, 8);
        // Texel (3, 0) → world (3, 0): beyond the ridge → shadowed.
        assert_eq!(bytes[3], 0, "texel beyond the ridge is shadowed");
        // Texel (0, 0) → world (0, 0): sun-facing side → lit.
        assert_eq!(bytes[0], 255, "sun-facing texel is lit");
    }

    /// The cache may be baked at a coarser resolution than the heightfield
    /// (the wasm cap): each cache texel still maps to the right world position
    /// and marches against the full-resolution field, so a ridge shadow shows
    /// up at the downscaled rate too.
    #[test]
    fn bake_cache_coarser_resolution_still_shadows() {
        // Two-texel ridge, for the reason spelled out in `bake_cache_ridge_casts_shadow`.
        let mut heights = vec![0.0f32; 64];
        for y in 0..8 {
            heights[y * 8 + 1] = 5.0;
            heights[y * 8 + 2] = 5.0;
        }
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), heights);
        let sun = Vec3::new(-1.0, 0.5, 0.0).normalize();
        // Bake at 4² (half the heightfield resolution).
        let bytes = field.bake_visibility_cache(sun, 0.0046, 4);
        assert_eq!(bytes.len(), 16);
        // Cache texel (3, 0) → world (7, 0): beyond the ridge → shadowed.
        assert_eq!(bytes[3], 0, "coarse-cache texel beyond the ridge is shadowed");
        // Cache texel (0, 0) → world (0, 0): sun-facing → lit.
        assert_eq!(bytes[0], 255, "coarse-cache sun-facing texel is lit");
    }
}
