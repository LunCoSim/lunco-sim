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
use lunco_materials::{to_snake_case, ParamValue, ShaderLook};
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
) {
    // From here on the prim is evaluated regardless of outcome.
    commands.entity(entity).try_insert(UsdShaderResolved);

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
    let Some(shader_path) = reader.asset(&shader_prim, "info:wgsl:sourceAsset") else {
        return;
    };

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
    let look = ShaderLook { shader, values, ..Default::default() };
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
    // Match the loader's resolution: `<cwd>/<assets_dir>/<shader_path>`.
    let full = std::env::current_dir()
        .unwrap_or_default()
        .join(lunco_assets::assets_dir())
        .join(shader_path);
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
fn bound_shader_prim<R: UsdRead>(reader: &R, sdf_path: &SdfPath) -> Option<SdfPath> {
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
