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
//! caches (a new material per frame, never freed). So the systems keep writing the
//! material directly — they just do it from the one crate that is allowed to name
//! one. Same reasoning, same shape as `terrain_maps.rs`.
//!
//! Ordering: both systems run in `Update` AFTER
//! `lunco_environment::horizon::finish_shadow_cache_bake`, preserving the original
//! single-crate `.chain()`.

use bevy::light::NotShadowCaster;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use bevy::camera::visibility::RenderLayers;
use lunco_environment::horizon::{
    finish_shadow_cache_bake, pick_sun, HorizonMap, HorizonShadowCache, HorizonShadowCacheConfig,
    SunQuery,
};
use crate::shader_material::ShaderMaterial;
use lunco_materials::ParamValue;

pub(crate) fn build(app: &mut App) {
    // `EnvironmentPlugin` also inits this (it drives the bake); `init_resource` is a
    // no-op when it is already there. Doing it here too means adding the render
    // plugin without the environment plugin cannot fail system-param validation.
    app.init_resource::<HorizonShadowCacheConfig>();
    app.add_systems(
        Update,
        (wire_terrain_materials, shade_dynamic_entities)
            .chain()
            .after(finish_shadow_cache_bake)
            // The bake half is gated on the asset stores existing; the material
            // half needs them too (plus the material assets, which are `Option`al
            // below so an app without `ShaderMaterialPlugin` degrades quietly).
            .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
    );
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

// ─────────────────────────────────────────────────────────────────────────
// Material wiring — heightfield + sun uniforms into the terrain shader
// ─────────────────────────────────────────────────────────────────────────

/// Keeps every horizon terrain's `ShaderMaterial` wired: heightfield
/// texture, static size/resolution, the per-frame sun direction, and the
/// **shadow cache** binding + `shadow_cache_on` flag.
/// A terrain with no authored shader gets the default `terrain_shadow.wgsl`
/// (albedo from its `displayColor`). Idempotent and self-healing against
/// later material swaps; steady-state cost is a uniform compare per terrain
/// (writes only when the sun moves or the cache swaps).
#[allow(clippy::type_complexity)]
pub fn wire_terrain_materials(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cfg: Res<HorizonShadowCacheConfig>,
    sun: SunQuery,
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
    // NOTE on the near-camera march fade (`csm_far`): the fade is a PERF
    // gate, not just cosmetics — inside it the live 48-step march is
    // skipped (CSM owns the near field), and "march everywhere" turned low
    // flight into a slideshow. (Streamed tiles DO get a baked cache on
    // native — `lunco-sandbox/src/terrain_horizon.rs` samples the oracle
    // into a `HorizonMap` and mirrors the cache to tiles — but the cache
    // fades in on the same `csm_far` boundary.) The cost of the fade is
    // that the CSM volume (~1.5 km) cannot contain multi-km ridge
    // occluders, so near terrain reads slightly lighter than the same
    // ground seen from altitude; the unconditional `SHADOW_FILL` in the
    // terrain shaders keeps that difference subtle.
    let to_sun_world: Vec3 = sun_gt.back().into();

    for (entity, terrain_gt, map, shadow_cache, shader_mat, std_mat) in &terrains {
        let sun_local = terrain_gt
            .affine()
            .inverse()
            .transform_vector3(to_sun_world)
            .normalize_or_zero();
        let hf_size_v = map.field.size();
        let hf_res = map.field.resolution() as f32;

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
        // Engage above ~0.01° elevation, release below ~0.003° — the band is
        // wider than any per-frame jitter, so the mode changes at most once
        // per real sunrise/sunset. The thresholds sit as LOW as possible: a
        // disengaged cache means every terrain pixel runs the 48-step march
        // per frame, and with the march now applied at ALL camera distances
        // (see `csm_far` above) a polar sun parked in a "cache off" band
        // turned whole sessions into a slideshow. Below the release
        // threshold the march's own below-horizon short-circuit is cheap.
        let engaged = {
            let prev = cache_engaged.get(&entity).copied().unwrap_or(false);
            let now = if prev { sun_local.y > 5.0e-5 } else { sun_local.y > 2.0e-4 };
            cache_engaged.insert(entity, now);
            now
        };
        let shadow_cache_on: f32 =
            if cfg.enabled && engaged && cache_image.is_some() { 1.0 } else { 0.0 };

        // Named engine uniforms consumed by the terrain shaders (regolith /
        // terrain_shadow declare these in their `Material` struct; the engine
        // packs them at the reflected offsets).
        let sun_dir = ParamValue::Vec3([sun_local.x, sun_local.y, sun_local.z]);
        // World-space to-sun for the BRDF opposition term. The march uses the
        // terrain-LOCAL `sun_dir` (heightfield space); the lunar BRDF runs in
        // world space (world N/V), so it needs the world-space sun. Passing the
        // CPU-picked canonical sun here means the shader never has to guess it
        // from `directional_lights[0]` — robust to the earthshine fill light.
        let sun_dir_world = ParamValue::Vec3([to_sun_world.x, to_sun_world.y, to_sun_world.z]);
        let hf_size = ParamValue::Vec2([hf_size_v.x, hf_size_v.y]);
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
                ("hf_res", ParamValue::F32(hf_res)),
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
                        .is_none_or(|v| (v.truncate() - sun_local).length() > 1e-4)
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
            // The terrain takes the SHADER path: drop any `PbrLook` intent with the
            // `StandardMaterial` it bound, or the mesh would carry two materials and
            // draw twice (the contract in `rebind_changed_pbr_look`).
            commands
                .entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .remove::<lunco_render::PbrLook>()
                .try_insert(MeshMaterial3d(handle));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Dynamic objects — darken by CPU-marched visibility
// ─────────────────────────────────────────────────────────────────────────

/// Minimum sun movement (cosine of angle) before objects are re-evaluated
/// — ~0.1°.
const SUN_EPSILON_COS: f32 = 0.999_998_5;

/// Scales a colour's linear RGB by `q`, keeping alpha.
fn scale_color(c: Color, q: f32) -> Color {
    let l = c.to_linear();
    Color::LinearRgba(LinearRgba::new(l.red * q, l.green * q, l.blue * q, l.alpha))
}

/// Fill floor for a horizon-shadowed body's albedo scale. `sun_vis` gates the
/// SUN, but the shadowless earthshine/ambient fill is ALWAYS present, so a body
/// in horizon shadow is dim — never a pure-black hole. Without it, a grazing sun
/// drives an occluded chassis's albedo to 0 (`scale_color(_, 0)`), so the whole
/// body reads black even though the terrain around it is fill-lit. Mirrors
/// `wheel.wgsl`'s `HORIZON_AMBIENT_FLOOR` for the ShaderMaterial (wheels) path.
const HORIZON_FILL_FLOOR: f32 = 0.22;

/// Runs every mesh entity's position through the same heightfield march the
/// terrain shader uses and darkens the entity by its sun visibility (see
/// `lunco_environment::horizon` §3). Change-driven: a full pass only when the
/// sun moved; otherwise only entities whose `GlobalTransform` changed.
#[allow(clippy::type_complexity)]
pub fn shade_dynamic_entities(
    mut commands: Commands,
    mut last_sun: Local<Option<Vec3>>,
    mut sweep_timer: Local<Option<Timer>>,
    time: Res<Time>,
    sun: SunQuery,
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
                .is_some_and(|m| m.get_scalar("sun_vis").is_none_or(|s| (s - q).abs() > 1e-3));
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
                            m.base_color = scale_color(original, q.max(HORIZON_FILL_FLOOR));
                            let unique = std_mats.add(m);
                            debug!("[horizon-dbg] {entity:?} {name:?} vis={q:.2} SHADE-NEW (std)");
                            commands.entity(entity).try_insert((
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
                            .try_insert(MeshMaterial3d(state.shared.clone()))
                            .remove::<HorizonShade>();
                        debug!("[horizon-dbg] {entity:?} {name:?} vis={q:.2} SHADE-CLEAR (std)");
                    } else if (state.last_vis - q).abs() > 1e-3 {
                        if let Some(mut m) = std_mats.get_mut(&handle.0) {
                            m.base_color = scale_color(state.original, q.max(HORIZON_FILL_FLOOR));
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
            commands.entity(entity).try_insert((NotShadowCaster, HorizonShadowed));
        } else if shadowed && vis > 0.65 {
            commands.entity(entity).remove::<(NotShadowCaster, HorizonShadowed)>();
        }
    }
}
