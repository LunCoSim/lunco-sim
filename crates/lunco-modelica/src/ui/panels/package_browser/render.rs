//! UI rendering helpers for the Modelica section of the Twin Browser.

use crate::package_tree::types::PackageNode;
use crate::state::ModelLibrary;
use bevy_egui::egui;

#[derive(Clone)]
pub(super) enum PackageAction {
    Open(String, String, ModelLibrary, bool),
    DragStart { msl_path: String },
}

/// Render helper for callers that hold a
/// `&PackageTreeCache` (no `&mut`) — e.g. the Twin Browser's
/// [`BrowserCtx`](lunco_workbench::BrowserCtx), which can't take the
/// cache mutably. Instead of mutating `is_loading` / pushing scan
/// tasks in place, an unscanned Category pushes its `(id, package_path)`
/// into `load_out`; the caller schedules the scan through the package-tree
/// owner. Egui
/// owns the expand/collapse state (CollapsingHeader id_salt), so the
/// read-only render still expands correctly.
pub(crate) fn render_node_single_ro(
    node: &PackageNode,
    ui: &mut egui::Ui,
    active_path: Option<&str>,
    _active_drill: Option<&str>,
    _depth: usize,
    load_out: &mut Vec<(String, String)>,
    theme: &lunco_theme::Theme,
) -> Option<PackageAction> {
    let mut action = None;
    match node {
        PackageNode::Category {
            id,
            name,
            package_path,
            fs_path: _,
            children,
            is_loading,
        } => {
            let header_resp =
                egui::CollapsingHeader::new(name)
                    .id_salt(id.as_str())
                    .show(ui, |ui| {
                        if let Some(kids) = children {
                            for kid in kids {
                                if let Some(a) = render_node_single_ro(
                                    kid,
                                    ui,
                                    active_path,
                                    _active_drill,
                                    _depth + 1,
                                    load_out,
                                    theme,
                                ) {
                                    action = Some(a);
                                }
                            }
                        } else {
                            // Unscanned: request a lazy scan (the caller
                            // defers the AsyncComputeTaskPool spawn) and show
                            // a loading row until the `ScanResult` lands.
                            load_out.push((id.clone(), package_path.clone()));
                        }
                        if children.is_none() || *is_loading {
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label("⌛ Loading...");
                            });
                        }
                    });
            let _ = header_resp;
        }
        PackageNode::Model {
            id,
            name,
            library,
            class_kind,
        } => {
            let is_active = active_path == Some(name.as_str());
            let row = ui.horizontal(|ui| {
                if let Some(kind) = *class_kind {
                    let badge = crate::ui::browser_section::type_badge_for_kind(kind, theme);
                    crate::ui::browser_section::paint_badge(ui, badge, theme);
                } else {
                    let icon = match library {
                        crate::state::ModelLibrary::MSL => "?",
                        crate::state::ModelLibrary::Bundled => "Bundled",
                        crate::state::ModelLibrary::User => "User",
                        crate::state::ModelLibrary::InMemory => "Memory",
                    };
                    ui.label(egui::RichText::new(icon).size(11.0));
                }
                let mut label = egui::RichText::new(name.as_str());
                if is_active {
                    label = label.strong().color(theme.tokens.accent);
                }
                ui.add(
                    egui::Label::new(label)
                        .selectable(false)
                        .sense(egui::Sense::click()),
                )
            });
            let mut resp = row.inner;
            if resp.clicked() {
                action = Some(PackageAction::Open(
                    id.clone(),
                    name.clone(),
                    library.clone(),
                    ui.input(|i| i.modifiers.command),
                ));
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
