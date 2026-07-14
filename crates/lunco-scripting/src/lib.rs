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
/// rhai task maps compiled onto the `lunco-behavior` kernel — the native tick
/// engine behind the prelude's `seq`/`par_*`/`repeat`/`wait_*` task vocabulary
/// (replaces the prelude's retired `__tick*` rhai recursion).
#[cfg(feature = "rhai")]
pub mod task_tree;
/// Importable rhai tool libraries (named `libname::fn` modules).
#[cfg(feature = "rhai")]
pub mod tool_libs;
/// Twin persistence + discovery for declarative mission timelines
/// (`<twin>/timelines/*.json`; `ListTimelines`/`GetTimeline`/`RunStoredTimeline`).
#[cfg(feature = "rhai")]
pub mod timelines;
/// Journaling for named registrations (tool libraries + timelines) — so a
/// `RegisterToolLibrary`/`RegisterTimeline` syncs + persists via the journal plane.
#[cfg(feature = "rhai")]
pub mod registration_journal;
/// Scripting adapter onto the unified diagnostics store (`ScriptStatus` query).
#[cfg(feature = "rhai")]
pub mod diagnostics;
/// Authoring catalog (`ScriptingCatalog` query) — the discoverability surface
/// for editor completion / hover / docs.
#[cfg(feature = "rhai")]
pub mod catalog;

use std::collections::HashMap;
use lunco_doc::{Document, DocumentId, DocumentHost};
use doc::{ScriptDocument, ScriptedModel};
// Brings the pyo3 method traits (`PyDictMethods::{set_item,get_item}`,
// `PyAnyMethods::{downcast,extract}`) into scope for `run_scripted_models`.
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[derive(Resource, Default)]
pub struct ScriptRegistry {
    pub documents: HashMap<DocumentId, DocumentHost<ScriptDocument>>,
    /// Twin-journal handle, wired once the [`JournalResource`](lunco_doc_bevy::JournalResource)
    /// appears (see [`wire_scripting_journal_handle`]). When set, every host gets
    /// a [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) so edits —
    /// including hot-reload `SetSource` and undo/redo — auto-record. `None` → no
    /// recording (the pre-journal / test path). Mirrors `ModelicaDocumentRegistry`.
    journal: Option<lunco_doc_bevy::JournalResource>,
}

impl ScriptRegistry {
    /// Insert (or replace) a `ScriptDocument` host under `id`, attaching a journal
    /// recorder when a journal is wired. **The one insert funnel** — every attach
    /// path (rhai scenario, Python cosim) routes through here so recording is
    /// automatic ("sync by default by design"), never dependent on remembering to
    /// wire it at each call site.
    pub fn insert_document(&mut self, id: DocumentId, doc: ScriptDocument) {
        self.documents.insert(id, DocumentHost::new(doc));
        self.attach_recorder(id);
    }

    /// Wire the Twin-journal handle and retro-fit a recorder onto every existing
    /// host. Called once, reactively, the frame the
    /// [`JournalResource`](lunco_doc_bevy::JournalResource) first appears; hosts
    /// created afterwards get their recorder at insert time.
    pub fn set_journal(&mut self, journal: lunco_doc_bevy::JournalResource) {
        self.journal = Some(journal);
        let ids: Vec<_> = self.documents.keys().copied().collect();
        for id in ids {
            self.attach_recorder(id);
        }
    }

    /// Attach a recorder to `id`'s host when a journal is wired and the host lacks
    /// one. Idempotent (`has_recorder` guard) — the auto-bridge seam that makes
    /// every apply/undo/redo record losslessly with no per-op code.
    fn attach_recorder(&mut self, id: DocumentId) {
        if let Some(journal) = &self.journal {
            if let Some(host) = self.documents.get_mut(&id) {
                if !host.has_recorder() {
                    lunco_doc_bevy::attach_journal_recorder(host, journal);
                }
            }
        }
    }

}

// `replay_op` deserializes the journal payload with serde_json, which is only
// pulled in under a backend feature. A script-free build (`--no-default-features`)
// has no scripts to replay, so this simply isn't compiled there.
#[cfg(any(feature = "rhai", feature = "python"))]
impl ScriptRegistry {
    /// Apply a **journal op** to `doc` for replay (journal→document projection —
    /// the networked-edit consume path) **without recording it**. Mirror of
    /// [`ModelicaDocumentRegistry::replay_op`](lunco_modelica::state::ModelicaDocumentRegistry::replay_op).
    /// The op is already in the journal (arrived via `append_remote`), so applying
    /// straight to the document bypasses the recorder to avoid a duplicate entry.
    /// `op` is the entry's serialized [`doc::ScriptOp`]. Returns `false` (logged,
    /// non-fatal) if the doc is unknown, the payload isn't a `ScriptOp`, or the
    /// apply is rejected (e.g. a read-only origin).
    pub fn replay_op(&mut self, doc: DocumentId, op: &serde_json::Value) -> bool {
        let parsed = match serde_json::from_value::<doc::ScriptOp>(op.clone()) {
            Ok(op) => op,
            Err(e) => {
                warn!("[script-replay] op payload is not a ScriptOp: {e}");
                return false;
            }
        };
        let Some(host) = self.documents.get_mut(&doc) else {
            return false;
        };
        match host.document_mut().apply(parsed) {
            Ok(_) => true,
            Err(e) => {
                warn!("[script-replay] apply rejected on doc {doc}: {e:?}");
                false
            }
        }
    }
}

/// A3 auto-bridge: hand the [`JournalResource`](lunco_doc_bevy::JournalResource)
/// to the `ScriptRegistry` the moment it appears, so it fits a recorder onto
/// existing and future script hosts. Reactive (`resource_added`), runs once.
pub fn wire_scripting_journal_handle(
    mut registry: ResMut<ScriptRegistry>,
    journal: Res<lunco_doc_bevy::JournalResource>,
) {
    registry.set_journal(journal.clone());
}

pub struct LunCoScriptingPlugin;

/// Register the built-in `policy→rhai` hooks from `assets/scripting/policy/*.rhai`.
/// Each is a small authored decision function consulted by a Rust seam by hook id;
/// authoring the rule in rhai keeps policy out of compiled code (tunable, no
/// rebuild). Currently: the spec-034 control-authority takeover rule.
#[cfg(feature = "rhai")]
fn register_builtin_policies() {
    // (policy file stem, hook id, entry fn)
    const BUILTINS: &[(&str, &str, &str)] = &[
        (
            "control_authority",
            lunco_core::session::CONTROL_AUTHORITY_HOOK,
            "may_take_control",
        ),
        // Boot-entry policy: what does the app do at startup? (onboard / load /
        // resume / nothing). Consulted by `lunco_tutorial::consult_boot`.
        ("boot", lunco_core::session::BOOT_HOOK, "boot_entry"),
        // (Link availability is not a builtin policy. The generic link kernel
        // computes the geometry and applies a builtin range+mask+occlusion rule;
        // an authored `link.connected` hook overrides the verdict, and routing is
        // rhai over `query("Links")` — see doc 49 / prelude/links.rhai.)
    ];
    for (stem, hook_id, entry) in BUILTINS {
        let Some(src) = lunco_assets::scripting::policy(stem) else {
            warn!("[policy] built-in policy '{stem}' missing from embedded assets");
            continue;
        };
        match lunco_hooks_rhai::register_rhai_hook(*hook_id, *entry, src, false) {
            Ok(_) => info!("[policy] registered built-in '{stem}' → {hook_id}"),
            Err(e) => error!("[policy] built-in policy '{stem}' failed to compile: {e}"),
        }
    }
}

impl Plugin for LunCoScriptingPlugin {
    fn build(&self, app: &mut App) {
        info!("Initializing LunCo Scripting Bridge...");

        // Scripting is INDEPENDENT of the HTTP API. `cmd()` dispatches through the
        // transport-free command core (reflect dispatcher + entity registry), so
        // we self-supply it here rather than assuming `LunCoApiPlugin` was added.
        // An app can now embed scripting with NO API server and scripts still
        // reach every `#[Command]`. Guarded, so it composes with `LunCoApiPlugin`
        // (which adds the same core) in either order. Only the rhai/python
        // backends pull `lunco-api`, hence the cfg.
        #[cfg(any(feature = "rhai", feature = "python"))]
        lunco_api::ensure_command_core(app);

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
        // A3 auto-bridge: when the Twin journal appears, fit a recorder onto every
        // ScriptDocument host so live script edits (rover behaviour changes) record
        // into the canonical journal like Modelica/USD — "scripts sync by design".
        app.add_systems(
            Update,
            wire_scripting_journal_handle
                .run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let repl = repl::spawn_repl_thread();
            app.insert_resource(repl);
        }

        app.register_type::<ScriptedModel>()
           .add_observer(doc::on_remove_scripted)
           .register_type::<doc::ScriptLanguage>();

        let python_status = python::get_python_status();
        app.insert_resource(python_status);

        // REPL drain: rhai (world-connected) is the default; python-only builds
        // fall back to the interpreter path. Wasm has no stdin, so neither runs.
        #[cfg(all(not(target_arch = "wasm32"), feature = "rhai"))]
        app.add_systems(Update, repl::drain_repl_rhai);
        #[cfg(all(not(target_arch = "wasm32"), feature = "python", not(feature = "rhai")))]
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
            // Built-in `policy→rhai` hooks: register each `assets/scripting/policy/
            // *.rhai` under its hook id so the possession / authorization paths
            // consult AUTHORED rhai rules, not hardcoded Rust (spec 034 control-
            // authority takeover). These are the weakest-scope defaults; a
            // `LunCoPolicy` USD prim projected at the same seam hot-replaces them.
            register_builtin_policies();
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
                    // One-shot `RunRhai` evals stay host-authoritative — they carry
                    // no `ScriptScope`, so a client can't run arbitrary sim-mutating
                    // snippets through the REPL/world-script queue.
                    world_bridge::drain_world_scripts.run_if(scripts_run_here),
                    // File-referenced scenarios (lunco:scriptPath): load the .rhai
                    // asset and swap the path marker for EmbeddedScenarioSource.
                    // Runs before attach so the loaded source attaches same frame.
                    commands::resolve_embedded_scenario_paths,
                    // USD-embedded scenarios: attach any the loader stamped with
                    // EmbeddedScenarioSource (lunco:script on the prim) so scene-
                    // authored scenarios run on spawn.
                    commands::attach_embedded_scenarios,
                    // Persistent per-entity scenario lifecycle (neutral driver,
                    // rhai backend). Runs on EVERY peer now: the driver gates each
                    // entity by its `ScriptScope` (host-only by default), and a
                    // client-scoped scenario's `cmd()`s are restricted to the
                    // client-local surface — so a predicting client runs only
                    // presentation/HUD scripts, never authoritative sim mutation.
                    world_bridge::tick_rhai_scenarios,
                ),
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


#[cfg(all(test, any(feature = "rhai", feature = "python")))]
mod journal_tests {
    use super::*;
    use doc::{ScriptDocument, ScriptLanguage, ScriptOp};

    // `replay_op` applies a journal-sourced `ScriptOp` straight to the document
    // (bypassing the recorder), so a peer's live source edit projects locally.
    #[test]
    fn replay_op_applies_setsource_to_the_document() {
        let mut reg = ScriptRegistry::default();
        let id = DocumentId::new(1);
        reg.insert_document(id, ScriptDocument::new(1, ScriptLanguage::Rhai, "v1"));

        let op = serde_json::to_value(ScriptOp::SetSource("v2".into())).unwrap();
        assert!(reg.replay_op(id, &op), "valid ScriptOp replays");
        assert_eq!(reg.documents.get(&id).unwrap().document().source, "v2");

        // Unknown doc and non-ScriptOp payloads fail softly (logged, false).
        assert!(!reg.replay_op(DocumentId::new(999), &op));
        assert!(!reg.replay_op(id, &serde_json::json!({ "nope": 1 })));
    }
}
