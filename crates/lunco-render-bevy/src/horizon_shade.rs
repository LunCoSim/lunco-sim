//! The render half of the heightfield sun-shadow pipeline — **moved here from
//! `lunco-environment::horizon`** (2026-07-13).
//!
//! `lunco-environment` bakes the heightfield, the R32Float height texture and the
//! R8Unorm sun-visibility cache. All of that is render-free and runs headless.
//! What could NOT stay there is this: feeding those textures and the per-frame sun
//! uniforms INTO a concrete material.
//!
//! It is deliberately not expressed as `PbrLook`/`ShaderLook` intent. This is a
//! **per-frame uniform feed** — `ShaderMaterial::set_many`, `height_map`,
//! `shadow_cache`, and a `StandardMaterial::base_color` scale on glb props (cloned
//! to a unique handle so shared materials don't darken together). An intent
//! component whose contents change every frame would defeat the content-keyed look
//! caches (a new material per frame, never freed). The systems therefore update
//! only engine-owned uniforms on already-bound materials; they never choose a
//! shader or create a terrain material.
//!
//! Ordering: material and frame projection run in `PostUpdate`, after BigSpace
//! has finished propagating the floating-origin frame used by the renderer.
//! Cache completion remains in `Update`; the post-update consumers see that
//! completed cache and the same finalized terrain transforms as the renderer.

use crate::shader_material::ShaderMaterial;
use bevy::asset::AssetId;
use bevy::camera::visibility::RenderLayers;
use bevy::pbr::MeshMaterial3d;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_environment::SunRenderState;
use lunco_environment::horizon::{
    HorizonMap, HorizonShadowCache, HorizonShadowCacheConfig, SunQuery, TerrainSunProjectionCache,
    pick_sun,
};
use lunco_materials::ParamValue;

struct TerrainEngineInputs {
    height_map: Option<Handle<Image>>,
    shadow_cache: Option<Handle<Image>>,
    sun_dir: Vec3,
    sun_dir_world: Vec3,
    sun_tan_radius: f32,
    hf_size: Vec2,
    hf_res: f32,
    terrain_geometry_on: f32,
    csm_far: f32,
    shadow_cache_on: f32,
    horizon_march_steps: f32,
}

fn terrain_material_needs_engine_inputs(
    material: &ShaderMaterial,
    inputs: &TerrainEngineInputs,
) -> bool {
    material.height_map != inputs.height_map
        || material.shadow_cache != inputs.shadow_cache
        || material
            .get_scalar("shadow_cache_on")
            .is_none_or(|value| (value - inputs.shadow_cache_on).abs() > 1.0e-3)
        || material
            .get_vec3("sun_dir")
            .is_none_or(|value| (value - inputs.sun_dir).length() > SUN_DIR_EPSILON)
        || material
            .get_vec3("sun_dir_world")
            .is_none_or(|value| (value - inputs.sun_dir_world).length() > SUN_DIR_EPSILON)
        || material
            .get_scalar("sun_tan_radius")
            .is_none_or(|value| (value - inputs.sun_tan_radius).abs() > 1.0e-6)
        || material.get("hf_size").is_none_or(|value| {
            let ParamValue::Vec2(value) = value else {
                return true;
            };
            let delta = Vec2::from_array(value) - inputs.hf_size;
            delta.length() > 1.0e-3
        })
        || material
            .get_scalar("hf_res")
            .is_none_or(|value| (value - inputs.hf_res).abs() > 1.0e-3)
        || material
            .get_scalar("terrain_geometry_on")
            .is_none_or(|value| (value - inputs.terrain_geometry_on).abs() > 1.0e-3)
        || material
            .get_scalar("csm_far")
            .is_none_or(|value| (value - inputs.csm_far).abs() > 1.0e-3)
        || material
            .get_scalar("horizon_march_steps")
            .is_none_or(|value| (value - inputs.horizon_march_steps).abs() > 1.0e-3)
}

fn write_terrain_engine_inputs(material: &mut ShaderMaterial, inputs: &TerrainEngineInputs) {
    if material.height_map != inputs.height_map {
        material.height_map = inputs.height_map.clone();
    }
    if material.shadow_cache != inputs.shadow_cache {
        material.shadow_cache = inputs.shadow_cache.clone();
    }
    material.set_many([
        ("sun_dir", ParamValue::Vec3(inputs.sun_dir.to_array())),
        (
            "sun_dir_world",
            ParamValue::Vec3(inputs.sun_dir_world.to_array()),
        ),
        ("sun_tan_radius", ParamValue::F32(inputs.sun_tan_radius)),
        (
            "hf_size",
            ParamValue::Vec2([inputs.hf_size.x, inputs.hf_size.y]),
        ),
        ("hf_res", ParamValue::F32(inputs.hf_res)),
        (
            "terrain_geometry_on",
            ParamValue::F32(inputs.terrain_geometry_on),
        ),
        ("csm_far", ParamValue::F32(inputs.csm_far)),
        ("shadow_cache_on", ParamValue::F32(inputs.shadow_cache_on)),
        (
            "horizon_march_steps",
            ParamValue::F32(inputs.horizon_march_steps),
        ),
    ]);
}

fn write_terrain_material_if_needed(
    materials: &mut Assets<ShaderMaterial>,
    handle: &Handle<ShaderMaterial>,
    inputs: &TerrainEngineInputs,
) {
    let needs = materials
        .get(handle)
        .is_some_and(|material| terrain_material_needs_engine_inputs(material, inputs));
    if needs {
        if let Some(mut material) = materials.get_mut(handle) {
            write_terrain_engine_inputs(&mut material, inputs);
        }
    }
}

pub(crate) fn build(app: &mut App) {
    // `EnvironmentPlugin` also inits this (it drives the bake); `init_resource` is a
    // no-op when it is already there. Doing it here too means adding the render
    // plugin without the environment plugin cannot fail system-param validation.
    app.init_resource::<HorizonShadowCacheConfig>();
    app.init_resource::<lunco_render::RenderingQualitySettings>();
    app.add_systems(
        Update,
        sync_horizon_quality_settings
            .run_if(resource_changed::<lunco_render::RenderingQualitySettings>),
    );
    // These consumers all use the finalized render-space frame. The camera-origin
    // writer and BigSpace recenter/propagation phases run earlier in PostUpdate;
    // keeping the whole group after low-precision propagation prevents the
    // current sun from being combined with the previous terrain GlobalTransform.
    app.add_systems(
        PostUpdate,
        (
            wire_terrain_materials,
            wire_sun_for_non_terrain_materials,
            wire_blueprint_origin,
        )
            .chain()
            .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision)
            .after(lunco_environment::finalize_sun_render_state)
            .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
    );
}

/// Project the persisted Graphics quality settings into the render-free horizon
/// bake resource. The environment crate owns the bake implementation, while
/// Graphics owns the user's rendering-quality intent; this is the sole bridge.
/// Invalid settings remain unapplied and are reported by the camera/shadow
/// policy that already validates the same resource.
fn sync_horizon_quality_settings(
    settings: Res<lunco_render::RenderingQualitySettings>,
    mut cfg: ResMut<HorizonShadowCacheConfig>,
) {
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "invalid Graphics horizon-shadow settings: {reason}; preserving current horizon configuration"
            );
            return;
        }
    };
    cfg.enabled = profile.horizon_shadow_cache_enabled;
    cfg.sun_threshold_deg = profile.horizon_shadow_cache_sun_threshold_deg;
    cfg.march_steps = profile.horizon_march_steps;
    cfg.samples_per_axis = profile.horizon_cache_samples_per_axis;
}

// ─────────────────────────────────────────────────────────────────────────
// Material wiring — heightfield + sun uniforms into the terrain shader
// ─────────────────────────────────────────────────────────────────────────

/// Keeps every horizon terrain's `ShaderMaterial` wired: heightfield
/// texture, static size/resolution, the per-frame sun direction, and the
/// **shadow cache** binding + `shadow_cache_on` flag.
/// A static terrain's material is supplied by the standard USD `UsdShade`
/// projection (or by an explicit `ShaderLook` on a command-created terrain).
/// This system only writes the heightfield/sun inputs after that intent has been
/// bound; it never selects or synthesizes a shader asset.
/// Clear engine-owned sun uniforms when the semantic/render sun is unavailable.
/// A previously valid material must not keep lighting from an old scene or
/// provider sample after the owning state has become invalid.
fn clear_sun_material(
    materials: &mut Assets<ShaderMaterial>,
    handle: &MeshMaterial3d<ShaderMaterial>,
) {
    let needs_clear = materials.get(&handle.0).is_some_and(|material| {
        material
            .get_vec3("sun_dir")
            .is_some_and(|value| value.length_squared() > 1.0e-12)
            || material
                .get_vec3("sun_dir_world")
                .is_some_and(|value| value.length_squared() > 1.0e-12)
            || material
                .get_scalar("sun_tan_radius")
                .is_some_and(|value| value.abs() > 1.0e-6)
            || material
                .get_scalar("shadow_cache_on")
                .is_some_and(|value| value.abs() > 1.0e-6)
    });
    if needs_clear {
        if let Some(mut material) = materials.get_mut(&handle.0) {
            material.set_many([
                ("sun_dir", ParamValue::Vec3([0.0, 0.0, 0.0])),
                ("sun_dir_world", ParamValue::Vec3([0.0, 0.0, 0.0])),
                ("sun_tan_radius", ParamValue::F32(0.0)),
                ("shadow_cache_on", ParamValue::F32(0.0)),
            ]);
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn wire_terrain_materials(
    cfg: Res<HorizonShadowCacheConfig>,
    sun: SunQuery,
    render_sun: Option<Res<SunRenderState>>,
    shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    terrains: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            Option<&HorizonMap>,
            Option<&HorizonShadowCache>,
            Option<&Mesh3d>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
        ),
        (
            With<lunco_terrain_surface::DemTerrainSurface>,
            Without<RenderLayers>,
        ),
    >,
    tile_materials: Query<
        (
            &lunco_terrain_surface::LodTileOf,
            &MeshMaterial3d<ShaderMaterial>,
        ),
        Without<RenderLayers>,
    >,
    // Hysteresis state for the cache↔march handoff, per terrain (see below).
    mut cache_engaged: Local<std::collections::HashMap<Entity, bool>>,
    // The visible tile set is rebuilt from ECS every frame, but its grouping
    // buffers persist so streaming does not allocate a new map/vector set per
    // frame.
    mut streamed_materials: Local<std::collections::HashMap<Entity, Vec<Handle<ShaderMaterial>>>>,
    mut written_materials: Local<HashSet<AssetId<ShaderMaterial>>>,
    // Reuses the local sun direction while both the finalized terrain frame and
    // the render sun revision remain unchanged; entries follow the terrain's
    // lifecycle cleanup below.
    mut sun_projection_cache: Local<TerrainSunProjectionCache>,
    mut removed_terrains: RemovedComponents<HorizonMap>,
) {
    for e in removed_terrains.read() {
        cache_engaged.remove(&e);
        sun_projection_cache.remove(e);
    }
    let Some(mut shader_mats) = shader_mats else {
        return;
    };

    // Recycle the per-owner vectors while dropping storage for owners that have
    // left the terrain query. Rebuild membership from the current tile query so
    // streamed tile replacement remains immediately visible to this binder.
    for materials in streamed_materials.values_mut() {
        materials.clear();
    }
    streamed_materials.retain(|owner, _| terrains.get(*owner).is_ok());
    for (owner, material) in &tile_materials {
        streamed_materials
            .entry(owner.0)
            .or_default()
            .push(material.0.clone());
    }
    let Some((_, tan_r, csm_far)) = pick_sun(&sun) else {
        for (entity, _, _, _, _, shader_mat) in &terrains {
            if let Some(shader_mat) = shader_mat {
                clear_sun_material(&mut shader_mats, shader_mat);
            }
            if let Some(materials) = streamed_materials.get(&entity) {
                for material in materials {
                    clear_sun_material(&mut shader_mats, &MeshMaterial3d(material.clone()));
                }
            }
        }
        return;
    };
    // NOTE on the near-camera march fade (`csm_far`): the fade is a PERF
    // gate, not just cosmetics — inside it the configured live march is
    // skipped (CSM owns the near field), and "march everywhere" turned low
    // flight into a slideshow. (Streamed tiles DO get a baked cache on
    // native — `lunco-luncosim-ui` samples the oracle
    // into a `HorizonMap` and mirrors the cache to tiles — but the cache
    // fades in on the same `csm_far` boundary.) The cost of the fade is
    // that the CSM volume (~1.5 km) cannot contain multi-km ridge occluders,
    // so near terrain can read slightly lighter than the same ground seen from
    // altitude. We deliberately do not compensate with a terrain-only fill:
    // it would make the terrain obey a different illumination model from every
    // dynamic PBR object and would incorrectly look like bounced light.
    let Some(to_sun_world) = render_sun
        .as_deref()
        .and_then(|state| state.direction_to_sun_world)
    else {
        for (entity, _, _, _, _, shader_mat) in &terrains {
            if let Some(shader_mat) = shader_mat {
                clear_sun_material(&mut shader_mats, shader_mat);
            }
            if let Some(materials) = streamed_materials.get(&entity) {
                for material in materials {
                    clear_sun_material(&mut shader_mats, &MeshMaterial3d(material.clone()));
                }
            }
        }
        return;
    };
    let cache_quality_valid = cfg.quality_is_valid();

    for (entity, terrain_gt, map, shadow_cache, _mesh, shader_mat) in &terrains {
        let tiles = streamed_materials
            .get(&entity)
            .map(Vec::as_slice)
            .unwrap_or_default();
        // A streamed terrain owner has no mesh: its visible materials live on
        // the LOD tile children. The same engine contract must nevertheless be
        // projected to those children; otherwise replacing a tile changes the
        // terrain's lighting model. A terrain with no material yet has nothing
        // to bind, but remains eligible on the next material-creation pass.
        if tiles.is_empty() && shader_mat.is_none() {
            continue;
        }
        let sun_local = sun_projection_cache.project_sun_local(
            entity,
            &terrain_gt,
            to_sun_world,
            render_sun.as_deref().map_or(0, |state| state.revision),
        );
        let (hf_size_v, hf_res, height_map_handle) = match map {
            Some(m) => (
                m.field.size(),
                m.field.resolution() as f32,
                Some(m.image.clone()),
            ),
            None => (Vec2::ONE, 0.0, None),
        };

        // Shadow cache binding + the uniform flag that tells the fragment
        // shader to sample it (`1.0`) instead of ray-marching (`0.0`). The
        // handle is bound whenever a cache exists (it stays allocated on the
        // `HorizonShadowCache` component regardless). A committed cache is
        // deliberately kept active while a replacement bake is pending or
        // while the sun has moved past its sample: the old image is the
        // double-buffered presentation value. Disabling it for every stale
        // sample exposed the 48-step live march exactly when a fast clock was
        // already consuming the bake workers, causing both stalls and an
        // apparent terrain-texture loss. Below-horizon sun still falls back
        // to the march, which short-circuits to 0 in its first branch.
        let cache_image: Option<Handle<Image>> = shadow_cache.map(|c| c.image.clone());
        let engaged = {
            let prev = cache_engaged.get(&entity).copied().unwrap_or(false);
            let now = if prev {
                sun_local.y > 5.0e-5
            } else {
                sun_local.y > 2.0e-4
            };
            cache_engaged.insert(entity, now);
            now
        };
        let shadow_cache_on: f32 =
            if cache_quality_valid && cfg.enabled && engaged && shadow_cache.is_some() {
                1.0
            } else {
                0.0
            };
        let horizon_march_steps = cfg.march_steps as f32;

        // Named engine uniforms consumed by the terrain shaders (regolith /
        // terrain_shadow declare these in their `Material` struct; the engine
        // packs them at the reflected offsets).
        // World-space to-sun for the BRDF opposition term. The march uses the
        // terrain-LOCAL `sun_dir` (heightfield space); the lunar BRDF runs in
        // world space (world N/V), so it needs the world-space sun. Passing the
        // CPU-picked canonical sun here means the shader never has to guess it
        // from `directional_lights[0]` — robust to the earthshine fill light.
        let inputs = TerrainEngineInputs {
            height_map: height_map_handle,
            shadow_cache: cache_image,
            sun_dir: sun_local,
            sun_dir_world: to_sun_world,
            sun_tan_radius: tan_r,
            hf_size: hf_size_v,
            hf_res,
            terrain_geometry_on: f32::from(map.is_some()),
            csm_far,
            shadow_cache_on,
            horizon_march_steps,
        };

        // Compare before `get_mut`: this is a change-driven material feed even
        // though the tile set itself is dynamic. The handle, heightfield/cache
        // identity, sun revision epsilon, and all engine scalars cover the
        // complete terrain shader contract.
        written_materials.clear();
        for handle in tiles {
            if written_materials.insert(handle.id()) {
                write_terrain_material_if_needed(&mut shader_mats, handle, &inputs);
            }
        }
        if let Some(shader_mat) = shader_mat {
            if written_materials.insert(shader_mat.0.id()) {
                write_terrain_material_if_needed(&mut shader_mats, &shader_mat.0, &inputs);
            }
        }
    }
}

/// Fill `sun_dir_world` on every OTHER `ShaderMaterial` — the ones with no
/// `HorizonMap` behind them.
///
/// [`wire_terrain_materials`] only sees genuine heightfield terrain, but
/// `regolith.wgsl` is bound to ordinary meshes too (the landing pad disc, the
/// marketing scenes' ground plate). The semantic sun projection writes the
/// uniform for those materials as well; this system never chooses a light or
/// derives a direction from render entities.
///
/// The sun is a scene-global fact, so it is written everywhere rather than
/// re-derived independently by each shader. Running across every non-terrain
/// `ShaderMaterial` is safe: a name the shader does not declare is kept in the
/// material's `values` map but has no schema offset, so `repack()` never packs it
/// into the uniform block — it costs a map entry and reaches no GPU binding.
///
/// Terrain is EXCLUDED (`Without<HorizonMap>`) — it is already written above,
/// with the local-space `sun_dir` this system has no business computing.
// The memo remains private; consumers install the public system as a whole.
#[allow(private_interfaces)]
pub fn wire_sun_for_non_terrain_materials(
    sun: SunQuery,
    render_sun: Option<Res<SunRenderState>>,
    shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    meshes: Query<&MeshMaterial3d<ShaderMaterial>, (Without<HorizonMap>, Without<RenderLayers>)>,
    changed_meshes: Query<
        &MeshMaterial3d<ShaderMaterial>,
        (
            Changed<MeshMaterial3d<ShaderMaterial>>,
            Without<HorizonMap>,
            Without<RenderLayers>,
        ),
    >,
    mut binding_state: Local<NonTerrainSunBindingState>,
) {
    let Some(mut shader_mats) = shader_mats else {
        return;
    };
    let next = pick_sun(&sun).and_then(|(_, tan_radius, _)| {
        render_sun
            .as_deref()
            .and_then(|state| state.direction_to_sun_world)
            .map(|direction| NonTerrainSunUniform {
                direction,
                tan_radius,
            })
    });

    // Accumulate motion against the last fully applied value, not the previous
    // frame. This retains the material-space tolerance while avoiding a full
    // mesh query and per-frame de-duplication below it. Changed/new bindings are
    // still initialized immediately.
    if binding_state.requires_full_refresh(next) {
        wire_non_terrain_sun_uniforms(&mut shader_mats, &meshes, next);
        binding_state.publish(next);
    } else {
        wire_non_terrain_sun_uniforms(&mut shader_mats, &changed_meshes, next);
    }
}

#[derive(Clone, Copy)]
struct NonTerrainSunUniform {
    direction: Vec3,
    tan_radius: f32,
}

#[derive(Default)]
struct NonTerrainSunBindingState {
    applied: Option<NonTerrainSunUniform>,
    initialized: bool,
}

impl NonTerrainSunBindingState {
    fn requires_full_refresh(&self, next: Option<NonTerrainSunUniform>) -> bool {
        if !self.initialized {
            return true;
        }
        match (self.applied, next) {
            (Some(applied), Some(next)) => {
                (applied.direction - next.direction).length() > SUN_DIR_EPSILON
                    || (applied.tan_radius - next.tan_radius).abs() > 1.0e-6
            }
            (None, None) => false,
            _ => true,
        }
    }

    fn publish(&mut self, applied: Option<NonTerrainSunUniform>) {
        self.applied = applied;
        self.initialized = true;
    }
}

fn wire_non_terrain_sun_uniforms<'a>(
    shader_mats: &mut Assets<ShaderMaterial>,
    meshes: impl IntoIterator<Item = &'a MeshMaterial3d<ShaderMaterial>>,
    sun: Option<NonTerrainSunUniform>,
) {
    let mut written: HashSet<AssetId<ShaderMaterial>> = HashSet::default();
    for handle in meshes {
        if !written.insert(handle.0.id()) {
            continue;
        }
        let Some(sun) = sun else {
            clear_sun_material(shader_mats, handle);
            continue;
        };

        // Compare before get_mut, or every frame re-uploads the asset (MAT-3).
        let needs = shader_mats.get(&handle.0).is_some_and(|material| {
            material
                .get_vec3("sun_dir_world")
                .is_none_or(|value| (value - sun.direction).length() > SUN_DIR_EPSILON)
                || material
                    .get_scalar("sun_tan_radius")
                    .is_none_or(|value| (value - sun.tan_radius).abs() > 1.0e-6)
        });
        if needs {
            if let Some(mut material) = shader_mats.get_mut(&handle.0) {
                material.set_many([
                    ("sun_dir_world", ParamValue::Vec3(sun.direction.to_array())),
                    ("sun_tan_radius", ParamValue::F32(sun.tan_radius)),
                ]);
            }
        }
    }
}

/// Keep Cartesian blueprint lines in the authored active terrain frame.
///
/// BigSpace deliberately exposes camera-relative `GlobalTransform` values to
/// the renderer. That is the right frame for rasterisation, but it is not the
/// frame in which an authored terrain grid is defined. The shader receives the
/// active frame's finalized render-space origin and inverse rotation before
/// evaluating its periodic coordinates. The active frame is a render
/// projection here; semantic pose remains owned by the f64 grid hierarchy.
/// Reading the finalized transform is important: re-projecting the semantic
/// pose through the WorldGrid would round the same nested hierarchy a second,
/// different way and make periodic lines move at large coordinates.
pub fn wire_blueprint_origin(
    origin: Query<(&CellCoord, &Grid), With<lunco_spatial::OriginAnchor>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    frame_render: Query<&GlobalTransform>,
    shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    meshes: Query<&MeshMaterial3d<ShaderMaterial>, Without<RenderLayers>>,
) {
    let Some(mut shader_mats) = shader_mats else {
        return;
    };
    let offset = origin
        .single()
        .ok()
        .map(|(cell, grid)| grid.cell_to_float(cell).as_vec3())
        .unwrap_or(Vec3::ZERO);
    let mut blueprint_origin = offset;
    let mut frame_origin = Vec3::ZERO;
    let mut frame_rotation = Vec4::new(0.0, 0.0, 0.0, 1.0);

    // A site-mounted surface is authored in the active physics/site grid, not
    // in inertial WorldGrid XZ. Use the exact finalized render projection that
    // BigSpace gave that frame. The subtraction is deliberately render-relative:
    // adding a huge absolute cell to a f32 shader coordinate would reintroduce
    // the precision loss BigSpace avoids.
    if let Some(active_frame) = active_frame {
        let Ok(frame_render) = frame_render.get(active_frame.0) else {
            return;
        };
        let (_, render_rotation, render_position) = frame_render.to_scale_rotation_translation();
        blueprint_origin = Vec3::ZERO;
        frame_origin = render_position;
        frame_rotation = Vec4::from_array(render_rotation.inverse().to_array());
    }

    let values = [
        (
            "blueprint_origin",
            ParamValue::Vec3(blueprint_origin.to_array()),
        ),
        (
            "blueprint_frame_origin",
            ParamValue::Vec3(frame_origin.to_array()),
        ),
        (
            "blueprint_frame_rotation",
            ParamValue::Vec4(frame_rotation.to_array()),
        ),
    ];
    let mut written: HashSet<AssetId<ShaderMaterial>> = HashSet::default();

    for handle in &meshes {
        if !written.insert(handle.0.id()) {
            continue;
        }
        let Some(material) = shader_mats.get(&handle.0) else {
            continue;
        };
        if values
            .iter()
            .any(|(name, _)| material.schema.field(name).is_none())
        {
            continue;
        }
        let unchanged = material
            .get_vec3("blueprint_origin")
            .is_some_and(|current| (current - blueprint_origin).length() <= SUN_DIR_EPSILON)
            && material
                .get_vec3("blueprint_frame_origin")
                .is_some_and(|current| (current - frame_origin).length() <= SUN_DIR_EPSILON)
            && material
                .get_vec4("blueprint_frame_rotation")
                .is_some_and(|current| (current - frame_rotation).length() <= SUN_DIR_EPSILON);
        if unchanged {
            continue;
        }
        if let Some(mut material) = shader_mats.get_mut(&handle.0) {
            material.set_many(values.iter().map(|(name, value)| (*name, *value)));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
/// Minimum change in a stored sun DIRECTION before the material carrying it is
/// repacked. On a unit vector this is ~0.006° — three orders of magnitude finer
/// than a visible shadow-direction change. Its job is to keep a *continuously*
/// moving sun from repacking every terrain material every frame.
const SUN_DIR_EPSILON: f32 = 1e-4;

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an app with just enough to run the sun-wiring system.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        // Fresh app, so this cannot clobber an existing store (`init_asset` is
        // destructive, not idempotent).
        app.init_asset::<ShaderMaterial>();
        app.insert_resource(lunco_environment::SunRenderState {
            direction_to_sun_world: Some(Vec3::Z),
            revision: 0,
        });
        app.add_systems(Update, wire_sun_for_non_terrain_materials);
        app
    }

    #[test]
    fn clearing_an_already_clear_material_does_not_modify_the_asset() {
        #[derive(Resource, Default)]
        struct Modified(usize);

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<ShaderMaterial>();
        let mut material = ShaderMaterial::default();
        material.set_many([
            ("sun_dir", ParamValue::Vec3([0.0, 0.0, 1.0])),
            ("sun_dir_world", ParamValue::Vec3([0.0, 0.0, 1.0])),
            ("sun_tan_radius", ParamValue::F32(0.01)),
            ("shadow_cache_on", ParamValue::F32(1.0)),
        ]);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(material);
        app.world_mut().spawn(MeshMaterial3d(handle));
        app.init_resource::<Modified>();
        app.add_systems(
            Update,
            (
                |mut materials: ResMut<Assets<ShaderMaterial>>,
                 meshes: Query<&MeshMaterial3d<ShaderMaterial>>| {
                    for handle in &meshes {
                        clear_sun_material(&mut materials, handle);
                    }
                },
                |mut events: MessageReader<AssetEvent<ShaderMaterial>>,
                 mut modified: ResMut<Modified>| {
                    modified.0 += events
                        .read()
                        .filter(|event| matches!(event, AssetEvent::Modified { .. }))
                        .count();
                },
            )
                .chain(),
        );

        for _ in 0..6 {
            app.update();
        }

        assert_eq!(
            app.world().resource::<Modified>().0,
            1,
            "clear should upload once when values change and remain quiet afterward"
        );
    }

    /// The blueprint material must use the same finalized render transform as
    /// the mesh in the same frame. If it runs in `Update`, it samples the
    /// previous render state while the mesh receives the new origin-relative
    /// transform in `PostUpdate`, producing a one-frame line displacement when
    /// the origin moves.
    #[test]
    fn blueprint_frame_uniform_tracks_finalized_big_space_origin() {
        use big_space::plugin::BigSpaceMinimalPlugins;
        use big_space::prelude::BigSpaceSystems;
        use std::sync::Arc;

        fn move_origin_once(mut origins: Query<&mut CellCoord, With<lunco_spatial::OriginAnchor>>) {
            origins
                .single_mut()
                .expect("the test has one canonical origin anchor")
                .set_if_neq(CellCoord::new(10, 0, 0));
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .add_plugins(BigSpaceMinimalPlugins)
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<bevy::pbr::StandardMaterial>()
            .init_asset::<ShaderMaterial>();

        let world_grid = lunco_spatial::ensure_world_root(app.world_mut());
        let origin = {
            let mut query = app
                .world_mut()
                .query_filtered::<Entity, With<lunco_spatial::OriginAnchor>>();
            query.single(app.world()).expect("one origin anchor")
        };
        app.world_mut()
            .entity_mut(origin)
            .insert(GlobalTransform::default());

        let frame = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::default(),
                Transform::from_xyz(5.0, 0.0, 0.0),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();
        app.world_mut()
            .insert_resource(lunco_spatial::ActivePhysicsFrame(frame));

        let schema = lunco_materials::ParamSchema::parse(
            "struct Material {\n\
                blueprint_origin: vec3<f32>,\n\
                blueprint_frame_origin: vec3<f32>,\n\
                blueprint_frame_rotation: vec4<f32>,\n\
            }",
        )
        .expect("the blueprint engine fields must be reflectable");
        let mut material = ShaderMaterial::default();
        material.set_schema(Arc::new(schema));
        let material_handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(material);
        app.world_mut()
            .spawn(MeshMaterial3d::<ShaderMaterial>(material_handle.clone()));

        // Exercise the production registration, including its PostUpdate
        // ordering, rather than registering the system again in the test.
        build(&mut app);

        // This models the authoritative camera-origin writer. It changes the
        // origin before BigSpace's own recenter and propagation phases.
        app.add_systems(
            PostUpdate,
            move_origin_once.before(BigSpaceSystems::RecenterLargeTransforms),
        );
        app.update();

        let expected = {
            let world_grid_component = app
                .world()
                .get::<Grid>(world_grid)
                .expect("canonical WorldGrid must remain a BigSpace grid");
            lunco_spatial::coords::grid_absolute_pose_to_render(
                world_grid_component,
                lunco_spatial::coords::GridPos(bevy::math::DVec3::new(5.0, 0.0, 0.0)),
                lunco_spatial::coords::GridRot(bevy::math::DQuat::IDENTITY),
            )
            .0
            .0
            .as_vec3()
        };
        let actual = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&material_handle)
            .and_then(|material| material.get_vec3("blueprint_frame_origin"))
            .expect("blueprint frame origin must be written");

        assert_eq!(actual, expected);
        assert_ne!(actual, Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn blueprint_frame_uniform_matches_nested_grid_render_transform() {
        use big_space::plugin::BigSpaceMinimalPlugins;
        use std::sync::Arc;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .add_plugins(BigSpaceMinimalPlugins)
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<bevy::pbr::StandardMaterial>()
            .init_asset::<ShaderMaterial>();

        let world_grid = lunco_spatial::ensure_world_root(app.world_mut());
        let origin = {
            let mut query = app
                .world_mut()
                .query_filtered::<Entity, With<lunco_spatial::OriginAnchor>>();
            query.single(app.world()).expect("one origin anchor")
        };
        app.world_mut()
            .entity_mut(origin)
            .insert(GlobalTransform::default());

        let body_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::new(160_000, -70_000, 90_000),
                Transform::from_xyz(317.25, -81.5, 142.0)
                    .with_rotation(Quat::from_rotation_y(0.713)),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::new(-43_000, 21_000, 18_000),
                Transform::from_xyz(-422.75, 61.25, 205.5)
                    .with_rotation(Quat::from_rotation_x(-0.417)),
                GlobalTransform::default(),
                ChildOf(body_grid),
            ))
            .id();
        let frame = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::new(11, -7, 13),
                Transform::from_xyz(47.125, 2.75, -31.5)
                    .with_rotation(Quat::from_rotation_z(0.291)),
                GlobalTransform::default(),
                ChildOf(surface_grid),
            ))
            .id();
        app.world_mut()
            .insert_resource(lunco_spatial::ActivePhysicsFrame(frame));

        let schema = lunco_materials::ParamSchema::parse(
            "struct Material {\n\
                blueprint_origin: vec3<f32>,\n\
                blueprint_frame_origin: vec3<f32>,\n\
                blueprint_frame_rotation: vec4<f32>,\n\
            }",
        )
        .expect("the blueprint engine fields must be reflectable");
        let mut material = ShaderMaterial::default();
        material.set_schema(Arc::new(schema));
        let material_handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(material);
        app.world_mut()
            .spawn(MeshMaterial3d::<ShaderMaterial>(material_handle.clone()));

        build(&mut app);
        fn move_origin_once(mut origins: Query<&mut CellCoord, With<lunco_spatial::OriginAnchor>>) {
            origins
                .single_mut()
                .expect("the test has one canonical origin anchor")
                .set_if_neq(CellCoord::new(160_007, -70_004, 90_002));
        }
        app.add_systems(
            PostUpdate,
            move_origin_once.before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms),
        );
        app.update();

        let actual_uniform = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&material_handle)
            .and_then(|material| material.get_vec3("blueprint_frame_origin"))
            .expect("blueprint frame origin must be written");
        let frame_render = app
            .world()
            .get::<GlobalTransform>(frame)
            .expect("the active frame must have a propagated render transform")
            .translation();
        let (_, frame_rotation, _) = app
            .world()
            .get::<GlobalTransform>(frame)
            .expect("the active frame must have a propagated render transform")
            .to_scale_rotation_translation();
        let actual_rotation = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&material_handle)
            .and_then(|material| material.get_vec4("blueprint_frame_rotation"))
            .expect("blueprint frame rotation must be written");
        let expected_rotation = Vec4::from_array(frame_rotation.inverse().to_array());

        assert!(
            (actual_uniform - frame_render).length() < 1.0e-3,
            "uniform {actual_uniform:?}, frame GlobalTransform {frame_render:?}, delta {:?}",
            actual_uniform - frame_render
        );
        assert!((actual_rotation - expected_rotation).length() < 1.0e-6);

        // A rotating ancestor changes the finalized render projection without
        // changing the authored active-frame pose. The next frame must still
        // feed that exact projection to the shader.
        app.world_mut()
            .get_mut::<Transform>(body_grid)
            .expect("the body grid must retain its transform")
            .rotation = Quat::from_rotation_y(1.137);
        app.update();

        let actual_uniform = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&material_handle)
            .and_then(|material| material.get_vec3("blueprint_frame_origin"))
            .expect("blueprint frame origin must be refreshed");
        let frame_render = app
            .world()
            .get::<GlobalTransform>(frame)
            .expect("the active frame must retain a propagated render transform")
            .translation();
        assert!((actual_uniform - frame_render).length() < 1.0e-3);
    }

    #[test]
    fn terrain_shadow_projection_uses_the_finalized_render_transform() {
        use big_space::plugin::BigSpaceMinimalPlugins;
        use std::f32::consts::FRAC_PI_2;

        fn finalize_terrain_render_pose(
            mut terrains: Query<
                &mut GlobalTransform,
                With<lunco_terrain_surface::DemTerrainSurface>,
            >,
        ) {
            for mut transform in &mut terrains {
                *transform = GlobalTransform::from_rotation(Quat::from_rotation_y(FRAC_PI_2));
            }
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .add_plugins(BigSpaceMinimalPlugins)
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<bevy::pbr::StandardMaterial>()
            .init_asset::<ShaderMaterial>();
        app.insert_resource(lunco_environment::SunRenderState {
            direction_to_sun_world: Some(Vec3::X),
            revision: 1,
        });
        app.world_mut().spawn(DirectionalLight::default());

        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        let terrain = app
            .world_mut()
            .spawn((
                lunco_terrain_surface::DemTerrainSurface,
                GlobalTransform::IDENTITY,
                MeshMaterial3d(handle.clone()),
            ))
            .id();

        app.add_systems(
            PostUpdate,
            finalize_terrain_render_pose.before(wire_terrain_materials),
        );
        build(&mut app);
        app.update();

        let actual = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&handle)
            .and_then(|material| material.get_vec3("sun_dir"))
            .expect("terrain sun direction must be wired");
        let expected = Quat::from_rotation_y(FRAC_PI_2).inverse() * Vec3::X;
        assert!(
            actual.abs_diff_eq(expected, 1.0e-6),
            "terrain shadow direction must use the finalized render transform: got {actual:?}, expected {expected:?}"
        );

        let tile_handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn((
            lunco_terrain_surface::LodTileOf(terrain),
            MeshMaterial3d(tile_handle.clone()),
        ));
        app.update();

        let tile_sun = app
            .world()
            .resource::<Assets<ShaderMaterial>>()
            .get(&tile_handle)
            .and_then(|material| material.get_vec3("sun_dir"))
            .expect("a newly streamed terrain tile must receive the owner projection");
        assert!(tile_sun.abs_diff_eq(expected, 1.0e-6));
    }

    /// A `ShaderMaterial` on a mesh with NO `HorizonMap` must still get the sun.
    ///
    /// This keeps ordinary meshes on the same semantic sun projection as
    /// heightfield terrain. If the projection is unavailable, the material is
    /// left unbound and the owning environment diagnostic remains visible.
    #[test]
    fn a_non_terrain_shader_material_gets_the_sun() {
        let mut app = test_app();

        // Sun: identity rotation ⇒ `GlobalTransform::back()` is +Z.
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 10_000.0,
                ..Default::default()
            },
        ));

        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));

        app.update();

        let mats = app.world().resource::<Assets<ShaderMaterial>>();
        let got = mats.get(&handle).and_then(|m| m.get("sun_dir_world"));
        assert_eq!(
            got,
            Some(ParamValue::Vec3([0.0, 0.0, 1.0])),
            "a non-terrain ShaderMaterial must receive the world-space to-sun vector"
        );
    }

    #[test]
    fn a_non_terrain_material_clears_after_the_sun_becomes_invalid() {
        let mut app = test_app();
        let sun = app
            .world_mut()
            .spawn((
                GlobalTransform::IDENTITY,
                DirectionalLight {
                    illuminance: 10_000.0,
                    ..Default::default()
                },
            ))
            .id();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));

        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<ShaderMaterial>>()
                .get(&handle)
                .and_then(|material| material.get("sun_dir_world")),
            Some(ParamValue::Vec3([0.0, 0.0, 1.0]))
        );

        app.world_mut().entity_mut(sun).despawn();
        app.world_mut()
            .resource_mut::<lunco_environment::SunRenderState>()
            .direction_to_sun_world = None;
        app.update();

        assert_eq!(
            app.world()
                .resource::<Assets<ShaderMaterial>>()
                .get(&handle)
                .and_then(|material| material.get("sun_dir_world")),
            Some(ParamValue::Vec3([0.0, 0.0, 0.0])),
            "invalid semantic lighting must clear the last valid shader direction"
        );
    }

    /// The single unscoped sun is selected structurally by `pick_sun`.
    #[test]
    fn the_single_structural_sun_sets_the_direction() {
        let mut app = test_app();

        // The render light's pose is deliberately unrelated: semantic state is
        // the direction authority consumed by the material binder.
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 100_000.0,
                ..Default::default()
            },
        ));
        app.world_mut()
            .insert_resource(lunco_environment::SunRenderState {
                direction_to_sun_world: Some(Vec3::X),
                revision: 1,
            });

        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));

        app.update();

        let mats = app.world().resource::<Assets<ShaderMaterial>>();
        let Some(ParamValue::Vec3(v)) = mats.get(&handle).and_then(|m| m.get("sun_dir_world"))
        else {
            panic!("sun_dir_world missing or not a Vec3");
        };
        assert!(v[0] > 0.99, "expected the sun's +X direction, got {v:?}");
    }

    /// STEADY STATE COSTS NOTHING. Running the system twice with an unmoved sun
    /// must not touch the material the second time.
    ///
    /// This is the assertion whose absence hid a permanent re-upload: the guard
    /// compared a `Vec3`-stored param with `get_vec4`, always answered "changed",
    /// and every material was rewritten every frame. Nothing failed — it was just
    /// silently expensive, which is why only an explicit steady-state check catches
    /// it. `Assets::get_mut` bumps the change tick, so that is what we observe.
    #[test]
    fn an_unmoved_sun_does_not_rewrite_the_material() {
        let mut app = test_app();
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 10_000.0,
                ..Default::default()
            },
        ));
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));

        // `Assets::get_mut` emits `AssetEvent::Modified` — that IS the re-upload
        // signal, so count them per frame rather than inspect the value.
        #[derive(Resource, Default)]
        struct Modified(usize);
        app.init_resource::<Modified>();
        app.add_systems(
            Update,
            (|mut ev: MessageReader<AssetEvent<ShaderMaterial>>, mut n: ResMut<Modified>| {
                n.0 += ev
                    .read()
                    .filter(|e| matches!(e, AssetEvent::Modified { .. }))
                    .count();
            })
            .after(wire_sun_for_non_terrain_materials),
        );

        // Count the TOTAL over several frames rather than diffing per frame:
        // `MessageReader` sees a frame's messages on the NEXT frame, so a per-frame
        // diff reads as one-behind and proves nothing.
        const FRAMES: usize = 6;
        for _ in 0..FRAMES {
            app.update();
        }
        assert_eq!(
            app.world()
                .resource::<Assets<ShaderMaterial>>()
                .get(&handle)
                .map(|m| m.get("sun_dir_world")),
            Some(Some(ParamValue::Vec3([0.0, 0.0, 1.0]))),
            "the sun must be written"
        );
        // Exactly ONE modification: the initial write. Anything more is the guard
        // failing open and re-uploading every frame.
        assert_eq!(
            app.world().resource::<Modified>().0,
            1,
            "expected a single write over {FRAMES} frames; an unmoved sun is \
             re-uploading the material — the change guard is not holding"
        );
    }

    /// A sun in CONTINUOUS MOTION must not repack the material every frame.
    ///
    /// The companion to `an_unmoved_sun_does_not_rewrite_the_material`, and the
    /// case that one cannot see. A parked sun is bit-identical frame to frame, so
    /// an exact compare looks correct against it — while the moment the celestial
    /// clock runs, every frame's direction differs in the last few bits and the
    /// guard fails open again, repacking every terrain material forever. That is
    /// the real deployment: the sun always moves. Only an epsilon closes it, and
    /// only a moving-sun test can tell the two apart.
    ///
    /// Rotates by ~0.0002° per frame — far below `SUN_DIR_EPSILON` but nonzero, so
    /// an exact compare would fire on every one of these frames.
    #[test]
    fn a_slowly_moving_sun_does_not_repack_every_frame() {
        let mut app = test_app();
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 10_000.0,
                ..Default::default()
            },
        ));
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle));

        #[derive(Resource, Default)]
        struct Modified(usize);
        app.init_resource::<Modified>();
        app.add_systems(
            Update,
            (|mut ev: MessageReader<AssetEvent<ShaderMaterial>>, mut n: ResMut<Modified>| {
                n.0 += ev
                    .read()
                    .filter(|e| matches!(e, AssetEvent::Modified { .. }))
                    .count();
            })
            .after(wire_sun_for_non_terrain_materials),
        );

        const FRAMES: usize = 8;
        const STEP_RAD: f32 = 3e-6; // ~0.0002° per frame, well under the epsilon
        for i in 0..FRAMES {
            let rot = Quat::from_rotation_x(STEP_RAD * i as f32);
            app.world_mut()
                .insert_resource(lunco_environment::SunRenderState {
                    direction_to_sun_world: Some(rot * Vec3::Z),
                    revision: i as u64,
                });
            app.update();
        }

        assert_eq!(
            app.world().resource::<Modified>().0,
            1,
            "expected a single write over {FRAMES} frames of a slowly moving sun; \
             the direction guard is comparing exactly and failing open on sub-\
             threshold motion — every terrain material is repacking every frame"
        );
    }

    #[test]
    fn new_materials_get_current_sun_and_existing_materials_refresh_at_tolerance() {
        let mut app = test_app();
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 10_000.0,
                ..Default::default()
            },
        ));
        let original = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(original.clone()));
        app.update();

        let subthreshold = Quat::from_rotation_x(5.0e-5) * Vec3::Z;
        app.world_mut()
            .insert_resource(lunco_environment::SunRenderState {
                direction_to_sun_world: Some(subthreshold),
                revision: 1,
            });
        let added = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(added.clone()));
        app.update();

        let materials = app.world().resource::<Assets<ShaderMaterial>>();
        assert_eq!(
            materials
                .get(&original)
                .and_then(|material| material.get_vec3("sun_dir_world")),
            Some(Vec3::Z),
            "existing materials retain the same sub-threshold directional tolerance"
        );
        assert_eq!(
            materials
                .get(&added)
                .and_then(|material| material.get_vec3("sun_dir_world")),
            Some(subthreshold),
            "a new binding receives the current sun without a full material traversal"
        );

        let above_threshold = Quat::from_rotation_x(1.2e-4) * Vec3::Z;
        app.world_mut()
            .insert_resource(lunco_environment::SunRenderState {
                direction_to_sun_world: Some(above_threshold),
                revision: 2,
            });
        app.update();

        assert_eq!(
            app.world()
                .resource::<Assets<ShaderMaterial>>()
                .get(&original)
                .and_then(|material| material.get_vec3("sun_dir_world")),
            Some(above_threshold),
            "crossing the established tolerance refreshes all bound materials"
        );
    }

    /// Terrain is excluded: `wire_terrain_materials` owns it, and this system has
    /// no business computing the heightfield-local `sun_dir` it also needs.
    #[test]
    fn terrain_is_left_to_the_terrain_wiring() {
        let mut app = test_app();
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight {
                illuminance: 10_000.0,
                ..Default::default()
            },
        ));

        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        // A HorizonMap marks this as terrain. Smallest valid field — the contents
        // are irrelevant, only the component's PRESENCE gates this system.
        let field = lunco_environment::HeightField::from_grid(
            2,
            Vec2::ZERO,
            Vec2::splat(1.0),
            std::sync::Arc::new(vec![0.0; 4]),
        );
        app.world_mut().spawn((
            MeshMaterial3d(handle.clone()),
            HorizonMap {
                field,
                image: Handle::default(),
            },
        ));

        app.update();

        let mats = app.world().resource::<Assets<ShaderMaterial>>();
        assert_eq!(
            mats.get(&handle).and_then(|m| m.get("sun_dir_world")),
            None,
            "terrain must be wired by wire_terrain_materials, not this system"
        );
    }
}
