//! Professional Modelica Engineering Workbench.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use crate::{ModelicaModel, ModelicaInput, ModelicaOutput, ModelicaChannels, ModelicaCommand};

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
    pub selected_model_name: String,
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
        let mut plotted = std::collections::HashSet::new();
        plotted.insert("soc_out".to_string());
        plotted.insert("voltage_out".to_string());

        let editor_buffer = std::fs::read_to_string("assets/models/Battery.mo").unwrap_or_default();

        Self {
            current_path: PathBuf::from("assets/models"),
            editor_buffer,
            selected_model_name: "Battery".to_string(),
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: plotted,
            logs: VecDeque::new(),
            max_history: 1000,
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
    q_models: Query<Entity, With<ModelicaModel>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    if state.selected_entity.is_none() {
        state.selected_entity = q_models.iter().next();
    }

    egui::Window::new("📝 Modelica Editor")
        .default_pos([270.0, 10.0])
        .default_size([600.0, 500.0])
        .min_width(400.0)
        .max_width(800.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("🚀 COMPILE & RUN").clicked() {
                    if let (Some(entity), Some(channels)) = (state.selected_entity, &channels) {
                        let _ = channels.tx.send(ModelicaCommand::Compile {
                            entity,
                            model_name: state.selected_model_name.clone(),
                            source: state.editor_buffer.clone(),
                        });
                    }
                }
                ui.add_space(20.0);

                if state.compilation_error.is_some() {
                    ui.colored_label(egui::Color32::LIGHT_RED, "⚠️ Solver Error!");
                    if ui.button("Clear").clicked() {
                        state.compilation_error = None;
                    }
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

/// Window 3: Live Telemetry
fn show_telemetry(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
    mut q_models: Query<(Entity, &mut ModelicaModel, Option<&Name>, Option<&Children>)>,
    mut q_inputs: Query<&mut ModelicaInput>,
    q_outputs: Query<&ModelicaOutput>,
    channels: Option<Res<ModelicaChannels>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📊 Live Telemetry")
        .default_pos([880.0, 10.0])
        .default_size([300.0, 400.0])
        .resizable(true)
        .show(ctx, |ui| {
            if let Some(entity) = state.selected_entity {
                if let Ok((_, mut model, name, children)) = q_models.get_mut(entity) {
                    let label = name.map(|n| n.as_str()).unwrap_or("Unnamed Model");
                    ui.heading(label);

                    ui.horizontal(|ui| {
                        if model.paused {
                            if ui.button("▶ Play").clicked() { model.paused = false; }
                        } else {
                            if ui.button("⏸ Pause").clicked() { model.paused = true; }
                        }
                        ui.label(format!("Time: {:.4} s", model.current_time));
                        
                        ui.add_space(ui.available_width() - 80.0);
                        if ui.button("🔄 Reset").on_hover_text("Hard reset worker state").clicked() {
                            if let Some(channels) = &channels {
                                let _ = channels.tx.send(ModelicaCommand::Reset { entity });
                            }
                            state.history.remove(&entity);
                            model.current_time = 0.0;
                            model.last_step_time = 0.0;
                        }
                    });

                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("Parameters (Live Tuning):");
                        ui.add_space(ui.available_width() - 180.0);
                        if ui.button("Apply Params").on_hover_text("Re-init stepper with new parameters").clicked() {
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

                    egui::ScrollArea::vertical().id_salt("params_scroll").max_height(100.0).show(ui, |ui| {
                        let mut param_keys: Vec<_> = model.parameters.keys().cloned().collect();
                        param_keys.sort();
                        for key in param_keys {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:", key));
                                let mut val = *model.parameters.get(&key).unwrap();
                                if ui.add(egui::DragValue::new(&mut val).speed(0.01)).changed() {
                                    model.parameters.insert(key, val);
                                }
                            });
                        }
                        if model.parameters.is_empty() {
                            ui.label("No parameters defined. Edit .mo to add parameters.");
                        }
                    });

                    ui.separator();
                    egui::ScrollArea::vertical().id_salt("inputs_scroll").max_height(150.0).show(ui, |ui| {
                        if let Ok(mut input) = q_inputs.get_mut(entity) {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:", input.variable_name));
                                ui.add(egui::DragValue::new(&mut input.value).speed(0.1));
                            });
                        }
                        if let Some(children) = children {
                            for child in children.iter() {
                                if let Ok(mut input) = q_inputs.get_mut(child) {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{}:", input.variable_name));
                                        ui.add(egui::DragValue::new(&mut input.value).speed(0.1));
                                    });
                                }
                            }
                        }
                    });

                    ui.separator();
                    ui.label("Variables (Toggle to Plot):");
                    
                    egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
                        // Pre-collect to avoid borrow checker conflicts
                        let var_names = if let Some(h) = state.history.get(&entity) {
                            let mut names: Vec<_> = h.keys().cloned().collect();
                            names.sort();
                            names
                        } else {
                            Vec::new()
                        };

                        for name in var_names {
                            ui.horizontal(|ui| {
                                let mut is_plotted = state.plotted_variables.contains(&name);
                                if ui.checkbox(&mut is_plotted, "").changed() {
                                    if is_plotted {
                                        state.plotted_variables.insert(name.clone());
                                    } else {
                                        state.plotted_variables.remove(&name);
                                    }
                                }
                                ui.label(format!("{}:", name));
                                
                                if let Some(entity_history) = state.history.get(&entity) {
                                    if let Some(history) = entity_history.get(&name) {
                                        if let Some([_, last_val]) = history.back() {
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.monospace(format!("{:.4}", last_val));
                                            });
                                        }
                                    }
                                }
                            });
                        }
                        if state.history.get(&entity).is_none() {
                            ui.label("Waiting for data...");
                        }
                    });
                }
            } else {
                ui.label("No model selected.");
            }
        });
}

/// Window 4: Plots
fn show_plots(
    mut contexts: EguiContexts,
    state: Res<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    if state.plotted_variables.is_empty() { return; }

    egui::Window::new("📈 Variable Plots")
        .default_pos([270.0, 520.0])
        .default_size([600.0, 300.0])
        .resizable(true)
        .show(ctx, |ui| {
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
        entries.sort_by(|a, b| {
            let a_is_dir = a.path().is_dir();
            let b_is_dir = b.path().is_dir();
            if a_is_dir != b_is_dir {
                b_is_dir.cmp(&a_is_dir)
            } else {
                a.path().cmp(&b.path())
            }
        });

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
                        state.selected_model_name = name.trim_end_matches(".mo").to_string();
                    }
                }
            }
        }
    }
}
