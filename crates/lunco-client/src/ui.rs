//! Graphical User Interface for the simulation client.
//!
//! This module implements the "Mission Control" center using `bevy_egui`. 
//! It provides tools for:
//! - **Time Management**: Controlling simulation epoch and time-warp speed.
//! - **Selection & Focus**: Inspecting entities and controlling the camera.
//! - **Mechanical Inspection**: Live tuning of suspension and motor parameters.
//! - **Surface Spawning**: Interactive vessel deployment on planetary surfaces.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_core::{RoverVessel, Vessel, Avatar, Spacecraft};
use lunco_celestial::{CelestialClock, CelestialBody, TrajectoryView, TrajectoryFrame};
use lunco_avatar::{OrbitalBehavior, FlybyBehavior, SurfaceBehavior, CameraScroll};
use lunco_controller::{ControllerLink, VesselIntent, get_default_input_map};
use lunco_mobility::Suspension;

/// Plugin for managing the simulation's graphical user interface.
pub struct LunCoUiPlugin;

impl Plugin for LunCoUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<SelectedEntity>()
           .init_resource::<PendingSpawn>()
           .add_observer(on_surface_click)
           .add_observer(on_rover_click)
           .add_systems(EguiPrimaryContextPass, main_ui_system);
    }
}

/// Resource tracking the currently selected entity in the UI.
#[derive(Resource, Default)]
struct SelectedEntity {
    entity: Option<Entity>,
}

/// Resource tracking a pending request to spawn a vessel on a surface.
#[derive(Resource, Default)]
struct PendingSpawn {
    request: Option<lunco_celestial::SurfaceClickEvent>,
}

/// Observer that captures surface clicks to initiate the spawning workflow.
fn on_surface_click(
    trigger: On<lunco_celestial::SurfaceClickEvent>,
    mut pending: ResMut<PendingSpawn>,
) {
    let ev = trigger.event();
    pending.request = Some(lunco_celestial::SurfaceClickEvent {
        planet: ev.planet,
        click_pos_local: ev.click_pos_local,
        surface_normal: ev.surface_normal,
    });
}

/// Observer that captures clicks on rovers to update the UI selection.
fn on_rover_click(
    trigger: On<lunco_celestial::RoverClickEvent>,
    mut selected: ResMut<SelectedEntity>,
) {
    selected.entity = Some(trigger.event().rover);
}

/// The primary UI system that renders the egui windows.
fn main_ui_system(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedEntity>,
    mut pending: ResMut<PendingSpawn>,
    mut clock: ResMut<CelestialClock>,
    q_rovers: Query<(Entity, &Name, &Vessel), With<RoverVessel>>,
    q_bodies: Query<(Entity, &Name, &CelestialBody)>,
    mut q_spacecraft: Query<(Entity, &Name, &mut Spacecraft)>,
    mut q_camera: Query<(Entity, &mut OrbitalBehavior), With<Avatar>>,
    mut q_flyby: Query<&mut FlybyBehavior, With<Avatar>>,
    mut q_surface: Query<&mut SurfaceBehavior, With<Avatar>>,
    mut q_trajectories: Query<(Entity, &Name, &mut TrajectoryView)>,
    q_children: Query<&Children>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut commands: Commands,
    mut scroll_res: ResMut<CameraScroll>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // Process scroll input for camera zoom when not hovering over UI panels.
    if !ctx.is_pointer_over_area() {
        scroll_res.delta = ctx.input(|i| i.raw_scroll_delta.y);
    }

    egui::Window::new("Mission Control").show(ctx, |ui| {
        // ... (egui window content)
        ui.heading("Epoch & UTC Time");
        ui.label(format!("JD: {:.4}", clock.epoch));
        ui.label(format!("UTC: {}", lunco_celestial::jd_to_utc_string(clock.epoch)));
        
        ui.horizontal(|ui| {
            if ui.button(if clock.paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                clock.paused = !clock.paused;
            }
        });

        ui.horizontal_wrapped(|ui| { 
            let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
            for &m in multipliers.iter() {
                if ui.selectable_label(clock.speed_multiplier == m, format!("{}x", m)).clicked() {
                    clock.speed_multiplier = m;
                }
            }
        });

        let mut target_to_focus = None;

        ui.separator();
        ui.collapsing("Celestial Bodies", |ui| {
            for (entity, name, _) in q_bodies.iter() {
                let res = ui.selectable_label(selected.entity == Some(entity), format!("{}", name));
                if res.clicked() {
                    selected.entity = Some(entity);
                }
                if res.double_clicked() {
                    target_to_focus = Some(entity);
                }
            }
        });

        ui.collapsing("Spacecraft", |ui| {
            for (entity, name, mut sc) in q_spacecraft.iter_mut() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut sc.user_visible, "");

                    let res = ui.selectable_label(selected.entity == Some(entity), format!("{}", name));
                    if res.clicked() {
                        selected.entity = Some(entity);
                    }
                    if res.double_clicked() {
                        target_to_focus = Some(entity);
                    }
                });
            }
        });

        ui.collapsing("Local Vessels", |ui| {
            for (entity, name, _) in q_rovers.iter() {
                 let res = ui.selectable_label(selected.entity == Some(entity), format!("{}", name));
                 if res.clicked() {
                     selected.entity = Some(entity);
                 }
                 if res.double_clicked() {
                     target_to_focus = Some(entity);
                 }
            }
        });

        ui.collapsing("Orbit Visualizations", |ui| {
            for (entity, name, mut view) in q_trajectories.iter_mut() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut view.user_visible, "");
                    let res = ui.selectable_label(selected.entity == Some(entity), format!("{}", name));
                    if res.clicked() {
                        selected.entity = Some(entity);
                    }
                    if res.double_clicked() {
                        target_to_focus = Some(entity);
                    }
                    
                    ui.separator();
                    if ui.selectable_label(view.frame == TrajectoryFrame::Inertial, "Inertial").on_hover_text("Fixed relative to stars").clicked() {
                        view.frame = TrajectoryFrame::Inertial;
                    }
                    if ui.selectable_label(view.frame == TrajectoryFrame::BodyFixed, "Body-Fixed").on_hover_text("Fixed relative to rotating body").clicked() {
                        view.frame = TrajectoryFrame::BodyFixed;
                    }
                });
            }
        });

        if let Some(target) = target_to_focus {
            for (_, mut obs) in q_camera.iter_mut() {
                obs.target = Some(target);
            }
            selected.entity = Some(target);
        }

        if let Some(target) = selected.entity {
            ui.separator();
            ui.heading("Selection Details");
            ui.label(format!("ID: {:?}", target));
            
            if ui.button("Focus Camera").clicked() {
                for (_, mut obs) in q_camera.iter_mut() {
                    obs.target = Some(target);
                }
            }

            if q_rovers.contains(target) {
                if ui.button("Take Control (Possess)").clicked() {
                    let avatar_ent = q_camera.iter().next().map(|(e, _)| e).unwrap_or(Entity::PLACEHOLDER);
                    commands.trigger(lunco_core::architecture::CommandMessage {
                        id: 0,
                        target: target,
                        name: "POSSESS".to_string(),
                        args: Default::default(),
                        source: avatar_ent,
                    });
                    info!("Possessing rover and focusing at 10m.");
                }
                
                ui.collapsing("Mechanical Inspector", |ui| {
                    inspect_suspension_recursive(ui, target, &q_children, &mut q_suspension);
                });
            }
        }

        if let Some(spawn_req) = pending.request {
             ui.separator();
             ui.heading("Surface Spawning");
             ui.label(format!("Surface: {:?}", spawn_req.planet));
             
             if ui.button("Spawn Ackermann Rover (Blue)").clicked() {
                 let _rover = lunco_robotics::rover::spawn_joint_rover(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    spawn_req.planet,
                    spawn_req.click_pos_local.as_vec3() + spawn_req.surface_normal * 1.5,
                    "Lunar Explorer",
                    Color::Srgba(bevy::color::palettes::basic::BLUE),
                    lunco_robotics::rover::SteeringType::Ackermann,
                 );
                 pending.request = None;
                 info!("Spawned rover at surface interaction point.");
             }
             if ui.button("Cancel").clicked() {
                 pending.request = None;
             }
        }
    });

    egui::Window::new("Telemetry").anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0]).show(ctx, |ui| {
        ui.heading("Avatar Status");
        ui.label(format!("Epoch: {:.4}", clock.epoch));
        ui.label(lunco_celestial::jd_to_utc_string(clock.epoch));
        ui.separator();

        for (ent, orbital) in q_camera.iter() {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                if let Ok(flyby) = q_flyby.get(ent) {
                     ui.colored_label(egui::Color32::from_rgb(255, 200, 50), "FLYBY");
                     ui.label(format!("Dist to Target: {:.1} m", flyby.offset.length()));
                } else if let Ok(surface) = q_surface.get(ent) {
                     ui.colored_label(egui::Color32::from_rgb(50, 255, 100), "SURFACE");
                     ui.label(format!("Height: {:.1} m", surface.height));
                } else {
                     ui.colored_label(egui::Color32::from_rgb(100, 150, 255), "ORBITAL");
                     ui.label(format!("Orbital Dist: {:.0} km", orbital.distance / 1000.0));
                }
            });
        }
        ui.separator();
        ui.label("WASD: move");
        ui.label("QE: Up/Down");
        ui.label("SHIFT: Speed boost");
        ui.label("SCROLL or +/-: zoom (Orbital)");
        ui.label("Right-Click: rotate");
        ui.label("SPACE: pause/unpause (Orbital/Flyby)");
    });
}

/// Recursively inspects the mechanical parameters (like suspension) of a vessel 
/// and its children, rendering egui controls for real-time tuning.
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

