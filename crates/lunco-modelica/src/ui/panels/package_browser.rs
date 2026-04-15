//! Package Browser — Dymola-style library tree.
//!
//! Scans the real MSL directory from disk (via `lunco_assets::msl_dir()`).
//! Bundled models are included as read-only entries.
//! Clicking any `.mo` file opens it in the Code Editor + Diagram panels.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::models::BUNDLED_MODELS;
use crate::ui::state::{ModelLibrary, OpenModel, WorkbenchState};

use bevy::tasks::{AsyncComputeTaskPool, Task};
use futures_lite::future;

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
        file_path: std::path::PathBuf,  // Path on disk, read lazily
    },
}

impl PackageNode {
    pub fn name(&self) -> &str {
        match self {
            PackageNode::Category { name, .. } | PackageNode::Model { name, .. } => name,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            PackageNode::Category { id, .. } | PackageNode::Model { id, .. } => id,
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

#[derive(Resource)]
pub struct PackageTreeCache {
    pub roots: Vec<PackageNode>,
    /// Active scanning tasks.
    pub tasks: Vec<Task<ScanResult>>,
    /// Active file loading tasks.
    pub file_tasks: Vec<Task<FileLoadResult>>,
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

        Self { roots, tasks: Vec::new(), file_tasks: Vec::new() }
    }
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
                    file_path: path,
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
            file_path: std::path::PathBuf::new(),
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

        workbench.open_model = Some(OpenModel {
            model_path: result.id,
            display_name: result.name,
            source: result.source,
            line_starts: result.line_starts,
            detected_name: result.detected_name,
            cached_galley,
            read_only: result.library != ModelLibrary::InMemory && result.library != ModelLibrary::User,
            library: result.library,
        });
        workbench.diagram_dirty = true;
        workbench.is_loading = false;
    }

}

// ---------------------------------------------------------------------------
// Package Browser Panel
// ---------------------------------------------------------------------------

pub struct PackageBrowserPanel;

impl WorkbenchPanel for PackageBrowserPanel {
    fn id(&self) -> &str { "modelica_package_browser" }
    fn title(&self) -> String { "📚 Package Browser".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(35, 35, 40))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Expand Bundled by default (first run)
        ui.memory_mut(|m| {
            if m.data.get_temp::<bool>(egui::Id::new("tree_expand_bundled_root")).is_none() {
                m.data.insert_temp(egui::Id::new("tree_expand_bundled_root"), true);
            }
        });

        let mut to_open = None;
        let mut show_dialog = false;

        // Fetch needed state from World before borrowing tree_cache mutably
        let (active_path_str, in_memory_model) = {
            let state = world.resource::<WorkbenchState>();
            let path = state.open_model.as_ref().map(|m| m.model_path.clone());
            let mem = state.open_model.as_ref()
                .filter(|m| m.library == ModelLibrary::InMemory)
                .map(|m| m.clone());
            (path, mem)
        };
        let active_path = active_path_str.as_deref();

        {
            let mut tree_cache = world.resource_mut::<PackageTreeCache>();

            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── Section 1: MSL Library ──
                ui.add_space(4.0);
                ui.label(egui::RichText::new("📚 Modelica Standard Library").size(12.0).color(egui::Color32::from_rgb(100, 180, 255)).strong());
                ui.label(egui::RichText::new("Read-only — reference components").size(9.0).color(egui::Color32::GRAY));

                // MSL root is first root
                let cache = &mut *tree_cache;
                let roots = &mut cache.roots;
                let tasks = &mut cache.tasks;

                if let Some(msl_root) = roots.first_mut() {
                    if let Some(req) = render_node(msl_root, ui, active_path, 0, tasks) {
                        to_open = Some(req);
                    }
                }

                ui.add_space(4.0);
                ui.separator();

                // ── Section 2: Bundled Models ──
                ui.label(egui::RichText::new("📦 Bundled Models").size(12.0).color(egui::Color32::from_rgb(100, 255, 100)).strong());
                ui.label(egui::RichText::new("Read-only — shipped examples").size(9.0).color(egui::Color32::GRAY));

                if roots.len() > 1 {
                    if let Some(req) = render_node(&mut roots[1], ui, active_path, 0, tasks) {
                        to_open = Some(req);
                    }
                }

                ui.add_space(4.0);
                ui.separator();

                // ── Section 3: Your Models ──
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("📁 Your Models").size(12.0).color(egui::Color32::YELLOW).strong());
                    if ui.small_button("➕").clicked() {
                        show_dialog = true;
                    }
                });
                ui.label(egui::RichText::new("Writable — your custom models").size(9.0).color(egui::Color32::GRAY));

                if let Some(ref model) = in_memory_model {
                    render_active_model(ui, &model);
                }

                if ui.button("➕ Create new model…").clicked() {
                    show_dialog = true;
                }

                // Show Open Folder placeholder
                if roots.len() > 2 {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("📂 Open Folder").size(10.0).color(egui::Color32::DARK_GRAY));
                    ui.label(egui::RichText::new("(coming soon)").size(8.0).color(egui::Color32::DARK_GRAY));
                }
            });
        }

        if show_dialog {
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("show_new_model_dialog"), true));
        }

        if let Some((id, name, library)) = to_open {
            open_model(world, id, name, library);
        }

        if ui.memory(|m| m.data.get_temp::<bool>(egui::Id::new("show_new_model_dialog")).unwrap_or(false)) {
            show_new_model_dialog(ui, world);
        }
    }
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
) -> Option<(String, String, ModelLibrary)> {
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

            if resp.clicked() {
                result = Some((id.clone(), name.clone(), library.clone()));
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

/// Render the currently active in-memory model in "Your Models" section.
fn render_active_model(ui: &mut egui::Ui, model: &OpenModel) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(egui::RichText::new(format!("💾 {} ✏️", model.display_name)).size(11.0).color(egui::Color32::YELLOW));
    }).response.on_hover_text("Your in-memory model — currently open in editor");
}

fn open_model(world: &mut World, id: String, name: String, library: ModelLibrary) {
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
        let source = format!("model {}\n\nend {};\n", mem_name_str, mem_name_str);
        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
            let source_arc: std::sync::Arc<str> = source.into();
            state.open_model = Some(OpenModel {
                model_path: id,
                display_name: name,
                source: source_arc.clone(),
                line_starts: vec![0].into(),
                detected_name: Some(mem_name_str),
                cached_galley: None,
                read_only: false,
                library,
            });
            state.editor_buffer = source_arc.to_string();
            state.diagram_dirty = true;
            state.is_loading = false;
        }
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

// ---------------------------------------------------------------------------
// New Model Dialog
// ---------------------------------------------------------------------------

fn show_new_model_dialog(ui: &mut egui::Ui, world: &mut World) {
    egui::Window::new("Create New Model")
        .id(egui::Id::new("new_model_dialog"))
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            let mut name_buf = ui.memory_mut(|m| {
                m.data.get_temp::<String>(egui::Id::new("new_model_name"))
                    .unwrap_or_else(|| "MyModel".to_string())
            });

            ui.label("Name for the new model:");
            let text_edit = ui.add(
                egui::TextEdit::singleline(&mut name_buf)
                    .desired_width(280.0)
                    .hint_text("e.g., MyRC_Circuit")
            );
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("new_model_name"), name_buf.clone()));

            if text_edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("show_new_model_dialog"), false));
            }

            ui.horizontal(|ui| {
                if ui.button("Create").clicked() || (text_edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    let name = name_buf.trim().to_string();
                    if name.is_empty() { return; }

                    let valid = name.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false)
                        && name.chars().all(|c| c.is_alphanumeric() || c == '_');

                    if !valid {
                        ui.colored_label(egui::Color32::LIGHT_RED, "Must start with a letter, only letters/numbers/_ allowed.");
                        return;
                    }

                    let source = format!("model {}\n\nend {};\n", name, name);

                    if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                        let source_arc: std::sync::Arc<str> = source.into();
                        state.open_model = Some(OpenModel {
                            model_path: format!("mem://{}", name),
                            display_name: name.clone(),
                            source: source_arc.clone(),
                            line_starts: vec![0].into(),
                            detected_name: Some(name.clone()),
                            cached_galley: None,
                            read_only: false,
                            library: ModelLibrary::InMemory,
                        });
                        state.editor_buffer = source_arc.to_string();
                        state.diagram_dirty = true;

                        if let Some(mut ds) = world.get_resource_mut::<crate::ui::panels::diagram::DiagramState>() {
                            ds.diagram = crate::visual_diagram::VisualDiagram::default();
                            ds.model_counter += 1;
                            ds.compile_status = None;
                        }
                    }

                    ui.memory_mut(|m| {
                        m.data.insert_temp(egui::Id::new("show_new_model_dialog"), false);
                        m.data.remove_temp::<String>(egui::Id::new("new_model_name"));
                    });
                }

                if ui.button("Cancel").clicked() {
                    ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("show_new_model_dialog"), false));
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label("📌 The model is in-memory (shown as 💾 in Package Browser).");
            ui.label("Add components from the MSL Library panel, then COMPILE & RUN.");
        });
}
