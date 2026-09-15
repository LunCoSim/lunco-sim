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
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::render::render_resource::{TextureDimension, TextureFormat};
use bevy::shader::Shader;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_materials::{
    rgba8_mip_chain, validate_shader_stage, ParamSchema, ProceduralSkybox, Rgba8MipMode,
    ShaderLook, ShaderLookBound, ShaderLookKey, ShaderLookReady, ShaderStage, TextureLayer,
};
use lunco_render::SurfaceAlpha;
use std::sync::Arc;

/// Shared `ShaderMaterial` per distinct [`ShaderLookKey`] — see the module docs.
/// Sharing, the `unshared` bypass, and eviction all live in
/// [`LookCache`](crate::look_cache::LookCache), shared with the PBR binder.
pub type ShaderLookCache = LookCache<ShaderLook>;

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

/// Return a stage error only when the requested shader asset is already loaded.
/// Unloaded assets are not errors yet: the normal asset event will validate them
/// at publication time. Keeping this distinction lets a command-created look
/// added after an asset event obey the same no-invalid-pipeline contract without
/// turning asset loading into a polling loop.
fn loaded_shader_stage_failure(
    look: &ShaderLook,
    shaders: Option<&Assets<Shader>>,
    asset_server: &AssetServer,
) -> Option<(ShaderStage, String)> {
    let shaders = shaders?;
    let fragment = asset_server.load::<Shader>(look.shader.clone());
    if let Some(source) = shaders.get(&fragment).and_then(wgsl_source) {
        if let Err(error) = validate_shader_stage(source, ShaderStage::Fragment) {
            return Some((ShaderStage::Fragment, error.to_string()));
        }
    }
    let Some(vertex_path) = look.vertex_shader.as_ref() else {
        return None;
    };
    let vertex = asset_server.load::<Shader>(vertex_path.clone());
    shaders
        .get(&vertex)
        .and_then(wgsl_source)
        .and_then(|source| {
            validate_shader_stage(source, ShaderStage::Vertex)
                .err()
                .map(|error| (ShaderStage::Vertex, error.to_string()))
        })
}

fn shader_id_for(
    path: &str,
    ids: &mut HashMap<String, AssetId<Shader>>,
    asset_server: &AssetServer,
) -> AssetId<Shader> {
    if let Some(id) = ids.get(path) {
        return *id;
    }
    let id = asset_server.load::<Shader>(path.to_owned()).id();
    ids.insert(path.to_owned(), id);
    id
}

fn clear_shader_render_components(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .try_remove::<MeshMaterial3d<ShaderMaterial>>()
        .try_remove::<MeshMaterial3d<StandardMaterial>>()
        .try_remove::<ShaderLookBound>()
        .try_remove::<ShaderLookReady>()
        .try_remove::<crate::procedural_sky::ProceduralSkyboxMaterial>();
}

fn record_loaded_shader_stage_failure(
    diagnostics: &mut lunco_core::RuntimeDiagnostics,
    entity: Entity,
    look: &ShaderLook,
    stage: ShaderStage,
    detail: String,
) {
    let subject = format!("entity {entity:?}");
    diagnostics
        .findings
        .retain(|finding| !(finding.producer == "shader-render" && finding.subject == subject));
    diagnostics.findings.push(lunco_core::RuntimeDiagnostic {
        code: "render-shader-stage".to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: "shader-render".to_string(),
        subject,
        message: format!(
            "{} shader asset for `{}` is invalid: {detail}",
            match stage {
                ShaderStage::Fragment => "fragment",
                ShaderStage::Vertex => "vertex",
            },
            look.shader
        ),
    });
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
    shaders: Option<Res<Assets<Shader>>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    if let Some((stage, detail)) =
        loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server)
    {
        clear_shader_render_components(&mut commands, e);
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
        }
        return;
    }
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
    shaders: Option<Res<Assets<Shader>>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    if let Some((stage, detail)) =
        loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server)
    {
        clear_shader_render_components(&mut commands, e);
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
        }
        return;
    }
    let handle = material_for(look, &mut cache, &mut materials, &asset_server);
    bind_shader_render_components(e, handle, look, true, &asset_server, &mut commands);
}

/// Mirror [`ShaderLook::no_shadow_cast`] onto the entity as `NotShadowCaster`.
///
/// `NotShadowCaster` is `bevy_light`, which is render-FREE — but it is applied
/// *here*, in the only crate that binds materials, so the render-free half of the
/// graph states the intent and never names the flag.
///
/// The shader look is the exclusive appearance owner, so this is a full
/// reconciliation: clearing the authored opt-out must remove a stale derived
/// `NotShadowCaster` marker as well as setting it when requested.
fn apply_shadow_intent(commands: &mut Commands, e: Entity, look: &ShaderLook) {
    if look.no_shadow_cast {
        commands.entity(e).try_insert(NotShadowCaster);
    } else {
        commands.entity(e).try_remove::<NotShadowCaster>();
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
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
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
        if let Some((stage, detail)) =
            loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server)
        {
            clear_shader_render_components(&mut commands, e);
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
            }
            continue;
        }
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
                let want_shader_id =
                    shader_id_for(&look.shader, &mut shader_ids, &asset_server);
                let want_vertex_shader_id = look
                    .vertex_shader
                    .as_deref()
                    .map(|path| shader_id_for(path, &mut shader_ids, &asset_server));
                let structural = existing.shader.id() != want_shader_id
                    || existing.vertex_shader.as_ref().map(Handle::id)
                        != want_vertex_shader_id
                    || !textures_match(&existing, look);
                if structural {
                    let schema = existing.schema.clone();
                    *existing = shader_material(look, &asset_server);
                    existing.set_schema(schema);
                    // The rebuild loaded the shader afresh; make the id cache agree
                    // with the material so the compare above stays quiet next tick.
                    shader_ids.insert(look.shader.clone(), existing.shader.id());
                    if let (Some(path), Some(id)) = (
                        look.vertex_shader.as_deref(),
                        existing.vertex_shader.as_ref().map(Handle::id),
                    ) {
                        shader_ids.insert(path.to_owned(), id);
                    }
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

/// Validate loaded shader stages on the asset event that changed them.
///
/// This is deliberately event-driven. A shader failure removes the concrete
/// material instead of substituting `StandardMaterial`, which keeps an authored
/// error visible to the runtime diagnostic surface and prevents an invalid
/// pipeline from being submitted every frame. A corrected asset is rebound by
/// the same owner, so an authored USD/Rhai edit can repair the scene without a
/// process restart.
fn validate_shader_assets_on_change(
    mut events: MessageReader<AssetEvent<Shader>>,
    looks: Query<(
        Entity,
        &ShaderLook,
        Option<&MeshMaterial3d<ShaderMaterial>>,
        Has<ProceduralSkybox>,
    )>,
    shaders: Option<Res<Assets<Shader>>>,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let mut changed: HashSet<AssetId<Shader>> = HashSet::default();
    let mut removed: HashSet<AssetId<Shader>> = HashSet::default();
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => {
                changed.insert(*id);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                changed.insert(*id);
                removed.insert(*id);
            }
        }
    }
    if changed.is_empty() {
        return;
    }
    let Some(shaders) = shaders else {
        return;
    };

    let mut findings = Vec::new();
    let mut repairs = Vec::new();
    let mut rejections = Vec::new();

    for (entity, look, current, skybox) in &looks {
        let fragment = asset_server.load::<Shader>(look.shader.clone());
        let vertex = look
            .vertex_shader
            .as_ref()
            .map(|path| asset_server.load::<Shader>(path.clone()));
        let fragment_changed = changed.contains(&fragment.id());
        let vertex_changed = vertex
            .as_ref()
            .is_some_and(|handle| changed.contains(&handle.id()));
        let affected = fragment_changed || vertex_changed;

        // Inspect every live look whenever any shader asset changes. The event
        // identifies when validation is needed, but the diagnostic set is the
        // complete current state for this producer; otherwise an unrelated
        // reload would erase an older shader error from the status surface.
        let fragment_source = shaders.get(&fragment).and_then(wgsl_source);
        let fragment_loaded = fragment_source.is_some() || removed.contains(&fragment.id());
        let mut failure = fragment_source
            .and_then(|source| {
                validate_shader_stage(source, ShaderStage::Fragment)
                    .err()
                    .map(|error| (ShaderStage::Fragment, error.to_string()))
            })
            .or_else(|| {
                removed.contains(&fragment.id()).then_some((
                    ShaderStage::Fragment,
                    "shader asset was removed before a valid stage was available".to_string(),
                ))
            });

        let vertex_loaded = vertex.as_ref().is_none_or(|vertex_handle| {
            shaders.get(vertex_handle).and_then(wgsl_source).is_some()
                || removed.contains(&vertex_handle.id())
        });
        if failure.is_none() {
            if let Some(vertex_handle) = vertex.as_ref() {
                failure = shaders
                    .get(vertex_handle)
                    .and_then(wgsl_source)
                    .and_then(|source| {
                        validate_shader_stage(source, ShaderStage::Vertex)
                            .err()
                            .map(|error| (ShaderStage::Vertex, error.to_string()))
                    })
                    .or_else(|| {
                        removed.contains(&vertex_handle.id()).then_some((
                            ShaderStage::Vertex,
                            "shader asset was removed before a valid stage was available"
                                .to_string(),
                        ))
                    });
            }
        }

        if let Some((stage, detail)) = failure {
            let message = format!(
                "{} shader asset for `{}` is invalid: {detail}",
                match stage {
                    ShaderStage::Fragment => "fragment",
                    ShaderStage::Vertex => "vertex",
                },
                look.shader
            );
            findings.push(lunco_core::RuntimeDiagnostic {
                code: "render-shader-stage".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "shader-render".to_string(),
                subject: format!("entity {entity:?}"),
                message,
            });
            if affected {
                rejections.push(entity);
            }
        } else if affected && current.is_none() && fragment_loaded && vertex_loaded {
            repairs.push((entity, skybox));
        }
    }

    for entity in rejections {
        clear_shader_render_components(&mut commands, entity);
    }
    for (entity, skybox) in repairs {
        let Ok((_, look, _, _)) = looks.get(entity) else {
            continue;
        };
        let handle = material_for(look, &mut cache, &mut materials, &asset_server);
        bind_shader_render_components(
            entity,
            handle,
            look,
            skybox,
            &asset_server,
            &mut commands,
        );
        apply_shadow_intent(&mut commands, entity, look);
    }
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("shader-render", findings);
    }
}

type ShaderImageMipKey = (AssetId<Image>, Rgba8MipMode);

/// Tracks the event-driven CPU preparation needed by authored shader rasters.
///
/// PNG/JPEG image loading supplies a single base level even when its sampler
/// requests trilinear filtering. The renderer owns the concrete `Image`, so it
/// is also the authoritative place to materialize the missing levels. Requests
/// are registered only when a look changes and are deduplicated by image id;
/// hundreds of streamed terrain tiles therefore cannot repeat the same bake.
#[derive(Resource, Default)]
struct ShaderImageMipState {
    /// Image-role requests discovered from live `ShaderLook` components. Keep
    /// these separate from `pending`: an image may be hot-reloaded after its
    /// first chain was installed, in which case the asset event must enqueue
    /// it again without requiring every look to change.
    requested: HashSet<ShaderImageMipKey>,
    pending: HashSet<ShaderImageMipKey>,
    tasks: HashMap<ShaderImageMipKey, Task<Option<MippedShaderImage>>>,
    /// Asset events are the image-content invalidation boundary. A completed
    /// worker result is applied only if no newer event has arrived since its
    /// snapshot, so a hot reload cannot publish stale pixels over the new image.
    epochs: HashMap<AssetId<Image>, u64>,
}

struct MippedShaderImage {
    id: AssetId<Image>,
    mode: Rgba8MipMode,
    epoch: u64,
    width: u32,
    height: u32,
    data: Vec<u8>,
    mip_levels: u32,
}

fn authored_shader_image_mip_mode(layer: TextureLayer) -> Option<Rgba8MipMode> {
    match layer {
        // These are the four filterable image roles authored by the USD shader
        // reader. Height and ShadowCache have different formats/access patterns
        // and are intentionally not treated as RGBA8 colour images here.
        TextureLayer::Albedo | TextureLayer::Mineral => Some(Rgba8MipMode::SrgbColor),
        TextureLayer::Surface => Some(Rgba8MipMode::Linear),
        TextureLayer::Normal => Some(Rgba8MipMode::Normal),
        TextureLayer::Height | TextureLayer::ShadowCache => None,
    }
}

fn authored_shader_image_format(mode: Rgba8MipMode) -> TextureFormat {
    match mode {
        Rgba8MipMode::SrgbColor => TextureFormat::Rgba8UnormSrgb,
        Rgba8MipMode::Linear | Rgba8MipMode::Normal => TextureFormat::Rgba8Unorm,
    }
}

fn authored_shader_image_base(image: &Image, mode: Rgba8MipMode) -> Option<(Vec<u8>, u32, u32)> {
    let descriptor = &image.texture_descriptor;
    if descriptor.dimension != TextureDimension::D2
        || descriptor.size.depth_or_array_layers != 1
        || descriptor.format != authored_shader_image_format(mode)
    {
        return None;
    }
    let (width, height) = (descriptor.size.width, descriptor.size.height);
    let expected_len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    let data = image.data.as_ref()?;
    (data.len() == expected_len).then(|| (data.clone(), width, height))
}

/// Prepare authored shader rasters after a look declares them.
///
/// This is event/change driven: no scene-wide per-frame scan and no duplicate
/// work for tiles sharing a streamed asset. The byte filter runs on the async
/// compute pool; the main thread only installs the completed chain and sampler
/// descriptor into the existing image asset.
fn prepare_authored_shader_image_mips(
    changed: Query<&ShaderLook, Changed<ShaderLook>>,
    mut image_events: Option<MessageReader<AssetEvent<Image>>>,
    mut state: ResMut<ShaderImageMipState>,
    images: Option<ResMut<Assets<Image>>>,
    quality: Option<Res<lunco_render::RenderingQualitySettings>>,
) {
    let (Some(image_events), Some(mut images)) = (image_events.as_mut(), images) else {
        return;
    };

    for event in image_events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let epoch = state.epochs.entry(*id).or_default();
                *epoch = epoch.saturating_add(1);
                let refreshed = state
                    .requested
                    .iter()
                    .filter(|(image_id, _)| image_id == id)
                    .copied()
                    .collect::<Vec<_>>();
                state.pending.extend(refreshed);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                state.pending.retain(|(image_id, _)| image_id != id);
                state.tasks.retain(|(image_id, _), _| image_id != id);
                state.epochs.remove(id);
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }

    for look in &changed {
        for (layer, image) in &look.textures {
            let Some(mode) = authored_shader_image_mip_mode(*layer) else {
                continue;
            };
            let key = (image.id(), mode);
            state.requested.insert(key);
            state.pending.insert(key);
        }
    }

    let pending: Vec<_> = state.pending.iter().copied().collect();
    for (id, mode) in pending {
        if state.tasks.contains_key(&(id, mode)) {
            continue;
        }
        let Some(image) = images.get(id) else {
            continue;
        };
        // KTX2/DDS and generated terrain images may already carry their full
        // chain. They need no CPU work, but their existing sampler remains the
        // source of truth.
        if image.texture_descriptor.mip_level_count > 1 {
            state.pending.remove(&(id, mode));
            continue;
        }
        // A role/format mismatch is an authored contract error, not a reason
        // to retry every frame. The USD reader assigns these formats at load
        // time; non-RGBA roles are handled by their dedicated bindings.
        if image.texture_descriptor.dimension != TextureDimension::D2
            || image.texture_descriptor.size.depth_or_array_layers != 1
            || image.texture_descriptor.format != authored_shader_image_format(mode)
        {
            state.pending.remove(&(id, mode));
            continue;
        }
        // An image asset can briefly exist without CPU data while a custom
        // loader finishes publishing it. Keep the request until its Modified
        // event makes the bytes available.
        let Some((base, width, height)) = authored_shader_image_base(image, mode) else {
            continue;
        };
        if width == 1 && height == 1 {
            state.pending.remove(&(id, mode));
            continue;
        }
        let epoch = *state.epochs.entry(id).or_default();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let (data, mip_levels) = rgba8_mip_chain(
                base,
                usize::try_from(width).ok()?,
                usize::try_from(height).ok()?,
                mode,
            )?;
            Some(MippedShaderImage {
                id,
                mode,
                epoch,
                width,
                height,
                data,
                mip_levels,
            })
        });
        state.pending.remove(&(id, mode));
        state.tasks.insert((id, mode), task);
    }

    let mut finished = Vec::new();
    for (key, task) in &mut state.tasks {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            finished.push((*key, result));
        }
    }
    for (key, _) in &finished {
        state.tasks.remove(key);
    }

    let default_anisotropy = lunco_render::RenderingQualitySettings::default()
        .profile()
        .terrain_derived_texture_anisotropy;
    let anisotropy = quality
        .as_ref()
        .and_then(|settings| settings.validated_profile().ok())
        .map(|profile| profile.terrain_derived_texture_anisotropy)
        .unwrap_or(default_anisotropy)
        .max(1);

    for (_, result) in finished {
        let Some(result) = result else { continue };
        if state.epochs.get(&result.id).copied().unwrap_or_default() != result.epoch {
            continue;
        }
        let Some(mut image) = images.get_mut(result.id) else {
            continue;
        };
        if image.texture_descriptor.mip_level_count > 1
            || image.texture_descriptor.dimension != TextureDimension::D2
            || image.texture_descriptor.size.width != result.width
            || image.texture_descriptor.size.height != result.height
            || image.texture_descriptor.format != authored_shader_image_format(result.mode)
        {
            continue;
        }
        image.data = Some(result.data);
        image.texture_descriptor.mip_level_count = result.mip_levels;
        let mut sampler = match &image.sampler {
            ImageSampler::Descriptor(descriptor) => descriptor.clone(),
            ImageSampler::Default => ImageSamplerDescriptor::linear(),
        };
        sampler
            .set_filter(ImageFilterMode::Linear)
            .set_anisotropic_filter(anisotropy);
        image.sampler = ImageSampler::Descriptor(sampler);
    }
}

fn clear_shader_image_mips(mut state: ResMut<ShaderImageMipState>) {
    state.requested.clear();
    state.pending.clear();
    state.tasks.clear();
    state.epochs.clear();
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
        .init_resource::<ShaderImageMipState>()
        .add_observer(bind_shader_look)
        .add_observer(bind_added_skybox_shader_look)
        .add_systems(
            Update,
            (
                rebind_changed_shader_look,
                invalidate_shader_look_ready,
                mark_shader_look_ready.after(crate::reflect_shader_schemas),
                validate_shader_assets_on_change.after(crate::reflect_shader_schemas),
                sweep_look_cache::<ShaderLook>,
            ),
        )
        .add_systems(
            Update,
            prepare_authored_shader_image_mips.after(rebind_changed_shader_look),
        )
        .add_systems(lunco_core::SceneTeardown, clear_shader_image_mips);
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
    let Some(source) = wgsl_source(shader) else {
        return false;
    };
    if validate_shader_stage(source, ShaderStage::Fragment).is_err() {
        return false;
    }
    let schema_ready = if let Some(reflected) = schemas.get(material.shader.id()) {
        Arc::ptr_eq(reflected, &material.schema)
    } else {
        ParamSchema::parse(source).is_none()
    };
    let vertex_ready = material.vertex_shader.as_ref().is_none_or(|vertex_handle| {
        shaders
            .get(vertex_handle)
            .and_then(wgsl_source)
            .is_some_and(|source| validate_shader_stage(source, ShaderStage::Vertex).is_ok())
    });
    schema_ready && vertex_ready && material_texture_dependencies_ready(material, images)
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
        if material_is_render_ready(material, &shaders, &images, schemas) {
            commands.entity(entity).try_insert(ShaderLookReady);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShaderSchemas;
    use bevy::render::render_resource::Extent3d;
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
                "@fragment fn fragment() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }",
                "test.wgsl",
            ));
        let mut material = ShaderMaterial {
            shader,
            ..Default::default()
        };
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
        let look = ShaderLook::new("shaders/terrain_layered.wgsl")
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
            ShaderLook::new("shaders/terrain_layered.wgsl")
                .with("morph_start", ParamValue::F32(0.0)),
        );
        app.world_mut().spawn(
            ShaderLook::new("shaders/terrain_layered.wgsl")
                .with("morph_start", ParamValue::F32(1.0)),
        );
        // A different shader path is also a different material.
        app.world_mut()
            .spawn(ShaderLook::new("shaders/terrain_debug.wgsl"));
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
                ShaderLook::new("shaders/terrain_layered.wgsl")
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
                ShaderLook::new("shaders/terrain_layered.wgsl")
                    .with_texture(TextureLayer::Surface, surface.clone())
                    .with_texture(TextureLayer::Normal, normal.clone()),
            )
            .id();
        app.world_mut()
            .spawn(ShaderLook::new("shaders/terrain_layered.wgsl"));
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
    fn authored_image_mips_are_rebuilt_after_hot_reload() {
        let mut app = app();
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new(
                Extent3d {
                    width: 2,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                vec![
                    0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
                ],
                TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::MAIN_WORLD,
            ));
        let entity = app
            .world_mut()
            .spawn(
                ShaderLook::new("shaders/terrain_layered.wgsl")
                    .with_texture(TextureLayer::Albedo, image.clone()),
            )
            .id();

        for _ in 0..8 {
            app.update();
            if app
                .world()
                .resource::<Assets<Image>>()
                .get(image.id())
                .is_some_and(|image| image.texture_descriptor.mip_level_count == 2)
            {
                break;
            }
        }
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(image.id())
                .expect("authored image")
                .texture_descriptor
                .mip_level_count,
            2
        );

        let reloaded_base = vec![
            32, 32, 32, 255, 224, 224, 224, 255, 32, 32, 32, 255, 224, 224, 224, 255,
        ];
        {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let mut reloaded = images
                .get_mut(image.id())
                .expect("authored image for hot reload");
            reloaded.data = Some(reloaded_base);
            reloaded.texture_descriptor.mip_level_count = 1;
        }

        for _ in 0..8 {
            app.update();
            if app
                .world()
                .resource::<Assets<Image>>()
                .get(image.id())
                .is_some_and(|image| image.texture_descriptor.mip_level_count == 2)
            {
                break;
            }
        }
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(image.id())
            .expect("reloaded authored image");
        assert_eq!(image.texture_descriptor.mip_level_count, 2);
        assert_eq!(image.data.as_ref().map(Vec::len), Some(20));
        assert!(matches!(
            image.sampler,
            ImageSampler::Descriptor(ImageSamplerDescriptor {
                mipmap_filter: ImageFilterMode::Linear,
                ..
            })
        ));
        assert!(app.world().entity(entity).contains::<ShaderLookBound>());
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
                MeshMaterial3d(existing),
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
