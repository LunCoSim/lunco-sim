use bevy::prelude::*;

pub mod backend;
#[cfg(any(feature = "rhai", feature = "python"))]
mod commands;
pub mod doc;
#[cfg(feature = "python")]
pub mod python;
/// Language-neutral scenario lifecycle driver (native task/mission policy plus
/// `on_start`/`on_tick`/`on_event`/`on_stop`, hot-reload, pause, teardown).
/// Backends implement `ScenarioRuntime`.
#[cfg(any(feature = "rhai", feature = "python"))]
pub mod scenario;
pub mod source_asset;

pub use doc::{ScenarioParameters, ScenarioReloadPolicy, ScriptDocument, ScriptedModel};
#[cfg(any(feature = "rhai", feature = "python"))]
use lunco_doc::Document;
use lunco_doc::{DocumentHost, DocumentId, FileBacked, Reject};
use std::collections::HashMap;
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
    /// recording (the pre-journal / test path). Mirrors the generic document
    /// registry's journal wiring.
    journal: Option<lunco_doc_bevy::JournalResource>,
}

/// Marks a script document and runtime whose ownership belongs to a loaded USD
/// scene. This covers authored Rhai scenarios and USD Python cosim documents.
///
/// Interactive/API scenarios deliberately have an independent document
/// lifetime. USD-embedded scenarios do not: keeping their document or compiled
/// state after the prim is gone lets a later scene observe the previous scene's
/// state. The scene owner uses this marker at its teardown boundary to stop the
/// program when applicable and close the scene-owned document.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SceneOwnedScript;

/// Fixed-step execution boundary for every stateful scripting backend.
///
/// Co-simulation owns the port exchange around this boundary: inputs are
/// copied into `ScriptedModel` before the set, the selected backend executes
/// exactly once for the fixed tick, and its output snapshot is published for
/// the next propagation phase. Keeping this as a public system set makes that
/// ordering an explicit cross-crate contract instead of relying on Bevy's
/// insertion order.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptingSet;

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

    /// Apply a user-authored operation to a script document through its host.
    ///
    /// Keeping this funnel on the registry is important for source edits that
    /// arrive from commands or the editor: the host owns the inverse stack and
    /// its recorder mirrors the operation, undo, and redo into the Twin
    /// journal. Replacing a host would silently discard both.
    pub fn apply(&mut self, doc: DocumentId, op: doc::ScriptOp) -> Result<lunco_doc::Ack, Reject> {
        let host = self
            .documents
            .get_mut(&doc)
            .ok_or_else(|| Reject::InvalidOp(format!("unknown script document {doc}")))?;
        host.apply(op)
    }

    /// Refresh a script from an external source owner without creating an
    /// editor history entry. USD-authored `info:sourceCode` and a changed
    /// file-backed `.rhai` asset already have their own authoritative source;
    /// mirroring that refresh as a second Script journal op would duplicate
    /// the same change and make undo restore a stale projection.
    pub fn reload_external_source(&mut self, doc: DocumentId, source: &str) -> bool {
        let Some(host) = self.documents.get_mut(&doc) else {
            return false;
        };
        FileBacked::reload_base(host.document_mut(), source)
    }
}

// `replay_op` deserializes the journal payload with serde_json, which is only
// pulled in under a backend feature. A script-free build (`--no-default-features`)
// has no scripts to replay, so this simply isn't compiled there.
#[cfg(any(feature = "rhai", feature = "python"))]
impl ScriptRegistry {
    /// Apply a **journal op** to `doc` for replay (journal→document projection —
    /// the networked-edit consume path) **without recording it**. Mirror of
    /// [`lunco_doc_bevy::DocumentRegistry::replay_op`].
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

/// Drop an independent script document's host on explicit close. Interactive
/// and API script documents follow the document-lifecycle doctrine: despawning
/// their `ScriptedModel` entity leaves the document open until `CloseDocument`.
/// Scene-authored scenario documents are the explicit exception: they carry
/// [`SceneOwnedScript`] and the scene teardown owner closes them before the
/// USD entities are reclaimed.
pub fn on_close_script_document(
    trigger: On<lunco_doc_bevy::CloseDocument>,
    mut registry: ResMut<ScriptRegistry>,
) {
    registry.documents.remove(&trigger.event().doc_id);
}

pub struct LunCoScriptingPlugin;

impl Plugin for LunCoScriptingPlugin {
    fn build(&self, app: &mut App) {
        info!("Initializing LunCo Scripting Bridge...");

        #[cfg(any(feature = "rhai", feature = "python"))]
        app.init_resource::<scenario::ScenarioExecutionGate>()
            .init_resource::<scenario::ScenarioReadinessArm>()
            .init_resource::<scenario::ScenarioSceneGeneration>()
            .add_observer(scenario::close_scenarios_for_scene_transition)
            .add_observer(scenario::arm_scenarios_after_scene_composition)
            .add_systems(
                PreUpdate,
                scenario::open_scenarios_when_scene_ready.after(lunco_readiness::ReadinessSet),
            );

        #[cfg(feature = "python")]
        if !app.is_plugin_added::<source_asset::PythonSourceAssetPlugin>() {
            app.add_plugins(source_asset::PythonSourceAssetPlugin);
        }
        app.init_resource::<ScriptRegistry>();
        // Attended (a person is watching) or not — read by the `is_unattended()`
        // verb so a lesson knows whether to drive itself. Windowed application
        // hosts opt into resolving this in Startup; the default remains
        // `Unattended` for headless hosts.
        #[cfg(any(feature = "rhai", feature = "python"))]
        {
            app.init_resource::<lunco_scripting_bridge_core::ScenarioAudience>();
            #[cfg(feature = "window-audience")]
            app.add_systems(Startup, scenario::resolve_scenario_audience);
        }
        app.add_observer(on_close_script_document);
        // A3 auto-bridge: when the Twin journal appears, fit a recorder onto every
        // ScriptDocument host so live script edits (rover behaviour changes) record
        // into the canonical journal like Modelica/USD — "scripts sync by design".
        app.add_systems(
            Update,
            wire_scripting_journal_handle.run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );

        app.register_type::<ScriptedModel>()
            .register_type::<doc::ScenarioParameters>()
            .register_type::<doc::ScriptLanguage>();

        configure_scripting_schedule(app);

        #[cfg(feature = "python")]
        app.init_resource::<python::PythonStatus>();

        // Per-tick Python `ScriptedModel` executor (the inputs/outputs dict
        // model used by USD Python-cosim port mapping in `lunco-usd-sim`:
        // `sync_script_inputs` feeds `ScriptedModel.inputs`, this runs the
        // script, `sync_script_outputs` reads `ScriptedModel.outputs`). Python
        // only — Rhai scenarios run in `lunco-scripting-rhai-runtime`.
        #[cfg(feature = "python")]
        {
            // Shared per-document diagnostics store (also init'd by the rhai
            // branch and Modelica; init_resource is idempotent). Python compile
            // errors land here and surface via the ScriptStatus query, exactly
            // like a rhai scenario's compile diagnostics.
            app.init_resource::<lunco_doc_bevy::DocumentDiagnostics>();
            app.add_systems(
                FixedUpdate,
                run_scripted_models
                    .in_set(ScriptingSet)
                    .run_if(scenario::scenario_execution_enabled)
                    .run_if(scenario::simulation_is_running),
            );
        }

        // Pluggable script backends — one per language, per cargo feature.
        // The matching `RunPython` command is `#[cfg]`-gated on the same
        // feature, so the language only appears on the API when its runtime
        // is actually compiled in (no "accepted but no-op" lie). Python is
        // the only backend today.
        #[cfg(feature = "python")]
        let backends = {
            let mut backends = backend::ScriptBackends::default();
            backends.insert(
                doc::ScriptLanguage::Python,
                Box::new(backend::PythonBackend),
            );
            backends
        };
        #[cfg(not(feature = "python"))]
        let backends = backend::ScriptBackends::default();
        app.insert_resource(backends);
        #[cfg(any(feature = "rhai", feature = "python"))]
        {
            commands::register_all_commands(app);
            commands::register_command_policies(app);
        }
    }
}

fn configure_scripting_schedule(app: &mut App) {
    #[cfg(any(feature = "rhai", feature = "python"))]
    app.configure_sets(
        FixedUpdate,
        ScriptingSet.after(lunco_core_runtime::SimTickSet),
    );
    #[cfg(not(any(feature = "rhai", feature = "python")))]
    app.configure_sets(FixedUpdate, ScriptingSet);
}

#[cfg(all(test, any(feature = "rhai", feature = "python")))]
mod schedule_tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ObservedTick(u64);

    fn advance_tick(mut tick: ResMut<lunco_core_runtime::SimTick>) {
        tick.0 += 1;
    }

    fn observe_tick(tick: Res<lunco_core_runtime::SimTick>, mut observed: ResMut<ObservedTick>) {
        observed.0 = tick.0;
    }

    #[test]
    fn scripting_runs_after_the_authoritative_fixed_tick() {
        let mut app = App::new();
        app.init_resource::<lunco_core_runtime::SimTick>()
            .init_resource::<ObservedTick>();
        configure_scripting_schedule(&mut app);
        app.add_systems(
            FixedUpdate,
            advance_tick.in_set(lunco_core_runtime::SimTickSet),
        )
        .add_systems(FixedUpdate, observe_tick.in_set(ScriptingSet));

        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(app.world().resource::<ObservedTick>().0, 1);
    }
}

/// A memoized Python compile outcome for one `ScriptDocument`, keyed on the
/// document generation (CQ-217). Mirrors the rhai scenario runtime's
/// content-addressed `CompileOutcome` memo: errors are memoized too, so a bad
/// source is diagnosed ONCE per edit instead of re-parsed (and re-logged)
/// every FixedUpdate tick.
#[cfg(feature = "python")]
struct PyCompiledDoc {
    /// `ScriptDocument::generation` this outcome was compiled at. A source
    /// edit bumps the generation → recompile on next tick.
    generation: u64,
    /// The cached code object (`builtins.compile(source, ..., 'exec')`), or
    /// `Err` for a source that failed to compile (already diagnosed).
    code: Result<pyo3::Py<pyo3::PyAny>, ()>,
}

/// Per-tick executor for Python `ScriptedModel`s (the port-mapped
/// inputs/outputs model). Python-only: rhai scenarios run via the world-bridge systems.
/// Feeds the USD Python-cosim path (`lunco-usd-sim/cosim.rs`), which syncs
/// `SimComponent` ports into `ScriptedModel.inputs` before this and reads
/// `ScriptedModel.outputs` after.
///
/// Sources are compiled ONCE per document generation (see [`PyCompiledDoc`]);
/// the per-tick work is executing the cached code object. Compile errors are
/// published to `DocumentDiagnostics` (Error state, surfaced by the
/// `ScriptStatus` query) and cleared with `set_ok` on a clean recompile —
/// the same lifecycle the rhai scenario driver gives its documents.
#[cfg(feature = "python")]
fn run_scripted_models(
    mut q_models: Query<&mut ScriptedModel>,
    registry: Res<ScriptRegistry>,
    mut python_status: ResMut<python::PythonStatus>,
    mut diagnostics: ResMut<lunco_doc_bevy::DocumentDiagnostics>,
    mut compiled: Local<std::collections::HashMap<u64, PyCompiledDoc>>,
) {
    for mut model in q_models.iter_mut() {
        if model.paused {
            continue;
        }

        let Some(doc_id_raw) = model.document_id else {
            continue;
        };
        let doc_id = DocumentId::new(doc_id_raw);
        let Some(host) = registry.documents.get(&doc_id) else {
            continue;
        };
        let doc = host.document();

        if doc.language != doc::ScriptLanguage::Python {
            continue;
        }

        python::ensure_initialized(&mut python_status);
        if *python_status != python::PythonStatus::Available {
            error_once!("Python is not available on this system. Cannot run Python scripts.");
            continue;
        }
        pyo3::Python::with_gil(|py| {
            // 1. Compile once per document generation (CQ-217). The memo also
            // caches failures, so a broken source costs one diagnostic per
            // edit, not one parse attempt per tick.
            let stale = compiled
                .get(&doc_id_raw)
                .map(|c| c.generation != doc.generation)
                .unwrap_or(true);
            if stale {
                let outcome = py
                    .import("builtins")
                    .and_then(|b| b.getattr("compile"))
                    .and_then(|c| c.call1((doc.source.as_str(), "<scripted_model>", "exec")));
                let code = match outcome {
                    Ok(code) => {
                        // Clean (re)compile clears any prior error state.
                        diagnostics.set_ok(DocumentId::new(doc_id_raw));
                        Ok(code.unbind())
                    }
                    Err(e) => {
                        error!("ScriptedModel Python compile error: {}", e);
                        diagnostics.set_diagnostics(
                            DocumentId::new(doc_id_raw),
                            vec![lunco_doc::Diagnostic::error(e.to_string(), None, None)],
                        );
                        Err(())
                    }
                };
                compiled.insert(
                    doc_id_raw,
                    PyCompiledDoc {
                        generation: doc.generation,
                        code,
                    },
                );
            }
            let Some(Ok(code)) = compiled.get(&doc_id_raw).map(|c| &c.code) else {
                // Already diagnosed at compile time — nothing to run.
                return;
            };

            // 2. Prepare inputs
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

            // 3. Execute the cached code object. `exec(code, globals, locals)`
            // with `__main__`'s dict as globals — the same environment
            // `py.run(source, None, Some(locals))` used before the compile
            // cache existed.
            let ran = py
                .import("__main__")
                .and_then(|m| {
                    let globals = m.dict();
                    py.import("builtins")?.getattr("exec")?.call1((
                        code.bind(py),
                        &globals,
                        &locals,
                    ))
                })
                .map(|_| ());
            if let Err(e) = ran {
                error!("ScriptedModel Python Error: {}", e);
            } else {
                // 4. Extract outputs
                if let Ok(Some(outputs)) = locals.get_item("outputs") {
                    if let Ok(dict) = outputs.downcast::<pyo3::types::PyDict>() {
                        for (k, v) in dict.iter() {
                            if let (Ok(key), Ok(val)) = (k.extract::<String>(), v.extract::<f64>())
                            {
                                model.outputs.insert(key, val);
                            }
                        }
                    }
                }
            }
        });
    }
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

    #[test]
    fn source_edits_and_script_undo_redo_share_the_journaled_host() {
        let mut reg = ScriptRegistry::default();
        let journal = lunco_doc_bevy::JournalResource::default_local();
        reg.set_journal(journal.clone());
        let id = DocumentId::new(2);
        reg.insert_document(id, ScriptDocument::new(2, ScriptLanguage::Rhai, "v1"));

        reg.apply(id, ScriptOp::SetSource("v2".into()))
            .expect("user source edit applies");
        assert_eq!(reg.documents.get(&id).unwrap().document().source, "v2");

        reg.documents
            .get_mut(&id)
            .unwrap()
            .undo()
            .expect("script undo applies");
        assert_eq!(reg.documents.get(&id).unwrap().document().source, "v1");

        reg.documents
            .get_mut(&id)
            .unwrap()
            .redo()
            .expect("script redo applies");
        assert_eq!(reg.documents.get(&id).unwrap().document().source, "v2");
        assert_eq!(journal.len(), 3, "apply, undo, and redo are all journaled");
    }
}
