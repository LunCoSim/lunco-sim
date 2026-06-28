//! Canvas diagram context menus + plot-node insertion.
//!
//! Right-click handlers that render the per-target context menu over
//! a node, edge, plot widget, or empty canvas. All menu actions
//! mutate panel state via the shared `CanvasDiagramState` resource
//! and emit `ModelicaOp` writes through `apply_ops_public` from the
//! parent module.

use bevy_egui::egui;
use lunco_workbench::PanelCtx;

use crate::document::ModelicaOp;

use super::ops::{component_headers, op_remove_component, op_remove_edge};
use super::palette::{self, PaletteSettings};
use super::{CanvasDiagramState, active_doc_from_world_ctx};
use crate::model_tabs_types::TabRenderContext;

/// Build a `SetConnectionLine` op from the current edge's waypoints
/// after applying `mutate` to a fresh copy. `mutate` may insert,
/// remove, or move waypoints freely (canvas coords). Returns `None`
/// if the edge or its endpoints can't be resolved. Used by the
/// right-click bend insert/delete entries.
fn op_modify_waypoints(
    ctx: &PanelCtx,
    state: &CanvasDiagramState,
    edge_id: lunco_canvas::EdgeId,
    class: &str,
    mutate: impl FnOnce(&mut Vec<lunco_canvas::Pos>),
) -> Option<ModelicaOp> {
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id(ctx);
    let scene = &state.get_for_render(tab, active_doc).canvas.scene;
    let edge = scene.edge(edge_id)?;
    let from_node = scene.node(edge.from.node)?;
    let to_node = scene.node(edge.to.node)?;
    let from_instance = from_node.origin.clone()?;
    let to_instance = to_node.origin.clone()?;
    let mut pts: Vec<lunco_canvas::Pos> = edge.waypoints.clone();
    mutate(&mut pts);
    // Canvas Y is +down; Modelica is +up. Flip on the way to source.
    let modelica_pts: Vec<(f32, f32)> = pts.iter().map(|p| (p.x, -p.y)).collect();
    Some(ModelicaOp::SetConnectionLine {
        class: class.to_string(),
        from: crate::pretty::PortRef::new(&from_instance, edge.from.port.as_str()),
        to: crate::pretty::PortRef::new(&to_instance, edge.to.port.as_str()),
        points: modelica_pts,
    })
}

/// Read the active tab id from `TabRenderContext`. `None` when called
/// outside a panel render call (observers, off-render systems);
/// callers fall back to first-tab semantics in that case via
/// `CanvasDiagramState::get_for_render`.
fn render_tab_id(ctx: &PanelCtx) -> Option<crate::model_tabs_types::TabId> {
    ctx
        .resource::<TabRenderContext>()
        .and_then(|c| c.tab_id)
}

pub(super) fn render_node_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    id: lunco_canvas::NodeId,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    // Plot nodes are scene-only (no Modelica counterpart) — show a
    // signal-binding submenu and a Delete entry, skip the component-
    // specific actions (Open class, Parameters, Duplicate).
    let node_kind: Option<String> = {
        let active_doc = active_doc_from_world_ctx(ctx);
        let tab = render_tab_id(ctx);
        state
            .get_for_render(tab, active_doc)
            .canvas
            .scene
            .node(id)
            .map(|n| n.kind.to_string())
    };
    if node_kind.as_deref()
        == Some(lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND)
    {
        render_plot_node_menu(ui, ctx, state, id);
        return;
    }
    let (instance, type_name) = component_headers(ctx, state, id);
    ui.label(egui::RichText::new(&instance).strong());
    if !type_name.is_empty() {
        ui.label(egui::RichText::new(&type_name).weak().small());
    }
    ui.separator();
    if ui.button("✂ Delete").on_hover_text("Remove this component from the diagram").clicked() {
        if let Some(class) = editing_class {
            if let Some(op) = op_remove_component(ctx, state, id, class) {
                out.push(op);
                // Optimistic scene mutation — `apply_ops` will then
                // bump `canvas_acked_gen` and the project gate skips
                // the redundant reproject.
                let active_doc = active_doc_from_world_ctx(ctx);
                let tab = render_tab_id(ctx);
                let docstate = state.get_mut_for_render(tab, active_doc);
                docstate.canvas.scene.remove_node(id);
            }
        }
        ui.close();
    }
    if ui.button("📋 Duplicate").on_hover_text("Create a copy of this component").clicked() {
        ui.close();
    }
    ui.separator();
    if ui.button("↧ Open class").on_hover_text("Open this component's class definition").clicked() {
        ui.close();
    }
    if ui.button("🔧 Parameters…").on_hover_text("Edit this component's parameters").clicked() {
        ui.close();
    }
}

/// Collect plottable scalar signals — every signal in the registry
/// that the owning `ModelicaModel` does not classify as a parameter
/// or input. Both maps are populated at compile time by walking the
/// document's [`crate::index::ModelicaIndex`] (variability /
/// causality on each `ComponentEntry`), so this is a free lookup —
/// no DAE introspection or runtime variance heuristic required.
fn collect_varying_signals(
    ctx: &PanelCtx,
) -> Vec<(bevy::prelude::Entity, String)> {
    use bevy::prelude::Entity;
    // A document's variables are registered in `SignalRegistry` under TWO
    // entities: the live cosim entity (`ModelicaDocumentRegistry`) and the
    // batch / Fast-Run playback entity (`PlaybackEntities`). Enumerating the
    // whole registry therefore lists every path once per entity — the
    // duplicate-variables bug. Resolve the active doc to its single canonical
    // signal entity using the SAME precedence the plot data-fetch uses
    // (`snapshots::stash_snapshots` doc→entity: the registry sim entity, lowest
    // bits, wins; else the playback entity), then read only that entity's
    // signals so the picker matches exactly what a doc-bound plot will show.
    let bound_entity: Option<Entity> = active_doc_from_world_ctx(ctx).and_then(|d| {
        let live = ctx
            .resource::<crate::state::ModelicaDocumentRegistry>()
            .and_then(|reg| {
                reg.iter_doc_for_entity()
                    .filter(|(_, dd)| *dd == d)
                    .map(|(e, _)| e)
                    .min_by_key(|e| e.to_bits())
            });
        let playback = ctx
            .resource::<crate::experiments_runner::PlaybackEntities>()
            .and_then(|p| p.0.get(&d).copied());
        live.or(playback)
    });
    let signals: Vec<(Entity, String)> = ctx
        .resource::<lunco_viz::SignalRegistry>()
        .map(|r| {
            r.iter_scalar()
                .filter(|(s, _)| bound_entity.map_or(true, |be| s.entity == be))
                .map(|(s, _)| (s.entity, s.path.clone()))
                .collect()
        })
        .unwrap_or_default();
    let mut v: Vec<_> = signals
        .into_iter()
        .filter(|(entity, path)| {
            ctx
                .get::<crate::ModelicaModel>(*entity)
                .map(|m| !m.parameters.contains_key(path) && !m.inputs.contains_key(path))
                .unwrap_or(true)
        })
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

pub(super) fn render_plot_node_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    id: lunco_canvas::NodeId,
) {
    use lunco_viz::kinds::canvas_plot_node::PlotNodeData;

    let current: PlotNodeData = {
        let active_doc = active_doc_from_world_ctx(ctx);
        let tab = render_tab_id(ctx);
        state
            .get_for_render(tab, active_doc)
            .canvas
            .scene
            .node(id)
            .and_then(|n| n.data.downcast_ref::<PlotNodeData>().cloned())
            .unwrap_or_default()
    };
    ui.label(egui::RichText::new("Plot").strong());
    if !current.signal_path.is_empty() {
        ui.label(
            egui::RichText::new(&current.signal_path)
                .weak()
                .small(),
        );
    } else {
        ui.label(
            egui::RichText::new("(unbound)")
                .weak()
                .small()
                .italics(),
        );
    }
    ui.separator();

    let sigs = collect_varying_signals(ctx);

    ui.menu_button("🔗 Bind signal", |ui| {
        if sigs.is_empty() {
            ui.label(
                egui::RichText::new("(no signals yet — run a simulation)")
                    .weak()
                    .small(),
            );
            return;
        }
        let max_h = ui.ctx().content_rect().height() * 0.7;
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (entity, path) in &sigs {
                    let is_current = current.binding.pinned_entity() == Some(entity.to_bits())
                        && path == &current.signal_path;
                    if ui.selectable_label(is_current, path).clicked() {
                        rebind_plot_node(ctx, state, id, entity.to_bits(), path);
                        ui.close();
                    }
                }
            });
    });

    if !current.signal_path.is_empty()
        && ui.button("Unbind").on_hover_text("Clear this plot's signal binding").clicked()
    {
        rebind_plot_node(ctx, state, id, 0, "");
        ui.close();
    }
    ui.separator();
    if ui.button("✂ Delete").on_hover_text("Remove this plot node from the diagram").clicked() {
        let active_doc = active_doc_from_world_ctx(ctx);
        let tab = render_tab_id(ctx);
        let docstate = state.get_mut_for_render(tab, active_doc);
        docstate.canvas.scene.remove_node(id);
        ui.close();
    }
}

pub(super) fn rebind_plot_node(
    ctx: &PanelCtx,
    state: &mut CanvasDiagramState,
    id: lunco_canvas::NodeId,
    entity_bits: u64,
    signal_path: &str,
) {
    use lunco_viz::kinds::canvas_plot_node::PlotNodeData;
    let payload = PlotNodeData {
        binding: lunco_viz::kinds::canvas_plot_node::PlotBinding::Pinned {
            entity: entity_bits,
        },
        signal_path: signal_path.to_string(),
        title: String::new(),
    };
    let data: lunco_canvas::NodeData = std::sync::Arc::new(payload);
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id(ctx);
    let docstate = state.get_mut_for_render(tab, active_doc);
    if let Some(node) = docstate.canvas.scene.node_mut(id) {
        node.data = data;
    }
}

pub(super) fn render_edge_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    id: lunco_canvas::EdgeId,
    hit: lunco_canvas::EdgeHitKind,
    click_world: lunco_canvas::Pos,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    ui.label(egui::RichText::new("Connection").strong());
    ui.separator();
    // ── Bend insert / delete (hit-kind dependent) ───────────────────
    if let Some(class) = editing_class {
        match hit {
            lunco_canvas::EdgeHitKind::Corner(idx) => {
                if ui.button("✕ Delete bend").on_hover_text("Remove this waypoint from the connection").clicked() {
                    if let Some(op) = op_modify_waypoints(
                        ctx,
                        state,
                        id,
                        class,
                        |pts| {
                            if idx < pts.len() {
                                pts.remove(idx);
                            }
                        },
                    ) {
                        out.push(op);
                    }
                    ui.close();
                }
            }
            lunco_canvas::EdgeHitKind::Segment(seg_idx) => {
                if ui.button("➕ Add bend here").on_hover_text("Insert a waypoint at the clicked point").clicked() {
                    if let Some(op) = op_modify_waypoints(
                        ctx,
                        state,
                        id,
                        class,
                        |pts| {
                            // Segment seg_idx spans (seg_idx-1, seg_idx)
                            // within the interior list, where indices
                            // outside that span correspond to the port
                            // endpoints. Insert the click position as
                            // a fresh interior bend at position seg_idx.
                            let insert_at = seg_idx.min(pts.len());
                            pts.insert(
                                insert_at,
                                lunco_canvas::Pos::new(click_world.x, click_world.y),
                            );
                        },
                    ) {
                        out.push(op);
                    }
                    ui.close();
                }
            }
            lunco_canvas::EdgeHitKind::Body => {}
        }
        ui.separator();
    }
    if ui.button("✂ Delete").on_hover_text("Remove this connection from the diagram").clicked() {
        if let Some(class) = editing_class {
            if let Some(op) = op_remove_edge(ctx, state, id, class) {
                out.push(op);
                let active_doc = active_doc_from_world_ctx(ctx);
                let tab = render_tab_id(ctx);
                let docstate = state.get_mut_for_render(tab, active_doc);
                docstate.canvas.scene.remove_edge(id);
            }
        }
        ui.close();
    }
    if ui.button("↺ Reverse direction").on_hover_text("Swap the connection's start and end ports").clicked() {
        if let Some(class) = editing_class {
            let active_doc = active_doc_from_world_ctx(ctx);
            let tab = render_tab_id(ctx);
            let scene = &state.get_for_render(tab, active_doc).canvas.scene;
            if let Some(edge) = scene.edge(id) {
                if let (Some(from_node), Some(to_node)) = (
                    scene.node(edge.from.node),
                    scene.node(edge.to.node),
                ) {
                    if let (Some(from_inst), Some(to_inst)) = (
                        from_node.origin.clone(),
                        to_node.origin.clone(),
                    ) {
                        out.push(ModelicaOp::ReverseConnection {
                            class: class.to_string(),
                            from: crate::pretty::PortRef::new(
                                &from_inst,
                                edge.from.port.as_str(),
                            ),
                            to: crate::pretty::PortRef::new(
                                &to_inst,
                                edge.to.port.as_str(),
                            ),
                        });
                    }
                }
            }
        }
        ui.close();
    }
    // ── Wire properties submenu ─────────────────────────────────────
    // Inline color/thickness/smooth controls. Each change emits a
    // separate `SetConnectionLineStyle` op so the source-level
    // surgical update only touches the field the user actually
    // changed (`Phase D` infrastructure handles preservation).
    let Some(class) = editing_class else { return };
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id(ctx);
    let scene = &state.get_for_render(tab, active_doc).canvas.scene;
    let Some(edge) = scene.edge(id) else { return };
    let Some(from_node) = scene.node(edge.from.node) else { return };
    let Some(to_node) = scene.node(edge.to.node) else { return };
    let Some(from_instance) = from_node.origin.clone() else { return };
    let Some(to_instance) = to_node.origin.clone() else { return };
    let from_port = edge.from.port.as_str().to_string();
    let to_port = edge.to.port.as_str().to_string();
    // Pull seed values from the live edge data. Color: ConnectionEdge
    // icon_color (per-connector tint); smooth: smooth_bezier flag.
    // Thickness has no plumbed scene-side mirror — seed at the
    // Modelica default (0.25). The surgical writer preserves any
    // existing thickness in source until the user actively drags
    // the slider, so a wrong seed doesn't lose user data.
    let data = edge
        .data
        .downcast_ref::<super::edge::ConnectionEdgeData>();
    let (mut color_rgb, mut smooth, default_thickness) = match data {
        Some(d) => {
            let c = d
                .icon_color
                .map(|c| [c.r(), c.g(), c.b()])
                .unwrap_or([0, 0, 0]);
            (c, d.smooth_bezier, 0.25_f64)
        }
        None => ([0u8, 0, 0], false, 0.25_f64),
    };
    // Egui keeps in-flight slider state across frames; we mirror the
    // last-emitted thickness into ctx data so the slider doesn't
    // snap back to the seed mid-edit.
    let thickness_id = egui::Id::new(("wire-thickness", id));
    let mut thickness: f64 = ui
        .ctx()
        .data(|d| d.get_temp::<f64>(thickness_id))
        .unwrap_or(default_thickness);
    ui.separator();
    ui.label(egui::RichText::new("Properties").strong());
    let mk_op = |color: Option<[u8; 3]>,
                 thickness: Option<f64>,
                 smooth_bezier: Option<bool>| {
        ModelicaOp::SetConnectionLineStyle {
            class: class.to_string(),
            from: crate::pretty::PortRef::new(&from_instance, &from_port),
            to: crate::pretty::PortRef::new(&to_instance, &to_port),
            color,
            thickness,
            smooth_bezier,
        }
    };
    ui.horizontal(|ui| {
        ui.label("Color");
        if ui.color_edit_button_srgb(&mut color_rgb).changed() {
            out.push(mk_op(Some(color_rgb), None, None));
        }
    });
    ui.horizontal(|ui| {
        ui.label("Thickness");
        let r = ui.add(
            egui::Slider::new(&mut thickness, 0.05..=2.0).step_by(0.05),
        );
        if r.changed() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(thickness_id, thickness));
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            out.push(mk_op(None, Some(thickness), None));
        }
    });
    if ui
        .checkbox(&mut smooth, "Smooth (Bezier)")
        .changed()
    {
        out.push(mk_op(None, None, Some(smooth)));
    }
}

pub(super) fn render_empty_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    click_world: lunco_canvas::Pos,
    editing_class: Option<&str>,
    out: &mut Vec<ModelicaOp>,
) {
    ui.label(egui::RichText::new("Add component").strong());
    ui.separator();

    // Hierarchical package navigation — each submenu level mirrors
    // Modelica's package tree (Modelica → Electrical → Analog →
    // Basic → Resistor). Matches how OMEdit and Dymola present
    // the library: user drills down by package instead of
    // scanning a flat list. Tree is built once, cached.
    let show_icons = ctx
        .resource::<PaletteSettings>()
        .map(|s| s.show_icon_only_classes)
        .unwrap_or(false);
    let active_doc = active_doc_from_world_ctx(ctx);
    palette::render_msl_package_menu(
        ui,
        ctx,
        state,
        active_doc,
        palette::msl_package_tree(),
        click_world,
        editing_class,
        show_icons,
        out,
    );
    ui.separator();
    // ── Add Plot ──────────────────────────────────────────────────
    // In-canvas scope: drop a `lunco.viz.plot` Scene node at the click
    // position. The "Empty plot" entry is always available so users
    // can place a chart while authoring, before any simulation has
    // run; signal entries appear once the active sim has populated
    // `SignalRegistry`. An empty plot can be bound later via the
    // inspector.
    let sigs = collect_varying_signals(ctx);
    ui.menu_button("📊 Add Plot here", |ui| {
        // TODO(menu-height): the height is "so-so" — sometimes
        // collapses to 3 rows. Match how the Modelica
        // "Add component" cascade works (see
        // `render_msl_package_menu` ~3065): plain
        // `ui.menu_button(..., |ui| ...)` recursively, no explicit
        // `set_min_*`/`set_max_*`. Egui auto-sizes from content
        // there and it Just Works. The current adaptive
        // computation below is a workaround — the real fix is to
        // mirror that simpler structure (probably means dropping
        // the ScrollArea wrapper too).
        const ROW_PX: f32 = 18.0;
        let max_h = (ui.ctx().content_rect().height() * 0.7).max(180.0);
        let wanted = ((sigs.len() + 3) as f32 * ROW_PX).min(max_h);
        ui.set_min_height(wanted);
        if ui.button("Empty plot (bind later)").on_hover_text("Add a blank plot node — bind a signal to it later").clicked() {
            insert_plot_node(ctx, state, click_world, 0, "");
            ui.close();
        }
        ui.separator();
        if sigs.is_empty() {
            ui.label(
                egui::RichText::new("(no signals yet — run a simulation to bind)")
                    .weak()
                    .small(),
            );
            return;
        }
        // ScrollArea caps the height at 80 % of the screen so the
        // popup never spills past the window. `auto_shrink: true`
        // for height — the popup itself only grows as tall as it
        // needs. `false` for width so long names don't trigger a
        // horizontal scrollbar.
        let max_h = ui.ctx().content_rect().height() * 0.8;
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (entity, path) in &sigs {
                    if ui.button(path).clicked() {
                        insert_plot_node(ctx, state, click_world, entity.to_bits(), path);
                        ui.close();
                    }
                }
            });
    });
    ui.separator();
    if ui.button("⎚ Fit all (F)").on_hover_text("Zoom and pan to fit the whole diagram in view").clicked() {
        let active_doc = active_doc_from_world_ctx(ctx);
        let tab = render_tab_id(ctx);
        let docstate = state.get_mut_for_render(tab, active_doc);
        if let Some(bounds) = docstate.canvas.scene.bounds() {
            let sr = lunco_canvas::Rect::from_min_max(
                lunco_canvas::Pos::new(0.0, 0.0),
                lunco_canvas::Pos::new(800.0, 600.0),
            );
            let (c, z) = docstate.canvas.viewport.fit_values(bounds, sr, 40.0);
            docstate.canvas.viewport.set_target(c, z);
        }
        ui.close();
    }
    if ui.button("⟲ Reset zoom").on_hover_text("Return to 100% zoom").clicked() {
        let active_doc = active_doc_from_world_ctx(ctx);
        let tab = render_tab_id(ctx);
        let docstate = state.get_mut_for_render(tab, active_doc);
        let c = docstate.canvas.viewport.center;
        docstate.canvas.viewport.set_target(c, 1.0);
        ui.close();
    }
}

pub(super) fn insert_plot_node(
    ctx: &PanelCtx,
    state: &mut CanvasDiagramState,
    click_world: lunco_canvas::Pos,
    entity_bits: u64,
    signal_path: &str,
) {
    let payload = lunco_viz::kinds::canvas_plot_node::PlotNodeData {
        binding: lunco_viz::kinds::canvas_plot_node::PlotBinding::Pinned {
            entity: entity_bits,
        },
        signal_path: signal_path.to_string(),
        title: String::new(),
    };
    let data: lunco_canvas::NodeData = std::sync::Arc::new(payload);
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id(ctx);
    let docstate = state.get_mut_for_render(tab, active_doc);
    let scene = &mut docstate.canvas.scene;
    let id = scene.alloc_node_id();
    scene.insert_node(lunco_canvas::scene::Node {
        id,
        rect: lunco_canvas::Rect::from_min_max(
            click_world,
            lunco_canvas::Pos::new(click_world.x + 60.0, click_world.y + 40.0),
        ),
        kind: lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND.into(),
        data,
        ports: Vec::new(),
        label: String::new(),
        origin: None,
        resizable: true,
        visual_rect: None,
    });
}