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
        for _ in 0..96 {
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
        (Without<HorizonMap>, Without<HorizonBakeTask>),
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
    if let Some(mesh) = meshes.get_mut(&mesh3d.0) {
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
            let csm_far = if light.shadows_enabled {
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
/// texture, static `engine2` (size/resolution), and the per-frame sun
/// direction in `engine`. A terrain with no authored shader gets the
/// default `terrain_shadow.wgsl` (albedo from its `displayColor`).
/// Idempotent and self-healing against later material swaps; steady-state
/// cost is a uniform compare per terrain (writes only when the sun moves).
#[allow(clippy::type_complexity)]
pub fn wire_terrain_materials(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
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
    terrains: Query<(
        Entity,
        &GlobalTransform,
        &HorizonMap,
        Option<&MeshMaterial3d<ShaderMaterial>>,
        Option<&MeshMaterial3d<StandardMaterial>>,
    )>,
) {
    let Some(mut shader_mats) = shader_mats else { return };
    let Some((sun_gt, tan_r, csm_far)) = pick_sun(&sun) else { return };
    let to_sun_world: Vec3 = sun_gt.back().into();

    for (entity, terrain_gt, map, shader_mat, std_mat) in &terrains {
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

        // Named engine uniforms consumed by the terrain shaders (regolith /
        // terrain_shadow declare these in their `Material` struct; the engine
        // packs them at the reflected offsets).
        let sun_dir = ParamValue::Vec3([engine.x, engine.y, engine.z]);
        let hf_size = ParamValue::Vec2([engine2.x, engine2.y]);
        let write_engine = |m: &mut ShaderMaterial| {
            m.height_map = Some(map.image.clone());
            m.set("sun_dir", sun_dir);
            m.set_scalar("sun_tan_radius", tan_r);
            m.set("hf_size", hf_size);
            m.set_scalar("hf_res", engine2.z);
            m.set_scalar("csm_far", csm_far);
        };

        if let Some(handle) = shader_mat {
            // Compare before `get_mut` — a blind `get_mut` re-uploads the
            // asset every frame. Sun direction + heightfield identity + csm
            // bound cover everything that changes.
            let needs = shader_mats.get(&handle.0).is_some_and(|m| {
                m.height_map.as_ref() != Some(&map.image)
                    || m.get_vec4("sun_dir")
                        .map_or(true, |v| (v.truncate() - Vec3::new(engine.x, engine.y, engine.z)).length() > 1e-4)
                    || m.get_scalar("csm_far").map_or(true, |c| (c - csm_far).abs() > 1e-3)
            });
            if needs {
                if let Some(m) = shader_mats.get_mut(&handle.0) {
                    write_engine(m);
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
    sun: Query<
        (
            &GlobalTransform,
            &DirectionalLight,
            Option<&SunAngularDiameter>,
            Option<&CascadeShadowConfig>,
        ),
        Without<RenderLayers>,
    >,
    terrains: Query<(&GlobalTransform, &HorizonMap)>,
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
    let sun_moved = match *last_sun {
        Some(prev) => prev.dot(to_sun_world) <= SUN_EPSILON_COS,
        None => true,
    };
    if sun_moved {
        *last_sun = Some(to_sun_world);
    }

    for (entity, gt, has_layers, shadowed, has_nsc, shader_mat, std_mat, shade, name) in
        &mut entities
    {
        if !sun_moved && !gt.is_changed() {
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
        for (terrain_gt, map) in &terrains {
            let inv = terrain_gt.affine().inverse();
            let local = inv.transform_point3(gt.translation());
            let sun_local = inv.transform_vector3(to_sun_world).normalize_or_zero();
            if let Some(v) =
                map.field.sun_visibility(Vec2::new(local.x, local.z), sun_local, tan_r)
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
                if let Some(m) = mats.get_mut(&handle.0) {
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
                                HorizonShade { original, last_vis: q },
                            ));
                        }
                    }
                }
                Some(mut state) => {
                    if (state.last_vis - q).abs() > 1e-3 {
                        if let Some(m) = std_mats.get_mut(&handle.0) {
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
        app.add_systems(
            Update,
            (
                start_horizon_bakes,
                finish_horizon_bakes,
                wire_terrain_materials,
                shade_dynamic_entities,
            )
                .chain(),
        );
    }
}
