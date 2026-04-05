//! Egui inspector for Modelica models.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use crate::ModelicaModel;
use crate::ModelicaOutput;
use crate::ModelicaInput;
use std::path::PathBuf;

pub struct ModelicaInspectorPlugin;

impl Plugin for ModelicaInspectorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MslBrowserState>()
           .add_systems(EguiPrimaryContextPass, inspector_ui);
    }
}

#[derive(Resource)]
struct MslBrowserState {
    pub current_path: PathBuf,
}

impl Default for MslBrowserState {
    fn default() -> Self {
        Self {
            current_path: PathBuf::from(".cache/msl/ModelicaStandardLibrary-4.0.0"),
        }
    }
}

fn inspector_ui(
    mut contexts: EguiContexts,
    q_models: Query<(Entity, &ModelicaModel, Option<&Children>)>,
    q_inputs: Query<&ModelicaInput>,
    q_outputs: Query<&ModelicaOutput>,
    mut browser_state: ResMut<MslBrowserState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::Window::new("Modelica Inspector").show(ctx, |ui| {
        ui.set_min_width(300.0);

        ui.collapsing("MSL Browser", |ui| {
            ui.horizontal(|ui| {
                if ui.button("UP").clicked() {
                    if let Some(parent) = browser_state.current_path.parent() {
                        if parent.starts_with(".cache/msl") {
                            browser_state.current_path = parent.to_path_buf();
                        }
                    }
                }
                ui.label(format!("{:?}", browser_state.current_path));
            });

            ui.separator();

            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                if let Ok(entries) = std::fs::read_dir(&browser_state.current_path) {
                    for mut entry in entries.flatten().collect::<Vec<_>>() {
                        let path = entry.path();
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        
                        if path.is_dir() {
                            if ui.link(format!("📁 {}", name)).clicked() {
                                browser_state.current_path = path;
                            }
                        } else if name.ends_with(".mo") {
                            ui.label(format!("📄 {}", name));
                        }
                    }
                } else {
                    ui.label("Failed to read directory. Is MSL downloaded?");
                }
            });
        });

        ui.separator();

        if q_models.is_empty() {
            ui.label("No active Modelica models in ECS.");
            return;
        }

        for (entity, model, children) in q_models.iter() {
            ui.collapsing(format!("Model: {} ({:?})", model.model_name, entity), |ui| {
                ui.label(format!("Path: {}", model.model_path));
                ui.label(format!("Time: {:.3}s", model.current_time));
                
                ui.separator();
                ui.label("Inputs:");
                if let Some(children) = children {
                    for child in children.iter() {
                        if let Ok(input) = q_inputs.get(child) {
                            ui.label(format!("  {}: {:.4}", input.variable_name, input.value));
                        }
                    }
                }
                if let Ok(input) = q_inputs.get(entity) {
                    ui.label(format!("  {}: {:.4}", input.variable_name, input.value));
                }

                ui.separator();
                ui.label("Outputs:");
                if let Some(children) = children {
                    for child in children.iter() {
                        if let Ok(output) = q_outputs.get(child) {
                            ui.label(format!("  {}: {:.4}", output.variable_name, output.value));
                        }
                    }
                }
                if let Ok(output) = q_outputs.get(entity) {
                    ui.label(format!("  {}: {:.4}", output.variable_name, output.value));
                }
            });
        }
    });
}
