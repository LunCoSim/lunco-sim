use bevy::prelude::*;

pub mod backend;
/// Language-neutral world bridge (verbs + native `ValueBuilder`); rhai/Python
/// are thin bindings over it. Compiled whenever a backend that uses it is
/// enabled — it pulls the ECS/world deps (`lunco-api`, `big_space`) those
/// features provide.
#[cfg(any(feature = "rhai", feature = "python"))]
pub mod bridge_core;
/// Language-neutral scenario lifecycle driver (`on_start`/`on_tick`/`on_event`/
/// `on_stop`, hot-reload, pause, teardown). Backends implement `ScenarioRuntime`.
#[cfg(any(feature = "rhai", feature = "python"))]
pub mod scenario;
pub mod commands;
pub mod python;
#[cfg(not(target_arch = "wasm32"))]
pub mod repl;
pub mod doc;
pub mod source_asset;
/// World-bound rhai execution (the `cmd`/`world_pos`/`get`/`find` bridge).
#[cfg(feature = "rhai")]
pub mod world_bridge;
/// Importable rhai tool libraries (named `libname::fn` modules).
#[cfg(feature = "rhai")]
pub mod tool_libs;
/// Twin persistence + discovery for declarative mission timelines
/// (`<twin>/timelines/*.json`; `ListTimelines`/`GetTimeline`/`RunStoredTimeline`).
#[cfg(feature = "rhai")]
pub mod timelines;
/// Scripting adapter onto the unified diagnostics store (`ScriptStatus` query).
#[cfg(feature = "rhai")]
pub mod diagnostics;
/// Authoring catalog (`ScriptingCatalog` query) — the discoverability surface
/// for editor completion / hover / docs.
#[cfg(feature = "rhai")]
pub mod catalog;

use std::collections::HashMap;
use lunco_doc::{DocumentId, DocumentHost};
use doc::{ScriptDocument, ScriptedModel};
// Brings the pyo3 method traits (`PyDictMethods::{set_item,get_item}`,
// `PyAnyMethods::{downcast,extract}`) into scope for `run_scripted_models`.
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[derive(Resource, Default)]
pub struct ScriptRegistry {
    pub documents: HashMap<DocumentId, DocumentHost<ScriptDocument>>,
}

pub struct LunCoScriptingPlugin;

impl Plugin for LunCoScriptingPlugin {
    fn build(&self, app: &mut App) {
        info!("Initializing LunCo Scripting Bridge...");
        python::initialize_python();

        if !app.is_plugin_added::<source_asset::PythonSourceAssetPlugin>() {
            app.add_plugins(source_asset::PythonSourceAssetPlugin);
        }
        // `.rhai` source asset loader — backs `lunco:scriptPath` (file-referenced
        // scenarios). Independent of the python feature.
        if !app.is_plugin_added::<source_asset::RhaiSourceAssetPlugin>() {
            app.add_plugins(source_asset::RhaiSourceAssetPlugin);
        }

        app.init_resource::<ScriptRegistry>();
        
        #[cfg(not(target_arch = "wasm32"))]
        {
            let repl = repl::spawn_repl_thread();
            app.insert_resource(repl);
        }

        app.register_type::<ScriptedModel>()
           .register_type::<doc::ScriptLanguage>();

        let python_status = python::get_python_status();
        app.insert_resource(python_status);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, repl::process_repl_commands);

        // Per-tick Python `ScriptedModel` executor (the inputs/outputs dict
        // model used by USD Python-cosim port mapping in `lunco-usd-sim`:
        // `sync_script_inputs` feeds `ScriptedModel.inputs`, this runs the
        // script, `sync_script_outputs` reads `ScriptedModel.outputs`). Python
        // only — rhai scenarios run via the world-bridge systems below.
        #[cfg(feature = "python")]
        app.add_systems(FixedUpdate, run_scripted_models);

        // World-bound rhai: a queue of (command_id, code) drained by an
        // exclusive system so scripts can `cmd()`/read the live `&mut World`.
        // `RunRhai` enqueues here instead of evaluating inline (an observer
        // can't hold `&mut World`); the drain records real stdout afterwards.
        #[cfg(feature = "rhai")]
        {
            // Seed built-in tool libraries (formation + the native mathx example)
            // BEFORE the runtime engine is built, so build_world_engine's refresh
            // binds them immediately.
            tool_libs::register_builtins();
            // Shared per-document diagnostics store (also init'd by Modelica;
            // init_resource is idempotent). Scenario compile/runtime errors land
            // here and surface via the ScriptStatus query.
            app.init_resource::<lunco_doc_bevy::DocumentDiagnostics>();
            app.init_resource::<world_bridge::PendingWorldScripts>();
            // The rhai scenario backend, wrapped in the language-neutral driver
            // (owns the on_start/on_tick/on_event/on_stop + hot-reload + pause +
            // teardown lifecycle; rhai supplies only the mechanics).
            app.init_resource::<scenario::ScenarioDriver<world_bridge::RhaiScenarioRuntime>>();
            // Mints document ids for scenarios attached via RunScenario.
            app.init_resource::<commands::ScenarioDocAllocator>();
            // Event channel: scenarios subscribe to the existing TelemetryEvent
            // bus via this observer (frame-delayed into on_event hooks). Neutral —
            // shared by every backend.
            app.init_resource::<scenario::ScriptEventInbox>();
            app.add_observer(scenario::collect_script_events);
            // Tool-library discovery on the API (ListToolLibraries/GetToolLibrary);
            // registration rides the RegisterToolLibrary command.
            tool_libs::register_queries(app);
            // Twin persistence: load every `<twin>/tools/*.rhai` shared tool
            // library when a Twin opens, so file-authored tools survive restarts
            // (native only — no filesystem on wasm).
            #[cfg(not(target_arch = "wasm32"))]
            app.add_observer(tool_libs::load_tools_on_twin_added);
            // Named mission timelines: in-memory store + `<twin>/timelines/*.json`
            // discovery (ListTimelines/GetTimeline), loaded on Twin open. The
            // RegisterTimeline/RunStoredTimeline commands ride this store.
            app.init_resource::<timelines::TimelineStore>();
            timelines::register_queries(app);
            #[cfg(not(target_arch = "wasm32"))]
            app.add_observer(timelines::load_timelines_on_twin_added);
            diagnostics::register_queries(app);
            // Authoring catalog: ScriptingCatalog aggregates the full callable
            // surface (verbs + commands + queries + tools + prelude) for editor
            // completion / hover / docs and agent discovery.
            catalog::register_queries(app);
            app.add_systems(
                FixedUpdate,
                (
                    world_bridge::drain_world_scripts,
                    // File-referenced scenarios (lunco:scriptPath): load the .rhai
                    // asset and swap the path marker for EmbeddedScenarioSource.
                    // Runs before attach so the loaded source attaches same frame.
                    commands::resolve_embedded_scenario_paths,
                    // USD-embedded scenarios: attach any the loader stamped with
                    // EmbeddedScenarioSource (lunco:script on the prim) so scene-
                    // authored scenarios run on spawn.
                    commands::attach_embedded_scenarios,
                    // Persistent per-entity scenario lifecycle (neutral driver,
                    // rhai backend).
                    world_bridge::tick_rhai_scenarios,
                )
                    // Same host-authoritative gate as the python path above — a
                    // predicting client must not run scripts (see `scripts_run_here`).
                    .run_if(scripts_run_here),
            );
        }

        // Pluggable script backends — one per language, per cargo feature.
        // The matching `RunPython` command is `#[cfg]`-gated on the same
        // feature, so the language only appears on the API when its runtime
        // is actually compiled in (no "accepted but no-op" lie). Python is
        // the only backend today.
        #[allow(unused_mut)]
        let mut backends = backend::ScriptBackends::default();
        // Rhai (pure Rust, wasm-clean) — the default backend, on by default.
        #[cfg(feature = "rhai")]
        backends.insert(doc::ScriptLanguage::Rhai, Box::new(backend::RhaiBackend));
        #[cfg(feature = "python")]
        backends.insert(doc::ScriptLanguage::Python, Box::new(backend::PythonBackend));
        app.insert_resource(backends);

        commands::register_all_commands(app);
        // Data-driven RBAC: declare the script-execution commands' policies in
        // the shared CommandPolicyRegistry so script submission goes through the
        // same authorization seam as every other command (Operator floor for
        // script execution; ownership-gated lifecycle). See the function's docs.
        #[cfg(any(feature = "rhai", feature = "python"))]
        commands::register_command_policies(app);
    }
}

/// Per-tick executor for Python `ScriptedModel`s (the legacy inputs/outputs
/// dict model). Python-only: rhai scenarios run via the world-bridge systems.
/// Feeds the USD Python-cosim path (`lunco-usd-sim/cosim.rs`), which syncs
/// `SimComponent` ports into `ScriptedModel.inputs` before this and reads
/// `ScriptedModel.outputs` after.
#[cfg(feature = "python")]
fn run_scripted_models(
    mut q_models: Query<&mut ScriptedModel>,
    registry: Res<ScriptRegistry>,
    python_status: Res<python::PythonStatus>,
) {
    for mut model in q_models.iter_mut() {
        if model.paused { continue; }

        let Some(doc_id_raw) = model.document_id else { continue };
        let doc_id = DocumentId::new(doc_id_raw);
        let Some(host) = registry.documents.get(&doc_id) else { continue };
        let doc = host.document();

        if doc.language != doc::ScriptLanguage::Python { continue; }

        if *python_status != python::PythonStatus::Available {
            error_once!("Python is not available on this system. Cannot run Python scripts.");
            continue;
        }
        pyo3::Python::with_gil(|py| {
            // 1. Prepare inputs
            let locals = pyo3::types::PyDict::new(py);
            let inputs_dict = pyo3::types::PyDict::new(py);
            for (k, v) in &model.inputs {
                let _ = inputs_dict.set_item(k, v);
            }
            let outputs_dict = pyo3::types::PyDict::new(py);
            for (k, v) in &model.outputs {
                let _ = outputs_dict.set_item(k, v);
            }
            let _ = locals.set_item("inputs", inputs_dict);
            let _ = locals.set_item("outputs", outputs_dict);

            // 2. Run source
            // TODO(CQ-217): `py.run` re-parses + recompiles the script
            // source from scratch on *every* FixedUpdate tick. Compile
            // once (`PyModule`/`compile` → cached code object, keyed on
            // doc source revision) and only execute the cached code per
            // tick. See docs/code-quality-remediation.md (CQ-217).
            let c_str = match std::ffi::CString::new(doc.source.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    error!("ScriptedModel: source contains a NUL byte; skipping");
                    return;
                }
            };
            if let Err(e) = py.run(&c_str, None, Some(&locals)) {
                error!("ScriptedModel Python Error: {}", e);
            } else {
                // 3. Extract outputs
                if let Ok(Some(outputs)) = locals.get_item("outputs") {
                    if let Ok(dict) = outputs.downcast::<pyo3::types::PyDict>() {
                        for (k, v) in dict.iter() {
                            if let (Ok(key), Ok(val)) = (k.extract::<String>(), v.extract::<f64>()) {
                                model.outputs.insert(key, val);
                            }
                        }
                    }
                }
            }
        });
    }
}

/// Run condition: may scenario/script systems execute in THIS process?
///
/// Scripts are **host-authoritative** — they are the authoritative decision-maker
/// for a scripted entity. They run on the `Host` and in single-player /
/// headless (`Standalone`, or no role at all), but NOT on a networked `Client`.
///
/// The netcode is forward predict-and-smooth (no rewind/resimulate), so a client
/// runs each `FixedUpdate` tick exactly once — but if scripts ran there they'd
/// independently re-decide behavior the host already decided: double-firing
/// `cmd()` commands and `emit()` telemetry into the client world, and advancing a
/// per-entity `this` state that lives OUTSIDE the replicated / reconciled set and
/// so would diverge from the host with nothing to correct it. A client must
/// instead receive scripted behavior purely via replication of the resulting
/// entity state. Mirrors cosim's identical gate (`lunco-cosim/src/lib.rs`).
#[cfg(feature = "rhai")]
fn scripts_run_here(role: Option<Res<lunco_core::NetworkRole>>) -> bool {
    !matches!(role.as_deref(), Some(lunco_core::NetworkRole::Client))
}

