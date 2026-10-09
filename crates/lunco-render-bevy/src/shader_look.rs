//! The `ShaderLook` → `ShaderMaterial` binder — the custom-shader half of the
//! render boundary.
//!
//! [`lunco_render::PbrLook`] covers a plain PBR surface; a *custom shader* look is
//! open-ended (the parameter set belongs to the `.wgsl`, not to Rust), so domain
//! crates state it as [`lunco_materials::ShaderLook`] — a shader **path**, a
//! `BTreeMap` of named [`ParamValue`](lunco_materials::ParamValue)s, and named
//! [`TextureLayer`]s. Neither the path nor `ShaderTexture` touches `bevy_pbr`, so
//! the crate that authors the look (the terrain streamer, notably) links no GPU
//! stack. This module is where it becomes a real `ShaderMaterial`.
//!
//! # The cache is load-bearing
//!
//! [`ShaderLookCache`] maps [`ShaderLookKey`] → one `Handle<ShaderMaterial>`. The
//! terrain LOD path depends on it: the ~150–500 resident tiles collapse onto a
//! handful of distinct looks (mode x morph-band bucket), and they
//! MUST resolve to the same material — one bind group, one batch, keyed by the
//! look's content.
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
//! by name, and is repacked the moment the schema lands. Stage validation,
//! schemas, and source interfaces share asset-scoped facts; tile replacement
//! never re-parses an unchanged shader.

use crate::look_cache::{CachedLook, LookCache, sweep_look_cache};
use crate::shader_material::{ShaderMaterial, build_shader_material, wgsl_source};
use bevy::asset::AssetId;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::shader::Shader;
use lunco_materials::image_loader::ShaderImageAssets;
use lunco_materials::{ColorMip, LinearMip, NormalMip, PreparedShaderImage};
use lunco_materials::{
    ParamSchema, ShaderLook, ShaderLookBound, ShaderLookKey, ShaderLookReady,
    ShaderLookSourceInterface, ShaderStage, TextureLayer, validate_shader_stage,
};
use lunco_render::{ProceduralSkybox, SurfaceAlpha};
use std::sync::Arc;

/// Shared `ShaderMaterial` per distinct [`ShaderLookKey`] — see the module docs.
/// Sharing, the `unshared` bypass, and eviction all live in
/// [`LookCache`](crate::look_cache::LookCache), shared with the PBR binder.
pub type ShaderLookCache = LookCache<ShaderLook>;

#[derive(Component)]
struct ShaderLookSourcePending {
    handle: Handle<Shader>,
    shader: String,
}

#[derive(Component)]
struct ShaderLookSourceHandle {
    handle: Handle<Shader>,
    shader: String,
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

struct ShaderLoadHandles {
    fragment: Handle<Shader>,
    vertex: Option<Handle<Shader>>,
}

pub(crate) fn load_shader(
    path: &str,
    asset_server: &AssetServer,
) -> Result<Handle<Shader>, String> {
    let path = lunco_assets_core::asset_path::load_asset_path(path, None, None, None)
        .map_err(|error| error.to_string())?;
    Ok(asset_server.load::<Shader>(path))
}

fn shader_load_handles(
    look: &ShaderLook,
    asset_server: &AssetServer,
) -> Result<ShaderLoadHandles, (ShaderStage, String)> {
    let fragment =
        load_shader(&look.shader, asset_server).map_err(|error| (ShaderStage::Fragment, error))?;
    let vertex = look
        .vertex_shader
        .as_deref()
        .map(|path| load_shader(path, asset_server))
        .transpose()
        .map_err(|error| (ShaderStage::Vertex, error))?;
    Ok(ShaderLoadHandles { fragment, vertex })
}

/// Build the concrete `ShaderMaterial` a look describes.
fn shader_material(
    look: &ShaderLook,
    handles: &ShaderLoadHandles,
    rasters: &ShaderImageAssets,
) -> ShaderMaterial {
    let mut m = ShaderMaterial {
        // A path, not a handle, in the intent — `bevy::shader` pulls naga, so the
        // domain crate cannot hold `Handle<Shader>`. Reuse the admitted handle.
        vertex_shader: handles.vertex.clone(),
        // `live` params are real shader params — they are merely absent from the
        // sharing key, so a freshly-built material still has to carry them.
        values: look
            .values()
            .iter()
            .map(|(name, value)| (name.clone(), *value))
            .chain(
                look.live_values()
                    .map(|(name, value)| (name.to_owned(), value)),
            )
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
            TextureLayer::ContinuationAlbedo => &mut m.continuation_albedo_map,
            TextureLayer::ContinuationSurface => &mut m.continuation_surface_map,
            TextureLayer::SurfaceAnnotations => &mut m.surface_annotations,
        };
        *slot = rasters.get(image).cloned();
    }
    // Packs against the (initially empty) schema; `reflect_shader_schemas` upgrades
    // it and repacks once the WGSL source lands. Same lifecycle as every other
    // `ShaderMaterial` in the codebase.
    m.repack();
    build_shader_material(handles.fragment.clone(), m)
}

/// Shader-source facts shared by binding, readiness, and reflection.
/// Asset publication invalidates them before the next render-owner Update.
#[derive(Resource, Default)]
pub(crate) struct ShaderSourceCache {
    ids_by_path: HashMap<String, AssetId<Shader>>,
    stage_failures: HashMap<(AssetId<Shader>, ShaderStage), Option<String>>,
    schemas: HashMap<AssetId<Shader>, Option<Arc<ParamSchema>>>,
    interfaces: HashMap<AssetId<Shader>, ShaderLookSourceInterface>,
}

impl ShaderSourceCache {
    fn shader_id(
        &mut self,
        path: &str,
        asset_server: &AssetServer,
    ) -> Result<AssetId<Shader>, String> {
        if let Some(id) = self.ids_by_path.get(path) {
            return Ok(*id);
        }
        let id = load_shader(path, asset_server)?.id();
        self.ids_by_path.insert(path.to_owned(), id);
        Ok(id)
    }

    pub(crate) fn failure(
        &mut self,
        id: AssetId<Shader>,
        stage: ShaderStage,
        source: &str,
    ) -> Option<String> {
        self.stage_failures
            .entry((id, stage))
            .or_insert_with(|| {
                let _span =
                    bevy::log::info_span!("shader_source_validate", shader = ?id, stage = ?stage)
                        .entered();
                validate_shader_stage(source, stage)
                    .err()
                    .map(|error| error.to_string())
            })
            .clone()
    }

    pub(crate) fn schema(&mut self, id: AssetId<Shader>, source: &str) -> Option<Arc<ParamSchema>> {
        self.schemas
            .entry(id)
            .or_insert_with(|| {
                let _span = bevy::log::info_span!("shader_source_schema", shader = ?id).entered();
                ParamSchema::parse(source).map(Arc::new)
            })
            .clone()
    }

    fn interface(
        &mut self,
        id: AssetId<Shader>,
        path: &str,
        shader: &Shader,
    ) -> ShaderLookSourceInterface {
        if !self.interfaces.contains_key(&id) {
            let source = wgsl_source(shader);
            let source_valid = source
                .is_some_and(|source| self.failure(id, ShaderStage::Fragment, source).is_none());
            let schema = source.and_then(|source| self.schema(id, source));
            self.interfaces.insert(
                id,
                ShaderLookSourceInterface {
                    shader: shader.path.clone(),
                    identifier: source
                        .and_then(lunco_materials::dyn_params::shader_interface_identifier)
                        .map(str::to_owned),
                    source_valid,
                    capabilities: source
                        .map(lunco_materials::dyn_params::shader_capabilities)
                        .unwrap_or_default(),
                    defaults: schema
                        .as_ref()
                        .map(|schema| {
                            schema
                                .fields
                                .iter()
                                .filter_map(|field| {
                                    field.default.map(|value| (field.name.clone(), value))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                },
            );
        }
        let mut interface = self.interfaces[&id].clone();
        if interface.shader != path {
            interface.shader = path.to_owned();
        }
        interface
    }

    fn invalidate(&mut self, id: AssetId<Shader>) {
        self.ids_by_path.retain(|_, cached| *cached != id);
        self.stage_failures.retain(|(cached, _), _| *cached != id);
        self.schemas.remove(&id);
        self.interfaces.remove(&id);
    }
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
    cache: &mut ShaderSourceCache,
) -> Option<(ShaderStage, String)> {
    let shaders = shaders?;
    let fragment_id = match cache.shader_id(&look.shader, asset_server) {
        Ok(id) => id,
        Err(error) => return Some((ShaderStage::Fragment, error)),
    };
    if let Some(source) = shaders.get(fragment_id).and_then(wgsl_source) {
        if let Some(detail) = cache.failure(fragment_id, ShaderStage::Fragment, source) {
            return Some((ShaderStage::Fragment, detail));
        }
    }

    let vertex_path = look.vertex_shader.as_deref()?;
    let vertex_id = match cache.shader_id(vertex_path, asset_server) {
        Ok(id) => id,
        Err(error) => return Some((ShaderStage::Vertex, error)),
    };
    shaders
        .get(vertex_id)
        .and_then(wgsl_source)
        .and_then(|source| {
            cache
                .failure(vertex_id, ShaderStage::Vertex, source)
                .map(|detail| (ShaderStage::Vertex, detail))
        })
}

fn clear_shader_render_components(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .try_remove::<MeshMaterial3d<ShaderMaterial>>()
        .try_remove::<MeshMaterial3d<StandardMaterial>>()
        .try_remove::<ShaderLookBound>()
        .try_remove::<ShaderLookReady>()
        .try_remove::<ShaderLookSourceInterface>()
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
    rasters: &ShaderImageAssets,
) -> Result<Option<Handle<ShaderMaterial>>, (ShaderStage, String)> {
    if look
        .textures
        .values()
        .any(|source| rasters.get(source).is_none())
    {
        return Ok(None);
    }
    let handles = shader_load_handles(look, asset_server)?;
    Ok(Some(cache.resolve(look, materials, |look| {
        shader_material(look, &handles, rasters)
    })))
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
    skybox: bool,
    materials: &Assets<ShaderMaterial>,
    commands: &mut Commands,
) {
    let Some(material) = materials.get(&handle) else {
        error!("shader material disappeared before render binding");
        clear_shader_render_components(commands, entity);
        lunco_core::trigger_runtime_error(
            commands,
            "shader-material-binding-failed",
            "shader material disappeared before render binding",
        );
        return;
    };
    let shader = material.shader.clone();
    let mut entity_commands = commands.entity(entity);
    entity_commands.try_remove::<MeshMaterial3d<StandardMaterial>>();
    if skybox {
        entity_commands.try_remove::<MeshMaterial3d<ShaderMaterial>>();
        entity_commands.try_insert((
            ShaderLookBound,
            crate::procedural_sky::ProceduralSkyboxMaterial::new(handle, shader),
        ));
    } else {
        entity_commands.try_insert((MeshMaterial3d(handle), ShaderLookBound));
        entity_commands.try_remove::<crate::procedural_sky::ProceduralSkyboxMaterial>();
    }
}

/// Does the material carry exactly the texture set the look states?
///
/// Slot-by-slot identity compare, so a driven TEXTURED look can take the
/// param-only update path. A textured look retains its material when its
/// resolved image identities are unchanged.
fn textures_match(m: &ShaderMaterial, look: &ShaderLook, rasters: &ShaderImageAssets) -> bool {
    use TextureLayer::*;
    [
        Height,
        Albedo,
        Mineral,
        Surface,
        Normal,
        ShadowCache,
        ContinuationAlbedo,
        ContinuationSurface,
        SurfaceAnnotations,
    ]
    .iter()
    .all(|layer| {
        let slot = match layer {
            Height => &m.height_map,
            Albedo => &m.albedo_map,
            Mineral => &m.mineral_map,
            Surface => &m.surface_map,
            Normal => &m.normal_map,
            ShadowCache => &m.shadow_cache,
            ContinuationAlbedo => &m.continuation_albedo_map,
            ContinuationSurface => &m.continuation_surface_map,
            SurfaceAnnotations => &m.surface_annotations,
        };
        slot.as_ref().map(Handle::id)
            == look
                .textures
                .get(layer)
                .and_then(|image| rasters.get(image))
                .map(Handle::id)
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
    rasters: ShaderImageAssets,
    shaders: Option<Res<Assets<Shader>>>,
    mut shader_cache: ResMut<ShaderSourceCache>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    if let Some((stage, detail)) =
        loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server, &mut shader_cache)
    {
        clear_shader_render_components(&mut commands, e);
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
        }
        return;
    }
    let handle = match material_for(look, &mut cache, &mut materials, &asset_server, &rasters) {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            clear_shader_render_components(&mut commands, e);
            return;
        }
        Err((stage, detail)) => {
            clear_shader_render_components(&mut commands, e);
            error!("shader asset admission failed: {detail}");
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
            }
            return;
        }
    };
    // Appearance intent is exclusive, but USD's visual projection and this
    // observer run in different schedules. A `PbrLook` may therefore already
    // have produced its concrete material before the projection swaps to a
    // `ShaderLook`. Remove that stale draw before adding ours: leaving both
    // material component types on one mesh submits it twice with incompatible
    // pipelines (visible as bright, serrated fragments at wheel silhouettes).
    let skybox = skyboxes.get(e).is_ok();
    bind_shader_render_components(e, handle, skybox, &materials, &mut commands);
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
    rasters: ShaderImageAssets,
    shaders: Option<Res<Assets<Shader>>>,
    mut shader_cache: ResMut<ShaderSourceCache>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    if let Some((stage, detail)) =
        loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server, &mut shader_cache)
    {
        clear_shader_render_components(&mut commands, e);
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
        }
        return;
    }
    let handle = match material_for(look, &mut cache, &mut materials, &asset_server, &rasters) {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            clear_shader_render_components(&mut commands, e);
            return;
        }
        Err((stage, detail)) => {
            clear_shader_render_components(&mut commands, e);
            error!("shader asset admission failed: {detail}");
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
            }
            return;
        }
    };
    bind_shader_render_components(e, handle, true, &materials, &mut commands);
}

/// Mirror the shader look's independent cast and receive intent onto Bevy markers.
///
/// Both markers belong to render-free `bevy_light`, but they are applied
/// *here*, in the only crate that binds materials, so the render-free half of the
/// graph states the intent and never names the flag.
///
/// The shader look is the exclusive appearance owner, so this is a full
/// reconciliation: clearing an opt-out removes its stale derived marker.
fn apply_shadow_intent(commands: &mut Commands, e: Entity, look: &ShaderLook) {
    if look.no_shadow_cast {
        commands.entity(e).try_insert(NotShadowCaster);
    } else {
        commands.entity(e).try_remove::<NotShadowCaster>();
    }
    if look.no_shadow_receive {
        commands.entity(e).try_insert(NotShadowReceiver);
    } else {
        commands.entity(e).try_remove::<NotShadowReceiver>();
    }
}

/// Re-bind when a look is edited in place — a terrain tile changing mode,
/// an overlay re-tune, a late-bound derived map, an Inspector edit.
///
/// Structural shared-look changes swap a cached handle. Live parameters and
/// explicitly unshared looks update their material in place. Unchanged looks
/// perform no binding work.
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
    rasters: ShaderImageAssets,
    shaders: Option<Res<Assets<Shader>>>,
    images: Option<Res<Assets<Image>>>,
    schemas: Option<Res<crate::ShaderSchemas>>,
    mut commands: Commands,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut shader_cache: ResMut<ShaderSourceCache>,
) {
    // Shared materials already written this run. Every terrain tile carries the same
    // global overlay values, so without this the one material they share would be
    // re-packed once per tile per change — hundreds of redundant writes per frame.
    let mut written: HashSet<AssetId<ShaderMaterial>> = HashSet::default();

    for (e, look, current, was_ready, skybox) in &changed {
        if look
            .textures
            .values()
            .any(|source| rasters.get(source).is_none())
        {
            clear_shader_render_components(&mut commands, e);
            continue;
        }
        if let Some((stage, detail)) =
            loaded_shader_stage_failure(look, shaders.as_deref(), &asset_server, &mut shader_cache)
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
                // asset-ID cache, avoiding repeated path resolution. Textures compare slot-by-slot
                // (`textures_match`): a TEXTURED look whose texture SET is
                // unchanged takes the cheap param path like everything else.
                let want_shader_id = match shader_cache.shader_id(&look.shader, &asset_server) {
                    Ok(id) => id,
                    Err(detail) => {
                        clear_shader_render_components(&mut commands, e);
                        error!("shader asset admission failed: {detail}");
                        if let Some(diagnostics) = diagnostics.as_deref_mut() {
                            record_loaded_shader_stage_failure(
                                diagnostics,
                                e,
                                look,
                                ShaderStage::Fragment,
                                detail,
                            );
                        }
                        continue;
                    }
                };
                let want_vertex_shader_id = match look
                    .vertex_shader
                    .as_deref()
                    .map(|path| shader_cache.shader_id(path, &asset_server))
                    .transpose()
                {
                    Ok(id) => id,
                    Err(detail) => {
                        clear_shader_render_components(&mut commands, e);
                        error!("shader asset admission failed: {detail}");
                        if let Some(diagnostics) = diagnostics.as_deref_mut() {
                            record_loaded_shader_stage_failure(
                                diagnostics,
                                e,
                                look,
                                ShaderStage::Vertex,
                                detail,
                            );
                        }
                        continue;
                    }
                };
                let structural = existing.shader.id() != want_shader_id
                    || existing.vertex_shader.as_ref().map(Handle::id) != want_vertex_shader_id
                    || !textures_match(&existing, look, &rasters);
                if structural {
                    let handles = match shader_load_handles(look, &asset_server) {
                        Ok(handles) => handles,
                        Err((stage, detail)) => {
                            clear_shader_render_components(&mut commands, e);
                            error!("shader asset admission failed: {detail}");
                            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                                record_loaded_shader_stage_failure(
                                    diagnostics,
                                    e,
                                    look,
                                    stage,
                                    detail,
                                );
                            }
                            continue;
                        }
                    };
                    let schema = existing.schema.clone();
                    *existing = shader_material(look, &handles, &rasters);
                    existing.set_schema(schema);
                    // The rebuild loaded the shader afresh; make the id cache agree
                    // with the material so the compare above stays quiet next tick.
                    shader_cache
                        .ids_by_path
                        .insert(look.shader.clone(), existing.shader.id());
                    if let (Some(path), Some(id)) = (
                        look.vertex_shader.as_deref(),
                        existing.vertex_shader.as_ref().map(Handle::id),
                    ) {
                        shader_cache.ids_by_path.insert(path.to_owned(), id);
                    }
                } else {
                    existing.set_many(
                        look.values()
                            .iter()
                            .map(|(name, value)| (name.as_str(), *value))
                            .chain(look.live_values()),
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
                                existing.shader.clone(),
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
        let handle = match material_for(look, &mut cache, &mut materials, &asset_server, &rasters) {
            Ok(Some(handle)) => handle,
            Ok(None) => {
                clear_shader_render_components(&mut commands, e);
                continue;
            }
            Err((stage, detail)) => {
                clear_shader_render_components(&mut commands, e);
                error!("shader asset admission failed: {detail}");
                if let Some(diagnostics) = diagnostics.as_deref_mut() {
                    record_loaded_shader_stage_failure(diagnostics, e, look, stage, detail);
                }
                continue;
            }
        };
        let same_material = current.is_some_and(|m| m.0.id() == handle.id());
        bind_shader_render_components(e, handle.clone(), skybox, &materials, &mut commands);
        // A content-key change normally needs to clear the entity's readiness
        // latch: the replacement material may still be waiting for reflection
        // or one of its declared images. Edge-stitch updates are the important
        // exception for streamed terrain: they select another cached material
        // with the same already-loaded shader and images. Preserve readiness when
        // that replacement is already render-ready, otherwise the deferred
        // remove/re-add cycle makes the terrain cover alternate every ECS turn.
        if !same_material && was_ready {
            let replacement_ready = materials
                .get(&handle)
                .zip(shaders.as_deref())
                .zip(images.as_deref())
                .zip(schemas.as_deref())
                .is_some_and(|(((material, shaders), images), schemas)| {
                    material_is_render_ready(material, shaders, images, schemas, &mut shader_cache)
                });
            if !replacement_ready {
                commands.entity(e).try_remove::<ShaderLookReady>();
            }
        }

        // The look changed but resolved to the material it is ALREADY on ⇒ only
        // `live` params moved (they are outside the key). Write them into that
        // material rather than leaving it stale: re-keying is what mints a new,
        // unprepared material every slider tick and makes the terrain flicker.
        if same_material && look.has_live_values() && written.insert(handle.id()) {
            if let Some(mut mat) = materials.get_mut(&handle) {
                mat.set_many(look.live_values());
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
    rasters: ShaderImageAssets,
    mut commands: Commands,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut shader_cache: ResMut<ShaderSourceCache>,
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
        let handles = match shader_load_handles(look, &asset_server) {
            Ok(handles) => handles,
            Err((stage, detail)) => {
                error!("shader asset admission failed: {detail}");
                findings.push(lunco_core::RuntimeDiagnostic {
                    code: "render-shader-stage".to_owned(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "shader-render".to_owned(),
                    subject: format!("entity {entity:?}"),
                    message: format!("{stage:?} shader admission failed: {detail}"),
                });
                rejections.push(entity);
                continue;
            }
        };
        let fragment = handles.fragment;
        let vertex = handles.vertex;
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
                shader_cache
                    .failure(fragment.id(), ShaderStage::Fragment, source)
                    .map(|error| (ShaderStage::Fragment, error))
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
                        shader_cache
                            .failure(vertex_handle.id(), ShaderStage::Vertex, source)
                            .map(|error| (ShaderStage::Vertex, error))
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
        let handle = match material_for(look, &mut cache, &mut materials, &asset_server, &rasters) {
            Ok(Some(handle)) => handle,
            Ok(None) => {
                clear_shader_render_components(&mut commands, entity);
                continue;
            }
            Err((stage, detail)) => {
                error!("shader asset admission failed: {detail}");
                clear_shader_render_components(&mut commands, entity);
                findings.push(lunco_core::RuntimeDiagnostic {
                    code: "render-shader-stage".to_owned(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "shader-render".to_owned(),
                    subject: format!("entity {entity:?}"),
                    message: format!("{stage:?} shader admission failed: {detail}"),
                });
                continue;
            }
        };
        bind_shader_render_components(entity, handle, skybox, &materials, &mut commands);
        apply_shadow_intent(&mut commands, entity, look);
    }
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("shader-render", findings);
    }
}

/// Wire the `ShaderLook` binder into an app. Called by
/// [`LuncoRenderPlugin`](crate::LuncoRenderPlugin).
///
/// NOTE: this does **not** add [`ShaderMaterialPlugin`](crate::ShaderMaterialPlugin)
/// — [`LuncoRenderPlugin`](crate::LuncoRenderPlugin) installs that plugin once,
/// after this binder. Keeping the two separate lets this
/// binder be unit-tested on a bare `MinimalPlugins` app, with no render pipeline.
pub(crate) fn build(app: &mut App) {
    if !app.is_plugin_added::<lunco_materials::LuncoImagePlugin>() {
        app.add_plugins(lunco_materials::LuncoImagePlugin);
    }
    // Register only absent stores: init_asset replaces Assets and its handle
    // provider, so re-registering an existing Shader store breaks live handles.
    // The binder needs both stores even without the GPU pipeline plugin.
    if !app.world().contains_resource::<Assets<ShaderMaterial>>() {
        bevy::asset::AssetApp::init_asset::<ShaderMaterial>(app);
    }
    if !app.world().contains_resource::<Assets<Shader>>() {
        bevy::asset::AssetApp::init_asset::<Shader>(app);
    }
    app.init_resource::<ShaderSourceCache>()
        .add_systems(
            PostUpdate,
            (
                invalidate_shader_source_cache,
                refresh_resized_image_materials,
            )
                .after(bevy::asset::AssetEventSystems),
        )
        .init_resource::<ShaderLookCache>()
        .add_observer(bind_shader_look)
        .add_observer(bind_added_skybox_shader_look)
        .add_systems(
            Update,
            (
                queue_shader_look_source_reflection,
                reflect_shader_look_source_interfaces,
                refresh_prepared_shader_looks.before(rebind_changed_shader_look),
                rebind_changed_shader_look,
                invalidate_shader_look_ready,
                mark_shader_look_ready.after(crate::reflect_shader_schemas),
                validate_shader_assets_on_change.after(crate::reflect_shader_schemas),
                sweep_look_cache::<ShaderLook>,
            ),
        );
    // Shader parameters become connection targets in `lunco-usd-sim`'s
    // `lunco-usd-sim-shader::ports` — beside the pass that authors `ShaderLook::driven`, so a
    // shader wire lands in a headless build too. The writes arrive in
    // `ShaderLook::live`, which `rebind_changed_shader_look` above drains.
}

/// Resolve the actual WGSL interface for every authored look, including USD
/// material carriers that do not own a render mesh. The source path remains in
/// `ShaderLook`; this render-side fact only records what the loaded asset says.
fn queue_shader_look_source_reflection(
    mut commands: Commands,
    changed: Query<(Entity, &ShaderLook, Option<&ShaderLookSourceHandle>), Changed<ShaderLook>>,
    asset_server: Option<Res<AssetServer>>,
) {
    let Some(asset_server) = asset_server else {
        return;
    };
    for (entity, look, current) in &changed {
        let handle = match load_shader(&look.shader, &asset_server) {
            Ok(handle) => handle,
            Err(error) => {
                commands
                    .entity(entity)
                    .try_remove::<ShaderLookSourceInterface>()
                    .try_remove::<ShaderLookSourceHandle>()
                    .try_remove::<ShaderLookSourcePending>();
                error!("shader reflection asset admission failed: {error}");
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    "shader-source-admission-failed",
                    error,
                );
                continue;
            }
        };
        if current.is_some_and(|current| current.handle.id() == handle.id()) {
            continue;
        }
        commands
            .entity(entity)
            .try_remove::<ShaderLookSourceInterface>()
            .try_remove::<ShaderLookSourceHandle>()
            .try_insert(ShaderLookSourcePending {
                handle,
                shader: look.shader.clone(),
            });
    }
}

/// Publish shader-source reflection after asset loading or hot reload. Pending
/// entries are only the unresolved shader assets; resolved looks are event
/// driven and do not rescan the shader store each frame.
fn reflect_shader_look_source_interfaces(
    mut commands: Commands,
    mut shader_events: Option<MessageReader<AssetEvent<Shader>>>,
    mut removed_looks: RemovedComponents<ShaderLook>,
    pending: Query<(Entity, &ShaderLookSourcePending)>,
    reflected: Query<(Entity, &ShaderLookSourceHandle)>,
    shaders: Option<Res<Assets<Shader>>>,
    asset_server: Option<Res<AssetServer>>,
    mut shader_cache: ResMut<ShaderSourceCache>,
) {
    let (Some(shaders), Some(asset_server), Some(shader_events)) =
        (shaders, asset_server, shader_events.as_mut())
    else {
        return;
    };
    let mut changed = HashSet::new();
    let mut removed = HashSet::new();
    for entity in removed_looks.read() {
        commands
            .entity(entity)
            .try_remove::<ShaderLookSourceInterface>()
            .try_remove::<ShaderLookSourcePending>()
            .try_remove::<ShaderLookSourceHandle>();
    }
    for event in shader_events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                changed.insert(*id);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                removed.insert(*id);
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }

    if !changed.is_empty() || !removed.is_empty() {
        for (entity, handle) in &reflected {
            if removed.contains(&handle.handle.id()) {
                commands
                    .entity(entity)
                    .try_remove::<ShaderLookSourceInterface>()
                    .try_remove::<ShaderLookSourceHandle>()
                    .try_insert(ShaderLookSourcePending {
                        handle: handle.handle.clone(),
                        shader: handle.shader.clone(),
                    });
            } else if changed.contains(&handle.handle.id()) {
                if let Some(shader) = shaders.get(&handle.handle) {
                    commands.entity(entity).try_insert(shader_cache.interface(
                        handle.handle.id(),
                        &handle.shader,
                        shader,
                    ));
                }
            }
        }
    }

    for (entity, pending) in &pending {
        if let Some(shader) = shaders.get(&pending.handle) {
            commands
                .entity(entity)
                .try_remove::<ShaderLookSourcePending>()
                .try_insert((
                    shader_cache.interface(pending.handle.id(), &pending.shader, shader),
                    ShaderLookSourceHandle {
                        handle: pending.handle.clone(),
                        shader: pending.shader.clone(),
                    },
                ));
        } else if asset_server
            .get_load_state(pending.handle.id())
            .is_some_and(|state| state.is_failed())
        {
            // A terminal asset failure is reflected as a resolved-invalid
            // source so required consumers can fail visibly instead of waiting
            // forever for an event that cannot arrive.
            commands
                .entity(entity)
                .try_remove::<ShaderLookSourcePending>()
                .try_insert((
                    ShaderLookSourceInterface {
                        shader: pending.shader.clone(),
                        identifier: None,
                        source_valid: false,
                        capabilities: Default::default(),
                        defaults: Default::default(),
                    },
                    ShaderLookSourceHandle {
                        handle: pending.handle.clone(),
                        shader: pending.shader.clone(),
                    },
                ));
        }
    }
}

fn invalidate_shader_source_cache(
    mut events: MessageReader<AssetEvent<Shader>>,
    mut cache: ResMut<ShaderSourceCache>,
) {
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id } => cache.invalidate(*id),
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
}

/// Image content uploads reuse the GPU binding only while its descriptor stays
/// unchanged. A resized image keeps its asset ID but receives a new GPU texture;
/// dependent materials must rebuild their bind groups without hiding the mesh.
fn refresh_resized_image_materials(
    mut events: MessageReader<AssetEvent<Image>>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    mut descriptors: Local<
        HashMap<AssetId<Image>, bevy::render::render_resource::TextureDescriptor<'static>>,
    >,
) {
    let mut resized = HashSet::new();
    for event in events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let Some(image) = images.get(*id) else {
                    continue;
                };
                let previous = descriptors.insert(*id, image.texture_descriptor.clone());
                if previous.as_ref() != Some(&image.texture_descriptor)
                    && matches!(event, AssetEvent::Modified { .. })
                {
                    resized.insert(*id);
                }
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                descriptors.remove(id);
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
    if resized.is_empty() {
        return;
    }
    let affected: Vec<_> = materials
        .iter()
        .filter_map(|(id, material)| {
            material_texture_handles(material)
                .any(|handle| resized.contains(&handle.id()))
                .then_some(id)
        })
        .collect();
    for id in affected {
        if let Some(material) = materials.get_mut(id) {
            // AssetMut is lazy: explicitly publish the descriptor-dependent
            // bind-group refresh even though material fields stay unchanged.
            let _ = material.into_inner();
        }
    }
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
    q: Query<(Entity, &MeshMaterial3d<ShaderMaterial>), With<ShaderLook>>,
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
        let image_changed = changed_images
            .iter()
            .any(|id| material_texture_handles(material_asset).any(|handle| handle.id() == *id));
        if shader_changed || image_changed {
            let mut entity = commands.entity(entity);
            if shader_changed {
                entity.try_remove::<ShaderLookSourceInterface>();
            }
            entity.try_remove::<ShaderLookReady>();
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
    cache: &mut ShaderSourceCache,
) -> bool {
    let Some(shader) = shaders.get(&material.shader) else {
        return false;
    };
    let Some(source) = wgsl_source(shader) else {
        return false;
    };
    if cache
        .failure(material.shader.id(), ShaderStage::Fragment, source)
        .is_some()
    {
        return false;
    }
    let schema_ready = if let Some(reflected) = schemas.get(material.shader.id()) {
        Arc::ptr_eq(reflected, &material.schema)
    } else {
        cache.schema(material.shader.id(), source).is_none()
    };
    let vertex_ready = material.vertex_shader.as_ref().is_none_or(|vertex_handle| {
        shaders
            .get(vertex_handle)
            .and_then(wgsl_source)
            .is_some_and(|source| {
                cache
                    .failure(vertex_handle.id(), ShaderStage::Vertex, source)
                    .is_none()
            })
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
    material_texture_handles(material).all(|handle| images.get(handle).is_some())
}

fn material_texture_handles(material: &ShaderMaterial) -> impl Iterator<Item = &Handle<Image>> {
    [
        material.height_map.as_ref(),
        material.albedo_map.as_ref(),
        material.mineral_map.as_ref(),
        material.surface_map.as_ref(),
        material.normal_map.as_ref(),
        material.shadow_cache.as_ref(),
        material.continuation_albedo_map.as_ref(),
        material.continuation_surface_map.as_ref(),
        material.surface_annotations.as_ref(),
    ]
    .into_iter()
    .flatten()
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
    mut shader_cache: ResMut<ShaderSourceCache>,
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
        if material_is_render_ready(material, &shaders, &images, schemas, &mut shader_cache) {
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
    fn shadow_receiver_intent_reconciles_independently_of_casting() {
        let mut app = App::new();
        app.add_systems(
            Update,
            |mut commands: Commands, looks: Query<(Entity, &ShaderLook)>| {
                for (entity, look) in &looks {
                    apply_shadow_intent(&mut commands, entity, look);
                }
            },
        );
        let terrain = app.world_mut().spawn(ShaderLook::new("surface.wgsl")).id();
        let mut shell_look = ShaderLook::new("surface.wgsl");
        shell_look.no_shadow_cast = true;
        shell_look.no_shadow_receive = true;
        assert_eq!(shell_look.key(), ShaderLook::new("surface.wgsl").key());
        let shell = app.world_mut().spawn(shell_look).id();
        app.update();
        assert!(app.world().entity(shell).contains::<NotShadowReceiver>());
        assert!(app.world().entity(shell).contains::<NotShadowCaster>());
        assert!(!app.world().entity(terrain).contains::<NotShadowReceiver>());
        assert!(!app.world().entity(terrain).contains::<NotShadowCaster>());

        app.world_mut()
            .entity_mut(shell)
            .get_mut::<ShaderLook>()
            .unwrap()
            .no_shadow_receive = false;
        app.update();
        assert!(!app.world().entity(shell).contains::<NotShadowReceiver>());
        assert!(app.world().entity(shell).contains::<NotShadowCaster>());
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
        let mut cache = ShaderSourceCache::default();

        assert!(material_is_render_ready(
            &material,
            app.world().resource::<Assets<Shader>>(),
            app.world().resource::<Assets<Image>>(),
            &schemas,
            &mut cache,
        ));

        material.height_map = Some(Handle::default());
        assert!(!material_is_render_ready(
            &material,
            app.world().resource::<Assets<Shader>>(),
            app.world().resource::<Assets<Image>>(),
            &schemas,
            &mut cache,
        ));
    }

    #[test]
    fn surface_annotation_resize_refreshes_only_dependent_materials() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>().init_asset::<ShaderMaterial>();
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let material = app
            .world_mut()
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial {
                surface_annotations: Some(image.clone()),
                ..Default::default()
            });
        app.world_mut()
            .write_message(AssetEvent::<Image>::Added { id: image.id() });
        app.world_mut()
            .run_system_cached(refresh_resized_image_materials)
            .unwrap();
        app.update();
        let mut events = app
            .world()
            .resource::<Messages<AssetEvent<ShaderMaterial>>>()
            .get_cursor_current();
        app.world_mut()
            .write_message(AssetEvent::<Image>::Modified { id: image.id() });
        app.world_mut()
            .run_system_cached(refresh_resized_image_materials)
            .unwrap();
        app.update();
        assert!(
            !events
                .read(
                    app.world()
                        .resource::<Messages<AssetEvent<ShaderMaterial>>>()
                )
                .any(|event| matches!(event,AssetEvent::Modified { id } if *id==material.id()))
        );
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&image)
            .unwrap()
            .texture_descriptor
            .size
            .height += 1;
        app.world_mut().clear_trackers();
        app.world_mut()
            .write_message(AssetEvent::<Image>::Modified { id: image.id() });
        app.world_mut()
            .run_system_cached(refresh_resized_image_materials)
            .unwrap();
        app.update();
        assert!(
            events
                .read(
                    app.world()
                        .resource::<Messages<AssetEvent<ShaderMaterial>>>()
                )
                .any(|event| matches!(event,AssetEvent::Modified { id } if *id==material.id()))
        );
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

    #[test]
    fn shader_source_facts_are_shared_and_invalidated_at_publication() {
        let mut app = app();
        let valid = "struct Material { gain: f32 }
@fragment fn fragment() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }";
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(valid, "test.wgsl"));
        // Publish initial addition before using the source cache.
        app.update();
        let id = shader.id();
        let source = Shader::from_wgsl(valid, "test.wgsl");
        let first_schema;
        {
            let mut cache = app.world_mut().resource_mut::<ShaderSourceCache>();
            assert_eq!(cache.failure(id, ShaderStage::Fragment, valid), None);
            assert!(cache.failure(id, ShaderStage::Vertex, valid).is_some());
            first_schema = cache.schema(id, valid).expect("Material layout");
            for _ in 0..500 {
                assert_eq!(cache.failure(id, ShaderStage::Fragment, valid), None);
                assert!(Arc::ptr_eq(
                    &first_schema,
                    &cache.schema(id, valid).unwrap()
                ));
                let interface = cache.interface(id, "alias.wgsl", &source);
                assert!(interface.source_valid);
                assert_eq!(interface.shader, "alias.wgsl");
            }
            assert_eq!(cache.stage_failures.len(), 2);
            assert_eq!(cache.schemas.len(), 1);
            assert_eq!(cache.interfaces.len(), 1);
            cache.ids_by_path.insert("test.wgsl".to_owned(), id);
        }
        // An in-place reload keeps asset identity but retires all derived facts.
        *app.world_mut()
            .resource_mut::<Assets<Shader>>()
            .get_mut(id)
            .unwrap() = Shader::from_wgsl("not a shader", "test.wgsl");
        app.update();
        {
            let mut cache = app.world_mut().resource_mut::<ShaderSourceCache>();
            assert!(cache.stage_failures.is_empty());
            assert!(cache.schemas.is_empty());
            assert!(cache.interfaces.is_empty());
            assert!(cache.ids_by_path.is_empty());
            let invalid = Shader::from_wgsl("not a shader", "test.wgsl");
            assert!(!cache.interface(id, "test.wgsl", &invalid).source_valid);
            assert!(
                cache
                    .failure(id, ShaderStage::Fragment, "not a shader")
                    .is_some()
            );
        }
        *app.world_mut()
            .resource_mut::<Assets<Shader>>()
            .get_mut(id)
            .unwrap() = Shader::from_wgsl(valid, "test.wgsl");
        app.update();
        {
            let mut cache = app.world_mut().resource_mut::<ShaderSourceCache>();
            assert!(cache.interface(id, "test.wgsl", &source).source_valid);
            let restored = cache.schema(id, valid).expect("restored layout");
            assert!(!Arc::ptr_eq(&first_schema, &restored));
        }
        app.world_mut().resource_mut::<Assets<Shader>>().remove(id);
        app.update();
        let cache = app.world().resource::<ShaderSourceCache>();
        assert!(cache.stage_failures.is_empty());
        assert!(cache.schemas.is_empty());
        assert!(cache.interfaces.is_empty());
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
            .set_value("morph_start", ParamValue::F32(0.5));
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
            .set_value("morph_start", ParamValue::F32(0.0));
        app.update();
        assert_eq!(material_of(&app, e), first);
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 2);
    }

    #[test]
    fn live_parameter_update_keeps_ready_cached_material() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn(ShaderLook::new("shaders/terrain_layered.wgsl"))
            .id();
        app.update();
        let handle = material_of(&app, e);
        app.world_mut().entity_mut(e).insert(ShaderLookReady);
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ShaderLook>()
            .expect("look")
            .set_live("transition", ParamValue::F32(0.5));

        app.update();

        assert_eq!(material_of(&app, e), handle);
        assert!(app.world().entity(e).contains::<ShaderLookReady>());
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

/// Source publication changes readiness, not authored texture identity.
/// Refresh only looks referencing an asset event; no idle scene scan occurs.
fn refresh_prepared_shader_looks(
    mut colors: MessageReader<AssetEvent<PreparedShaderImage<ColorMip>>>,
    mut linear: MessageReader<AssetEvent<PreparedShaderImage<LinearMip>>>,
    mut normals: MessageReader<AssetEvent<PreparedShaderImage<NormalMip>>>,
    mut looks: Query<&mut ShaderLook>,
) {
    let mut changed = HashSet::new();
    for event in colors.read() {
        let id = match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id }
            | AssetEvent::LoadedWithDependencies { id } => id,
        };
        changed.insert(id.untyped());
    }
    for event in linear.read() {
        let id = match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id }
            | AssetEvent::LoadedWithDependencies { id } => id,
        };
        changed.insert(id.untyped());
    }
    for event in normals.read() {
        let id = match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id }
            | AssetEvent::LoadedWithDependencies { id } => id,
        };
        changed.insert(id.untyped());
    }
    if changed.is_empty() {
        return;
    }
    for mut look in &mut looks {
        if look
            .textures
            .values()
            .any(|source| changed.contains(&source.id()))
        {
            look.set_changed();
        }
    }
}
