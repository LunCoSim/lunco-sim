//! Egui inspector for Modelica models.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::ModelicaModel;
use crate::ModelicaOutput;
use crate::ModelicaInput;

pub struct ModelicaInspectorPlugin;

impl Plugin for ModelicaInspectorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, inspector_ui);
    }
}

fn inspector_ui(
    mut contexts: EguiContexts,
    q_models: Query<(Entity, &ModelicaModel, Option<&Children>)>,
    q_inputs: Query<&ModelicaInput>,
    q_outputs: Query<&ModelicaOutput>,
) {
    egui::Window::new("Modelica Inspector").show(contexts.ctx_mut(), |ui| {
        if q_models.is_empty() {
            ui.label("No Modelica models found in world.");
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
