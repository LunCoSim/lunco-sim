//! Telemetry panel — model parameters, inputs, and variable plotting toggles.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use std::collections::HashMap;

use crate::ui::{ModelicaDocumentRegistry, WorkbenchState};
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};

/// Telemetry panel — model parameters, inputs, and variable plotting toggles.
pub struct TelemetryPanel;

impl WorkbenchPanel for TelemetryPanel {
    fn id(&self) -> &str { "modelica_inspector" }
    fn title(&self) -> String { "📊 Telemetry".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(35, 35, 40))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
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
        let (model_name, is_paused, current_time, parameters, inputs) = {
            if let Some(model) = world.get::<ModelicaModel>(entity) {
                (model.model_name.clone(), model.paused, model.current_time,
                 model.parameters.clone(), model.inputs.clone())
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
                if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                    s.history.remove(&entity);
                }
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
                        ui.label(format!("{key:16}:"));
                        let mut v = val;
                        if ui.add(egui::DragValue::new(&mut v).speed(0.01).fixed_decimals(2)).changed() {
                            let mut trigger_update = false;
                            let mut model_name = String::new();
                            let mut session_id = 0;
                            let mut new_params = HashMap::new();

                            // Read canonical source from the Document registry
                            // (falls back to the legacy component field only if
                            // the entity was somehow never checkpointed — e.g.
                            // a pre-migration Modelica flow we haven't touched
                            // yet). Once every Modelica spawn path goes through
                            // the registry, the fallback can be dropped.
                            let source = world
                                .get_resource::<ModelicaDocumentRegistry>()
                                .and_then(|r| r.host(entity).map(|h| h.document().source().to_string()));

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

                            let source = source.unwrap_or_else(|| {
                                world
                                    .query::<&ModelicaModel>()
                                    .get(world, entity)
                                    .map(|m| m.original_source.to_string())
                                    .unwrap_or_default()
                            });

                            if trigger_update {
                                if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                                    let new_source = crate::ast_extract::substitute_params_in_source(&source, &new_params);
                                    let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                                        entity,
                                        session_id,
                                        model_name,
                                        source: new_source,
                                    });
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
                        ui.label(format!("{key:16}:"));
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

        // Variables (Toggle to Plot)
        ui.label("Variables (Toggle to Plot):");
        egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
            // Read current plotted variables and model variables
            let (plotted, model_vars, model_inputs) = {
                let state = world.resource::<WorkbenchState>();
                let p = state.plotted_variables.clone();
                let (vars, inps) = if let Some(m) = world.get::<ModelicaModel>(entity) {
                    (m.variables.keys().cloned().collect::<Vec<_>>(),
                     m.inputs.keys().cloned().collect::<Vec<_>>())
                } else {
                    (Vec::new(), Vec::new())
                };
                (p, vars, inps)
            };

            let mut all_names: Vec<_> = model_vars;
            all_names.extend(model_inputs);
            all_names.sort();
            all_names.dedup();

            for name in all_names {
                let mut is_plotted = plotted.contains(&name);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut is_plotted, "").changed() {
                        if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                            if is_plotted {
                                s.plotted_variables.insert(name.clone());
                            } else {
                                s.plotted_variables.remove(&name);
                            }
                        }
                    }
                    ui.label(&name);
                });
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.add_space(ui.available_width() - 80.0);
            if ui.button("🔍 Auto-Fit").clicked() {
                if let Some(mut st) = world.get_resource_mut::<WorkbenchState>() {
                    st.plot_auto_fit = true;
                }
            }
        });
    }
}
