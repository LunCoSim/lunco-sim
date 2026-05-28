//! USD → cosim translator.
//!
//! Reads `lunco:modelicaModel` / `lunco:pythonModel` and `lunco:simWires`
//! attributes from USD prims after `sync_usd_visuals` has spawned the
//! entity, and drives the full cosim lifecycle end-to-end:
//!
//! - **Modelica**: opens the source file, inserts a `ModelicaModel`
//!   stub, dispatches `ModelicaCommand::Compile` directly to the
//!   worker channel, and (once `model.variables` populates) wraps the
//!   result in a `SimComponent` so the propagate / apply-forces
//!   pipeline can read it.
//! - **Python**: opens the script, registers a `ScriptDocument`,
//!   attaches `ScriptedModel`, and creates the matching `SimComponent`.
//! - **Wires**: each `lunco:simWires` entry spawns one
//!   `SimConnection` self-loop on the same entity (Modelica/Python
//!   ports ↔ AvianSim ports).
//!
//! Wire entry format inside `lunco:simWires`:
//! `"fromPort:toPort,fromPort:toPort:scale,..."` — comma-separated
//! because `string[]` arrays don't compose across `references` in the
//! current openusd parser.
//!
//! No domain-specific markers (`BalloonModelMarker`, …) are inserted
//! here. The legacy catalog/imperative spawn path in
//! `lunco-sandbox-edit` keeps using its own markers; this translator
//! is the authoritative path for USD-defined cosim entities.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_assets::assets_dir;
use lunco_core::{Command, on_command};
use lunco_cosim::{SimComponent, SimConnection, SimStatus};
use lunco_doc::DocumentId;
use lunco_modelica::{
    extract_inputs_with_defaults_from_ast, extract_model_name_from_ast,
    extract_parameters_from_ast, ModelicaChannels, ModelicaCommand, ModelicaModel,
};
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    ScriptRegistry,
};
use lunco_doc::DocumentHost;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use std::collections::HashMap;

use crate::UsdSimProcessed;

/// Marker indicating a USD-driven cosim entity has been wired up by
/// `process_usd_cosim_prims`. Prevents the system from re-processing
/// the same entity on subsequent ticks.
#[derive(Component, Default)]
pub struct UsdSourcedCosim;

/// Marker for cross-entity wire prims that have been resolved into a
/// `SimConnection`. Wire prims are typeless USD nodes carrying the
/// `lunco:wireFrom` / `lunco:wireTo` rels — the translator rescans
/// each frame until both endpoints exist as ECS entities, then spawns
/// the wire and tags itself with this marker.
#[derive(Component, Default)]
pub struct UsdSourcedWire;

/// Reads cosim attributes from USD prims and dispatches model
/// compilation + wires. Runs in `Update` after `sync_usd_visuals` so
/// `Transform` / `Mesh3d` / `Material` are already present.
pub fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdSourcedCosim>>,
    stages: Res<Assets<UsdStageAsset>>,
    channels: Res<ModelicaChannels>,
    mut script_registry: ResMut<ScriptRegistry>,
) {
    for (entity, prim_path) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };
        let reader = (*stage.reader).clone();

        // Gate on `lunco:simWires` presence — that's the attribute that
        // distinguishes a *wired* cosim entity (balloons, future devices)
        // from prims that merely declare a Modelica reference for
        // documentation (wheels, motors, batteries — `lunco:modelicaModel`
        // alone is a forward-looking schema, no translator behaviour).
        let Some(wires_csv) = reader.prim_attribute_value::<String>(&sdf_path, "lunco:simWires") else { continue; };

        let modelica_path = reader.prim_attribute_value::<String>(&sdf_path, "lunco:modelicaModel");
        let python_path = reader.prim_attribute_value::<String>(&sdf_path, "lunco:pythonModel");

        if modelica_path.is_none() && python_path.is_none() {
            warn!(
                "[usd-cosim] {} declares lunco:simWires but neither lunco:modelicaModel nor lunco:pythonModel — skipping",
                prim_path.path
            );
            continue;
        }

        commands.entity(entity).insert((
            UsdSourcedCosim,
            UsdSimProcessed,
            lunco_core::SelectableRoot,
        ));

        if let Some(rel) = modelica_path.as_ref() {
            dispatch_modelica(&mut commands, entity, rel, &channels);
        }
        if let Some(rel) = python_path.as_ref() {
            dispatch_python(&mut commands, entity, rel, &mut script_registry);
        }

        for raw in wires_csv.split(',') {
            let trimmed = raw.trim();
            if trimmed.is_empty() { continue; }
            if let Some((from_port, to_port, scale)) = parse_wire(trimmed) {
                commands.spawn(SimConnection {
                    start_element: entity,
                    start_connector: from_port,
                    end_element: entity,
                    end_connector: to_port,
                    scale,
                });
            } else {
                warn!("[usd-cosim] {} — could not parse wire entry '{}'", prim_path.path, trimmed);
            }
        }

        let kind = if modelica_path.is_some() { "modelica" } else { "python" };
        info!("[usd-cosim] wired {} ({}) from USD attrs", prim_path.path, kind);
    }
}

fn dispatch_modelica(
    commands: &mut Commands,
    entity: Entity,
    rel_path: &str,
    channels: &ModelicaChannels,
) {
    let model_path = assets_dir().join(rel_path);
    let source = match std::fs::read_to_string(&model_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[usd-cosim] failed to read Modelica source `{}`: {e}", model_path.display());
            return;
        }
    };

    // Single best-effort parse, three AST-driven extracts. The
    // string-call variants of these helpers re-parse on every call;
    // `_from_ast` lets us share one parse across all three. Lenient
    // parsing means a model with a semantic error still produces
    // usable name/parameter/input snapshots — same recovery
    // semantics `Session::recovered_file_query` uses on the engine
    // side.
    // Lenient parse: even a model with a semantic error produces
    // usable name/parameter/input snapshots. Inlined here (rather than
    // wrapped in a helper) so the parse cost is visible at the call
    // site — same principle as the AST-canonical engine surface
    // (see lunco-doc/domain_engine.rs).
    let ast = rumoca_phase_parse::parse_to_syntax(&source, "cosim-dispatch.mo")
        .best_effort()
        .clone();
    let model_name = extract_model_name_from_ast(&ast).unwrap_or_else(|| "Model".into());
    let parameters = extract_parameters_from_ast(&ast);
    let inputs = extract_inputs_with_defaults_from_ast(&ast).into_iter().collect();

    commands.entity(entity).insert(ModelicaModel {
        model_path,
        model_name: model_name.clone(),
        parameters,
        inputs,
        ..default()
    });

    let _ = channels.tx.send(ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
        extra_sources: Vec::new(),
        stream: None,
    });
}

fn dispatch_python(
    commands: &mut Commands,
    entity: Entity,
    rel_path: &str,
    registry: &mut ScriptRegistry,
) {
    let script_path = assets_dir().join(rel_path);
    let source = match std::fs::read_to_string(&script_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[usd-cosim] failed to read Python source `{}`: {e}", script_path.display());
            return;
        }
    };

    // Offset doc id away from any Modelica-allocated ids on the same
    // entity (legacy catalog Python balloon does the same).
    let doc_id = DocumentId::new(entity.index().index() as u64 + 10_000);
    registry.documents.insert(
        doc_id,
        DocumentHost::new(ScriptDocument {
            id: doc_id.raw(),
            generation: 0,
            language: ScriptLanguage::Python,
            source,
            inputs: vec!["height".to_string(), "velocity".to_string()],
            outputs: vec!["netForce".to_string()],
        }),
    );
    commands.entity(entity).insert(ScriptedModel {
        document_id: Some(doc_id.raw()),
        language: Some(ScriptLanguage::Python),
        paused: false,
        inputs: Default::default(),
        outputs: Default::default(),
    });

    // Python execution doesn't compile on a separate worker; the
    // SimComponent can be created right away (no need to wait for
    // variables to populate the way Modelica does).
    commands.entity(entity).insert(SimComponent {
        model_name: format!("Python:{}", rel_path),
        parameters: Default::default(),
        inputs: Default::default(),
        outputs: Default::default(),
        status: SimStatus::Running,
        is_stepping: false,
    });
}

/// On-Modelica-compile-complete: wraps the populated `ModelicaModel`
/// into a `SimComponent` so propagate/apply systems can pick it up.
/// Idempotent — only runs for USD-driven entities that don't already
/// have a `SimComponent` and whose Modelica variables have populated.
pub fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<
        (Entity, &ModelicaModel),
        (With<UsdSourcedCosim>, Without<SimComponent>),
    >,
) {
    for (entity, model) in q_new.iter() {
        if model.variables.is_empty() {
            continue;
        }
        commands.entity(entity).insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: if model.paused { SimStatus::Paused } else { SimStatus::Running },
            is_stepping: model.is_stepping,
        });
    }
}

/// Per-tick: ModelicaModel.variables → SimComponent.outputs.
/// Lets `propagate_connections` see fresh Modelica outputs each step.
pub fn sync_modelica_outputs(
    mut q: Query<(&ModelicaModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
        comp.status = if model.paused { SimStatus::Paused } else { SimStatus::Running };
    }
}

/// Per-tick: SimComponent.inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step.
pub fn sync_modelica_inputs(
    mut q: Query<(&SimComponent, &mut ModelicaModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}

/// Per-tick: ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q: Query<(&ScriptedModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.outputs {
            comp.outputs.insert(name.clone(), *val);
        }
    }
}

/// Per-tick: SimComponent.inputs → ScriptedModel.inputs.
pub fn sync_script_inputs(
    mut q: Query<(&SimComponent, &mut ScriptedModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.to_string(), *val);
        }
    }
}

/// Cross-entity wire translator. Reads typeless USD prims that carry
/// `rel lunco:wireFrom = </path>` + `rel lunco:wireTo = </path>` plus
/// `lunco:fromPort` / `lunco:toPort` / optional `lunco:scale`, resolves
/// the rel targets to ECS entities, and spawns one `SimConnection`.
///
/// Defers when an endpoint hasn't been spawned yet (asset loading is
/// async); reruns each frame until both sides exist or the wire is
/// dropped from the scene.
pub fn process_usd_cosim_wires(
    mut commands: Commands,
    q_unprocessed: Query<(Entity, &UsdPrimPath), Without<UsdSourcedWire>>,
    q_all: Query<(Entity, &UsdPrimPath)>,
    stages: Res<Assets<UsdStageAsset>>,
) {
    // Bail before building the path index in the common steady-state
    // case where every wire is already processed. The earlier version
    // walked `q_all` (every USD prim in the world) and allocated a
    // String per entity *every frame*, even when there was no work to
    // do — that turned a one-shot setup into a per-frame full-scene
    // scan.
    if q_unprocessed.is_empty() {
        return;
    }

    // Index every UsdPrimPath entity by its sdf path string. Wire prims
    // and their endpoints are typically all in the same stage, so this
    // is cheap relative to the work it saves.
    let mut by_path: HashMap<String, Entity> = HashMap::new();
    for (e, p) in q_all.iter() {
        by_path.insert(p.path.clone(), e);
    }

    for (entity, prim_path) in q_unprocessed.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };
        let reader = (*stage.reader).clone();

        let Some(from_path) = read_rel_target(&reader, &sdf_path, "lunco:wireFrom") else { continue; };
        let Some(to_path) = read_rel_target(&reader, &sdf_path, "lunco:wireTo") else { continue; };
        let from_port = reader.prim_attribute_value::<String>(&sdf_path, "lunco:fromPort");
        let to_port = reader.prim_attribute_value::<String>(&sdf_path, "lunco:toPort");

        let (Some(from_port), Some(to_port)) = (from_port, to_port) else {
            warn!(
                "[usd-cosim] {} declares wire rels but missing lunco:fromPort or lunco:toPort",
                prim_path.path
            );
            commands.entity(entity).insert(UsdSourcedWire);
            continue;
        };
        let scale = reader
            .prim_attribute_value::<f64>(&sdf_path, "lunco:scale")
            .unwrap_or(1.0);

        let from_str = from_path.to_string();
        let to_str = to_path.to_string();
        let (Some(&start_element), Some(&end_element)) =
            (by_path.get(&from_str), by_path.get(&to_str))
        else {
            // Endpoint(s) not spawned yet — try again next frame.
            continue;
        };

        commands.spawn(SimConnection {
            start_element,
            start_connector: from_port.clone(),
            end_element,
            end_connector: to_port.clone(),
            scale,
        });
        commands.entity(entity).insert(UsdSourcedWire);
        info!(
            "[usd-cosim] wire {}.{} → {}.{} (scale={})",
            from_str, from_port, to_str, to_port, scale,
        );
    }
}

/// Reads a single-target `rel <name> = </path>` from a USD prim. USD
/// stores rels under the `targetPaths` field (not `default`) as a
/// `PathListOp`. Returns the first contributing target.
fn read_rel_target(
    reader: &openusd::usda::TextReader,
    prim: &SdfPath,
    name: &str,
) -> Option<SdfPath> {
    let prop_path = prim.append_property(name).ok()?;
    let val = reader.get(&prop_path, "targetPaths").ok()?;
    match val.as_ref() {
        Value::PathListOp(list_op) => list_op.iter().next().cloned(),
        _ => None,
    }
}

/// Parses a `"from:to"` or `"from:to:scale"` wire entry. Empty ports are rejected.
fn parse_wire(raw: &str) -> Option<(String, String, f64)> {
    let mut parts = raw.split(':');
    let from = parts.next()?.trim();
    let to = parts.next()?.trim();
    if from.is_empty() || to.is_empty() { return None; }
    let scale = match parts.next() {
        Some(s) => s.trim().parse::<f64>().ok()?,
        None => 1.0,
    };
    Some((from.to_string(), to.to_string(), scale))
}

/// API query provider: `curl … {"command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. Lets you probe the running
/// binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str { "CosimStatus" }
    fn execute(
        &self,
        world: &mut World,
        _params: &serde_json::Value,
    ) -> lunco_api::ApiResponse {
        let mut q = world.query_filtered::<(
            &Name,
            &Transform,
            Option<&SimComponent>,
            Option<&ModelicaModel>,
            Option<&avian3d::prelude::LinearVelocity>,
        ), With<UsdSourcedCosim>>();

        let entities: Vec<serde_json::Value> = q.iter(world).map(|(name, tf, comp, model, lv)| {
            serde_json::json!({
                "name": name.as_str(),
                "y": tf.translation.y,
                "vy": lv.map(|v| v.0.y).unwrap_or(0.0),
                "has_simcomponent": comp.is_some(),
                "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
                "netForce": comp.and_then(|c| c.outputs.get("netForce").copied()),
                "force_y_input": comp.and_then(|c| c.inputs.get("force_y").copied()),
                "buoyancy": comp.and_then(|c| c.outputs.get("buoyancy").copied()),
            })
        }).collect();
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": entities }))
    }
}

/// Reload (or load) a USD scene at runtime via the API.
///
/// `curl … {"command":"LoadScene","params":{"path":"scenes/sandbox/sandbox_scene.usda"}}`
///
/// - `path`: USD asset path relative to the asset root.
/// - `root_prim`: optional override for the SDF path of the prim to
///   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
///   if absent, falls back to `/` (walk all top-level prims).
///
/// Despawns every existing entity carrying `UsdPrimPath` plus every
/// `SimConnection` (cosim wires are scene-derived in current code), then
/// reloads the asset from disk and spawns a fresh root entity. Existing
/// pipelines (`sync_usd_visuals`, `process_usd_cosim_prims`, the
/// avian/sim translators) take it from there. The first `Grid` entity
/// in the world is used as the parent — i.e. the `BigSpace` host
/// stays put across reloads.
///
/// Cleans up worker-side state too: sends `ModelicaCommand::Despawn`
/// for every entity carrying a `ModelicaModel` (the Modelica worker
/// drops its `steppers` / `cached_models` / `sim_streams` entries) and
/// drops `ScriptRegistry::documents` entries for every `ScriptedModel`.
/// Without this, repeated reloads accumulate stale steppers and parsed
/// scripts indefinitely.
#[Command(default)]
pub struct LoadScene {
    /// USD asset path (relative to `assets/`).
    pub path: String,
    /// Optional override for the prim to spawn. Empty (default) reads
    /// `defaultPrim` from the stage's metadata header, falling back to
    /// `/` when none is declared.
    pub root_prim: String,
}

#[on_command(LoadScene)]
fn on_load_scene(
    cmd: LoadScene,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath)>,
    q_wires: Query<Entity, With<SimConnection>>,
    q_modelica: Query<Entity, With<ModelicaModel>>,
    q_scripted: Query<&ScriptedModel>,
    channels: Res<ModelicaChannels>,
    mut script_registry: ResMut<lunco_scripting::ScriptRegistry>,
) {
    let path = cmd.path.clone();
    let root_prim = resolve_root_prim(&path, &cmd.root_prim);

    // Blender-style no-op: same path + root prim already loaded.
    let new_id = asset_server.load::<UsdStageAsset>(&path).id();
    if q_usd.iter().any(|(_, upp)| upp.stage_handle.id() == new_id && upp.path == root_prim) {
        info!("[load-scene] `{}` @ `{}` already loaded — no-op", path, root_prim);
        return;
    }

    info!("[load-scene] reload path=`{}` root=`{}`", path, root_prim);

    // Worker-side cleanup before despawn. Send Despawn for every
    // Modelica-bearing entity so the worker's `steppers` /
    // `cached_models` / `sim_streams` hashmaps don't leak.
    let mut modelica_freed = 0usize;
    for e in q_modelica.iter() {
        let _ = channels.tx.send(ModelicaCommand::Despawn { entity: e });
        modelica_freed += 1;
    }
    // Drop registered Python script documents for every ScriptedModel.
    let mut scripts_freed = 0usize;
    for sm in q_scripted.iter() {
        if let Some(raw_id) = sm.document_id {
            if script_registry.documents.remove(&DocumentId::new(raw_id)).is_some() {
                scripts_freed += 1;
            }
        }
    }

    let mut despawned = 0usize;
    for (e, _) in q_usd.iter() { commands.entity(e).try_despawn(); despawned += 1; }
    for e in q_wires.iter() { commands.entity(e).try_despawn(); despawned += 1; }
    info!(
        "[load-scene] cleanup: {} entities despawned, {} Modelica steppers freed, {} Python docs freed",
        despawned, modelica_freed, scripts_freed,
    );

    // Force fresh read from disk in case the user edited the file.
    asset_server.reload(&path);

    // Spawn via shared helper, deferred so despawns flush first.
    commands.queue(move |world: &mut World| {
        spawn_scene_root_world(world, &path, &root_prim);
    });
}

/// Spawn a USD scene root under the first `Grid` entity.
///
/// Shared by `LoadScene` (after its clear step) and `OpenFile` (additive
/// import). Blender-style no-op when the same `(asset, root_prim)` is
/// already mounted. Returns the spawned entity, or `None` on no-op /
/// missing `Grid`.
pub fn spawn_scene_root_world(
    world: &mut World,
    path_in: &str,
    root_prim_in: &str,
) -> Option<Entity> {
    // Normalize to asset-server-relative. The asset server prepends
    // its configured `file_path` (the `assets/` root) to every load
    // string, so absolute paths must have that prefix stripped.
    let pb = std::path::PathBuf::from(path_in);
    let asset_path: String = if pb.is_absolute() {
        let assets = assets_dir();
        match pb.strip_prefix(&assets) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => {
                warn!("[scene] `{}` is outside assets dir — cannot load", path_in);
                return None;
            }
        }
    } else {
        path_in.to_string()
    };
    let root_prim = resolve_root_prim(&asset_path, root_prim_in);
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(asset_path.clone());
    let new_id = handle.id();

    {
        let mut q = world.query::<&UsdPrimPath>();
        if q.iter(world).any(|upp| upp.stage_handle.id() == new_id && upp.path == root_prim) {
            info!("[scene] `{}` @ `{}` already loaded — no-op", asset_path, root_prim);
            return None;
        }
    }

    let grid = {
        let mut q = world.query_filtered::<Entity, With<Grid>>();
        q.iter(world).next()
    };
    let Some(grid) = grid else {
        warn!("[scene] no `Grid` entity — `{}` won't be parented", asset_path);
        return None;
    };

    let root = world.spawn((
        Name::new(format!("Scene:{}", asset_path)),
        UsdPrimPath { stage_handle: handle, path: root_prim.clone() },
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
        CellCoord::default(),
    )).id();
    world.entity_mut(grid).add_child(root);
    info!("[scene] spawned `{}` @ `{}` (entity {})", asset_path, root_prim, root);
    Some(root)
}

/// Resolve the SDF mount path for a scene load.
///
/// Priority:
/// 1. explicit `override_in` (non-empty caller-supplied path)
/// 2. stage's `defaultPrim` metadata
/// 3. `/` — mount the whole stage (matches `usdview` / Omniverse
///    behavior when a stage lacks `defaultPrim`).
///
/// Per USD spec, `defaultPrim` is only required for files that will be
/// *referenced* by other USD files (composition arcs need a target
/// prim). Opening a stage directly works fine without it. We warn at
/// load time so authors notice missing metadata before another file
/// tries to reference theirs.
pub fn resolve_root_prim(asset_path: &str, override_in: &str) -> String {
    if !override_in.is_empty() {
        return override_in.to_string();
    }
    let abs = assets_dir().join(asset_path);
    match read_default_prim(&abs) {
        Some(name) => {
            info!("[scene] `{}` defaultPrim = `{}`", asset_path, name);
            format!("/{}", name)
        }
        None => {
            warn!(
                "[scene] `{}` has no `defaultPrim` — mounting at `/`. \
                 Add `( defaultPrim = \"Name\" )` to the stage header if \
                 this file will be referenced from other USD files.",
                asset_path
            );
            "/".to_string()
        }
    }
}

/// Scan a `.usda` file's metadata header for `defaultPrim = "Name"`.
/// Returns the prim name (without leading slash) or `None` if the file
/// can't be read or no `defaultPrim` is declared.
///
/// The metadata block sits in the first few hundred bytes of every
/// stage, right after `#usda 1.0`, so we only read the file head.
fn read_default_prim(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let head = &raw[..raw.len().min(4096)];
    for line in head.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("defaultPrim") else { continue };
        // Accept `defaultPrim = "Name"` and `defaultPrim="Name"`.
        let rest = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
        let name = rest.trim_matches('"');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Plugin install hook — registers translator systems, per-tick sync
/// systems, and the API query provider. Called from `UsdSimPlugin::build`.
///
/// Per-tick ordering inside `FixedUpdate` matches the cosim master
/// algorithm:
///   `ModelicaSet::HandleResponses → sync_*_outputs →
///    PropagateCosimSet::Propagate → ApplyForcesCosimSet::ApplyForces →
///    sync_*_inputs → ModelicaSet::SpawnRequests`.
pub(crate) fn install(app: &mut App) {
    use lunco_cosim::systems::{apply_forces::CosimSet as ApplyForcesCosimSet, propagate::CosimSet as PropagateCosimSet};
    use lunco_modelica::ModelicaSet;

    app.add_systems(
        Update,
        (
            process_usd_cosim_prims,
            // Cross-entity wires must run after participant prims are
            // processed (so their entities are addressable in the
            // path → entity index built each call).
            process_usd_cosim_wires,
            wrap_modelica_into_simcomponent,
        ).chain().after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        FixedUpdate,
        (
            sync_modelica_outputs.after(ModelicaSet::HandleResponses).before(PropagateCosimSet::Propagate),
            sync_script_outputs.after(ModelicaSet::HandleResponses).before(PropagateCosimSet::Propagate),
            sync_modelica_inputs.after(ApplyForcesCosimSet::ApplyForces).before(ModelicaSet::SpawnRequests),
            sync_script_inputs.after(ApplyForcesCosimSet::ApplyForces).before(ModelicaSet::SpawnRequests),
        ),
    );

    app.add_systems(Startup, |mut reg: ResMut<lunco_api::ApiQueryRegistry>| {
        reg.register(CosimStatusProvider);
    });

    // Generated by `#[on_command(LoadScene)]` — registers the type and
    // installs the observer.
    __register_on_load_scene(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wire_default_scale() {
        assert_eq!(parse_wire("netForce:force_y"), Some(("netForce".into(), "force_y".into(), 1.0)));
    }

    #[test]
    fn parse_wire_with_scale() {
        assert_eq!(parse_wire("a:b:2.5"), Some(("a".into(), "b".into(), 2.5)));
    }

    #[test]
    fn parse_wire_rejects_empty() {
        assert_eq!(parse_wire(":b"), None);
        assert_eq!(parse_wire("a:"), None);
        assert_eq!(parse_wire(""), None);
    }

    // ── defaultPrim resolution ───────────────────────────────────────

    use std::io::Write;

    /// Write a `.usda` source into a unique tempfile and return its
    /// absolute path. Each call uses a counter so parallel tests don't
    /// collide on the same name.
    fn tmp_usda(source: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "lunco_usd_sim_default_prim_{}_{}.usda",
            std::process::id(),
            n
        ));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(source.as_bytes()).unwrap();
        p
    }

    #[test]
    fn read_default_prim_extracts_name() {
        let p = tmp_usda(
            "#usda 1.0\n(\n    defaultPrim = \"SandboxScene\"\n    upAxis = \"Y\"\n)\n\ndef Xform \"SandboxScene\" {}\n",
        );
        assert_eq!(read_default_prim(&p), Some("SandboxScene".to_string()));
    }

    #[test]
    fn read_default_prim_accepts_unspaced_equals() {
        let p = tmp_usda("#usda 1.0\n(\n    defaultPrim=\"Foo\"\n)\n");
        assert_eq!(read_default_prim(&p), Some("Foo".to_string()));
    }

    #[test]
    fn read_default_prim_returns_none_when_absent() {
        // Top-level prims declared but no metadata block.
        let p = tmp_usda("#usda 1.0\n\ndef Xform \"World\" {}\n");
        assert_eq!(read_default_prim(&p), None);
    }

    #[test]
    fn read_default_prim_returns_none_for_missing_file() {
        let p = std::env::temp_dir().join("lunco_usd_sim_no_such_file.usda");
        let _ = std::fs::remove_file(&p);
        assert_eq!(read_default_prim(&p), None);
    }

    // ── resolve_root_prim ────────────────────────────────────────────
    //
    // `resolve_root_prim` joins its `asset_path` arg with `assets_dir()`
    // (= relative `"assets"`). To exercise it hermetically we hand it
    // absolute temp paths — `PathBuf::join` of a base with an absolute
    // path returns the absolute path, so the join is a no-op.

    #[test]
    fn resolve_root_prim_override_wins() {
        let p = tmp_usda("#usda 1.0\n(\n    defaultPrim = \"Stage\"\n)\n");
        let p_str = p.to_string_lossy().into_owned();
        assert_eq!(resolve_root_prim(&p_str, "/Override"), "/Override");
    }

    #[test]
    fn resolve_root_prim_uses_default_prim_when_override_empty() {
        let p = tmp_usda("#usda 1.0\n(\n    defaultPrim = \"SandboxScene\"\n)\n");
        let p_str = p.to_string_lossy().into_owned();
        assert_eq!(resolve_root_prim(&p_str, ""), "/SandboxScene");
    }

    #[test]
    fn resolve_root_prim_falls_back_to_root_when_no_default_prim() {
        let p = tmp_usda("#usda 1.0\n\ndef Xform \"World\" {}\n");
        let p_str = p.to_string_lossy().into_owned();
        assert_eq!(resolve_root_prim(&p_str, ""), "/");
    }

    #[test]
    fn resolve_root_prim_falls_back_to_root_when_file_missing() {
        // Nonexistent path → no defaultPrim → "/" fallback.
        let p = std::env::temp_dir().join("lunco_usd_sim_resolve_missing.usda");
        let _ = std::fs::remove_file(&p);
        let p_str = p.to_string_lossy().into_owned();
        assert_eq!(resolve_root_prim(&p_str, ""), "/");
    }
}
