//! The `ShaderLook` → `ShaderMaterial` binder — the custom-shader half of the
//! render boundary.
//!
//! [`lunco_render::PbrLook`] covers a plain PBR surface; a *custom shader* look is
//! open-ended (the parameter set belongs to the `.wgsl`, not to Rust), so domain
//! crates state it as [`lunco_materials::ShaderLook`] — a shader **path**, a
//! `BTreeMap` of named [`ParamValue`](lunco_materials::ParamValue)s, and named
//! [`TextureLayer`]s. Neither the path nor `Handle<Image>` touches `bevy_pbr`, so
//! the crate that authors the look (the terrain streamer, notably) links no GPU
//! stack. This module is where it becomes a real `ShaderMaterial`.
//!
//! # The cache is load-bearing
//!
//! [`ShaderLookCache`] maps [`ShaderLookKey`] → one `Handle<ShaderMaterial>`. The
//! terrain LOD path depends on it: the ~150–500 resident tiles collapse onto a
//! handful of distinct looks (mode x morph-band bucket), and they
//! MUST resolve to the same material — one bind group, one batch. This is exactly
//! the hand-rolled `LodMaterials`/`MatKey` cache the terrain used to carry, done
//! once, generically, keyed by the look's own content.
//!
//! Shared cached materials stay structurally immutable after they are built: a
//! tile that changes (an overlay re-tune, a late-bound derived map) edits its
//! `ShaderLook`, and the binder swaps the *handle* to another cached material.
//! Only the explicitly `unshared` hot path and live parameter updates mutate an
//! asset in place; structural shared-material changes do not cause a per-tile
//! repack or an `AssetEvent` storm.
//!
//! The schema (parameter name → std140 offset, reflected out of the WGSL) is
//! filled in by [`reflect_shader_schemas`](crate::reflect_shader_schemas) once the shader
//! source loads; a freshly built material carries the empty schema and its values
//! by name, and is repacked the moment the schema lands. That machinery is
//! untouched.

use crate::look_cache::{sweep_look_cache, CachedLook, LookCache};
use crate::shader_material::{build_shader_material, wgsl_source, ShaderMaterial};
use bevy::asset::AssetId;
use bevy::light::NotShadowCaster;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::shader::Shader;
use lunco_materials::{
    ParamSchema, ProceduralSkybox, ShaderLook, ShaderLookBound, ShaderLookKey, ShaderLookReady,
    TextureLayer,
};
use lunco_render::SurfaceAlpha;
use std::sync::Arc;

/// The small set of blend-state variants the fast custom-shader fallback needs.
/// Mask cutoffs intentionally use one conservative threshold: this profile is
/// about avoiding shader pipelines, not reproducing a user shader's details.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum FastAlpha {
    Opaque,
    Mask,
    Blend,
    Add,
}

impl FastAlpha {
    fn from_surface(alpha: SurfaceAlpha) -> Self {
        match alpha {
            SurfaceAlpha::Opaque => Self::Opaque,
            SurfaceAlpha::Mask(_) => Self::Mask,
            SurfaceAlpha::Blend => Self::Blend,
            SurfaceAlpha::Add => Self::Add,
        }
    }

    fn material_alpha_mode(self) -> AlphaMode {
        match self {
            Self::Opaque => AlphaMode::Opaque,
            Self::Mask => AlphaMode::Mask(0.5),
            Self::Blend => AlphaMode::Blend,
            Self::Add => AlphaMode::Add,
        }
    }
}

/// Fast mode keeps one unlit fallback for each required pipeline state. This
/// preserves batching for the hundreds of terrain tiles that normally share a
/// WGSL material, while avoiding shader and texture asset loading altogether.
#[derive(Resource, Default)]
struct FastShaderFallbacks {
    materials: HashMap<(FastAlpha, bool), Handle<StandardMaterial>>,
}

/// Shared `ShaderMaterial` per distinct [`ShaderLookKey`] — see the module docs.
/// Sharing, the `unshared` bypass, and eviction all live in
/// [`LookCache`](crate::look_cache::LookCache), shared with the PBR binder.
pub type ShaderLookCache = LookCache<ShaderLook>;

/// Bind custom-shader intent without loading custom shaders.
///
/// This is intentionally a separate build path rather than a flag threaded
/// through the regular binder: `ShaderMaterialPlugin` must not be registered in
/// fast mode, otherwise wgpu still compiles every authored WGSL pipeline.
pub(crate) fn build_fast(app: &mut App) {
    app.init_resource::<FastShaderFallbacks>()
        .add_observer(bind_fast_shader_look)
        .add_systems(Update, rebind_changed_fast_shader_look);
}

fn fast_material_for(
    look: &ShaderLook,
    fallbacks: &mut FastShaderFallbacks,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let key = (FastAlpha::from_surface(look.alpha), look.double_sided);
    fallbacks
        .materials
        .entry(key)
        .or_insert_with(|| {
            materials.add(StandardMaterial {
                // ShaderLook has no universal base-colour field. A neutral
                // lunar grey is an honest, stable fallback for arbitrary WGSL.
                base_color: Color::srgb(0.42, 0.42, 0.42),
                unlit: true,
                alpha_mode: key.0.material_alpha_mode(),
                double_sided: key.1,
                cull_mode: if key.1 {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                ..default()
            })
        })
        .clone()
}

fn bind_fast_shader_look(
    add: On<Add, ShaderLook>,
    looks: Query<&ShaderLook>,
    mut fallbacks: ResMut<FastShaderFallbacks>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let entity = add.entity;
    let Ok(look) = looks.get(entity) else {
        return;
    };
    let handle = fast_material_for(look, &mut fallbacks, &mut materials);
    commands
        .entity(entity)
        .try_remove::<MeshMaterial3d<ShaderMaterial>>()
        .try_insert((MeshMaterial3d(handle), ShaderLookBound, ShaderLookReady));
    apply_shadow_intent(&mut commands, entity, look);
}

fn rebind_changed_fast_shader_look(
    changed: Query<(Entity, &ShaderLook), Changed<ShaderLook>>,
    mut fallbacks: ResMut<FastShaderFallbacks>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (entity, look) in &changed {
        let handle = fast_material_for(look, &mut fallbacks, &mut materials);
        commands
            .entity(entity)
            .try_remove::<MeshMaterial3d<ShaderMaterial>>()
            .try_insert((MeshMaterial3d(handle), ShaderLookBound, ShaderLookReady));
        apply_shadow_intent(&mut commands, entity, look);
    }
}

impl CachedLook for ShaderLook {
    type Key = ShaderLookKey;
    type Material = ShaderMaterial;

    fn look_key(&self) -> ShaderLookKey {
        self.key()
    }
    fn is_unshared(&self) -> bool {
        self.unshared
    }
}

/// Build the concrete `ShaderMaterial` a look describes.
fn shader_material(look: &ShaderLook, asset_server: &AssetServer) -> ShaderMaterial {
    let mut m = ShaderMaterial {
        // A path, not a handle, in the intent — `bevy::shader` pulls naga, so the
        // domain crate cannot hold `Handle<Shader>`. Load it here.
        vertex_shader: look
            .vertex_shader
            .clone()
            .map(|p| asset_server.load::<Shader>(p)),
        // `live` params are real shader params — they are merely absent from the
        // sharing key, so a freshly-built material still has to carry them.
        values: look
            .values
            .iter()
            .chain(look.live.iter())
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
        // The same mapping `lunco-render-bevy`'s PBR binder applies to a `PbrLook`,
        // so a prim's authored transparency means the same thing on either path.
        alpha_mode: match look.alpha {
            SurfaceAlpha::Opaque => AlphaMode::Opaque,
            SurfaceAlpha::Mask(t) => AlphaMode::Mask(t),
            SurfaceAlpha::Blend => AlphaMode::Blend,
            SurfaceAlpha::Add => AlphaMode::Add,
        },
        // Same rule as `alpha_mode`: authored `doubleSided` means the same thing
        // on either material path.
        double_sided: look.double_sided,
        ..Default::default()
    };
    for (layer, image) in &look.textures {
        let slot = match layer {
            TextureLayer::Height => &mut m.height_map,
            TextureLayer::Albedo => &mut m.albedo_map,
            TextureLayer::Mineral => &mut m.mineral_map,
            TextureLayer::Surface => &mut m.surface_map,
            TextureLayer::Normal => &mut m.normal_map,
            TextureLayer::ShadowCache => &mut m.shadow_cache,
        };
        *slot = Some(image.clone());
    }
    // Packs against the (initially empty) schema; `reflect_shader_schemas` upgrades
    // it and repacks once the WGSL source lands. Same lifecycle as every other
    // `ShaderMaterial` in the codebase.
    m.repack();
    build_shader_material(asset_server.load::<Shader>(look.shader.clone()), m)
}

/// Resolve a look to a handle. Sharing + the `unshared` bypass are
/// [`LookCache::resolve`]'s job; this only supplies the build recipe.
fn material_for(
    look: &ShaderLook,
    cache: &mut ShaderLookCache,
    materials: &mut Assets<ShaderMaterial>,
    asset_server: &AssetServer,
) -> Handle<ShaderMaterial> {
    cache.resolve(look, materials, |l| shader_material(l, asset_server))
}

/// Bind a shader look to its one render owner.
///
/// A procedural sky uses the dedicated fullscreen background pass. It has no
/// mesh material because its shader is a fullscreen fragment stage, not a mesh
/// vertex/fragment pair. Keeping both render paths on the same entity would
/// submit an invalid mesh pipeline in addition to the valid background item.
fn bind_shader_render_components(
    entity: Entity,
    handle: Handle<ShaderMaterial>,
    look: &ShaderLook,
    skybox: bool,
    asset_server: &AssetServer,
    commands: &mut Commands,
) {
    let mut entity_commands = commands.entity(entity);
    entity_commands.try_remove::<MeshMaterial3d<StandardMaterial>>();
    if skybox {
        entity_commands.try_remove::<MeshMaterial3d<ShaderMaterial>>();
        entity_commands.try_insert((
            ShaderLookBound,
            crate::procedural_sky::ProceduralSkyboxMaterial::new(
                handle,
                &look.shader,
                asset_server,
            ),
        ));
    } else {
        entity_commands.try_insert((MeshMaterial3d(handle), ShaderLookBound));
        entity_commands.try_remove::<crate::procedural_sky::ProceduralSkyboxMaterial>();
    }
}

/// Does the material carry exactly the texture set the look states?
///
/// Slot-by-slot identity compare, so a driven TEXTURED look can take the
/// param-only update path: the old test was `!look.textures.is_empty()`, which
/// classified every textured look as a structural change and rebuilt its
/// material from scratch every tick the look moved.
fn textures_match(m: &ShaderMaterial, look: &ShaderLook) -> bool {
    use TextureLayer::*;
    [Height, Albedo, Mineral, Surface, Normal, ShadowCache]
        .iter()
        .all(|layer| {
            let slot = match layer {
                Height => &m.height_map,
                Albedo => &m.albedo_map,
                Mineral => &m.mineral_map,
                Surface => &m.surface_map,
                Normal => &m.normal_map,
                ShadowCache => &m.shadow_cache,
            };
            slot.as_ref().map(Handle::id) == look.textures.get(layer).map(Handle::id)
        })
}

/// `On<Add, ShaderLook>` — the moment intent appears, give it a material.
fn bind_shader_look(
    add: On<Add, ShaderLook>,
    looks: Query<&ShaderLook>,
    skyboxes: Query<(), With<ProceduralSkybox>>,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    let handle = material_for(look, &mut cache, &mut materials, &asset_server);
    // Appearance intent is exclusive, but USD's visual projection and this
    // observer run in different schedules. A `PbrLook` may therefore already
    // have produced its concrete material before the projection swaps to a
    // `ShaderLook`. Remove that stale draw before adding ours: leaving both
    // material component types on one mesh submits it twice with incompatible
    // pipelines (visible as bright, serrated fragments at wheel silhouettes).
    let skybox = skyboxes.get(e).is_ok();
    bind_shader_render_components(e, handle, look, skybox, &asset_server, &mut commands);
    apply_shadow_intent(&mut commands, e, look);
}

/// If the USD reader adds the explicit sky marker after the shader look, move
/// the already-resolved look onto the same render-side sky material contract.
fn bind_added_skybox_shader_look(
    add: On<Add, ProceduralSkybox>,
    looks: Query<&ShaderLook>,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    let handle = material_for(look, &mut cache, &mut materials, &asset_server);
    bind_shader_render_components(e, handle, look, true, &asset_server, &mut commands);
}

/// Mirror [`ShaderLook::no_shadow_cast`] onto the entity as `NotShadowCaster`.
///
/// `NotShadowCaster` is `bevy_light`, which is render-FREE — but it is applied
/// *here*, in the only crate that binds materials, so the render-free half of the
/// graph states the intent and never names the flag.
///
/// **Insert-only, deliberately.** Clearing the flag here whenever a look says
/// nothing about shadows would re-enable a shadow pass that some other authoring
/// path switched off. The cost is that turning `primvars:doNotCastShadows` back
/// off needs a reload rather than taking effect live — a fair trade against
/// silently re-enabling a shadow pass someone else switched off.
fn apply_shadow_intent(commands: &mut Commands, e: Entity, look: &ShaderLook) {
    if look.no_shadow_cast {
        commands.entity(e).try_insert(NotShadowCaster);
    }
}

/// Re-bind when a look is edited in place — a terrain tile changing mode,
/// an overlay re-tune, a late-bound derived map, an Inspector edit.
///
/// Change-driven, and it swaps a *handle* from the cache; it never touches the
/// material asset. A static scene costs nothing.
fn rebind_changed_shader_look(
    changed: Query<
        (
            Entity,
            &ShaderLook,
            Option<&MeshMaterial3d<ShaderMaterial>>,
            Has<ShaderLookReady>,
            Has<ProceduralSkybox>,
        ),
        Changed<ShaderLook>,
    >,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    shaders: Option<Res<Assets<Shader>>>,
    images: Option<Res<Assets<Image>>>,
    schemas: Option<Res<crate::ShaderSchemas>>,
    mut commands: Commands,
    // Shader path → resolved `AssetId`, so the driven hot path can compare shader
    // identity WITHOUT `asset_server.load::<Shader>()` per prim per tick. A path's
    // id is minted once and the bound material's own `Handle<Shader>` keeps the
    // asset alive, so a cached id stays valid while any look uses it; the
    // structural branch refreshes the entry from the freshly built material.
    mut shader_ids: Local<HashMap<String, AssetId<Shader>>>,
) {
    // Shared materials already written this run. Every terrain tile carries the same
    // global overlay values, so without this the one material they share would be
    // re-packed once per tile per change — hundreds of redundant writes per frame.
    let mut written: HashSet<AssetId<ShaderMaterial>> = HashSet::default();

    for (e, look, current, was_ready, skybox) in &changed {
        apply_shadow_intent(&mut commands, e, look);
        if look.unshared {
            // Private material: overwrite the asset it already owns, rather than
            // adding one per change (that would leak a material per frame).
            let current_handle = current.map(|material| material.0.clone());
            if let Some(mut existing) = current.and_then(|m| materials.get_mut(&m.0)) {
                // A driven look changes EVERY tick, so this is the hot path, and a
                // full rebuild here is wrong twice over.
                //
                // Correctness: `shader_material` builds from `..Default::default()`,
                // whose schema is `empty_schema_arc()`, and its trailing `repack()`
                // then packs against NO fields — every parameter zeroed. Harmless
                // when a material is being CREATED (`reflect_shader_schemas` fills
                // the schema in once the WGSL lands), fatal when it recurs: the two
                // systems are unordered `Update` members contending for
                // `Assets<ShaderMaterial>`, so if reflection runs first the zeroing
                // write is the last one each frame and the uniforms stay dead.
                //
                // Cost: it also re-collects the parameter map, re-resolves the
                // texture slots and calls `asset_server.load` twice, per driven prim
                // per tick, to express what is usually a single moved float.
                //
                // So rebuild only when the SHADER ITSELF changed (a hot-reloaded
                // `shaderPath`, a genuinely different texture set) — and carry the
                // reflected schema across when we do. Otherwise write the values in
                // place, which is what `set_many` exists for: one repack for N
                // fields, against the live schema. Shader identity comes from the
                // `shader_ids` cache — `asset_server.load` mints a strong handle
                // and touches the asset infrastructure, far too heavy for a
                // per-tick id compare. Textures compare slot-by-slot
                // (`textures_match`): a TEXTURED look whose texture SET is
                // unchanged takes the cheap param path like everything else.
                let want_shader_id = match shader_ids.get(look.shader.as_str()) {
                    Some(id) => *id,
                    None => {
                        let id = asset_server.load::<Shader>(look.shader.clone()).id();
                        shader_ids.insert(look.shader.clone(), id);
                        id
                    }
                };
                let structural = existing.shader.id() != want_shader_id
                    || existing.vertex_shader.is_some() != look.vertex_shader.is_some()
                    || !textures_match(&existing, look);
                if structural {
                    let schema = existing.schema.clone();
                    *existing = shader_material(look, &asset_server);
                    existing.set_schema(schema);
                    // The rebuild loaded the shader afresh; make the id cache agree
                    // with the material so the compare above stays quiet next tick.
                    shader_ids.insert(look.shader.clone(), existing.shader.id());
                } else {
                    existing.set_many(
                        look.values
                            .iter()
                            .chain(look.live.iter())
                            .map(|(k, v)| (k.as_str(), *v)),
                    );
                }
                if skybox {
                    commands
                        .entity(e)
                        .try_remove::<MeshMaterial3d<ShaderMaterial>>();
                    if let Some(handle) = current_handle {
                        commands.entity(e).try_insert(
                            crate::procedural_sky::ProceduralSkyboxMaterial::new(
                                handle,
                                &look.shader,
                                &asset_server,
                            ),
                        );
                    }
                } else {
                    commands
                        .entity(e)
                        .try_remove::<crate::procedural_sky::ProceduralSkyboxMaterial>();
                }
                continue;
            }
        }
        let handle = material_for(look, &mut cache, &mut materials, &asset_server);
        let same_material = current.is_some_and(|m| m.0.id() == handle.id());
        bind_shader_render_components(
            e,
            handle.clone(),
            look,
            skybox,
            &asset_server,
            &mut commands,
        );
        // A content-key change normally needs to clear the entity's readiness
        // latch: the replacement material may still be waiting for reflection
        // or one of its declared images. Edge-stitch updates are the important
        // exception for streamed terrain: they select another cached material
        // with the same already-loaded shader and images. Preserve readiness when
        // that replacement is already render-ready, otherwise the deferred
        // remove/re-add cycle makes the terrain cover alternate every ECS turn.
        let replacement_ready = materials
            .get(&handle)
            .zip(shaders.as_deref())
            .zip(images.as_deref())
            .zip(schemas.as_deref())
            .is_some_and(|(((material, shaders), images), schemas)| {
                material_is_render_ready(material, shaders, images, schemas)
            });
        if !same_material && was_ready && !replacement_ready {
            commands.entity(e).try_remove::<ShaderLookReady>();
        }

        // The look changed but resolved to the material it is ALREADY on ⇒ only
        // `live` params moved (they are outside the key). Write them into that
        // material rather than leaving it stale: re-keying is what mints a new,
        // unprepared material every slider tick and makes the terrain flicker.
        if same_material && !look.live.is_empty() && written.insert(handle.id()) {
            if let Some(mut mat) = materials.get_mut(&handle) {
                mat.set_many(
                    look.live
                        .iter()
                        .map(|(name, value)| (name.as_str(), *value)),
                );
            }
        }
    }
}

/// Wire the `ShaderLook` binder into an app. Called by
/// [`LuncoRenderPlugin`](crate::LuncoRenderPlugin).
///
/// NOTE: this does **not** add [`ShaderMaterialPlugin`](crate::ShaderMaterialPlugin)
/// — [`LuncoRenderPlugin`](crate::LuncoRenderPlugin) does, right after calling this,
/// and exactly once (the hand-rolled adds in `lunco-luncosim` and `luncosim` were
/// deleted; Bevy panics on a duplicate plugin). Keeping the two separate lets this
/// binder be unit-tested on a bare `MinimalPlugins` app, with no render pipeline.
pub(crate) fn build(app: &mut App) {
    // The `ShaderMaterial` store must exist for the binder even before the pipeline plugin
    // registers it (plugin order is not ours to control), and the `Shader` asset must be
    // registered for `asset_server.load::<Shader>` not to panic.
    //
    // GUARDED, because `init_asset` is NOT idempotent — this code used to claim it was, and
    // that was the bug. `AssetApp::init_asset::<A>` unconditionally builds a fresh
    // `Assets::<A>::default()`, hands the `AssetServer` a NEW handle provider for `A`, and
    // `insert_resource`s the empty store OVER the existing one. In a GUI build bevy's own
    // shader plugin already owns `Assets<Shader>`, so calling it again wiped the populated
    // store and swapped the index allocator underneath it. Handles minted by the OLD
    // allocator then completed loading and were inserted by index into the NEW, empty
    // storage — `index out of bounds: the len is 6 but the index is 7`, a hard panic in
    // `handle_internal_asset_events` on every startup that loaded a shader.
    //
    // Init only what nobody has registered yet.
    if !app.world().contains_resource::<Assets<ShaderMaterial>>() {
        bevy::asset::AssetApp::init_asset::<ShaderMaterial>(app);
    }
    if !app.world().contains_resource::<Assets<Shader>>() {
        bevy::asset::AssetApp::init_asset::<Shader>(app);
    }
    app.init_resource::<ShaderLookCache>()
        .add_observer(bind_shader_look)
        .add_observer(bind_added_skybox_shader_look)
        .add_systems(
            Update,
            (
                rebind_changed_shader_look,
                invalidate_shader_look_ready,
                mark_shader_look_ready.after(crate::reflect_shader_schemas),
                sweep_look_cache::<ShaderLook>,
            ),
        );
    // Shader parameters become connection targets in `lunco-usd-sim`'s
    // `shader_ports` — beside the pass that authors `ShaderLook::driven`, so a
    // shader wire lands in a headless build too. The writes arrive in
    // `ShaderLook::live`, which `rebind_changed_shader_look` above drains.
}

/// A shader hot reload invalidates the material layout that was previously
/// proven ready. Keep the mesh hidden until reflection and material repacking
/// have completed for the new source; otherwise a reload can expose a zeroed
/// uniform block for exactly one frame and create a black terrain tile.
///
/// An image's contents are allowed to change in place: Bevy's `Added` and
/// `Modified` notifications do not make an already-bound material unusable.
/// Treating those notifications as dependency loss removes `ShaderLookReady`
/// for one ECS turn, which makes streamed terrain disappear and reappear while
/// an image is being published. Only removal of a referenced image invalidates
/// the dependency contract.
fn invalidate_shader_look_ready(
    mut shader_events: Option<MessageReader<AssetEvent<Shader>>>,
    mut image_events: Option<MessageReader<AssetEvent<Image>>>,
    q: Query<(Entity, &MeshMaterial3d<ShaderMaterial>), With<ShaderLookReady>>,
    materials: Option<Res<Assets<ShaderMaterial>>>,
    mut commands: Commands,
) {
    let (Some(shader_events), Some(image_events), Some(materials)) =
        (shader_events.as_mut(), image_events.as_mut(), materials)
    else {
        return;
    };
    let changed_shaders: HashSet<AssetId<Shader>> = shader_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => Some(*id),
            AssetEvent::Removed { .. }
            | AssetEvent::Unused { .. }
            | AssetEvent::LoadedWithDependencies { .. } => None,
        })
        .collect();
    let changed_images: HashSet<AssetId<Image>> = image_events
        .read()
        .filter_map(image_dependency_removed)
        .collect();
    if changed_shaders.is_empty() && changed_images.is_empty() {
        return;
    }
    for (entity, material) in &q {
        let Some(material_asset) = materials.get(&material.0) else {
            continue;
        };
        let shader_changed = changed_shaders.contains(&material_asset.shader.id());
        let image_changed = changed_images.iter().any(|id| {
            [
                material_asset.height_map.as_ref(),
                material_asset.albedo_map.as_ref(),
                material_asset.mineral_map.as_ref(),
                material_asset.surface_map.as_ref(),
                material_asset.normal_map.as_ref(),
                material_asset.shadow_cache.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|handle| handle.id() == *id)
        });
        if shader_changed || image_changed {
            commands.entity(entity).try_remove::<ShaderLookReady>();
        }
    }
}

/// Return the image assets whose disappearance can make a ready material
/// invalid. Content publication (`Added`/`Modified`) is safe in place and must
/// not toggle the render-readiness latch.
fn image_dependency_removed(event: &AssetEvent<Image>) -> Option<AssetId<Image>> {
    match event {
        AssetEvent::Removed { id } | AssetEvent::Unused { id } => Some(*id),
        AssetEvent::Added { .. }
        | AssetEvent::Modified { .. }
        | AssetEvent::LoadedWithDependencies { .. } => None,
    }
}

fn material_is_render_ready(
    material: &ShaderMaterial,
    shaders: &Assets<Shader>,
    images: &Assets<Image>,
    schemas: &crate::ShaderSchemas,
) -> bool {
    let Some(shader) = shaders.get(&material.shader) else {
        return false;
    };
    let schema_ready = if let Some(reflected) = schemas.get(material.shader.id()) {
        Arc::ptr_eq(reflected, &material.schema)
    } else {
        let Some(source) = wgsl_source(shader) else {
            return false;
        };
        ParamSchema::parse(source).is_none()
    };
    schema_ready && material_texture_dependencies_ready(material, images)
}

/// A material is render-ready only when every texture it declares has an image
/// asset. Bevy can bind its fallback image for an absent optional handle, but
/// that is not a valid state for a terrain material: it turns a late streamed
/// map into a dark/black tile for one or more frames. The binder owns this
/// dependency invariant for every custom material, so terrain does not need a
/// second visibility workaround.
fn material_texture_dependencies_ready(material: &ShaderMaterial, images: &Assets<Image>) -> bool {
    [
        material.height_map.as_ref(),
        material.albedo_map.as_ref(),
        material.mineral_map.as_ref(),
        material.surface_map.as_ref(),
        material.normal_map.as_ref(),
        material.shadow_cache.as_ref(),
    ]
    .into_iter()
    .flatten()
    .all(|handle| images.get(handle).is_some())
}

/// Promote a custom look only after its shader source and reflected material
/// layout are available. The binder must create the asset before asynchronous
/// asset loading completes, but terrain visibility must not use that interval
/// as a render state: an empty schema packs terrain uniforms as zero.
fn mark_shader_look_ready(
    mut commands: Commands,
    q: Query<
        (Entity, &MeshMaterial3d<ShaderMaterial>),
        (With<ShaderLook>, Without<ShaderLookReady>),
    >,
    materials: Option<Res<Assets<ShaderMaterial>>>,
    shaders: Option<Res<Assets<Shader>>>,
    images: Option<Res<Assets<Image>>>,
    schemas: Option<Res<crate::ShaderSchemas>>,
) {
    let (Some(materials), Some(shaders), Some(images)) = (materials, shaders, images) else {
        return;
    };
    let Some(schemas) = schemas.as_deref() else {
        return;
    };
    for (entity, material_handle) in &q {
        let Some(material) = materials.get(&material_handle.0) else {
            continue;
        };
        if material_is_render_ready(material, &shaders, &images, &schemas) {
            commands.entity(entity).try_insert(ShaderLookReady);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShaderSchemas;
    use lunco_materials::ParamValue;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        // `Image` is registered by `ImagePlugin` in a real build; the texture-layer
        // test needs it in this bare one.
        app.init_asset::<Image>();
        build(&mut app);
        app
    }

    #[test]
    fn shader_material_readiness_includes_declared_images() {
        let mut app = App::new();
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<Image>();
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let mut material = ShaderMaterial::default();

        assert!(material_texture_dependencies_ready(
            &material,
            app.world().resource::<Assets<Image>>()
        ));

        material.height_map = Some(image);
        assert!(material_texture_dependencies_ready(
            &material,
            app.world().resource::<Assets<Image>>()
        ));

        material.shadow_cache = Some(Handle::default());
        assert!(!material_texture_dependencies_ready(
            &material,
            app.world().resource::<Assets<Image>>()
        ));
    }

    #[test]
    fn render_ready_requires_loaded_shader_and_declared_images() {
        let mut app = App::new();
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<Shader>();

        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                "// no dynamic Material struct",
                "test.wgsl",
            ));
        let mut material = ShaderMaterial::default();
        material.shader = shader;
        let schemas = ShaderSchemas::default();

        assert!(material_is_render_ready(
            &material,
            app.world().resource::<Assets<Shader>>(),
            app.world().resource::<Assets<Image>>(),
            &schemas,
        ));

        material.height_map = Some(Handle::default());
        assert!(!material_is_render_ready(
            &material,
            app.world().resource::<Assets<Shader>>(),
            app.world().resource::<Assets<Image>>(),
            &schemas,
        ));
    }

    #[test]
    fn image_content_publication_does_not_invalidate_ready_materials() {
        let id = Handle::<Image>::default().id();

        assert_eq!(
            image_dependency_removed(&AssetEvent::Added { id }),
            None,
            "adding an image cannot invalidate a material already bound to it"
        );
        assert_eq!(
            image_dependency_removed(&AssetEvent::Modified { id }),
            None,
            "in-place image content updates preserve material readiness"
        );
        assert_eq!(
            image_dependency_removed(&AssetEvent::Removed { id }),
            Some(id)
        );
        assert_eq!(
            image_dependency_removed(&AssetEvent::Unused { id }),
            Some(id)
        );
    }

    fn material_of(app: &App, e: Entity) -> Handle<ShaderMaterial> {
        app.world()
            .entity(e)
            .get::<MeshMaterial3d<ShaderMaterial>>()
            .expect("bound")
            .0
            .clone()
    }

    /// THE property the cache exists for: N tiles in the same LOD band
    /// step must share ONE material and ONE bind group. If this regresses, terrain
    /// batching dies and the draw-call count goes linear in the tile count.
    #[test]
    fn identical_looks_share_one_material() {
        let mut app = app();
        let look = ShaderLook::new("shaders/terrain_geomorph.wgsl")
            .with_vertex_shader("shaders/terrain_geomorph.wgsl")
            .with("morph_start", ParamValue::F32(0.7))
            .with("morph_end", ParamValue::F32(1.0));
        let ids: Vec<Entity> = (0..64)
            .map(|_| app.world_mut().spawn(look.clone()).id())
            .collect();
        app.update();

        let handles: Vec<_> = ids.iter().map(|&e| material_of(&app, e)).collect();
        assert!(
            handles.windows(2).all(|w| w[0] == w[1]),
            "64 identical looks must share one material handle"
        );
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 1);
        assert_eq!(app.world().resource::<ShaderLookCache>().len(), 1);
    }

    /// Two genuinely different looks must NOT collide into one material.
    #[test]
    fn different_looks_get_different_materials() {
        let mut app = app();
        app.world_mut().spawn(
            ShaderLook::new("shaders/terrain_geomorph.wgsl")
                .with("morph_start", ParamValue::F32(0.0)),
        );
        app.world_mut().spawn(
            ShaderLook::new("shaders/terrain_geomorph.wgsl")
                .with("morph_start", ParamValue::F32(1.0)),
        );
        // A different shader path is also a different material.
        app.world_mut()
            .spawn(ShaderLook::new("shaders/terrain_geomorph_flat.wgsl"));
        app.update();
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 3);
    }

    /// A `Changed<ShaderLook>` re-binds — this is how a tile's late-bound maps and the
    /// live overlay re-tune reach the GPU, WITHOUT mutating any material asset.
    #[test]
    fn changed_look_rebinds_from_the_cache() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn(
                ShaderLook::new("shaders/terrain_geomorph.wgsl")
                    .with("morph_start", ParamValue::F32(0.0)),
            )
            .id();
        app.update();
        let first = material_of(&app, e);

        // Edit a param in place — the same shape of edit the tile pipeline makes.
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ShaderLook>()
            .unwrap()
            .values
            .insert("morph_start".into(), ParamValue::F32(0.5));
        app.update();
        let second = material_of(&app, e);
        assert_ne!(
            first, second,
            "a changed look must bind a different material"
        );
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 2);

        // …and stepping BACK to a look already seen reuses the cached material
        // instead of minting a third (the band lattice is a small shared set).
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ShaderLook>()
            .unwrap()
            .values
            .insert("morph_start".into(), ParamValue::F32(0.0));
        app.update();
        assert_eq!(material_of(&app, e), first);
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 2);
    }

    /// Texture layers land on the right `ShaderMaterial` slots, and two looks that
    /// differ ONLY by a bound texture do not share a material (per-place quality:
    /// the near tile's 2048² albedo and the far tile's 256² one are two materials).
    #[test]
    fn texture_layers_map_onto_material_slots() {
        let mut app = app();
        let surface: Handle<Image> = app.world().resource::<AssetServer>().load("a.png");
        let normal: Handle<Image> = app.world().resource::<AssetServer>().load("b.png");
        let e = app
            .world_mut()
            .spawn(
                ShaderLook::new("shaders/terrain_geomorph.wgsl")
                    .with_texture(TextureLayer::Surface, surface.clone())
                    .with_texture(TextureLayer::Normal, normal.clone()),
            )
            .id();
        app.world_mut()
            .spawn(ShaderLook::new("shaders/terrain_geomorph.wgsl"));
        app.update();

        let h = material_of(&app, e);
        let mats = app.world().resource::<Assets<ShaderMaterial>>();
        let m = mats.get(&h).expect("material");
        assert_eq!(m.surface_map.as_ref(), Some(&surface));
        assert_eq!(m.normal_map.as_ref(), Some(&normal));
        assert!(m.height_map.is_none());
        assert_eq!(mats.len(), 2, "a bound texture is part of the sharing key");
    }

    #[test]
    fn fast_mode_falls_back_without_creating_a_shader_material() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>();
        build_fast(&mut app);
        let e = app
            .world_mut()
            .spawn(ShaderLook::new("shaders/terrain_geomorph.wgsl"))
            .id();

        app.update();

        let material = app
            .world()
            .entity(e)
            .get::<MeshMaterial3d<StandardMaterial>>()
            .expect("fast fallback material")
            .0
            .clone();
        assert!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&material)
                .expect("fallback asset")
                .unlit
        );
        assert!(!app
            .world()
            .entity(e)
            .contains::<MeshMaterial3d<ShaderMaterial>>());
    }

    /// A USD material can be projected as plain PBR before its WGSL binding is
    /// resolved. Taking the shader path must replace the concrete material too,
    /// not merely the render-free intent, or Bevy draws the mesh twice.
    #[test]
    fn shader_look_replaces_a_preexisting_standard_material() {
        let mut app = app();
        app.init_asset::<StandardMaterial>();
        let standard = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let e = app
            .world_mut()
            .spawn((
                MeshMaterial3d(standard),
                ShaderLook::new("shaders/wheel.wgsl"),
            ))
            .id();

        app.update();

        let entity = app.world().entity(e);
        assert!(entity.contains::<MeshMaterial3d<ShaderMaterial>>());
        assert!(
            !entity.contains::<MeshMaterial3d<StandardMaterial>>(),
            "the shader material must replace, not overlay, the PBR material"
        );
    }

    #[test]
    fn procedural_skybox_owns_background_pass_without_mesh_material() {
        let mut app = app();
        let existing = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::default());
        let before_look = app
            .world_mut()
            .spawn((
                ShaderLook::new("shaders/starfield.wgsl"),
                ProceduralSkybox,
                MeshMaterial3d(existing.clone()),
            ))
            .id();
        let after_look = app
            .world_mut()
            .spawn(ShaderLook::new("shaders/starfield.wgsl"))
            .id();

        app.update();
        app.world_mut()
            .entity_mut(after_look)
            .insert(ProceduralSkybox);
        app.update();

        for entity in [before_look, after_look] {
            let entity_ref = app.world().entity(entity);
            assert!(
                !entity_ref.contains::<MeshMaterial3d<ShaderMaterial>>(),
                "a procedural sky must not enter the mesh material pipeline"
            );
            assert!(entity_ref.contains::<crate::procedural_sky::ProceduralSkyboxMaterial>());
        }
    }
}
