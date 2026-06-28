//! User interaction handlers: menus, DND, and clicks.

use bevy_egui::egui;
use lunco_canvas::{Pos as CanvasPos, Rect as CanvasRect};
use lunco_workbench::PanelCtx;
use crate::document::ModelicaOp;
use crate::state::{ModelicaDocumentRegistry};
use super::super::{CanvasDiagramState, ContextMenuTarget, PendingContextMenu, ICON_W, ops, menus};
use super::super::loads::drill_into_class;

pub(crate) fn handle_context_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    response: &egui::Response,
    doc_id: Option<lunco_doc::DocumentId>,
    render_tab_id: Option<crate::model_tabs_types::TabId>,
    tab_read_only: bool,
    editing_class: Option<&str>,
) -> Vec<ModelicaOp> {
    let active_doc = doc_id;
    let screen_rect = CanvasRect::from_min_max(
        CanvasPos::new(response.rect.min.x, response.rect.min.y),
        CanvasPos::new(response.rect.max.x, response.rect.max.y),
    );
    let popup_was_open_before = egui::Popup::is_any_open(ui.ctx());
    let mut suppress_menu = tab_read_only;

    if tab_read_only {
        let our_menu_cached = state.get(active_doc).context_menu.is_some();
        if our_menu_cached {
            egui::Popup::close_all(ui.ctx());
            state.get_mut(active_doc).context_menu = None;
        }
    }

    if !tab_read_only && response.secondary_clicked() {
        let press = ui.ctx().input(|i| i.pointer.press_origin());
        if let Some(p) = press.or_else(|| response.interact_pointer_pos()) {
            let our_menu_open = popup_was_open_before && state.get(active_doc).context_menu.is_some();
            if our_menu_open {
                state.get_mut(active_doc).context_menu = None;
                egui::Popup::close_all(ui.ctx());
                suppress_menu = true;
            } else {
                if popup_was_open_before { egui::Popup::close_all(ui.ctx()); }
                let (world_pos, target) = {
                    let docstate = match render_tab_id { Some(t) => state.get_for_tab(t), None => state.get(active_doc) };
                    let world_pos = docstate.canvas.viewport.screen_to_world(CanvasPos::new(p.x, p.y), screen_rect);
                    let hit_node = docstate.canvas.scene.hit_node(world_pos, 6.0);
                    let hit_edge = docstate.canvas.scene.hit_edge_kind(world_pos, 4.0, true, 5.0);
                    let target = match (hit_node, hit_edge) {
                        (Some((id, _)), _) => ContextMenuTarget::Node(id),
                        (_, Some((id, kind))) => ContextMenuTarget::Edge(id, kind),
                        _ => ContextMenuTarget::Empty,
                    };
                    (world_pos, target)
                };
                state.get_mut(active_doc).context_menu = Some(PendingContextMenu {
                    screen_pos: p,
                    world_pos,
                    target,
                });
            }
        }
    }

    if suppress_menu {
        Vec::new()
    } else {
        let mut collected = Vec::new();
        let cached = state.get(active_doc).context_menu.clone();
        response.context_menu(|ui| {
            let Some(menu) = cached.as_ref() else {
                ui.label("(no click target)");
                return;
            };
            match &menu.target {
                ContextMenuTarget::Node(id) => {
                    menus::render_node_menu(ui, ctx, state, *id, editing_class, &mut collected);
                }
                ContextMenuTarget::Edge(id, kind) => {
                    menus::render_edge_menu(ui, ctx, state, *id, *kind, menu.world_pos, editing_class, &mut collected);
                }
                ContextMenuTarget::Empty => {
                    menus::render_empty_menu(ui, ctx, state, menu.world_pos, editing_class, &mut collected);
                }
            }
        });

        let popup_open_now = egui::Popup::is_any_open(ui.ctx());
        if !popup_open_now && state.get(active_doc).context_menu.is_some() {
            state.get_mut(active_doc).context_menu = None;
        }
        collected
    }
}

pub(crate) fn handle_drag_and_drop(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    response: &egui::Response,
    doc_id: Option<lunco_doc::DocumentId>,
    render_tab_id: Option<crate::model_tabs_types::TabId>,
    tab_read_only: bool,
    editing_class: Option<String>,
) {
    let active_doc = doc_id;
    let drag_payload_def = ctx.resource::<crate::ui::panels::palette::ComponentDragPayload>().and_then(|p| p.def.clone());
    if let Some(def) = drag_payload_def {
        let hover_pos = response.hover_pos();
        if let Some(p) = hover_pos {
            let painter = ui.painter_at(response.rect);
            let zoom = state.get(active_doc).canvas.viewport.zoom;
            let half = (ICON_W * zoom * 0.5).max(12.0);
            let ghost_rect = egui::Rect::from_center_size(p, egui::vec2(half * 2.0, half * 2.0));
            let accent = egui::Color32::from_rgb(120, 180, 255);
            painter.rect_filled(ghost_rect, 4.0, egui::Color32::from_rgba_unmultiplied(120, 180, 255, 50));
            painter.rect_stroke(ghost_rect, 4.0, egui::Stroke::new(1.5, accent), egui::StrokeKind::Outside);
            painter.text(egui::pos2(ghost_rect.center().x, ghost_rect.max.y + 4.0), egui::Align2::CENTER_TOP, def.short_name(), egui::FontId::proportional(11.0), accent);
            ui.ctx().request_repaint();
        }

        if ui.input(|i| i.pointer.any_released()) {
            let drop_target = hover_pos.filter(|p| response.rect.contains(*p));
            if let (Some(p), Some(doc_id)) = (drop_target, active_doc) {
                if !tab_read_only {
                    let screen_rect = CanvasRect::from_min_max(
                        CanvasPos::new(response.rect.min.x, response.rect.min.y),
                        CanvasPos::new(response.rect.max.x, response.rect.max.y),
                    );
                    let click_world = state.get(active_doc).canvas.viewport.screen_to_world(CanvasPos::new(p.x, p.y), screen_rect);
                    let class = editing_class.unwrap_or_else(|| {
                        ctx.resource::<crate::model_tabs::ModelTabs>()
                            .and_then(|t| t.drilled_class_for_doc(doc_id))
                            .or_else(|| {
                                let registry = ctx.resource::<ModelicaDocumentRegistry>()?;
                                let host = registry.host(doc_id)?;
                                let ast = host.document().strict_ast()?;
                                crate::ast_extract::extract_model_name_from_ast(&ast)
                            })
                            .unwrap_or_default()
                    });
                    if !class.is_empty() {
                        let instance_name = {
                            ops::pick_add_instance_name(&def, &match render_tab_id { Some(t) => state.get_for_tab(t), None => state.get(Some(doc_id)) }.canvas.scene)
                        };
                        let op = ops::op_add_component_with_name(&def, &instance_name, click_world, &class);
                        ctx.defer(move |world| super::super::ops::apply_ops_public(world, doc_id, vec![op]));
                        ui.ctx().request_repaint();
                    }
                }
            }
            ctx.defer(|world| {
                if let Some(mut payload) = world.get_resource_mut::<crate::ui::panels::palette::ComponentDragPayload>() {
                    payload.def = None;
                }
            });
        }
    }
}

pub(crate) fn handle_node_double_click(
    ctx: &mut PanelCtx,
    state: &CanvasDiagramState,
    events: &[lunco_canvas::SceneEvent],
    doc_id: Option<lunco_doc::DocumentId>,
) {
    for ev in events {
        if let lunco_canvas::SceneEvent::NodeDoubleClicked { id } = ev {
            let type_name = {
                state.get(doc_id).canvas.scene.node(*id)
                    .and_then(|n| n.data.downcast_ref::<super::super::node::IconNodeData>())
                    .map(|d| d.qualified_type.clone())
            };
            if let Some(qualified) = type_name {
                ctx.defer(move |world| drill_into_class(world, &qualified));
            }
        }
    }
}
