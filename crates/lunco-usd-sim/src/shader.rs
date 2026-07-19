//! USD → [`ShaderLook`] authoring, **race-free by construction**.
//!
//! [`ShaderLook`](lunco_materials::ShaderLook) is the custom-shader appearance
//! **intent**: a shader asset path plus an OPEN, user-defined parameter set
//! (`BTreeMap<String, ParamValue>` — exactly what a USD prim's authored primvars
//! are). It is render-free; `lunco-render-bevy` observes it and binds the real
//! `ShaderMaterial`. This crate therefore never names `MeshMaterial3d` — see
//! `docs/architecture/render-decoupling.md`.
//!
//! ## Why a system, not an observer
//!
//! The obvious implementation is an `On<Add, UsdVisualSynced>` observer that
//! swaps in the look. That works for prims nobody else touches
//! (balloons, panels), but it **races** any consumer that runs synchronously in
//! the same frame and mutates the prim's appearance — notably the wheel
//! physics/visual split in [`process_usd_sim_prims`](crate::process_usd_sim_prims),
//! which moves the look onto a child entity. An observer's `insert` is a
//! *deferred* command flushed at an unspecified later sync point, so some wheels
//! got split while still carrying the plain `PbrLook` → plain wheels.
//!
//! Instead this is a plain system, [`apply_usd_shader_materials`], explicitly
//! ordered `after(sync_usd_visuals).before(process_usd_sim_prims)`. Bevy's
//! automatic sync-point insertion flushes its commands *before* any consumer
//! runs, so the `ShaderLook` is always present by the time a wheel is split.
//! Adding a new consumer needs no special handling — the ordering guarantees it.
//!
//! ## One look per entity
//!
//! Taking the shader path **removes** the prim's [`PbrLook`]. An entity carrying
//! both would get a `MeshMaterial3d<StandardMaterial>` *and* a
//! `MeshMaterial3d<ShaderMaterial>` from the two binders — the mesh would draw
//! twice. Replacing the material is not enough; the intent must be removed.

use bevy::prelude::*;
use lunco_usd_bevy::{
    get_attribute_as_vec3, CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset, UsdVisualSynced,
};
use openusd::sdf::Path as SdfPath;
use lunco_materials::{to_snake_case, ParamValue, ShaderLook, TextureLayer};
use lunco_render::PbrLook;
use std::collections::BTreeMap;

/// Marks a prim whose `ShaderLook` authoring has been evaluated, so the
/// every-frame query collapses to empty once the scene settles. We mark a prim
/// resolved whether or not it actually wanted a shader (a non-shader prim is
/// "resolved: nothing to do"), but **only after its stage has loaded** — until
/// then we leave it unmarked and retry next frame.
#[derive(Component)]
pub struct UsdShaderResolved;

/// Authors [`ShaderLook`] from the `UsdShade` material a prim is bound to — the
/// `Shader` prim's `info:wgsl:sourceAsset` and its `inputs:`. Runs between
/// `sync_usd_visuals` and the sim consumers (see module docs).
pub fn apply_usd_shader_materials(
    q: Query<(Entity, &UsdPrimPath), (With<UsdVisualSynced>, Without<UsdShaderResolved>)>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset's recipe.
    mut canonical: NonSendMut<CanonicalStages>,
    mut commands: Commands,
    settings: Option<Res<lunco_settings::TerrainSettings>>,
    // For `asset`-typed shader inputs (texture layers): root-relative paths
    // resolve against the SCENE's own source root and load through the asset
    // server — the same authority rule as the sandbox layer binder (the scene
    // the material came from decides the root, never a guessed twin).
    asset_server: Res<AssetServer>,
) {
    let enable_shaders = settings.as_ref().map(|s| s.enable_shaders).unwrap_or(true);
    for (entity, prim_path) in q.iter() {
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        // No live stage (asset carries no recipe / build failed) yet → retry next
        // frame (do NOT mark resolved).
        let Some(cs) = canonical.get(id) else { continue };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            commands.entity(entity).try_insert(UsdShaderResolved);
            continue;
        };
        apply_usd_shader_material_read(
            &cs.view(), entity, prim_path, &sdf_path, &mut commands, enable_shaders,
            &asset_server,
        );
    }
}

/// Per-prim shader authoring, generic over the read source ([`UsdRead`]) — drives
/// off either the live canonical `StageView` or the flattened `sdf::Data`,
/// identically. Marks the prim [`UsdShaderResolved`] the moment its stage is
/// readable (whether or not it ends up wanting a shader).
#[allow(clippy::too_many_arguments)]
fn apply_usd_shader_material_read<R: UsdRead>(
    reader: &R,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: &SdfPath,
    commands: &mut Commands,
    enable_shaders: bool,
    asset_server: &AssetServer,
) {
    // From here on the prim is evaluated regardless of outcome.
    commands.entity(entity).try_insert(UsdShaderResolved);

    // TERRAIN prims are excluded: a DEM terrain's material is authored by the
    // terrain pipeline (engine-filled height/shadow params, derived maps), and
    // its bound Material network is consumed by the terrain layer binder
    // (`lunco-sandbox::bind_terrain_layers`) instead. Minting a ShaderLook
    // here would hand the terrain entity a SECOND material that replaces the
    // engine-authored one — losing the heightfield binding and every engine
    // param the moment a scene binds a Material to its Terrain prim.
    if reader.text(sdf_path, "lunco:assetMode").is_some() {
        return;
    }

    // A shader is bound the way USD binds shaders: `rel material:binding` → a
    // `Material` → the `Shader` its surface comes from. Nothing here is ours — a WGSL
    // shader is simply a shader whose source is named for the `wgsl` render context
    // (`info:wgsl:sourceAsset`), exactly as an MDL shader is named by
    // `info:mdl:sourceAsset`. So usdview, Blender and Omniverse all see a material
    // where a material is, and a prim bound to a `UsdPreviewSurface` instead keeps its
    // `PbrLook` — it has no `wgsl` source, and that is the whole test.
    let Some(shader_prim) = bound_shader_prim(reader, sdf_path) else {
        return;
    };
    let Some(raw_shader_path) = reader.asset(&shader_prim, "info:wgsl:sourceAsset") else {
        return;
    };
    // Normalise to the engine-library-relative form (strip a `lunco://` scheme) so an
    // authored `@lunco://shaders/x.wgsl@` and a bare `@shaders/x.wgsl@` behave
    // identically downstream: the string comparisons below, the `@fragment` pre-check,
    // and `engine_asset_uri` re-adding the scheme for the loader. A `twin://` custom
    // shader is left schemed and passes through untouched.
    let shader_path = lunco_assets::engine_asset_rel(&raw_shader_path).to_string();

    if !enable_shaders && (shader_path == "shaders/regolith.wgsl" || shader_path == "shaders/terrain_layered.wgsl") {
        return;
    }

    // ROBUSTNESS: refuse a shader that isn't a usable material shader. A pure
    // library (`#define_import_path`, meant to be `#import`ed — e.g.
    // pbr_lit.wgsl) has no `@fragment` entry, so binding a ShaderMaterial to it
    // builds an INVALID render pipeline that wgpu rejects on EVERY frame (the
    // `opaque_mesh_pipeline` validation storm → dropped frames / viewport
    // blink, and it poisons the pipeline cache until the app restarts). Keep
    // the `PbrLook` (displayColor) instead so the app renders normally.
    if !shader_has_fragment_entry(&shader_path) {
        warn!(
            "[shader] prim {} → '{}' has no `@fragment` entry point (it looks \
             like a shader LIBRARY, not a material shader). Keeping the \
             PbrLook to avoid an invalid render pipeline. Point \
             primvars:shaderPath at a whole shader (one with `@fragment fn …`).",
            prim_path.path, shader_path
        );
        return;
    }

    // The shader's parameters are the Shader prim's `inputs:` — typed, declared, and
    // belonging to the shader that consumes them.
    let values = read_shader_inputs(reader, &shader_prim);
    // `asset`-typed inputs are TEXTURE layers (doc 18 §3.1): `inputs:albedo_map =
    // @terrain/site/…/ortho.png@` fills the material slot of the same reflected
    // name. Root-relative paths resolve against the SCENE's source root (the
    // `twin://<name>` the stage itself was loaded from); already-schemed paths
    // pass through. A scene from Bevy's default source has no root to resolve
    // against — those inputs warn and skip rather than guess.
    let mut textures: BTreeMap<TextureLayer, Handle<Image>> = BTreeMap::new();
    for (layer, authored) in read_shader_texture_inputs(reader, &shader_prim) {
        let uri = if authored.contains("://") {
            authored
        } else {
            let Some(base) = scene_base_uri(prim_path, asset_server) else {
                warn!(
                    "[shader] prim {}: texture input `{authored}` is root-relative but \
                     the scene carries no source root to resolve it against — skipped",
                    prim_path.path
                );
                continue;
            };
            format!("{base}/{authored}")
        };
        textures.insert(layer, asset_server.load(&uri));
    }
    #[cfg(target_arch = "wasm32")]
    let resolved_shader_path = if shader_path == "shaders/regolith.wgsl" {
        "shaders/regolith_web.wgsl".to_string()
    } else if shader_path == "shaders/terrain_layered.wgsl" {
        "shaders/terrain_layered_web.wgsl".to_string()
    } else {
        shader_path
    };
    #[cfg(not(target_arch = "wasm32"))]
    let resolved_shader_path = shader_path;

    debug!("[shader] applied {} to {}", resolved_shader_path, prim_path.path);
    // A path, not a `Handle<Shader>`: `bevy::shader` pulls naga. The binder loads it.
    // Route a bare built-in reference (`shaders/wheel.wgsl`) through the `lunco://`
    // engine library so it resolves from ANYWHERE — including with an external Twin
    // open, where Bevy's default source is the wrong root and the shipped shader
    // would miss (→ a black-hole ShaderMaterial). An already-schemed `twin://…`
    // custom shader is passed through untouched. See `lunco_assets::engine_asset_uri`.
    let shader = lunco_assets::engine_asset_uri(&resolved_shader_path);
    // `primvars:doNotCastShadows` — read on the GPRIM, not on the shader, because
    // two prims sharing one material can legitimately disagree about casting. Same
    // attribute and same polarity the `PbrLook` path reads in `lunco-usd-bevy`;
    // it has to be carried here too because taking the shader path REMOVES the
    // `PbrLook`, which dropped the author's shadow intent on the floor the moment a
    // prim gained a `.wgsl`.
    let no_shadow_cast =
        lunco_usd_bevy::get_attribute_as_bool(reader, sdf_path, "primvars:doNotCastShadows")
            .unwrap_or(false);
    let look = ShaderLook { shader, values, textures, no_shadow_cast, ..Default::default() };
    // REMOVE the `PbrLook`, don't just overlay: an entity carrying both intents
    // gets two materials from the two binders and the mesh draws TWICE.
    commands
        .entity(entity)
        .remove::<PbrLook>()
        .try_insert(look);
}

/// True if `shader_path` is a usable material shader — i.e. it declares a
/// fragment entry point (`@fragment`). A pure shader LIBRARY (`#define_import_path`,
/// meant only to be `#import`ed) has none, and binding a material to it produces
/// an invalid render pipeline (see the call site).
///
/// Best-effort by design: it reads the on-disk source (native) and only VETOES a
/// shader we can positively prove lacks `@fragment`. If the file can't be read
/// (wasm, embedded source, or a path the loader resolves elsewhere), it returns
/// `true` so the normal asset path — and its own error handling — still runs; we
/// never block a shader we couldn't inspect.
#[cfg(not(target_arch = "wasm32"))]
fn shader_has_fragment_entry(shader_path: &str) -> bool {
    // Resolve through the SAME infra the loader uses (`lunco://` → `<cwd>/assets`),
    // so a schemed reference is inspected at its real path rather than becoming a
    // bogus `assets/lunco://…` segment that never exists (→ a wrong veto that would
    // starve the wheel's `ShaderLook` and deadlock physics on a render-only visual).
    let Some(full) = lunco_assets::engine_asset_local_path(shader_path) else {
        // Another scheme's root (`twin://…`) — can't inspect it here; don't veto.
        return true;
    };
    match std::fs::read_to_string(&full) {
        // Check the CODE portion of each line (before any `//`), so an EXAMPLE
        // `@fragment` inside a doc comment — as library shaders like pbr_lit.wgsl
        // carry to show how to import them — isn't mistaken for a real entry point.
        Ok(src) => src.lines().any(|line| {
            let code = line.split_once("//").map_or(line, |(c, _)| c);
            code.contains("@fragment")
        }),
        // A missing file can never build a valid pipeline → veto (fall back to the
        // StandardMaterial). Any OTHER read error (permissions, etc.) is treated as
        // "can't tell" → don't veto, so the normal asset path still runs.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(target_arch = "wasm32")]
fn shader_has_fragment_entry(_shader_path: &str) -> bool {
    true
}

/// The `Shader` prim whose surface a gprim's bound `Material` produces — the USD
/// path from a piece of geometry to the thing that shades it.
///
/// `rel material:binding` names the `Material`; the `Material`'s
/// `outputs:surface.connect` names the `Shader`. Both hops are stock `UsdShade`, and
/// both are composed reads, so a material bound through a reference arc resolves the
/// same as one authored in place.
///
/// `None` means this prim is not shaded by a material (most prims aren't — they carry
/// a `displayColor` and get a `PbrLook`).
///
/// `pub`: the terrain layer binder (`lunco-sandbox`) walks the same two hops
/// to read a terrain's Material network — one definition of "the bound
/// shader", not two that can drift.
pub fn bound_shader_prim<R: UsdRead>(reader: &R, sdf_path: &SdfPath) -> Option<SdfPath> {
    let material = reader.rel_target(sdf_path, "material:binding")?;
    let material = SdfPath::new(&material).ok()?;
    // `/Scene/Looks/Regolith/Shader.outputs:surface` → `/Scene/Looks/Regolith/Shader`
    let surface = reader.connection_source(&material, "outputs:surface")?;
    let (shader, _) = surface.rsplit_once('.')?;
    SdfPath::new(shader).ok()
}

/// Reads a `Shader` prim's authored `inputs:*` into the look's parameter map, by
/// name — so a shader is configured by exactly the parameters it declares, and each
/// one is a typed, schema-visible property of the shader that consumes it.
///
/// Colours are read as `vec3`, everything else as a scalar; the material's reflected
/// schema resolves the final packing once the shader loads. Names the shader does not
/// declare pack to nothing (harmless).
///
/// Inputs that are CONNECTED (fed by another shader node) are skipped: this pipeline
/// binds a single shader, not a graph, and a connected input has no authored value to
/// read anyway.
fn read_shader_inputs<R: UsdRead>(
    reader: &R,
    shader_prim: &SdfPath,
) -> BTreeMap<String, ParamValue> {
    let mut values = BTreeMap::new();
    for attr in reader.attr_names(shader_prim) {
        let Some(name) = attr.strip_prefix("inputs:") else { continue };
        if !reader.connections(shader_prim, &attr).is_empty() {
            continue;
        }
        let key = to_snake_case(name);
        if let Some(c) = get_attribute_as_vec3(reader, shader_prim, &attr) {
            values.insert(key, ParamValue::Vec4([c.x, c.y, c.z, 1.0]));
        } else if let Some(v) = reader.real_f32(shader_prim, &attr) {
            values.insert(key, ParamValue::F32(v));
        }
    }
    values
}

/// The texture slot a shader input name addresses — the `asset`-typed half of
/// the `inputs:` contract. Snake-cased names match the `ShaderMaterial` slot
/// fields ([`TextureLayer`] is the fixed, binding-limited set); anything else
/// returns `None` (a value param or a graph-only port, not a slot).
fn texture_layer_for_input(snake: &str) -> Option<TextureLayer> {
    match snake {
        "albedo_map" => Some(TextureLayer::Albedo),
        "mineral_map" => Some(TextureLayer::Mineral),
        "surface_map" => Some(TextureLayer::Surface),
        "normal_map" => Some(TextureLayer::Normal),
        // `height_map` and `shadow_cache` are engine-filled (horizon system);
        // authoring them from USD would fight the engine writer, so they are
        // deliberately NOT addressable here.
        _ => None,
    }
}

/// Reads the `asset`-typed `inputs:*` of a `Shader` prim: `(slot, authored
/// path)` pairs. CONNECTED inputs are skipped for the same reason as in
/// [`read_shader_inputs`] — a connected port is fed by a producer node
/// (doc 18 Tier B), not by an authored file.
fn read_shader_texture_inputs<R: UsdRead>(
    reader: &R,
    shader_prim: &SdfPath,
) -> Vec<(TextureLayer, String)> {
    let mut out = Vec::new();
    for attr in reader.attr_names(shader_prim) {
        let Some(name) = attr.strip_prefix("inputs:") else { continue };
        let Some(layer) = texture_layer_for_input(&to_snake_case(name)) else { continue };
        if !reader.connections(shader_prim, &attr).is_empty() {
            continue;
        }
        if let Some(path) = reader.asset(shader_prim, &attr) {
            out.push((layer, path));
        }
    }
    out
}

/// The `source://root` base URI of the scene a prim was loaded from — the root
/// its root-relative texture inputs resolve against. Same derivation as the
/// sandbox layer binder: the stage asset's own path is the only authority
/// (`twin://<name>/sim/scenes/x.usda` → `twin://<name>`). `None` for a stage
/// from Bevy's default source (no root to resolve against — caller warns).
fn scene_base_uri(prim_path: &UsdPrimPath, asset_server: &AssetServer) -> Option<String> {
    let asset_path = asset_server.get_path(prim_path.stage_handle.id())?;
    let source = match asset_path.source() {
        bevy::asset::io::AssetSourceId::Name(n) => n.to_string(),
        bevy::asset::io::AssetSourceId::Default => return None,
    };
    let root = asset_path
        .path()
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())?;
    Some(format!("{source}://{root}"))
}
