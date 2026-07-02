//! USD → [`ShaderMaterial`] binding, **race-free by construction**.
//!
//! The general [`ShaderMaterial`](lunco_materials::ShaderMaterial) is
//! engine-agnostic (it lives in `lunco-materials` and knows nothing about USD).
//! This module is the *one* place that authors it from USD primvars.
//!
//! ## Why a system, not an observer
//!
//! The obvious implementation is an `On<Add, UsdVisualSynced>` observer that
//! swaps in a `ShaderMaterial`. That works for prims nobody else touches
//! (balloons, panels), but it **races** any consumer that runs synchronously in
//! the same frame and mutates the prim's material — notably the wheel
//! physics/visual split in [`process_usd_sim_prims`](crate::process_usd_sim_prims),
//! which moves the material onto a child entity. An observer's `insert` is a
//! *deferred* command flushed at an unspecified later sync point, so some wheels
//! got split while still carrying the default `StandardMaterial` → plain wheels.
//!
//! Instead this is a plain system, [`apply_usd_shader_materials`], explicitly
//! ordered `after(sync_usd_visuals).before(process_usd_sim_prims)`. Bevy's
//! automatic sync-point insertion flushes its commands *before* any consumer
//! runs, so the `ShaderMaterial` is always present by the time a wheel is split.
//! Adding a new consumer needs no special handling — the ordering guarantees it.

use bevy::prelude::*;
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::{get_attribute_as_vec3, UsdData, UsdPrimPath, UsdStageAsset, UsdVisualSynced};
use openusd::sdf::Path as SdfPath;
use lunco_materials::{apply_param, ShaderMaterial};

/// Marks a prim whose `ShaderMaterial` authoring has been evaluated, so the
/// every-frame query collapses to empty once the scene settles. We mark a prim
/// resolved whether or not it actually wanted a shader (a non-shader prim is
/// "resolved: nothing to do"), but **only after its stage has loaded** — until
/// then we leave it unmarked and retry next frame.
#[derive(Component)]
pub struct UsdShaderResolved;

/// Authors [`ShaderMaterial`] from `primvars:materialType = "shader"` (or the
/// legacy alias `"usd_shader"`) + `primvars:shaderPath`, reading generic
/// colors/params from primvars. Runs between `sync_usd_visuals` and the sim
/// consumers (see module docs).
pub fn apply_usd_shader_materials(
    q: Query<(Entity, &UsdPrimPath), (With<UsdVisualSynced>, Without<UsdShaderResolved>)>,
    stages: Res<Assets<UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    // `Option<...>` so the system no-ops (instead of panicking on param
    // validation) in minimal apps that never register the `ShaderMaterial`
    // asset — e.g. headless tests using `MinimalPlugins` without the materials
    // plugin. Production always registers it, so behaviour there is unchanged.
    materials: Option<ResMut<Assets<ShaderMaterial>>>,
    mut commands: Commands,
) {
    let Some(mut materials) = materials else { return };
    for (entity, prim_path) in q.iter() {
        // Stage not loaded yet → retry next frame (do NOT mark resolved).
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            commands.entity(entity).insert(UsdShaderResolved);
            continue;
        };
        let reader = &*stage.reader;

        // From here on the prim is evaluated regardless of outcome.
        commands.entity(entity).insert(UsdShaderResolved);

        let mat_type: Option<String> =
            reader.prim_attribute_value(&sdf_path, "primvars:materialType");
        if !matches!(mat_type.as_deref(), Some("shader") | Some("usd_shader")) {
            continue;
        }

        let Some(shader_path) =
            reader.prim_attribute_value::<String>(&sdf_path, "primvars:shaderPath")
        else {
            warn!(
                "[shader] prim {} has materialType=shader but no primvars:shaderPath",
                prim_path.path
            );
            continue;
        };

        // ROBUSTNESS: refuse a shader that isn't a usable material shader. A pure
        // library (`#define_import_path`, meant to be `#import`ed — e.g.
        // pbr_lit.wgsl) has no `@fragment` entry, so binding a ShaderMaterial to it
        // builds an INVALID render pipeline that wgpu rejects on EVERY frame (the
        // `opaque_mesh_pipeline` validation storm → dropped frames / viewport
        // blink, and it poisons the pipeline cache until the app restarts). Keep
        // the StandardMaterial (displayColor) instead so the app renders normally.
        if !shader_has_fragment_entry(&shader_path) {
            warn!(
                "[shader] prim {} → '{}' has no `@fragment` entry point (it looks \
                 like a shader LIBRARY, not a material shader). Keeping the \
                 StandardMaterial to avoid an invalid render pipeline. Point \
                 primvars:shaderPath at a whole shader (one with `@fragment fn …`).",
                prim_path.path, shader_path
            );
            continue;
        }

        // Shader chosen by `primvars:shaderPath` (e.g. "shaders/wheel.wgsl");
        // generic colors/params come from primvars.
        let mut material = ShaderMaterial::default();
        read_authored_params(reader, &sdf_path, &mut material);
        material.shader = asset_server.load(&shader_path);

        debug!("[shader] applied {} to {}", shader_path, prim_path.path);
        let handle = materials.add(material);
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(handle));
    }
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

/// Reads every authored `primvars:*` attribute (except the shader-routing
/// `materialType`/`shaderPath`) into the material **by its real name** — so
/// each prim authors exactly the parameters its shader's `Material` struct
/// declares. Names the shader doesn't declare pack to nothing (harmless).
/// Colours are read as `vec3`, everything else as a scalar; the material's
/// reflected schema resolves the final type once the shader loads.
///
/// A prim's attributes are child specs at `<prim>.<attr>`, so we enumerate the
/// reader's specs and keep the ones directly under this prim (split on the USD
/// `.` property separator) — no hardcoded parameter names.
fn read_authored_params(reader: &UsdData, sdf_path: &SdfPath, m: &mut ShaderMaterial) {
    let prefix = format!("{}.", sdf_path.as_str());
    let attr_names: Vec<String> = reader
        .iter()
        .filter_map(|(p, _)| p.as_str().strip_prefix(&prefix).map(str::to_string))
        .collect();
    for attr in attr_names {
        let Some(name) = attr.strip_prefix("primvars:") else { continue };
        if name == "materialType" || name == "shaderPath" {
            continue;
        }
        if let Some(c) = get_attribute_as_vec3(reader, sdf_path, &attr) {
            apply_param(m, name, &format!("{},{},{}", c.x, c.y, c.z));
        } else if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, &attr) {
            apply_param(m, name, &v.to_string());
        } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, &attr) {
            apply_param(m, name, &(v as f32).to_string());
        }
    }
}
