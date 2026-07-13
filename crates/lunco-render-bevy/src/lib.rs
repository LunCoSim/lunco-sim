//! The `bevy_pbr` binding layer — the one crate that turns appearance *intent*
//! into a real material.
//!
//! Domain crates spawn `Mesh3d` + a [`lunco_render::PbrLook`] and stop. This crate
//! observes the intent and inserts `MeshMaterial3d<StandardMaterial>`. It is the
//! **only** crate in the domain graph that depends on `bevy_pbr`, which is what
//! keeps `bevy_render` → wgpu + naga out of the `--no-ui` server and the wasm
//! worker.
//!
//! Headless does not add [`LuncoRenderPlugin`]. That is the entire gate — there is
//! no `#[cfg(feature = "render")]` anywhere in the simulation crates.
//!
//! See `docs/architecture/render-decoupling.md`.

mod env_light;
pub mod horizon_shade;
mod scene_camera;
mod sensor_beams;
pub mod shader_material;
mod shader_look;
mod terrain_maps;
mod world_label;

pub use shader_look::ShaderLookCache;
// The concrete custom-shader material + its render pipeline. It lived in
// `lunco-materials` until the render decoupling; that crate is now render-free and
// holds only the *intent* (`ShaderLook`) and the reflected schema. Re-exported here
// so the GUI binaries (and anything else that legitimately needs the concrete type
// in a RENDER build) can still reach it.
pub use shader_material::*;

use bevy::light::NotShadowCaster;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lunco_render::{PbrLook, PbrLookKey, SurfaceAlpha};

/// Binds appearance intent to concrete materials. Add this in render builds; omit
/// it headless.
///
/// Three kinds of intent, three binders:
/// - [`PbrLook`] → `MeshMaterial3d<StandardMaterial>` (below) — a plain surface;
/// - [`lunco_materials::ShaderLook`] → `MeshMaterial3d<ShaderMaterial>`
///   (`shader_look`) — a custom `.wgsl` with an open, user-defined parameter set;
/// - [`lunco_render::SceneCamera`] → `Camera3d` + tonemapping + MSAA + bloom
///   (`scene_camera`) — because `Camera3d` was being used as the *query filter* for
///   "which entity is the scene camera", which made domain crates link a GPU stack
///   just to ask a question.
///
/// The look binders cache by *content*, so identical looks share one material and
/// one bind group. That sharing is not an optimisation afterthought: the rock
/// scatter and the terrain LOD band/reveal lattice depend on it for batching.
///
/// It also hosts the render-only code that has **no intent form** and therefore had
/// to move here bodily rather than be expressed as a component:
/// - `horizon_shade` — the per-frame heightfield/sun *uniform feed* into the terrain
///   `ShaderMaterial` and the `StandardMaterial` darkening of shadowed props (from
///   `lunco-environment`);
/// - `env_light` — the `bloom` arm of `SetEnvironmentLight` (from `lunco-environment`);
/// - `terrain_maps` — the derived-layer bind onto the async USD terrain material.
///
/// **Screenshots deliberately do NOT live here.** `CaptureScreenshot` has exactly one
/// implementation, in `lunco-api::executor` — which has to own it, because raw-PNG
/// mode defers the HTTP response until `ScreenshotCaptured` fires. A second
/// `#[Command]` + observer once existed (in `lunco-avatar`, and briefly here) that
/// also spawned `Screenshot::primary_window()`; it was **unreachable dead code**,
/// because `execute_request` matches the command by name and returns early. It is
/// gone. Do not re-add it.
pub struct LuncoRenderPlugin;

impl Plugin for LuncoRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PbrLookCache>()
            .add_observer(bind_pbr_look)
            .add_systems(Update, rebind_changed_pbr_look);
        scene_camera::build(app);
        // `shader_look::build` first: it registers the `ShaderMaterial` + `Shader`
        // asset stores (idempotently), which `ShaderMaterialPlugin` needs in place
        // before it loads the shared WGSL modules through the `AssetServer`.
        shader_look::build(app);
        // The `ShaderMaterial` RENDER PIPELINE. Added here and ONLY here — it used
        // to be added by hand in `lunco-sandbox`'s UI plugin and `luncosim`'s main;
        // both were deleted when the material moved into this crate, because Bevy
        // panics on a duplicate plugin.
        app.add_plugins(shader_material::ShaderMaterialPlugin);
        terrain_maps::build(app);
        horizon_shade::build(app);
        env_light::build(app);
        world_label::build(app);
        sensor_beams::build(app);
    }
}

/// Shared `StandardMaterial` per distinct [`PbrLookKey`].
///
/// This is load-bearing for batching, not an optimisation afterthought: scattering
/// 6000 rocks that all look alike must cost ONE material and ONE bind group. The
/// pre-decoupling code achieved that by hand-threading a single `Handle` through
/// the scatter loop; the cache makes it automatic and impossible to forget.
#[derive(Resource, Default)]
struct PbrLookCache(HashMap<PbrLookKey, Handle<StandardMaterial>>);

/// Build the concrete `StandardMaterial` a look describes.
fn standard_material(look: &PbrLook) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::from(look.base_color),
        emissive: look.emissive,
        perceptual_roughness: look.perceptual_roughness,
        metallic: look.metallic,
        reflectance: look.reflectance,
        ior: look.ior,
        clearcoat: look.clearcoat,
        clearcoat_perceptual_roughness: look.clearcoat_perceptual_roughness,
        specular_tint: Color::from(look.specular_tint),
        unlit: look.unlit,
        double_sided: look.double_sided,
        alpha_mode: match look.alpha {
            SurfaceAlpha::Opaque => AlphaMode::Opaque,
            SurfaceAlpha::Mask(t) => AlphaMode::Mask(t),
            SurfaceAlpha::Blend => AlphaMode::Blend,
            SurfaceAlpha::Add => AlphaMode::Add,
        },
        base_color_texture: look.textures.base_color.clone(),
        emissive_texture: look.textures.emissive.clone(),
        metallic_roughness_texture: look.textures.metallic_roughness.clone(),
        normal_map_texture: look.textures.normal_map.clone(),
        occlusion_texture: look.textures.occlusion.clone(),
        // A double-sided material with default culling renders its back faces
        // unlit-black; Bevy wants culling off too.
        cull_mode: if look.double_sided {
            None
        } else {
            Some(bevy::render::render_resource::Face::Back)
        },
        ..default()
    }
}

/// Resolve a look to a handle.
///
/// Shared looks go through the content-keyed cache. **`unshared` looks bypass it
/// entirely** and get a private material — which is what keeps an ANIMATED look (a
/// USD `displayColor` sweep, a pulsing highlight) from re-keying the cache every
/// frame and minting a material per distinct value that is never freed. That leak
/// presents as a slow memory climb, not an obvious bug, so the opt-out is explicit
/// rather than inferred.
fn material_for(
    look: &PbrLook,
    cache: &mut PbrLookCache,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    if look.unshared {
        return materials.add(standard_material(look));
    }
    cache
        .0
        .entry(look.key())
        .or_insert_with(|| materials.add(standard_material(look)))
        .clone()
}

/// `On<Add, PbrLook>` — the moment intent appears, give it a material.
fn bind_pbr_look(
    add: On<Add, PbrLook>,
    looks: Query<&PbrLook>,
    mut cache: ResMut<PbrLookCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    let handle = material_for(look, &mut cache, &mut materials);

    let mut ec = commands.entity(e);
    ec.insert(MeshMaterial3d(handle));
    if look.no_shadow_cast {
        ec.insert(NotShadowCaster);
    }
}

/// Re-bind when a look is edited in place (the Inspector, a script, a USD reload).
///
/// Change-driven: `Changed<PbrLook>` only, so a static scene costs nothing.
///
/// **Animated (`unshared`) looks are MUTATED IN PLACE**, not re-added. Adding a new
/// material on every change would leak one per frame — the same trap the cache
/// bypass exists to close, just moved one system along.
///
/// **Contract for callers:** an entity must not carry `PbrLook` and a custom-shader
/// material at the same time. A system that takes over an entity's shading (e.g.
/// `lunco-usd-sim`'s `apply_usd_shader_materials`) must `remove::<PbrLook>()`, not
/// merely replace the material — otherwise this system re-inserts
/// `MeshMaterial3d<StandardMaterial>` alongside the shader material and the mesh
/// draws twice.
fn rebind_changed_pbr_look(
    changed: Query<(Entity, &PbrLook, Option<&MeshMaterial3d<StandardMaterial>>), Changed<PbrLook>>,
    mut cache: ResMut<PbrLookCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (e, look, current) in &changed {
        if look.unshared {
            // Private material: overwrite the asset it already owns.
            if let Some(mut existing) = current.and_then(|m| materials.get_mut(&m.0)) {
                *existing = standard_material(look);
                apply_shadow_flag(&mut commands, e, look);
                continue;
            }
        }
        let handle = material_for(look, &mut cache, &mut materials);
        commands.entity(e).insert(MeshMaterial3d(handle));
        apply_shadow_flag(&mut commands, e, look);
    }
}

fn apply_shadow_flag(commands: &mut Commands, e: Entity, look: &PbrLook) {
    let mut ec = commands.entity(e);
    if look.no_shadow_cast {
        ec.insert(NotShadowCaster);
    } else {
        ec.remove::<NotShadowCaster>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole cache exists for: N entities with the same look must
    /// share ONE material. If this regresses, batching dies and the draw-call count
    /// goes linear in the rock count.
    #[test]
    fn identical_looks_share_one_material() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);

        let look = PbrLook::matte(LinearRgba::rgb(0.22, 0.21, 0.20));
        let ids: Vec<Entity> = (0..64).map(|_| app.world_mut().spawn(look.clone()).id()).collect();
        app.update();

        let handles: Vec<_> = ids
            .iter()
            .map(|&e| app.world().entity(e).get::<MeshMaterial3d<StandardMaterial>>().unwrap().0.clone())
            .collect();
        assert!(handles.windows(2).all(|w| w[0] == w[1]), "64 identical looks must share one handle");
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 1);
    }

    /// Two different looks must NOT collide into one material.
    #[test]
    fn different_looks_get_different_materials() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);

        app.world_mut().spawn(PbrLook::matte(LinearRgba::rgb(1.0, 0.0, 0.0)));
        app.world_mut().spawn(PbrLook::matte(LinearRgba::rgb(0.0, 1.0, 0.0)));
        app.update();

        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);
    }

    /// `no_shadow_cast` must reach the render world as `NotShadowCaster` — the
    /// terrain/rock shadow saving depends on it.
    #[test]
    fn no_shadow_cast_inserts_not_shadow_caster() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);

        let e = app.world_mut().spawn(PbrLook::default().no_shadows()).id();
        app.update();
        assert!(app.world().entity(e).contains::<NotShadowCaster>());
    }
}
