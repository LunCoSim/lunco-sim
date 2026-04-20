//! Package Browser — Dymola-style library tree.
//!
//! Scans the real MSL directory from disk (via `lunco_assets::msl_dir()`).
//! Bundled models are included as read-only entries.
//! Clicking any `.mo` file opens it in the Code Editor + Diagram panels.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::models::BUNDLED_MODELS;
use crate::ui::state::{ModelicaDocumentRegistry, ModelLibrary, OpenModel, WorkbenchState};

use bevy::tasks::{AsyncComputeTaskPool, Task};
use futures_lite::future;
use lunco_doc::DocumentId;

// ---------------------------------------------------------------------------
// Tree Nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PackageNode {
    Category {
        id: String,
        name: String,
        /// Modelica dot-path (e.g. "Modelica.Electrical.Analog")
        package_path: String,
        /// Real filesystem path
        fs_path: std::path::PathBuf,
        /// None means not yet scanned. Some(vec![]) means scanned and empty.
        children: Option<Vec<PackageNode>>,
        /// Whether a background scan is currently in progress.
        is_loading: bool,
    },
    Model {
        id: String,
        name: String,
        library: ModelLibrary,
    },
}

impl PackageNode {
    pub fn name(&self) -> &str {
        match self {
            PackageNode::Category { name, .. } | PackageNode::Model { name, .. } => name,
        }
    }
}

// ---------------------------------------------------------------------------
// Cached Tree
// ---------------------------------------------------------------------------

pub struct ScanResult {
    pub parent_id: String,
    pub children: Vec<PackageNode>,
}

pub struct FileLoadResult {
    pub id: String,
    pub name: String,
    pub library: ModelLibrary,
    pub source: std::sync::Arc<str>,
    pub line_starts: std::sync::Arc<[usize]>,
    pub detected_name: Option<String>,
    pub layout_job: Option<bevy_egui::egui::text::LayoutJob>,
}

/// Tracks one in-memory ("scratch") model the user has created this
/// session. The document itself lives in [`ModelicaDocumentRegistry`];
/// this is the Package Browser's view of it (display name + id).
#[derive(Debug, Clone)]
pub struct InMemoryEntry {
    /// Human-readable name (matches the `model <name>` declaration).
    pub display_name: String,
    /// The `mem://<name>` id used as a stable `OpenModel.model_path`.
    pub id: String,
    /// DocumentId in the registry — source of truth for the model's text.
    /// Kept for direct lookups (close-entry, duplicate, etc.); the
    /// re-open path currently resolves via `find_by_path(id)` and
    /// doesn't strictly need this field.
    #[allow(dead_code)]
    pub doc: DocumentId,
}

#[derive(Resource)]
pub struct PackageTreeCache {
    pub roots: Vec<PackageNode>,
    /// Active scanning tasks.
    pub tasks: Vec<Task<ScanResult>>,
    /// Active file loading tasks.
    pub file_tasks: Vec<Task<FileLoadResult>>,
    /// In-memory models created via "New Model…" this session. Listed
    /// under "Your Models" so the user can click back into one after
    /// they've navigated away.
    pub in_memory_models: Vec<InMemoryEntry>,
    /// Currently-open Twin folder (if any) + its scanned file tree.
    /// Populated by the "Open Folder" button, cleared by "Close Twin".
    pub twin: Option<TwinState>,
    /// In-flight async scan of a just-picked folder. Polled by
    /// `handle_package_loading_tasks`. While set, the Twin section
    /// shows a spinner so the UI never freezes.
    pub twin_scan_task: Option<Task<TwinState>>,
    /// Path currently being renamed (if any) + its edit buffer.
    pub rename: RenameState,
}

/// User's Twin workspace — a folder on disk being browsed as a tree.
///
/// Read-only in this first pass: scanning + open-on-click. Edits
/// (new/rename/delete, drag-move) land in the next phase.
#[derive(Clone)]
pub struct TwinState {
    /// Root folder the user picked via Open Folder.
    pub root: std::path::PathBuf,
    /// Recursive tree of files + subfolders under `root`.
    pub root_node: TwinNode,
}

/// Transient rename state — which path is in rename mode + the
/// buffer the user is typing into. Lives on the cache so
/// render-state survives frame boundaries.
#[derive(Default, Clone)]
pub struct RenameState {
    /// The tree entry the user invoked Rename on.
    pub target: Option<std::path::PathBuf>,
    /// Current buffer (defaults to the original name on entry).
    pub buffer: String,
    /// When Some, the inline TextEdit should steal focus this frame
    /// (first frame of the rename — so the user can immediately type).
    pub needs_focus: bool,
}

/// One file or folder inside a Twin. Tree-shaped so `CollapsingHeader`
/// renders it cleanly (one level of nesting per depth step).
#[derive(Clone)]
pub struct TwinNode {
    /// Absolute path on disk.
    pub path: std::path::PathBuf,
    /// Display name — just the file/folder name.
    pub name: String,
    /// Directory nodes have `children`; file nodes have an empty vec.
    pub children: Vec<TwinNode>,
    /// True for `.mo` files (clickable, opens a tab). Other files
    /// are rendered greyed out / non-clickable so users see the
    /// structure but don't accidentally try to open non-Modelica docs.
    pub is_modelica: bool,
}

impl TwinNode {
    fn is_dir(&self) -> bool {
        // An empty file looks like an empty dir; distinguish by
        // file-system check on the path.
        !self.children.is_empty() || self.path.is_dir()
    }
}

impl PackageTreeCache {
    pub fn new() -> Self {
        let msl_root = lunco_assets::msl_dir();
        let modelica_dir = msl_root.join("Modelica");

        let mut roots = Vec::new();

        roots.push(PackageNode::Category {
            id: "msl_root".into(),
            name: "📚 Modelica Standard Library".into(),
            package_path: "Modelica".into(),
            fs_path: modelica_dir,
            children: None, // Will be loaded lazily
            is_loading: false,
        });

        roots.push(PackageNode::Category {
            id: "bundled_root".into(),
            name: "📦 Bundled Models".into(),
            package_path: "Bundled".into(),
            fs_path: std::path::PathBuf::new(),
            children: Some(build_bundled_tree()),
            is_loading: false,
        });

        roots.push(PackageNode::Category {
            id: "folder_root".into(),
            name: "📁 Open Folder".into(),
            package_path: "User".into(),
            fs_path: std::path::PathBuf::new(),
            children: Some(vec![PackageNode::Category {
                id: "folder_empty".into(),
                name: "(no folder open)".into(),
                package_path: "User.Empty".into(),
                fs_path: std::path::PathBuf::new(),
                children: Some(vec![]),
                is_loading: false,
            }]),
            is_loading: false,
        });

        Self {
            roots,
            tasks: Vec::new(),
            file_tasks: Vec::new(),
            in_memory_models: Vec::new(),
            twin: None,
            twin_scan_task: None,
            rename: RenameState::default(),
        }
    }
}

/// Recursively scan `root` into a [`TwinNode`] tree.
/// Skips hidden dirs, `.git`, common build / dependency caches.
/// Synchronous — callers run this on a background task so the UI
/// never blocks.
pub fn scan_twin_folder(root: std::path::PathBuf) -> TwinState {
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let root_node = TwinNode {
        children: scan_children(&root),
        path: root.clone(),
        name,
        is_modelica: false,
    };
    TwinState { root, root_node }
}

fn scan_children(dir: &std::path::Path) -> Vec<TwinNode> {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in iter.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if should_skip(&name) {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let is_modelica = !is_dir
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false);
        let children = if is_dir { scan_children(&path) } else { Vec::new() };
        out.push(TwinNode {
            path,
            name,
            children,
            is_modelica,
        });
    }
    // Directories first, then files, each alphabetically. Standard
    // explorer ordering — nothing sadder than a file tree with files
    // interleaved with folders in creation order.
    out.sort_by(|a, b| {
        let a_dir = !a.children.is_empty() || a.path.is_dir();
        let b_dir = !b.children.is_empty() || b.path.is_dir();
        b_dir.cmp(&a_dir).then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn should_skip(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target" | "shared_target" | "node_modules" | "__pycache__"
        )
}

// ---------------------------------------------------------------------------
// MSL Tree Builder — scans real .mo files from disk
// ---------------------------------------------------------------------------

fn scan_msl_dir(dir: &std::path::Path, package_path: String) -> Vec<PackageNode> {
    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if name.starts_with('.') || name == "__MACOSX" { continue; }
                let sub_path = format!("{}.{}", package_path, name);
                let id = format!("msl_{}", sub_path.replace('.', "_").replace('/', "_"));
                results.push(PackageNode::Category {
                    id,
                    name,
                    package_path: sub_path,
                    fs_path: path,
                    children: None, // Lazy load
                    is_loading: false,
                });
            } else if path.extension().map(|e| e == "mo").unwrap_or(false) {
                let display_name = name.strip_suffix(".mo").unwrap_or(&name).to_string();
                let model_path = format!("{}.{}", package_path, name);
                let id = format!("msl_path:{}", model_path.strip_prefix("Modelica.").unwrap_or(&model_path));
                results.push(PackageNode::Model {
                    id,
                    name: display_name,
                    library: ModelLibrary::MSL,
                });
            }
        }
    }

    results.sort_by_key(|n| n.name().to_lowercase());
    results
}

fn build_bundled_tree() -> Vec<PackageNode> {
    // Use the bundled:// URL scheme as the id so open_model can find it.
    BUNDLED_MODELS
        .iter()
        .map(|m| PackageNode::Model {
            id: format!("bundled://{}", m.filename),
            name: m
                .filename
                .strip_suffix(".mo")
                .unwrap_or(m.filename)
                .to_string(),
            library: ModelLibrary::Bundled,
        })
        .collect()
}

fn find_and_update_node(nodes: &mut [PackageNode], parent_id: &str, children: Vec<PackageNode>) -> bool {
    for node in nodes {
        match node {
            PackageNode::Category { id, children: node_children, is_loading, .. } => {
                if id == parent_id {
                    *node_children = Some(children);
                    *is_loading = false;
                    return true;
                }
                if let Some(ref mut sub_children) = node_children {
                    if find_and_update_node(sub_children, parent_id, children.clone()) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// System that checks for finished scanning tasks and updates the cache.
pub fn handle_package_loading_tasks(
    mut cache: ResMut<PackageTreeCache>,
    mut workbench: ResMut<WorkbenchState>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut model_tabs: ResMut<crate::ui::panels::model_view::ModelTabs>,
    mut layout: ResMut<lunco_workbench::WorkbenchLayout>,
    mut egui_ctx: bevy_egui::EguiContexts,
    mut pending_drill_ins: ResMut<crate::ui::browser_dispatch::PendingDrillIns>,
    mut drilled_in: ResMut<crate::ui::panels::canvas_diagram::DrilledInClassNames>,
) {
    let mut finished_results = Vec::new();

    cache.tasks.retain_mut(|task| {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            finished_results.push(result);
            false // Remove task
        } else {
            true // Keep task
        }
    });

    for result in finished_results {
        find_and_update_node(&mut cache.roots, &result.parent_id, result.children);
    }

    // Process file loading tasks
    let mut finished_files = Vec::new();
    cache.file_tasks.retain_mut(|task| {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            finished_files.push(result);
            false
        } else {
            true
        }
    });

    // Poll any in-flight Twin folder scan. When it finishes, install
    // the scanned tree into `cache.twin`. Keeps the spinner up while
    // pending; drops it to `None` when done.
    if let Some(mut task) = cache.twin_scan_task.take() {
        if let Some(scanned) = future::block_on(future::poll_once(&mut task)) {
            cache.twin = Some(scanned);
            cache.twin_scan_task = None;
        } else {
            cache.twin_scan_task = Some(task);
        }
    }

    for result in finished_files {
        // Final font-dependent shaping on main thread
        let cached_galley = result.layout_job.map(|job| {
            egui_ctx.ctx_mut().unwrap().fonts_mut(|f| f.layout_job(job))
        });

        // Allocate the Document *now* so we have a stable DocumentId
        // to key a tab by. Previously we deferred allocation until the
        // first Compile, but multi-tab needs the id up front.
        let writable = matches!(result.library, ModelLibrary::User);
        let origin = lunco_doc::DocumentOrigin::File {
            path: std::path::PathBuf::from(&result.id),
            writable,
        };
        let doc_id = registry.allocate_with_origin(
            result.source.to_string(),
            origin,
        );

        // If the Twin Browser dispatcher queued a drill-in for this
        // file, apply it now. The canvas projector reads
        // `DrilledInClassNames` on its next tick and lands on the
        // requested class — saves a second click.
        let queued_qualified = pending_drill_ins.take(&result.id);
        if let Some(qualified) = queued_qualified {
            drilled_in.set(doc_id, qualified);
        }

        workbench.open_model = Some(OpenModel {
            model_path: result.id,
            display_name: result.name,
            source: result.source,
            line_starts: result.line_starts,
            detected_name: result.detected_name,
            cached_galley,
            read_only: result.library != ModelLibrary::InMemory
                && result.library != ModelLibrary::User,
            library: result.library,
            doc: Some(doc_id),
        });
        workbench.diagram_dirty = true;
        workbench.is_loading = false;

        // Open (or focus) the multi-instance tab for this document.
        model_tabs.ensure(doc_id);
        layout.open_instance(
            crate::ui::panels::model_view::MODEL_VIEW_KIND,
            doc_id.raw(),
        );
    }
}

// ---------------------------------------------------------------------------
// Package Browser Panel
// ---------------------------------------------------------------------------

pub struct PackageBrowserPanel;

impl Panel for PackageBrowserPanel {
    fn id(&self) -> PanelId { PanelId("modelica_package_browser") }
    fn title(&self) -> String { "📚 Package Browser".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Expand Bundled by default (first run)
        ui.memory_mut(|m| {
            if m.data.get_temp::<bool>(egui::Id::new("tree_expand_bundled_root")).is_none() {
                m.data.insert_temp(egui::Id::new("tree_expand_bundled_root"), true);
            }
        });


        // Fetch needed state from World before borrowing tree_cache mutably
        let active_path_str = {
            let state = world.resource::<WorkbenchState>();
            state.open_model.as_ref().map(|m| m.model_path.clone())
        };
        let active_path = active_path_str.as_deref();
        let mut to_open: Option<PackageAction> = None;
        let mut reopen_in_memory: Option<String> = None;
        let mut create_new = false;
        let mut open_twin_picker = false;
        let mut close_twin = false;
        let mut open_twin_file: Option<std::path::PathBuf> = None;
        let mut pending_rename: Option<(std::path::PathBuf, std::path::PathBuf)> = None;

        {
            let mut tree_cache = world.resource_mut::<PackageTreeCache>();

            // `auto_shrink([false; 2])` tells egui to fill the full
            // panel rect regardless of content size — without it the
            // scroll viewport can end up shorter than the panel
            // height, cutting off the last items and giving users no
            // way to scroll to them (the symptom you hit with long
            // package trees).
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                // Clamp every descendant label to the panel width
                // and truncate with ellipsis if it doesn't fit.
                // Without this, long names (deep MSL paths, rename
                // buffers, workspace paths) spill past the panel
                // edge and the leading characters end up hidden
                // behind neighbouring UI.
                ui.set_max_width(ui.available_width());
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let cache = &mut *tree_cache;

                // ── WORKSPACE ──
                // One unified section. Header shows the open Twin
                // folder's name (or "No folder") and exposes the
                // open/close action. Inside, an always-visible
                // (Untitled) virtual group gathers scratch models
                // that aren't yet bound to a path — matches VS Code's
                // handling of untitled buffers in Explorer.
                let twin_label = if let Some(twin) = cache.twin.as_ref() {
                    twin.root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| twin.root.display().to_string())
                } else {
                    "No folder".to_string()
                };
                section_header(ui, &twin_label, |ui| {
                    // ➕ New is always here (VS Code parity — you can
                    // always make a scratch model).
                    if ui
                        .small_button("➕")
                        .on_hover_text("New model (Ctrl+N)")
                        .clicked()
                    {
                        create_new = true;
                    }
                    if cache.twin_scan_task.is_some() {
                        ui.spinner();
                    } else if cache.twin.is_some() {
                        if ui
                            .small_button("✕")
                            .on_hover_text("Close folder")
                            .clicked()
                        {
                            close_twin = true;
                        }
                    } else if ui
                        .small_button("📁")
                        .on_hover_text("Open a folder")
                        .clicked()
                    {
                        open_twin_picker = true;
                    }
                });

                // Untitled virtual folder — only rendered when there's
                // at least one scratch model. Always top of the tree
                // so recently-created items stay visible.
                if !cache.in_memory_models.is_empty() {
                    egui::CollapsingHeader::new(
                        egui::RichText::new("(Untitled)")
                            .size(11.0)
                            .italics()
                            .color(egui::Color32::from_rgb(220, 220, 160)),
                    )
                    .id_salt("workspace_untitled")
                    .default_open(true)
                    .show(ui, |ui| {
                        if let Some(id) = render_in_memory_models(
                            ui,
                            &cache.in_memory_models,
                            active_path,
                        ) {
                            reopen_in_memory = Some(id);
                        }
                    });
                }

                // Twin folder (if any).
                if cache.twin_scan_task.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Scanning folder…")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    });
                } else if let Some(twin) = cache.twin.clone() {
                    if twin.root_node.children.is_empty() {
                        section_empty_state(
                            ui,
                            "Empty folder. Add a .mo file on disk and reopen.",
                        );
                    } else {
                        // `cache.rename` is borrowed mutably into
                        // every node render so the active rename row
                        // can own the TextEdit buffer. `twin` is
                        // cloned above to avoid aliasing.
                        for child in &twin.root_node.children {
                            let action = render_twin_node(ui, child, &mut cache.rename);
                            if let Some(path) = action.open {
                                open_twin_file = Some(path);
                            }
                            if let Some(path) = action.rename {
                                cache.rename.target = Some(path.clone());
                                cache.rename.buffer = path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                cache.rename.needs_focus = true;
                            }
                            if let Some((from, to)) = action.commit_rename {
                                pending_rename = Some((from, to));
                            }
                            if action.cancel_rename {
                                cache.rename = RenameState::default();
                            }
                        }
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(twin.root.display().to_string())
                            .size(9.0)
                            .color(egui::Color32::DARK_GRAY),
                    );
                } else if cache.in_memory_models.is_empty() {
                    // No Twin AND no scratch — show the real empty
                    // state so the sidebar isn't a blank rectangle.
                    section_empty_state(
                        ui,
                        "No work open. Open a folder, or ➕ for a new model.",
                    );
                }
            });
        }

        if create_new {
            // VS Code-style: one click → new Untitled tab immediately.
            // The observer in `ui::commands` picks a unique
            // `Untitled<N>` name, allocates the doc, and opens a tab.
            world.commands().trigger(crate::ui::commands::CreateNewScratchModel);
        }

        // ── Twin lifecycle ──────────────────────────────────────
        if open_twin_picker {
            // Native dialog still blocks the main thread while visible,
            // but the scan afterwards moves to AsyncComputeTaskPool so
            // a huge twin doesn't freeze the UI on open.
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Open Twin folder")
                .pick_folder()
            {
                if let Some(mut console) = world
                    .get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
                {
                    console.info(format!("Scanning twin folder: {}", folder.display()));
                }
                let pool = AsyncComputeTaskPool::get();
                let task = pool.spawn(async move { scan_twin_folder(folder) });
                let mut cache = world.resource_mut::<PackageTreeCache>();
                cache.twin = None; // clear old tree so spinner shows
                cache.twin_scan_task = Some(task);
            }
        }
        if close_twin {
            let mut cache = world.resource_mut::<PackageTreeCache>();
            cache.twin = None;
            cache.twin_scan_task = None;
        }
        if let Some(path) = open_twin_file {
            // Treat the clicked .mo as a user-writable file. Use the
            // existing disk-load path so loading + tab-open flows are
            // consistent with clicks on Examples (minus writability).
            let id = path.to_string_lossy().into_owned();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| id.clone());
            open_model(world, id, name, ModelLibrary::User);
        }

        // Commit a rename — std::fs::rename, then trigger a rescan so
        // the tree reflects the new name. If rename fails (conflict,
        // permissions) log and leave state unchanged so the user can
        // retry or cancel.
        if let Some((from, to)) = pending_rename {
            if to.exists() {
                let msg = format!(
                    "Rename cancelled: '{}' already exists.",
                    to.display()
                );
                log::warn!("[Rename] {msg}");
                if let Some(mut console) = world
                    .get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
                {
                    console.warn(msg);
                }
            } else if let Err(e) = std::fs::rename(&from, &to) {
                let msg = format!(
                    "Rename failed: {} -> {}: {e}",
                    from.display(),
                    to.display()
                );
                log::error!("[Rename] {msg}");
                if let Some(mut console) = world
                    .get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
                {
                    console.error(msg);
                }
            } else {
                let msg = format!("Renamed {} -> {}", from.display(), to.display());
                log::info!("[Rename] {msg}");
                if let Some(mut console) = world
                    .get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
                {
                    console.info(msg);
                }
                // Re-scan the twin to pick up the new name. Same
                // async path as Open Folder.
                if let Some(root) = world
                    .resource::<PackageTreeCache>()
                    .twin
                    .as_ref()
                    .map(|t| t.root.clone())
                {
                    use bevy::tasks::AsyncComputeTaskPool;
                    let pool = AsyncComputeTaskPool::get();
                    let task = pool.spawn(async move { scan_twin_folder(root) });
                    let mut cache = world.resource_mut::<PackageTreeCache>();
                    cache.twin_scan_task = Some(task);
                }
            }
            world.resource_mut::<PackageTreeCache>().rename = RenameState::default();
        }

        if let Some(action) = to_open {
            match action {
                PackageAction::Open(id, name, lib) => open_model(world, id, name, lib),
                PackageAction::Instantiate(id) => instantiate_model(world, id),
            }
        }

        // Re-open an already-allocated in-memory model. We pass the id;
        // `open_model`'s mem:// branch now consults the registry to
        // restore the user's current source rather than regenerating
        // from a template.
        if let Some(id) = reopen_in_memory {
            // Name is the part after "mem://".
            let name = id.strip_prefix("mem://").unwrap_or(&id).to_string();
            open_model(world, id, name, ModelLibrary::InMemory);
        }
    }
}

enum PackageAction {
    Open(String, String, ModelLibrary),
    Instantiate(String),
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_node(
    node: &mut PackageNode,
    ui: &mut egui::Ui,
    active_path: Option<&str>,
    depth: usize,
    tasks: &mut Vec<Task<ScanResult>>,
) -> Option<PackageAction> {
    let indent = depth as f32 * 16.0 + 4.0;
    let mut result = None;

    match node {
        PackageNode::Category { id, name, children, is_loading, fs_path, package_path } => {
            let expand_id = egui::Id::new(format!("tree_expand_{}", id));
            let is_expanded = ui.memory(|m| m.data.get_temp::<bool>(expand_id).unwrap_or(false));
            let arrow = if is_expanded { "▼" } else { "▶" };

            let resp = ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.add_sized([12.0, 12.0], egui::Label::new(
                    egui::RichText::new(arrow).size(8.0).color(egui::Color32::GRAY)
                ));
                ui.add(egui::Label::new(
                    egui::RichText::new(name.as_str()).size(11.0)
                ).sense(egui::Sense::click()))
            }).inner;

            if resp.clicked() {
                ui.memory_mut(|m| m.data.insert_temp(expand_id, !is_expanded));
            }

            if is_expanded {
                if let Some(ref mut children_vec) = children {
                    let limit_id = egui::Id::new(format!("tree_limit_{}", id));
                    let limit = ui.memory(|m| m.data.get_temp::<usize>(limit_id).unwrap_or(100));

                    for (idx, child) in children_vec.iter_mut().enumerate() {
                        if idx >= limit {
                            ui.horizontal(|ui| {
                                ui.add_space(indent + 16.0);
                                if ui.button(format!("... and {} more (click to show all)", children_vec.len() - limit)).clicked() {
                                    ui.memory_mut(|m| m.data.insert_temp(limit_id, children_vec.len()));
                                }
                            });
                            break;
                        }
                        if let Some(req) = render_node(child, ui, active_path, depth + 1, tasks) {
                            result = Some(req);
                        }
                    }
                } else if !*is_loading {
                    // Trigger load
                    *is_loading = true;
                    let pool = AsyncComputeTaskPool::get();
                    let parent_id = id.clone();
                    let scan_dir = fs_path.clone();
                    let pkg_path = package_path.clone();

                    let task = pool.spawn(async move {
                        let children = scan_msl_dir(&scan_dir, pkg_path);
                        ScanResult { parent_id, children }
                    });
                    tasks.push(task);
                }

                if *is_loading {
                    ui.horizontal(|ui| {
                        ui.add_space(indent + 16.0);
                        ui.label(egui::RichText::new("⌛ Loading...").size(10.0).italics().color(egui::Color32::GRAY));
                    });
                }
            }
        }

        PackageNode::Model { id, name, library, .. } => {
            let is_active = active_path == Some(id.as_str());

            let bg = if is_active {
                egui::Color32::from_rgba_unmultiplied(80, 80, 0, 40)
            } else {
                egui::Color32::TRANSPARENT
            };

            let lib_icon = match library {
                ModelLibrary::MSL => "📚",
                ModelLibrary::Bundled => "📦",
                ModelLibrary::User => "📁",
                ModelLibrary::InMemory => "💾",
            };

            let resp = ui.horizontal(|ui| {
                ui.add_space(indent + 16.0);
                ui.add(egui::Label::new(
                    egui::RichText::new(format!("{} {}", lib_icon, name)).size(11.0)
                ).sense(egui::Sense::click()))
            }).inner;

            if is_active {
                ui.painter().rect_filled(resp.rect, 2.0, bg);
            }

            let mut instantiate_requested = false;
            if library == &ModelLibrary::MSL {
                resp.context_menu(|ui| {
                    if ui.button("➕ Instantiate in Diagram").clicked() {
                        instantiate_requested = true;
                        ui.close();
                    }
                });
            }

            if instantiate_requested {
                result = Some(PackageAction::Instantiate(id.clone()));
            } else if resp.clicked() {
                result = Some(PackageAction::Open(id.clone(), name.clone(), library.clone()));
            }

            if resp.hovered() {
                let info = match library {
                    ModelLibrary::MSL => "📚 MSL — read-only",
                    ModelLibrary::Bundled => "📦 Bundled — read-only",
                    ModelLibrary::User => "📁 User model — writable",
                    ModelLibrary::InMemory => "💾 In-memory — writable",
                };
                resp.on_hover_text(format!("{}\n{}", name, info));
            }
        }
    }

    result
}

/// Flush any in-progress edits on the currently-open model into its
/// Document so navigating away doesn't lose work.
///
/// Two paths are covered:
///
/// 1. **Text edits in the code editor** — the TextEdit focus-loss hook
///    already handles the common case, but a click in the Package
///    Browser doesn't always trigger `lost_focus()` on the editor. We
///    re-commit defensively from `EditorBufferState.text`.
/// 2. **Visual diagram edits** — `DiagramState.diagram` holds the
///    user's placed components / wires. If non-empty, regenerate
///    Modelica source and checkpoint it into the Document. This is the
///    diagram equivalent of focus-loss commit.
///
/// Both write through `ModelicaDocumentRegistry::checkpoint_source`,
/// which fires `DocumentChanged` so any subscriber (including the
/// re-open path via `find_by_path`) sees the fresh source.
///
/// No-op when the current model is read-only, has no backing Document,
/// or both buffers are empty.
fn commit_current_model_edits(world: &mut World) {
    // Snapshot everything we need up-front so we don't fight the borrow
    // checker when mutating the registry below.
    let (doc_id, is_read_only, model_name) = {
        let state = world.resource::<WorkbenchState>();
        let Some(m) = state.open_model.as_ref() else { return };
        (m.doc, m.read_only, m.detected_name.clone().unwrap_or_else(|| m.display_name.clone()))
    };
    let Some(doc_id) = doc_id else { return };
    if is_read_only {
        return;
    }

    // In Phase α, the diagram panel emits AST ops directly to the
    // document on every edit — the document is already the source of
    // truth for anything the user did in the diagram. No
    // regenerate-from-VisualDiagram checkpoint is needed (or correct —
    // the old path would overwrite hand-typed comments and
    // unrepresented annotations). We just commit the text-buffer
    // residue here: the code editor's focus-loss commit is normally
    // enough, but the user may switch panels before the widget has
    // fired `lost_focus()`, so we force a checkpoint on model switch.
    let _ = model_name; // kept above for future per-class targeting
    let buffer = world
        .get_resource::<crate::ui::panels::code_editor::EditorBufferState>()
        .map(|b| b.text.clone());
    if let Some(src) = buffer {
        if !src.is_empty() {
            world
                .resource_mut::<ModelicaDocumentRegistry>()
                .checkpoint_source(doc_id, src);
        }
    }
}

/// Uniform section header for the sidebar. Label on the left in
/// muted caps, optional right-aligned action slot (e.g. `➕`, spinner,
/// close button). Matches VS Code Explorer's section-heading cadence.
fn section_header<F: FnOnce(&mut egui::Ui)>(
    ui: &mut egui::Ui,
    title: &str,
    right_actions: F,
) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .size(10.0)
                .color(egui::Color32::from_rgb(160, 160, 180))
                .strong(),
        );
        // Push actions to the right edge.
        let remaining = ui.available_width() - 60.0;
        if remaining > 0.0 {
            ui.add_space(remaining);
        }
        right_actions(ui);
    });
    ui.separator();
}

/// Muted placeholder text for empty sections. Keeps sections visually
/// present but non-noisy when there's nothing to show.
fn section_empty_state(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(text)
                .size(10.0)
                .italics()
                .color(egui::Color32::from_rgb(130, 130, 140)),
        );
    });
}

/// Signals that can come out of rendering a single tree node.
/// Returned from `render_twin_node`; the outer loop applies them
/// after the render pass so we don't mutate the cache while walking
/// the tree.
#[derive(Default)]
pub struct TwinNodeAction {
    /// User clicked a `.mo` file — open it as a tab.
    pub open: Option<std::path::PathBuf>,
    /// User invoked "Rename" from the context menu — enter rename
    /// mode for this path.
    pub rename: Option<std::path::PathBuf>,
    /// User pressed Enter in the rename TextEdit — commit rename
    /// from the first path to the second.
    pub commit_rename: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// User cancelled rename (Escape / blur).
    pub cancel_rename: bool,
}

impl TwinNodeAction {
    fn merge(&mut self, other: TwinNodeAction) {
        if other.open.is_some() {
            self.open = other.open;
        }
        if other.rename.is_some() {
            self.rename = other.rename;
        }
        if other.commit_rename.is_some() {
            self.commit_rename = other.commit_rename;
        }
        if other.cancel_rename {
            self.cancel_rename = true;
        }
    }
}

/// Render one Twin tree node. Directories use `CollapsingHeader` so
/// the twisty arrow / indentation / hover highlight come from egui
/// for free. Files are a selectable row with a right-click context
/// menu (Rename). Non-Modelica files render greyed + disabled so the
/// tree structure is visible but users can't try to open a README.
///
/// `rename` holds the current rename state; when the rendered node's
/// path matches `rename.target`, the row becomes an inline TextEdit.
pub fn render_twin_node(
    ui: &mut egui::Ui,
    node: &TwinNode,
    rename: &mut RenameState,
) -> TwinNodeAction {
    let mut action = TwinNodeAction::default();

    // Rename mode — replace the normal row with an inline TextEdit.
    // Shared for files and folders; the commit handler checks what
    // was at the path and renames.
    if rename.target.as_deref() == Some(node.path.as_path()) {
        let response = ui.add(
            egui::TextEdit::singleline(&mut rename.buffer)
                .desired_width(f32::INFINITY),
        );
        if rename.needs_focus {
            response.request_focus();
            rename.needs_focus = false;
        }
        // Enter → commit, Escape → cancel, loss of focus → cancel.
        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let new_name = rename.buffer.trim().to_string();
                if !new_name.is_empty() && new_name != node.name {
                    let parent = node.path.parent().unwrap_or(std::path::Path::new(""));
                    let new_path = parent.join(&new_name);
                    action.commit_rename = Some((node.path.clone(), new_path));
                } else {
                    action.cancel_rename = true;
                }
            } else {
                // Escape or click-away — treat as cancel.
                action.cancel_rename = true;
            }
        }
        return action;
    }

    if node.is_dir() {
        let id = egui::Id::new(("twin_node", node.path.as_os_str()));
        let header = egui::CollapsingHeader::new(
            egui::RichText::new(format!("📁 {}", node.name))
                .size(11.0)
                .color(egui::Color32::from_rgb(180, 200, 230)),
        )
        .id_salt(id)
        .default_open(false);
        let header_response = header
            .show(ui, |ui| {
                for child in &node.children {
                    action.merge(render_twin_node(ui, child, rename));
                }
            })
            .header_response;
        node_context_menu(&header_response, node, &mut action);
    } else {
        let (icon, color) = if node.is_modelica {
            ("📄", egui::Color32::from_rgb(220, 220, 160))
        } else {
            ("·", egui::Color32::from_rgb(110, 110, 120))
        };
        let row = egui::Button::selectable(
            false,
            egui::RichText::new(format!("{icon}  {}", node.name))
                .size(11.0)
                .color(color),
        );
        let resp = ui.add_enabled(node.is_modelica || !node.is_modelica, row);
        if resp.clicked() && node.is_modelica {
            action.open = Some(node.path.clone());
        }
        node_context_menu(&resp, node, &mut action);
    }
    action
}

/// Attach a right-click context menu to `resp` with Rename.
/// Kept small today — Delete + New File land in the next phase
/// alongside filesystem guards for dangerous operations.
fn node_context_menu(
    resp: &egui::Response,
    node: &TwinNode,
    action: &mut TwinNodeAction,
) {
    resp.context_menu(|ui| {
        if ui.button("✏  Rename").clicked() {
            action.rename = Some(node.path.clone());
            ui.close();
        }
    });
}

/// Render every in-memory model the user has created this session.
/// Returns the id of the one the user clicked (if any).
///
/// `active_id` is the currently-open model's `model_path`, used to mark
/// the active entry so the user can see which one is being edited.
fn render_in_memory_models(
    ui: &mut egui::Ui,
    entries: &[InMemoryEntry],
    active_id: Option<&str>,
) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut clicked = None;
    for entry in entries {
        let is_active = active_id == Some(entry.id.as_str());
        let label = if is_active {
            egui::RichText::new(format!("💾 {} ✏️", entry.display_name))
                .size(11.0)
                .color(egui::Color32::YELLOW)
                .strong()
        } else {
            egui::RichText::new(format!("💾 {}", entry.display_name))
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 220, 180))
        };
        let resp = ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.add(egui::Label::new(label).sense(egui::Sense::click()))
        }).inner;
        if resp.clicked() && !is_active {
            clicked = Some(entry.id.clone());
        }
    }
    clicked
}

pub(crate) fn open_model(world: &mut World, id: String, name: String, library: ModelLibrary) {
    // Before navigating away, flush any in-progress work on the current
    // model into its Document. Matches the text editor's focus-loss
    // commit so the user's changes survive a round-trip.
    commit_current_model_edits(world);

    if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
        let prev_path = state.open_model.as_ref().map(|m| m.model_path.clone());
        if let Some(p) = prev_path {
            state.navigation_stack.push(p);
        }
        state.is_loading = true;
    }

    // Determine the loading strategy based on the ID scheme
    if id.starts_with("mem://") {
        let mem_name_str = id.strip_prefix("mem://").unwrap_or("NewModel").to_string();

        // Find the existing Document for this in-memory model. If one
        // exists (user created it earlier this session), we restore its
        // *current* source and hold on to the id so further edits keep
        // landing on the same Document. Only fall back to a fresh
        // template if nothing is registered — a defensive path; shouldn't
        // normally fire because New Model allocates up front.
        let mem_path = std::path::PathBuf::from(&id);
        let (source, doc_id) = {
            let registry = world.resource::<ModelicaDocumentRegistry>();
            match registry.find_by_path(&mem_path) {
                Some(doc) => {
                    let src = registry
                        .host(doc)
                        .map(|h| h.document().source().to_string())
                        .unwrap_or_default();
                    (src, Some(doc))
                }
                None => (
                    format!("model {}\n\nend {};\n", mem_name_str, mem_name_str),
                    None,
                ),
            }
        };

        // Compute line starts for the restored source so the code editor
        // can lay it out correctly.
        let mut line_starts = vec![0usize];
        for (i, byte) in source.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(i + 1);
            }
        }

        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
            let source_arc: std::sync::Arc<str> = source.into();
            state.open_model = Some(OpenModel {
                model_path: id.clone(),
                display_name: name,
                source: source_arc.clone(),
                line_starts: line_starts.into(),
                detected_name: Some(mem_name_str),
                cached_galley: None,
                read_only: false,
                library,
                doc: doc_id,
            });
            state.editor_buffer = source_arc.to_string();
            state.diagram_dirty = true;
            state.is_loading = false;
        }

        // Open (or focus) the tab for this in-memory model.
        // Panels render inside `render_workbench`, which extracts
        // `WorkbenchLayout` from the world for the duration — touching
        // the resource directly here would panic. Fire an event; the
        // workbench's `on_open_tab` observer picks it up after the
        // render system completes.
        if let Some(doc) = doc_id {
            world
                .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
                .ensure(doc);
            world.commands().trigger(lunco_workbench::OpenTab {
                kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
                instance: doc.raw(),
            });
        }
        let _ = id;
        return;
    }

    // Background load for all other types (Disk or Bundled)
    let pool = AsyncComputeTaskPool::get();
    let id_clone = id.clone();
    let name_clone = name.clone();
    let name_result = name.clone();
    let lib_clone = library.clone();

    let task = pool.spawn(async move {
        let source_text = if id_clone.starts_with("bundled://") {
            let filename = id_clone.strip_prefix("bundled://").unwrap_or("");
            crate::models::get_model(filename).unwrap_or("").to_string()
        } else if let Some(rel_path) = id_clone.strip_prefix("msl_path:") {
            let disk_path = rel_path.replace('.', "/").replace("/mo", ".mo");
            let msl_root = lunco_assets::msl_dir();
            let full_path = msl_root.join("Modelica").join(disk_path);
            std::fs::read_to_string(&full_path).unwrap_or_else(|e| {
                format!("// Error reading {}\n// {:?}", full_path.display(), e)
            })
        } else {
            // Default User model load
            let path = std::path::PathBuf::from(&id_clone);
            std::fs::read_to_string(&path).unwrap_or_else(|e| {
                format!("// Error reading {:?}\n// {:?}", path, e)
            })
        };

        // Compute line starts (zero allocation scan)
        let mut line_starts = vec![0];
        for (i, byte) in source_text.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(i + 1);
            }
        }

        // Use the name from the UI immediately instead of parsing the whole AST.
        let detected_name = Some(name_clone);

        // Pre-compute text layout in the background (no fonts needed for LayoutJob logic)
        let style = egui::Style::default();
        let mut layout_job = crate::ui::panels::code_editor::modelica_layouter(&style, &source_text);
        layout_job.wrap.max_width = f32::INFINITY;

        FileLoadResult {
            id: id_clone,
            name: name_result,
            library: lib_clone,
            source: source_text.into(),
            line_starts: line_starts.into(),
            detected_name,
            layout_job: Some(layout_job),
        }
    });

    if let Some(mut cache) = world.get_resource_mut::<PackageTreeCache>() {
        cache.file_tasks.push(task);
    }
}

fn instantiate_model(world: &mut World, id: String) {
    let msl_path = if let Some(stripped) = id.strip_prefix("msl_path:") {
        format!("Modelica.{}", stripped)
    } else {
        id.clone()
    };

    if let Some(def) = crate::visual_diagram::msl_component_by_path(&msl_path) {
        if let Some(mut state) = world.get_resource_mut::<crate::ui::panels::diagram::DiagramState>() {
            state.placement_counter += 1;
            let x = 100.0 + (state.placement_counter % 3) as f32 * 200.0;
            let y = 80.0 + (state.placement_counter / 3) as f32 * 160.0;
            state.add_component(def, egui::Pos2::new(x, y));
            // Ensure diagram switches to the active tab if necessary
            world.resource_mut::<WorkbenchState>().diagram_dirty = true;
        }
    } else {
        log::warn!("Component definition not found for MSL path: {}", msl_path);
    }
}

// The legacy "New Model" modal (name-prompt dialog) used to live here.
// VS Code's one-click "New Untitled" flow replaces it — the ➕
// buttons fire `CreateNewScratchModel`, the observer in
// `ui::commands` picks the next free `UntitledN` name, allocates the
// doc, and opens a tab. Rename is deferred to Save-As.
