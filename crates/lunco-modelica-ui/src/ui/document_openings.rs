//! Per-document container for in-flight parse tasks.
//!
//! Holds one [`OpeningState`] per [`DocumentId`] until the parse
//! resolves and the driver hands the document to
//! [`crate::ui::document_context::ModelicaDocuments`]. Each variant
//! owns its own typed `Task<...>` plus a [`lunco_status_core::status_bus::BusyHandle`]
//! that keeps a `(BusyScope::Document, "opening"|"drill-in"|"duplicate")`
//! entry on the bus for the parse lifetime.
//!
//! **This is not the loading-state authority.** UI panels query the
//! [`lunco_status_core::status_bus::StatusBus`] directly
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

/// File-open facts captured before dispatch, without filesystem identity reads.
/// Canonical worker results use these pins to distinguish an explicit resident
/// reload from source that another request installed after admission.
#[derive(Clone, Debug)]
pub struct FileOpenAdmission {
    pub(crate) file: lunco_workspace::FileDocumentAdmission,
    residents: Vec<ResidentFileSource>,
}

#[derive(Clone, Debug)]
struct ResidentFileSource {
    document: DocumentId,
    path: std::path::PathBuf,
    canonical: Option<std::path::PathBuf>,
    runtime: Option<lunco_workspace::DocumentRuntimeOwner>,
    generation: u64,
}

impl FileOpenAdmission {
    pub(crate) fn capture(
        registry: &crate::ui::document_context::ModelicaDocuments,
        workspace: Option<&lunco_workspace::Workspace>,
        replication: Option<&lunco_workspace::ReplicationOwner>,
    ) -> Self {
        let residents = registry
            .iter()
            .filter_map(|(document, host)| {
                let source = host.document();
                Some(ResidentFileSource {
                    document,
                    path: source.origin().canonical_path()?.to_path_buf(),
                    canonical: None,
                    runtime: workspace
                        .and_then(|workspace| workspace.document(document))
                        .map(|entry| entry.runtime_context.clone()),
                    generation: source.generation_owned(),
                })
            })
            .collect();
        Self {
            file: lunco_workspace::FileDocumentAdmission::capture(workspace, replication),
            residents,
        }
    }

    /// Resolve captured aliases on the same file-loading worker. A vanished
    /// unrelated resident cannot match the successfully read source identity.
    pub(crate) fn resolve_residents(&mut self) {
        for resident in &mut self.residents {
            resident.canonical = lunco_storage::canonicalize_file_path(&resident.path).ok();
        }
    }

    pub(crate) fn resident_for_path(&self, path: &std::path::Path) -> Option<DocumentId> {
        self.residents
            .iter()
            .find(|resident| resident.canonical.as_deref() == Some(path))
            .map(|resident| resident.document)
    }

    pub(crate) fn validate_resident(
        &self,
        document: DocumentId,
        path: Option<&std::path::Path>,
        runtime: Option<&lunco_workspace::DocumentRuntimeOwner>,
        generation: u64,
        dirty: bool,
        incoming: &lunco_workspace::DocumentRuntimeOwner,
    ) -> Result<(), String> {
        let admitted = self
            .residents
            .iter()
            .find(|resident| resident.document == document);
        if let Some(admitted) = admitted {
            if Some(admitted.path.as_path()) != path
                || admitted.runtime.as_ref() != runtime
                || admitted.generation != generation
            {
                return Err("resident source changed after file-open admission".into());
            }
        } else if runtime != Some(incoming) {
            return Err(
                "different-owner resident source was installed after file-open admission".into(),
            );
        }
        if dirty && runtime != Some(incoming) {
            return Err("dirty resident source belongs to a different runtime owner".into());
        }
        Ok(())
    }
}

/// One in-flight document open. Each variant carries the typed
/// `Task<...>` plus the metadata that variant's driver needs to
/// finish the install (drilled-class name, display name, busy
/// handle for the status bus, etc.).
pub enum OpeningState {
    /// Bundled or user-file read driven by the Package Browser. The
    /// Task returns a fully-built [`FileLoadResult`] and resolved resident
    /// source pins for user files; bundled sources have Application lifetime.
    FileLoad {
        display_name: String,
        task: Task<(FileLoadResult, Option<FileOpenAdmission>)>,
        /// RAII guard registered with [`lunco_status_core::status_bus::StatusBus`]
        /// at insert time. Same role as [`DrillInBinding::busy`] and
        /// [`DuplicateBinding::busy`]: keeps a `(Document(doc_id),
        /// "opening")` entry on the bus from "user clicked open" until
        /// the file-load driver hands it off to the projection stage
        /// via [`crate::ui::panels::canvas_diagram::CanvasDiagramState::stash_projection_handoff`].
        busy: lunco_status_core::status_bus::BusyHandle,
    },
    /// source library drill-in slim-slice load. Built by
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
/// [`lunco_status_core::status_bus::StatusBus`], not here.
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

    /// End an admitted document task at its owning lifecycle boundary.
    pub fn cancel(&mut self, doc: DocumentId) -> bool {
        let Some(mut opening) = self.remove(doc) else {
            return false;
        };
        let busy = match &mut opening {
            OpeningState::FileLoad { busy, .. } => busy,
            OpeningState::DrillIn(binding) => &mut binding.busy,
            OpeningState::Duplicate(binding) => &mut binding.busy,
        };
        busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Cancelled);
        true
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
    handles: HashMap<DocumentId, lunco_status_core::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for AST reparse state. Mints a `StatusBus`
/// handle when `ast_is_stale()` flips from false → true and drops it
/// when it flips back. Lets the canvas overlay rely on
/// `bus.lifecycle(Document(d), ...)` alone without an ast-stale
/// fallback predicate.
pub fn track_ast_reparse_busy(
    registry: Res<crate::ui::document_context::ModelicaDocuments>,
    mut handles: ResMut<AstReparseBusyHandles>,
    mut bus: ResMut<lunco_status_core::status_bus::StatusBus>,
) {
    use lunco_status_core::status_bus::{BusyScope, StatusBus};
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
/// `Failed(msg)`) recorded for [`lunco_status_core::status_bus::StatusBus::lifecycle`]
/// consumers.
///
/// Compile runs in the off-thread Modelica worker; the dispatch path
/// (`commands/compile.rs::on_compile_model`) is far enough from the
/// completion path (`worker.rs` result handler) that threading a
/// handle through both would be invasive. Derive-from-state covers
/// both paths with a single system.
#[derive(Resource, Default)]
pub struct CompileBusyHandles {
    handles: HashMap<DocumentId, lunco_status_core::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for per-doc compile lifecycle. Mints a
/// `(Document(d), "compile")` bus entry when `CompileState`
/// transitions into `Compiling`, drops it (with `Failed(msg)` if
/// the terminal state is `Error`) when it transitions out.
pub fn track_compile_busy(
    compile_states: Res<lunco_doc_bevy::DocumentDiagnostics>,
    registry: Res<crate::ui::document_context::ModelicaDocuments>,
    mut handles: ResMut<CompileBusyHandles>,
    mut bus: ResMut<lunco_status_core::status_bus::StatusBus>,
) {
    use lunco_status_core::status_bus::{BusyOutcome, BusyScope, StatusBus};
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
/// [`lunco_modelica_runner::ModelicaRunner`] is a process-global
/// singleton — only one run at a time — so a single `Option` is
/// sufficient. Scope is `Global` because the runner doesn't track
/// which document owns the active experiment.
#[derive(Resource, Default)]
pub struct SimulateBusyHandle {
    handle: Option<lunco_status_core::status_bus::BusyHandle>,
}

/// Edge-triggered tracker for Fast Run lifecycle. Mints when
/// [`lunco_modelica_runner::ModelicaRunner::is_busy`] rises,
/// drops when it falls.
pub fn track_simulate_busy(
    runner: Option<Res<lunco_modelica_runner::ModelicaRunnerResource>>,
    mut state: ResMut<SimulateBusyHandle>,
    mut bus: ResMut<lunco_status_core::status_bus::StatusBus>,
) {
    use lunco_status_core::status_bus::{BusyScope, StatusBus};
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
/// and clear the entry after validating its admitted runtime lifetime.
pub fn drive_file_load_openings(
    mut openings: ResMut<DocumentOpenings>,
    mut registry: ResMut<crate::ui::document_context::ModelicaDocuments>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut canvas_state: ResMut<crate::ui::panels::canvas_diagram::CanvasDiagramState>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut bus: ResMut<lunco_status_core::status_bus::StatusBus>,
    mut commands: Commands,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    use bevy::tasks::futures_lite::future;
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let doc_ids = openings.doc_ids();
    for doc_id in doc_ids {
        let ready = match openings.get_mut(doc_id) {
            Some(OpeningState::FileLoad { task, .. }) => future::block_on(future::poll_once(task)),
            _ => None,
        };
        let Some((ready, admission)) = ready else {
            continue;
        };
        let Some(OpeningState::FileLoad { busy, .. }) = openings.remove(doc_id) else {
            continue;
        };
        match ready.result {
            Ok((doc, runtime)) => {
                if !runtime.is_current(Some(&workspace.0), replication.as_ref()) {
                    let message = format!(
                        "File open {} cancelled: its admitted runtime owner retired",
                        ready.doc_id
                    );
                    bevy::log::warn!("[ModelicaOpen] {message}");
                    bus.push(
                        "open",
                        lunco_status_core::status_bus::StatusLevel::Warn,
                        message,
                    );
                    let mut busy = busy;
                    busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Cancelled);
                    if registry.host(ready.doc_id).is_none() {
                        close_pending_file_tabs(ready.doc_id, &mut tabs, &mut commands);
                    }
                    continue;
                }
                let origin = doc.origin().clone();
                // Worker outcomes carry canonical file paths, so another pending
                // open of the same file can reuse its newly installed identity.
                let actual_id = origin
                    .canonical_path()
                    .and_then(|path| {
                        registry.ids().find(|id| {
                            registry.host(*id).is_some_and(|host| {
                                host.document().origin().canonical_path() == Some(path)
                            })
                        })
                    })
                    .or_else(|| {
                        origin.canonical_path().and_then(|path| {
                            admission
                                .as_ref()
                                .and_then(|admission| admission.resident_for_path(path))
                                .filter(|id| registry.host(*id).is_some())
                        })
                    })
                    .unwrap_or(ready.doc_id);
                if let Some(host) = registry.host(actual_id) {
                    let resident_owner = workspace
                        .document(actual_id)
                        .map(|entry| workspace.runtime_owner_for(entry));
                    let conflict = admission.as_ref().and_then(|admission| {
                        admission
                            .validate_resident(
                                actual_id,
                                host.document().origin().canonical_path(),
                                resident_owner.as_ref(),
                                host.document().generation_owned(),
                                host.document().is_dirty(),
                                &runtime,
                            )
                            .err()
                    });
                    if let Some(reason) = conflict {
                        let message = format!("File reopen {actual_id} refused: {reason}");
                        bevy::log::warn!("[ModelicaOpen] {message}");
                        bus.push(
                            "open",
                            lunco_status_core::status_bus::StatusLevel::Error,
                            message.clone(),
                        );
                        let mut busy = busy;
                        busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(
                            message,
                        ));
                        if registry.host(ready.doc_id).is_none() {
                            close_pending_file_tabs(ready.doc_id, &mut tabs, &mut commands);
                        }
                        continue;
                    }
                }
                if let Some(host) = registry.host_mut(actual_id) {
                    if host.document().is_dirty() {
                        let message = format!(
                            "Document {actual_id} has unsaved edits; file reload kept its resident source"
                        );
                        bevy::log::warn!("[ModelicaOpen] {message}");
                        bus.push(
                            "open",
                            lunco_status_core::status_bus::StatusLevel::Warn,
                            message,
                        );
                    } else {
                        lunco_doc::FileBacked::reload_base(host.document_mut(), doc.source());
                        host.document_mut().set_origin(origin.clone());
                        registry.mark_changed(actual_id);
                    }
                } else if let Err(error) = registry.install_prebuilt(actual_id, doc) {
                    let message = format!("Failed to install file document {actual_id}: {error}");
                    bevy::log::warn!("[ModelicaOpen] {message}");
                    bus.push(
                        "open",
                        lunco_status_core::status_bus::StatusLevel::Error,
                        message.clone(),
                    );
                    let mut busy = busy;
                    busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(message));
                    if registry.host(ready.doc_id).is_none() {
                        close_pending_file_tabs(ready.doc_id, &mut tabs, &mut commands);
                    }
                    continue;
                }
                lunco_modelica_core::doc_ops::register_document_context(
                    &mut workspace,
                    actual_id,
                    origin,
                    runtime,
                    registry
                        .host(actual_id)
                        .is_some_and(|host| host.document().is_dirty()),
                );
                for (_, tab) in tabs.iter_mut_for_doc(ready.doc_id) {
                    tab.doc = actual_id;
                }
                // Success: hand the parse-phase handle to the canvas
                // state so the bus stays busy across the file-load →
                // projection boundary; the projection spawn releases
                // it via `complete_projection_handoff`.
                canvas_state.stash_projection_handoff(actual_id, busy);
                workspace.active_document = Some(actual_id);
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
                    lunco_status_core::status_bus::StatusLevel::Error,
                    msg.clone(),
                );
                let mut busy = busy;
                busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(msg));
                drop(busy);
                if registry.host(ready.doc_id).is_none() {
                    close_pending_file_tabs(ready.doc_id, &mut tabs, &mut commands);
                }
            }
        }
    }
}

fn close_pending_file_tabs(
    document: DocumentId,
    tabs: &mut crate::model_tabs::ModelTabs,
    commands: &mut Commands,
) {
    let tab_ids: Vec<_> = tabs.iter_mut_for_doc(document).map(|(id, _)| id).collect();
    for tab_id in tab_ids {
        commands.trigger(lunco_workbench_core::commands::CloseTab {
            kind: crate::ui::MODEL_VIEW_KIND,
            instance: tab_id,
        });
        tabs.close_tab(tab_id);
    }
}

#[cfg(test)]
mod file_open_admission_tests {
    use super::*;
    use lunco_workspace::{DocumentRuntimeOwner, TwinId};

    #[test]
    fn canonical_file_reopen_keeps_admitted_source_fences() {
        let old = DocumentRuntimeOwner::LocalTwin(TwinId::new(1));
        let incoming = DocumentRuntimeOwner::LocalTwin(TwinId::new(2));
        let id = DocumentId::new(1);
        let stored = std::path::PathBuf::from("alias/model.mo");
        let canonical = std::path::PathBuf::from("canonical/model.mo");
        let admission = FileOpenAdmission {
            file: lunco_workspace::FileDocumentAdmission::capture(None, None),
            residents: vec![ResidentFileSource {
                document: id,
                path: stored.clone(),
                canonical: Some(canonical.clone()),
                runtime: Some(old.clone()),
                generation: 7,
            }],
        };
        assert_eq!(admission.resident_for_path(&canonical), Some(id));
        assert!(
            admission
                .validate_resident(id, Some(&stored), Some(&old), 7, false, &incoming)
                .is_ok()
        );
        assert!(
            admission
                .validate_resident(id, Some(&stored), Some(&old), 7, true, &incoming)
                .unwrap_err()
                .contains("dirty resident")
        );
        assert!(
            admission
                .validate_resident(id, Some(&stored), Some(&old), 8, false, &incoming)
                .unwrap_err()
                .contains("changed after")
        );
        assert!(
            admission
                .validate_resident(id, Some(&stored), Some(&incoming), 7, false, &incoming)
                .unwrap_err()
                .contains("changed after")
        );
        assert!(
            admission
                .validate_resident(id, None, Some(&old), 7, false, &incoming)
                .unwrap_err()
                .contains("changed after")
        );
        assert!(
            admission
                .validate_resident(
                    DocumentId::new(2),
                    Some(&canonical),
                    Some(&old),
                    0,
                    false,
                    &incoming
                )
                .unwrap_err()
                .contains("installed after")
        );
        assert!(
            admission
                .validate_resident(
                    DocumentId::new(2),
                    Some(&canonical),
                    Some(&incoming),
                    0,
                    false,
                    &incoming
                )
                .is_ok()
        );
    }
}
