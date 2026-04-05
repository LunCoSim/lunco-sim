//! Professional Modelica Engineering Workbench.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use crate::{ModelicaModel, ModelicaOutput, ModelicaChannels, ModelicaCommand};

pub struct ModelicaInspectorPlugin;

impl Plugin for ModelicaInspectorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
           .add_systems(EguiPrimaryContextPass, workbench_ui);
    }
}

#[derive(Resource)]
pub struct WorkbenchState {
    pub current_path: PathBuf,
    pub editor_buffer: String,
    pub selected_entity: Option<Entity>,
    pub compilation_error: Option<String>,
    pub history: HashMap<String, VecDeque<[f64; 2]>>,
    pub plotted_variables: std::collections::HashSet<String>,
    pub max_history: usize,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        let mut plotted = std::collections::HashSet::new();
        plotted.insert("soc_out".to_string());
        plotted.insert("voltage_out".to_string());

        Self {
            current_path: PathBuf::from("assets/models"),
            editor_buffer: String::new(),
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: plotted,
            max_history: 1000,
        }
    }
}

fn workbench_ui(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
    q_models: Query<(Entity, &ModelicaModel, Option<&Name>)>,
    q_outputs: Query<&ModelicaOutput>,
    channels: Option<Res<ModelicaChannels>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("📐 Modelica Engineering Workbench")
        .default_size([1000.0, 600.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // --- LEFT PANE: Browser ---
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    ui.heading("Library Browser");
                    
                    if ui.button("📁 MSL 4.0").clicked() {
                        state.current_path = PathBuf::from(".cache/msl/ModelicaStandardLibrary-4.0.0");
                    }
                    if ui.button("📁 Projects").clicked() {
                        state.current_path = PathBuf::from("assets/models");
                    }

                    ui.separator();

                    egui::ScrollArea::vertical().id_salt("browser").show(ui, |ui| {
                        render_browser(ui, &mut state);
                    });
                });

                ui.separator();

                // --- CENTER PANE: Editor ---
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() * 0.6);
                    ui.horizontal(|ui| {
                        ui.heading("Model Editor");
                        ui.add_space(ui.available_width() - 120.0);
                        if ui.button("🚀 COMPILE & RUN").clicked() {
                            if let (Some(entity), Some(channels)) = (state.selected_entity, &channels) {
                                let _ = channels.tx.send(ModelicaCommand::Compile {
                                    entity,
                                    model_name: "Battery".to_string(), 
                                    source: state.editor_buffer.clone(),
                                });
                            }
                        }
                    });

                    // Manual code editor implementation using standard TextEdit to avoid version conflicts
                    egui::ScrollArea::both()
                        .id_salt("editor_scroll")
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut state.editor_buffer)
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .lock_focus(true)
                                    .desired_rows(30)
                            );
                        });

                    if let Some(err) = &state.compilation_error {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("❌ {}", err));
                    }
                });

                ui.separator();

                // --- RIGHT PANE: Live Data ---
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.heading("Live Telemetry");
                    
                    if state.selected_entity.is_none() {
                        state.selected_entity = q_models.iter().next().map(|(e, _, _)| e);
                    }

                    if let Some(entity) = state.selected_entity {
                        if let Ok((_, model, name)) = q_models.get(entity) {
                            let label = name.map(|n| n.as_str()).unwrap_or("Unnamed Model");
                            ui.label(format!("Active: {} (t={:.2}s)", label, model.current_time));
                            
                            ui.separator();
                            ui.label("Plots");
                            
                            Plot::new("live_plot")
                                .view_aspect(2.0)
                                .legend(egui_plot::Legend::default())
                                .show(ui, |plot_ui| {
                                    for var_name in &state.plotted_variables {
                                        if let Some(data) = state.history.get(var_name) {
                                            let points: Vec<[f64; 2]> = data.iter().cloned().collect();
                                            plot_ui.line(Line::new(var_name.clone(), PlotPoints::from(points)));
                                        }
                                    }
                                });

                            ui.separator();
                            ui.label("Variables:");
                            egui::ScrollArea::vertical().id_salt("vars").show(ui, |ui| {
                                for output in q_outputs.iter() {
                                    ui.horizontal(|ui| {
                                        let mut is_plotted = state.plotted_variables.contains(&output.variable_name);
                                        if ui.checkbox(&mut is_plotted, "").changed() {
                                            if is_plotted {
                                                state.plotted_variables.insert(output.variable_name.clone());
                                            } else {
                                                state.plotted_variables.remove(&output.variable_name);
                                            }
                                        }
                                        ui.label(format!("{}: {:.4}", output.variable_name, output.value));
                                    });
                                }
                            });
                        }
                    }
                });
            });
        });
}

fn render_browser(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    ui.horizontal(|ui| {
        if ui.button("UP").clicked() {
            if let Some(parent) = state.current_path.parent() {
                state.current_path = parent.to_path_buf();
            }
        }
        ui.label(format!("{:?}", state.current_path));
    });

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
