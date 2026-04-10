//! Graphical User Interface for the simulation client.
//!
//! This module implements the "Mission Control" center using `bevy_egui`. 
//! It provides tools for:
//! - **Time Management**: Controlling simulation epoch and time-warp speed.
//! - **Selection & Focus**: Inspecting entities and controlling the camera.
//! - **Mechanical Inspection**: Live tuning of suspension and motor parameters.
//! - **Surface Spawning**: Interactive vessel deployment on planetary surfaces.

use bevy::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_core::{RoverVessel, Vessel, Avatar, Spacecraft};
use lunco_celestial::{CelestialClock, CelestialBody, TrajectoryView, TrajectoryFrame, LocalGravityField};
use lunco_avatar::{SpringArmCamera, OrbitCamera, FreeFlightCamera, FrameBlend, CameraScroll};
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

#[derive(SystemParam)]
struct MainUiParams<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    selected: ResMut<'w, SelectedEntity>,
    pending: ResMut<'w, PendingSpawn>,
    clock: ResMut<'w, CelestialClock>,
    gravity_field: Res<'w, LocalGravityField>,
    q_rovers: Query<'w, 's, (Entity, &'static Name, &'static Vessel), With<RoverVessel>>,
    q_bodies: Query<'w, 's, (Entity, &'static Name, &'static CelestialBody)>,
    q_spacecraft: Query<'w, 's, (Entity, &'static Name, &'static mut Spacecraft)>,
    q_camera: Query<'w, 's, Entity, With<Avatar>>,
    q_camera_tf: Query<'w, 's, &'static Transform, With<Avatar>>,
    q_camera_cell: Query<'w, 's, &'static big_space::prelude::CellCoord, With<Avatar>>,
    q_camera_child_of: Query<'w, 's, &'static ChildOf, With<Avatar>>,
    q_grids: Query<'w, 's, &'static big_space::prelude::Grid>,
    q_camera_spring: Query<'w, 's, &'static SpringArmCamera, With<Avatar>>,
    q_camera_orbit: Query<'w, 's, &'static OrbitCamera, With<Avatar>>,
    q_camera_freeflight: Query<'w, 's, &'static FreeFlightCamera, With<Avatar>>,
    q_camera_blend: Query<'w, 's, &'static FrameBlend, With<Avatar>>,
    q_trajectories: Query<'w, 's, (Entity, &'static Name, &'static mut TrajectoryView)>,
    q_children: Query<'w, 's, &'static Children>,
    q_suspension: Query<'w, 's, (Entity, &'static mut Suspension)>,
    commands: Commands<'w, 's>,
    scroll_res: ResMut<'w, CameraScroll>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// The primary UI system that renders the egui windows.
fn main_ui_system(mut params: MainUiParams) {
    let Ok(ctx) = params.contexts.ctx_mut() else { return; };
    
    // Process scroll input for camera zoom when not hovering over UI panels.
    // We add to the delta instead of overwriting, and lunco-avatar systems will consume it.
    if !ctx.is_pointer_over_area() {
        params.scroll_res.delta += ctx.input(|i| i.raw_scroll_delta.y);
    }

    egui::Window::new("Mission Control").show(ctx, |ui| {
        ui.heading("Epoch & UTC Time");
        ui.label(format!("JD: {:.4}", params.clock.epoch));
        ui.label(format!("UTC: {}", lunco_celestial::jd_to_utc_string(params.clock.epoch)));
        
        ui.horizontal(|ui| {
            if ui.button(if params.clock.paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                params.clock.paused = !params.clock.paused;
            }
        });

        ui.horizontal_wrapped(|ui| { 
            let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
            for &m in multipliers.iter() {
                if ui.selectable_label(params.clock.speed_multiplier == m, format!("{}x", m)).clicked() {
                    params.clock.speed_multiplier = m;
                }
            }
        });

        let mut target_to_focus = None;

        ui.separator();
        ui.collapsing("Celestial Bodies", |ui| {
            for (entity, name, _) in params.q_bodies.iter() {
                let res = ui.selectable_label(params.selected.entity == Some(entity), format!("{}", name));
                if res.clicked() { params.selected.entity = Some(entity); }
                if res.double_clicked() { target_to_focus = Some(entity); }
            }
        });

        ui.collapsing("Spacecraft", |ui| {
            for (entity, name, mut sc) in params.q_spacecraft.iter_mut() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut sc.user_visible, "");
                    let res = ui.selectable_label(params.selected.entity == Some(entity), format!("{}", name));
                    if res.clicked() { params.selected.entity = Some(entity); }
                    if res.double_clicked() { target_to_focus = Some(entity); }
                });
            }
        });

        ui.collapsing("Local Vessels", |ui| {
            for (entity, name, _) in params.q_rovers.iter() {
                 let res = ui.selectable_label(params.selected.entity == Some(entity), format!("{}", name));
                 if res.clicked() { params.selected.entity = Some(entity); }
                 if res.double_clicked() { target_to_focus = Some(entity); }
            }
        });

        ui.collapsing("OrbitCamera Visualizations", |ui| {
            for (entity, name, mut view) in params.q_trajectories.iter_mut() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut view.user_visible, "");
                    let res = ui.selectable_label(params.selected.entity == Some(entity), format!("{}", name));
                    if res.clicked() { params.selected.entity = Some(entity); }
                    if res.double_clicked() { target_to_focus = Some(entity); }
                    
                    ui.separator();
                    if ui.selectable_label(view.frame == TrajectoryFrame::Inertial, "Inertial").on_hover_text("Fixed relative to stars").clicked() { view.frame = TrajectoryFrame::Inertial; }
                    if ui.selectable_label(view.frame == TrajectoryFrame::BodyFixed, "Body-Fixed").on_hover_text("Fixed relative to rotating body").clicked() { view.frame = TrajectoryFrame::BodyFixed; }
                });
            }
        });

        if let Some(target) = target_to_focus {
            params.commands.trigger(lunco_core::architecture::CommandMessage {
                id: 0, target, name: "FOCUS".to_string(), args: Default::default(),
                source: params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER),
            });
            params.selected.entity = Some(target);
        }

        if let Some(target) = params.selected.entity {
            ui.separator();
            ui.heading("Selection Details");
            ui.label(format!("ID: {:?}", target));
            
            ui.horizontal(|ui| {
                if ui.button("Focus Camera").clicked() {
                    params.commands.trigger(lunco_core::architecture::CommandMessage {
                        id: 0, target, name: "FOCUS".to_string(), args: Default::default(),
                        source: params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER),
                    });
                }

                if ui.button("Release (Free Fly)").clicked() {
                    params.commands.trigger(lunco_core::architecture::CommandMessage {
                        id: 0, target: params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER),
                        name: "RELEASE".to_string(), args: Default::default(), source: Entity::PLACEHOLDER,
                    });
                }
            });

            // Go to Surface — show when the camera is in OrbitCamera mode targeting a body.
            // Also show when a body is explicitly selected in the UI.
            let orbit_target_body = params.q_camera_orbit.iter().next()
                .and_then(|orbit| params.q_bodies.get(orbit.target).ok())
                .map(|(e, _, _)| e);
            let surface_target = orbit_target_body.or_else(|| {
                params.selected.entity.filter(|t| params.q_bodies.contains(*t))
            });

            if let Some(body_ent) = surface_target {
                let (_, _, body) = params.q_bodies.get(body_ent).unwrap();
                if ui.button(format!("🌕 Go to Surface ({})", body.name)).clicked() {
                    let avatar_ent = params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER);
                    let args: Vec<f64> = vec![body_ent.to_bits() as f64];
                    params.commands.trigger(lunco_core::architecture::CommandMessage {
                        id: 0, target: body_ent, name: "TELEPORT_SURFACE".to_string(),
                        args: args.into_iter().collect(),
                        source: avatar_ent,
                    });
                }
            }

            if params.q_rovers.contains(target) {
                if ui.button("Take Control (Possess)").clicked() {
                    let avatar_ent = params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER);
                    params.commands.trigger(lunco_core::architecture::CommandMessage {
                        id: 0, target: target, name: "POSSESS".to_string(), args: Default::default(), source: avatar_ent,
                    });
                }
                ui.collapsing("Mechanical Inspector", |ui| { inspect_suspension_recursive(ui, target, &params.q_children, &mut params.q_suspension); });
            }
        }

        if let Some(spawn_req) = params.pending.request {
             ui.separator();
             ui.heading("Surface Spawning");
             ui.label(format!("Surface: {:?}", spawn_req.planet));
             if ui.button("Spawn Ackermann Rover (Blue)").clicked() {
                 let _rover = lunco_robotics::rover::spawn_joint_rover(&mut params.commands, &mut params.meshes, &mut params.materials, spawn_req.planet, spawn_req.click_pos_local.as_vec3() + spawn_req.surface_normal * 1.5, "Lunar Explorer", Color::Srgba(bevy::color::palettes::basic::BLUE), lunco_robotics::rover::SteeringType::Ackermann);
                 params.pending.request = None;
             }
             if ui.button("Cancel").clicked() { params.pending.request = None; }
        }
    });

    egui::Window::new("Telemetry").anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0]).show(ctx, |ui| {
        ui.heading("Avatar Status");
        ui.separator();

        // Surface mode: show Return to Orbit button and coordinates
        if let Some(body) = params.gravity_field.body_entity {
            ui.horizontal(|ui| {
                ui.label("Surface Mode — Body:");
                ui.colored_label(egui::Color32::from_rgb(255, 180, 50), format!("{:?}", body));
            });
            ui.label(format!("Gravity: {:.3} m/s²", params.gravity_field.surface_g));

            // Compute lat/lon/height from avatar's body-local position
            let mut lat_lon_height = None;
            if let Ok(avatar_ent) = params.q_camera.single() {
                let Ok(tf) = params.q_camera_tf.get(avatar_ent) else { return };
                let Ok(cell) = params.q_camera_cell.get(avatar_ent) else { return };
                let Ok(child_of) = params.q_camera_child_of.get(avatar_ent) else { return };
                let Ok(grid) = params.q_grids.get(child_of.0) else { return };

                let body_local = grid.grid_position_double(cell, tf);
                let dist = body_local.length();
                if let Ok((_, _, body_comp)) = params.q_bodies.get(body) {
                    let height = dist - body_comp.radius_m;
                    let body_local_norm = if dist > 1e-6 { body_local / dist } else { DVec3::Y };
                    // Convert body-fixed unit vector to lat/lon (Y-up convention)
                    let lat = body_local_norm.y.asin().to_degrees();
                    let lon = body_local_norm.x.atan2(body_local_norm.z).to_degrees();
                    lat_lon_height = Some((lat, lon, height));
                }
            }

            if let Some((lat, lon, height)) = lat_lon_height {
                ui.separator();
                ui.heading("Position");
                let lat_dir = if lat >= 0.0 { "N" } else { "S" };
                let lon_dir = if lon >= 0.0 { "E" } else { "W" };
                ui.label(format!("Lat: {:.4}° {}", lat.abs(), lat_dir));
                ui.label(format!("Lon: {:.4}° {}", lon.abs(), lon_dir));
                ui.label(format!("Alt: {:.1} m", height));
            }

            ui.separator();
            if ui.button("🏠 Return to Orbit").clicked() {
                let avatar_ent = params.q_camera.iter().next().unwrap_or(Entity::PLACEHOLDER);
                params.commands.trigger(lunco_core::architecture::CommandMessage {
                    id: 0, target: body, name: "LEAVE_SURFACE".to_string(),
                    args: Default::default(),
                    source: avatar_ent,
                });
            }
            ui.separator();
        }

        // Display active camera behavior.
        if let Ok(blend) = params.q_camera_blend.single() {
            let progress = (blend.t / blend.duration * 100.0).min(100.0) as i32;
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.colored_label(egui::Color32::from_rgb(200, 200, 50), format!("TRANSITION ({}%)", progress));
            });
        } else if let Ok(arm) = params.q_camera_spring.single() {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.colored_label(egui::Color32::from_rgb(255, 100, 50), "SPRING ARM");
            });
            ui.label(format!("Distance: {:.1} m", arm.distance));
        } else if let Ok(orbit) = params.q_camera_orbit.single() {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.colored_label(egui::Color32::from_rgb(100, 150, 255), "ORBIT");
            });
            ui.label(format!("Distance: {:.1} m", orbit.distance));
        } else if let Ok(_ff) = params.q_camera_freeflight.single() {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.colored_label(egui::Color32::from_rgb(255, 200, 50), "FREE FLIGHT");
            });
        } else {
            ui.label("Mode: UNKNOWN");
        }
        ui.separator();
        ui.label("WASD: move");
        ui.label("QE: Up/Down");
        ui.label("SHIFT: Speed boost");
        ui.label("SCROLL or +/-: zoom (Spring/Orbit)");
        ui.label("Right-Click: rotate");
        ui.label("SPACE: pause/unpause");
    });
}

/// Recursively inspects mechanical parameters.
fn inspect_suspension_recursive(ui: &mut egui::Ui, entity: Entity, q_children: &Query<&Children>, q_suspension: &mut Query<(Entity, &mut Suspension)>) {
    if let Ok((_e, mut susp)) = q_suspension.get_mut(entity) {
        ui.label(format!("Hub: {:?}", entity));
        ui.add(egui::Slider::new(&mut susp.rest_length, 0.1..=2.0).text("Rest Length"));
        ui.add(egui::Slider::new(&mut susp.spring_k, 1000.0..=100000.0).text("Spring K"));
        ui.add(egui::Slider::new(&mut susp.damping_c, 100.0..=10000.0).text("Damping C"));
        ui.separator();
    }
    if let Ok(children) = q_children.get(entity) {
        for child in children.iter() { inspect_suspension_recursive(ui, child, q_children, q_suspension); }
    }
}
