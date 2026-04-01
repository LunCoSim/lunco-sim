use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_sim_physics::Suspension;
use lunco_sim_rover_raycast::WheelRaycast;
use lunco_sim_core::RoverVessel;
use lunco_sim_celestial::CelestialClock;

pub struct LunCoSimUiPlugin;

impl Plugin for LunCoSimUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<SelectedRover>();
        app.add_systems(EguiPrimaryContextPass, main_ui_system);
    }
}

#[derive(Resource, Default)]
struct SelectedRover {
    entity: Option<Entity>,
}

fn main_ui_system(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedRover>,
    q_rovers: Query<(Entity, &Name), With<RoverVessel>>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut q_raycast_wheels: Query<(Entity, &mut WheelRaycast)>,
    q_children: Query<&Children>,
    mut world_clock: ResMut<CelestialClock>,
    q_sun: Query<&GlobalTransform, (With<lunco_sim_celestial::CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>,
    q_camera: Query<(&Camera, &GlobalTransform)>,
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
    
    egui::Window::new("Celestial Control")
        .default_width(300.0)
        .show(contexts.ctx_mut().expect("No Egui context found"), |ui| {
            ui.heading("Time Scrubber");
            ui.separator();
            
            ui.label(format!("Julian Date: {:.4}", world_clock.epoch));
            
            ui.horizontal(|ui| {
                if ui.button(if world_clock.paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                    world_clock.paused = !world_clock.paused;
                }
            });

            ui.label(format!("Speed: {:.0}x", world_clock.speed_multiplier));
            
            let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
            ui.horizontal_wrapped(|ui| {
                for &m in &multipliers {
                    if ui.selectable_label(world_clock.speed_multiplier == m, format!("{}x", m)).clicked() {
                        world_clock.speed_multiplier = m;
                    }
                }
            });
            
            if ui.button("J2000").clicked() {
                world_clock.epoch = 2_451_545.0;
            }
        });

    // Sun Marker (FR-022)
    draw_sun_marker(&mut contexts, &q_sun, &q_camera);
}

fn draw_sun_marker(
    contexts: &mut EguiContexts,
    q_sun: &Query<&GlobalTransform, (With<lunco_sim_celestial::CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>,
    q_camera: &Query<(&Camera, &GlobalTransform)>,
) {
    let Some(sun_gtf) = q_sun.iter().next() else { return; };
    let Some((camera, cam_gtf)) = q_camera.iter().next() else { return; };
    
    let sun_pos_abs = sun_gtf.translation();
    let cam_pos_abs = cam_gtf.translation();
    let dir_to_sun = (sun_pos_abs - cam_pos_abs).normalize_or_zero();
    
    // Project sun direction to screen
    // We can't just project sun_pos_abs because it might be too far.
    // Instead, project a point in the sun direction far away.
    if let Ok(screen_pos) = camera.world_to_viewport(cam_gtf, cam_pos_abs + dir_to_sun * 1000.0) {
        let ctx = contexts.ctx_mut().expect("No Egui context");
        let painter = ctx.debug_painter();
        
        painter.circle_filled(
            egui::pos2(screen_pos.x, screen_pos.y),
            10.0,
            egui::Color32::from_rgb(255, 255, 0),
        );
        painter.text(
            egui::pos2(screen_pos.x, screen_pos.y + 15.0),
            egui::Align2::CENTER_TOP,
            "SUN",
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
    }
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
