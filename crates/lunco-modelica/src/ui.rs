//! Professional Modelica Engineering Workbench.
//!
//! Provides a 5-window egui interface for Modelica model browsing, editing,
//! simulation control, telemetry visualization, and logging.
//!
//! ## Windows
//! - **Library Browser**: Navigate local models and MSL
//! - **Model Editor**: Edit Modelica code with syntax highlighting
//! - **Live Telemetry**: Real-time parameter tuning and variable monitoring
//! - **Real-time Graphs**: Plot simulation history
//! - **System Logs**: View compilation and runtime messages

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand, extract_model_name, extract_parameters, extract_input_names, extract_inputs_with_defaults};

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
    /// When true, the next plot render will call `.reset()` to auto-fit the view.
    pub plot_auto_fit: bool,
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
            plot_auto_fit: false,
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
            ui.horizontal(|ui| {
                if ui.selectable_label(state.current_path.starts_with("assets/models"), "📦 Models").clicked() {
                    state.current_path = PathBuf::from("assets/models");
                }
                if ui.selectable_label(state.current_path.starts_with(".cache/msl"), "📚 MSL").clicked() {
                    state.current_path = PathBuf::from(".cache/msl");
                }
            });
            ui.separator();
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
                            let params = extract_parameters(&state.editor_buffer);
                            let inputs_with_defaults = extract_inputs_with_defaults(&state.editor_buffer);
                            let runtime_inputs = extract_input_names(&state.editor_buffer);

                            if let Ok((_, mut model)) = q_models.get_mut(entity) {
                                // Preserve old input values where user may have adjusted them
                                let old_inputs: std::collections::HashMap<String, f64> = std::mem::take(&mut model.inputs);

                                model.session_id += 1;
                                model.is_stepping = true;
                                model.model_name = model_name.clone();
                                model.parameters = params;

                                // Merge all inputs (with defaults + without defaults), preserving user values
                                model.inputs.clear();
                                for (name, val) in &inputs_with_defaults {
                                    let existing = old_inputs.get(name).copied();
                                    model.inputs.entry(name.clone())
                                        .or_insert_with(|| existing.unwrap_or(*val));
                                }
                                for name in &runtime_inputs {
                                    let existing = old_inputs.get(name).copied();
                                    model.inputs.entry(name.clone())
                                        .or_insert_with(|| existing.unwrap_or(0.0));
                                }

                                model.variables.clear();
                                model.paused = false;
                                model.current_time = 0.0;
                                model.last_step_time = 0.0;

                                let _ = channels.tx.send(ModelicaCommand::Compile {
                                    entity,
                                    session_id: model.session_id,
                                    model_name,
                                    source: state.editor_buffer.clone(),
                                });
                            }
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
        .min_width(250.0)
        .max_width(400.0)
        .resizable([false, true])
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
                                model.session_id += 1;
                                model.is_stepping = true;
                                let _ = channels.tx.send(ModelicaCommand::Reset { entity, session_id: model.session_id });
                            }
                            state.history.remove(&entity);
                            model.current_time = 0.0;
                            model.last_step_time = 0.0;
                        }
                    });

                    ui.separator();

                    // Parameters section (parameter Real - require recompilation)
                    if !model.parameters.is_empty() {
                        ui.label("Parameters (Dynamic Tuning — requires recompile):");
                        egui::ScrollArea::vertical().id_salt("params_scroll").max_height(150.0).show(ui, |ui| {
                            let mut param_keys: Vec<_> = model.parameters.keys().cloned().collect();
                            param_keys.sort();
                            let mut changed = false;
                            for key in &param_keys {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(200.0);
                                    ui.set_max_width(f32::INFINITY);
                                    ui.label(format!("{:16}:", key));
                                    let val = model.parameters.get_mut(key).unwrap();
                                    if ui.add(egui::DragValue::new(val).speed(0.01).fixed_decimals(2)).changed() {
                                        changed = true;
                                    }
                                });
                            }

                            if changed {
                                let modified_source = substitute_params_in_source(
                                    &state.editor_buffer,
                                    &model.parameters
                                );

                                if let Some(channels) = &channels {
                                    model.session_id += 1;
                                    model.is_stepping = true;
                                    state.editor_buffer = modified_source.clone();
                                    let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                                        entity,
                                        session_id: model.session_id,
                                        model_name: model.model_name.clone(),
                                        source: modified_source,
                                    });
                                }
                            }
                        });
                        ui.separator();
                    }

                    // Inputs section (input Real — applied on every step, no recompile)
                    if !model.inputs.is_empty() {
                        ui.label("Inputs (Real-time — no recompile needed):");
                        egui::ScrollArea::vertical().id_salt("inputs_scroll").max_height(120.0).show(ui, |ui| {
                            let mut input_keys: Vec<_> = model.inputs.keys().cloned().collect();
                            input_keys.sort();
                            for key in input_keys {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(200.0);
                                    ui.set_max_width(f32::INFINITY);
                                    ui.label(format!("{:16}:", key));
                                    let val = model.inputs.get_mut(&key).unwrap();
                                    ui.add(egui::DragValue::new(val).speed(0.1).fixed_decimals(2));
                                });
                            }
                        });
                        ui.separator();
                    }

                    ui.label("Variables (Toggle to Plot):");
                    egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
                        // Only variables and inputs — parameters are constants, no need to plot
                        let mut all_names: Vec<_> = model.variables.keys().cloned().collect();
                        all_names.extend(model.inputs.keys().cloned());
                        all_names.sort();
                        all_names.dedup();

                        for name in all_names {
                            ui.horizontal(|ui| {
                                let mut is_plotted = state.plotted_variables.contains(&name);
                                if ui.checkbox(&mut is_plotted, "").changed() {
                                    if is_plotted { state.plotted_variables.insert(name.clone()); }
                                    else { state.plotted_variables.remove(&name); }
                                }
                                ui.label(format!("{}:", name));
                                ui.add_space(ui.available_width() - 60.0);
                                if let Some(&val) = model.variables.get(&name) {
                                    ui.label(format!("{:.4}", val));
                                } else if let Some(&val) = model.inputs.get(&name) {
                                    ui.label(format!("{:.4}", val));
                                }
                            });
                        }
                    });
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.label("No model selected.");
                    ui.label("Use the Library Browser to load a .mo file.");
                });
            }
        });
}

/// Window 4: Real-time Plots
fn show_plots(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📈 Real-time Graphs")
        .default_pos([270.0, 520.0])
        .default_size([910.0, 350.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("📈 Graphs");
                ui.add_space(ui.available_width() - 100.0);
                if ui.button("🎯 Auto-Fit").clicked() {
                    state.plot_auto_fit = true;
                }
            });
            ui.separator();

            if let Some(entity) = state.selected_entity {
                let do_reset = state.plot_auto_fit;
                state.plot_auto_fit = false;

                // Clone history to avoid borrow conflict with plot_auto_fit
                let entity_history = state.history.get(&entity).cloned();
                let plotted = state.plotted_variables.clone();

                if let Some(entity_history) = entity_history {
                    let plot = Plot::new("modelica_plot")
                        .view_aspect(2.5)
                        .legend(egui_plot::Legend::default())
                        .allow_drag(true)
                        .allow_zoom(true)
                        .allow_scroll(true)
                        .allow_double_click_reset(true);

                    let plot = if do_reset { plot.reset() } else { plot };

                    plot.show(ui, |plot_ui| {
                        for name in &plotted {
                            if let Some(points) = entity_history.get(name) {
                                let plot_points: PlotPoints = points.iter().cloned().collect();
                                plot_ui.line(Line::new(name, plot_points));
                            }
                        }
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Wait for simulation data...");
                    });
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a model to see plots.");
                });
            }
        });
}

/// Window 5: System Logs
fn show_logs(
    mut contexts: EguiContexts,
    state: Res<WorkbenchState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📋 System Logs")
        .default_pos([10.0, 420.0])
        .default_size([250.0, 450.0])
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for log in &state.logs {
                    ui.label(log);
                }
            });
        });
}

fn render_browser(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    ui.horizontal(|ui| {
        ui.label("Path:");
        ui.label(state.current_path.to_string_lossy());
    });
    ui.separator();

    if let Ok(entries) = std::fs::read_dir(&state.current_path) {
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if ui.button(format!("📁 {}", path.file_name().unwrap().to_string_lossy())).clicked() {
                    state.current_path = path;
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
                if ui.button(format!("📄 {}", path.file_name().unwrap().to_string_lossy())).clicked() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        state.editor_buffer = content;
                    }
                }
            }
        }
    }

    if state.current_path != PathBuf::from("assets/models") && state.current_path != PathBuf::from(".cache/msl") {
        if ui.button("⬅ Back").clicked() {
            state.current_path.pop();
        }
    }
}

/// Substitute parameter values into Modelica source code.
///
/// This replaces `parameter Real <name> = <value>` lines with the given values,
/// allowing recompilation with different parameter values.
fn substitute_params_in_source(source: &str, parameters: &HashMap<String, f64>) -> String {
    let mut modified = source.to_string();
    for (name, value) in parameters {
        let pattern = format!(
            r"(?m)(^\s*parameter\s+Real\s+{}\s*=\s*)[-+]?[0-9]*\.?[0-9]+([eE][-+]?[0-9]+)?",
            regex::escape(name)
        );
        if let Ok(re) = regex::Regex::new(&pattern) {
            modified = re.replace_all(&modified, format!("${{1}}{}", value)).to_string();
        }
    }
    modified
}
