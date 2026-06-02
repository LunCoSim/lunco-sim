//! Package Browser — Dymola-style library tree.

use bevy::prelude::*;
use bevy_egui::egui;
use crate::class_ref::{ClassRef, Library};
use crate::ui::state::ModelicaDocumentRegistry;
use std::path::{PathBuf};

pub mod types;
pub mod cache;
pub mod scanner;
pub mod render;

pub use types::{PackageNode, InMemoryEntry};
pub use cache::PackageTreeCache;
pub use render::PackageBrowserPanel;
pub use scanner::{scan_twin_folder, discover_third_party_libs};

pub struct PackageBrowserPlugin;

impl Plugin for PackageBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PackageTreeCache>()
            .add_systems(Update, handle_package_loading_tasks);
    }
}

pub fn handle_package_loading_tasks(
    mut cache: ResMut<PackageTreeCache>,
) {
    use futures_lite::future;

    let mut finished_results = Vec::new();
    cache.tasks.retain_mut(|task| {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            finished_results.push(result);
            false
        } else {
            true
        }
    });

    for result in finished_results {
        find_and_update_node(&mut cache.roots, &result.parent_id, result.children);
    }

    if let Some(mut task) = cache.twin_scan_task.take() {
        if let Some(scanned) = future::block_on(future::poll_once(&mut task)) {
            cache.twin = Some(scanned);
        } else {
            cache.twin_scan_task = Some(task);
        }
    }
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

/// Render the children of one named root in [`PackageTreeCache::roots`]
/// inside the Twin panel's per-library section. Lazily kicks off the
/// first scan if the root hasn't been populated yet; subsequent
/// renders walk the cached tree.
pub fn render_root_subtree(world: &mut World, ui: &mut egui::Ui, root_id: &str) {
    use bevy::tasks::AsyncComputeTaskPool;

    let active_path_str = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
        .and_then(|d| crate::ui::state::display_name_for(world, d));
    let active_path = active_path_str.as_deref();
    let theme = world
        .get_resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);

    let mut action: Option<render::PackageAction> = None;
    {
        let mut cache = world.resource_mut::<PackageTreeCache>();
        let cache = &mut *cache;
        let Some(root) = cache
            .roots
            .iter_mut()
            .find(|r| matches!(r, PackageNode::Category { id, .. } if id == root_id))
        else {
            return;
        };
        if let PackageNode::Category {
            id, package_path, fs_path, children, is_loading, ..
        } = root
        {
            ui.set_max_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            if let Some(kids) = children {
                for kid in kids.iter_mut() {
                    if let Some(a) = render::render_node_single(
                        kid, ui, active_path, None, 0, &mut cache.tasks, &theme,
                    ) {
                        action = Some(a);
                    }
                }
            } else if !*is_loading {
                *is_loading = true;
                let pool = AsyncComputeTaskPool::get();
                let parent_id = id.clone();
                let scan_dir = fs_path.clone();
                let pkg_path = package_path.clone();
                let task = pool.spawn(async move {
                    let children = crate::ui::panels::package_browser::scanner::scan_msl_dir(
                        &scan_dir, pkg_path,
                    );
                    crate::ui::panels::package_browser::cache::ScanResult { parent_id, children }
                });
                cache.tasks.push(task);
            }
            if *is_loading {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new("⌛ Loading...")
                            .size(10.0)
                            .italics()
                            .color(egui::Color32::GRAY),
                    );
                });
            }
        }
    }

    if let Some(render::PackageAction::Open(id, _name, _lib, pinned)) = action {
        if let Some(class) = ClassRef::parse_tree_id(&id) {
            open_class(world, class, pinned);
        } else if let Some(class) = resolve_mem_id(world, &id) {
            open_class(world, class, pinned);
        } else {
            bevy::log::warn!("[PackageBrowser] unparseable tree id `{id}`");
        }
    } else if let Some(render::PackageAction::DragStart { msl_path }) = action {
        if let Some(def) = crate::visual_diagram::msl_class_by_path(&msl_path) {
            world
                .get_resource_or_insert_with::<crate::ui::panels::palette::ComponentDragPayload>(
                    Default::default,
                )
                .def = Some(def);
        }
    }
}

/// Single entry point for "open a Modelica class in the workbench".
///
/// Replaces the legacy `open_model` / `open_bundled_in_world` /
/// per-scheme branches with one dispatch on [`Library`]. Every UI
/// gesture (tree click, palette drop, typed command, session
/// restore) translates its intent into a [`ClassRef`] and calls
/// this function — there is no second code path that can disagree
/// about how a given `ClassRef` should load, dedupe, or drill in.
///
/// Loading strategy by library:
/// - [`Library::Msl`] / [`Library::ThirdParty`]: slim-slice load via
///   [`drill_into_class`]. Extracts the target class (~5–10 KB)
///   instead of parsing the wrapper package file (often 100+ KB),
///   so the canvas paints in well under a second.
/// - [`Library::Bundled`]: in-memory source from
///   [`crate::models::get_model`]; cheap, eager whole-file load
///   because bundled files are small by design.
/// - [`Library::UserFile`]: full file read; the user's source is
///   the canvas of authority, slim slices would lose context for
///   sibling-class references.
/// - [`Library::Untitled`]: focus the existing tab for the doc id;
///   there's no source to load.
pub(crate) fn open_class(world: &mut World, class: ClassRef, pinned: bool) {
    let _ = pinned; // VS Code preview/pin semantics — wired through later.
    match &class.library {
        Library::Msl | Library::ThirdParty { .. } => {
            // The slim-slice drill-in path is exactly what we need
            // for system libraries: it owns the file lookup, the
            // class-slice extraction, the tab plumbing, and the
            // `DocumentOpenings` busy state. Pass the absolute qualified
            // name so its `library_fs::resolve_class_path_indexed`
            // can find the owning .mo file.
            crate::ui::panels::canvas_diagram::drill_into_class(world, &class.qualified());
        }
        Library::Bundled => {
            open_bundled_class(world, &class);
        }
        Library::UserFile { path } => {
            open_user_file_class(world, path.clone(), &class);
        }
        Library::Untitled(doc_id) => {
            focus_existing_doc_tab(world, *doc_id, class.qualified());
        }
    }
}

/// Resolve a legacy `mem://<name>` tree id to a [`ClassRef`] by
/// consulting [`PackageTreeCache::in_memory_models`]. Lives here
/// rather than in [`ClassRef::parse_tree_id`] because the mapping
/// from name → `DocumentId` requires world state the parser doesn't
/// own.
pub(crate) fn resolve_mem_id(world: &World, id: &str) -> Option<ClassRef> {
    let mem_name = id.strip_prefix("mem://")?;
    let entry = world
        .resource::<PackageTreeCache>()
        .in_memory_models
        .iter()
        .find(|e| e.id == id || e.display_name == mem_name)?;
    Some(ClassRef::untitled(entry.doc, [mem_name.to_string()]))
}

fn open_bundled_class(world: &mut World, class: &ClassRef) {
    use crate::ui::panels::model_view::MODEL_VIEW_KIND;
    use bevy::tasks::AsyncComputeTaskPool;

    let filename = match class.path.first() {
        Some(stem) => format!("{stem}.mo"),
        None => return, // Library root click — no class to open.
    };
    let drilled = class.qualified();
    let drilled_for_tab = if class.path.len() > 1 { Some(drilled.clone()) } else { None };

    // Dedup: same bundled file already loaded → reuse the doc, just
    // ensure a tab keyed on the new drill target.
    let already_open = world.resource::<ModelicaDocumentRegistry>().find_bundled(&filename);
    if let Some(doc) = already_open {
        let tab_id = world
            .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
            .ensure_for(doc, drilled_for_tab.clone());
        world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });
        return;
    }

    let reserved_doc_id = world.resource_mut::<ModelicaDocumentRegistry>().reserve_id();
    let tab_id = world
        .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
        .ensure_for(reserved_doc_id, drilled_for_tab.clone());
    world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });

    let display_name = class.short_name().to_string();
    let origin = lunco_doc::DocumentOrigin::Bundled { filename: filename.clone() };
    let filename_for_task = filename.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let result = match crate::models::get_model(&filename_for_task) {
            Some(source_text) => Ok(crate::document::ModelicaDocument::with_origin(
                reserved_doc_id,
                source_text.to_string(),
                origin,
            )),
            None => Err(format!(
                "Bundled model not found: {filename_for_task}"
            )),
        };
        crate::ui::panels::package_browser::cache::FileLoadResult {
            doc_id: reserved_doc_id,
            result,
        }
    });
    // Mint a `StatusBus` handle BEFORE inserting into `DocumentOpenings`
    // so the canvas overlay sees the doc as busy from the very first
    // frame after the user clicked open. Handed off to the projection
    // stage by `drive_file_load_openings` (see that fn).
    let busy = world
        .resource_mut::<lunco_workbench::status_bus::StatusBus>()
        .begin(
            lunco_workbench::status_bus::BusyScope::Document(reserved_doc_id.0),
            "opening",
            format!("Loading {display_name}…"),
        );
    world
        .resource_mut::<crate::ui::document_openings::DocumentOpenings>()
        .insert(
            reserved_doc_id,
            crate::ui::document_openings::OpeningState::FileLoad {
                display_name,
                task,
                busy,
            },
        );
}

fn open_user_file_class(world: &mut World, path: PathBuf, class: &ClassRef) {
    use crate::ui::panels::model_view::{MODEL_VIEW_KIND, ModelViewMode};
    use bevy::tasks::AsyncComputeTaskPool;

    let drilled = if class.path.is_empty() { None } else { Some(class.qualified()) };
    // Non-`.mo` files have no Modelica classes to render in Canvas
    // mode — default the tab to Text mode so the user sees the raw
    // file contents instead of an empty diagram.
    let initial_mode = if path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| e.eq_ignore_ascii_case("mo"))
        .unwrap_or(false)
    {
        None
    } else {
        Some(ModelViewMode::Text)
    };
    let already_open = world.resource::<ModelicaDocumentRegistry>().find_by_path(&path);
    if let Some(doc) = already_open {
        // Re-Opening an already-open file reloads it from disk so external
        // edits (an editor, a tool, an agent writing the `.mo`) are picked up
        // — previously this just focused the stale tab. Read synchronously
        // (user-initiated, small file) and apply through the op pipeline so
        // canvas/plots/compile reproject; skip if the buffer already matches.
        if let Ok(disk) = std::fs::read_to_string(&path) {
            let differs = world
                .resource::<ModelicaDocumentRegistry>()
                .host(doc)
                .map(|h| h.document().source() != disk)
                .unwrap_or(false);
            if differs {
                use crate::document::ModelicaOp;
                match crate::ui::panels::canvas_diagram::apply_one_op_as(
                    world,
                    doc,
                    ModelicaOp::ReplaceSource { new: disk },
                    lunco_twin_journal::AuthorTag::for_tool("open-file-reload"),
                ) {
                    Ok(_) => bevy::log::info!(
                        "[OpenFile] reloaded `{}` from disk",
                        path.display()
                    ),
                    Err(e) => bevy::log::warn!(
                        "[OpenFile] reload-from-disk failed for {}: {e:?}",
                        path.display()
                    ),
                }
            }
        }
        let tab_id = world
            .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
            .ensure_for(doc, drilled.clone());
        if let Some(mode) = initial_mode {
            world
                .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
                .set_view_mode(tab_id, mode);
        }
        world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });
        return;
    }

    let reserved_doc_id = world.resource_mut::<ModelicaDocumentRegistry>().reserve_id();
    let tab_id = world
        .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
        .ensure_for(reserved_doc_id, drilled.clone());
    if let Some(mode) = initial_mode {
        world
            .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
            .set_view_mode(tab_id, mode);
    }
    world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });

    let display_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Opened").to_string();
    let origin = lunco_doc::DocumentOrigin::File { path: path.clone(), writable: true };
    let path_for_task = path.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let result = std::fs::read_to_string(&path_for_task)
            .map(|source_text| {
                crate::document::ModelicaDocument::with_origin(
                    reserved_doc_id,
                    source_text,
                    origin,
                )
            })
            .map_err(|e| format!("Failed to read {}: {e}", path_for_task.display()));
        crate::ui::panels::package_browser::cache::FileLoadResult {
            doc_id: reserved_doc_id,
            result,
        }
    });
    // Mint a `StatusBus` handle BEFORE inserting; handed off to the
    // projection stage in `drive_file_load_openings`. See the matching
    // block in `open_bundled_file_class`.
    let busy = world
        .resource_mut::<lunco_workbench::status_bus::StatusBus>()
        .begin(
            lunco_workbench::status_bus::BusyScope::Document(reserved_doc_id.0),
            "opening",
            format!("Loading {display_name}…"),
        );
    world
        .resource_mut::<crate::ui::document_openings::DocumentOpenings>()
        .insert(
            reserved_doc_id,
            crate::ui::document_openings::OpeningState::FileLoad {
                display_name,
                task,
                busy,
            },
        );
}

fn focus_existing_doc_tab(world: &mut World, doc: lunco_doc::DocumentId, qualified: String) {
    use crate::ui::panels::model_view::MODEL_VIEW_KIND;
    let drilled = if qualified.is_empty() { None } else { Some(qualified) };
    let tab_id = world
        .resource_mut::<crate::ui::panels::model_view::ModelTabs>()
        .ensure_for(doc, drilled);
    world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });
}

