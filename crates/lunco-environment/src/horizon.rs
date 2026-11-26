//! Heightfield-ray-marched terrain sun shadows — the long-range half of the
//! two-system shadow design.
//!
//! Native cascades render mesh and object shadows near the viewer. The shared
//! heightfield march supplies distant terrain self-shadow, with penumbra width
//! derived from the UsdLux light's `inputs:angle`. Native workers cache angular
//! samples in an R8 visibility image, keeping the last completed presentation
//! sample resident while a replacement is prepared. Physical solar queries use
//! their independent f64 oracle.
//!
//! ## What lives here, and what does not
//!
//! **This module is render-free**: the heightfield, the bakes, the
//! sun-visibility cache and the R32Float/R8Unorm `Image` uploads are all
//! `bevy_mesh` / `bevy_image` / `bevy_light` — no `bevy_pbr`, no wgpu. It runs
//! headless.
//!
//! The half that writes those textures and uniforms INTO a concrete material —
//! `wire_terrain_materials` — lives in
//! **`lunco-render-bevy::horizon_shade`**. It is a per-frame *uniform feed*,
//! not appearance intent, so `PbrLook`/`ShaderLook` cannot express it; the
//! binder crate (the only one allowed to name a material) hosts it instead —
//! the same render-boundary split used by terrain map source reconciliation. See
//! `docs/architecture/render-decoupling.md`.
//!
//! ## Pipeline
//!
//! 1. **Bake** (once per terrain, async, ~100 ms): rasterize the terrain
//!    `Mesh3d` into a heightmap over its local XZ bounds. The heightmap is
//!    geometry, not lighting — it never needs re-baking. Terrains opt in
//!    via the [`HorizonShadowTerrain`] marker (USD:
//!    `custom bool lunco:terrain:horizonShadows`).
//! 2. **Material wiring** (idempotent, RENDER-SIDE): the heightfield
//!    (R32Float texture) and sun uniforms are written into an already-bound
//!    terrain `ShaderMaterial`. Its appearance comes from the standard USD
//!    `UsdShade` network or an explicit command-owned look; missing material
//!    intent remains unbound. The mesh gets planar UVs (done here, in
//!    [`install_horizon_map`]) so shaders can address the heightfield.
//!
//!    The terrain stays a **CSM caster**: within the sun's cascade range the
//!    shadow map renders the actual mesh, giving mesh-accurate self-shadow
//!    and contact shadows. The horizon takes over within the final tenth of forward cascade depth
//!    (`csm_far` carries the CSM far bound), so its heightfield-texel-quantized
//!    edges never show up close and near pixels skip the march entirely.
//! 2b. **Shadow cache** (sun-driven, off-thread on native): the per-pixel
//!     march is expensive — the configured number of steps × the configured cache
//!     supersample count per fragment. Because the sun moves slowly (a lunar day ≈ 29.5 Earth
//!     days), the march result is cached into an `R8Unorm` visibility
//!     texture ([`HorizonShadowCache`]) baked from the SAME
//!     [`HeightField::sun_visibility`] algorithm, refreshed only when the
//!     sun's terrain-local direction rotates past
//!     [`HorizonShadowCacheConfig::sun_threshold_deg`]. The fragment shader
//!     then does a single `textureSampleLevel` (guarded by the
//!     `shadow_cache_on` uniform) instead of the loop — dropping fragment
//!     cost to near zero. See [`start_shadow_cache_bake`].
//! 3. **Object shadows**: Bevy's native directional-light shadow pass handles
//!    all dynamic PBR objects. Streamed terrain tiles are CSM receivers, so the
//!    same Sun shadow map carries rover, rock, and equipment shadows onto the
//!    ground; the DEM horizon cache remains the terrain self-shadow owner.
//!
//! ## Limits
//!
//! - Heights are sampled at the entity's ground cell; a hovering object
//!   uses the ground-level result under it.
//! - The requested bake resolution and sampling quality are used on every
//!   target; wasm performs the explicit work inline because it has no worker
//!   thread pool.

use std::collections::HashMap;
use std::sync::Arc;

use crate::SunRenderState;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::image::ImageSampler;
use bevy::light::CascadeShadowConfig;
use bevy::math::Affine3A;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_core::{HorizonShadowTerrain, SunAngularDiameter};
// The POD texture descriptors `bevy_image` itself takes from `wgpu-types`.
// `bevy::render::render_resource` only RE-EXPORTS them, and importing THAT would
// drag wgpu + naga into this crate. `wgpu-types` is not `wgpu`.
use lunco_render::data_texture::{DataTexturePatch, DataTextureTexels, DataTextureUpdates};
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

/// Tan of the sun's angular radius for a diameter given in degrees.
fn tan_sun_radius(diameter_deg: f32) -> f32 {
    // Angular projection is the explicit GPU boundary; f64 keeps the 180-degree
    // schema endpoint on the positive side of the tangent singularity.
    ((f64::from(diameter_deg).to_radians() * 0.5).tan()) as f32
}

/// Resolve the standard UsdLux angular-size fallback without turning an
/// explicitly malformed component into an omitted attribute. Zero is valid:
/// it represents a point-like distant source and the march's texel floor keeps
/// that hard-shadow case finite.
fn sun_diameter_deg(angle: Option<&SunAngularDiameter>) -> Option<f32> {
    match angle {
        None => Some(lunco_core::SOLAR_ANGULAR_DIAMETER_DEG),
        Some(SunAngularDiameter(diameter))
            if diameter.is_finite() && (0.0..=180.0).contains(diameter) =>
        {
            Some(*diameter)
        }
        Some(_) => None,
    }
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
        Self {
            resolution,
            min,
            size: size.max(Vec2::splat(f32::EPSILON)),
            heights,
        }
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

    fn sun_visibility(&self, local_xz: Vec2, sun: &HorizonSun, march: Vec4) -> Option<f32> {
        let p0 = local_xz - self.min;
        if p0.min_element() < 0.0 || p0.x > self.size.x || p0.y > self.size.y {
            return None;
        }
        if sun.upper <= 0.0 || sun.horizontal == Vec2::ZERO {
            return Some(sun.visible_above(0.0));
        }
        let to_grid = (self.resolution - 1) as f32 / self.size;
        let receiver = self.height_at(p0 * to_grid) + march.y;
        let texel = self.size.min_element() / (self.resolution - 1) as f32;
        let mut distance = texel;
        let mut skyline = 0.0_f32;
        for _ in 0..march.x as usize {
            let p = p0 + sun.horizontal * distance;
            if p.min_element() < 0.0 || p.x > self.size.x || p.y > self.size.y {
                break;
            }
            skyline = skyline.max((self.height_at(p * to_grid) - receiver) / distance);
            if skyline >= sun.upper {
                return Some(0.0);
            }
            distance = distance * march.z + texel * march.w;
        }
        Some(sun.visible_above(skyline))
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
    /// (0..255 for the terrain transmission of the above-horizon disc) over the
    /// heightfield footprint, for the given terrain-local sun direction.
    ///
    /// This is the CPU side of the **horizon shadow cache**: the result is
    /// uploaded to an `R8Unorm` texture the terrain fragment shader samples
    /// with a single `textureSampleLevel` instead of running the configured
    /// horizon march per pixel. `target_res` may differ from the heightfield's
    /// own resolution (a coarser cache on memory-constrained targets); each
    /// cache texel maps to `min + (frac) * size` and marches against the
    /// full-resolution heightfield. Cache resolution controls the distance between
    /// stored samples.
    ///
    /// Uses the same skyline and solar-disc model as the live GPU march. Off-thread on
    /// native (`AsyncComputeTaskPool`); inline on wasm (see
    /// [`start_shadow_cache_bake`]).
    ///
    /// [`sun_visibility`]: HeightField::sun_visibility
    pub fn bake_visibility_cache(
        &self,
        sun_local: Vec3,
        tan_sun_r: f32,
        target_res: u32,
        march_steps: usize,
        samples_per_axis: usize,
    ) -> Vec<u8> {
        if target_res < 2 || march_steps == 0 || samples_per_axis == 0 {
            return Vec::new();
        }
        let res = target_res;
        let mut bytes = vec![0u8; (res as usize) * (res as usize)];
        let inv = 1.0 / (res - 1) as f32;
        // The configured grid of sub-texel rays prevents a single sample from
        // aliasing an elongated shadow edge. Native bakes run off-thread; wasm
        // pays the explicitly requested cost on the main thread.
        let sample_count = samples_per_axis.saturating_mul(samples_per_axis);
        if sample_count == 0 {
            return Vec::new();
        }
        let max_g = (res - 1) as f32;
        let sun = HorizonSun::new(sun_local, tan_sun_r);
        let open = sun.visible_above(0.0);
        if open <= 0.0 {
            return bytes;
        }
        let march = horizon_march_parameters(march_steps);
        for y in 0..res {
            for x in 0..res {
                let mut vis = 0.0;
                for sy in 0..samples_per_axis {
                    for sx in 0..samples_per_axis {
                        let offset = Vec2::new(
                            (sx as f32 + 0.5) / samples_per_axis as f32 - 0.5,
                            (sy as f32 + 0.5) / samples_per_axis as f32 - 0.5,
                        );
                        // Clamp the subsample back into the grid. This avoids inventing
                        // fully-lit terrain beyond the border: a below-horizon sun must
                        // not bake a lit rim, and a ridge shadow must not leak at the edge.
                        let g = (Vec2::new(x as f32, y as f32) + offset)
                            .clamp(Vec2::ZERO, Vec2::splat(max_g));
                        let local_xz = self.min + g * inv * self.size;
                        let Some(visibility) = self.sun_visibility(local_xz, &sun, march) else {
                            return Vec::new();
                        };
                        vis += visibility;
                    }
                }
                bytes[(y as usize) * (res as usize) + (x as usize)] =
                    (vis / (sample_count as f32 * open) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;
            }
        }
        bytes
    }
}

/// Site march contract shared with the terrain shader: step count, receiver
/// lift in metres, step growth, and additive advance in heightfield texels.
pub fn horizon_march_parameters(steps: usize) -> Vec4 {
    Vec4::new(steps as f32, 0.35, 1.18, 0.5)
}

struct HorizonSun {
    horizontal: Vec2,
    elevation: f32,
    radius: f32,
    upper: f32,
    lower: f32,
}

impl HorizonSun {
    fn new(direction: Vec3, tan_radius: f32) -> Self {
        let horizontal = Vec2::new(direction.x, direction.z);
        let elevation = direction.y.atan2(horizontal.length());
        let radius = tan_radius.atan();
        let slope = |angle: f32| {
            if angle >= std::f32::consts::FRAC_PI_2 {
                f32::INFINITY
            } else if angle <= -std::f32::consts::FRAC_PI_2 {
                f32::NEG_INFINITY
            } else {
                angle.tan()
            }
        };
        Self {
            horizontal: horizontal.normalize_or_zero(),
            elevation,
            radius,
            upper: slope(elevation + radius),
            lower: slope(elevation - radius),
        }
    }
    fn visible_above(&self, skyline: f32) -> f32 {
        if skyline >= self.upper {
            return 0.0;
        }
        if skyline <= self.lower {
            return 1.0;
        }
        let x = ((self.elevation - skyline.atan()) / self.radius).clamp(-1.0, 1.0);
        ((-x).acos() + x * (1.0 - x * x).max(0.0).sqrt()) / std::f32::consts::PI
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
/// The cache replaces the per-pixel configured heightfield ray-march in the
/// terrain fragment shader with a single `textureSampleLevel` of a pre-baked
/// `R8Unorm` visibility texture, refreshed only when the sun's terrain-local
/// direction moves past [`sun_threshold_deg`](Self::sun_threshold_deg). Tune
/// live through the Graphics settings.
#[derive(Resource, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct HorizonShadowCacheConfig {
    /// Master switch. `false` → the fragment shader keeps ray-marching per
    /// pixel. `true` → committed cache images are baked and sampled instead.
    pub enabled: bool,
    /// Re-bake the cache when the sun's terrain-local direction has rotated
    /// more than this many degrees from the direction it was last baked at.
    /// Small → crisper shadows during a moving sun, more frequent bakes;
    /// large → cheaper, shadows lag slightly behind the sun. A lunar day is
    /// ~29.5 Earth days (~0.5°/h), so 0.05° re-bakes roughly every 6 minutes
    /// of real time at 1× — and far less often when the sun is static.
    pub sun_threshold_deg: f32,
    /// Maximum iterations of the live CPU/GPU horizon ray march.
    pub march_steps: usize,
    /// Cache bake supersamples per axis.
    pub samples_per_axis: usize,
    /// Minimum wall-clock interval between replacement requests for one
    /// terrain cache. A committed cache remains usable while the replacement
    /// is pending, so accelerated CelestialTime cannot turn every completed
    /// bake into another immediate CPU/GPU workload.
    pub min_refresh_interval_secs: f32,
}

impl Default for HorizonShadowCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sun_threshold_deg: 0.05,
            march_steps: 48,
            samples_per_axis: 2,
            min_refresh_interval_secs: 0.25,
        }
    }
}

impl HorizonShadowCacheConfig {
    /// Whether the cache quality inputs are usable without normalization.
    pub fn quality_is_valid(&self) -> bool {
        self.sun_threshold_deg.is_finite()
            && self.sun_threshold_deg > 0.0
            && self.sun_threshold_deg < 180.0
            && self.march_steps > 0
            && self.samples_per_axis > 0
            && self.min_refresh_interval_secs.is_finite()
            && self.min_refresh_interval_secs >= 0.0
    }
    /// Whether this configured cache has a meaningful terrain visibility sample
    /// for the given terrain-local sun. A fully set disc needs no terrain map.
    pub fn requires_sample(&self, sun_local: Vec3, tan_radius: f32) -> bool {
        self.enabled
            && self.quality_is_valid()
            && HorizonSun::new(sun_local, tan_radius).upper > 0.0
    }
}

/// The baked sun-visibility cache on a terrain entity: an `R8Unorm` texture
/// (0..1 visibility over the heightfield footprint) + the terrain-local sun
/// direction it was baked at. The fragment shader samples this with a single
/// `textureSampleLevel` (guarded by the `shadow_cache_on` uniform) instead of
/// the configured ray-march. Refreshed by [`start_shadow_cache_bake`] when the
/// sun moves past the configured threshold.
#[derive(Component)]
pub struct HorizonShadowCache {
    /// The GPU visibility texture (binding 10). Equal-size bakes update resident
    /// texels without an asset modification or material rebinding.
    pub image: Handle<Image>,
    /// Terrain-local to-sun direction the cache was baked for. The cache is
    /// scheduled for replacement when the Sun moves beyond `sun_threshold_deg`.
    last_sun_local: Vec3,
    /// Wall-clock timestamp at which the current replacement was requested.
    /// This is scheduling state, not a freshness claim; `last_sun_local` is
    /// the authoritative angular sample represented by the image.
    last_refresh: Instant,
    quality: ShadowCacheQuality,
}

#[derive(Clone, Copy, PartialEq)]
struct ShadowCacheQuality {
    tan_radius: f32,
    march_steps: usize,
    samples_per_axis: usize,
}

impl HorizonShadowCache {
    /// Whether the committed texture uses the requested shadow-cache quality.
    pub fn matches_quality(&self, config: &HorizonShadowCacheConfig, tan_radius: f32) -> bool {
        self.quality
            == ShadowCacheQuality {
                tan_radius,
                march_steps: config.march_steps,
                samples_per_axis: config.samples_per_axis,
            }
    }

    /// Terrain-local angular sample represented by the committed visibility image.
    pub fn sampled_sun_local(&self) -> Vec3 {
        self.last_sun_local
    }

    /// Whether the current Sun lies within the configured angular sample tolerance.
    ///
    /// The bake is an angular snapshot, not a generic terrain mask. The
    /// committed image remains the last completed presentation sample while a replacement
    /// is pending; this freshness decision belongs to the scheduler, not to
    /// material binding.
    pub fn is_valid_for_sun(&self, sun_local: Vec3, threshold_deg: f32) -> bool {
        if !threshold_deg.is_finite() || threshold_deg <= 0.0 || threshold_deg >= 180.0 {
            return false;
        }
        let sun_local = sun_local.normalize_or_zero();
        // Chord length retains small angular differences without cos(theta)
        // rounding to one at the rendering precision boundary.
        let chord = 2.0 * (threshold_deg.to_radians() * 0.5).sin();
        self.last_sun_local.distance_squared(sun_local) <= chord * chord
    }
}

/// Change-gated world-to-terrain sun projection shared by cache and material
/// consumers. BigSpace's finalized `GlobalTransform` and the render sun
/// revision are the only inputs that can invalidate a terrain-local direction;
/// stationary terrain and sun therefore do no per-frame reprojection.
#[derive(Default)]
pub struct TerrainSunProjectionCache {
    entries: HashMap<Entity, TerrainSunProjection>,
}

struct TerrainSunProjection {
    inverse: Affine3A,
    sun_world: Vec3,
    sun_local: Vec3,
    sun_revision: u64,
}

impl TerrainSunProjectionCache {
    /// Remove projection state for a terrain that left the world.
    pub fn remove(&mut self, entity: Entity) {
        self.entries.remove(&entity);
    }

    /// Project the render sun through a finalized terrain transform, reusing
    /// the previous result until one of the projection inputs changes.
    pub fn project_sun_local(
        &mut self,
        entity: Entity,
        terrain_gt: &Ref<GlobalTransform>,
        sun_world: Vec3,
        sun_revision: u64,
    ) -> Vec3 {
        let transform_changed = terrain_gt.is_changed();
        match self.entries.entry(entity) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let projection = entry.get_mut();
                if transform_changed {
                    projection.inverse = terrain_gt.affine().inverse();
                }
                if transform_changed
                    || projection.sun_revision != sun_revision
                    || projection.sun_world != sun_world
                {
                    projection.sun_world = sun_world;
                    projection.sun_local = projection
                        .inverse
                        .transform_vector3(sun_world)
                        .normalize_or_zero();
                    projection.sun_revision = sun_revision;
                }
                projection.sun_local
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let inverse = terrain_gt.affine().inverse();
                let sun_local = inverse.transform_vector3(sun_world).normalize_or_zero();
                entry
                    .insert(TerrainSunProjection {
                        inverse,
                        sun_world,
                        sun_local,
                        sun_revision,
                    })
                    .sun_local
            }
        }
    }
}

/// In-flight async visibility-cache bake for a terrain entity (native only).
#[derive(Component)]
pub struct ShadowCacheBakeTask(Task<ShadowCacheResult>);

struct ShadowCacheResult {
    bytes: Vec<u8>,
    resolution: u32,
    sun_local: Vec3,
    requested_at: Instant,
    source_image: Handle<Image>,
    quality: ShadowCacheQuality,
    millis: u128,
    #[cfg(not(target_arch = "wasm32"))]
    _permit: lunco_core_runtime::async_work::ExternalWorkPermit,
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
pub fn start_horizon_bakes(
    mut commands: Commands,
    #[cfg(not(target_arch = "wasm32"))] meshes: Res<Assets<Mesh>>,
    #[cfg(target_arch = "wasm32")] mut meshes: ResMut<Assets<Mesh>>,
    #[cfg(target_arch = "wasm32")] mut images: ResMut<Assets<Image>>,
    #[cfg(target_arch = "wasm32")] mut uploads: ResMut<DataTextureUpdates>,
    q: Query<
        (
            Entity,
            &HorizonShadowTerrain,
            &Mesh3d,
            Has<lunco_render::SharedMeshAsset>,
        ),
        // `Without<RenderLayers>` mirrors `pick_sun`: terrain spawned under the
        // RTT preview `scene_root` carries a RenderLayers and must NOT bake into
        // the main scene (ARC-1 — cross-scene contamination + wasted bake).
        (
            Without<HorizonMap>,
            Without<HorizonBakeTask>,
            Without<RenderLayers>,
        ),
    >,
) {
    for (entity, cfg, mesh3d, shared) in &q {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = shared;
        if cfg.resolution < 2 {
            warn!(
                "[horizon] terrain {entity:?} has invalid resolution {}; removing horizon-shadow opt-in",
                cfg.resolution
            );
            commands.entity(entity).remove::<HorizonShadowTerrain>();
            continue;
        }
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue;
        }; // not loaded yet
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
            // thread), so bake INLINE at the explicitly authored resolution.
            // This is a one-time load cost, never a per-frame stall.
            let resolution = cfg.resolution;
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
                shared,
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
            g.iter()
                .map(|v| v.x)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
            (g.iter().map(|v| v.x).fold(f32::MIN, f32::max).ceil() as usize).min(r - 1),
        );
        let (lo_y, hi_y) = (
            g.iter()
                .map(|v| v.y)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
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
    let floor = heights
        .iter()
        .copied()
        .filter(|h| h.is_finite())
        .fold(f32::MAX, f32::min);
    for h in &mut heights {
        if !h.is_finite() {
            *h = floor;
        }
    }

    BakeResult {
        field: HeightField {
            resolution,
            min,
            size,
            heights: Arc::new(heights),
        },
        millis: start.elapsed().as_millis(),
    }
}

/// Collects finished bakes: installs the `HorizonMap` (CPU field + R32Float
/// GPU texture) and gives the mesh planar UVs (how shaders address the
/// heightfield). A static terrain remains in the native directional-shadow
/// caster/receiver set: CSM owns the mesh-accurate near field and shadows cast
/// by dynamic objects, while the heightfield path takes over beyond the CSM
/// range. Material wiring happens render-side
/// (`lunco-render-bevy::horizon_shade::wire_terrain_materials`).
pub fn finish_horizon_bakes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<(
        Entity,
        &mut HorizonBakeTask,
        &Mesh3d,
        Has<lunco_render::SharedMeshAsset>,
    )>,
) {
    use bevy::tasks::futures_lite::future;
    for (entity, mut task, mesh3d, shared) in &mut q {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        install_horizon_map(
            &mut commands,
            &mut meshes,
            &mut images,
            entity,
            mesh3d,
            shared,
            result.field,
            result.millis,
        );
    }
}

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
    shared: bool,
    field: HeightField,
    millis: u128,
) {
    let private;
    let target = if shared {
        let Some(mesh) = meshes.get(&mesh3d.0).cloned() else {
            warn!(
                "[horizon] shared mesh for {entity:?} disappeared before UV installation; retiring its bake"
            );
            commands
                .entity(entity)
                .try_remove::<HorizonBakeTask>()
                .try_remove::<HorizonShadowTerrain>();
            return;
        };
        private = meshes.add(mesh);
        commands
            .entity(entity)
            .try_insert(Mesh3d(private.clone()))
            .try_remove::<lunco_render::SharedMeshAsset>();
        &private
    } else {
        &mesh3d.0
    };
    if let Some(mut mesh) = meshes.get_mut(target) {
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
    // This static mesh uses native CSM for the near field and dynamic-object
    // shadows. Remove a stale engine marker if this entity is reused after an
    // older horizon binding; the shader's csm_far handoff prevents the horizon
    // visibility term from double-darkening the same terrain pixels.
    commands
        .entity(entity)
        .try_remove::<bevy::light::NotShadowReceiver>();
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
        Extent3d {
            width: field.resolution,
            height: field.resolution,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::R32Float,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    ));

    debug!(
        "[horizon] heightfield installed for {entity:?}: {}² in {} ms — far-field \
         ray-march shadows active (near field stays on CSM)",
        field.resolution, millis
    );
    commands
        .entity(entity)
        .remove::<HorizonBakeTask>()
        .try_insert(HorizonMap { field, image });
}

// ─────────────────────────────────────────────────────────────────────────
// 2b. Shadow cache bake — pre-bake the ray-march into an R8Unorm texture
// ─────────────────────────────────────────────────────────────────────────

/// Builds (or replaces) the `R8Unorm` visibility texture from baked bytes.
/// Linear filtering so the fragment shader's `textureSampleLevel` bilinearly
/// interpolates the cache (the visibility is smooth across penumbra, so this
/// is visually lossless and avoids per-texel march edges).
fn make_shadow_cache_image(bytes: Vec<u8>, res: u32) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: res,
            height: res,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// Kicks off a visibility-cache (re)bake for every horizon terrain whose sun
/// has rotated past the configured threshold since its last bake — the
/// low-frequency update that makes the cache worthwhile (a lunar day is
/// ~29.5 Earth days, so at 0.05° the cache re-bakes roughly every 6 minutes
/// at 1×, and far less when the sun is static).
///
/// Debounced by `Without<ShadowCacheBakeTask>` and the configured wall-clock
/// interval: while a bake is in flight the sun may keep moving, but no second
/// bake starts. The committed cache stays bound as the presentation value and
/// keeps its image identity; once the task lands, the completed pixels replace
/// its resident texels in place. The next replacement is admitted only after the
/// minimum refresh interval.
///
/// Below-horizon Sun needs no bake: the shader evaluates its current local
/// direction before reading the cache and returns zero visibility. The cache
/// stays bound overnight; sunrise schedules a replacement through the same
/// angular threshold without changing the material workload.
// `Instant` is bevy's portable clock (see `bake_heightfield`) — the
// `disallowed_methods` hit is the documented native-only false positive.
#[allow(clippy::type_complexity, clippy::disallowed_methods)]
pub fn start_shadow_cache_bake(
    mut commands: Commands,
    #[cfg(target_arch = "wasm32")] mut images: ResMut<Assets<Image>>,
    #[cfg(target_arch = "wasm32")] mut uploads: ResMut<DataTextureUpdates>,
    cfg: Res<HorizonShadowCacheConfig>,
    #[cfg(not(target_arch = "wasm32"))] admission: Res<lunco_core_runtime::AsyncWorkAdmission>,
    sun: SunQuery,
    render_sun: Option<Res<SunRenderState>>,
    terrains: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            &HorizonMap,
            Option<&mut HorizonShadowCache>,
        ),
        (Without<RenderLayers>, Without<ShadowCacheBakeTask>),
    >,
    mut sun_projection_cache: Local<TerrainSunProjectionCache>,
    mut removed_maps: RemovedComponents<HorizonMap>,
) {
    for entity in removed_maps.read() {
        sun_projection_cache.remove(entity);
    }
    if !cfg.enabled {
        return;
    }
    if !cfg.quality_is_valid() {
        warn!(
            "[horizon] invalid cache quality settings: threshold_deg={}, march_steps={}, samples_per_axis={}; skipping cache bake",
            cfg.sun_threshold_deg, cfg.march_steps, cfg.samples_per_axis
        );
        return;
    }
    let Some((_, tan_r, _csm_far)) = pick_sun(&sun) else {
        return;
    };
    let Some(render_sun) = render_sun.as_deref() else {
        return;
    };
    let Some(to_sun_world) = render_sun.direction_to_sun_world else {
        return;
    };
    let quality = ShadowCacheQuality {
        tan_radius: tan_r,
        march_steps: cfg.march_steps,
        samples_per_axis: cfg.samples_per_axis,
    };
    let march_steps = cfg.march_steps;
    let samples_per_axis = cfg.samples_per_axis;
    let requested_at = Instant::now();

    for (entity, terrain_gt, map, mut cache) in terrains {
        if map.field.resolution() < 2 {
            warn!(
                "[horizon] terrain {entity:?} has an invalid heightfield resolution; skipping shadow-cache bake"
            );
            continue;
        }
        let sun_local = sun_projection_cache.project_sun_local(
            entity,
            &terrain_gt,
            to_sun_world,
            render_sun.revision,
        );
        // The shader resolves a fully set solar disc without a terrain sample.
        if !cfg.requires_sample(sun_local, tan_r) {
            continue;
        }
        // Within the threshold of the last bake → cache still valid.
        if let Some(c) = cache.as_deref() {
            if c.matches_quality(&cfg, tan_r)
                && c.is_valid_for_sun(sun_local, cfg.sun_threshold_deg)
            {
                continue;
            }
            if requested_at.duration_since(c.last_refresh).as_secs_f64()
                < f64::from(cfg.min_refresh_interval_secs)
            {
                continue;
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        let permit = {
            use lunco_core_runtime::{AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
            let key = AsyncWorkKey::new(
                AsyncWorkKind::VisualizationPreparation,
                0,
                entity.to_bits() as u128,
                render_sun.revision,
                u64::MAX - 3,
            );
            let Ok(permit) = admission.admit_external(AsyncWorkPriority::Background, key) else {
                continue;
            };
            permit
        };
        if let Some(c) = cache.as_deref_mut() {
            c.last_refresh = requested_at;
        }

        let target_res = map.field.resolution();

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Off-thread: clone the HeightField (cheap — `Arc<Vec<f32>>`)
            // and march every cache texel on a worker. The frame never blocks.
            let field = map.field.clone();
            let source_image = map.image.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                let start = Instant::now();
                let bytes = field.bake_visibility_cache(
                    sun_local,
                    tan_r,
                    target_res,
                    march_steps,
                    samples_per_axis,
                );
                ShadowCacheResult {
                    bytes,
                    resolution: target_res,
                    sun_local,
                    requested_at,
                    source_image,
                    quality,
                    millis: start.elapsed().as_millis(),
                    _permit: permit,
                }
            });
            commands.entity(entity).insert(ShadowCacheBakeTask(task));
        }
        #[cfg(target_arch = "wasm32")]
        {
            // Inline (no worker threads on wasm): a one-time cost per sun
            // threshold crossing at the explicitly requested cache resolution.
            let start = Instant::now();
            let bytes = map.field.bake_visibility_cache(
                sun_local,
                tan_r,
                target_res,
                march_steps,
                samples_per_axis,
            );
            let millis = start.elapsed().as_millis();
            install_shadow_cache(
                &mut commands,
                &mut images,
                &mut uploads,
                entity,
                cache.as_deref().map(|cache| cache.image.clone()),
                bytes,
                target_res,
                sun_local,
                requested_at,
                quality,
                millis,
            );
        }
    }
}

/// Collects finished off-thread visibility-cache bakes and installs them
/// (native). The committed cache keeps one stable image handle for the
/// terrain's lifetime; equal-size bakes queue pixels through DataTextureUpdates.
/// That leaves every tile material binding and shadow-ownership decision
/// unchanged while the renderer uploads the new pixels. While the bake ran the
/// old cache stayed bound, so the renderer never enters the expensive live
/// march merely because a replacement is pending.
pub fn finish_shadow_cache_bake(
    mut commands: Commands,
    cfg: Res<HorizonShadowCacheConfig>,
    sun: SunQuery,
    mut images: ResMut<Assets<Image>>,
    mut uploads: ResMut<DataTextureUpdates>,
    mut q: Query<(
        Entity,
        &mut ShadowCacheBakeTask,
        Option<&HorizonShadowCache>,
        Option<&HorizonMap>,
    )>,
) {
    use bevy::tasks::futures_lite::future;
    let quality = pick_sun(&sun).map(|(_, tan_radius, _)| ShadowCacheQuality {
        tan_radius,
        march_steps: cfg.march_steps,
        samples_per_axis: cfg.samples_per_axis,
    });
    for (entity, mut task, cache, map) in &mut q {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        // A completed visual product belongs to the immutable source it read.
        // Terrain replacement must never publish a result from the retired map.
        if map.is_none_or(|map| map.image != result.source_image) || quality != Some(result.quality)
        {
            commands.entity(entity).remove::<ShadowCacheBakeTask>();
            continue;
        }
        install_shadow_cache(
            &mut commands,
            &mut images,
            &mut uploads,
            entity,
            cache.map(|cache| cache.image.clone()),
            result.bytes,
            result.resolution,
            result.sun_local,
            result.requested_at,
            result.quality,
            result.millis,
        );
    }
}

fn retire_shadow_cache_uploads(
    event: On<Remove<HorizonShadowCache>>,
    caches: Query<&HorizonShadowCache>,
    mut uploads: ResMut<DataTextureUpdates>,
) {
    if let Ok(cache) = caches.get(event.entity) {
        uploads.retire(cache.image.id());
    }
}

/// Installs a finished visibility-cache bake on the terrain entity: uploads
/// the `R8Unorm` texture and inserts/replaces [`HorizonShadowCache`]. An
/// existing cache keeps its image handle and publishes the new pixels through
/// the same asset identity, so material bind groups and tile ownership do not
/// churn when a replacement completes. Shared by the native async finish and
/// the wasm inline bake so the two paths cannot drift.
fn install_shadow_cache(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    uploads: &mut DataTextureUpdates,
    entity: Entity,
    existing_image: Option<Handle<Image>>,
    bytes: Vec<u8>,
    resolution: u32,
    sun_local: Vec3,
    requested_at: Instant,
    quality: ShadowCacheQuality,
    millis: u128,
) {
    let image_handle = if let Some(existing_image) = existing_image {
        let image = images
            .get_mut_untracked(existing_image.id())
            .expect("committed horizon cache image handle must remain valid");
        if image.texture_descriptor.size.width == resolution
            && image.texture_descriptor.size.height == resolution
        {
            // Keep the CPU recovery copy and GPU texture on the same revision.
            // Only the typed upload owner touches resident GPU texels; no asset
            // Modified event invalidates materials or allocates another texture.
            image.data = Some(bytes.clone());
            let sequence = uploads.reserve_sequence();
            uploads.submit(DataTexturePatch {
                image: existing_image.clone(),
                sequence,
                size: UVec2::splat(resolution),
                texels: DataTextureTexels::R8Unorm(bytes),
            });
        } else {
            uploads.retire(existing_image.id());
            images
                .insert(
                    existing_image.id(),
                    make_shadow_cache_image(bytes, resolution),
                )
                .expect("committed horizon cache image handle must remain valid");
        }
        existing_image
    } else {
        images.add(make_shadow_cache_image(bytes, resolution))
    };
    debug!("[horizon] shadow cache baked for {entity:?}: {resolution}² in {millis} ms");
    commands
        .entity(entity)
        .remove::<ShadowCacheBakeTask>()
        .try_insert(HorizonShadowCache {
            image: image_handle,
            last_sun_local: sun_local,
            last_refresh: requested_at,
            quality,
        });
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
/// The filters are STRUCTURAL, not a guess about brightness:
///
/// * `Without<Earthshine>` — a body's reflected fill is authored as a
///   `DistantLight` CHILD of the body prim it comes from
///   (`lunco://lighting/earthshine.usda` under the Earth prim), and
///   `lunco-usd-sim` stamps [`crate::Earthshine`] from exactly that namespace
///   nesting. The scene's key light is the top-level `DistantLight`. That is a
///   fact about where each prim sits in the stage, so the query states it.
/// * `Without<RenderLayers>` — preview-viewport suns are scoped to their own
///   layer and are not the scene's sun.
///
/// All components are render-FREE: `DirectionalLight` / `CascadeShadowConfig`
/// are `bevy_light`, `RenderLayers` is `bevy_camera`.
pub type SunQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static DirectionalLight,
        Option<&'static SunAngularDiameter>,
        Option<&'static CascadeShadowConfig>,
    ),
    (Without<RenderLayers>, Without<crate::Earthshine>),
>;

/// Resolve the unique scene Sun used by the horizon cache, sky and material lighting.
/// Return its entity, tangent of its angular radius, and farthest native shadow depth
/// in metres. A zero depth gives the horizon renderer the entire view.
pub fn pick_sun(sun: &SunQuery) -> Option<(Entity, f32, f32)> {
    let mut it = sun.iter();
    let first = it.next()?;
    if it.next().is_some() {
        warn_once!(
            "[environment] {} scene suns: more than one authored `DistantLight` is lit and \
             unscoped, so which one lights the scene is ambiguous. Author ONE sun \
             (see `lunco://lighting/sun.usda`); the others should be scoped to a \
             `RenderLayers` preview or removed.",
            sun.iter().count()
        );
        return None;
    }
    let (entity, light, ang, csm) = first;
    let csm_far = if light.shadow_maps_enabled {
        csm.and_then(|c| c.bounds.last().copied()).unwrap_or(0.0)
    } else {
        0.0
    };
    let Some(diameter_deg) = sun_diameter_deg(ang) else {
        warn_once!(
            "[environment] authored `SunAngularDiameter` is invalid; refusing the sun until it is finite and in [0, 180] degrees"
        );
        return None;
    };
    Some((entity, tan_sun_radius(diameter_deg), csm_far))
}

// ─────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────

/// Registers the heightfield-shadow **bake** pipeline (render-free). Added by
/// [`EnvironmentPlugin`](crate::EnvironmentPlugin) — binaries need no extra
/// wiring; terrains opt in via the [`HorizonShadowTerrain`] marker (stamped by
/// the USD loader).
///
/// The material half (`wire_terrain_materials`) is added by
/// `lunco_render_bevy::LuncoRenderPlugin`. Cache completion remains in Update;
/// material and terrain-local frame projection consume the completed cache and
/// finalized BigSpace render transforms in PostUpdate. Headless simply never
/// adds the material system: heightfields and caches are still baked (they are
/// simulation-visible data — a shadow query, a solar-power model), they just
/// never reach a GPU material.
pub struct HorizonShadowPlugin;

impl Plugin for HorizonShadowPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
            app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
        }
        app.register_type::<HorizonShadowTerrain>();
        app.register_type::<HorizonShadowCacheConfig>();
        app.init_resource::<HorizonShadowCacheConfig>();
        app.init_resource::<DataTextureUpdates>();
        app.add_observer(retire_shadow_cache_uploads);
        // Every system here needs the asset stores (`Assets<Mesh>`/`Assets<Image>`).
        // Gate the chain on those existing so an app with no AssetPlugin (cosim
        // integration tests) cleanly skips it instead of panicking on a missing
        // `ResMut<Assets<…>>`. The real app always has them, so this is a no-op there.
        app.add_systems(
            Update,
            (
                start_horizon_bakes,
                finish_horizon_bakes,
                finish_shadow_cache_bake,
            )
                .chain()
                .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
        );
        // The cache's sun direction is terrain-local, so its decision must use
        // the same finalized floating-origin transform that the renderer sees.
        // BigSpace updates GlobalTransform in PostUpdate; starting this bake in
        // Update would combine the current sun with the previous render frame.
        app.add_systems(
            PostUpdate,
            start_shadow_cache_bake
                .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision)
                .after(crate::finalize_sun_render_state)
                .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource)]
    struct HorizonInstallTarget(Entity);

    fn install_horizon_fixture(
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut images: ResMut<Assets<Image>>,
        target: Res<HorizonInstallTarget>,
        mesh_query: Query<(&Mesh3d, Has<lunco_render::SharedMeshAsset>)>,
    ) {
        let entity = target.0;
        let (mesh, shared) = mesh_query
            .get(entity)
            .expect("the horizon fixture must keep its mesh");
        install_horizon_map(
            &mut commands,
            &mut meshes,
            &mut images,
            entity,
            mesh,
            shared,
            make_field(2, Vec2::ZERO, Vec2::ONE, vec![0.0; 4]),
            0,
        );
        commands.remove_resource::<HorizonInstallTarget>();
    }

    fn make_field(res: u32, min: Vec2, size: Vec2, heights: Vec<f32>) -> HeightField {
        assert_eq!(heights.len(), (res * res) as usize);
        HeightField {
            resolution: res,
            min,
            size,
            heights: Arc::new(heights),
        }
    }

    #[test]
    fn sun_angle_uses_schema_default_only_when_omitted() {
        assert_eq!(
            sun_diameter_deg(None),
            Some(lunco_core::SOLAR_ANGULAR_DIAMETER_DEG)
        );
        assert_eq!(sun_diameter_deg(Some(&SunAngularDiameter(0.0))), Some(0.0));
        assert_eq!(
            sun_diameter_deg(Some(&SunAngularDiameter(0.53))),
            Some(0.53)
        );
        assert_eq!(sun_diameter_deg(Some(&SunAngularDiameter(-1.0))), None);
        assert_eq!(sun_diameter_deg(Some(&SunAngularDiameter(f32::NAN))), None);
        assert_eq!(sun_diameter_deg(Some(&SunAngularDiameter(181.0))), None);
    }

    #[test]
    fn horizon_install_preserves_shared_meshes_and_native_shadow_reception() {
        for shared in [false, true] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .add_plugins(bevy::asset::AssetPlugin::default())
                .init_asset::<Image>()
                .init_asset::<Mesh>();

            let mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[9.0, 9.0]; 3])
            .with_inserted_indices(Indices::U32(vec![0, 1, 2]));
            let mesh_handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
            let entity = app
                .world_mut()
                .spawn((Mesh3d(mesh_handle.clone()), bevy::light::NotShadowReceiver))
                .id();
            if shared {
                app.world_mut()
                    .entity_mut(entity)
                    .insert(lunco_render::SharedMeshAsset);
            }
            app.insert_resource(HorizonInstallTarget(entity));
            app.add_systems(Update, install_horizon_fixture);

            app.update();

            assert!(app.world().entity(entity).contains::<HorizonMap>());
            assert!(
                !app.world()
                    .entity(entity)
                    .contains::<bevy::light::NotShadowReceiver>(),
                "static horizon terrain must receive native CSM and dynamic-object shadows"
            );
            let installed = &app.world().entity(entity).get::<Mesh3d>().unwrap().0;
            let meshes = app.world().resource::<Assets<Mesh>>();
            assert_eq!(
                meshes
                    .get(installed)
                    .unwrap()
                    .attribute(Mesh::ATTRIBUTE_UV_0),
                Some(&VertexAttributeValues::Float32x2(vec![
                    [0.0, 0.0],
                    [1.0, 0.0],
                    [0.0, 1.0]
                ]))
            );
            if shared {
                assert_ne!(installed.id(), mesh_handle.id());
                assert!(
                    !app.world()
                        .entity(entity)
                        .contains::<lunco_render::SharedMeshAsset>()
                );
                assert_eq!(
                    meshes
                        .get(&mesh_handle)
                        .unwrap()
                        .attribute(Mesh::ATTRIBUTE_UV_0),
                    Some(&VertexAttributeValues::Float32x2(vec![[9.0, 9.0]; 3]))
                );
            } else {
                assert_eq!(installed.id(), mesh_handle.id());
            }
        }
    }

    #[test]
    fn bake_cache_zenith_all_lit() {
        let field = make_field(4, Vec2::ZERO, Vec2::splat(3.0), vec![0.0; 16]);
        let bytes = field.bake_visibility_cache(Vec3::new(0.0, 1.0, 0.0), 0.0046, 4, 48, 2);
        assert!(bytes.iter().all(|&b| b == 255), "zenith sun → all 255");
    }

    #[test]
    fn bake_cache_below_horizon_all_shadow() {
        let field = make_field(4, Vec2::ZERO, Vec2::splat(3.0), vec![0.0; 16]);
        let bytes = field.bake_visibility_cache(Vec3::new(0.0, -1.0, 0.0), 0.0046, 4, 48, 2);
        assert!(bytes.iter().all(|&b| b == 0), "below-horizon sun → all 0");
    }

    #[test]
    fn sunrise_resolves_the_disc_and_point_source_limit() {
        let field = make_field(4, Vec2::ZERO, Vec2::splat(3.0), vec![0.0; 16]);
        for (elevation, radius, expected) in
            [(0.0, 0.0046, 128u8), (0.0, 0.0, 0), (0.00001, 0.0, 255)]
        {
            let bytes =
                field.bake_visibility_cache(Vec3::new(1.0, elevation, 0.0), radius, 4, 48, 2);
            let open = HorizonSun::new(Vec3::new(1.0, elevation, 0.0), radius).visible_above(0.0);
            assert!(
                bytes
                    .iter()
                    .all(|&value| (f32::from(value) * open).round() as u8 == expected)
            );
        }
    }

    #[test]
    fn bake_cache_flat_terrain_all_lit() {
        let field = make_field(8, Vec2::ZERO, Vec2::splat(7.0), vec![1.5; 64]);
        let sun = Vec3::new(1.0, 0.3, 0.2).normalize();
        let bytes = field.bake_visibility_cache(sun, 0.0046, 8, 48, 2);
        assert!(bytes.iter().all(|&b| b == 255), "flat terrain → all lit");
    }

    #[test]
    fn shadow_cache_reports_angular_freshness_for_scheduling() {
        let cache = HorizonShadowCache {
            image: Handle::default(),
            last_sun_local: Vec3::Y,
            last_refresh: Instant::now(),
            quality: ShadowCacheQuality {
                tan_radius: 0.0046,
                march_steps: 48,
                samples_per_axis: 2,
            },
        };

        assert!(cache.is_valid_for_sun(Vec3::Y, 0.05));
        assert!(cache.is_valid_for_sun(
            Quat::from_rotation_z(0.0005_f32.to_radians()) * Vec3::Y,
            0.05
        ));
        assert!(!cache.is_valid_for_sun(Vec3::NEG_Y, 0.05));
        assert!(!cache.is_valid_for_sun(
            Quat::from_rotation_z(0.0002_f32.to_radians()) * Vec3::Y,
            0.0001
        ));
        assert_eq!(tan_sun_radius(0.0), 0.0);
        assert!(tan_sun_radius(180.0).is_finite() && tan_sun_radius(180.0) > 0.0);
    }

    #[test]
    fn invalid_cache_quality_is_rejected_without_normalization() {
        let mut cfg = HorizonShadowCacheConfig::default();
        assert!(cfg.quality_is_valid());

        cfg.march_steps = 0;
        assert!(!cfg.quality_is_valid());
        cfg.march_steps = 48;
        cfg.sun_threshold_deg = f32::NAN;
        assert!(!cfg.quality_is_valid());
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
        let bytes = field.bake_visibility_cache(sun, 0.0046, 8, 48, 2);
        // Texel (3, 0) → world (3, 0): beyond the ridge → shadowed.
        assert_eq!(bytes[3], 0, "texel beyond the ridge is shadowed");
        // Texel (0, 0) → world (0, 0): sun-facing side → lit.
        assert_eq!(bytes[0], 255, "sun-facing texel is lit");
    }

    /// The cache may be baked at a coarser resolution than the heightfield:
    /// each cache texel still maps to the right world position and marches
    /// against the full-resolution field, so a ridge shadow shows up at the
    /// requested sampling rate too.
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
        let bytes = field.bake_visibility_cache(sun, 0.0046, 4, 48, 2);
        assert_eq!(bytes.len(), 16);
        // Cache texel (3, 0) → world (7, 0): beyond the ridge → shadowed.
        assert_eq!(
            bytes[3], 0,
            "coarse-cache texel beyond the ridge is shadowed"
        );
        // Cache texel (0, 0) → world (0, 0): sun-facing → lit.
        assert_eq!(bytes[0], 255, "coarse-cache sun-facing texel is lit");
    }

    #[test]
    fn bake_cache_rejects_resolution_below_two() {
        let field = make_field(2, Vec2::ZERO, Vec2::splat(1.0), vec![0.0; 4]);
        assert!(
            field
                .bake_visibility_cache(Vec3::Y, 0.0046, 0, 48, 2)
                .is_empty()
        );
        assert!(
            field
                .bake_visibility_cache(Vec3::Y, 0.0046, 1, 48, 2)
                .is_empty()
        );
        assert!(
            field
                .bake_visibility_cache(Vec3::Y, 0.0046, 2, 0, 2)
                .is_empty()
        );
        assert!(
            field
                .bake_visibility_cache(Vec3::Y, 0.0046, 2, 48, 0)
                .is_empty()
        );
    }
}
