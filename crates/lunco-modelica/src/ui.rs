//! Professional Modelica Engineering Workbench.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand, extract_model_name, extract_parameters};

pub struct ModelicaInspectorPlugin;

impl Plugin for ModelicaInspectorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
           .add_systems(EguiPrimaryContextPass, (
               show_library_browser,
               show_model_editor,
               show_telemetry,
               show_plots,
               show_logs,
           ));
    }
}

#[derive(Resource)]
pub struct WorkbenchState {
    pub current_path: PathBuf,
    pub editor_buffer: String,
    pub selected_entity: Option<Entity>,
    pub compilation_error: Option<String>,
    /// History of variables: Entity -> VariableName -> DataPoints
    pub history: HashMap<Entity, HashMap<String, VecDeque<[f64; 2]>>>,
    pub plotted_variables: std::collections::HashSet<String>,
    pub logs: VecDeque<String>,
    pub max_history: usize,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            current_path: PathBuf::from("assets/models"),
            editor_buffer: String::new(),
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: std::collections::HashSet::new(),
            logs: VecDeque::new(),
            max_history: 10000,
        }
    }
}

/// Window 1: Library Browser
fn show_library_browser(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    egui::Window::new("📁 Library Browser")
        .default_pos([10.0, 10.0])
        .default_size([250.0, 400.0])
        .resizable(true)
        .show(ctx, |ui| {
            render_browser(ui, &mut state);
        });
}

/// Window 2: Model Editor
fn show_model_editor(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
    channels: Option<Res<ModelicaChannels>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    // Auto-select the first model if none selected
    if state.selected_entity.is_none() {
        state.selected_entity = q_models.iter().map(|(e, _)| e).next();
    }

    egui::Window::new("📝 Modelica Editor")
        .default_pos([270.0, 10.0])
        .default_size([600.0, 500.0])
        .min_width(400.0)
        .max_width(1000.0)
        .resizable(true)
        .show(ctx, |ui| {
            let detected_name = extract_model_name(&state.editor_buffer);

            ui.horizontal(|ui| {
                ui.heading(format!("Editor: {}", detected_name.as_deref().unwrap_or("Unknown")));
                ui.add_space(ui.available_width() - 150.0);

                if ui.button("🚀 COMPILE & RUN").clicked() {
                    if let Some(model_name) = detected_name {
                        if let (Some(entity), Some(channels)) = (state.selected_entity, &channels) {
                            // Pre-discover parameters for immediate UI update
                            let initial_params = extract_parameters(&state.editor_buffer);
                            if let Ok((_, mut model)) = q_models.get_mut(entity) {
                                model.parameters = initial_params;
                                model.inputs.clear();
                                model.variables.clear();
                            }
                            
                            let _ = channels.tx.send(ModelicaCommand::Compile {
                                entity,
                                model_name,
                                source: state.editor_buffer.clone(),
                            });
                        }
                    } else {
                        state.compilation_error = Some("Could not find a valid model declaration.".to_string());
                    }
                }
                
                if state.compilation_error.is_some() {
                    ui.colored_label(egui::Color32::LIGHT_RED, "⚠️ Error!");
                    if ui.button("Clear").clicked() { state.compilation_error = None; }
                } else {
                    ui.colored_label(egui::Color32::GREEN, "Ready");
                }
            });

            ui.separator();

            egui::ScrollArea::both().id_salt("editor_scroll").show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut state.editor_buffer)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .desired_rows(35)
                );
            });

            if let Some(err) = &state.compilation_error {
                ui.separator();
                egui::ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                });
            }
        });
}

/// Window 3: Live Telemetry & Dynamic Controls
fn show_telemetry(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
    mut q_models: Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<Res<ModelicaChannels>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📊 Live Telemetry")
        .default_pos([880.0, 10.0])
        .default_size([300.0, 400.0])
        .resizable(true)
        .show(ctx, |ui| {
            if let Some(entity) = state.selected_entity {
                if let Ok((_, mut model, name)) = q_models.get_mut(entity) {
                    let label = name.map(|n| n.as_str()).unwrap_or("Unnamed Model");
                    ui.heading(format!("{} ({})", label, model.model_name));

                    ui.horizontal(|ui| {
                        if model.paused {
                            if ui.button("▶ Play").clicked() { model.paused = false; }
                        } else {
                            if ui.button("⏸ Pause").clicked() { model.paused = true; }
                        }
                        ui.label(format!("Time: {:.4} s", model.current_time));
                        
                        ui.add_space(ui.available_width() - 80.0);
                        if ui.button("🔄 Reset").clicked() {
                            if let Some(channels) = &channels {
                                let _ = channels.tx.send(ModelicaCommand::Reset { entity });
                            }
                            state.history.remove(&entity);
                            model.current_time = 0.0;
                            model.last_step_time = 0.0;
                        }
                    });

                    ui.separator();

                    if !model.parameters.is_empty() {
                        ui.label("Parameters (Dynamic Tuning):");
                        egui::ScrollArea::vertical().id_salt("params_scroll").max_height(150.0).show(ui, |ui| {
                            let mut param_keys: Vec<_> = model.parameters.keys().cloned().collect();
                            param_keys.sort();
                            let mut changed = false;
                            for key in param_keys {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}:", key));
                                    let val = model.parameters.get_mut(&key).unwrap();
                                    if ui.add(egui::DragValue::new(val).speed(0.01)).changed() {
                                        changed = true;
                                    }
                                });
                            }
                            
                            if changed {
                                if let Some(channels) = &channels {
                                    let params: Vec<(String, f64)> = model.parameters.iter().map(|(k, v)| (k.clone(), *v)).collect();
                                    let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                                        entity,
                                        model_path: model.model_path.clone(),
                                        model_name: model.model_name.clone(),
                                        parameters: params,
                                    });
                                }
                            }
                        });
                        ui.separator();
                    }

                    if !model.inputs.is_empty() {
                        ui.label("Control Inputs (Real-time):");
                        egui::ScrollArea::vertical().id_salt("inputs_scroll").max_height(120.0).show(ui, |ui| {
                            let mut input_keys: Vec<_> = model.inputs.keys().cloned().collect();
                            input_keys.sort();
                            for key in input_keys {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}:", key));
                                    let val = model.inputs.get_mut(&key).unwrap();
                                    ui.add(egui::DragValue::new(val).speed(0.1));
                                });
                            }
                        });
                        ui.separator();
                    }

                    ui.label("Live Variables (Toggle to Plot):");
                    egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
                        let mut var_names: Vec<_> = model.variables.keys().cloned().collect();
                        var_names.sort();

                        for name in var_names {
                            ui.horizontal(|ui| {
                                let mut is_plotted = state.plotted_variables.contains(&name);
                                if ui.checkbox(&mut is_plotted, "").changed() {
                                    if is_plotted { state.plotted_variables.insert(name.clone()); }
                                    else { state.plotted_variables.remove(&name); }
                                }
                                ui.label(format!("{}:", name));
                                
                                if let Some(val) = model.variables.get(&name) {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.monospace(format!("{:.4}", val));
                                    });
                                }
                            });
                        }
                    });
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Waiting for model...");
                });
            }
        });
}

/// Window 4: Plots
fn show_plots(
    mut contexts: EguiContexts,
    state: Res<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📈 Variable Plots")
        .default_pos([270.0, 520.0])
        .default_size([600.0, 300.0])
        .resizable(true)
        .show(ctx, |ui| {
            if state.plotted_variables.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("Check variables in Live Telemetry to plot them.");
                });
            } else {
                Plot::new("workbench_plot")
                    .view_aspect(2.0)
                    .legend(egui_plot::Legend::default())
                    .show(ui, |plot_ui| {
                        if let Some(entity) = state.selected_entity {
                            if let Some(entity_history) = state.history.get(&entity) {
                                for var_name in &state.plotted_variables {
                                    if let Some(data) = entity_history.get(var_name) {
                                        let points: Vec<[f64; 2]> = data.iter().cloned().collect();
                                        plot_ui.line(Line::new(var_name.clone(), PlotPoints::from(points)));
                                    }
                                }
                            }
                        }
                    });
            }
        });
}

/// Window 5: Engineering Console (Logs)
fn show_logs(
    mut contexts: EguiContexts,
    state: Res<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📟 Engineering Console")
        .default_pos([10.0, 420.0])
        .default_size([250.0, 200.0])
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("log_scroll")
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for log in &state.logs {
                        ui.label(log);
                    }
                });
        });
}

fn render_browser(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    ui.horizontal(|ui| {
        if ui.button("🏠").on_hover_text("Projects").clicked() {
            state.current_path = PathBuf::from("assets/models");
        }
        if ui.button("📚").on_hover_text("MSL 4.0").clicked() {
            state.current_path = PathBuf::from(".cache/msl/ModelicaStandardLibrary-4.0.0");
        }
        ui.separator();
        if ui.button("⬆").clicked() {
            if let Some(parent) = state.current_path.parent() {
                if state.current_path.starts_with(".cache/msl") || state.current_path.starts_with("assets/models") {
                     state.current_path = parent.to_path_buf();
                }
            }
        }
    });
    
    ui.label(format!("Path: {:?}", state.current_path));
    ui.separator();

    if let Ok(entries) = std::fs::read_dir(&state.current_path) {
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            
            if path.is_dir() {
                if ui.link(format!("📁 {}", name)).clicked() {
                    state.current_path = path;
                }
            } else if name.ends_with(".mo") {
                if ui.link(format!("📄 {}", name)).clicked() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        state.editor_buffer = content;
                    }
                }
            }
        }
    }
}
