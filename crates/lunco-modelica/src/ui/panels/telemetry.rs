//! Telemetry panel — model parameters, inputs, and variable plotting toggles.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use std::collections::HashMap;

use crate::ui::{CompileState, CompileStates, ModelicaDocumentRegistry, WorkbenchState};
use crate::ui::viz::{is_signal_plotted, set_signal_plotted};
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};

/// Telemetry panel — model parameters, inputs, and variable plotting toggles.
pub struct TelemetryPanel;

impl Panel for TelemetryPanel {
    fn id(&self) -> PanelId { PanelId("modelica_inspector") }
    fn title(&self) -> String { "📊 Telemetry".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Fix selection leakage
        ui.style_mut().interaction.selectable_labels = false;

        // Auto-select first ModelicaModel entity if none selected (matches old behavior)
        {
            let needs_select = world.get_resource::<WorkbenchState>()
                .map_or(true, |s| s.selected_entity.is_none());
            if needs_select {
                type Q = bevy::ecs::query::QueryState<Entity, bevy::ecs::query::With<crate::ModelicaModel>>;
                let mut query_state = Q::new(world);
                if let Some(entity) = query_state.iter(world).next() {
                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.selected_entity = Some(entity);
                    }
                }
            }
        }

        // Read snapshot of state to avoid borrow conflicts
        let (entity, has_data) = {
            let state = match world.get_resource::<WorkbenchState>() {
                Some(s) => s,
                None => { ui.label("No state"); return; },
            };
            let e = state.selected_entity;
            let has = e.map(|e| world.get::<ModelicaModel>(e).is_some()).unwrap_or(false);
            (e, has)
        };

        let Some(entity) = entity else {
            ui.label("No model selected.");
            return;
        };
        if !has_data {
            ui.label("Model not found.");
            return;
        }

        // Read model snapshot for display
        let (model_name, is_paused, current_time, parameters, inputs, descriptions) = {
            if let Some(model) = world.get::<ModelicaModel>(entity) {
                (model.model_name.clone(), model.paused, model.current_time,
                 model.parameters.clone(), model.inputs.clone(),
                 model.descriptions.clone())
            } else {
                ui.label("Model not found.");
                return;
            }
        };

        let display_name = world.query::<Option<&Name>>().get(world, entity).ok().flatten()
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| "Unnamed Model".to_string());

        ui.heading(format!("{display_name} ({model_name})"));

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
                // The worker's Reset handler pushes a fresh set of
                // samples into `SignalRegistry`; clearing per-signal
                // history is handled there.
            }
        });
        ui.separator();

        // Parameters
        if !parameters.is_empty() {
            ui.label("Parameters (Dynamic Tuning):");
            egui::ScrollArea::vertical().id_salt("params_scroll").max_height(150.0).show(ui, |ui| {
                let mut param_keys: Vec<_> = parameters.keys().cloned().collect();
                param_keys.sort();
                for key in &param_keys {
                    let val = parameters.get(key).copied().unwrap_or(0.0);
                    ui.horizontal(|ui| {
                        // Hover the name label for the Modelica
                        // description string (MLS §A.2.5), if any.
                        //
                        // `ui.label()` makes a non-interactive widget;
                        // `on_hover_text` silently no-ops there. Go
                        // through `Label::new(...).sense(Sense::hover())`
                        // so the response is actually hoverable.
                        let label = egui::Label::new(format!("{key:16}:"))
                            .sense(egui::Sense::hover());
                        let resp = ui.add(label);
                        if let Some(desc) = descriptions.get(key) {
                            resp.on_hover_text(desc);
                        }
                        let mut v = val;
                        if ui.add(egui::DragValue::new(&mut v).speed(0.01).fixed_decimals(2)).changed() {
                            let mut trigger_update = false;
                            let mut model_name = String::new();
                            let mut session_id = 0;
                            let mut new_params = HashMap::new();

                            // Resolve entity → DocumentId → source via the
                            // registry. If either lookup misses, the entity
                            // hasn't been through Compile/UpdateParameters yet
                            // and there's nothing coherent to substitute into.
                            let (doc_id, source) = {
                                let registry = world.resource::<ModelicaDocumentRegistry>();
                                let doc = registry.document_of(entity);
                                let src = doc.and_then(|d| registry.host(d))
                                    .map(|h| h.document().source().to_string());
                                (doc, src)
                            };

                            if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                                if let Some(p) = m.parameters.get_mut(key) {
                                    *p = v;
                                    trigger_update = true;
                                    model_name = m.model_name.clone();
                                    m.session_id += 1;
                                    session_id = m.session_id;
                                    new_params = m.parameters.clone();
                                    m.is_stepping = true; // prevent steps while updating
                                }
                            }

                            if trigger_update {
                                if let (Some(doc), Some(source)) = (doc_id, source) {
                                    let new_source = crate::ast_extract::substitute_params_in_source(&source, &new_params);
                                    // Checkpoint the parameter-substituted
                                    // source into the Document BEFORE sending
                                    // to the worker — the Document remains the
                                    // single source of truth even if the
                                    // worker result never arrives.
                                    world
                                        .resource_mut::<ModelicaDocumentRegistry>()
                                        .checkpoint_source(doc, new_source.clone());
                                    // UpdateParameters recompiles on the
                                    // worker side, so mark the document as
                                    // compiling until the result lands.
                                    world
                                        .resource_mut::<CompileStates>()
                                        .set(doc, CompileState::Compiling);
                                    if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                                        let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                                            entity,
                                            session_id,
                                            model_name,
                                            source: new_source,
                                        });
                                    }
                                }
                            }
                        }
                    });
                }
            });
            ui.separator();
        }

        // Inputs
        if !inputs.is_empty() {
            ui.label("Inputs (Real-time):");
            egui::ScrollArea::vertical().id_salt("inputs_scroll").max_height(120.0).show(ui, |ui| {
                let mut input_keys: Vec<_> = inputs.keys().cloned().collect();
                input_keys.sort();
                for key in input_keys {
                    let val = inputs.get(&key).copied().unwrap_or(0.0);
                    ui.horizontal(|ui| {
                        let label = egui::Label::new(format!("{key:16}:"))
                            .sense(egui::Sense::hover());
                        let resp = ui.add(label);
                        if let Some(desc) = descriptions.get(&key) {
                            resp.on_hover_text(desc);
                        }
                        let mut v = val;
                        ui.add(egui::DragValue::new(&mut v).speed(0.1).fixed_decimals(2));
                        if (v - val).abs() > 1e-10 {
                            if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                                if let Some(inp) = m.inputs.get_mut(&key) { *inp = v; }
                            }
                        }
                    });
                }
            });
            ui.separator();
        }

        // Variables (Toggle to Plot).
        //
        // Checkboxes read / write the default Modelica plot's
        // `VisualizationConfig.inputs` directly — no shadow state,
        // no per-frame sync. Toggling here instantly shows/hides the
        // variable in the Graphs panel since both read the same
        // config.
        ui.label("Variables (Toggle to Plot):");
        egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
            let (model_vars, model_inputs) = if let Some(m) = world.get::<ModelicaModel>(entity) {
                (m.variables.keys().cloned().collect::<Vec<_>>(),
                 m.inputs.keys().cloned().collect::<Vec<_>>())
            } else {
                (Vec::new(), Vec::new())
            };

            // Read plotted-set from the viz registry. Clone once so
            // we don't reborrow the resource inside the loop.
            let plotted: std::collections::HashSet<String> = world
                .get_resource::<lunco_viz::VisualizationRegistry>()
                .and_then(|r| r.get(crate::ui::viz::DEFAULT_MODELICA_GRAPH))
                .map(|cfg| cfg.inputs.iter()
                    .filter(|b| b.source.entity == entity)
                    .map(|b| b.source.path.clone())
                    .collect())
                .unwrap_or_default();

            let mut all_names: Vec<_> = model_vars;
            all_names.extend(model_inputs);
            all_names.sort();
            all_names.dedup();

            for name in all_names {
                let mut is_plotted = plotted.contains(&name);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut is_plotted, "").changed() {
                        if let Some(mut reg) =
                            world.get_resource_mut::<lunco_viz::VisualizationRegistry>()
                        {
                            set_signal_plotted(
                                &mut reg,
                                lunco_viz::SignalRef::new(entity, name.clone()),
                                is_plotted,
                            );
                        }
                    }
                    let label = egui::Label::new(&name).sense(egui::Sense::hover());
                    let resp = ui.add(label);
                    if let Some(desc) = descriptions.get(&name) {
                        resp.on_hover_text(desc);
                    }
                });
                let _ = is_signal_plotted; // re-export available for future UIs
            }
        });

        // Auto-Fit button was here but moved to the Graphs panel's own
        // toolbar — users couldn't find it buried at the bottom of
        // Telemetry. Telemetry now does parameters / inputs / variable
        // toggles only; graph-axis controls live on the graph itself.
    }
}
