//! Document lifecycle commands — creation, opening, duplication, and closing.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::{CloseDocument, DocumentSaved};
use lunco_workbench::file_ops::{NewDocument, OpenFile};
use lunco_core::{Command, on_command};
use std::sync::Arc;

use crate::document::duplicate::{
    build_duplicate_source, collect_parent_imports, extract_class_spans_inline,
};
use crate::state::{
    CompileStates, ModelicaDocumentRegistry, WorkbenchState,
};
use crate::model_tabs::ModelTabs; use crate::model_tabs_types::MODEL_VIEW_KIND;
use crate::package_tree::PackageTreeCache;

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Request to create a new untitled Modelica model and open its tab.
///
/// Both fields default to `None` for the plain "New model" entry points
/// (File ▸ New, the package browser, the welcome screen). The URL-share
/// loader (`crate::model_share`) fires this with `source`/`name`
/// populated so a shared model reuses this exact creation + tab-open
/// path instead of duplicating it.
#[Command(default)]
pub struct CreateNewScratchModel {
    /// Initial source. `None` → a minimal `model <name> end <name>;` stub.
    pub source: Option<String>,
    /// Display name, deduplicated against existing in-memory models.
    /// `None` → the model name parsed from `source`, else an
    /// auto-incremented "Untitled".
    pub name: Option<String>,
}

/// Request to duplicate a read-only (library) model into a new
/// editable Untitled document.
#[Command(default)]
pub struct DuplicateModelFromReadOnly {
    pub source_doc: DocumentId,
}

/// API shim: duplicate the active read-only document into a fresh
/// editable workspace tab.
#[Command(default)]
pub struct DuplicateActiveDoc {
    pub doc: DocumentId,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default, bevy::reflect::Reflect)]
#[serde(tag = "kind")]
pub enum ClassAction {
    #[default]
    View,
    Duplicate {
        name: String,
    },
}

#[Command(default)]
pub struct OpenClass {
    pub qualified: String,
    #[serde(default)]
    pub action: ClassAction,
}

/// Open (or focus, if already open) an MSL class as a fresh editable
/// copy.
#[Command(default)]
pub struct OpenExample {
    pub qualified: String,
}

/// Open the same document in a new tab (split / sibling view).
#[Command(default)]
pub struct OpenInNewView {
    pub doc: DocumentId,
}

/// Unified open command — dispatches on the URI scheme.
#[Command(default)]
pub struct Open {
    pub uri: String,
}

// ─── Resources ───────────────────────────────────────────────────────────────

#[derive(Resource, Default)]
pub struct CloseDialogState {
    pub pending: Vec<(DocumentId, u64)>,
    pub requested: std::collections::HashMap<(DocumentId, u64), lunco_ui::modal::ModalId>,
}

#[derive(Resource, Default)]
pub struct PendingCloseAfterSave {
    docs: std::collections::HashMap<DocumentId, Vec<u64>>,
}

impl PendingCloseAfterSave {
    pub fn queue(&mut self, doc: DocumentId, tab: u64) {
        self.docs.entry(doc).or_default().push(tab);
    }
    pub fn take(&mut self, doc: DocumentId) -> Vec<u64> {
        self.docs.remove(&doc).unwrap_or_default()
    }
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

/// A VS-Code-style multi-tab close request raised from the model-tab
/// context menu. The anchor is the right-clicked tab; the scope picks
/// which *other* tabs go with it. Resolved by
/// [`resolve_tab_close_scopes`], which expands the scope into concrete
/// instance ids (using dock order from [`lunco_workbench::WorkbenchLayout`])
/// and feeds them through the existing [`lunco_workbench::PendingTabCloses`]
/// pipeline — so each dirty tab still gets its Save / Don't-save prompt.
#[derive(Clone, Copy, Debug)]
pub enum TabCloseScope {
    /// Close every model tab except the anchor.
    Others,
    /// Close model tabs sitting to the right of the anchor.
    Right,
    /// Close every model tab.
    All,
    /// Close model tabs with no unsaved changes (the anchor included
    /// if it too is clean).
    Saved,
}

#[derive(Resource, Default)]
pub struct PendingTabCloseScopes {
    requests: Vec<(u64, TabCloseScope)>,
}

impl PendingTabCloseScopes {
    /// Queue a multi-close anchored on `instance`.
    pub fn push(&mut self, instance: u64, scope: TabCloseScope) {
        self.requests.push((instance, scope));
    }
}

/// State for the Dymola-style app-close save flow. When the user
/// requests exit (via API `Exit`, menu, window-X), [`request_app_close`]
/// arms this and pushes every dirty doc's tab into the existing
/// [`lunco_workbench::PendingTabCloses`] queue. The per-tab Save/Don't
/// save/Cancel modal infrastructure (`render_close_dialogs`) walks one
/// prompt at a time. The [`finalize_app_close`] system polls the
/// close pipeline and fires `AppExit` once all prompts resolve cleanly,
/// or disarms when the user picks Cancel anywhere.
#[derive(Resource, Default)]
pub struct AppCloseFlow {
    pub armed: bool,
    /// Frames to wait after arming before the finalizer may fire.
    /// Bridges the gap between `request_app_close` pushing tabs into
    /// `PendingTabCloses` and `render_close_dialogs` actually
    /// enqueueing the per-tab modals (which only happens in the egui
    /// pass after the Update schedule). Without this the finalizer
    /// sees "no pending modals" before any modal exists and concludes
    /// "user cancelled" prematurely.
    pub cooldown_frames: u8,
}

/// Entry point invoked by the `Exit` command and by the window-X
/// interceptor. If no Modelica docs are dirty, fires `AppExit`
/// immediately. Otherwise, arms the close flow and pushes every dirty
/// doc's open tabs into the existing per-tab close pipeline — the
/// `render_close_dialogs` system pops one Save / Don't save / Cancel
/// modal per tab, just like Dymola.
pub fn request_app_close(world: &mut World) {
    if world.get_resource::<AppCloseFlow>().map(|f| f.armed).unwrap_or(false) {
        // Already in progress — re-clicking X just lets the existing
        // sequential prompt continue. Avoid re-queueing tabs.
        return;
    }
    // Cross-domain dirty list — `UnsavedDocs` is the shared bus every
    // domain registry pushes into (Modelica today; USD/Python/etc.
    // when they land). Reading from it instead of
    // `ModelicaDocumentRegistry` directly means future domains' dirty
    // docs are automatically picked up by the close prompt with no
    // change here.
    let dirty_tabs: Vec<(DocumentId, u64)> = {
        let Some(unsaved) = world.get_resource::<lunco_workbench::UnsavedDocs>()
        else {
            fire_app_exit(world);
            return;
        };
        let dirty_ids: Vec<DocumentId> = unsaved
            .entries
            .iter()
            .filter(|e| e.is_unsaved)
            .map(|e| e.id)
            .collect();
        if dirty_ids.is_empty() {
            fire_app_exit(world);
            return;
        }
        // Find each dirty doc's open tab(s). ModelTabs is Modelica's
        // tab table — when other domains add their own InstancePanel
        // tab tables, extend this to consult them too (or generalise
        // to a workbench-level "find tab(s) for doc id" registry).
        // Synthetic instance=0 is the fallback when no tab is open
        // for the dirty doc (registry-only edits).
        let tabs = world.get_resource::<crate::model_tabs::ModelTabs>();
        let mut out = Vec::new();
        for doc_id in dirty_ids {
            let mut any = false;
            if let Some(tabs) = tabs {
                for (tab_id, state) in tabs.iter() {
                    if state.doc == doc_id {
                        out.push((state.doc, tab_id));
                        any = true;
                    }
                }
            }
            if !any {
                out.push((doc_id, 0));
            }
        }
        out
    };
    if dirty_tabs.is_empty() {
        fire_app_exit(world);
        return;
    }
    bevy::log::info!(
        "[AppClose] {} dirty tab(s) — prompting before exit",
        dirty_tabs.len()
    );
    if let Some(mut flow) = world.get_resource_mut::<AppCloseFlow>() {
        flow.armed = true;
        flow.cooldown_frames = 4;
    }
    // Push tabs into the workbench's per-tab close queue. The existing
    // `drain_pending_tab_closes` system will detect dirty docs and
    // enqueue Save/Don't save/Cancel modals through `render_close_dialogs`.
    if let Some(mut pending) =
        world.get_resource_mut::<lunco_workbench::PendingTabCloses>()
    {
        for (_doc, tab) in dirty_tabs {
            pending.push(lunco_workbench::TabId::Instance {
                kind: MODEL_VIEW_KIND,
                instance: tab,
            });
        }
    }
}

fn fire_app_exit(world: &mut World) {
    cancel_inflight_runs(world);
    arm_shutdown_watchdog();
    if let Some(mut messages) =
        world.get_resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
    {
        bevy::log::info!("[AppClose] no dirty docs — exiting");
        messages.write(bevy::app::AppExit::Success);
    }
}

/// Hard-exit safety net. The graceful `AppExit` path waits for Bevy's
/// schedule + TaskPool to wind down; a runaway compute thread (e.g. a
/// rumoca compile that never yields) can block that join indefinitely,
/// forcing the user to SIGKILL. Once we're committed to exiting we arm a
/// detached watchdog that force-terminates the process after a short grace
/// period if the clean shutdown hasn't finished. Idempotent via `Once`, so
/// arming it from both exit commit points (no-dirty path + finalizer) is safe.
pub(crate) fn arm_shutdown_watchdog() {
    use std::sync::Once;
    static WATCHDOG: Once = Once::new();
    WATCHDOG.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("shutdown-watchdog".into())
            .spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(4));
                bevy::log::warn!(
                    "[AppClose] graceful exit stalled >4s (busy compute thread) — forcing process exit"
                );
                std::process::exit(0);
            });
    });
}

/// Best-effort: signal every in-flight experiment to cancel so worker
/// threads stop at their next solver-step / compile boundary. Speeds the
/// graceful path so the watchdog rarely has to fire; it can't interrupt a
/// thread stuck inside a rumoca compile (no cancel hook there) — that's what
/// the watchdog is for.
pub(crate) fn cancel_inflight_runs(world: &World) {
    if let Some(pending) =
        world.get_resource::<crate::experiments_runner::PendingHandles>()
    {
        if !pending.0.is_empty() {
            bevy::log::info!(
                "[AppClose] cancelling {} in-flight run(s) before exit",
                pending.0.len()
            );
            for handle in &pending.0 {
                handle.cancel();
            }
        }
    }
}

/// Intercept the window's X-button close request. Bevy's default
/// behaviour is "close immediately" when `Window::close_when_requested`
/// is true; we set that to `false` (in `bin/lunica.rs`) so this system
/// runs first and can route through the save-prompt flow.
pub fn on_window_close_requested(
    mut events: bevy::ecs::message::MessageReader<bevy::window::WindowCloseRequested>,
    mut commands: Commands,
) {
    if events.is_empty() {
        return;
    }
    // Drain — we don't need per-window info; one request triggers the
    // app-wide close flow.
    let _ = events.read().count();
    commands.queue(|world: &mut World| {
        request_app_close(world);
    });
}

/// Finalizer: when the close flow is armed and every per-tab modal
/// has resolved (either Save completed or Don't-save closed the tab),
/// fires `AppExit`. If the user picked Cancel anywhere (dirty docs
/// still exist after all modals settle), disarms and stays open.
pub fn finalize_app_close(
    flow: Option<ResMut<AppCloseFlow>>,
    close_dialogs: Option<Res<CloseDialogState>>,
    pending_save_close: Option<Res<PendingCloseAfterSave>>,
    pending_tab_closes: Option<Res<lunco_workbench::PendingTabCloses>>,
    registry: Option<Res<ModelicaDocumentRegistry>>,
    pending_runs: Option<Res<crate::experiments_runner::PendingHandles>>,
    mut exit_events: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
) {
    let Some(mut flow) = flow else { return };
    if !flow.armed {
        return;
    }
    // Cooldown: the modals don't exist yet on the frame the flow is
    // armed (request_app_close → PendingTabCloses → drained next frame
    // → render_close_dialogs enqueues modal in egui pass). Decrement
    // and skip until expired so we don't conclude "no pending modals
    // ⇒ user cancelled" before any modal renders.
    if flow.cooldown_frames > 0 {
        flow.cooldown_frames -= 1;
        return;
    }
    // Wait for the full pipeline to drain:
    //   PendingTabCloses → CloseDialogState.pending → .requested → outcome.
    let tabs_pending = pending_tab_closes
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let modals_settled = close_dialogs
        .as_ref()
        .map(|d| d.pending.is_empty() && d.requested.is_empty())
        .unwrap_or(true);
    let saves_done = pending_save_close
        .as_ref()
        .map(|p| p.is_empty())
        .unwrap_or(true);
    if tabs_pending || !modals_settled || !saves_done {
        return;
    }
    // All prompts processed. Anything still dirty means the user
    // cancelled at least one — abort the close.
    let any_dirty = registry
        .as_ref()
        .map(|r| {
            r.iter().any(|(_, host)| {
                let d = host.document();
                d.is_dirty() && !d.is_read_only()
            })
        })
        .unwrap_or(false);
    if any_dirty {
        bevy::log::info!("[AppClose] cancelled — staying open");
        flow.armed = false;
        return;
    }
    bevy::log::info!("[AppClose] all prompts resolved — exiting");
    flow.armed = false;
    if let Some(pending) = pending_runs.as_ref() {
        if !pending.0.is_empty() {
            bevy::log::info!(
                "[AppClose] cancelling {} in-flight run(s) before exit",
                pending.0.len()
            );
            for handle in &pending.0 {
                handle.cancel();
            }
        }
    }
    arm_shutdown_watchdog();
    exit_events.write(bevy::app::AppExit::Success);
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(CreateNewScratchModel)]
pub fn on_create_new_scratch_model(
    trigger: On<CreateNewScratchModel>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut cache: ResMut<PackageTreeCache>,
    mut model_tabs: ResMut<ModelTabs>,
    mut workbench: ResMut<WorkbenchState>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut commands: Commands,
) {
    let req_source = trigger.event().source.clone();
    let req_name = trigger.event().name.clone();

    let taken: std::collections::HashSet<String> = cache
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();

    // Base name: explicit request name → else the model name parsed from
    // the supplied source → else "Untitled". Then dedup with a numeric
    // suffix ("Untitled", "Untitled2", … — matching the prior scheme).
    let base = req_name
        .or_else(|| req_source.as_deref().and_then(crate::extract_model_name))
        .unwrap_or_else(|| "Untitled".to_string());
    let mut name = base.clone();
    let mut n: u32 = 2;
    while taken.contains(&name) {
        name = format!("{base}{n}");
        n += 1;
    }

    let source = req_source.unwrap_or_else(|| format!("model {name}\nend {name};\n"));
    let mem_id = format!("mem://{name}");
    let doc_id = registry.allocate_with_origin(
        source.clone(),
        DocumentOrigin::untitled(name.clone()),
    );

    cache.in_memory_models.retain(|e| e.id != mem_id);
    cache
        .in_memory_models
        .push(crate::package_tree::InMemoryEntry {
            display_name: name,
            id: mem_id,
            doc: doc_id,
        });

    let source_arc: Arc<str> = source.into();
    workbench.editor_buffer = source_arc.to_string();

    workspace.active_document = Some(doc_id);

    let tab_id = model_tabs.ensure_for(doc_id, None);
    commands.trigger(lunco_workbench::OpenTab {
        kind: MODEL_VIEW_KIND,
        instance: tab_id,
    });
}

#[on_command(DuplicateModelFromReadOnly)]
pub fn on_duplicate_model_from_read_only(
    trigger: On<DuplicateModelFromReadOnly>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut cache: ResMut<PackageTreeCache>,
    mut model_tabs: ResMut<ModelTabs>,
    mut openings: ResMut<crate::ui::document_openings::DocumentOpenings>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
    mut egui_q: Query<&mut bevy_egui::EguiContext>,
) {
    let source_doc = trigger.event().source_doc;

    let (source_full, origin_class_short, origin_fqn, inner_drill) = {
        let Some(host) = registry.host(source_doc) else {
            console.error("Duplicate failed: source doc not found in registry");
            return;
        };
        let doc = host.document();
        let fqn = model_tabs.drilled_class_for_doc(source_doc);
        let ast_opt = doc.strict_ast();
        let top_short = ast_opt
            .as_ref()
            .and_then(|ast| ast.classes.iter().next().map(|(n, _)| n.clone()))
            .or_else(|| {
                fqn.as_ref()
                    .and_then(|q| q.split('.').next().map(String::from))
            })
            .unwrap_or_else(|| doc.origin().display_name());
        
        let inner_drill: Option<String> = fqn.as_ref().and_then(|q| {
            let suffix = q.rsplit('.').next().unwrap_or("");
            (suffix != top_short).then(|| {
                let after_top = q
                    .split('.')
                    .skip_while(|seg| *seg != top_short)
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(".");
                after_top
            }).filter(|s| !s.is_empty())
        });
        (doc.source_arc(), top_short, fqn, inner_drill)
    };

    let taken: std::collections::HashSet<String> = cache
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();
    let base_name = format!("{origin_class_short}Copy");
    let mut name = base_name.clone();
    let mut n: u32 = 2;
    while taken.contains(&name) {
        name = format!("{base_name}{n}");
        n += 1;
    }

    let doc_id = registry.reserve_id();

    let mem_id = format!("mem://{name}");
    cache.in_memory_models.retain(|e| e.id != mem_id);
    cache
        .in_memory_models
        .push(crate::package_tree::InMemoryEntry {
            display_name: name.clone(),
            id: mem_id,
            doc: doc_id,
        });
    let tab_id = model_tabs.ensure_for(doc_id, None);
    if let Some(tab) = model_tabs.get_mut(tab_id) {
        tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
    }
    commands.trigger(lunco_workbench::OpenTab {
        kind: MODEL_VIEW_KIND,
        instance: tab_id,
    });

    let origin_short_for_task = origin_class_short.clone();
    let name_for_task = name.clone();
    let origin_fqn_for_task = origin_fqn;
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let class_src: &str = &*source_full;
        let imports = origin_fqn_for_task
            .as_deref()
            .and_then(crate::library_fs::resolve_class_path_indexed)
            .map(|p| collect_parent_imports(&p))
            .unwrap_or_default();
        let spans = extract_class_spans_inline(class_src, &origin_short_for_task);
        let copy_src = build_duplicate_source(
            class_src,
            spans.as_ref(),
            &name_for_task,
            origin_fqn_for_task.as_deref(),
            &imports,
        );
        crate::document::ModelicaDocument::with_origin(
            doc_id,
            copy_src,
            DocumentOrigin::untitled(name_for_task),
        )
    });

    let busy = bus.begin(
        lunco_workbench::status_bus::BusyScope::Document(doc_id.0),
        "duplicate",
        format!("Duplicating {origin_class_short} → {name}"),
    );
    openings.insert(
        doc_id,
        crate::ui::document_openings::OpeningState::Duplicate(
            crate::ui::panels::canvas_diagram::DuplicateBinding {
                display_name: name.clone(),
                origin_short: origin_class_short.clone(),
                inner_drill: inner_drill,
                task,
                busy,
            },
        ),
    );
    console.info(format!(
        "📄 Duplicating `{origin_class_short}` → `{name}` (building…)"
    ));
    for mut ctx in egui_q.iter_mut() {
        ctx.get_mut().request_repaint();
    }
}

#[on_command(DuplicateActiveDoc)]
pub fn on_duplicate_active_doc(trigger: On<DuplicateActiveDoc>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[DuplicateActiveDoc] no active document");
            return;
        };
        world.commands().trigger(DuplicateModelFromReadOnly { source_doc: doc });
    });
}

#[on_command(OpenClass)]
pub fn on_open_class(trigger: On<OpenClass>, mut commands: Commands) {
    let ev = trigger.event();
    let qualified = ev.qualified.clone();
    let action = ev.action.clone();
    commands.queue(move |world: &mut World| match action {
        ClassAction::View => {
            crate::ui::panels::canvas_diagram::drill_into_class(world, &qualified);
        }
        ClassAction::Duplicate { name } => {
            spawn_duplicate_class_task(world, qualified, name);
        }
    });
}

pub fn spawn_duplicate_class_task(world: &mut World, qualified: String, name_hint: String) {
    let origin_short = qualified
        .rsplit('.')
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| qualified.clone());

    let taken: std::collections::HashSet<String> = world
        .resource::<PackageTreeCache>()
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();
    let base_name = if name_hint.is_empty() {
        format!("{origin_short}Copy")
    } else {
        name_hint
    };
    let mut name = base_name.clone();
    let mut n: u32 = 2;
    while taken.contains(&name) {
        name = format!("{base_name}{n}");
        n += 1;
    }

    let doc_id = world
        .resource_mut::<ModelicaDocumentRegistry>()
        .reserve_id();
    let mem_id = format!("mem://{name}");
    {
        let mut cache = world
            .resource_mut::<PackageTreeCache>();
        cache.in_memory_models.retain(|e| e.id != mem_id);
        cache
            .in_memory_models
            .push(crate::package_tree::InMemoryEntry {
                display_name: name.clone(),
                id: mem_id,
                doc: doc_id,
            });
    }
    let tab_id = {
        let mut model_tabs = world
            .resource_mut::<ModelTabs>();
        let tab_id = model_tabs.ensure_for(doc_id, None);
        if let Some(tab) = model_tabs.get_mut(tab_id) {
            tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
        }
        tab_id
    };
    world.commands().trigger(lunco_workbench::OpenTab {
        kind: MODEL_VIEW_KIND,
        instance: tab_id,
    });

    let qualified_for_task = qualified.clone();
    let origin_short_for_task = origin_short.clone();
    let name_for_task = name.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let Some(path) = crate::library_fs::resolve_class_path_indexed(&qualified_for_task) else {
            return crate::document::ModelicaDocument::with_origin(
                doc_id,
                format!("// Could not locate MSL file for {qualified_for_task}\n"),
                DocumentOrigin::untitled(name_for_task),
            );
        };
        let source_full = lunco_assets::msl::msl_read(&path)
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        
        // Prefer the path-cached spans (cheap on repeat MSL duplications);
        // fall back to an inline parse if resolution fails. Either way the
        // spans are absolute in `source_full` and `build_duplicate_source`
        // slices to the class span before rewriting.
        let spans = crate::document::duplicate::extract_class_spans_via_path(
            &path,
            &source_full,
            &origin_short_for_task,
        )
        .filter(|s| s.full_start < s.full_end && s.full_end <= source_full.len())
        .or_else(|| extract_class_spans_inline(&source_full, &origin_short_for_task));
        let imports = collect_parent_imports(&path);
        let copy_src = build_duplicate_source(
            &source_full,
            spans.as_ref(),
            &name_for_task,
            Some(&qualified_for_task),
            &imports,
        );
        crate::document::ModelicaDocument::with_origin(
            doc_id,
            copy_src,
            DocumentOrigin::untitled(name_for_task),
        )
    });

    let busy = world
        .resource_mut::<lunco_workbench::status_bus::StatusBus>()
        .begin(
            lunco_workbench::status_bus::BusyScope::Document(doc_id.0),
            "duplicate",
            format!("Opening {qualified} → {name}"),
        );
    world
        .resource_mut::<crate::ui::document_openings::DocumentOpenings>()
        .insert(
            doc_id,
            crate::ui::document_openings::OpeningState::Duplicate(
                crate::ui::panels::canvas_diagram::DuplicateBinding {
                    display_name: name.clone(),
                    origin_short: origin_short,
                    inner_drill: None,
                    task,
                    busy,
                },
            ),
        );
    world
        .resource_mut::<crate::ui::panels::console::ConsoleLog>()
        .info(format!(
            "📄 Opening class `{qualified}` → editable `{name}` (building…)"
        ));
}

#[on_command(OpenExample)]
pub fn on_open_example(
    trigger: On<OpenExample>,
    mut commands: Commands,
) {
    let qualified = trigger.event().qualified.clone();
    commands.trigger(OpenClass {
        qualified,
        action: ClassAction::Duplicate { name: String::new() },
    });
}

#[on_command(OpenInNewView)]
pub fn on_open_in_new_view(trigger: On<OpenInNewView>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let drilled = world
            .get_resource::<ModelTabs>()
            .and_then(|t| t.drilled_class_for_doc(doc));
        let new_id = world
            .resource_mut::<ModelTabs>()
            .open_new(doc, drilled);
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: MODEL_VIEW_KIND,
            instance: new_id,
        });
    });
}

#[on_command(OpenFile)]
pub fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    commands.queue(move |world: &mut World| {
        // `mem://` lookups need the in-memory cache to resolve a
        // DocumentId; tree-id parser can't see it, so handle here.
        if let Some(name) = path.strip_prefix("mem://") {
            focus_in_memory_doc(world, name);
            return;
        }
        // Everything else (bundled://, file://, raw .mo path) flows
        // through the typed ClassRef + single `open_class` entry.
        if let Some(class) = crate::class_ref::ClassRef::parse_tree_id(&path) {
            crate::ui::panels::package_browser::open_class(world, class, true);
            return;
        }

        let lower = path.to_ascii_lowercase();
        let is_modelica = std::path::Path::new(&lower)
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext == "mo")
            .unwrap_or(false);
        if !is_modelica {
            return;
        }

        let path_buf = std::path::PathBuf::from(&path);

        // wasm has no filesystem: the web file picker already read the
        // chosen file's text browser-side and stashed it under its
        // name. Pull it back and feed the same result channel.
        #[cfg(target_arch = "wasm32")]
        {
            let read_result = match lunco_workbench::picker::take_picked_content(&path) {
                Some(content) => Ok(content),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no picked content for this path (wasm has no filesystem)",
                )),
            };
            let _ = open_file_result_tx().send(OpenFileResult {
                path: path_buf,
                read_result,
            });
        }

        // Native: read the file off the main thread. A 150 KB MSL
        // package file synchronously read on the input path is ~30 ms
        // of stutter; spawn on AsyncCompute and re-enter the World via
        // a one-shot channel drained on the Update tick.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path_for_task = path_buf.clone();
            let task = bevy::tasks::AsyncComputeTaskPool::get()
                .spawn(async move { std::fs::read_to_string(&path_for_task) });
            bevy::tasks::AsyncComputeTaskPool::get()
                .spawn(async move {
                    let read_result = task.await;
                    let _ = open_file_result_tx().send(OpenFileResult {
                        path: path_buf,
                        read_result,
                    });
                })
                .detach();
        }
    });
}

/// Lazily-initialised sender for the `OpenFile` read-result channel.
/// Both the native async path and the wasm picker path funnel results
/// here; [`drain_open_file_results`] consumes them on the Update tick.
fn open_file_result_tx() -> &'static std::sync::mpsc::Sender<OpenFileResult> {
    OPEN_FILE_RESULT_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<OpenFileResult>();
        let _ = OPEN_FILE_RESULT_RX.set(std::sync::Mutex::new(rx));
        tx
    })
}

struct OpenFileResult {
    path: std::path::PathBuf,
    read_result: std::io::Result<String>,
}

static OPEN_FILE_RESULT_TX: std::sync::OnceLock<std::sync::mpsc::Sender<OpenFileResult>> =
    std::sync::OnceLock::new();
static OPEN_FILE_RESULT_RX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<OpenFileResult>>> =
    std::sync::OnceLock::new();

/// Drain pending `OpenFile` reads and install them as documents.
/// Runs each tick; cheap when the queue is empty.
pub fn drain_open_file_results(world: &mut bevy::prelude::World) {
    let Some(rx_mutex) = OPEN_FILE_RESULT_RX.get() else {
        return;
    };
    let pending: Vec<OpenFileResult> = {
        let Ok(rx) = rx_mutex.lock() else {
            return;
        };
        rx.try_iter().collect()
    };
    for result in pending {
        let path = result.path;
        let source = match result.read_result {
            Ok(s) => s,
            Err(e) => {
                bevy::log::warn!("[OpenFile] {} read failed: {}", path.display(), e);
                continue;
            }
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Opened")
            .to_string();
        let mut registry =
            world.resource_mut::<ModelicaDocumentRegistry>();
        let doc_id = registry.allocate_with_origin(
            source,
            DocumentOrigin::File {
                path: path.clone(),
                writable: true,
            },
        );
        let mut tabs = world.resource_mut::<ModelTabs>();
        let tab_id = tabs.ensure_for(doc_id, None);
        if let Some(tab) = tabs.get_mut(tab_id) {
            tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
        }
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: MODEL_VIEW_KIND,
            instance: tab_id,
        });
        bevy::log::info!("[OpenFile] opened `{}` as `{}`", path.display(), stem);
    }
}

pub fn focus_in_memory_doc(world: &mut World, name: &str) {
    let target_id = format!("mem://{}", name);
    let cache = world.resource::<PackageTreeCache>();
    let entry = cache
        .in_memory_models
        .iter()
        .find(|e| e.id == target_id)
        .map(|e| e.doc);
    let Some(doc_id) = entry else {
        bevy::log::warn!(
            "[OpenFile] no Untitled doc named `{}` (mem:// requires an existing tab)",
            name
        );
        return;
    };
    let tab_id = world
        .resource_mut::<ModelTabs>()
        .ensure_for(doc_id, None);
    world.commands().trigger(lunco_workbench::OpenTab {
        kind: MODEL_VIEW_KIND,
        instance: tab_id,
    });
}

#[on_command(Open)]
pub fn on_open(trigger: On<Open>, mut commands: Commands) {
    let uri = trigger.event().uri.clone();
    if uri.is_empty() {
        bevy::log::warn!("[Open] empty uri");
        return;
    }

    if uri.contains("://") {
        commands.trigger(OpenFile { path: uri });
        return;
    }

    let looks_like_qualified_name = uri.contains('.')
        && !uri.contains('/')
        && !uri.contains('\\');
    if looks_like_qualified_name {
        commands.trigger(OpenExample { qualified: uri });
        return;
    }

    commands.trigger(OpenFile { path: uri });
}

#[on_command(CloseDocument)]
pub fn on_close_document(
    trigger: On<CloseDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    if registry.host(doc).is_none() {
        return;
    }
    // Despawn any `ModelicaModel` entity backing this doc *before*
    // dropping the document. The despawn fires `RemovedComponents`,
    // which `cleanup_removed_documents` picks up to purge the doc's
    // signal histories + plot bindings from the SignalRegistry /
    // VisualizationRegistry — otherwise stale variables (der(C2.v),
    // …) linger in the Graphs X/Y picker after the doc is closed.
    for entity in registry.entities_linked_to(doc) {
        commands.entity(entity).despawn();
    }
    registry.remove_document(doc);
}

pub fn on_document_closed_cleanup(
    trigger: On<CloseDocument>,
    mut model_tabs: ResMut<ModelTabs>,
    mut cache: ResMut<PackageTreeCache>,
    mut compile_states: ResMut<CompileStates>,
    mut workbench: ResMut<WorkbenchState>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut doc_pins: Option<ResMut<crate::ui::doc_pin::DocPinState>>,
    mut experiments: Option<ResMut<lunco_experiments::ExperimentRegistry>>,
    mut drafts: Option<ResMut<crate::experiments_runner::ExperimentDrafts>>,
) {
    let doc = trigger.event().doc;
    model_tabs.close(doc);
    cache.in_memory_models.retain(|e| e.doc != doc);
    compile_states.remove(doc);
    if workspace.active_document == Some(doc) {
        workspace.active_document = None;
        workbench.editor_buffer.clear();
    }
    if let Some(pins) = doc_pins.as_mut() {
        pins.forget(doc);
    }
    // Drop this doc's experiment history + setup drafts on close.
    // Re-opening the same file path allocates a new DocumentId, so
    // retaining records keyed by the old id would be permanent
    // leakage (no UI path back to them).
    if let Some(reg) = experiments.as_mut() {
        let twin = crate::ui::doc_pin::twin_id_for_doc(doc);
        reg.delete_for_twin(&twin);
    }
    if let Some(d) = drafts.as_mut() {
        d.forget_doc(doc);
    }
}

pub fn finish_close_after_save(
    trigger: On<DocumentSaved>,
    pending: Option<ResMut<PendingCloseAfterSave>>,
    mut commands: Commands,
) {
    let Some(mut pending) = pending else { return };
    let doc = trigger.event().doc;
    let tab_ids = pending.take(doc);
    if tab_ids.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for tab_id in tab_ids {
            world.commands().trigger(lunco_workbench::CloseTab {
                kind: MODEL_VIEW_KIND,
                instance: tab_id,
            });
            if let Some(mut tabs) = world
                .get_resource_mut::<ModelTabs>()
            {
                tabs.close_tab(tab_id);
            }
            if let Some(mut state) = world
                .get_resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
            {
                state.drop_tab(tab_id);
            }
        }
        let last_gone = world
            .resource::<ModelTabs>()
            .count_for_doc(doc)
            == 0;
        if last_gone {
            world.commands().trigger(CloseDocument { doc });
        }
    });
}

/// Expand queued [`PendingTabCloseScopes`] (Close Others / to the
/// Right / All / Saved) into concrete tab ids and hand them to
/// [`lunco_workbench::PendingTabCloses`]. Runs before
/// [`drain_pending_tab_closes`] so the expanded tabs flow through the
/// same dirty-check + Save-prompt pipeline a single × click uses.
pub fn resolve_tab_close_scopes(
    mut scopes: ResMut<PendingTabCloseScopes>,
    layout: Res<lunco_workbench::WorkbenchLayout>,
    registry: Res<ModelicaDocumentRegistry>,
    model_tabs: Res<ModelTabs>,
    mut pending: ResMut<lunco_workbench::PendingTabCloses>,
) {
    if scopes.requests.is_empty() {
        return;
    }
    // Visual left-to-right order of the model tabs, needed for the
    // "Others" / "to the Right" anchor maths.
    let ordered = layout.instances_in_order(MODEL_VIEW_KIND);
    let is_clean = |inst: u64| -> bool {
        let Some(doc) = model_tabs.get(inst).map(|s| s.doc) else {
            return true;
        };
        registry
            .host(doc)
            .map(|h| !h.document().is_dirty())
            .unwrap_or(true)
    };

    for (anchor, scope) in scopes.requests.drain(..) {
        let anchor_pos = ordered.iter().position(|&i| i == anchor);
        let targets: Vec<u64> = match scope {
            TabCloseScope::Others => {
                ordered.iter().copied().filter(|&i| i != anchor).collect()
            }
            TabCloseScope::Right => match anchor_pos {
                Some(p) => ordered[p + 1..].to_vec(),
                None => Vec::new(),
            },
            TabCloseScope::All => ordered.clone(),
            TabCloseScope::Saved => {
                ordered.iter().copied().filter(|&i| is_clean(i)).collect()
            }
        };
        for instance in targets {
            pending.push(lunco_workbench::TabId::Instance {
                kind: MODEL_VIEW_KIND,
                instance,
            });
        }
    }
}

pub fn drain_pending_tab_closes(
    mut pending: ResMut<lunco_workbench::PendingTabCloses>,
    registry: Res<ModelicaDocumentRegistry>,
    mut model_tabs: ResMut<ModelTabs>,
    mut dialogs: ResMut<CloseDialogState>,
    mut commands: Commands,
) {
    for tab in pending.drain() {
        let lunco_workbench::TabId::Instance { kind, instance } = tab else {
            continue;
        };
        if kind == lunco_viz::VIZ_PANEL_KIND {
            commands.trigger(lunco_workbench::CloseTab { kind, instance });
            commands.queue(move |world: &mut World| {
                if let Some(mut reg) =
                    world.get_resource_mut::<lunco_viz::VisualizationRegistry>()
                {
                    reg.remove(lunco_viz::viz::VizId(instance));
                }
            });
            continue;
        }
        if kind != MODEL_VIEW_KIND {
            continue;
        }
        let Some(doc) = model_tabs.get(instance).map(|s| s.doc) else {
            commands.trigger(lunco_workbench::CloseTab { kind, instance });
            continue;
        };
        let (is_dirty, is_read_only) = registry
            .host(doc)
            .map(|h| {
                let d = h.document();
                (d.is_dirty(), d.is_read_only())
            })
            .unwrap_or((false, false));
        if is_dirty && !is_read_only {
            if !dialogs.pending.iter().any(|(d, t)| *d == doc && *t == instance) {
                dialogs.pending.push((doc, instance));
            }
        } else {
            commands.trigger(lunco_workbench::CloseTab { kind, instance });
            model_tabs.close_tab(instance);
            commands.queue(move |world: &mut World| {
                if let Some(mut state) = world
                    .get_resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
                {
                    state.drop_tab(instance);
                }
            });
            if model_tabs.count_for_doc(doc) == 0 {
                commands.trigger(CloseDocument { doc });
            }
        }
    }
}

const SAVE_LABEL: &str = "Save";
const DONT_SAVE_LABEL: &str = "Don't save";
const CANCEL_LABEL: &str = "Cancel";

pub fn render_close_dialogs(
    registry: Res<ModelicaDocumentRegistry>,
    mut dialogs: ResMut<CloseDialogState>,
    mut modals: ResMut<lunco_ui::modal::ModalQueue>,
    mut pending_save_close: Option<ResMut<PendingCloseAfterSave>>,
    mut commands: Commands,
) {
    use lunco_ui::modal::{ModalBody, ModalButton, ModalOutcome, ModalRequest};

    let pending = std::mem::take(&mut dialogs.pending);
    let mut survivors = Vec::with_capacity(pending.len());
    for (doc, originating_tab) in pending {
        let Some(host) = registry.host(doc) else {
            dialogs.requested.remove(&(doc, originating_tab));
            continue;
        };

        enum DialogAction {
            None,
            Save,
            DontSave,
            Cancel,
        }

        let key = (doc, originating_tab);
        let modal_id = match dialogs.requested.get(&key).copied() {
            Some(id) => id,
            None => {
                let document = host.document();
                let display_name = document.origin().display_name().to_string();
                let is_untitled = document.origin().is_untitled();
                let is_read_only = document.is_read_only();
                let can_save = !is_read_only;

                let body_text = if is_untitled {
                    "Your changes will be lost if you don't save them.\n\n\
                     This model has never been saved — picking Save will \
                     open a Save-As dialog to bind it to a file."
                        .to_string()
                } else if is_read_only {
                    "Your changes will be lost if you don't save them.\n\n\
                     This is a read-only library class; Save is unavailable. \
                     Use Duplicate to Workspace if you want to keep your edits."
                        .to_string()
                } else {
                    "Your changes will be lost if you don't save them.".to_string()
                };

                let mut buttons = Vec::new();
                if can_save {
                    buttons.push(ModalButton::Confirm(SAVE_LABEL.into()));
                }
                buttons.push(ModalButton::Destructive(DONT_SAVE_LABEL.into()));
                buttons.push(ModalButton::Cancel(CANCEL_LABEL.into()));

                let id = modals.request(ModalRequest {
                    title: format!("Save changes to '{display_name}'?"),
                    body: ModalBody::Custom(Arc::new(move |ui| {
                        ui.label(egui::RichText::new(&body_text).size(12.0));
                    })),
                    buttons,
                    dismiss_on_esc: true,
                });
                dialogs.requested.insert(key, id);
                survivors.push((doc, originating_tab));
                continue;
            }
        };

        let action = match modals.poll(modal_id) {
            None => DialogAction::None,
            Some(ModalOutcome::Confirmed(label)) if label == SAVE_LABEL => DialogAction::Save,
            Some(ModalOutcome::Destructive(label)) if label == DONT_SAVE_LABEL => {
                DialogAction::DontSave
            }
            Some(_) => DialogAction::Cancel,
        };

        if !matches!(action, DialogAction::None) {
            dialogs.requested.remove(&key);
        }
        match action {
            DialogAction::None => {
                survivors.push((doc, originating_tab));
            }
            DialogAction::Save => {
                if let Some(q) = pending_save_close.as_mut() {
                    q.queue(doc, originating_tab);
                }
                commands.trigger(lunco_doc_bevy::SaveDocument { doc });
            }
            DialogAction::DontSave => {
                let tab = originating_tab;
                commands.queue(move |world: &mut World| {
                    world.commands().trigger(lunco_workbench::CloseTab {
                        kind: MODEL_VIEW_KIND,
                        instance: tab,
                    });
                    if let Some(mut tabs) = world
                        .get_resource_mut::<ModelTabs>()
                    {
                        tabs.close_tab(tab);
                    }
                    if let Some(mut state) = world
                        .get_resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
                    {
                        state.drop_tab(tab);
                    }
                    let last_gone = world
                        .resource::<ModelTabs>()
                        .count_for_doc(doc)
                        == 0;
                    if last_gone {
                        world.commands().trigger(CloseDocument { doc });
                    }
                });
            }
            DialogAction::Cancel => { }
        }
    }
    let alive: std::collections::HashSet<(DocumentId, u64)> =
        survivors.iter().copied().collect();
    let stale: Vec<((DocumentId, u64), lunco_ui::modal::ModalId)> = dialogs
        .requested
        .iter()
        .filter(|(k, _)| !alive.contains(k))
        .map(|(k, id)| (*k, *id))
        .collect();
    for (key, id) in stale {
        modals.cancel(id);
        dialogs.requested.remove(&key);
    }
    dialogs.pending = survivors;
}

#[on_command(NewDocument)]
pub fn on_new_modelica_document(trigger: On<lunco_workbench::file_ops::NewDocument>, mut commands: Commands) {
    if trigger.event().kind != "modelica" {
        return;
    }
    commands.trigger(CreateNewScratchModel::default());
}

#[Command(default)]
pub struct GetFile {
    pub path: String,
}

#[on_command(GetFile)]
pub fn on_get_file(trigger: On<GetFile>) {
    let path = trigger.event().path.clone();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            bevy::log::info!(
                "[GetFile] {} ({} bytes) -- BEGIN --\n{}\n-- END --",
                path,
                content.len(),
                content,
            );
        }
        Err(e) => {
            bevy::log::warn!("[GetFile] {} read failed: {}", path, e);
        }
    }
}

pub fn prewarm_msl_library() {
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async {
            let t0 = web_time::Instant::now();
            let n = crate::visual_diagram::msl_class_library().len();
            bevy::log::info!(
                "[MSL] prewarmed component library: {n} entries in {:?}",
                t0.elapsed()
            );
        })
        .detach();
}
