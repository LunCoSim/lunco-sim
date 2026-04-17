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
}

/// User's Twin workspace — a folder on disk being browsed as a tree.
///
/// Read-only in this first pass: scanning + open-on-click. Edits
/// (new/rename/delete, drag-move) land in the next phase.
#[derive(Clone)]
pub struct TwinState {
    /// Root folder the user picked via Open Folder.
    pub root: std::path::PathBuf,
    /// Flat list of entries (files + folders), with nesting captured
    /// by `depth`. Built via one synchronous `walkdir` at open time;
    /// cheap for hundreds-of-files twins, can become lazy later.
    pub entries: Vec<TwinEntry>,
}

/// One row in the Twin Explorer tree.
#[derive(Clone)]
pub struct TwinEntry {
    /// Absolute path on disk.
    pub path: std::path::PathBuf,
    /// Display name — just the file/folder name (no directory path).
    pub name: String,
    /// Nesting depth for indentation (0 = direct child of root).
    pub depth: usize,
    /// Whether this is a directory (rendered as `📁`) or a file.
    pub is_dir: bool,
    /// Whether this entry is a Modelica source (`.mo`). Other files
    /// are shown but greyed out / non-clickable.
    pub is_modelica: bool,
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
        }
    }
}

/// Scan `root` for a Twin tree. Skips hidden dirs + `.git` + common
/// build dirs. `.mo` files are flagged as instantiable; other files
/// are listed for context but can't be opened.
pub fn scan_twin_folder(root: std::path::PathBuf) -> TwinState {
    use walkdir::WalkDir;
    let mut entries = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !n.starts_with('.')
                && n != "target"
                && n != "shared_target"
                && n != "node_modules"
        })
        .flatten()
    {
        let path = entry.path().to_path_buf();
        if path == root {
            continue; // Skip the root itself.
        }
        let depth = entry.depth().saturating_sub(1);
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_dir();
        let is_modelica = !is_dir
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false);
        entries.push(TwinEntry {
            path,
            name,
            depth,
            is_dir,
            is_modelica,
        });
    }
    TwinState { root, entries }
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
    // Use the bundled:// URL scheme as the id so open_model can find it
    BUNDLED_MODELS.iter().map(|(filename, _source)| {
        PackageNode::Model {
            id: format!("bundled://{}", filename),
            name: filename.strip_suffix(".mo").unwrap_or(filename).to_string(),
            library: ModelLibrary::Bundled,
        }
    }).collect()
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

        {
            let mut tree_cache = world.resource_mut::<PackageTreeCache>();

            egui::ScrollArea::vertical().show(ui, |ui| {
                let cache = &mut *tree_cache;
                let roots = &mut cache.roots;
                let tasks = &mut cache.tasks;
                let in_memory = &cache.in_memory_models;

                // ── Section 1: Your Models ──
                // Session's scratch models (Untitled-N) + the future
                // Twin file tree live here. MSL is NOT in this browser
                // anymore — it's the Component Palette's responsibility.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("📁 Your Models").size(12.0).color(egui::Color32::YELLOW).strong());
                    if ui.small_button("➕")
                        .on_hover_text("New untitled model (Ctrl+N)")
                        .clicked()
                    {
                        create_new = true;
                    }
                });

                if in_memory.is_empty() {
                    ui.label(
                        egui::RichText::new("(no models — click ➕ or press Ctrl+N)")
                            .size(10.0)
                            .color(egui::Color32::GRAY),
                    );
                } else if let Some(id) = render_in_memory_models(ui, in_memory, active_path) {
                    reopen_in_memory = Some(id);
                }

                ui.add_space(4.0);
                ui.separator();

                // ── Section 2: Examples ──
                // Complete example models shipped with the app. Open to
                // study or simulate — these are files, not drag-drop
                // building blocks (those live in the Component Palette).
                ui.label(
                    egui::RichText::new("📦 Examples")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(100, 255, 100))
                        .strong(),
                );

                if roots.len() > 1 {
                    if let Some(req) = render_node(&mut roots[1], ui, active_path, 0, tasks) {
                        to_open = Some(req);
                    }
                }

                ui.add_space(4.0);
                ui.separator();

                // ── Section 3: Twin (folder on disk) ──
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📂 Twin")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(140, 200, 255))
                            .strong(),
                    );
                    if cache.twin.is_some() {
                        if ui.small_button("✕ close").on_hover_text("Close this folder").clicked() {
                            close_twin = true;
                        }
                    } else if ui.small_button("📁 Open…")
                        .on_hover_text("Pick a folder to browse as your workspace")
                        .clicked()
                    {
                        open_twin_picker = true;
                    }
                });

                if let Some(twin) = cache.twin.as_ref() {
                    // Root label
                    let root_name = twin
                        .root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| twin.root.display().to_string());
                    ui.label(
                        egui::RichText::new(format!("📁 {}", root_name))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 220, 255)),
                    );
                    ui.label(
                        egui::RichText::new(twin.root.display().to_string())
                            .size(9.0)
                            .color(egui::Color32::DARK_GRAY),
                    );
                    ui.add_space(2.0);

                    if twin.entries.is_empty() {
                        ui.label(
                            egui::RichText::new("(empty folder)")
                                .size(10.0)
                                .color(egui::Color32::GRAY),
                        );
                    } else {
                        for entry in &twin.entries {
                            let indent = entry.depth as f32 * 12.0 + 4.0;
                            let icon = if entry.is_dir {
                                "📁"
                            } else if entry.is_modelica {
                                "📄"
                            } else {
                                "·"
                            };
                            let text = egui::RichText::new(format!("{icon} {}", entry.name))
                                .size(10.5)
                                .color(if entry.is_modelica {
                                    egui::Color32::from_rgb(220, 220, 160)
                                } else if entry.is_dir {
                                    egui::Color32::from_rgb(160, 200, 240)
                                } else {
                                    egui::Color32::DARK_GRAY
                                });
                            let resp = ui
                                .horizontal(|ui| {
                                    ui.add_space(indent);
                                    ui.add(
                                        egui::Label::new(text)
                                            .sense(if entry.is_modelica {
                                                egui::Sense::click()
                                            } else {
                                                egui::Sense::hover()
                                            }),
                                    )
                                })
                                .inner;
                            if entry.is_modelica && resp.clicked() {
                                open_twin_file = Some(entry.path.clone());
                            }
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new("(no folder open)")
                            .size(10.0)
                            .color(egui::Color32::GRAY),
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
            // Synchronous native picker — blocks the UI thread for
            // the short moment the dialog is visible. Async version
            // lands when we want multi-window.
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Open Twin folder")
                .pick_folder()
            {
                let scanned = scan_twin_folder(folder);
                world.resource_mut::<PackageTreeCache>().twin = Some(scanned);
            }
        }
        if close_twin {
            world.resource_mut::<PackageTreeCache>().twin = None;
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

    // Visual diagram → source. If the user has placed components this
    // takes precedence over the text buffer; in the current UX only one
    // of the two is edited at a time.
    let diagram_source = world
        .get_resource::<crate::ui::panels::diagram::DiagramState>()
        .filter(|ds| !ds.diagram.nodes.is_empty())
        .map(|ds| crate::visual_diagram::generate_modelica_source(&ds.diagram, &model_name));

    if let Some(src) = diagram_source {
        world
            .resource_mut::<ModelicaDocumentRegistry>()
            .checkpoint_source(doc_id, src);
        return;
    }

    // Fallback: commit the text buffer. If the user was in Text mode,
    // their latest keystrokes may not have triggered `lost_focus()`
    // before the click on the Package Browser.
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

fn open_model(world: &mut World, id: String, name: String, library: ModelLibrary) {
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
