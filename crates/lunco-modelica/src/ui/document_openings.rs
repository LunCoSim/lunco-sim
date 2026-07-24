//! Per-document container for in-flight parse tasks.
//!
//! Holds one [`OpeningState`] per [`DocumentId`] until the parse
//! resolves and the driver hands the document to
//! [`crate::state::ModelicaDocumentRegistry`]. Each variant
//! owns its own typed `Task<...>` plus a [`lunco_workbench::status_bus::BusyHandle`]
//! that keeps a `(BusyScope::Document, "opening"|"drill-in"|"duplicate")`
//! entry on the bus for the parse lifetime.
//!
//! **This is not the loading-state authority.** UI panels query the
//! [`lunco_workbench::status_bus::StatusBus`] directly
//! (`bus.is_busy(BusyScope::Document(d.0))` or
//! `bus.lifecycle(...)`) so a single predicate covers every async
//! stage that contributes to a doc's view (parse, projection,
//! reparse, future fetch/index/etc.). The accessors here (`detail`,
//! `progress`, `drill_in_qualified`, `duplicate_display`) return
//! metadata *about* the in-flight task — display name, drill-in
//! target, elapsed time — used by tab-title / placeholder-snapshot
//! code that needs to know what the doc *will be* before it's
//! installed.

use bevy::prelude::*;
use bevy::tasks::Task;
use lunco_doc::DocumentId;
use std::collections::HashMap;

use crate::package_tree::cache::FileLoadResult;
use crate::ui::panels::canvas_diagram::loads::{DrillInBinding, DuplicateBinding};

/// One in-flight document open. Each variant carries the typed
/// `Task<...>` plus the metadata that variant's driver needs to
/// finish the install (drilled-class name, display name, busy
/// handle for the status bus, etc.).
pub enum OpeningState {
    /// Bundled or user-file read driven by the Package Browser. The
    /// task returns a fully-built [`FileLoadResult`]; the driver
    /// installs `result.doc` against `result.doc_id`.
    FileLoad {
        display_name: String,
        task: Task<FileLoadResult>,
        /// RAII guard registered with [`lunco_workbench::status_bus::StatusBus`]
        /// at insert time. Same role as [`DrillInBinding::busy`] and
        /// [`DuplicateBinding::busy`]: keeps a `(Document(doc_id),
        /// "opening")` entry on the bus from "user clicked open" until
        /// the file-load driver hands it off to the projection stage
        /// via [`crate::ui::panels::canvas_diagram::CanvasDiagramState::stash_projection_handoff`].
        busy: lunco_workbench::status_bus::BusyHandle,
    },
    /// MSL drill-in slim-slice load. Built by
    /// [`crate::ui::panels::canvas_diagram::drill_into_class`].
    DrillIn(DrillInBinding),
    /// `Duplicate to Workspace` bg parse. Built by
    /// [`crate::ui::commands::lifecycle::on_duplicate_model_from_read_only`].
    Duplicate(DuplicateBinding),
}

/// Per-document task container. Drivers iterate its entries
/// filtered to their own variant; panels that need *metadata about*
/// an in-flight open (tab title, placeholder snapshot) read via the
/// accessors below. "Is this doc busy?" queries belong on the
/// [`lunco_workbench::status_bus::StatusBus`], not here.
#[derive(Resource, Default)]
pub struct DocumentOpenings {
    pub in_flight: HashMap<DocumentId, OpeningState>,
}

impl DocumentOpenings {
    /// Qualified class name of an in-flight drill-in for `doc`, if
    /// any. Used by placeholder snapshot code (`model_view/context.rs`)
    /// to construct tab titles + URIs before the doc is installed.
    pub fn drill_in_qualified(&self, doc: DocumentId) -> Option<&str> {
        match self.in_flight.get(&doc)? {
            OpeningState::DrillIn(b) => Some(b.qualified.as_str()),
            _ => None,
        }
    }

    /// Display name of an in-flight duplicate for `doc`, if any.
    /// Same placeholder-snapshot role as [`Self::drill_in_qualified`].
    pub fn duplicate_display(&self, doc: DocumentId) -> Option<&str> {
        match self.in_flight.get(&doc)? {
            OpeningState::Duplicate(b) => Some(b.display_name.as_str()),
            _ => None,
        }
    }

    pub fn insert(&mut self, doc: DocumentId, state: OpeningState) {
        self.in_flight.insert(doc, state);
    }

    pub fn remove(&mut self, doc: DocumentId) -> Option<OpeningState> {
        self.in_flight.remove(&doc)
    }

    pub fn get_mut(&mut self, doc: DocumentId) -> Option<&mut OpeningState> {
        self.in_flight.get_mut(&doc)
    }

    pub fn doc_ids(&self) -> Vec<DocumentId> {
        self.in_flight.keys().copied().collect()
    }

    pub fn has_any_drill_in(&self) -> bool {
        self.in_flight
            .values()
            .any(|s| matches!(s, OpeningState::DrillIn(_)))
    }

    pub fn has_any_duplicate(&self) -> bool {
        self.in_flight
            .values()
            .any(|s| matches!(s, OpeningState::Duplicate(_)))
    }
}

/// In-flight per-document `StatusBus` handles for AST reparse —
/// the debounced background parse that runs after a free-form source
/// edit. Distinct from file-load / drill-in / duplicate openings
/// because reparse doesn't have its own typed `Task<...>` we can
/// hang a handle off (parse is dispatched through
/// `ModelicaEngineHandle::upsert_document_async`, which takes a
/// caller-provided spawn callback, plus a wasm worker fallback —
/// too many paths to thread a handle through individually).
///
/// Instead, [`track_ast_reparse_busy`] derives "is reparse in
/// flight?" from the document's own `ast_is_stale()` predicate each
/// frame: rising edge mints a `Document(d) / "reparse"` entry on the
/// bus; falling edge drops it. Renders see continuous busy across
/// typing-debounce → parse → AST-install without a per-edit gap.
#[derive(Resource, Default)]
pub struct AstReparseBusyHandles {
    handles: HashMap<DocumentId, lunco_workbench::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for AST reparse state. Mints a `StatusBus`
/// handle when `ast_is_stale()` flips from false → true and drops it
/// when it flips back. Lets the canvas overlay rely on
/// `bus.lifecycle(Document(d), ...)` alone without an ast-stale
/// fallback predicate.
pub fn track_ast_reparse_busy(
    registry: Res<crate::state::ModelicaDocumentRegistry>,
    mut handles: ResMut<AstReparseBusyHandles>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
) {
    use lunco_workbench::status_bus::{BusyScope, StatusBus};
    let mut still_stale: std::collections::HashSet<DocumentId> = Default::default();
    for (doc_id, host) in registry.iter() {
        if !host.document().ast_is_stale() {
            continue;
        }
        still_stale.insert(doc_id);
        if handles.handles.contains_key(&doc_id) {
            continue;
        }
        let h = StatusBus::begin(
            &mut bus,
            BusyScope::Document(doc_id.0),
            "reparse",
            "Reparsing…",
        );
        handles.handles.insert(doc_id, h);
    }
    // Drop handles for docs that are no longer stale (or have been
    // closed). `Drop` clears the bus entry on the next
    // `drain_busy_drops` tick.
    handles.handles.retain(|d, _| still_stale.contains(d));
}

/// In-flight per-document `StatusBus` handles for compile work.
/// Same edge-triggered pattern as [`AstReparseBusyHandles`]: minted
/// when [`lunco_doc_bevy::DocumentDiagnostics::is_compiling`] rises, dropped
/// when it falls — with the terminal outcome (`Succeeded` /
/// `Failed(msg)`) recorded for [`lunco_workbench::status_bus::StatusBus::lifecycle`]
/// consumers.
///
/// Compile runs in the off-thread Modelica worker; the dispatch path
/// (`commands/compile.rs::on_compile_model`) is far enough from the
/// completion path (`worker.rs` result handler) that threading a
/// handle through both would be invasive. Derive-from-state covers
/// both paths with a single system.
#[derive(Resource, Default)]
pub struct CompileBusyHandles {
    handles: HashMap<DocumentId, lunco_workbench::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for per-doc compile lifecycle. Mints a
/// `(Document(d), "compile")` bus entry when `CompileState`
/// transitions into `Compiling`, drops it (with `Failed(msg)` if
/// the terminal state is `Error`) when it transitions out.
pub fn track_compile_busy(
    compile_states: Res<lunco_doc_bevy::DocumentDiagnostics>,
    registry: Res<crate::state::ModelicaDocumentRegistry>,
    mut handles: ResMut<CompileBusyHandles>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
) {
    use lunco_workbench::status_bus::{BusyOutcome, BusyScope, StatusBus};
    let mut still_compiling: std::collections::HashSet<DocumentId> = Default::default();
    for (doc_id, _host) in registry.iter() {
        if !compile_states.is_compiling(doc_id) {
            continue;
        }
        still_compiling.insert(doc_id);
        if handles.handles.contains_key(&doc_id) {
            continue;
        }
        let h = StatusBus::begin(
            &mut bus,
            BusyScope::Document(doc_id.0),
            "compile",
            "Compiling…",
        );
        handles.handles.insert(doc_id, h);
    }
    // Compile finished (or doc closed) — drop the handle with the
    // appropriate terminal outcome. The bus's `last_outcome` then
    // surfaces compile errors via `lifecycle()` for any panel that
    // wants them.
    let to_drop: Vec<DocumentId> = handles
        .handles
        .keys()
        .filter(|d| !still_compiling.contains(d))
        .copied()
        .collect();
    for doc_id in to_drop {
        if let Some(mut handle) = handles.handles.remove(&doc_id) {
            if let Some(msg) = compile_states.error_message(doc_id) {
                handle.set_outcome(BusyOutcome::Failed(msg.to_string()));
            }
            // Drop on scope exit clears the bus entry.
            let _ = handle;
        }
    }
}

/// `StatusBus` handle for an in-flight Fast Run.
/// [`crate::experiments_runner::ModelicaRunner`] is a process-global
/// singleton — only one run at a time — so a single `Option` is
/// sufficient. Scope is `Global` because the runner doesn't track
/// which document owns the active experiment.
#[derive(Resource, Default)]
pub struct SimulateBusyHandle {
    handle: Option<lunco_workbench::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for Fast Run lifecycle. Mints when
/// [`crate::experiments_runner::ModelicaRunner::is_busy`] rises,
/// drops when it falls.
pub fn track_simulate_busy(
    runner: Option<Res<crate::ModelicaRunnerResource>>,
    mut state: ResMut<SimulateBusyHandle>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
) {
    use lunco_workbench::status_bus::{BusyScope, StatusBus};
    let Some(runner) = runner else { return };
    let busy = runner.0.is_busy();
    match (busy, state.handle.is_some()) {
        (true, false) => {
            state.handle = Some(StatusBus::begin(
                &mut bus,
                BusyScope::Global,
                "simulate",
                "Running…",
            ));
        }
        (false, true) => {
            state.handle = None;
        }
        _ => {}
    }
}

/// Drive [`OpeningState::FileLoad`] entries: poll each pending
/// file-read task, install the resulting document into the registry,
/// and clear the entry. Mirrors the previous `cache.file_tasks`
/// drain that lived in `handle_package_loading_tasks`.
pub fn drive_file_load_openings(
    mut openings: ResMut<DocumentOpenings>,
    mut registry: ResMut<crate::state::ModelicaDocumentRegistry>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut canvas_state: ResMut<crate::ui::panels::canvas_diagram::CanvasDiagramState>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
    mut commands: Commands,
) {
    use futures_lite::future;
    let doc_ids = openings.doc_ids();
    for doc_id in doc_ids {
        let ready = match openings.get_mut(doc_id) {
            Some(OpeningState::FileLoad { task, .. }) => future::block_on(future::poll_once(task)),
            _ => None,
        };
        let Some(ready) = ready else { continue };
        let Some(OpeningState::FileLoad { busy, .. }) = openings.remove(doc_id) else {
            continue;
        };
        match ready.result {
            Ok(doc) => {
                // Success: hand the parse-phase handle to the canvas
                // state so the bus stays busy across the file-load →
                // projection boundary; the projection spawn releases
                // it via `complete_projection_handoff`.
                canvas_state.stash_projection_handoff(ready.doc_id, busy);
                registry.install_prebuilt(ready.doc_id, doc);
                workspace.active_document = Some(ready.doc_id);
            }
            Err(msg) => {
                // Failure: surface the error to the user via the
                // status bar's history popover (`bus.push`) and
                // record `Failed` on the bus entry's outcome.
                // Close every tab pre-emptively opened against
                // the reserved doc id — without this, the user
                // is left with orphan tabs pointing at a doc
                // that was never installed (registry lookups
                // return `None` and the canvas would show the
                // load-failed overlay indefinitely). The reserved
                // id itself is just a `u64` counter bump; no
                // memory leak beyond that.
                bevy::log::warn!(
                    "[DocumentOpenings] file-load failed doc={} err={msg}",
                    ready.doc_id.raw(),
                );
                bus.push(
                    "open",
                    lunco_workbench::status_bus::StatusLevel::Error,
                    msg.clone(),
                );
                let mut busy = busy;
                busy.set_outcome(lunco_workbench::status_bus::BusyOutcome::Failed(msg));
                drop(busy);
                let orphan_tab_ids: Vec<crate::model_tabs_types::TabId> = tabs
                    .iter_mut_for_doc(ready.doc_id)
                    .map(|(id, _)| id)
                    .collect();
                for tab_id in orphan_tab_ids {
                    commands.trigger(lunco_workbench::CloseTab {
                        kind: crate::ui::MODEL_VIEW_KIND,
                        instance: tab_id,
                    });
                    tabs.close_tab(tab_id);
                }
            }
        }
    }
}
