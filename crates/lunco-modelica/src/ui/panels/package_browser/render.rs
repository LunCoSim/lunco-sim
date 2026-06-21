//! UI rendering for the Package Browser egui panel.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::state::{ModelLibrary};
use crate::package_tree::types::{PackageNode, TwinNode};
use crate::package_tree::cache::{PackageTreeCache, ScanResult};

pub struct PackageBrowserPanel;

#[derive(Clone)]
pub enum PackageAction {
    Open(String, String, ModelLibrary, bool),
    DragStart { msl_path: String },
}

impl Panel for PackageBrowserPanel {
    fn id(&self) -> PanelId { PanelId("modelica_package_browser") }
    fn title(&self) -> String { "📚 Package Browser".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let active_path_str = world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
            .and_then(|d| crate::state::display_name_for(world, d));
        let active_path = active_path_str.as_deref();
        
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);

        let mut action: Option<PackageAction> = None;
        let mut reopen_in_memory: Option<String> = None;
        let mut create_new = false;
        let mut open_twin_picker = false;
        let mut close_twin = false;

        {
            let mut tree_cache = world.resource_mut::<PackageTreeCache>();
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let cache = &mut *tree_cache;

                let twin_label = if let Some(twin) = cache.twin.as_ref() {
                    twin.root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| twin.root.display().to_string())
                } else {
                    "No folder".to_string()
                };
                section_header(ui, &twin_label, |ui| {
                    if ui.small_button("➕").clicked() { create_new = true; }
                    if cache.twin_scan_task.is_some() {
                        ui.spinner();
                    } else if cache.twin.is_some() {
                        if ui.small_button("✕").clicked() { close_twin = true; }
                    } else if ui.small_button("📁").clicked() { open_twin_picker = true; }
                });

                if let Some(twin) = cache.twin.as_mut() {
                    for kid in &mut twin.root_node.children {
                        if let Some(a) = render_twin_node(kid, ui, active_path, &theme) {
                            action = Some(a);
                        }
                    }
                }

                if !cache.in_memory_models.is_empty() {
                    ui.add_space(8.0);
                    section_header(ui, "Your Models", |_| {});
                    for entry in &cache.in_memory_models {
                        let is_active = active_path == Some(&entry.display_name);
                        if ui.selectable_label(is_active, format!("📄 {}", entry.display_name)).clicked() {
                            reopen_in_memory = Some(entry.id.clone());
                        }
                    }
                }

                for root in &mut cache.roots {
                    if let Some(a) = render_node_single(root, ui, active_path, None, 0, &mut cache.tasks, &theme) {
                        action = Some(a);
                    }
                }
            });
        }

        if create_new {
            world.commands().trigger(crate::ui::commands::CreateNewScratchModel::default());
        }
        if open_twin_picker {
            world.commands().trigger(lunco_workbench::picker::PickHandle {
                mode: lunco_workbench::picker::PickMode::OpenFolder,
                on_resolved: lunco_workbench::picker::PickFollowUp::OpenTwin,
            });
        }
        if close_twin {
            world.resource_mut::<PackageTreeCache>().twin = None;
        }
        if let Some(id) = reopen_in_memory {
            world.commands().trigger(lunco_workbench::file_ops::OpenFile { path: id });
        }
        if let Some(a) = action {
            match a {
                PackageAction::Open(id, _name, _lib, pinned) => {
                    if let Some(class) = crate::class_ref::ClassRef::parse_tree_id(&id) {
                        super::open_class(world, class, pinned);
                    } else if let Some(class) = super::resolve_mem_id(world, &id) {
                        super::open_class(world, class, pinned);
                    } else {
                        bevy::log::warn!("[PackageBrowser] unparseable tree id `{id}`");
                    }
                }
                PackageAction::DragStart { msl_path } => {
                    if let Some(def) = crate::visual_diagram::msl_class_by_path(&msl_path) {
                        world.get_resource_or_insert_with::<crate::ui::panels::palette::ComponentDragPayload>(Default::default).def = Some(def);
                    }
                }
            }
        }
    }
}

fn section_header(ui: &mut egui::Ui, title: &str, buttons: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title.to_uppercase()).strong().size(11.0).color(egui::Color32::GRAY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons);
    });
}

fn render_twin_node(
    node: &mut TwinNode,
    ui: &mut egui::Ui,
    active_path: Option<&str>,
    theme: &lunco_theme::Theme,
) -> Option<PackageAction> {
    if node.is_modelica {
        let is_active = active_path == Some(node.name.as_str());
        let mut label = egui::RichText::new(format!("📄 {}", node.name));
        if is_active {
            label = label.strong().color(theme.tokens.accent);
        }
        if ui.selectable_label(is_active, label).clicked() {
            return Some(PackageAction::Open(
                format!("file://{}", node.path.display()),
                node.name.clone(),
                ModelLibrary::User,
                ui.input(|i| i.modifiers.command),
            ));
        }
        None
    } else {
        let mut action = None;
        egui::CollapsingHeader::new(format!("📁 {}", node.name))
            .id_salt(node.path.to_string_lossy().to_string())
            .show(ui, |ui| {
                for kid in &mut node.children {
                    if let Some(a) = render_twin_node(kid, ui, active_path, theme) {
                        action = Some(a);
                    }
                }
            });
        action
    }
}

pub(crate) fn render_node_single(
    node: &mut PackageNode,
    ui: &mut egui::Ui,
    active_path: Option<&str>,
    active_drill: Option<&str>,
    depth: usize,
    tasks: &mut Vec<bevy::tasks::Task<ScanResult>>,
    theme: &lunco_theme::Theme,
) -> Option<PackageAction> {
    let mut action = None;

    match node {
        PackageNode::Category { id, name, package_path, fs_path: _, children, is_loading } => {
            let header_resp = egui::CollapsingHeader::new(format!("📁 {}", name))
                .id_salt(id.as_str())
                .show(ui, |ui| {
                    if let Some(kids) = children {
                        for kid in kids {
                            if let Some(a) = render_node_single(kid, ui, active_path, active_drill, depth + 1, tasks, theme) {
                                action = Some(a);
                            }
                        }
                    } else if !*is_loading {
                        *is_loading = true;
                        let pool = bevy::tasks::AsyncComputeTaskPool::get();
                        let pid = id.clone();
                        let pp = package_path.clone();
                        tasks.push(pool.spawn(async move {
                            let kids = crate::package_tree::library_tree::library_tree().children(&pp);
                            ScanResult { parent_id: pid, children: kids }
                        }));
                    }
                    if *is_loading {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label("⌛ Loading...");
                        });
                    }
                });
            let _ = header_resp;
        }
        PackageNode::Model { id, name, library, class_kind } => {
            let is_active = active_path == Some(name.as_str());
            let row = ui.horizontal(|ui| {
                if let Some(kind) = *class_kind {
                    let badge = crate::ui::browser_section::type_badge_for_kind(kind, theme);
                    crate::ui::browser_section::paint_badge(ui, badge, theme);
                } else {
                    let icon = match library {
                        crate::state::ModelLibrary::MSL => "?",
                        crate::state::ModelLibrary::Bundled => "📦",
                        crate::state::ModelLibrary::User => "📁",
                        crate::state::ModelLibrary::InMemory => "💾",
                    };
                    ui.label(egui::RichText::new(icon).size(11.0));
                }
                let mut label = egui::RichText::new(name.as_str());
                if is_active {
                    label = label.strong().color(theme.tokens.accent);
                }
                ui.add(egui::Label::new(label).selectable(false).sense(egui::Sense::click()))
            });
            let mut resp = row.inner;
            if resp.clicked() {
                action = Some(PackageAction::Open(id.clone(), name.clone(), library.clone(), ui.input(|i| i.modifiers.command)));
            }
            if let Some(kind) = class_kind {
                resp = resp.on_hover_text(format!("Kind: {}", kind.as_keyword()));
            }

            if matches!(library, crate::state::ModelLibrary::MSL) {
                let msl_path = id.strip_prefix("msl_path:").unwrap_or(id).to_string();
                if ui.rect_contains_pointer(resp.rect) && ui.input(|i| i.pointer.any_down()) {
                    action = Some(PackageAction::DragStart { msl_path });
                }
            }
        }
    }

    action
}
