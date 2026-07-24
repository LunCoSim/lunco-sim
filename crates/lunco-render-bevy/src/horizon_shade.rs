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
        (
            wire_terrain_materials,
            wire_sun_for_non_terrain_materials,
            shade_dynamic_entities,
        )
            .chain()
            .after(finish_shadow_cache_bake)
            // The bake half is gated on the asset stores existing; the material
            // half needs them too (plus the material assets, which are `Option`al
            // below so an app without `ShaderMaterialPlugin` degrades quietly).
            .run_if(resource_exists::<Assets<Image>>.and_then(resource_exists::<Assets<Mesh>>)),
    );
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
            Option<&HorizonMap>,
            Option<&HorizonShadowCache>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
        ),
        (
            With<lunco_terrain_surface::DemTerrainSurface>,
            Without<RenderLayers>,
        ),
    >,
    // Hysteresis state for the cache↔march handoff, per terrain (see below).
    mut cache_engaged: Local<std::collections::HashMap<Entity, bool>>,
    mut removed_terrains: RemovedComponents<HorizonMap>,
) {
    for e in removed_terrains.read() {
        cache_engaged.remove(&e);
    }
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
    // ground seen from altitude; the `shadow_fill` term in the terrain
    // shaders keeps that difference subtle. That fill gates itself on
    // `hf_res` being non-zero — i.e. on THIS function having wired the surface,
    // which it does only for entities carrying a `HorizonMap`. So the fill
    // reaches horizon terrain only and can never leak onto a plain mesh that
    // merely shares the shader family (`regolith.wgsl` is also bound directly
    // by scenes as a studio ground material).
    let to_sun_world: Vec3 = sun_gt.back().into();

    for (entity, terrain_gt, map, shadow_cache, shader_mat, std_mat) in &terrains {
        let sun_local = terrain_gt
            .affine()
            .inverse()
            .transform_vector3(to_sun_world)
            .normalize_or_zero();
        let (hf_size_v, hf_res, height_map_handle) = match map {
            Some(m) => (m.field.size(), m.field.resolution() as f32, Some(m.image.clone())),
            None => (Vec2::ONE, 0.0, None),
        };

        // Shadow cache binding + the uniform flag that tells the fragment
        // shader to sample it (`1.0`) instead of ray-marching (`0.0`). The
        // handle is bound whenever a cache exists (it stays allocated on the
        // `HorizonShadowCache` component regardless); only the flag toggles —
        // cheap uniform write, no bind-group churn — when the sun dips below
        // the horizon or the cache is disabled. Below-horizon sun falls back
        // to the march, which short-circuits to 0 in its first branch.
        let cache_image: Option<Handle<Image>> = shadow_cache.map(|c| c.image.clone());
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
            if m.height_map != height_map_handle {
                m.height_map = height_map_handle.clone();
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
            //
            // `sun_dir` compares via `get_vec3`, NOT `get_vec4`. It is written as a
            // `Vec3` (see `write_engine`) and `get_vec4` matches only
            // `ParamValue::Vec4` — so it answered `None` for a value that was
            // present and correct, `needs` was permanently true, and EVERY terrain
            // material was re-uploaded every frame.
            //
            // EPSILON, not exact equality, for the same reason the scalars beside it
            // use one. An exact compare is only quiet while the sun is BIT-identical
            // frame to frame — true for a parked sun, false the moment the celestial
            // clock runs, and then every terrain material repacks every frame again:
            // the original cost, re-entered through a different door. At the lunar
            // rate (360° / 29.5 d) `SUN_DIR_EPSILON` coalesces the write to roughly
            // once every ten seconds, and the direction error it tolerates (~0.006°)
            // is far below anything a shadow direction can show.
            let needs = shader_mats.get(&handle.0).is_some_and(|m| {
                m.height_map != height_map_handle
                    || m.shadow_cache != cache_image
                    || m.get_scalar("shadow_cache_on").is_none_or(|s| (s - shadow_cache_on).abs() > 1e-3)
                    || m.get_vec3("sun_dir").is_none_or(|v| (v - sun_local).length() > SUN_DIR_EPSILON)
                    || m.get_vec3("sun_dir_world").is_none_or(|v| (v - to_sun_world).length() > SUN_DIR_EPSILON)
                    || m.get_scalar("hf_res").is_none_or(|r| (r - hf_res).abs() > 1e-3)
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
                height_map: height_map_handle.clone(),
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

/// Fill `sun_dir_world` on every OTHER `ShaderMaterial` — the ones with no
/// `HorizonMap` behind them.
///
/// [`wire_terrain_materials`] only sees genuine heightfield terrain, but
/// `regolith.wgsl` is bound to ordinary meshes too (the landing pad disc, the
/// marketing scenes' ground plate). On those the uniform kept its zero default,
/// and the shader covered for it by picking the brightest directional light —
/// a GUESS that is exact only while the brightest light happens to be the sun,
/// and silent when it is not. A scene with a bright artificial fill keyed the
/// whole lunar BRDF off the wrong vector with nothing in the log to say so.
///
/// The sun is a scene-global fact, so the fix is to write it everywhere rather
/// than let each shader re-derive it. Running across every non-terrain
/// `ShaderMaterial` is safe: a name the shader does not declare is kept in the
/// material's `values` map but has no schema offset, so `repack()` never packs it
/// into the uniform block — it costs a map entry and reaches no GPU binding.
///
/// Terrain is EXCLUDED (`Without<HorizonMap>`) — it is already written above,
/// with the local-space `sun_dir` this system has no business computing.
pub fn wire_sun_for_non_terrain_materials(
    sun: SunQuery,
    shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    meshes: Query<&MeshMaterial3d<ShaderMaterial>, (Without<HorizonMap>, Without<RenderLayers>)>,
) {
    let Some(mut shader_mats) = shader_mats else { return };
    let Some((sun_gt, tan_r, _csm_far)) = pick_sun(&sun) else { return };
    let to_sun_world: Vec3 = sun_gt.back().into();
    let sun_dir_world = ParamValue::Vec3([to_sun_world.x, to_sun_world.y, to_sun_world.z]);

    for handle in &meshes {
        // Compare before `get_mut`, or every frame re-uploads the asset (MAT-3).
        //
        // Compare via `get_vec3`, not `get_vec4`. `sun_dir_world` is written as a
        // `Vec3`, and `get_vec4` matches only `ParamValue::Vec4` — so it answers
        // `None` for a value that is present and correct, `needs` is always true,
        // and the asset is re-uploaded every frame. `SUN_DIR_EPSILON` for the same
        // reason as the terrain path above: an exact compare only stays quiet while
        // the sun is parked, and re-enters the per-frame repack as soon as the
        // celestial clock moves it.
        let needs = shader_mats.get(&handle.0).is_some_and(|m| {
            m.get_vec3("sun_dir_world")
                .is_none_or(|v| (v - to_sun_world).length() > SUN_DIR_EPSILON)
        });
        if needs {
            if let Some(mut m) = shader_mats.get_mut(&handle.0) {
                m.set_many([
                    ("sun_dir_world", sun_dir_world),
                    ("sun_tan_radius", ParamValue::F32(tan_r)),
                ]);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Dynamic objects — darken by CPU-marched visibility
// ─────────────────────────────────────────────────────────────────────────

/// Minimum sun movement (cosine of angle) before objects are re-evaluated
/// — ~0.1°.
const SUN_EPSILON_COS: f32 = 0.999_998_5;

/// Minimum change in a stored sun DIRECTION before the material carrying it is
/// repacked. On a unit vector this is ~0.006° — three orders of magnitude finer
/// than [`SUN_EPSILON_COS`]'s visibility re-evaluation, because this one only
/// has to be below "a shadow direction anyone can see", not below "the horizon
/// answer changed". Its job is to keep a *continuously* moving sun from
/// repacking every terrain material every frame.
const SUN_DIR_EPSILON: f32 = 1e-4;

/// Runs every mesh entity's position through the same heightfield march the
/// terrain shader uses and darkens the entity by its sun visibility (see
/// `lunco_environment::horizon` §3). Change-driven: a full pass only when the
/// sun moved; otherwise only entities whose `GlobalTransform` changed.
#[allow(clippy::type_complexity)]
pub fn shade_dynamic_entities(
    mut last_sun: Local<Option<Vec3>>,
    mut sweep_timer: Local<Option<Timer>>,
    time: Res<Time>,
    sun: SunQuery,
    terrains: Query<(&GlobalTransform, &HorizonMap), Without<RenderLayers>>,
    mut shader_mats: Option<ResMut<Assets<ShaderMaterial>>>,
    _std_mats: ResMut<Assets<StandardMaterial>>,
    mut entities: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            Has<RenderLayers>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
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

    for (_entity, gt, has_layers, shader_mat, std_mat, _name) in &mut entities {
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
        } else if let Some(_handle) = std_mat {
            // StandardMaterials (chassis, props): retain authored base_color so
            // textures are not crushed or darkened when horizon maps initialize.
            // Directional sun light and CSM shadows handle real-time GPU lighting.
        }

    }
}

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
        app.add_systems(Update, wire_sun_for_non_terrain_materials);
        app
    }

    /// A `ShaderMaterial` on a mesh with NO `HorizonMap` must still get the sun.
    ///
    /// This is the contract that replaced `regolith.wgsl`'s `sun_to_light()` guess.
    /// `wire_terrain_materials` only sees heightfield terrain, so the landing pad
    /// disc and the marketing ground plate used to keep a zero `sun_dir_world` and
    /// the shader silently substituted the brightest directional light. If this
    /// test fails, those surfaces lose the lunar BRDF entirely — there is no
    /// fallback behind it any more, by design.
    #[test]
    fn a_non_terrain_shader_material_gets_the_sun() {
        let mut app = test_app();

        // Sun: identity rotation ⇒ `GlobalTransform::back()` is +Z.
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight { illuminance: 10_000.0, ..Default::default() },
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

    /// The brightest directional light wins — an earthshine fill must not be
    /// mistaken for the sun. Mirrors `pick_sun`'s rule.
    #[test]
    fn the_brightest_light_is_the_sun() {
        let mut app = test_app();

        // Dim fill pointing +Z, bright sun pointing +X. Brightness, not order, decides.
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight { illuminance: 10.0, ..Default::default() },
        ));
        app.world_mut().spawn((
            GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(
                std::f32::consts::FRAC_PI_2,
            ))),
            DirectionalLight { illuminance: 100_000.0, ..Default::default() },
        ));

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
        assert!(v[0] > 0.99, "expected the BRIGHT light's +X direction, got {v:?}");
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
            DirectionalLight { illuminance: 10_000.0, ..Default::default() },
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
            app.world().resource::<Assets<ShaderMaterial>>().get(&handle).map(|m| m.get("sun_dir_world")),
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
        let sun = app
            .world_mut()
            .spawn((
                GlobalTransform::IDENTITY,
                DirectionalLight { illuminance: 10_000.0, ..Default::default() },
            ))
            .id();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));

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
            *app.world_mut().entity_mut(sun).get_mut::<GlobalTransform>().unwrap() =
                GlobalTransform::from(Transform::from_rotation(rot));
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

    /// Terrain is excluded: `wire_terrain_materials` owns it, and this system has
    /// no business computing the heightfield-local `sun_dir` it also needs.
    #[test]
    fn terrain_is_left_to_the_terrain_wiring() {
        let mut app = test_app();
        app.world_mut().spawn((
            GlobalTransform::IDENTITY,
            DirectionalLight { illuminance: 10_000.0, ..Default::default() },
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
            HorizonMap { field, image: Handle::default() },
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
