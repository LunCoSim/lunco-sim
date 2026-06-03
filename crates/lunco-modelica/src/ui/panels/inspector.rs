//! Inspector panel — shows the canvas's current selection and lets the
//! user edit a component's modifications (parameter overrides).
//!
//! ## Architecture
//!
//! - **Reads** [`crate::ui::panels::canvas_diagram::CanvasDiagramState`] —
//!   the canvas owns selection state. The `primary()` selection's
//!   `Node(id)` is mapped through the scene to the Modelica instance
//!   name (`Node.origin`), then cross-referenced against the doc's AST
//!   to find the matching `Component` declaration.
//! - **Writes** via the unified [`crate::api_edits::ApplyModelicaOps`]
//!   Reflect event with [`crate::api_edits::ApiOp::SetParameter`]. The
//!   GUI never mutates state directly; per AGENTS.md §4.1 every edit
//!   goes through the same command surface an external API caller
//!   would use.
//!
//! ## What it shows
//!
//! For the selected component:
//! - instance name + declared type
//! - list of modifications (`R = 10`, `unit = "kg"`, …) with
//!   editable values
//!
//! Description strings, port lists, and the broader class structure
//! are deliberately not surfaced here — the agent-facing
//! `describe_model` API endpoint is the canonical place for those.
//! The inspector is the lightweight per-selection editor.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::SelectItem;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::api::{ApiOp, ApplyModelicaOps};

pub struct InspectorPanel;

impl Panel for InspectorPanel {
    fn id(&self) -> PanelId {
        PanelId("modelica_diagram_inspector")
    }

    fn title(&self) -> String {
        "Inspector".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let (warning, muted) = world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| (t.tokens.warning, t.tokens.text_subdued))
            .unwrap_or((egui::Color32::from_rgb(180, 150, 90), egui::Color32::GRAY));
        // ── Resolve target doc ───────────────────────────────────
        // Follow-active by default; honor the panel's pin if set
        // (see `doc_pin::DocPinState`). Pin header below lets the
        // user toggle.
        crate::ui::doc_pin::render_pin_header(
            ui,
            world,
            crate::ui::doc_pin::PinKind::Inspector,
        );
        let Some(doc_id) = crate::ui::doc_pin::resolved_inspector_doc(world) else {
            placeholder(ui, "No active document.");
            return;
        };

        // Read-only state — derived from the document's origin (the
        // canonical source of truth; `ModelicaDocument::apply` enforces
        // the same invariant). We use it purely for UX (dim the text
        // editors) — even if a stray edit slipped through, the doc
        // would reject it. The defensive guard before firing
        // `ApplyModelicaOps` is a belt-and-braces gesture.
        let read_only = {
            let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
            registry
                .host(doc_id)
                .map(|h| h.document().is_read_only())
                .unwrap_or(false)
        };

        // ── Resolve the selected node ──────────────────────────
        //
        // Edges are not currently inspectable — we only surface
        // component-level edits. Plot nodes get a dedicated signal-
        // binding editor (see `render_plot_node_editor`); component
        // nodes go through the AST-driven modifications path below.
        let mut selection_kind = "none";
        let primary = world
            .get_resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
            .and_then(|cs| {
                let docstate = cs.get(Some(doc_id));
                let primary = docstate.canvas.selection.primary()?;
                match primary {
                    SelectItem::Node(node_id) => {
                        selection_kind = "node";
                        let node = docstate.canvas.scene.node(node_id)?;
                        Some((node_id, node.kind.clone(), node.origin.clone()))
                    }
                    SelectItem::Edge(_) => {
                        selection_kind = "edge";
                        None
                    }
                }
            });

        let Some((node_id, node_kind, node_origin)) = primary else {
            match selection_kind {
                "edge" => placeholder(ui, "Wire editing not supported yet."),
                _ => placeholder(ui, "Select a node on the canvas."),
            }
            return;
        };

        if node_kind.as_str() == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND {
            render_plot_node_editor(ui, world, doc_id, node_id);
            return;
        }
        if node_kind.as_str() == crate::ui::text_node::TEXT_NODE_KIND {
            render_text_node_editor(ui, world, doc_id, node_id);
            return;
        }

        let Some(instance_name) = node_origin else {
            placeholder(ui, "Select a node on the canvas.");
            return;
        };

        // ── Resolve the active class on this doc ────────────────
        //
        // Mirrors `canvas_diagram::active_class_for_doc`: the
        // drilled-in pin wins when set, otherwise the document's
        // first non-package class (the same default the canvas
        // projects).
        let drilled_in =
            crate::ui::panels::model_view::drilled_class_for_doc(world, doc_id);

        // Resolve the target class + component via the per-document
        // [`crate::index::ModelicaIndex`]. The Index is patched
        // optimistically on every structural op (see
        // `ModelicaDocument::apply_patch`) so this read sees fresh
        // state even during the 2.5 s AST-reparse debounce.
        let (component_info, class, param_desc) = {
            let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
            let Some(host) = registry.host(doc_id) else {
                placeholder(ui, "Document not in registry.");
                return;
            };
            let index = host.document().index();
            // Resolve the target class. Drilled-in pin first; otherwise
            // pick the first non-package class in the Index (mirrors
            // `extract_model_name_from_ast`'s behaviour, but reading
            // the projection rather than walking the AST).
            let Some(class) = drilled_in.or_else(|| {
                index
                    .classes
                    .values()
                    .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
                    .map(|c| c.name.clone())
            }) else {
                placeholder(ui, "Could not resolve target class.");
                return;
            };
            // The Index keys components by qualified-class. The
            // drilled-in pin is already qualified; the fallback above
            // returns a qualified path. Look up directly.
            let Some(entry) = index.find_component(&class, &instance_name) else {
                let short = class.rsplit('.').next().unwrap_or(&class);
                placeholder(
                    ui,
                    &format!(
                        "Selected node `{instance_name}` not declared in `{short}`."
                    ),
                );
                return;
            };
            // Project the Index entry into the inspector's
            // [`ComponentInfo`] shape so the rest of this function
            // (rendering, edit collection) doesn't have to change.
            let info = crate::ast_extract::ComponentInfo {
                name: entry.name.clone(),
                type_name: entry.type_name.clone(),
                description: entry.description.clone(),
                modifications: entry.modifications.clone(),
            };
            // Build a name→description map for the component TYPE's own
            // parameters so each modification row can show the original
            // Modelica `"..."` comment on hover. Modifications are keyed
            // by the type's parameter names, so resolve the type class
            // (direct, then short-name suffix match) and harvest its
            // component descriptions.
            let type_name = entry.type_name.clone();
            let mut param_desc: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let type_path = if index.classes.contains_key(&type_name) {
                Some(type_name.clone())
            } else {
                index
                    .classes
                    .keys()
                    .find(|k| k.rsplit('.').next() == Some(type_name.as_str()))
                    .cloned()
            };
            if let Some(tp) = type_path {
                if let Some(keys) = index.components_by_class.get(&tp) {
                    for key in keys {
                        if let Some(comp) = index.components.get(key.0 as usize) {
                            if !comp.description.is_empty() {
                                param_desc
                                    .insert(comp.name.clone(), comp.description.clone());
                            }
                        }
                    }
                }
            }
            (info, class, param_desc)
        };

        // ── Render header ───────────────────────────────────────
        ui.add_space(4.0);
        ui.heading(&component_info.name);
        ui.label(
            egui::RichText::new(&component_info.type_name)
                .size(11.0)
                .color(muted),
        );
        if !component_info.description.is_empty() {
            ui.label(&component_info.description);
        }
        if read_only {
            ui.label(
                egui::RichText::new(
                    "🔒 Read-only library tab — duplicate to workspace to edit.",
                )
                .italics()
                .color(warning),
            );
        }
        ui.separator();

        // ── Render modifications + collect edits ────────────────
        //
        // `text_edit_singleline` returns a Response; we apply the
        // edit on `lost_focus()` rather than `changed()` so a partial
        // value mid-typing doesn't fire a SetParameter on every
        // keystroke (which would push N undo entries onto the stack).
        let mut edits: Vec<(String, String)> = Vec::new();
        if component_info.modifications.is_empty() {
            ui.label(
                egui::RichText::new("No modifications declared. Edits will append new ones.")
                    .italics()
                    .color(muted),
            );
        }
        // Stable order: sort by name so the inspector layout doesn't
        // jitter as the underlying HashMap iteration order shifts.
        let mut entries: Vec<(&String, &String)> = component_info.modifications.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        ui.collapsing("⚙ Modifications", |ui| {
            egui::Grid::new("modelica_inspector_mods")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (k, v) in entries {
                        let name_resp =
                            ui.add(egui::Label::new(k).sense(egui::Sense::hover()));
                        if let Some(d) = param_desc.get(k) {
                            name_resp.on_hover_text(d);
                        }
                        let mut buf = v.clone();
                        // `add_enabled` disables the input on read-only
                        // tabs — egui dims it and ignores keystrokes,
                        // so the user clearly sees they can't edit.
                        let resp = ui.add_enabled(
                            !read_only,
                            egui::TextEdit::singleline(&mut buf),
                        );
                        if let Some(d) = param_desc.get(k) {
                            resp.clone().on_hover_text(d);
                        }
                        if !read_only && resp.lost_focus() && buf != *v {
                            edits.push((k.clone(), buf));
                        }
                        ui.end_row();
                    }
                });
        });

        // ── Apply edits as a single batched event ───────────────
        // Defensive: even if a stray edit slipped through (shouldn't
        // happen given the disabled inputs above), don't fire ops on
        // a read-only doc.
        if !edits.is_empty() && !read_only {
            let ops: Vec<ApiOp> = edits
                .into_iter()
                .map(|(param, value)| ApiOp::SetParameter {
                    class: class.clone(),
                    component: instance_name.clone(),
                    param,
                    value,
                })
                .collect();
            world.commands().trigger(ApplyModelicaOps {
                doc: doc_id,
                ops,
            });
        }
    }
}

fn placeholder(ui: &mut egui::Ui, msg: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.label(
            egui::RichText::new(msg)
                .italics()
                .color(egui::Color32::GRAY),
        );
    });
}

/// Inspector view for a selected `lunco.viz.plot` node — current
/// binding plus a clickable list of available signals. Picking a
/// signal swaps the node's `PlotNodeData` so the visual immediately
/// renders the new line on the next frame. The list is empty until
/// the active simulator has populated `SignalRegistry`; that case
/// shows a short hint instead of a blank panel.
fn render_plot_node_editor(
    ui: &mut egui::Ui,
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    node_id: lunco_canvas::NodeId,
) {
    use lunco_viz::kinds::canvas_plot_node::PlotNodeData;

    let current: PlotNodeData = world
        .get_resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
        .and_then(|cs| {
            cs.get(Some(doc_id))
                .canvas
                .scene
                .node(node_id)?
                .data
                .downcast_ref::<PlotNodeData>()
                .cloned()
        })
        .unwrap_or_default();

    ui.add_space(4.0);
    ui.heading("Plot");
    if current.signal_path.is_empty() {
        ui.label(
            egui::RichText::new("Unbound — pick a signal below.")
                .italics()
                .color(egui::Color32::GRAY),
        );
    } else {
        ui.label(
            egui::RichText::new(format!("Bound to: {}", current.signal_path))
                .small(),
        );
        if ui.button("Unbind").clicked() {
            apply_plot_binding(world, doc_id, node_id, 0, "");
        }
        // Title editor — writes back to source as the
        // `LunCoAnnotations.PlotNode(title="…")` field. Buffer per node
        // id keyed in egui memory so typing isn't preempted by the
        // re-projection that follows each commit. The op only
        // fires on Enter / focus loss to avoid a write per
        // keystroke (each one would re-parse the document).
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Title").small().weak());
        let buf_id = egui::Id::new(("plot_title_buf", node_id.0));
        let mut buf: String = ui
            .memory(|m| m.data.get_temp::<String>(buf_id))
            .unwrap_or_else(|| current.title.clone());
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .hint_text(&current.signal_path)
                .desired_width(f32::INFINITY),
        );
        let committed = resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            || (resp.lost_focus() && !resp.has_focus());
        if resp.changed() {
            ui.memory_mut(|m| m.data.insert_temp(buf_id, buf.clone()));
        }
        if committed && buf != current.title {
            apply_plot_title(world, doc_id, node_id, &buf);
            ui.memory_mut(|m| m.data.remove::<String>(buf_id));
        }
    }
    ui.separator();

    let sigs: Vec<(bevy::prelude::Entity, String)> = world
        .get_resource::<lunco_viz::SignalRegistry>()
        .map(|r| {
            let mut v: Vec<_> = r
                .iter_scalar()
                .map(|(s, _)| (s.entity, s.path.clone()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        })
        .unwrap_or_default();

    if sigs.is_empty() {
        ui.label(
            egui::RichText::new("(no signals yet — run a simulation to bind)")
                .weak()
                .small(),
        );
        return;
    }

    let max_h = ui.ctx().content_rect().height() * 0.6;
    egui::ScrollArea::vertical()
        .max_height(max_h)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (entity, path) in &sigs {
                let is_current = current.binding.pinned_entity() == Some(entity.to_bits())
                    && path == &current.signal_path;
                let resp = ui.selectable_label(is_current, path);
                if resp.clicked() && !is_current {
                    apply_plot_binding(world, doc_id, node_id, entity.to_bits(), path);
                }
            }
        });
}

fn apply_plot_title(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    node_id: lunco_canvas::NodeId,
    title: &str,
) {
    use crate::document::ModelicaOp;
    use lunco_viz::kinds::canvas_plot_node::PlotNodeData;

    // Snapshot the plot's signal path — that's the op key. Skip
    // when the node isn't a plot or has no binding (no source row
    // to update).
    let signal_path = {
        let state = world
            .resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let scene = &state.get(Some(doc_id)).canvas.scene;
        let Some(node) = scene.node(node_id) else { return };
        if node.kind != lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND {
            return;
        }
        node.data
            .downcast_ref::<PlotNodeData>()
            .map(|d| d.signal_path.clone())
            .unwrap_or_default()
    };
    if signal_path.is_empty() {
        return;
    }
    // Optimistic in-memory update — visual reflects the title now,
    // before the source rewrite + reproject completes.
    {
        let mut state =
            world.resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let docstate = state.get_mut(Some(doc_id));
        if let Some(node) = docstate.canvas.scene.node_mut(node_id) {
            if let Some(prev) = node.data.downcast_ref::<PlotNodeData>() {
                let updated = PlotNodeData {
                    title: title.to_string(),
                    ..prev.clone()
                };
                node.data = std::sync::Arc::new(updated);
            }
        }
    }
    let class = crate::ui::panels::model_view::drilled_class_for_doc(world, doc_id)
        .or_else(|| {
            world
                .get_resource::<crate::ui::WorkbenchState>()
                .and_then(|_s| crate::ui::state::detected_name_for(world, doc_id))
        })
        .unwrap_or_default();
    if class.is_empty() {
        return;
    }
    crate::ui::panels::canvas_diagram::apply_ops_public(
        world,
        doc_id,
        vec![ModelicaOp::SetPlotNodeTitle {
            class,
            signal_path,
            title: title.to_string(),
        }],
    );
}

fn apply_plot_binding(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    node_id: lunco_canvas::NodeId,
    entity_bits: u64,
    signal_path: &str,
) {
    use crate::document::ModelicaOp;
    use lunco_viz::kinds::canvas_plot_node::PlotNodeData;

    // Snapshot the previous binding, mode, and rect so we can build
    // the right op pair (remove old, add new), preserve the tile's
    // binding mode (Pinned vs Doc) across the rebind, and keep the
    // optimistic in-memory swap consistent with the source rewrite.
    let (prev_signal, prev_binding, rect, kind_is_plot) = {
        let state = world
            .resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let scene = &state.get(Some(doc_id)).canvas.scene;
        let Some(node) = scene.node(node_id) else { return };
        let prev_data = node.data.downcast_ref::<PlotNodeData>();
        let prev_sig = prev_data.map(|d| d.signal_path.clone()).unwrap_or_default();
        let prev_bind = prev_data
            .map(|d| d.binding.clone())
            .unwrap_or_else(lunco_viz::kinds::canvas_plot_node::PlotBinding::default);
        let is_plot = node.kind == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND;
        (prev_sig, prev_bind, node.rect, is_plot)
    };
    if !kind_is_plot {
        return;
    }
    // 1. Optimistic in-memory swap so the visual updates this frame.
    //    Preserve binding mode: a source-backed (Doc) tile stays
    //    Doc-bound after a signal rebind — only the signal path
    //    changes. A Telemetry-pinned tile updates its `entity` to
    //    the newly-chosen one. Without this, picking a new signal
    //    in the inspector silently demoted source-backed tiles to
    //    pinned mode, breaking the per-doc resolution policy.
    use lunco_viz::kinds::canvas_plot_node::PlotBinding;
    let binding = match prev_binding {
        PlotBinding::Doc { .. } => prev_binding,
        PlotBinding::Pinned { .. } => PlotBinding::Pinned { entity: entity_bits },
    };
    let payload = PlotNodeData {
        binding,
        signal_path: signal_path.to_string(),
        title: String::new(),
    };
    let data: lunco_canvas::NodeData = std::sync::Arc::new(payload);
    {
        let mut state =
            world.resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let docstate = state.get_mut(Some(doc_id));
        if let Some(node) = docstate.canvas.scene.node_mut(node_id) {
            node.data = data;
        }
    }
    // 2. Source rewrite: rebinding is `Remove(old) + Add(new)` so
    //    the diagram annotation tracks the user's choice. Empty
    //    `signal_path` is "unbind" — only emit the Remove. Empty
    //    `prev_signal` is "first bind" — only emit the Add.
    let class = crate::ui::panels::model_view::drilled_class_for_doc(world, doc_id)
        .or_else(|| {
            world
                .get_resource::<crate::ui::WorkbenchState>()
                .and_then(|_s| crate::ui::state::detected_name_for(world, doc_id))
        })
        .unwrap_or_default();
    if class.is_empty() {
        return;
    }
    let mut ops: Vec<ModelicaOp> = Vec::new();
    if !prev_signal.is_empty() && prev_signal != signal_path {
        ops.push(ModelicaOp::RemovePlotNode {
            class: class.clone(),
            signal_path: prev_signal,
        });
    }
    if !signal_path.is_empty() {
        ops.push(ModelicaOp::AddPlotNode {
            class,
            plot: crate::pretty::LunCoPlotNodeSpec {
                x1: rect.min.x,
                y1: rect.min.y,
                x2: rect.max.x,
                y2: rect.max.y,
                signal: signal_path.to_string(),
                title: String::new(),
            },
        });
    }
    if !ops.is_empty() {
        crate::ui::panels::canvas_diagram::apply_ops_public(world, doc_id, ops);
    }
}

// ────────────────────────────────────────────────────────────────────
// Diagram-text editor — string + font-size override
// ────────────────────────────────────────────────────────────────────

/// Inspector view for a selected `lunco.modelica.text` scene Node.
/// Lets the user rewrite the label text. Move/resize go through the
/// canvas drag handles; this panel just owns the text content. The
/// commit cadence (Enter / focus-loss, not per-keystroke) matches
/// the plot-title editor — every commit re-parses the source, so a
/// per-keystroke flush would stall the UI.
fn render_text_node_editor(
    ui: &mut egui::Ui,
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    node_id: lunco_canvas::NodeId,
) {
    use crate::ui::text_node::TextNodeData;

    let (current_text, idx_opt) = {
        let state = world
            .resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let scene = &state.get(Some(doc_id)).canvas.scene;
        let Some(node) = scene.node(node_id) else { return };
        let text = node
            .data
            .downcast_ref::<TextNodeData>()
            .map(|d| d.text.clone())
            .unwrap_or_default();
        let idx = node
            .origin
            .as_deref()
            .and_then(|o| o.strip_prefix("text:"))
            .and_then(|n| n.parse::<usize>().ok());
        (text, idx)
    };
    let Some(idx) = idx_opt else {
        ui.label(
            egui::RichText::new("Untracked text — save first to edit.")
                .italics()
                .color(egui::Color32::GRAY),
        );
        return;
    };

    ui.add_space(4.0);
    ui.heading("Text");
    let buf_id = egui::Id::new(("text_node_buf", node_id.0));
    let mut buf: String = ui
        .memory(|m| m.data.get_temp::<String>(buf_id))
        .unwrap_or_else(|| current_text.clone());
    let resp = ui.add(
        egui::TextEdit::multiline(&mut buf)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    if resp.changed() {
        ui.memory_mut(|m| m.data.insert_temp(buf_id, buf.clone()));
    }
    let committed = (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        || (resp.lost_focus() && !resp.has_focus());
    if committed && buf != current_text {
        apply_diagram_text_string(world, doc_id, node_id, idx, &buf);
        ui.memory_mut(|m| m.data.remove::<String>(buf_id));
    }
}

fn apply_diagram_text_string(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    node_id: lunco_canvas::NodeId,
    index: usize,
    text: &str,
) {
    use crate::document::ModelicaOp;
    use crate::ui::text_node::TextNodeData;

    // Optimistic in-memory swap so the visual updates this frame.
    {
        let mut state = world
            .resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let docstate = state.get_mut(Some(doc_id));
        if let Some(node) = docstate.canvas.scene.node_mut(node_id) {
            if let Some(prev) = node.data.downcast_ref::<TextNodeData>() {
                let updated = TextNodeData {
                    text: text.to_string(),
                    ..prev.clone()
                };
                node.data = std::sync::Arc::new(updated);
            }
        }
    }
    let class = crate::ui::panels::model_view::drilled_class_for_doc(world, doc_id)
        .or_else(|| {
            world
                .get_resource::<crate::ui::WorkbenchState>()
                .and_then(|_s| crate::ui::state::detected_name_for(world, doc_id))
        })
        .unwrap_or_default();
    if class.is_empty() {
        return;
    }
    crate::ui::panels::canvas_diagram::apply_ops_public(
        world,
        doc_id,
        vec![ModelicaOp::SetDiagramTextString {
            class,
            index,
            text: text.to_string(),
        }],
    );
}

