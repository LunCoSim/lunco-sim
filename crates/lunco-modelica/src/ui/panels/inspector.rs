use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use crate::ui::panels::diagram::DiagramState;

#[derive(Default)]
pub struct InspectorPanel;

impl WorkbenchPanel for InspectorPanel {
    fn id(&self) -> &str { "inspector" }
    fn title(&self) -> String { "🔍 Inspector".to_string() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mut updates = Vec::new();
        let mut delete_clicked = false;

        {
            let Some(ds) = world.get_resource::<DiagramState>() else {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label("No diagram state found.");
                });
                return;
            };

            let selected_id = ds.selected_node;

            if let Some(id) = selected_id {
                // Find the node in VisualDiagram
                let node_opt = ds.diagram.nodes.iter().find(|n| n.id == id);

                if let Some(node) = node_opt {
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.heading(format!("📦 {}", node.instance_name));
                        ui.label(egui::RichText::new(&node.component_def.msl_path).size(10.0).color(egui::Color32::GRAY));
                        ui.separator();

                        if let Some(desc) = &node.component_def.description {
                            ui.label(desc);
                            ui.add_space(4.0);
                        }

                        ui.collapsing("⚙ Parameters", |ui| {
                            egui::Grid::new("inspector_params").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                                for param in &node.component_def.parameters {
                                    ui.label(&param.name);
                                    
                                    let current_val = node.parameter_values.get(&param.name).cloned().unwrap_or_else(|| param.default.clone());
                                    let mut edit_val = current_val.clone();
                                    
                                    if ui.text_edit_singleline(&mut edit_val).changed() {
                                        updates.push((param.name.clone(), edit_val));
                                    }
                                    ui.end_row();
                                }
                            });
                        });

                        ui.add_space(8.0);
                        ui.collapsing("🔌 Ports", |ui| {
                            for port in &node.component_def.ports {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("•").strong());
                                    ui.label(&port.name);
                                    ui.label(egui::RichText::new(&port.connector_type).size(10.0).color(egui::Color32::GRAY));
                                });
                            }
                        });

                        ui.add_space(20.0);
                        if ui.button("🗑 Delete Component").clicked() {
                            delete_clicked = true;
                        }
                    });
                } else {
                    ui.label("Selected node not found in diagram.");
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(egui::RichText::new("No component selected").italics().color(egui::Color32::GRAY));
                    ui.label("Click a component on the canvas to inspect it.");
                });
            }
        }

        // Apply changes
        if !updates.is_empty() || delete_clicked {
            if let Some(mut ds_mut) = world.get_resource_mut::<DiagramState>() {
                if let Some(selected_id) = ds_mut.selected_node {
                    if delete_clicked {
                        ds_mut.diagram.remove_node(selected_id);
                        ds_mut.selected_node = None;
                        ds_mut.rebuild_snarl();
                    } else {
                        if let Some(node_mut) = ds_mut.diagram.get_node_mut(selected_id) {
                            for (name, val) in updates {
                                node_mut.parameter_values.insert(name, val);
                            }
                        }
                    }
                }
            }
        }
    }
}
