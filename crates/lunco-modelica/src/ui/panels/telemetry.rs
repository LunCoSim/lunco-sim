//! Telemetry panel — model parameters, inputs, and variable plotting toggles.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::ui::WorkbenchState;
use crate::ui::viz::{is_signal_plotted, set_signal_plotted};
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};

/// Per-input metadata snapshot — built once per render from
/// [`crate::index::ModelicaIndex`] so the grid loop doesn't reborrow
/// the document registry per row. Description and bounds resolve via
/// [`crate::index::ModelicaIndex::find_component_by_leaf`].
struct InputRow {
    name: String,
    value: f64,
    description: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
}

/// Render `body` inside a fixed-height region with a draggable
/// horizontal divider beneath it. The divider lets the user grow /
/// shrink the region at the expense of whatever follows it in the
/// panel — Telemetry uses this to share vertical space between
/// Parameters, Inputs, and the Variables list. Height persists
/// across sessions in egui memory keyed by `id`.
///
/// `egui::Resize` ships a tiny corner grip that's invisible to most
/// users; this gives them a wide, painted bar with a `ResizeRow`
/// cursor on hover — the affordance professional UIs use.
fn resizable_v_section<R>(
    ui: &mut egui::Ui,
    id: &str,
    default_h: f32,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.make_persistent_id(id);
    let mut h = ui
        .memory_mut(|m| m.data.get_persisted::<f32>(id))
        .unwrap_or(default_h);
    let avail_w = ui.available_width();
    let result = ui
        .allocate_ui_with_layout(
            egui::vec2(avail_w, h),
            egui::Layout::top_down(egui::Align::Min),
            body,
        )
        .inner;
    // Drag handle — a 6 px tall horizontal strip with a centred
    // "grip" line so the affordance is visible even at rest.
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(avail_w, 6.0), egui::Sense::drag());
    let visuals = ui.visuals();
    let stroke_color = if resp.hovered() || resp.dragged() {
        visuals.selection.bg_fill
    } else {
        visuals.widgets.inactive.bg_stroke.color
    };
    let y = rect.center().y;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 8.0, y),
            egui::pos2(rect.right() - 8.0, y),
        ],
        egui::Stroke::new(2.0, stroke_color),
    );
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeRow);
    }
    if resp.dragged() {
        h = (h + resp.drag_delta().y).clamp(40.0, 800.0);
        ui.memory_mut(|m| m.data.insert_persisted(id, h));
    }
    result
}

/// Telemetry panel — model parameters, inputs, and variable plotting toggles.
pub struct TelemetryPanel;

impl Panel for TelemetryPanel {
    fn id(&self) -> PanelId { PanelId("modelica_inspector") }
    fn title(&self) -> String { "📊 Telemetry".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Fix selection leakage
        ui.style_mut().interaction.selectable_labels = false;
        let muted = world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.text_subdued)
            .unwrap_or(egui::Color32::from_rgb(140, 140, 160));

        // Pin header — follow active tab by default; click 📍 to
        // pin this panel to the currently active model so it stays
        // put while the user edits another tab.
        crate::ui::doc_pin::render_pin_header(
            ui,
            world,
            crate::ui::doc_pin::PinKind::Telemetry,
        );

        // Component inspector — when one or more nodes are selected
        // on the active diagram, show their parameters and let the
        // user edit them. Works pre- and post-compile; edits go
        // through the same `SetParameter` op the canvas drag flow
        // uses, so undo / re-projection / journaling stay consistent.
        render_selected_components_inspector(ui, world, muted);
        render_active_class_parameters(ui, world, muted);

        // Resolve the document and entity to display. Telemetry follows
        // its own pin or the active document.
        let doc_id = crate::ui::doc_pin::resolved_telemetry_doc(world);
        let Some(doc_id) = doc_id else {
            render_runtime_hint(
                ui,
                muted,
                world,
                "No document active.",
            );
            return;
        };

        // Resolve the entity to display: explicit pin (`selected_entity`)
        // wins, otherwise follow the resolved document.
        let (entity, has_data) = {
            let pinned_entity = world
                .get_resource::<WorkbenchState>()
                .and_then(|s| s.selected_entity);
            let resolved = crate::ui::state::simulator_for(world, doc_id)
                .or(pinned_entity);
            let has = resolved
                .map(|e| world.get::<ModelicaModel>(e).is_some())
                .unwrap_or(false);
            (resolved, has)
        };

        if let (Some(entity), true) = (entity, has_data) {
            // Read model snapshot for display. Parameter editing lives in
            // `render_selected_components_inspector` (op-pipeline based);
            // the panel only reads runtime values here.
            let (is_paused, current_time, inputs) = {
                if let Some(model) = world.get::<ModelicaModel>(entity) {
                    (model.paused, model.current_time, model.inputs.clone())
                } else {
                    (true, 0.0, std::collections::HashMap::new())
                }
            };

            // Snapshot per-input metadata from the document index so the
            // inputs grid below can render tooltips and bound-clamped
            // sliders without reborrowing the registry per row.
            let input_rows: Vec<InputRow> = {
                let mut sorted: Vec<(String, f64)> = inputs.into_iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                let registry = world.get_resource::<crate::ui::ModelicaDocumentRegistry>();
                let index_ref = registry
                    .and_then(|r| r.host(doc_id))
                    .map(|h| h.document().index());
                sorted
                    .into_iter()
                    .map(|(name, value)| {
                        let entry =
                            index_ref.and_then(|idx| idx.find_component_by_leaf(&name));
                        InputRow {
                            description: entry
                                .map(|e| e.description.clone())
                                .filter(|s| !s.is_empty()),
                            min: entry
                                .and_then(|e| e.modifications.get("min").and_then(|s| s.parse().ok())),
                            max: entry
                                .and_then(|e| e.modifications.get("max").and_then(|s| s.parse().ok())),
                            name,
                            value,
                        }
                    })
                    .collect()
            };

            // Play/Pause
            ui.horizontal(|ui| {
                if is_paused {
                    if ui.button("▶ Play").clicked() {
                        if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                            m.paused = false;
                        }
                    }
                } else {
                    if ui.button("⏸ Pause").clicked() {
                        if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                            m.paused = true;
                        }
                    }
                }
                ui.label(format!("Time: {current_time:.4} s"));

                ui.add_space(ui.available_width() - 70.0);
                if ui.button("🔄 Reset").clicked() {
                    let sid = if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                        m.session_id += 1;
                        m.is_stepping = true;
                        m.current_time = 0.0;
                        m.last_step_time = 0.0;
                        Some(m.session_id)
                    } else { None };
                    if let (Some(sid), Some(channels)) = (sid, world.get_resource::<ModelicaChannels>()) {
                        let _ = channels.tx.send(ModelicaCommand::Reset { entity, session_id: sid });
                    }
                }
            });
            ui.separator();

            // Inputs
            if !input_rows.is_empty() {
                ui.label("Inputs (Real-time):");
                resizable_v_section(ui, "inputs_height", 120.0, |ui| {
                    egui::ScrollArea::vertical().id_salt("inputs_scroll").auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("inputs_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                for row in &input_rows {
                                    let label = egui::Label::new(row.name.clone())
                                        .sense(egui::Sense::hover());
                                    let resp = ui.add(label);
                                    if let Some(desc) = &row.description {
                                        resp.on_hover_text(desc);
                                    }
                                    let mut v = row.value;
                                    let avail = ui.available_width().max(60.0);
                                    let val_resp = ui.add_sized(
                                        [avail, 20.0],
                                        egui::DragValue::new(&mut v)
                                            .speed(0.1)
                                            .fixed_decimals(2)
                                            .range(
                                                row.min.unwrap_or(f64::NEG_INFINITY)
                                                    ..=row.max.unwrap_or(f64::INFINITY),
                                            ),
                                    );
                                    if let Some(desc) = &row.description {
                                        val_resp.on_hover_text(desc);
                                    }
                                    ui.end_row();
                                    if (v - row.value).abs() > 1e-10 {
                                        if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                                            if let Some(inp) = m.inputs.get_mut(&row.name) { *inp = v; }
                                        }
                                    }
                                }
                            });
                    });
                });
            }
        } else {
            // Pre-compile state OR post-Reset. Show a hint and Compile
            // button, but continue to show the Variables list below.
            render_runtime_hint(
                ui,
                muted,
                world,
                if !has_data && entity.is_some() {
                    "Stepper went away — recompile to restore live telemetry."
                } else {
                    "Live telemetry appears here once the model is compiled."
                },
            );
        }

        // Variables (Toggle to Plot).
        //
        // Checkboxes write to TWO things in lockstep so this is the
        // single place to pick variables for plotting:
        //   1. `VisualizationConfig.inputs` — drives the live cosim
        //      plot in Graphs (ticked vars stream samples there).
        //   2. `ExperimentVisibility.picked_vars` — drives the
        //      experiment plot in Graphs (ticked vars are drawn for
        //      every visible Fast Run).
        // Names match between the two paths (both come from the same
        // model compile), so one tick = one curve per source.
        //
        // Filter + group: search field collapses noise on big models;
        // collapsing-headers per top-level component group keep the
        // panel scannable.
        ui.horizontal(|ui| {
            ui.label("Variables");
            ui.weak("(toggle to plot)");
        });

        // Filter input lives on ExperimentVisibility — same resource
        // that already holds `picked_vars`, no new state.
        let mut filter_text = world
            .get_resource::<crate::ui::panels::experiments::ExperimentVisibility>()
            .map(|v| v.var_filter.clone())
            .unwrap_or_default();
        let mut filter_changed = false;
        ui.horizontal(|ui| {
            ui.label("🔍");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut filter_text)
                    .hint_text("filter…")
                    .desired_width(160.0),
            );
            if resp.changed() {
                filter_changed = true;
            }
            if ui.small_button("✕").on_hover_text("Clear filter").clicked() {
                filter_text.clear();
                filter_changed = true;
            }
        });
        if filter_changed {
            if let Some(mut vis) = world
                .get_resource_mut::<crate::ui::panels::experiments::ExperimentVisibility>()
            {
                vis.var_filter = filter_text.clone();
            }
        }
        let filter_lower = filter_text.to_ascii_lowercase();

        // "Plot in" router — pins Telemetry checkboxes to a specific
        // plot tab. Default (None) = active plot (Dymola "current
        // window" behaviour). Snapshot the open plots up front; the
        // dropdown re-resolves on next frame after target changes.
        let plot_options: Vec<(lunco_viz::viz::VizId, String)> = {
            let mut opts: Vec<_> = world
                .get_resource::<lunco_viz::VisualizationRegistry>()
                .map(|r| {
                    r.iter()
                        .map(|(id, cfg)| (*id, cfg.title.clone()))
                        .collect()
                })
                .unwrap_or_default();
            opts.sort_by_key(|(id, _)| id.0);
            opts
        };
        let pinned = world
            .get_resource::<crate::ui::panels::experiments::ExperimentVisibility>()
            .and_then(|v| v.target_plot);
        let mut new_target: Option<Option<lunco_viz::viz::VizId>> = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("→").size(11.0).color(muted));
            let label = match pinned {
                None => "Active plot".to_string(),
                Some(id) => plot_options
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_else(|| format!("Plot #{}", id.0)),
            };
            egui::ComboBox::from_id_salt("telem_target_plot")
                .selected_text(label)
                .width(140.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(pinned.is_none(), "Active plot")
                        .on_hover_text(
                            "Route to whichever plot tab was last focused.",
                        )
                        .clicked()
                    {
                        new_target = Some(None);
                    }
                    for (id, title) in &plot_options {
                        let label = format!("{title}  (#{})", id.0);
                        if ui
                            .selectable_label(pinned == Some(*id), label)
                            .clicked()
                        {
                            new_target = Some(Some(*id));
                        }
                    }
                });
        });
        if let Some(t) = new_target {
            if let Some(mut vis) = world
                .get_resource_mut::<crate::ui::panels::experiments::ExperimentVisibility>()
            {
                vis.target_plot = t;
            }
        }

        egui::ScrollArea::vertical()
            .id_salt("telemetry_scroll")
            // Fill the panel's full width instead of shrinking to the
            // (short) variable-name content — otherwise the scrollbar
            // floats mid-panel with dead space to its right.
            .auto_shrink([false, true])
            .show(ui, |ui| {
            let (model_vars, model_inputs) = if let Some(e) = entity {
                if let Some(m) = world.get::<ModelicaModel>(e) {
                    (m.variables.keys().cloned().collect::<Vec<_>>(),
                     m.inputs.keys().cloned().collect::<Vec<_>>())
                } else {
                    (Vec::new(), Vec::new())
                }
            } else {
                (Vec::new(), Vec::new())
            };

            // Picked-for-experiments set, snapshotted once. Routes
            // through the "Plot in" target — pinned plot if set,
            // else the active plot. Same VizId used for the toggle
            // writes below so reads and writes always agree.
            let active_plot = world
                .get_resource::<crate::ui::panels::experiments::ActivePlot>()
                .copied()
                .unwrap_or_default()
                .or_default();
            let target_plot = pinned.unwrap_or(active_plot);

            // Read plotted-set from the viz registry for the target plot.
            // Clone once so we don't reborrow the resource inside the loop.
            let plotted: std::collections::HashSet<String> = if let Some(e) = entity {
                world
                    .get_resource::<lunco_viz::VisualizationRegistry>()
                    .and_then(|r| r.get(target_plot))
                    .map(|cfg| {
                        cfg.inputs
                            .iter()
                            .filter(|b| b.source.entity == e)
                            .map(|b| b.source.path.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                std::collections::HashSet::new()
            };

            let picked_exp: std::collections::BTreeSet<String> = world
                .get_resource::<crate::ui::panels::experiments::PlotPanelStates>()
                .map(|s| s.picked(target_plot))
                .unwrap_or_default();

            // Variables sourced from completed experiments — surface
            // them even when there's no live cosim entity yet.
            let exp_vars: std::collections::BTreeSet<String> =
                crate::ui::panels::experiments::all_experiment_variables_for_doc(world, doc_id);

            let mut all_names: Vec<_> = model_vars;
            all_names.extend(model_inputs);
            all_names.extend(exp_vars.iter().cloned());
            
            // Proactively pull names from the document index if we're
            // looking at a model that hasn't run yet.
            let registry = world.get_resource::<crate::ui::ModelicaDocumentRegistry>();
            let index_ref = registry
                .and_then(|r| r.host(doc_id))
                .map(|h| h.document().index());
            if let Some(index) = index_ref {
                for comp in &index.components {
                    // Very rough heuristic: if it's a Real and not
                    // obviously a parameter, it's probably an
                    // observable. This is just to seed the list
                    // before the first compile.
                    if comp.type_name == "Real"
                        && comp.variability == crate::index::Variability::Continuous
                    {
                        all_names.push(comp.name.clone());
                    }
                }
            }

            all_names.sort();
            all_names.dedup();

            // Snapshot per-variable descriptions from the document index
            // up front so the row loop doesn't reborrow the registry per
            // checkbox.
            let var_desc: std::collections::HashMap<String, String> = {
                if let Some(index) = index_ref {
                    all_names
                        .iter()
                        .filter_map(|n| {
                            let entry = index.find_component_by_leaf(n)?;
                            if entry.description.is_empty() {
                                None
                            } else {
                                Some((n.clone(), entry.description.clone()))
                            }
                        })
                        .collect()
                } else {
                    std::collections::HashMap::new()
                }
            };

            // Group by leading dotted segment for compactness.
            // Filtering happens before grouping so empty groups don't
            // render at all.
            let mut groups: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            for name in all_names {
                if !filter_lower.is_empty()
                    && !name.to_ascii_lowercase().contains(&filter_lower)
                {
                    continue;
                }
                let head = name.split('.').next().unwrap_or(name.as_str()).to_string();
                groups.entry(head).or_default().push(name);
            }

            let mut toggles: Vec<(String, bool)> = Vec::new();
            for (group_name, names) in &groups {
                let picked_in_group = names
                    .iter()
                    .filter(|n| plotted.contains(*n) || picked_exp.contains(*n))
                    .count();
                let header = if picked_in_group > 0 {
                    format!("{} ({}/{})", group_name, picked_in_group, names.len())
                } else {
                    format!("{} ({})", group_name, names.len())
                };
                let default_open = !filter_lower.is_empty() || picked_in_group > 0;
                egui::CollapsingHeader::new(header)
                    .id_salt(format!("telem_var_group_{group_name}"))
                    .default_open(default_open)
                    .show(ui, |ui| {
                        for name in names {
                            let mut is_picked =
                                plotted.contains(name) || picked_exp.contains(name);
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut is_picked, "").changed() {
                                    toggles.push((name.clone(), is_picked));
                                }
                                let short = name
                                    .strip_prefix(&format!("{group_name}."))
                                    .unwrap_or(name);
                                let label =
                                    egui::Label::new(short).sense(egui::Sense::hover());
                                let resp = ui.add(label);
                                if let Some(desc) =
                                    var_desc.get(name).filter(|d| !d.trim().is_empty())
                                {
                                    resp.on_hover_text(desc);
                                    ui.label(
                                        egui::RichText::new(desc.trim())
                                            .italics()
                                            .color(muted)
                                            .size(11.0),
                                    )
                                    .on_hover_text(desc);
                                }
                            });
                        }
                    });
            }
            if groups.is_empty() {
                ui.weak("No variables match the filter.");
            }

            // Apply toggles after the loop — avoids reborrowing
            // resources mid-iteration. Each toggle writes to BOTH the
            // viz registry (live cosim) and ExperimentVisibility
            // (Fast Run) so the user picks once.
            for (name, on) in toggles {
                if let Some(e) = entity {
                    if let Some(mut reg) =
                        world.get_resource_mut::<lunco_viz::VisualizationRegistry>()
                    {
                        set_signal_plotted(
                            &mut reg,
                            target_plot,
                            lunco_viz::SignalRef::new(e, name.clone()),
                            on,
                        );
                    }
                }
                if let Some(mut states) = world
                    .get_resource_mut::<crate::ui::panels::experiments::PlotPanelStates>()
                {
                    states.set_var(target_plot, name, on);
                }
            }
            let _ = is_signal_plotted; // re-export available for future UIs
        });

        // Auto-Fit button was here but moved to the Graphs panel's own
        // toolbar — users couldn't find it buried at the bottom of
        // Telemetry. Telemetry now does parameters / inputs / variable
        // toggles only; graph-axis controls live on the graph itself.
    }
}

/// Empty-state for the runtime-telemetry section: a muted explanation
/// + an inline 🚀 Compile button that triggers the same
/// `CompileActiveModel` the toolbar fires. Used both pre-compile (no
/// stepper exists yet) and after a stepper loses its component
/// (post-Reset / mid-rebuild). Keeping the action in-panel means users
/// don't have to hunt the toolbar to escape the empty state.
fn render_runtime_hint(
    ui: &mut egui::Ui,
    muted: egui::Color32,
    world: &mut World,
    msg: &str,
) {
    let active_doc = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|w| w.active_document);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(msg).color(muted).size(11.0));
        if let Some(doc) = active_doc {
            if ui
                .small_button("🚀 Compile")
                .on_hover_text("Compile the active model and start the stepper (F5)")
                .clicked()
            {
                world
                    .commands()
                    .trigger(crate::ui::commands::CompileActiveModel {
                        doc,
                        class: String::new(),
                    });
            }
        }
    });
}

/// Render the parameter inspector for component nodes selected on
/// the active diagram. One header per node (instance — class), then
/// editable rows for every parameter on that class. Edits dispatch
/// `ModelicaOp::SetParameter` through the canvas's apply_ops pipeline
/// so they show up in the source, the projection, undo history, and
/// — once compiled — the simulator.
fn render_selected_components_inspector(
    ui: &mut egui::Ui,
    world: &mut World,
    muted: egui::Color32,
) {
    use crate::document::ModelicaOp;
    use crate::ui::panels::canvas_diagram::{
        active_class_for_doc, active_doc_from_world, apply_ops_public,
        CanvasDiagramState, IconNodeData,
    };

    let Some(doc_id) = active_doc_from_world(world) else { return };
    // Snapshot the selected nodes' (id, instance, class, params) up
    // front so we can release the canvas-state borrow before issuing
    // commands.queue / apply_ops_public mutations.
    struct NodeRow {
        instance: String,
        qualified_type: String,
        // (param_name, current_value).
        parameters: Vec<(String, String)>,
        // param_name → Modelica description-comment, for hover help.
        param_desc: std::collections::HashMap<String, String>,
    }
    let mut rows: Vec<NodeRow> = {
        let Some(state) = world.get_resource::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get(Some(doc_id));
        let scene = &docstate.canvas.scene;
        let selection = &docstate.canvas.selection;
        selection
            .iter()
            .filter_map(|item| match *item {
                lunco_canvas::SelectItem::Node(id) => {
                    let node = scene.node(id)?;
                    let icon = node.data.downcast_ref::<IconNodeData>()?;
                    Some(NodeRow {
                        instance: node.label.clone(),
                        qualified_type: icon.qualified_type.clone(),
                        parameters: icon.parameters.clone(),
                        param_desc: std::collections::HashMap::new(),
                    })
                }
                _ => None,
            })
            .collect()
    };
    if rows.is_empty() {
        return;
    }
    // Second pass (canvas borrow released): harvest each component
    // type's parameter description-comments from the index for hover
    // help on the rows below.
    {
        let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
        if let Some(host) = registry.host(doc_id) {
            let index = host.document().index();
            for row in &mut rows {
                row.param_desc = type_param_descriptions(index, &row.qualified_type);
            }
        }
    }

    let editing_class = active_class_for_doc(world, doc_id);

    egui::CollapsingHeader::new(format!(
        "🧩 Selected components ({})",
        rows.len()
    ))
    .default_open(true)
    .show(ui, |ui| {
        if editing_class.is_none() {
            ui.label(
                egui::RichText::new(
                    "No active class — open a model class on the canvas to edit parameters.",
                )
                .size(11.0)
                .color(muted),
            );
            return;
        }
        let class = editing_class.expect("class is Some by the branch above");
        // Per-node block. CollapsingHeader so multi-select stays
        // navigable on tall lists.
        for row in &rows {
            let leaf_type = row
                .qualified_type
                .rsplit('.')
                .next()
                .unwrap_or(&row.qualified_type)
                .to_string();
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} — {}", row.instance, leaf_type)).strong(),
            )
            .id_salt(("selected_component", row.instance.as_str()))
            .default_open(true)
            .show(ui, |ui| {
                if row.parameters.is_empty() {
                    ui.label(
                        egui::RichText::new("(no parameters)")
                            .size(11.0)
                            .color(muted)
                            .italics(),
                    );
                    return;
                }
                // Two-pass: collect edits during the row loop, apply
                // after the immutable borrow on `rows` is done. Using
                // a String value keeps the editor general — Modelica
                // params can be Real / Integer / Boolean / enumeration,
                // and `SetParameter` accepts a textual replacement.
                let mut edits: Vec<(String, String)> = Vec::new();
                for (name, value) in &row.parameters {
                    let mut buf = value.clone();
                    let desc = row.param_desc.get(name);
                    ui.horizontal(|ui| {
                        let name_resp = ui.add(
                            egui::Label::new(format!("{name:14}"))
                                .sense(egui::Sense::hover()),
                        );
                        if let Some(d) = desc {
                            name_resp.on_hover_text(d);
                        }
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .desired_width(120.0),
                        );
                        if let Some(d) = desc {
                            resp.clone().on_hover_text(d);
                        }
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            if buf != *value {
                                edits.push((name.clone(), buf.clone()));
                            }
                        } else if resp.lost_focus() && buf != *value {
                            edits.push((name.clone(), buf.clone()));
                        }
                    });
                }
                for (param, value) in edits {
                    apply_ops_public(
                        world,
                        doc_id,
                        vec![ModelicaOp::SetParameter {
                            class: class.clone(),
                            component: row.instance.clone(),
                            param,
                            value,
                        }],
                    );
                }
            });
        }
    });
    ui.separator();
}

/// Render every top-level `parameter` / `constant` declaration on the
/// active class as an editable list. Complements
/// [`render_selected_components_inspector`] by surfacing parameters
/// declared *directly* on the root model — these have no canvas icon
/// and would otherwise be unreachable through the inspector.
///
/// Reads from the document's [`crate::index::ModelicaIndex`] (already
/// kept current by the op pipeline) — no AST walk per frame, no engine
/// lock, no shadow ECS state. Edits dispatch
/// `ModelicaOp::SetParameter { component, param: "", value }` — the
/// `""` sentinel routes the value into the component's primary
/// binding.
/// One row in the flattened parameter view. `chain` is the component
/// instance hierarchy from the active class down — `[]` for a direct
/// parameter, `["tank"]` for `tank.volume`, `["engine","combustor"]`
/// for `engine.combustor.eta`. `value` already accounts for
/// modification overrides on the parent's component declaration.
struct FlatParam {
    chain: Vec<String>,
    leaf: String,
    value: String,
    /// Modelica description-comment for this parameter
    /// (`parameter Real g = 9.81 "Gravity"`). Empty when none authored.
    /// Surfaced as hover help on the parameter row.
    description: String,
}

impl FlatParam {
    fn path(&self) -> String {
        if self.chain.is_empty() {
            self.leaf.clone()
        } else {
            format!("{}.{}", self.chain.join("."), self.leaf)
        }
    }
    fn depth(&self) -> usize {
        self.chain.len()
    }
}

/// Recursively flatten a class's parameters using only the local
/// [`ModelicaIndex`] — no engine session, no async lookup. Always
/// synchronous with the latest parse, never lags behind an edit,
/// never depends on the rumoca-compile bookkeeping for the active
/// document.
///
/// Walk order per class:
/// 1. Each `extends` base (resolved against `index.classes`) is
///    flattened first; its rows merge into the deriving class's set
///    so inherited parameters surface alongside locally-declared ones.
/// 2. Then the class's own components: Parameter/Constant declarations
///    emit rows; component instances recurse into their type's class
///    entry. Modifications on the parent's component declaration
///    (`Tank tank(volume = 4000)`) shadow the type's declared default.
///
/// Type / base resolution: try the name verbatim against
/// `index.classes`, then qualified by `index.within_path` if any.
/// Names that don't resolve in this document's index are silently
/// skipped — typically MSL or other-doc types. Those rows are absent
/// rather than misleadingly empty; user can drill into the component
/// via the canvas to see its params.
///
/// Cycle / fanout safety: bounded by `MAX_DEPTH` and a `visited` set
/// on class qualified-names that guards both the component-recursion
/// path and the extends-recursion path. Dedupe-by-leaf within a class
/// so an `extends` and a direct declaration of the same param don't
/// double-emit (deriving class wins, Modelica-correct).
/// Map a class's directly-declared parameter/component names to their
/// Modelica description-comments. Resolves `type_name` against the
/// index (direct key, then short-name suffix match) and harvests
/// non-empty descriptions. Used for hover help on parameter rows.
fn type_param_descriptions(
    index: &crate::index::ModelicaIndex,
    type_name: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let type_path = if index.classes.contains_key(type_name) {
        Some(type_name.to_string())
    } else {
        let short = type_name.rsplit('.').next().unwrap_or(type_name);
        index
            .classes
            .keys()
            .find(|k| k.rsplit('.').next() == Some(short))
            .cloned()
    };
    if let Some(tp) = type_path {
        if let Some(keys) = index.components_by_class.get(&tp) {
            for key in keys {
                if let Some(comp) = index.components.get(key.0 as usize) {
                    if !comp.description.is_empty() {
                        out.insert(comp.name.clone(), comp.description.clone());
                    }
                }
            }
        }
    }
    out
}

fn flatten_class_parameters(
    index: &crate::index::ModelicaIndex,
    class: &str,
) -> Vec<FlatParam> {
    const MAX_DEPTH: usize = 4;

    /// Modelica-style class lookup, scope-walked. Tries the type
    /// name verbatim first, then walks up the enclosing-package chain
    /// of `scope` (e.g. `AnnotatedRocketStage.RocketStage` →
    /// `AnnotatedRocketStage.Tank` resolves `Tank` to its sibling),
    /// then falls back to `within_path` if any.
    fn resolve_class<'a>(
        index: &'a crate::index::ModelicaIndex,
        scope: &str,
        type_name: &str,
    ) -> Option<&'a String> {
        if let Some((k, _)) = index.classes.get_key_value(type_name) {
            return Some(k);
        }
        let mut cur = scope;
        loop {
            let candidate = if cur.is_empty() {
                type_name.to_string()
            } else {
                format!("{cur}.{type_name}")
            };
            if let Some((k, _)) = index.classes.get_key_value(&candidate) {
                return Some(k);
            }
            if cur.is_empty() {
                break;
            }
            cur = crate::ast_extract::parent_qualified(cur);
        }
        if let Some(w) = index.within_path.as_ref() {
            let q = format!("{w}.{type_name}");
            if let Some((k, _)) = index.classes.get_key_value(&q) {
                return Some(k);
            }
        }
        None
    }

    fn walk(
        index: &crate::index::ModelicaIndex,
        owner_class: &str,
        type_path: &str,
        chain: &mut Vec<String>,
        visited: &mut std::collections::HashSet<String>,
        out: &mut Vec<FlatParam>,
    ) {
        if chain.len() > MAX_DEPTH || !visited.insert(type_path.to_string()) {
            return;
        }

        // Snapshot the count before this class's recursion so we can
        // dedupe new entries by leaf name against this class's own
        // contributions (and inherited entries from earlier in the
        // walk). Modelica resolution is "deriving class wins"; we
        // achieve that by emitting `extends` first and skipping any
        // later entry whose leaf name already appeared.
        let pre_len = out.len();

        // 1) Inherited members from extends bases. Each base is
        // resolved like a component type. Recurse with the same chain
        // (no new prefix — extends is "as if declared here").
        if let Some(class_entry) = index.classes.get(type_path) {
            for base in &class_entry.extends {
                if let Some(base_path) = resolve_class(index, type_path, base) {
                    walk(index, owner_class, base_path, chain, visited, out);
                }
            }
        }

        // 2) Direct components on this class.
        let Some(keys) = index.components_by_class.get(type_path) else {
            visited.remove(type_path);
            return;
        };
        for key in keys {
            let Some(comp) = index.components.get(key.0 as usize) else { continue };
            match comp.variability {
                crate::index::Variability::Parameter | crate::index::Variability::Constant => {
                    // Modification override on the parent's component
                    // declaration shadows this class's declared default.
                    // For depth 0 (the root call) chain.last() is None,
                    // so no override applies — value is the binding text.
                    let override_value = chain.last().and_then(|parent_comp| {
                        index
                            .find_component(owner_class, parent_comp)
                            .and_then(|c| c.modifications.get(&comp.name).cloned())
                    });
                    let value = override_value
                        .or_else(|| comp.binding.clone())
                        .unwrap_or_default();
                    // Dedupe-by-leaf within this class's contribution so
                    // an inherited param is not re-emitted by a direct
                    // declaration of the same name.
                    let already_seen = out[pre_len..]
                        .iter()
                        .any(|fp| fp.chain == *chain && fp.leaf == comp.name);
                    if !already_seen {
                        out.push(FlatParam {
                            chain: chain.clone(),
                            leaf: comp.name.clone(),
                            value,
                            description: comp.description.clone(),
                        });
                    }
                }
                _ => {
                    // Component instance — recurse into its type with
                    // an extended chain. Parent for override lookup at
                    // the next level is THIS class, since modifications
                    // live on the parent's declaration.
                    let Some(child_type) = resolve_class(index, type_path, &comp.type_name)
                    else {
                        // Unresolvable type (MSL / cross-doc) — skip.
                        continue;
                    };
                    chain.push(comp.name.clone());
                    walk(index, type_path, child_type, chain, visited, out);
                    chain.pop();
                }
            }
        }

        visited.remove(type_path);
    }

    let mut out = Vec::new();
    let mut chain: Vec<String> = Vec::new();
    let mut visited: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    walk(index, class, class, &mut chain, &mut visited, &mut out);

    // Drop sub-component rows whose value is a bare identifier matching
    // a top-level parameter — those are bindings (e.g. `airframe.g` ←
    // outer `g`), not independent knobs. Editing them is a no-op because
    // they're rebound at compile time; surfacing them just clutters the
    // panel and confuses users (issue: typing -10 in `airframe.g`
    // doesn't change anything since the row is bound to `g`).
    let top_level: std::collections::HashSet<String> = out
        .iter()
        .filter(|fp| fp.chain.is_empty())
        .map(|fp| fp.leaf.clone())
        .collect();
    out.retain(|fp| {
        if fp.chain.is_empty() {
            return true;
        }
        let v = fp.value.trim();
        let is_bare_ident =
            !v.is_empty() && v.chars().all(|c| c.is_alphanumeric() || c == '_');
        !(is_bare_ident && top_level.contains(v))
    });
    out
}

/// Convert a parameter's raw AST literal into a human-friendly
/// decimal form. Modelica authors routinely write `3.0e6` /
/// `1.0e-4`; that exponent form is fine for source but unreadable
/// in a parameter editor. If the string parses cleanly as a number
/// we reformat without scientific notation; anything that doesn't
/// parse (expressions, strings, booleans, enum literals) is left
/// untouched.
fn format_param_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(v) = trimmed.parse::<f64>() else {
        return raw.to_string();
    };
    if !v.is_finite() {
        return raw.to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    // Pick a precision that preserves the value but trims trailing
    // zeros. `{:.*}` with 12 decimals + trim handles both very large
    // (3e6 → "3000000") and small (1e-4 → "0.0001") cleanly.
    let mag = v.abs();
    let decimals = if mag >= 1.0 {
        // Show up to 6 fractional digits for "normal" sized values.
        6
    } else {
        // Small values need more decimals to retain significance.
        let zeros = (-mag.log10()).ceil() as usize;
        (zeros + 6).min(20)
    };
    let s = format!("{:.*}", decimals, v);
    let s = if s.contains('.') {
        let t = s.trim_end_matches('0');
        let t = t.trim_end_matches('.');
        t.to_string()
    } else {
        s
    };
    s
}

fn render_active_class_parameters(
    ui: &mut egui::Ui,
    world: &mut World,
    muted: egui::Color32,
) {
    use crate::document::ModelicaOp;
    use crate::ui::panels::canvas_diagram::{
        active_class_for_doc, active_doc_from_world, apply_ops_public,
    };

    let Some(doc_id) = active_doc_from_world(world) else { return };
    let Some(active) = active_class_for_doc(world, doc_id) else { return };

    // Flatten using only the local AST index — synchronous with the
    // latest parse, no engine session dependency, no async lag.
    // Also snapshot the doc's read-only state so we can disable
    // editing on bundled / locked-source documents (otherwise the
    // user types into a field that silently rejects the write).
    let (rows, read_only): (Vec<FlatParam>, bool) = {
        let Some(registry) = world.get_resource::<crate::ui::ModelicaDocumentRegistry>() else {
            return;
        };
        let Some(host) = registry.host(doc_id) else { return };
        let doc = host.document();
        (
            flatten_class_parameters(doc.index(), &active),
            doc.is_read_only(),
        )
    };
    if rows.is_empty() {
        return;
    }

    egui::CollapsingHeader::new(format!("⚙ Parameters ({})", rows.len()))
        .id_salt("active_class_parameters")
        .default_open(true)
        .show(ui, |ui| {
            // Editable rows are depth ≤ 1 — direct parameters of the
            // active class (`SetParameter { component, param: "" }`)
            // and one-level component modifications
            // (`SetParameter { component, param }`). `ast_mut::set_parameter`
            // doesn't yet walk dotted component paths, so deeper rows
            // render read-only with the muted style. Two-pass: collect
            // edits, apply after the borrow on `rows` is released.
            let mut edits: Vec<(String, String, String)> = Vec::new(); // (component, param, value)
            // Bound the list height so a model with dozens of parameters
            // (40+ here) scrolls within its own region instead of pushing
            // the rest of the Telemetry panel off-screen — this section
            // renders outside the panel's main scroll (pre-compile path).
            egui::ScrollArea::vertical()
                .id_salt("active_params_scroll")
                .max_height(320.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
            // Two-column grid so every value field starts at the same x,
            // regardless of name length. (Space-padding a proportional
            // font never lines up.)
            egui::Grid::new("active_params_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
            for row in &rows {
                // Read-only docs (bundled examples) can't accept any
                // edits — fall back to the muted display path for
                // every row.
                let editable = !read_only && row.depth() <= 1;
                let path = row.path();
                {
                    let display_value = format_param_value(&row.value);
                    // Modelica description-comment, shown as hover help on
                    // both the name and the value cell.
                    let desc = (!row.description.is_empty()).then(|| row.description.clone());
                    if editable {
                        let name_resp = ui.add(
                            egui::Label::new(path.clone())
                                .sense(egui::Sense::hover()),
                        );
                        if let Some(d) = &desc {
                            name_resp.on_hover_text(d);
                        }
                        // Persist the edit buffer in egui memory keyed
                        // by the row's path. The naive `let mut buf =
                        // row.value.clone()` resets keystrokes every
                        // frame because `row.value` comes from
                        // rumoca-compile's cache, which only refreshes
                        // ~1.5s after the source patch. While the field
                        // has focus we keep the user's draft; on focus
                        // loss we drop the draft so the next frame
                        // syncs to whatever the AST now says.
                        let edit_id = egui::Id::new(("telem_flat_param", path.as_str()));
                        // Latched buffer: persists the committed value
                        // until `display_value` (rumoca-compile cache,
                        // ~1.5s lag) catches up. Without this, the field
                        // visually reverts to the stale pre-edit value
                        // on focus loss, and a subsequent click+blur
                        // re-fires the commit against a now-stale
                        // ast_mut text range, producing
                        // "text range out of bounds" failures.
                        let latched: Option<String> =
                            ui.data_mut(|d| d.get_temp::<String>(edit_id));
                        let mut buf = latched
                            .clone()
                            .unwrap_or_else(|| display_value.clone());
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .id(edit_id)
                                .desired_width(120.0),
                        );
                        if let Some(d) = &desc {
                            resp.clone().on_hover_text(d);
                        }
                        if resp.has_focus() {
                            ui.data_mut(|d| d.insert_temp(edit_id, buf.clone()));
                        }
                        // Change detection compares against the latched
                        // committed value (or display_value if nothing
                        // is latched yet) — never against the lagging
                        // display_value alone.
                        let baseline = latched
                            .clone()
                            .unwrap_or_else(|| display_value.clone());
                        let commit = resp.lost_focus()
                            && (ui.input(|i| i.key_pressed(egui::Key::Enter))
                                || buf != baseline);
                        if commit && buf != baseline {
                            let (component, param) = if row.depth() == 0 {
                                (row.leaf.clone(), String::new())
                            } else {
                                (row.chain[0].clone(), row.leaf.clone())
                            };
                            bevy::log::info!(
                                "[Telemetry] commit param edit: class='{}' component='{}' param='{}' value='{}' (was '{}')",
                                active, component, param, buf, row.value
                            );
                            edits.push((component, param, buf.clone()));
                            // Latch the committed value so the field
                            // keeps showing it until the AST/cache
                            // reflects the change.
                            ui.data_mut(|d| d.insert_temp(edit_id, buf.clone()));
                        } else if !resp.has_focus() {
                            // Drop the latch once display_value reflects
                            // it; otherwise leave it in place.
                            if let Some(l) = &latched {
                                if l == &display_value {
                                    ui.data_mut(|d| d.remove::<String>(edit_id));
                                }
                            }
                        }
                    } else {
                        // Deeper than one level — read-only label. Edit
                        // by drilling into the component's class via
                        // the canvas, then editing there.
                        let ro_name = ui.add(
                            egui::Label::new(
                                egui::RichText::new(path.clone())
                                    .color(muted)
                                    .size(11.0),
                            )
                            .sense(egui::Sense::hover()),
                        );
                        if let Some(d) = &desc {
                            ro_name.on_hover_text(d);
                        }
                        ui.label(
                            egui::RichText::new(&display_value)
                                .monospace()
                                .size(11.0),
                        );
                    }
                }
                ui.end_row();
            }
                }); // end parameter grid
            }); // end bounded parameter-list scroll
            if edits.is_empty() {
                return;
            }
            let ops: Vec<ModelicaOp> = edits
                .into_iter()
                .map(|(component, param, value)| ModelicaOp::SetParameter {
                    class: active.clone(),
                    component,
                    param,
                    value,
                })
                .collect();
            apply_ops_public(world, doc_id, ops);
        });
    ui.separator();
}
