use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_sim_physics::Suspension;
use lunco_sim_rover_raycast::WheelRaycast;
use lunco_sim_core::RoverVessel;

pub struct LunCoSimUiPlugin;

impl Plugin for LunCoSimUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<SelectedRover>();
        app.add_systems(EguiPrimaryContextPass, rover_control_ui);
    }
}

#[derive(Resource, Default)]
struct SelectedRover {
    entity: Option<Entity>,
}

fn rover_control_ui(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedRover>,
    q_rovers: Query<(Entity, &Name), With<RoverVessel>>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut q_raycast_wheels: Query<(Entity, &mut WheelRaycast)>,
    q_children: Query<&Children>,
) {
    egui::Window::new("Rover Parameters")
        .default_width(300.0)
        .show(contexts.ctx_mut().expect("No Egui context found"), |ui| {
            ui.heading("Settings");
            ui.separator();

            ui.label("Select Rover:");
            egui::ComboBox::from_id_salt("rover_select")
                .selected_text(
                    selected.entity
                        .and_then(|e| q_rovers.get(e).ok())
                        .map(|(_, name)| name.as_str())
                        .unwrap_or("None")
                )
                .show_ui(ui, |ui| {
                    for (entity, name) in q_rovers.iter() {
                        ui.selectable_value(&mut selected.entity, Some(entity), name.as_str());
                    }
                });

            if let Some(rover_entity) = selected.entity {
                if let Ok((_ent, name)) = q_rovers.get(rover_entity) {
                    ui.group(|ui| {
                        ui.label(format!("Editing: {}", name));
                        
                        // Find all suspension/wheel components in children
                        if let Ok(children) = q_children.get(rover_entity) {
                             ui.collapsing("Suspension Parameters (Joint-based)", |ui| {
                                for child in children.iter() {
                                    inspect_suspension_recursive(ui, child, &q_children, &mut q_suspension);
                                }
                             });

                             ui.collapsing("Raycast Wheel Parameters", |ui| {
                                for child in children.iter() {
                                    if let Ok((_e, mut wheel)) = q_raycast_wheels.get_mut(child) {
                                        ui.label(format!("Wheel: {:?}", child));
                                        ui.add(egui::Slider::new(&mut wheel.rest_length, 0.1..=2.0).text("Rest Length"));
                                        ui.add(egui::Slider::new(&mut wheel.spring_k, 1000.0..=50000.0).text("Spring K"));
                                        ui.add(egui::Slider::new(&mut wheel.damping_c, 100.0..=10000.0).text("Damping C"));
                                        ui.add(egui::Slider::new(&mut wheel.wheel_radius, 0.1..=1.0).text("Radius"));
                                        ui.separator();
                                    }
                                }
                             });
                        }
                    });
                } else {
                    selected.entity = None;
                }
            }
        });
}

fn inspect_suspension_recursive(
    ui: &mut egui::Ui,
    entity: Entity,
    q_children: &Query<&Children>,
    q_suspension: &mut Query<(Entity, &mut Suspension)>,
) {
    if let Ok((_e, mut susp)) = q_suspension.get_mut(entity) {
        ui.label(format!("Hub: {:?}", entity));
        ui.add(egui::Slider::new(&mut susp.rest_length, 0.1..=2.0).text("Rest Length"));
        ui.add(egui::Slider::new(&mut susp.spring_k, 1000.0..=100000.0).text("Spring K"));
        ui.add(egui::Slider::new(&mut susp.damping_c, 100.0..=10000.0).text("Damping C"));
        ui.separator();
    }
    
    if let Ok(children) = q_children.get(entity) {
        for child in children.iter() {
            inspect_suspension_recursive(ui, child, q_children, q_suspension);
        }
    }
}
