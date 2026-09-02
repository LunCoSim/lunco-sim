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
pub mod link_beams;
pub mod look_cache;
mod procedural_sky;
mod scene_camera;
mod sensor_beams;
mod shader_look;
pub mod shader_material;

mod world_label;

pub use shader_look::ShaderLookCache;
// The concrete custom-shader material + its render pipeline. It lived in
// `lunco-materials` until the render decoupling; that crate is now render-free and
// holds only the *intent* (`ShaderLook`) and the reflected schema. Export the
// concrete render type from this crate's public render façade so GUI binaries
// do not depend on the implementation module directly.
pub use shader_material::*;

use bevy::light::NotShadowCaster;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use lunco_render::{PbrLook, PbrLookKey, SurfaceAlpha};

/// Startup-only rendering policy selected by the desktop binary.
///
/// [`RenderProfile::Fast`] is deliberately a compatibility mode, not a second
/// appearance model: it keeps authored meshes and colours visible but replaces
/// ordinary PBR materials with texture-free unlit materials. GPU rendering still
/// requires Bevy's built-in shader; it avoids the expensive lighting, texture
/// sampling, HDR/bloom and MSAA paths rather than promising an impossible
/// "no-shader" renderer.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderProfile {
    #[default]
    Standard,
    Fast,
}

impl RenderProfile {
    pub const fn is_fast(self) -> bool {
        matches!(self, Self::Fast)
    }
}

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
/// scatter and the terrain LOD band lattice depend on it for batching.
///
/// It also hosts the render-only code that has **no intent form** and therefore had
/// to move here bodily rather than be expressed as a component:
/// - `horizon_shade` — the per-frame heightfield/sun *uniform feed* into the terrain
///   `ShaderMaterial` and the `StandardMaterial` darkening of shadowed props (from
///   `lunco-environment`);
/// - `env_light` — the `bloom` arm of `SetEnvironmentLight` (from `lunco-environment`);
/// Terrain-derived map selection remains render-free on the terrain's
/// `ShaderLook`; this crate only binds that intent to the generic GPU material.
///
/// **Screenshots deliberately do NOT live here** — they live in
/// `lunco_workbench::screenshot`. This crate is the 3D *material* binder, and `lunica`
/// takes screenshots without ever adding it; putting capture here would silently kill the
/// Modelica workbench's screenshots. The workbench is the smallest crate for which "this
/// binary can render something" is already true, and both GUI binaries add it.
///
/// A second `#[Command]` + observer once existed (in `lunco-avatar`, and briefly here) that
/// also spawned `Screenshot::primary_window()`; it was **unreachable dead code**. Gone. Do
/// not re-add it.
pub struct LuncoRenderPlugin;

impl Plugin for LuncoRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderProfile>()
            .init_resource::<lunco_render::CommunicationLineSettings>()
            .init_resource::<lunco_render::SceneBloomOverride>()
            .init_resource::<PbrLookCache>()
            // `PbrLook` derives `Reflect` but was never REGISTERED, which left it
            // invisible to every generic reflection surface in the codebase:
            // `get(id, "PbrLook.emissive.red")` from any scripting language, the
            // HTTP API's component reads and the MCP bridge all resolve a
            // component by short type path through `AppTypeRegistry`, and an
            // unregistered type is simply not there.
            //
            // That gap mattered the moment anything wanted to ASK what a surface
            // looks like rather than assert it. `PbrLook` is already the resolved
            // answer to exactly that question: the USD loader fills it by walking
            // the standard UsdShade chain — `material:binding` → `Material` →
            // `outputs:surface.connect` → `Shader` → `inputs:*`
            // (`lunco_usd_bevy::resolve_bound_shader`) — so the component holds the
            // scene's authored `UsdPreviewSurface` intent in typed, render-free
            // form. Registering it is what turns that from an internal detail into
            // a UNIVERSAL read surface: one line, no new verb, no per-language
            // adapter — anything that wants to know what a surface looks like asks the
            // component the loader already filled, rather than re-deriving it.
            .register_type::<PbrLook>()
            .add_observer(bind_pbr_look)
            .add_systems(
                Update,
                (
                    rebind_changed_pbr_look,
                    look_cache::sweep_look_cache::<PbrLook>,
                )
                    // Names the binders; carries no ordering rule. The despawn race
                    // that used to live here is solved a schedule up — the USD
                    // projector runs in `PreUpdate`. See `lunco_render::LookRebind`.
                    .in_set(lunco_render::LookRebind),
            );
        scene_camera::build(app);
        // Shadow filtering is a render policy, not a workbench concern. Attach it
        // when a camera enters the render graph so windowed and offscreen captures
        // use the same lunar terminator without either binary having a second
        // implementation or a per-frame sweep.
        app.add_observer(ensure_lunar_shadow_filtering);
        // Lights and transforms become connection targets, so a value the
        // simulation publishes reaches them through the ordinary port graph rather
        // than through a script that samples a port every tick.
        // `shader_look::build` first: it registers the `ShaderMaterial` + `Shader`
        // asset stores (idempotently), which `ShaderMaterialPlugin` needs in place
        // before it loads the shared WGSL modules through the `AssetServer`.
        if app.world().resource::<RenderProfile>().is_fast() {
            // Do not register the user-WGSL material pipeline in fast mode. The
            // fallback keeps those meshes visible with one shared unlit material;
            // terrain map and horizon wiring only feed that retired pipeline.
            shader_look::build_fast(app);
        } else {
            shader_look::build(app);
            procedural_sky::build(app);
            // The `ShaderMaterial` RENDER PIPELINE. Added here and ONLY here — it used
            // to be added by hand in `lunco-luncosim`'s UI plugin and `luncosim`'s main;
            // both were deleted when the material moved into this crate, because Bevy
            // panics on a duplicate plugin.
            app.add_plugins(shader_material::ShaderMaterialPlugin);
            horizon_shade::build(app);
            env_light::build(app);
        }
        world_label::build(app);
        sensor_beams::build(app);
        // Connectivity beams: runtime-spawned mesh, authored look, local Transform (no
        // gizmo, no GlobalTransform, no jitter). This is the only connectivity visual.
        link_beams::build(app);
    }
}

/// Apply the renderer's lunar shadow filter to every 3D camera, including
/// cameras created after startup by an asynchronously loaded USD scene.
fn ensure_lunar_shadow_filtering(
    add: On<Add, Camera3d>,
    profile: Res<RenderProfile>,
    cameras: Query<(), Without<bevy::light::ShadowFilteringMethod>>,
    mut commands: Commands,
) {
    if profile.is_fast() {
        return;
    }
    let entity = add.entity;
    if cameras.get(entity).is_ok() {
        commands
            .entity(entity)
            .try_insert(bevy::light::ShadowFilteringMethod::Gaussian);
    }
}

/// Shared `StandardMaterial` per distinct [`PbrLookKey`].
///
/// This is load-bearing for batching, not an optimisation afterthought: scattering
/// 6000 rocks that all look alike must cost ONE material and ONE bind group. The
/// pre-decoupling code achieved that by hand-threading a single `Handle` through
/// the scatter loop; the cache makes it automatic and impossible to forget.
///
/// Sharing, the `unshared` bypass, and eviction are
/// [`LookCache`](look_cache::LookCache)'s — shared with the `ShaderLook` binder, so
/// the two cannot drift apart on policy again (they already had: this one never
/// swept, and grew without bound).
type PbrLookCache = look_cache::LookCache<PbrLook>;

/// Records whether the material currently attached to a PBR look is private.
///
/// `PbrLook::unshared` can change after the initial USD projection (waypoint
/// visit tinting is one such session-only change). Without this binding state,
/// the first transition from a cached shared material to an unshared look would
/// mutate the shared asset in place and recolour every entity using that handle.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct PbrLookBinding {
    private: bool,
}

impl look_cache::CachedLook for PbrLook {
    type Key = PbrLookKey;
    type Material = StandardMaterial;

    fn look_key(&self) -> PbrLookKey {
        self.key()
    }
    fn is_unshared(&self) -> bool {
        self.unshared
    }
}

/// Bevy's `reflectance` for a given index of refraction.
///
/// Both parameterise the SAME physical quantity — the normal-incidence specular
/// reflectance F₀ — just on different curves:
///
/// ```text
/// Fresnel (USD):      F0 = ((1 - ior) / (1 + ior))²
/// Filament (Bevy):    F0 = 0.16 · reflectance²
/// ```
///
/// Equating them and solving gives the closed form below. It is a bijection, not an
/// approximation: `ior` 1.5 → `reflectance` 0.5, and both mean F₀ = 0.04, the 4%
/// dielectric every non-metal has. That is why this conversion is a no-op for
/// every existing look.
///
/// This function is the ONLY place in the workspace that knows Filament's curve —
/// which is correct, because the curve is a fact about *Bevy*, not about the
/// material. `PbrLook` carries the physics (`ior`); the backend adapter remaps.
///
/// Bevy's `reflectance` saturates at 1.0, which `ior` reaches at 2.33 — above a
/// diamond's 2.42 and far above anything in a lunar scene, so the clamp is a
/// boundary of Bevy's parameterisation, not a limit we impose.
fn bevy_reflectance_from_ior(ior: f32) -> f32 {
    (2.5 * (ior - 1.0) / (ior + 1.0)).clamp(0.0, 1.0)
}

/// Build the concrete `StandardMaterial` a look describes.
fn standard_material(look: &PbrLook, profile: RenderProfile) -> StandardMaterial {
    if profile.is_fast() {
        return StandardMaterial {
            // Flat colour keeps the authored scene legible without lighting,
            // texture sampling, normal maps, clearcoat or environment probes.
            base_color: Color::from(look.base_color),
            emissive: look.emissive,
            unlit: true,
            double_sided: look.double_sided,
            alpha_mode: match look.alpha {
                SurfaceAlpha::Opaque => AlphaMode::Opaque,
                SurfaceAlpha::Mask(t) => AlphaMode::Mask(t),
                SurfaceAlpha::Blend => AlphaMode::Blend,
                SurfaceAlpha::Add => AlphaMode::Add,
            },
            cull_mode: if look.double_sided {
                None
            } else {
                Some(bevy::render::render_resource::Face::Back)
            },
            ..default()
        };
    }

    StandardMaterial {
        base_color: Color::from(look.base_color),
        emissive: look.emissive,
        perceptual_roughness: look.perceptual_roughness,
        metallic: look.metallic,
        reflectance: bevy_reflectance_from_ior(look.ior),
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

/// Resolve a look to a handle. Sharing + the `unshared` bypass are
/// [`LookCache::resolve`](look_cache::LookCache::resolve)'s job; this only supplies
/// the build recipe.
fn material_for(
    look: &PbrLook,
    profile: RenderProfile,
    cache: &mut PbrLookCache,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    cache.resolve(look, materials, |look| standard_material(look, profile))
}

/// `On<Add, PbrLook>` — the moment intent appears, give it a material.
fn bind_pbr_look(
    add: On<Add, PbrLook>,
    looks: Query<&PbrLook>,
    profile: Res<RenderProfile>,
    mut cache: ResMut<PbrLookCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    let handle = material_for(look, *profile, &mut cache, &mut materials);

    let mut ec = commands.entity(e);
    // Keep the concrete render material exclusive too. Although callers must
    // replace the render-free look, a late observer can otherwise leave an old
    // WGSL material beside this StandardMaterial and submit the same mesh twice.
    // `try_remove` for the same reason as the `try_insert` beside it: the entity may
    // already be despawned when the deferred command runs, and a plain `remove` turns
    // that into a formatted error through the command error handler every frame.
    ec.try_remove::<MeshMaterial3d<ShaderMaterial>>();
    ec.try_insert(MeshMaterial3d(handle));
    ec.try_insert(PbrLookBinding {
        private: look.unshared,
    });
    if look.no_shadow_cast {
        ec.try_insert(NotShadowCaster);
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
    changed: Query<
        (
            Entity,
            &PbrLook,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&PbrLookBinding>,
        ),
        Changed<PbrLook>,
    >,
    profile: Res<RenderProfile>,
    mut cache: ResMut<PbrLookCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (e, look, current, binding) in &changed {
        if look.unshared {
            if binding.is_some_and(|binding| binding.private) {
                // Private material: overwrite the asset it already owns.
                if let Some(mut existing) = current.and_then(|m| materials.get_mut(&m.0)) {
                    *existing = standard_material(look, *profile);
                    apply_shadow_flag(&mut commands, e, look);
                    continue;
                }
            }
            // The look just became unshared. Its current handle may still be a
            // cache-owned shared material, so allocate a fresh private asset
            // before any tint is applied. Mutating `current` here would recolour
            // every entity that shares the original handle.
            let handle = material_for(look, *profile, &mut cache, &mut materials);
            commands
                .entity(e)
                .try_remove::<MeshMaterial3d<ShaderMaterial>>()
                .try_insert((MeshMaterial3d(handle), PbrLookBinding { private: true }));
            apply_shadow_flag(&mut commands, e, look);
            continue;
        }
        let handle = material_for(look, *profile, &mut cache, &mut materials);
        // `try_insert`, not `insert`. The USD projector's despawns can no longer
        // race this (it runs in `PreUpdate`), but `ClearScene` and the preview
        // viewport still despawn entities *within* `Update`, and Bevy's deferred
        // commands make "queued insert on an entity despawned later this frame" a
        // real state. `try_insert` is Bevy's answer to exactly that; here it is
        // correct rather than a cover-up — the entity is genuinely gone, so there
        // is nothing to render and nothing to lose.
        commands
            .entity(e)
            .try_remove::<MeshMaterial3d<ShaderMaterial>>()
            .try_insert((MeshMaterial3d(handle), PbrLookBinding { private: false }));
        apply_shadow_flag(&mut commands, e, look);
    }
}

fn apply_shadow_flag(commands: &mut Commands, e: Entity, look: &PbrLook) {
    // `try_insert` for the same reason as the caller: the entity can be despawned
    // (a live-edit subtree rebuild) before this command buffer applies. `remove`
    // on a despawned entity is already a no-op.
    let mut ec = commands.entity(e);
    if look.no_shadow_cast {
        ec.try_insert(NotShadowCaster);
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
        let ids: Vec<Entity> = (0..64)
            .map(|_| app.world_mut().spawn(look.clone()).id())
            .collect();
        app.update();

        let handles: Vec<_> = ids
            .iter()
            .map(|&e| {
                app.world()
                    .entity(e)
                    .get::<MeshMaterial3d<StandardMaterial>>()
                    .unwrap()
                    .0
                    .clone()
            })
            .collect();
        assert!(
            handles.windows(2).all(|w| w[0] == w[1]),
            "64 identical looks must share one handle"
        );
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 1);
    }

    /// Two different looks must NOT collide into one material.
    #[test]
    fn different_looks_get_different_materials() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);

        app.world_mut()
            .spawn(PbrLook::matte(LinearRgba::rgb(1.0, 0.0, 0.0)));
        app.world_mut()
            .spawn(PbrLook::matte(LinearRgba::rgb(0.0, 1.0, 0.0)));
        app.update();

        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);
    }

    #[test]
    fn shared_material_is_detached_before_an_unshared_tint() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);

        let green = PbrLook::matte(LinearRgba::rgb(0.12, 0.72, 0.34));
        let reached = app.world_mut().spawn(green.clone()).id();
        let unreached = app.world_mut().spawn(green).id();
        app.update();

        let shared = app
            .world()
            .entity(reached)
            .get::<MeshMaterial3d<StandardMaterial>>()
            .expect("initial PBR binding")
            .0
            .clone();
        assert_eq!(
            shared,
            app.world()
                .entity(unreached)
                .get::<MeshMaterial3d<StandardMaterial>>()
                .expect("second initial PBR binding")
                .0
        );

        let mut tinted = app.world_mut().get_mut::<PbrLook>(reached).unwrap();
        tinted.base_color = LinearRgba::rgb(0.38, 0.38, 0.38);
        tinted.unshared = true;
        app.update();

        let reached_handle = app
            .world()
            .entity(reached)
            .get::<MeshMaterial3d<StandardMaterial>>()
            .unwrap()
            .0
            .clone();
        let unreached_handle = app
            .world()
            .entity(unreached)
            .get::<MeshMaterial3d<StandardMaterial>>()
            .unwrap()
            .0
            .clone();
        assert_ne!(reached_handle, unreached_handle);
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&unreached_handle)
                .unwrap()
                .base_color,
            Color::from(LinearRgba::rgb(0.12, 0.72, 0.34))
        );
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&reached_handle)
                .unwrap()
                .base_color,
            Color::from(LinearRgba::rgb(0.38, 0.38, 0.38))
        );
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

    /// A late PBR projection supersedes an existing WGSL material instead of
    /// leaving two render pipelines attached to one mesh.
    #[test]
    fn pbr_look_replaces_a_preexisting_shader_material() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);
        let shader = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        let e = app
            .world_mut()
            .spawn((
                MeshMaterial3d(shader),
                PbrLook::matte(LinearRgba::rgb(0.2, 0.2, 0.2)),
            ))
            .id();

        app.update();

        let entity = app.world().entity(e);
        assert!(entity.contains::<MeshMaterial3d<StandardMaterial>>());
        assert!(
            !entity.contains::<MeshMaterial3d<ShaderMaterial>>(),
            "the PBR material must replace, not overlay, the shader material"
        );
    }

    fn app_with_n_distinct_looks(n: usize) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .add_plugins(LuncoRenderPlugin);
        let ids = (0..n)
            .map(|i| {
                // Distinct base colours ⇒ distinct keys ⇒ one cache entry each.
                let c = LinearRgba::rgb(i as f32 / n as f32, 0.5, 0.5);
                app.world_mut().spawn(PbrLook::matte(c)).id()
            })
            .collect();
        app.update();
        (app, ids)
    }

    /// The PBR cache used to grow WITHOUT BOUND — it had no sweep at all, while the
    /// shader cache next door swept at 1024. Both now share one policy. A scene swap
    /// (despawn everything) must reclaim the dead entries once the cache is
    /// implausibly large, or every dead look keeps pinning its textures forever.
    #[test]
    fn sweep_reclaims_entries_no_live_look_refers_to() {
        let (mut app, ids) = app_with_n_distinct_looks(1100);
        assert_eq!(app.world().resource::<PbrLookCache>().len(), 1100);

        for e in ids {
            app.world_mut().entity_mut(e).despawn();
        }
        app.update();

        assert_eq!(
            app.world().resource::<PbrLookCache>().len(),
            0,
            "no look is live, so the sweep must drop every cached material"
        );
    }

    /// …and BELOW the threshold it must not run: a steady scene pays nothing, and a
    /// look that is momentarily unspawned (a tile between LOD bands, a reloading
    /// prim) must still find its material cached when it comes back.
    #[test]
    fn sweep_does_not_run_below_the_threshold() {
        let (mut app, ids) = app_with_n_distinct_looks(10);
        for e in ids {
            app.world_mut().entity_mut(e).despawn();
        }
        app.update();

        assert_eq!(
            app.world().resource::<PbrLookCache>().len(),
            10,
            "under the sweep threshold the cache is retained for reuse"
        );
    }

    /// The IOR → reflectance remap must agree with Fresnel, because the two are the
    /// same physical quantity (F₀) on different curves — not an artistic choice.
    ///
    /// The anchor that matters: USD's default `ior` 1.5 must land exactly on Bevy's
    /// default `reflectance` 0.5. Both mean F₀ = 0.04, the 4% dielectric. That
    /// equality is *why* dropping the separate `reflectance` field re-looks nothing.
    #[test]
    fn ior_maps_to_bevy_reflectance_via_fresnel() {
        // F₀ from Fresnel (USD) vs from Filament's remap (Bevy) — same number.
        let f0_fresnel = |ior: f32| ((1.0 - ior) / (1.0 + ior)).powi(2);
        let f0_filament = |r: f32| 0.16 * r * r;

        for ior in [1.0f32, 1.2, 1.5, 1.8, 2.33] {
            let r = bevy_reflectance_from_ior(ior);
            assert!(
                (f0_filament(r) - f0_fresnel(ior)).abs() < 1e-6,
                "ior {ior} → reflectance {r}: F0 {} != Fresnel {}",
                f0_filament(r),
                f0_fresnel(ior),
            );
        }

        // The defaults coincide, and a vacuum reflects nothing.
        assert!((bevy_reflectance_from_ior(1.5) - 0.5).abs() < 1e-6);
        assert!(bevy_reflectance_from_ior(1.0).abs() < 1e-6);
        assert!((f0_fresnel(1.5) - 0.04).abs() < 1e-6);

        // Above Bevy's parameterisation ceiling the remap saturates rather than
        // producing a reflectance > 1.
        assert_eq!(bevy_reflectance_from_ior(3.0), 1.0);
    }

    /// `PbrLook::default()` must still produce Bevy's own `StandardMaterial` defaults.
    /// This is the regression guard for the field deletion: if the remap ever drifts,
    /// every material in the workspace silently changes its specular response.
    #[test]
    fn default_look_keeps_bevy_default_reflectance() {
        let m = standard_material(&PbrLook::default(), RenderProfile::Standard);
        assert!((m.reflectance - StandardMaterial::default().reflectance).abs() < 1e-6);
        assert!((m.ior - StandardMaterial::default().ior).abs() < 1e-6);
    }

    #[test]
    fn fast_profile_uses_unlit_texture_free_materials() {
        let mut look = PbrLook::matte(LinearRgba::rgb(0.2, 0.3, 0.4));
        look.textures.base_color = Some(Handle::default());
        look.textures.emissive = Some(Handle::default());
        look.textures.metallic_roughness = Some(Handle::default());
        look.textures.normal_map = Some(Handle::default());
        look.textures.occlusion = Some(Handle::default());

        let material = standard_material(&look, RenderProfile::Fast);
        assert!(material.unlit);
        assert!(material.base_color_texture.is_none());
        assert!(material.emissive_texture.is_none());
        assert!(material.metallic_roughness_texture.is_none());
        assert!(material.normal_map_texture.is_none());
        assert!(material.occlusion_texture.is_none());
    }
}
