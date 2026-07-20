//! USD → cosim translator.
//!
//! Reads `lunco:modelicaModel` / `lunco:pythonModel` and native
//! `connectionPaths` from USD prims after `sync_usd_visuals` has spawned
//! the entity, and drives the full cosim lifecycle end-to-end:
//!
//! - **Modelica**: opens the source file, inserts a `ModelicaModel`
//!   stub, dispatches `ModelicaCommand::Compile` directly to the
//!   worker channel, and (once `model.variables` populates) wraps the
//!   result in a `SimComponent` so the propagate / apply-forces
//!   pipeline can read it.
//! - **Python**: opens the script, registers a `ScriptDocument`,
//!   attaches `ScriptedModel`, and creates the matching `SimComponent`.
//! - **Wiring**: [`rewire_usd_connections`] derives one `SimConnection`
//!   per authored `connectionPaths` source on a prim's `inputs:*`
//!   attributes — a consuming input `/B.inputs:force_y` connected to a
//!   producing output `/A.outputs:netForce` (self-loop when `A == B`).
//!   The derived set is a pure cache of USD, rebuilt on stage change.
//!
//! No domain-specific markers (`BalloonModelMarker`, …) are inserted
//! here. The legacy catalog/imperative spawn path in
//! `lunco-sandbox-edit` keeps using its own markers; this translator
//! is the authoritative path for USD-defined cosim entities.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_core::{on_command, register_commands, Command, OriginAnchor, WorldGrid};
use lunco_cosim::{SimComponent, SimConnection, SimStatus};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_modelica::source_asset::ModelicaSource;
use lunco_modelica::{
    extract_inputs_with_defaults_from_ast, extract_model_name_from_ast,
    extract_parameters_from_ast, ModelicaChannels, ModelicaCommand, ModelicaModel,
};
use lunco_scripting::source_asset::PythonSource;
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    ScriptRegistry,
};
use lunco_usd_bevy::{
    CanonicalStages, UsdAwaitingStage, UsdInstanceMember, UsdInstanceRoot,
    UsdPrimPath, UsdRead, UsdStageAsset,
};
use openusd::sdf::Path as SdfPath;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::UsdSimProcessed;

/// Marker indicating a USD-driven cosim entity has been wired up by
/// `process_usd_cosim_prims`. Prevents the system from re-processing
/// the same entity on the same tick.
#[derive(Component, Default)]
pub struct UsdSourcedCosim;

/// Single-flight guard for [`LoadScene`]: set the instant a scene load is
/// dispatched, cleared once `sync_usd_visuals` has drained every
/// `UsdAwaitingStage` prim for that scene's stage asset.
///
/// **Why.** Two independent triggers fire `LoadScene` on web startup —
/// the boot policy's `StartTutorial` (which `load_scene`s its own
/// environment) and the page's `autoloadDefaultScene` hook (which
/// `LoadScene`s the deploy default, e.g. moonbase). On a first run
/// both land in the same event-loop window. Without a guard, the
/// second `LoadScene`'s cleanup despawns the first scene's prims while
/// `sync_usd_visuals` still has deferred writes queued for them → the
/// "Entity despawned" panic that aborts wasm (the `try_insert` patch
/// above makes that a quiet no-op, but the deeper fix is to prevent the
/// second load from firing at all while the first is still spawning).
///
/// **Policy: first in-flight load wins.** A `LoadScene` arriving while
/// this guard holds a *different* path is suppressed (log + no-op). The
/// tutorial's `load_scene` runs during `Startup`, the page autoload
/// runs after the first frame paints — so the tutorial load is queued
/// first and the page autoload is the one suppressed. On a returning
/// run the boot policy stands down (no `StartTutorial`), no load is
/// in-flight by autoload time, and the moonbase autoload proceeds
/// normally. A later user-driven `LoadScene` (picking a different scene
/// in the browser) finds the guard cleared (the prior scene finished
/// spawning) and proceeds via the normal clear+respawn path.
///
/// The guard is keyed by stage `AssetId` (not path string) so the
/// clearing system can match it against `UsdPrimPath::stage_handle.id()`
/// on draining `UsdAwaitingStage` entities.
#[derive(Resource)]
pub struct SceneLoadInFlight {
    /// Asset-relative path of the in-flight scene (informational; logged
    /// on suppression so the console names the losing load).
    pub path: String,
    /// Stage asset id of the in-flight load. The clearing system watches
    /// for the last `UsdAwaitingStage` entity carrying this id to gain
    /// `UsdVisualSynced` (i.e. leave the awaiting pool).
    pub stage_id: bevy::asset::AssetId<UsdStageAsset>,
}

/// Queued Modelica source load. Inserted by `process_usd_cosim_prims`;
/// drained by `dispatch_loaded_modelica_sources` once the
/// `Handle<ModelicaSource>` has resolved to bytes.
#[derive(Component)]
pub struct PendingModelicaSource {
    pub handle: Handle<ModelicaSource>,
    /// Asset-relative path, copied for use as the eventual
    /// `ModelicaModel::model_path` (purely informational once the
    /// source is in memory).
    pub asset_path: String,
}

/// Same for Python.
#[derive(Component)]
pub struct PendingPythonSource {
    pub handle: Handle<PythonSource>,
    pub asset_path: String,
}

/// Reads cosim attributes from USD prims and dispatches model
/// compilation + wires. Runs in `Update` after `sync_usd_visuals` so
/// `Transform` / `Mesh3d` / `Material` are already present.
/// Run condition: any `UsdPrimPath` entity still lacks `UsdSourcedCosim`.
fn any_unprocessed_usd_cosim(q: Query<(), (With<UsdPrimPath>, Without<UsdSourcedCosim>)>) -> bool {
    !q.is_empty()
}

/// Run condition: any `UsdSourcedCosim` modelica model still needs wrapping
/// into a `SimComponent`.
fn any_unwrapped_modelica(
    q: Query<
        (),
        (
            With<UsdSourcedCosim>,
            With<ModelicaModel>,
            Without<SimComponent>,
        ),
    >,
) -> bool {
    !q.is_empty()
}

/// Clears [`SceneLoadInFlight`] once `sync_usd_visuals` has drained every
/// `UsdAwaitingStage` prim for the in-flight scene's stage — i.e. once the
/// scene's prims have all spawned (or failed to load). After this runs, a
/// later `LoadScene` (e.g. the user picking a different scene in the
/// browser) proceeds via the normal clear+respawn path instead of being
/// suppressed. Runs every `Update` but is a single `is_empty` query when no
/// guard is set.
fn clear_scene_load_in_flight(
    in_flight: Option<Res<SceneLoadInFlight>>,
    q_awaiting: Query<&UsdPrimPath, With<UsdAwaitingStage>>,
    mut commands: Commands,
) {
    let Some(g) = in_flight else { return };
    // Still spawning if any prim tagged for this stage hasn't been
    // processed by `sync_usd_visuals` (i.e. still carries
    // `UsdAwaitingStage`).
    let still_awaiting = q_awaiting
        .iter()
        .any(|upp| upp.stage_handle.id() == g.stage_id);
    if !still_awaiting {
        commands.remove_resource::<SceneLoadInFlight>();
    }
}

pub fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdSourcedCosim>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset's recipe.
    mut canonical: NonSendMut<CanonicalStages>,
    asset_server: Res<AssetServer>,
) {
    for (entity, prim_path) in query.iter() {
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            continue;
        };

        // Acquire a read source: the live canonical stage, built on demand from
        // the asset recipe. If it is not available yet the asset is still
        // loading — retry next frame WITHOUT marking, so the prim stays in the
        // `Without<UsdSourcedCosim>` query.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }

        // Mark examined up front so each prim is inspected exactly once.
        // Without this, every *non-cosim* prim (wheels, ground, ramps — the
        // bulk of the scene) failed the active-cosim gate below via the
        // early `continue` WITHOUT ever gaining `UsdSourcedCosim`, so it stayed
        // in the `Without<UsdSourcedCosim>` query forever — and this system
        // re-ran every frame, deep-cloning the whole stage per prim. That was
        // the dominant sandbox CPU cost (see scripts/perf/README.md).
        // Safe: every other `UsdSourcedCosim` consumer also requires a
        // `ModelicaModel` / `SimComponent` / `ScriptedModel` that a non-cosim
        // prim never gains, so marking it here matches nothing downstream.
        // No live stage (asset carries no recipe / build failed) yet — skip,
        // leaving the prim in the `Without<UsdSourcedCosim>` query to retry.
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        // `try_insert` (not `.insert`): a `LoadScene` cleanup may despawn this
        // prim between this system's iterate and ApplyDeferred — the canonical
        // race is the moonbase autoload vs a first-run tutorial on web. `.insert`
        // routes through Bevy's panic error handler, which aborts wasm; `try_insert`
        // silently drops the write on a despawned entity. Every entity-tied insert
        // queued by this pipeline uses the same despawn-safe form for the same
        // reason. See `lunco_usd_bevy::sync_usd_visuals` for the policy.
        commands.entity(entity).try_insert(UsdSourcedCosim);
        process_usd_cosim_prim_read(
            &cs.view(),
            entity,
            prim_path,
            &sdf_path,
            &mut commands,
            &asset_server,
        );
    }
}

/// Reads one cosim prim's attributes and dispatches its model + wires + events,
/// generic over the read source ([`UsdRead`]) — drives off either the live
/// canonical `StageView` or the flattened `sdf::Data`, identically.
fn process_usd_cosim_prim_read(
    reader: &lunco_usd_bevy::StageView<'_>,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: &SdfPath,
    commands: &mut Commands,
    asset_server: &AssetServer,
) {
    // Active-cosim gate: a prim is stepped iff it BOTH binds a behavior model
    // AND declares connectable ports (`inputs:`/`outputs:` attributes). The two
    // non-active cases skip silently: a model with no ports is a
    // documentation-only reference (wheels/motors/batteries carry
    // `lunco:modelicaModel` for provenance); ports with no model are a pure
    // physics sink driven through its backend (a joint receiving
    // `inputs:angle`, a rigid body receiving `inputs:force_y`). Wiring itself
    // is native `connectionPaths`, derived by `rewire_usd_connections`
    // (the journaled, distributed path), never parsed here.
    // A program names its source as an `asset`. The LANGUAGE comes from the file's
    // extension, never from a second attribute: the same `.py` is a plant on one
    // prim and a script on the next, so a `lunco:pythonModel`-style name would be
    // asserting a role the file does not have. This is how USD itself dispatches
    // `.usda` / `.usdc` / `.usdz`.
    let source = reader.asset(sdf_path, "info:sourceAsset");
    let (modelica_path, python_path) = match source.as_deref().map(solver_language) {
        Some(Some(SolverLanguage::Modelica)) => (source.clone(), None),
        Some(Some(SolverLanguage::Python)) => (None, source.clone()),
        // A program this crate does not solve (a `.rhai` script, a `.xml` tree).
        // It is somebody else's to run; it is not a cosim model.
        Some(None) => return,
        None => return,
    };
    let has_ports = reader
        .attr_names(sdf_path)
        .iter()
        .any(|n| n.starts_with("inputs:") || n.starts_with("outputs:"));
    if !has_ports {
        return;
    }

    // `UsdSourcedCosim` already inserted above; add the cosim-only markers.
    //
    // NB: this stamps `UsdSimProcessed`, which makes `process_usd_sim_prims` skip this
    // prim — fine, because link/celestial projection is now its OWN system
    // (`project_celestial_comms_prims`), gated by its OWN marker, so a cosim antenna
    // still gets its `LinkNode`. The two concerns no longer race on one flag.
    commands
        .entity(entity)
        .try_insert((UsdSimProcessed, lunco_core::SelectableRoot));

    // NOTE: there is no possessable/vessel tag to stamp — possession is not gated by
    // a marker at all (an avatar may possess anything; WHO may hold it is the
    // authority layer's call). A prim's command CAPABILITY comes from its `Controls`
    // scope → `ControlBinding` + `CommandInputs`, stamped in the general USD
    // translator (`lunco-usd-bevy`), which runs for every prim — not here, which only
    // sees model-bound cosim prims. A lander's actuation backend is its
    // `SimComponent` manual-override ports (written by `SetPorts`), read
    // by topology at possess/route time.

    // Opaque-body guard, applied HERE (cosim intent is known the instant we
    // read `lunco:modelicaModel`/`lunco:pythonModel`) rather than only later
    // in `tag_cosim_opaque`, which waits for the asynchronously-wrapped
    // `SimComponent`. That async gap was a prediction-takeover race: on a
    // client, `maintain_predicted_dynamic` (sandbox-edit) could stamp a balloon
    // `PredictedDynamic` during the multi-frame window before `NotPredictable`
    // landed — once b99991dd dropped the `SkipContentStamp` structural guard,
    // `NotPredictable` became the SOLE membership guard, so a late stamp meant
    // the body got predicted (local physics + cosim forces) and diverged.
    // Stamping at prim-read time closes the window. No vessel-kind exception:
    // a body reaching here has connectable ports + a model, so its motion is
    // cosim-driven by definition (a locally-driven rover chassis never gains
    // a `SimComponent` — under the sub-prim-per-model convention its Modelica
    // subsystems live on child prims, not the moving body). Harmless on
    // non-`RigidBody` cosim prims (e.g. a joint-driven solar tracker): the
    // marker is inert where prediction never runs.
    commands.entity(entity).try_insert(lunco_core::NotPredictable);

    // Source files are loaded through Bevy's `AssetServer`: on native it reads
    // from the workspace `assets/` source, on wasm it issues an HTTP fetch
    // against the same path. Either way the actual Compile dispatch
    // happens later, in `dispatch_loaded_modelica_sources` /
    // `dispatch_loaded_python_sources`, once the asset is ready.
    // See `docs/architecture/40-asset-io.md`.
    if let Some(asset_path) = modelica_path {
        commands.entity(entity).try_insert(PendingModelicaSource {
            handle: asset_server.load(asset_path.clone()),
            asset_path,
        });
    }
    if let Some(asset_path) = python_path {
        commands.entity(entity).try_insert(PendingPythonSource {
            handle: asset_server.load(asset_path.clone()),
            asset_path,
        });
    }

    // The realtime promise — `lunco:program:realtimeSafe = true`. DECLARED, never
    // inferred: no amount of reading a model's source establishes how long it takes
    // to step. Absent ⇒ not promised, and `rewire_usd_connections` refuses it a
    // force/torque port on a client-predicted body (see
    // `docs/architecture/28-modelica-realtime-physics.md`).
    if reader
        .scalar::<bool>(sdf_path, "lunco:program:realtimeSafe")
        .unwrap_or(false)
    {
        commands.entity(entity).try_insert(lunco_cosim::RealtimeSafe);
    }

    // Event rules are `LunCoPortEvent` CHILD prims — one prim per rule, each with a
    // port, a comparison, a threshold and a name. Each turns a threshold crossing on
    // a model output into a discrete TelemetryEvent (see `fire_model_port_events`).
    let rules = read_port_event_prims(reader, sdf_path);
    if !rules.is_empty() {
        commands.entity(entity).try_insert(ModelEventRules(rules));
    }

    info!(
        "[usd-cosim] program {} bound ({})",
        prim_path.path,
        source.as_deref().unwrap_or("<none>"),
    );
}

/// The languages this crate can put a solver behind. Everything else is a program
/// somebody else runs — the rhai engine, the behaviour-tree compiler — and this
/// crate leaves it alone.
enum SolverLanguage {
    Modelica,
    Python,
}

/// Which solver, if any, runs a program — decided by its file's extension, exactly
/// as USD picks a file-format plugin by `.usda` / `.usdc` / `.usdz`. `None` is not
/// an error: it is a program with a different engine behind it.
fn solver_language(path: &str) -> Option<SolverLanguage> {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("mo") => Some(SolverLanguage::Modelica),
        Some("py") => Some(SolverLanguage::Python),
        _ => None,
    }
}

/// Read a program's [`LunCoPortEvent`] children into threshold rules.
///
/// One prim per rule, so each part of it — the port, the comparison, the threshold,
/// the event name — is a typed property that validates, inspects, journals and
/// replicates like any other. A rule packed into a string is none of those things.
fn read_port_event_prims(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
) -> Vec<PortEventRule> {
    let mut rules = Vec::new();
    for child in reader.children(sdf_path) {
        if reader.type_name(&child).as_deref() != Some("LunCoPortEvent") {
            continue;
        }
        let (Some(port), Some(emit)) = (
            reader.text(&child, "lunco:event:port"),
            reader.text(&child, "lunco:event:emit"),
        ) else {
            warn!(
                "[usd-cosim] {}: a LunCoPortEvent needs both `lunco:event:port` and \
                 `lunco:event:emit` — ignoring it",
                child.as_str(),
            );
            continue;
        };
        let op = match reader
            .text(&child, "lunco:event:op")
            .unwrap_or_else(|| "lt".to_string())
            .as_str()
        {
            "lt" => EdgeOp::Lt,
            "le" => EdgeOp::Le,
            "gt" => EdgeOp::Gt,
            "ge" => EdgeOp::Ge,
            other => {
                warn!(
                    "[usd-cosim] {}: `lunco:event:op = \"{}\"` is not one of lt/le/gt/ge — \
                     ignoring this rule",
                    child.as_str(),
                    other,
                );
                continue;
            }
        };
        rules.push(PortEventRule {
            port,
            op,
            threshold: reader.real(&child, "lunco:event:threshold").unwrap_or(0.0),
            event: emit,
            armed: true,
        });
    }
    rules
}

/// Drain `PendingModelicaSource` for entities whose `.mo` text has
/// finished loading via `AssetServer`. Parses the source, populates a
/// `ModelicaModel` stub, dispatches `ModelicaCommand::Compile`, and
/// removes the pending marker. Stable retry behaviour: if the asset
/// isn't ready this frame we just skip — the system runs again next
/// frame.
pub fn dispatch_loaded_modelica_sources(
    mut commands: Commands,
    q: Query<(Entity, &PendingModelicaSource)>,
    sources: Res<Assets<ModelicaSource>>,
    asset_server: Res<AssetServer>,
    channels: Option<Res<ModelicaChannels>>,
) {
    let Some(channels) = channels else { return };
    for (entity, pending) in q.iter() {
        // Bail loud if the asset failed to load — without this the
        // entity stays Pending forever and the user sees nothing.
        if asset_server.load_state(&pending.handle).is_failed() {
            warn!(
                "[usd-cosim] failed to load Modelica source `{}` via AssetServer",
                pending.asset_path
            );
            commands.entity(entity).remove::<PendingModelicaSource>();
            continue;
        }
        let Some(src) = sources.get(&pending.handle) else {
            continue;
        };

        // Single best-effort parse, three AST-driven extracts. Lenient
        // parsing means a model with a semantic error still produces
        // usable name/parameter/input snapshots — same recovery
        // semantics `Session::recovered_file_query` uses on the engine
        // side.
        let ast = rumoca_phase_parse::parse_to_syntax(&src.text, "cosim-dispatch.mo")
            .best_effort()
            .clone();
        let model_name = extract_model_name_from_ast(&ast).unwrap_or_else(|| "Model".into());
        let parameters = extract_parameters_from_ast(&ast);
        let inputs = extract_inputs_with_defaults_from_ast(&ast)
            .into_iter()
            .collect();

        commands.entity(entity).try_insert(ModelicaModel {
            model_path: PathBuf::from(&pending.asset_path),
            model_name: model_name.clone(),
            parameters,
            inputs,
            // USD-cosim models are part of the live scene (balloon
            // buoyancy, the solar tracker) — they should simulate as soon
            // as they compile, not land paused. The doc/UI Run path doesn't
            // reach them (they have no DocumentId), so without this they
            // would stay frozen forever. The worker's compile-success
            // handler sets `paused = !resume_after_compile`.
            resume_after_compile: true,
            ..default()
        });

        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id: 0,
            model_name,
            source: src.text.clone(),
            // Stable per-asset session URI (its asset path) — keeps this
            // model's overlay distinct in the worker session and consistent
            // across recompiles. See `ModelicaCommand::Compile::doc_uri`.
            doc_uri: pending.asset_path.to_string(),
            extra_sources: Vec::new(),
            stream: None,
        });

        commands.entity(entity).remove::<PendingModelicaSource>();
    }
}

/// Drain `PendingPythonSource` analogously to the Modelica version.
pub fn dispatch_loaded_python_sources(
    mut commands: Commands,
    q: Query<(Entity, &PendingPythonSource)>,
    sources: Res<Assets<PythonSource>>,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<ScriptRegistry>,
) {
    for (entity, pending) in q.iter() {
        if asset_server.load_state(&pending.handle).is_failed() {
            warn!(
                "[usd-cosim] failed to load Python source `{}` via AssetServer",
                pending.asset_path
            );
            commands.entity(entity).remove::<PendingPythonSource>();
            continue;
        }
        let Some(src) = sources.get(&pending.handle) else {
            continue;
        };

        // Offset doc id away from any Modelica-allocated ids on the same
        // entity (legacy catalog Python balloon does the same).
        let doc_id = DocumentId::new(entity.index().index() as u64 + 10_000);
        // Route through the registry funnel so a journal recorder attaches (edits
        // to this cosim script record like any other domain).
        registry.insert_document(
            doc_id,
            ScriptDocument {
                id: doc_id.raw(),
                generation: 0,
                language: ScriptLanguage::Python,
                source: src.text.clone(),
                origin: DocumentOrigin::untitled(format!("Python-{}", doc_id.raw())),
                inputs: vec!["height".to_string(), "velocity".to_string()],
                outputs: vec!["netForce".to_string()],
                params: String::new(),
                // No asset id: this source is SYNTHESIZED from a USD prim's inline
                // script, so it has no location for a relative `import` to anchor
                // against. `None` is the honest answer — an invented id would let a
                // relative import silently resolve against some unrelated root.
                asset_id: None,
                // Untitled, synthesized from a USD prim's inline source — never
                // on disk, so it is genuinely unsaved.
                last_saved_generation: None,
            },
        );
        commands.entity(entity).try_insert(ScriptedModel {
            document_id: Some(doc_id.raw()),
            language: Some(ScriptLanguage::Python),
            paused: false,
            inputs: Default::default(),
            outputs: Default::default(),
        });

        // Python execution doesn't compile on a separate worker; the
        // SimComponent can be created right away (no need to wait for
        // variables to populate the way Modelica does).
        commands.entity(entity).try_insert(SimComponent {
            model_name: format!("Python:{}", pending.asset_path),
            parameters: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            status: SimStatus::Running,
            is_stepping: false,
        });

        commands.entity(entity).remove::<PendingPythonSource>();
    }
}

/// On-Modelica-compile-complete: wraps the populated `ModelicaModel`
/// into a `SimComponent` so propagate/apply systems can pick it up.
/// Idempotent — only runs for USD-driven entities that don't already
/// have a `SimComponent` and whose Modelica variables have populated.
pub fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<(Entity, &ModelicaModel), (With<UsdSourcedCosim>, Without<SimComponent>)>,
) {
    for (entity, model) in q_new.iter() {
        if model.variables.is_empty() {
            continue;
        }
        commands.entity(entity).try_insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: if model.paused {
                SimStatus::Paused
            } else {
                SimStatus::Running
            },
            is_stepping: model.is_stepping,
        });
    }
}

/// Copy `f64` port values into a destination map, allocating a `String`
/// key only on the *first* tick a port appears. The cosim sync systems
/// below run every `FixedUpdate`; the keys (`"height"`, `"netForce"`, …)
/// are stable, so after the first step every port already exists and
/// this updates in place with zero allocation. The old
/// `dst.insert(name.clone(), v)` re-allocated every key every tick.
#[inline]
fn upsert_ports<'a>(
    dst: &mut HashMap<String, f64>,
    src: impl Iterator<Item = (&'a String, &'a f64)>,
) {
    for (name, val) in src {
        match dst.get_mut(name) {
            Some(slot) => *slot = *val,
            None => {
                dst.insert(name.clone(), *val);
            }
        }
    }
}

/// Per-tick: ModelicaModel.variables → SimComponent.outputs.
/// Lets `propagate_connections` see fresh Modelica outputs each step.
pub fn sync_modelica_outputs(
    mut q: Query<(&ModelicaModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        upsert_ports(&mut comp.outputs, model.variables.iter());
        comp.status = if model.paused {
            SimStatus::Paused
        } else {
            SimStatus::Running
        };
    }
}

/// Per-tick: SimComponent.inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step.
pub fn sync_modelica_inputs(
    mut q: Query<(&SimComponent, &mut ModelicaModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        upsert_ports(&mut model.inputs, comp.inputs.iter());
    }
}

/// Per-tick: ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q: Query<(&ScriptedModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        upsert_ports(&mut comp.outputs, model.outputs.iter());
    }
}

/// Per-tick: SimComponent.inputs → ScriptedModel.inputs.
pub fn sync_script_inputs(
    mut q: Query<(&SimComponent, &mut ScriptedModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        upsert_ports(&mut model.inputs, comp.inputs.iter());
    }
}

// ── Modelica `when` / port-edge → event bridge ───────────────────────────────
//
// Modelica HAS discrete events (`when`, zero-crossings), but our rumoca codegen
// surfaces only continuous variable VALUES as ports, and conditions on wired
// INPUTS read defaults — so putting the event condition INSIDE the model is
// unreliable. Instead the model stays a continuous SIGNAL emitter, and the edge
// detection happens HERE in trusted Rust: a declarative `lunco:portEvents` rule
// turns a threshold crossing of a model OUTPUT into a discrete `TelemetryEvent`
// on the shared bus — the same bus rhai reads via `on_event` / `wait_for`.

#[derive(Clone, Copy)]
enum EdgeOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl EdgeOp {
    fn holds(self, v: f64, thr: f64) -> bool {
        match self {
            EdgeOp::Lt => v < thr,
            EdgeOp::Le => v <= thr,
            EdgeOp::Gt => v > thr,
            EdgeOp::Ge => v >= thr,
        }
    }
}

/// One declarative port-edge rule: emit `event` when output `port` `op`-crosses
/// `threshold`. Re-triggerable — `armed` clears on fire and re-arms when the
/// condition is false again, so it fires once per rising edge.
#[derive(Clone)]
struct PortEventRule {
    port: String,
    op: EdgeOp,
    threshold: f64,
    event: String,
    armed: bool,
}

/// Port-edge event rules on a program, read from its [`LunCoPortEvent`] children.
#[derive(Component, Default)]
pub struct ModelEventRules(Vec<PortEventRule>);

/// Per-tick: evaluate each entity's port-edge rules against its fresh
/// `SimComponent.outputs` and fire a `TelemetryEvent` (source = the model
/// entity) on each rising edge. This is the "Modelica events" bridge — a
/// continuous model signal in, a discrete event out.
pub fn fire_model_port_events(
    mut q: Query<(
        &mut ModelEventRules,
        &SimComponent,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    mut commands: Commands,
) {
    for (mut rules, comp, gid) in &mut q {
        let src = gid.map(|g| g.get()).unwrap_or(0);
        for rule in rules.0.iter_mut() {
            let Some(&v) = comp.outputs.get(&rule.port) else {
                continue;
            };
            let cond = rule.op.holds(v, rule.threshold);
            if cond && rule.armed {
                rule.armed = false;
                commands.trigger(lunco_core::TelemetryEvent {
                    name: rule.event.clone(),
                    source: src,
                    severity: lunco_core::Severity::Info,
                    data: lunco_core::TelemetryValue::F64(v),
                    timestamp: 0.0,
                });
            } else if !cond {
                rule.armed = true;
            }
        }
    }
}

/// Marker on a [`SimConnection`] that was **derived** from USD `connectionPaths`
/// (as opposed to authored some other way). [`rewire_usd_connections`] despawns
/// every tagged edge and rebuilds the set from the composed stage, which is what
/// makes `SimConnection` a **pure derived cache** of USD wiring.
#[derive(Component, Default)]
pub struct UsdWiredConnection;

/// Set when a drained live edit — a journaled (hence distributed)
/// `connectionPaths` change on an **already-spawned** prim — requires the wiring
/// to be re-derived. Structural changes (prim spawn/despawn) need no flag; they
/// are detected directly via change-detection in [`rewire_usd_connections`].
#[derive(Resource, Default)]
pub struct WiringDirty(pub bool);

/// Derive the co-sim wiring from native USD `connectionPaths` — the USD-native,
/// journaled, distributed replacement for the deleted `lunco:simWires` / wire-prim
/// producers. `SimConnection`s are a **pure derived cache**: whenever the wiring
/// topology may have changed, the whole derived set is rebuilt from the composed
/// stage. A full rebuild (not a per-prim patch) is what makes the lifecycle
/// correct — an edge exists exactly when *both* its endpoints do, regardless of
/// the order they spawn or which end is removed.
///
/// Trigger (dormant otherwise — steady state is zero work):
/// - **structural** — any `UsdPrimPath` entity added or removed. Covers initial
///   scene load (the reconcile spawns prims → this fires), async payload/vessel
///   spawn, source-after-sink ordering (the late source's spawn re-runs this and
///   completes the deferred edge), and prim removal (the rebuild omits any edge
///   whose endpoint is gone — no dangling `SimConnection`).
/// - **live edit** — [`WiringDirty`], set by the op-driven projection
///   ([`lunco_usd::live_consume`]) when a `connectionPaths` change is drained
///   from the live stage (an edit that is not itself a prim spawn/despawn).
///
/// A connection whose source prim is not yet spawned is skipped (its later spawn
/// re-runs this); a malformed source path is logged and skipped — restoring the
/// diagnostic the deleted `process_usd_cosim_wire_read` emitted.
pub fn rewire_usd_connections(
    mut commands: Commands,
    added: Query<(), Added<UsdPrimPath>>,
    mut removed: RemovedComponents<UsdPrimPath>,
    mut dirty: ResMut<WiringDirty>,
    q_all: Query<(Entity, &UsdPrimPath)>,
    q_edges: Query<Entity, With<UsdWiredConnection>>,
    // Wire endpoints resolve by IDENTITY, not raw prim path. Two runtime spawns of
    // the same asset compose byte-IDENTICAL stage-relative paths (`/DescentLander`,
    // …), so a flat path→entity map collapses them onto one entity — a lander's
    // force self-loop would then bind to the OTHER lander's model and both bodies
    // move as one. A prim's *instance* is named by its instance-root `GlobalEntityId`:
    // `Provenance::Derived{parent}` for a descendant, the root's own GID for the
    // instance root. That id is unique per spawn, identical on every peer, and
    // STABLE across a program/script hot-swap (it is `derive_id(parent, role)`, a
    // pure function of identity, not of the ephemeral `Entity`) — so a wire re-
    // resolves to the same endpoints after a dynamic script change.
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    // Identity is minted a frame after the prim spawns (`assign_global_entity_ids`,
    // PostUpdate). Re-run once the ids land so the instance-scoped resolution below
    // sees them — otherwise the first, pre-identity pass would fall back to the
    // scene namespace and briefly cross-wire the spawns.
    id_assigned: Query<(), Added<lunco_core::GlobalEntityId>>,
    // The realtime gate: whether the SOURCE program promised it is realtime-safe,
    // and whether the SINK is a client-predicted dynamic body (a `RigidBody` NOT
    // opted out of prediction). Absence of the promise is the dangerous case;
    // absence of the body is the safe one.
    q_realtime_safe: Query<&lunco_cosim::RealtimeSafe>,
    q_predicted_body: Query<
        &avian3d::prelude::RigidBody,
        Without<lunco_core::NotPredictable>,
    >,
    q_defaults: Query<&UsdInputDefaults>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
) {
    let structural =
        !added.is_empty() || !id_assigned.is_empty() || removed.read().next().is_some();
    if !structural && !dirty.0 {
        return;
    }
    dirty.0 = false;

    // A prim's instance identity (its instance-root GID, `None` for scene prims)
    // is what keeps two spawns of one asset — byte-identical stage-relative paths
    // and all — from collapsing onto one entity below. See `instance_key`.
    let instance_of =
        |e: Entity| lunco_usd_bevy::instance_key(e, &q_provenance, &q_gid, &q_instance_root);

    // Index every prim entity by (instance, path). Keying on the instance is what
    // keeps two spawns of one asset distinct: their identical stage-relative paths
    // now land under different instance keys instead of overwriting each other.
    let mut by_path: HashMap<(Option<u64>, String), Entity> = HashMap::new();
    for (e, p) in q_all.iter() {
        by_path.insert((instance_of(e), p.path.clone()), e);
    }

    // Authored constants on unconnected `inputs:` ports — a model's parameters.
    // Gathered in the same sweep that derives the wires, because "has no wire" is
    // exactly what makes an input a parameter.
    let mut defaults: HashMap<Entity, HashMap<String, f64>> = HashMap::new();

    // Rebuild: drop every derived edge, then re-derive from the composed stage.
    for e in q_edges.iter() {
        commands.entity(e).try_despawn();
    }

    for (entity, prim_path) in q_all.iter() {
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        let view = cs.view();
        let Ok(sink_sdf) = SdfPath::new(&prim_path.path) else {
            continue;
        };

        // Resolve this prim's wires within its OWN instance — a source path names a
        // prim of the same spawn, never a same-named prim of a different one.
        let sink_instance = instance_of(entity);

        for attr in view.attr_names(&sink_sdf) {
            // Only `inputs:` attributes are connection sinks; connector = the leaf.
            let Some(sink_conn) = attr.strip_prefix("inputs:") else {
                continue;
            };
            // SSP `LinearTransformation`: the propagated value is `src * factor +
            // offset`. Authored on the sink prim, keyed by the consuming port
            // (`lunco:factor:<port>` / `:offset:<port>`), so each input carries its
            // own scaling. Absent ⇒ identity (1, 0), matching the pre-migration
            // `lunco:scale` default. The transform is invariant across the fan-in
            // sources, so it is read once per sink port, above the source loop.
            // Tolerant of `float` or `double` authoring — a wire naturally matches
            // the `float`-typed port it scales, so a strict `double` read would
            // silently drop the transform.
            let scale = view
                .real(&sink_sdf, &format!("lunco:factor:{sink_conn}"))
                .unwrap_or(1.0);
            let offset = view
                .real(&sink_sdf, &format!("lunco:offset:{sink_conn}"))
                .unwrap_or(0.0);

            // A PARAMETER IS AN INPUT WITH A CONSTANT INSTEAD OF A CONNECTION.
            // An `inputs:` port with no wire into it is authored data — `float
            // inputs:kv = 1.2` — and it is the ONLY way USD reaches a model's
            // parameters. Collected here (the one pass that already enumerates
            // every `inputs:` port with the composed reader in hand) and applied
            // by `seed_usd_input_defaults` once the model exists.
            let sources = view.connections(&sink_sdf, &attr);
            if sources.is_empty() {
                if let Some(v) = view.real(&sink_sdf, &attr) {
                    defaults
                        .entry(entity)
                        .or_default()
                        .insert(sink_conn.to_string(), v);
                }
                continue;
            }

            for src in sources {
                // Split `/A.outputs:netForce` → prim `/A`, leaf `outputs:netForce`.
                let Some((src_prim, src_leaf)) = src.rsplit_once('.') else {
                    warn!(
                        "[usd-cosim] {}.{}: malformed connection source '{}' (no `.<connector>`)",
                        prim_path.path, attr, src
                    );
                    continue;
                };
                let Some(&start_element) = by_path.get(&(sink_instance, src_prim.to_string()))
                else {
                    // Two very different situations, and they must not look alike.
                    // A prim that EXISTS on the stage but has no entity yet is
                    // mid-spawn: its later spawn is a structural change that re-runs
                    // this and completes the edge. A prim that is not on the stage at
                    // all is a typo'd or stale target that will never resolve, and a
                    // silently dropped wire is how a vehicle ends up with no forces
                    // and no explanation.
                    if let Ok(src_sdf) = SdfPath::new(src_prim) {
                        if !view.has_prim(&src_sdf) {
                            warn!(
                                "[usd-cosim] {}.{}: connection source '{}' names a prim that does \
                                 not exist on this stage — the wire is dropped. Check the path.",
                                prim_path.path, attr, src_prim
                            );
                        }
                    }
                    continue;
                };
                let src_conn = src_leaf
                    .strip_prefix("outputs:")
                    .or_else(|| src_leaf.strip_prefix("inputs:"))
                    .unwrap_or(src_leaf);

                // ── The realtime gate ───────────────────────────────────────
                // A program may only push a client-predicted `Dynamic` body around
                // if it PROMISED it steps fast enough
                // (`lunco:program:realtimeSafe = true`). Without that promise — the
                // common case, since the default is `false` — an adaptive,
                // variable-cost solver is deciding the forces inside the prediction
                // loop, and the body diverges from the server every frame the solver
                // runs late.
                //
                // Warn, don't refuse: cosim prims are stamped `NotPredictable` at
                // prim-read time, so a scene that trips this gate has ALREADY
                // routed around the guard some other way, and dropping the wire
                // silently would leave a vehicle with no forces at all. The warn
                // names the attribute and the prim so it is actionable.
                if lunco_cosim::is_physics_force_port(sink_conn)
                    && matches!(
                        q_predicted_body.get(entity),
                        Ok(avian3d::prelude::RigidBody::Dynamic)
                    )
                    && q_realtime_safe.get(start_element).is_err()
                {
                    warn!(
                        "[usd-cosim] {}.{}: the program at {} drives a force/torque port on a \
                         CLIENT-PREDICTED dynamic body without declaring \
                         `lunco:program:realtimeSafe = true`. Its step sequence and cost are not \
                         guaranteed identical across peers — the predicted body can diverge. \
                         Declare it on the program prim (see \
                         docs/architecture/28-modelica-realtime-physics.md).",
                        prim_path.path, attr, src_prim,
                    );
                }

                commands.spawn((
                    SimConnection {
                        start_element,
                        start_connector: src_conn.to_string(),
                        end_element: entity,
                        end_connector: sink_conn.to_string(),
                        scale,
                        offset,
                    },
                    UsdWiredConnection,
                ));
            }
        }
    }

    // Publish the authored parameters — but ONLY where they changed. This runs on
    // every structural change (any prim spawning anywhere re-runs the whole pass),
    // and `seed_usd_input_defaults` reacts to `Changed`. Re-inserting an identical
    // map would fire `Changed` anyway and re-seed the model, clobbering a value a
    // script had since written through `SetPorts` — an autopilot's `engage` would
    // silently snap back to its authored default the next time anything spawned.
    for (entity, map) in defaults {
        if q_defaults.get(entity).map(|d| d.0 != map).unwrap_or(true) {
            commands.entity(entity).try_insert(UsdInputDefaults(map));
        }
    }
}

/// The authored constants on a prim's unconnected `inputs:` ports — a model's
/// parameters, as USD stated them.
///
/// Kept as its own component rather than written straight into `SimComponent`
/// because the two arrive in either order: the wiring pass reads USD the frame the
/// prim spawns, while the `SimComponent` only exists once the model has been
/// fetched, compiled, and wrapped, which is several frames later on native and an
/// HTTP round-trip later on the web.
#[derive(Component, Debug, Clone, Default)]
pub struct UsdInputDefaults(pub HashMap<String, f64>);

/// Seed a model's inputs from the constants USD authored on its unconnected ports.
///
/// This is the ONLY path from USD to a model's parameters. Runs when the model
/// appears (`Added<SimComponent>`) and when the authored values change (a live edit
/// re-runs the wiring pass, which re-publishes [`UsdInputDefaults`]) — never on a
/// plain re-derive, so a value written by a script or the network is not undone.
///
/// A key the model does not declare is dropped by the port backend, so a typo'd
/// parameter is not a silent no-op: it is named here.
pub fn seed_usd_input_defaults(
    mut q: Query<
        (&UsdInputDefaults, &mut SimComponent, &UsdPrimPath),
        Or<(Added<SimComponent>, Changed<UsdInputDefaults>)>,
    >,
) {
    for (defaults, mut sim, prim_path) in q.iter_mut() {
        for (port, value) in &defaults.0 {
            if sim.inputs.contains_key(port) {
                sim.inputs.insert(port.clone(), *value);
            } else {
                warn!(
                    "[usd-cosim] {}: `inputs:{}` is authored but the model ({}) declares no such \
                     input — the value is ignored. Check the port name against the model.",
                    prim_path.path, port, sim.model_name,
                );
            }
        }
    }
}

// ── Uniform port commands (ListPorts / GetPort / SetPort) ───────────────────
//
// The single API surface over the cosim **port table** (`lunco_cosim::ports`).
// Every exposed value — Modelica var, Avian force/state, joint angle, env
// signal — is read/written/listed here uniformly, regardless of which backend
// owns it. These are the canonical port verbs; they are not aliases of
// `CosimStatus` (which stays as richer per-entity cosim introspection).

/// Map a [`lunco_cosim::PortDirection`] to a stable wire string.
fn port_dir_str(d: lunco_cosim::PortDirection) -> &'static str {
    match d {
        lunco_cosim::PortDirection::In => "in",
        lunco_cosim::PortDirection::Out => "out",
        lunco_cosim::PortDirection::InOut => "inout",
    }
}

fn port_to_json(p: &lunco_core::ports::PortRef) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "direction": port_dir_str(p.direction),
        "value": p.value,
    })
}

/// Resolve the optional `api_id` / `entity` field of a params object to an ECS
/// `Entity` via the `ApiEntityRegistry`. Returns `None` when absent (the
/// caller lists all) or when the id doesn't resolve.
fn resolve_param_entity(world: &mut World, params: &serde_json::Value) -> Option<Entity> {
    let raw = params
        .get("api_id")
        .or_else(|| params.get("entity"))
        .and_then(|v| v.as_u64())?;
    let reg = world.get_resource::<lunco_api::ApiEntityRegistry>()?;
    reg.resolve(&lunco_core::GlobalEntityId::from_raw(raw))
}

/// `ListPorts` — enumerate exposed ports. With `{"api_id": N}`, lists that
/// entity's ports; without, lists every registered entity that has any port.
///
/// `curl … {"command":"ListPorts","params":{"api_id":12345}}`
pub struct ListPortsProvider;

impl lunco_api::ApiQueryProvider for ListPortsProvider {
    fn name(&self) -> &'static str {
        "ListPorts"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        // Single-entity form.
        if let Some(e) = resolve_param_entity(world, params) {
            let ports: Vec<_> = ports_reg
                .entity_ports(world, e)
                .iter()
                .map(port_to_json)
                .collect();
            return lunco_api::ApiResponse::ok(serde_json::json!({ "ports": ports }));
        }
        // All-entities form: snapshot the registry list first (owned), then
        // read ports — avoids holding the resource borrow across `entity_ports`.
        let Some(reg) = world.get_resource::<lunco_api::ApiEntityRegistry>() else {
            return lunco_api::ApiResponse::ok(serde_json::json!({ "entities": [] }));
        };
        let entries = reg.entities();
        let mut rows = Vec::new();
        for (api_id, e) in entries {
            let ports = ports_reg.entity_ports(world, e);
            if ports.is_empty() {
                continue;
            }
            rows.push(serde_json::json!({
                "api_id": api_id.get(),
                "name": world.get::<Name>(e).map(|n| n.as_str().to_string()).unwrap_or_default(),
                "ports": ports.iter().map(port_to_json).collect::<Vec<_>>(),
            }));
        }
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": rows }))
    }
}

/// `GetPort` — read one port value.
///
/// `curl … {"command":"GetPort","params":{"api_id":N,"name":"yaw"}}`
pub struct GetPortProvider;

impl lunco_api::ApiQueryProvider for GetPortProvider {
    fn name(&self) -> &'static str {
        "GetPort"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "GetPort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "GetPort requires a `name`",
            );
        };
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        match ports_reg.read_port(world, e, name) {
            Some(value) => {
                lunco_api::ApiResponse::ok(serde_json::json!({ "name": name, "value": value }))
            }
            None => lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                format!("no port `{}` on entity", name),
            ),
        }
    }
}

/// `SetPort` — write a setpoint to one input port.
///
/// `curl … {"command":"SetPort","params":{"api_id":N,"name":"angle","value":1.2}}`
///
/// TODO(ports): this writes the input slot once via [`lunco_core::ports::PortRegistry::write_port`];
/// per decision 2 it must become a ControlStream **hold** (latest-wins,
/// `hold_last(timeout)`, overriding a live wire until released).
pub struct SetPortProvider;

impl lunco_api::ApiQueryProvider for SetPortProvider {
    fn name(&self) -> &'static str {
        "SetPort"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "SetPort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "SetPort requires a `name`",
            );
        };
        let Some(value) = params.get("value").and_then(|v| v.as_f64()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "SetPort requires a numeric `value`",
            );
        };
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        if ports_reg.write_port(world, e, name, value) {
            lunco_api::ApiResponse::ok(serde_json::json!({ "name": name, "value": value }))
        } else {
            lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                format!("no writable input port `{}` on entity", name),
            )
        }
    }
}

/// API query provider: `curl … {"command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. Lets you probe the running
/// binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str {
        "CosimStatus"
    }
    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let mut q = world.query_filtered::<(
            &Name,
            &Transform,
            Option<&SimComponent>,
            Option<&ModelicaModel>,
            Option<&avian3d::prelude::LinearVelocity>,
        ), With<UsdSourcedCosim>>();

        let entities: Vec<serde_json::Value> = q
            .iter(world)
            .map(|(name, tf, comp, model, lv)| {
                // Full input/output maps so any cosim signal is readable
                // (the solar tracker's `yaw`/`tracking_error`, the balloon's
                // `buoyancy`, …) — not just a hardcoded set. This is the
                // general "read cosim world state" surface.
                let outputs = comp
                    .map(|c| {
                        c.outputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                let inputs = comp
                    .map(|c| {
                        c.inputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "name": name.as_str(),
                    "y": tf.translation.y,
                    "yaw": tf.rotation.to_euler(EulerRot::YXZ).0,
                    "vy": lv.map(|v| v.0.y).unwrap_or(0.0),
                    "has_simcomponent": comp.is_some(),
                    "model": comp.map(|c| c.model_name.clone()).unwrap_or_default(),
                    "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                    "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                    "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
                    "outputs": outputs,
                    "inputs": inputs,
                })
            })
            .collect();
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

// The `LoadScene` OBSERVER lives in `lunco-usd`
// (`commands.rs::on_load_scene`), not here: mounting a scene has to resolve the
// requested path to its DOCUMENT first (a doc-backed scene must mount its
// composed `base ⊕ runtime`, never the base file), and the document registry
// lives one layer up. This crate owns the mount MECHANICS the observer drives —
// [`normalize_scene_asset_path`], [`resolve_root_prim`], [`clear_scene_entities`],
// [`spawn_scene_root_world`], [`SceneLoadInFlight`] — as its public mount API.

/// Reload the CURRENTLY-ACTIVE scene from disk — the "restart" verb.
///
/// [`LoadScene`] deliberately no-ops when asked to load the scene that is already
/// active (same path + root), so it cannot pick up on-disk edits to the LIVE
/// scene. `RestartScene` always clears the current scene's entities, force-reloads
/// its stage asset from disk (busting the asset cache), and respawns a single
/// fresh root — so editing a `.usda` then `restart_scene()` shows the change with
/// no duplicate instances. Takes no args: it targets whatever scene is loaded.
/// Paired with `pause()` this is the "reload-then-freeze" one-liner the workflow
/// wanted (`restart_scene(); pause();`).
#[Command(default)]
pub struct RestartScene {}

#[on_command(RestartScene)]
fn on_restart_scene(
    trigger: On<RestartScene>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath)>,
    scene: SceneEntities,
) {
    // Every loaded prim shares the scene's stage handle. REUSE that handle (not a
    // freshly-resolved path) so the exact same asset — INCLUDING its source scheme
    // (`twin://…`, `lunco://…`) — is respawned. Resolving via `.path()` would
    // drop the scheme and load a *different* raw-file asset, breaking twin routing
    // (avatar/camera setup, composed runtime edits) and leaving a stale camera.
    let Some((_, upp)) = q_usd.iter().next() else {
        warn!("[restart-scene] no scene is loaded — nothing to restart");
        return;
    };
    let handle = upp.stage_handle.clone();
    // Full asset path WITH source scheme (owned, so `reload` doesn't need a
    // `'static` borrow), for the reload key + the scene-root label. `None` only
    // for a document-backed stage with no registered path — still respawnable
    // from the handle, just unlabelled.
    let asset_path = asset_server.get_path(handle.id()).map(|p| p.into_owned());
    let label = asset_path
        .as_ref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "restarted-scene".to_string());
    info!("[restart-scene] reloading `{}` from disk", label);

    // Despawn the old scene + free worker-side state (shared with `ClearScene`).
    // Every scene-authored entity (incl. the Avatar camera) carries `UsdPrimPath`,
    // so `try_despawn` (hierarchy-recursive) tears the old camera down here — no
    // stale window camera survives into the fresh scene.
    clear_scene_entities(&mut commands, &scene);

    // Force a fresh disk read so on-disk edits actually apply (the whole point).
    // Reloading by the full path (scheme intact) targets the SAME asset id the
    // handle holds, so it fires exactly one fresh `LoadedWithDependencies` → a
    // single re-instantiation pass, no duplicate scene or camera.
    if let Some(ap) = asset_path {
        asset_server.reload(ap);
    }

    // Respawn from the SAME handle (defaultPrim resolution), deferred so the
    // despawns flush first.
    commands.queue(move |world: &mut World| {
        spawn_scene_root_with_stage(world, &label, "", handle);
    });
}

/// Clear the active scene — despawn every USD prim entity + cosim wire
/// and free the worker-side Modelica steppers / Python script docs they
/// referenced, leaving an empty viewport.
///
/// Fired when a Twin / folder opens with nothing to show — no
/// `[usd] default_scene`, or a plain folder with no USD content — so the
/// viewport reflects the newly opened folder instead of keeping the
/// previously loaded scene. (`LoadScene` does this same clear *before*
/// loading its new scene.) Also useful standalone over the API / MCP as
/// a "clear the world" verb.
#[Command(default)]
pub struct ClearScene {}

#[on_command(ClearScene)]
fn on_clear_scene(
    trigger: On<ClearScene>,
    mut commands: Commands,
    scene: SceneEntities,
) {
    info!("[clear-scene] clearing viewport");
    clear_scene_entities(&mut commands, &scene);
}

/// Despawn the current scene's USD entities + cosim wires.
/// External/worker state (such as Modelica steppers or Python script documents)
/// is cleaned up automatically via reactive `On<Remove, T>` component observers
/// registered in their respective home crates (`lunco-modelica` and `lunco-scripting`).
/// Shared by [`LoadScene`] (clear-before-reload) and [`ClearScene`]
/// (clear-to-empty). Despawns are deferred through `commands`.
///
/// TODO(avian-bump): this plain batch despawn trips a DEBUG-only assert in avian
/// 0.7 — `island.contact_count == 0` (islands/mod.rs:1372), from
/// `BodyIslandNode::on_remove` when an island's last body leaves while a contact is
/// still registered against it. It is currently silenced by
/// `[profile.dev.package.avian3d] debug-assertions = false` (see the workspace
/// Cargo.toml for the full rationale) — a MASK, not a fix. Verified benign: the
/// island is deleted on the next line, and physics simulates correctly after a
/// reload (rover stays finite and rests on terrain).
///
/// DO NOT "fix" this by reordering the teardown. Every sanctioned order was tried
/// and ALL still panic: remove `RigidBody` then `Collider`; `Collider` then
/// `RigidBody`; `RigidBody` alone; insert `ColliderDisabled` + `RigidBodyDisabled`;
/// gather colliders via `RigidBodyColliders` rather than the Bevy hierarchy; and
/// even stepping `PhysicsSchedule` mid-teardown. Each left islands holding contacts.
/// Root cause is upstream: a collider's contacts drain ONLY on adding
/// `ColliderDisabled`/`Disabled` or REMOVING `ColliderMarker` — and since
/// `ColliderMarker` is a REQUIRED component, dropping `Collider` drains nothing
/// while still unlinking it from `RigidBodyColliders` (which defeats the body's own
/// drain); and `remove_collider_on` early-returns on a non-TOUCHING edge without
/// unlinking it from the island. Re-test on the next avian bump.
///
/// NOTE: any system that touches scene entities through `Commands` must use the
/// FALLIBLE forms (`try_despawn`/`try_remove`/`try_insert`) — its queries are built
/// before this despawn flushes, so its targets can already be dead. A plain
/// `remove`/`insert` panics in `apply_deferred` and takes the app down mid-reload
/// (that was the `sync_gizmo_camera` crash).
/// The scene-owned entities a teardown touches, bundled as one `SystemParam`.
///
/// Every scene-lifecycle observer — `LoadScene` (in `lunco-usd`), `ClearScene`,
/// `RestartScene` — needs exactly this set. Bundling keeps the mount API honest:
/// a caller drives a teardown without naming `WorldGrid`, `OriginAnchor` or the
/// cosim `SimConnection` wire type, so `lunco-usd` needs no dependency on
/// `lunco-cosim` to orchestrate a scene swap.
#[derive(bevy::ecs::system::SystemParam)]
pub struct SceneEntities<'w, 's> {
    grid: Query<'w, 's, &'static Children, With<WorldGrid>>,
    origin: Query<'w, 's, Entity, With<OriginAnchor>>,
    wires: Query<'w, 's, Entity, With<SimConnection>>,
}

pub fn clear_scene_entities(commands: &mut Commands, scene: &SceneEntities) {
    let (q_grid, q_origin, q_wires) = (&scene.grid, &scene.origin, &scene.wires);
    let mut despawned = 0usize;

    // Despawn all children of the WorldGrid (recursively), except the persistent OriginAnchor
    if let Ok(children) = q_grid.single() {
        for child in children.iter() {
            if !q_origin.contains(child) {
                commands.entity(child).try_despawn();
                despawned += 1;
            }
        }
    }

    // The despawn above takes the scene camera with it, and that camera is usually
    // what holds `FloatingOrigin` (`process_usd_sim_prims` strips it off the anchor
    // when a USD Avatar prim claims it). Hand it back to the anchor in THIS flush.
    // Leaving the gap for `anchor_owns_origin_by_default` to close in PostUpdate is
    // what logged "BigSpace … has no floating origins" on every scene change: the
    // guard is a backstop, not the handover. `try_insert` is a no-op if the anchor
    // already holds it (the origin never left home for this scene).
    if let Ok(anchor) = q_origin.single() {
        commands.entity(anchor).try_insert(big_space::prelude::FloatingOrigin);
    }

    // Despawn any root-level derived connection wires (which are spawned as root entities)
    for e in q_wires.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }
    info!(
        "[scene] cleanup: {} entities despawned",
        despawned
    );
    commands.trigger(lunco_core::RestoreFallbackLights);
    // Every scene clear resets the whole clock tree to defaults (doc 19 §11b): a sky
    // left detached at 100 000×, a scrubbed animation, a paused transport — none of it
    // may survive into the next scene. This is the single choke point all three reload
    // paths funnel through, so the reset lives here, not at each call site.
    commands.trigger(lunco_time::ResetTime {});
}

/// Despawn a single USD prim **subtree** (one runtime prim and its descendants).
///
/// Under Bevy 0.19's relationship system, despawning the root entity recursively
/// despawns all descendants. Component removal triggers (`On<Remove, T>`) fire automatically,
/// freeing any worker-side state (such as Modelica steppers or Python script documents)
/// via the reactive observers registered in `lunco-modelica` and `lunco-scripting`.
pub fn despawn_usd_subtree(world: &mut World, root: Entity) {
    if let Ok(em) = world.get_entity_mut(root) {
        em.despawn();
        info!("[scene] incremental despawn: entity {:?}", root);
    }
}

/// Spawn one new USD child prim into a live scene, mirroring the child branch of
/// [`lunco_usd_bevy::instantiate_usd_prim`] — the per-prim analogue of a full
/// scene-root mount, used by E2 incremental spawn ([`lunco_usd::live_consume`])
/// when a `Resync` reports a prim added to the composed document.
///
/// `stage_handle_id` scopes the lookup to one scene; `reader` is the *fresh*
/// composed stage (the asset store's current reader, so the `on_usd_prim_added`
/// observer that fires on the new `UsdPrimPath` sees the prim). The parent live
/// entity is found by composed path; the child is spawned with the same atomic
/// `(UsdPrimPath, ChildOf, transform, instance-membership)` bundle the loader
/// uses, so the observer instantiates its geometry + subtree in place without
/// disturbing siblings. Returns `None` (no-op) if the parent isn't live yet or
/// the prim is already spawned.
pub fn spawn_usd_child(
    world: &mut World,
    stage_handle_id: bevy::asset::AssetId<UsdStageAsset>,
    reader: &lunco_usd_bevy::StageView<'_>,
    path: &str,
) -> Option<Entity> {
    // Pre-populate the translate so physics sees the spawn offset before the
    // observer refines the full transform (matches the loader's child branch).
    let sdf_path = SdfPath::new(path).ok()?;
    let tf = lunco_usd_bevy::get_attribute_as_vec3(reader, &sdf_path, "xformOp:translate")
        .map(Transform::from_translation)
        .unwrap_or_default();
    spawn_usd_child_with_translate(world, stage_handle_id, path, tf)
}

/// Reader-free core of [`spawn_usd_child`]: spawn the stub child entity for
/// `path` under its already-live parent, with a pre-read transform `tf`,
/// inheriting grid-anchoring + instance membership from the parent. The
/// `on_usd_prim_added` observer then builds the subtree from the canonical
/// stage.
///
/// Split out so the live-stage projection bridge can pre-read the translate
/// under a *short* immutable borrow of the `!Send` `CanonicalStage` and then
/// spawn here with `&mut World` — the stage itself can't be held across the
/// spawn (it aliases the world), but the observer that fires on insert reads it
/// fresh from `CanonicalStages`.
pub fn spawn_usd_child_with_translate(
    world: &mut World,
    stage_handle_id: bevy::asset::AssetId<UsdStageAsset>,
    path: &str,
    tf: Transform,
) -> Option<Entity> {
    // Parent path = `path` minus its final `/segment`.
    let (parent_prefix, _name) = path.rsplit_once('/')?;
    let parent_path = if parent_prefix.is_empty() {
        "/"
    } else {
        parent_prefix
    };

    // Resolve the live parent entity (same scene) and bail if it isn't
    // instantiated yet — a following full load / reconcile will cover it.
    let parent_entity = {
        let mut q = world.query::<(Entity, &UsdPrimPath)>();
        q.iter(world)
            .find(|(_, upp)| upp.stage_handle.id() == stage_handle_id && upp.path == parent_path)
            .map(|(e, _)| e)
    }?;
    // Idempotent: never double-spawn a path that already has a live entity.
    let already = {
        let mut q = world.query::<&UsdPrimPath>();
        q.iter(world)
            .any(|upp| upp.stage_handle.id() == stage_handle_id && upp.path == path)
    };
    if already {
        return None;
    }

    let stage_handle = world
        .get::<UsdPrimPath>(parent_entity)?
        .stage_handle
        .clone();

    // Inherit grid-anchoring + instance membership from the parent exactly as
    // `instantiate_usd_prim` derives them for its children.
    let parent_member = world.get::<UsdInstanceMember>(parent_entity).cloned();
    let parent_is_root = world.get::<UsdInstanceRoot>(parent_entity).is_some();
    let member = parent_member.or_else(|| {
        parent_is_root.then(|| UsdInstanceMember {
            root: parent_entity,
            root_path: parent_path.to_string(),
        })
    });

    let base = (
        Name::new(path.to_string()),
        UsdPrimPath {
            stage_handle,
            path: path.to_string(),
        },
        tf,
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    );
    // Plain child of its USD parent, per the anchoring contract: the scene root
    // is the one grid anchor and everything under it inherits that frame. A prim
    // carrying its own `CellCoord` under the grid fights avian's writeback and
    // freezes its render (see `instantiate_usd_prim` / `SpawnAnchor`).
    let entity = match member {
        Some(m) => world.spawn((base, ChildOf(parent_entity), m)).id(),
        None => world.spawn((base, ChildOf(parent_entity))).id(),
    };
    info!("[scene] incremental spawn: `{}` (entity {})", path, entity);
    Some(entity)
}

/// Normalize a scene path to asset-server-relative form. Accepts an
/// absolute path under the workspace `assets/` dir (Twin manifests store
/// scenes as twin-root-relative; the caller joins them to an absolute
/// path) or an already-relative asset path. Returns `None` (with a warn)
/// if an absolute path lies outside the assets dir.
pub fn normalize_scene_asset_path(path_in: &str) -> Option<String> {
    // Already a scheme path (`abs://`, `lunco://`, …) — the AssetServer routes
    // it to the named source as-is.
    if lunco_assets::has_scheme(path_in) {
        return Some(path_in.to_string());
    }
    let pb = std::path::PathBuf::from(path_in);
    if pb.is_absolute() {
        // Under the project `assets/` dir → asset-relative (default source).
        // `lunco-assets` owns that mapping; this only decides what `LoadScene`
        // does when it does NOT apply.
        match lunco_assets::library_rel(&pb) {
            Some(rel) => Some(rel),
            None => {
                // `LoadScene` takes SCHEME-QUALIFIED addresses (`lunco://`,
                // `twin://`) — it loads an already-addressable asset and has no
                // access to the workspace/Twin layer, so it cannot resolve a
                // bare filesystem path to a root or mount it doc-first.
                //
                // `OpenFile` is the entry point that owns that step: it resolves
                // the scene's root, registers it, and mounts through the document
                // overlay. Routing a raw path here instead would mount a
                // base-only stage and silently drop runtime edits.
                warn!(
                    "[scene] `{}` is a bare filesystem path — `LoadScene` takes \
                     scheme addresses (`lunco://…`, `twin://…`). Use `OpenFile` \
                     to open a scene by path; it resolves the owning root.",
                    path_in
                );
                None
            }
        }
    } else {
        Some(path_in.to_string())
    }
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
    let Some(asset_path) = normalize_scene_asset_path(path_in) else {
        return None;
    };
    // File-backed source: the AssetServer reads + composes the on-disk
    // stage. lunco-usd's E1 projection takes the other door
    // ([`spawn_scene_root_with_stage`]) to mount a document's *composed*
    // (base ⊕ runtime) stage instead.
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(asset_path.clone());
    spawn_scene_root_with_stage(world, &asset_path, root_prim_in, handle)
}

/// The mounted scene root — the entity a scene's whole prim subtree hangs from.
///
/// It is the only entity that knows **both** halves of "where does a scene-level
/// edit go?": its [`UsdPrimPath::stage_handle`] resolves to the editable document
/// (via `lunco_usd::twin_projection::scene_document_for`), and its
/// [`UsdPrimPath::path`] is the *mounted root prim* — `/SandboxScene`, `/World`,
/// `/HdriTest`, whatever this scene's `defaultPrim` happens to be.
///
/// Before this marker existed, a command that wanted to author a new top-level
/// prim had to guess at both (count the document registry; hardcode `/World`) —
/// and a hardcoded `/World` authors under a parent that does not exist in a scene
/// rooted at `/SandboxScene`, so the prim composes into the layer and is then
/// never mounted. The scene root is the answer to both questions; ask it.
///
/// The preview viewport (`lunco_usd::ui::viewport`) mounts its own private root
/// the same way, so consumers that must act on the *running* scene should scope
/// their query rather than assume a single one exists.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneRoot;

/// Spawn a USD scene root from an **already-built** stage handle.
///
/// The handle-supplying sibling of [`spawn_scene_root_world`]: instead of
/// loading the stage from disk via the `AssetServer`, the caller hands in a
/// `Handle<UsdStageAsset>` it built itself. This is the seam E1 uses — lunco-usd
/// passes a handle holding a [`UsdDocument`](../../lunco_usd/document)'s
/// *composed* (`base ⊕ runtime`) stage, so the live world projects the editable
/// document (with its persisted runtime spawns/moves) rather than the raw file.
///
/// `label` names the root (`Scene:{label}`) and feeds `defaultPrim` resolution;
/// `root_prim_in` empty defers the mount path to the stage's `defaultPrim`
/// (see [`resolve_root_prim`]). Blender-style no-op when the same
/// `(handle, root_prim)` is already mounted. Returns the spawned entity, or
/// `None` on no-op.
pub fn spawn_scene_root_with_stage(
    world: &mut World,
    label: &str,
    root_prim_in: &str,
    handle: Handle<UsdStageAsset>,
) -> Option<Entity> {
    let asset_path = label.to_string();
    let root_prim = resolve_root_prim(&asset_path, root_prim_in);
    let new_id = handle.id();

    {
        let mut q = world.query::<&UsdPrimPath>();
        if q.iter(world)
            .any(|upp| upp.stage_handle.id() == new_id && upp.path == root_prim)
        {
            info!(
                "[scene] `{}` @ `{}` already loaded — no-op",
                asset_path, root_prim
            );
            return None;
        }
    }

    // Mount under the canonical world grid. `ensure_world_root` is create-or-get:
    // it builds the persistent shell (root + WorldGrid + single FloatingOrigin) on
    // the first scene load and returns the same grid on every reload — so the root
    // is never duplicated and never absent. Replaces the old "first `Grid` found"
    // heuristic, which was ambiguous once celestial / preview grids also existed.
    let grid = lunco_core::ensure_world_root(world);

    // Scene-root entity is itself the Grid-direct `GridAnchor`. Its
    // children — top-level USD prims (rovers, balls, terrain) — stay
    // as plain Bevy children, inheriting GlobalTransform from this
    // anchor via Bevy's normal transform propagation (handled by
    // big_space's `propagate_low_precision`). This restores the working
    // hierarchy where avian rigid bodies on rover roots compute
    // `Position` relative to the scene-root anchor instead of needing
    // their own CellCoord, which conflicted with avian's writeback.
    // Atomic spawn: `ChildOf(grid)` in the bundle so parent + CellCoord +
    // Transform land together — same contract as `migrate_to_grid`. Avoids
    // the observer race that mis-tagged rover chassis as `RigidBody::Static`.
    let root = world
        .spawn((
            Name::new(format!("Scene:{}", asset_path)),
            UsdSceneRoot,
            UsdPrimPath {
                stage_handle: handle,
                path: root_prim.clone(),
            },
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            CellCoord::default(),
            lunco_core::GridAnchor,
            ChildOf(grid),
        ))
        .id();
    info!(
        "[scene] spawned `{}` @ `{}` (entity {})",
        asset_path, root_prim, root
    );
    Some(root)
}

/// Resolve the SDF mount path for a scene load.
///
/// Priority:
/// 1. explicit `override_in` (non-empty caller-supplied path) wins.
/// 2. otherwise return the empty *deferred-resolution sentinel* — the
///    scene-root entity is spawned with an empty path, and
///    `lunco_usd_bevy::instantiate_usd_prim` resolves it from the
///    stage's `defaultPrim` metadata once the asset has parsed
///    (falling back to `/` — whole-stage mount — when none is declared).
///
/// The defaultPrim lookup is deliberately deferred rather than read
/// here: this runs synchronously at command time, before the stage
/// asset finishes loading. It is resolved from the parsed `TextReader`
/// at instantiate time instead — correct on both native and web, and
/// yielding the defaultPrim subtree rather than a whole-stage `/` mount.
///
/// Per USD spec, `defaultPrim` is only required for files that will be
/// *referenced* by other USD files (composition arcs need a target
/// prim). Opening a stage directly works fine without it.
pub fn resolve_root_prim(_asset_path: &str, override_in: &str) -> String {
    if !override_in.is_empty() {
        return override_in.to_string();
    }
    // Deferred sentinel — resolved against the parsed stage downstream.
    String::new()
}

/// Plugin install hook — registers translator systems, per-tick sync
/// systems, and the API query provider. Called from `UsdSimPlugin::build`.
///
/// Opaque-body guard (prediction-membership design in git history): stamp
/// [`lunco_core::NotPredictable`] on every cosim-driven physics body — one with a
/// [`SimComponent`] (its motion comes from Modelica/script forces the client does
/// not run) AND a [`RigidBody`]. This is the cosim **takeover** site: the same
/// `SimComponent`-attachment that makes a body server-driven also marks it
/// unpredictable, so the client's prediction systems (`maintain_predicted_dynamic`,
/// and any future contact-island promotion) refuse to ever predict it and keep it
/// on the interpolated proxy path. No vessel-kind exception: a `SimComponent` on
/// a `RigidBody` means the body's motion IS the cosim solver's output, which the
/// client can't reproduce. A locally-driven rover chassis never carries a
/// `SimComponent` (its Modelica subsystems live on child prims under the
/// sub-prim-per-model convention), so it is naturally excluded by topology.
/// Runs on both peers (cheap, idempotent — `Without<NotPredictable>` makes it a
/// one-shot per body); harmless where prediction never runs.
fn tag_cosim_opaque(
    mut commands: Commands,
    q: Query<
        Entity,
        (
            With<SimComponent>,
            With<avian3d::prelude::RigidBody>,
            Without<lunco_core::NotPredictable>,
        ),
    >,
) {
    for e in q.iter() {
        commands.entity(e).try_insert(lunco_core::NotPredictable);
    }
}

/// Per-tick ordering inside `FixedUpdate` matches the cosim master
/// algorithm:
///   `ModelicaSet::HandleResponses → sync_*_outputs →
///    PropagateCosimSet::Propagate → ApplyForcesCosimSet::ApplyForces →
///    sync_*_inputs → ModelicaSet::SpawnRequests`.
pub(crate) fn install(app: &mut App) {
    use lunco_cosim::systems::{
        apply_forces::CosimSet as ApplyForcesCosimSet, propagate::CosimSet as PropagateCosimSet,
    };
    use lunco_modelica::ModelicaSet;

    // Ensure the source asset types this module's systems read/allocate are
    // registered. Idempotent — production registers these via the Modelica /
    // scripting plugins; doing it here lets minimal apps (headless tests using
    // `MinimalPlugins` without those plugins) run the cosim systems without
    // panicking on a missing `Assets<…>` resource.
    app.init_asset::<ModelicaSource>()
        .init_asset::<PythonSource>()
        .init_resource::<lunco_scripting::ScriptRegistry>()
        .init_resource::<WiringDirty>();

    app.add_systems(
        Update,
        // Drain the single-flight guard the frame after the last prim of
        // the in-flight scene leaves the awaiting pool. Cheap (one
        // `Option<Res>` + a bounded `Query::iter` only when a guard is
        // set); no per-frame cost in steady state.
        clear_scene_load_in_flight.after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        Update,
        (
            // Gated on `any unprocessed cosim prim`: stay dormant
            // after scene-load is complete. Same archetype-check
            // pattern used for `process_usd_sim_prims`.
            process_usd_cosim_prims.run_if(any_unprocessed_usd_cosim),
            // Source-load drain runs every Update; cheap when no
            // `PendingModelicaSource` / `PendingPythonSource` entities
            // exist. Splitting it from `process_usd_cosim_prims` is
            // intentional — the source asset may take multiple frames
            // to load (network on wasm, async I/O on native).
            dispatch_loaded_modelica_sources,
            dispatch_loaded_python_sources,
            // Wiring is derived from native `connectionPaths`: rebuilds the
            // `SimConnection` set whenever prims spawn/despawn (structural) or a
            // `connectionPaths` edit is drained (`WiringDirty`); dormant otherwise.
            rewire_usd_connections,
            wrap_modelica_into_simcomponent.run_if(any_unwrapped_modelica),
            // Parameters: the authored constants the wiring pass gathered off the
            // unconnected `inputs:` ports, pushed into the model once it exists.
            // After the wrap, because it needs the `SimComponent` to write into.
            seed_usd_input_defaults,
            // §6 opaque guard: once a body is cosim-driven, mark it unpredictable
            // (after the SimComponent wrap above, so it sees freshly-wrapped bodies).
            tag_cosim_opaque,
        )
            .chain()
            .after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        FixedUpdate,
        (
            sync_modelica_outputs
                .after(ModelicaSet::HandleResponses)
                .before(PropagateCosimSet::Propagate),
            sync_script_outputs
                .after(ModelicaSet::HandleResponses)
                .before(PropagateCosimSet::Propagate),
            sync_modelica_inputs
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests),
            sync_script_inputs
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests),
            // Modelica `when` bridge: edge-detect on fresh outputs, after they sync.
            fire_model_port_events
                .after(sync_modelica_outputs)
                .after(sync_script_outputs),
        ),
    );

    app.add_systems(
        Startup,
        |reg: Option<ResMut<lunco_api::ApiQueryRegistry>>| {
            if let Some(mut reg) = reg {
                // Canonical uniform port verbs (over `lunco_cosim::ports`).
                reg.register(ListPortsProvider);
                reg.register(GetPortProvider);
                reg.register(SetPortProvider);
                // Richer per-entity cosim introspection (not an alias of the above).
                reg.register(CosimStatusProvider);
            }
        },
    );

    // Registers the LoadScene type + observer (see register_commands! below).
    register_all_commands(app);
}

register_commands!(on_clear_scene, on_restart_scene,);



#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_root_prim ────────────────────────────────────────────
    //
    // `resolve_root_prim` no longer touches the filesystem: an explicit
    // override wins, and an empty override yields the deferred-resolution
    // sentinel (empty string). The actual `defaultPrim` lookup is done
    // from the parsed stage in `lunco_usd_bevy::instantiate_usd_prim`
    // (covered by `stage_default_prim` tests there) — correct on wasm too.

    #[test]
    fn resolve_root_prim_override_wins() {
        assert_eq!(resolve_root_prim("scene.usda", "/Override"), "/Override");
    }

    #[test]
    fn resolve_root_prim_empty_override_defers() {
        // Empty override → empty sentinel; resolved downstream against
        // the parsed stage, not here.
        assert_eq!(resolve_root_prim("scene.usda", ""), "");
    }
}
