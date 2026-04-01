use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_sim_physics::Suspension;
use lunco_sim_core::RoverVessel;
use lunco_sim_celestial::{CelestialClock, ObserverCamera, CelestialBody, ActiveTerrainTile};

pub struct LunCoSimUiPlugin;

impl Plugin for LunCoSimUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<SelectedRover>()
           .add_systems(PreUpdate, manual_egui_input_bridge) 
           .add_systems(EguiPrimaryContextPass, main_ui_system);
    }
}

#[derive(Resource, Default)]
struct SelectedRover {
    entity: Option<Entity>,
}

fn manual_egui_input_bridge(
    mut contexts: EguiContexts,
    mut click_events: MessageReader<bevy::input::mouse::MouseButtonInput>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    for event in click_events.read() {
        let pressed = event.state == bevy::input::ButtonState::Pressed;
        let button = match event.button {
            MouseButton::Left => egui::PointerButton::Primary,
            MouseButton::Right => egui::PointerButton::Secondary,
            _ => egui::PointerButton::Middle,
        };

        ctx.input_mut(|i| {
            i.events.push(egui::Event::PointerButton {
                pos: i.pointer.hover_pos().unwrap_or(egui::Pos2::ZERO),
                button,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        });
    }
}

fn main_ui_system(
    mut commands: Commands,
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedRover>,
    q_rovers: Query<(Entity, &Name), With<RoverVessel>>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    q_children: Query<&Children>,
    world_clock: Option<ResMut<CelestialClock>>,
    q_sun: Query<&GlobalTransform, (With<CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>,
    mut q_camera: Query<(Entity, &Camera, &GlobalTransform, &mut ObserverCamera)>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody, &Name)>,
    q_tiles: Query<Entity, With<ActiveTerrainTile>>,
    mut frame_timer: Local<u32>,
) {
    *frame_timer += 1;
    if *frame_timer < 5 { return; }

    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let Some(mut world_clock) = world_clock else { return; };

    let mut focus_rover_ui = false;
    let mut current_focus = None;
    if let Some((_, _, _, obs)) = q_camera.iter().next() {
        if let Some(target) = obs.focus_target {
            current_focus = Some(target);
            if q_rovers.contains(target) {
                focus_rover_ui = true;
            }
        }
    }

    if focus_rover_ui {
        egui::Window::new("Rover Parameters").default_width(300.0).show(ctx, |ui| {
            ui.heading("Settings");
            ui.separator();
            ui.label("Select Rover:");
            egui::ComboBox::from_id_salt("rover_select").selected_text(selected.entity.and_then(|e| q_rovers.get(e).ok()).map(|(_, name)| name.as_str()).unwrap_or("None")).show_ui(ui, |ui| {
                for (entity, name) in q_rovers.iter() {
                    ui.selectable_value(&mut selected.entity, Some(entity), name.as_str());
                }
            });
            if let Some(rover_entity) = selected.entity {
                if let Ok((_ent, name)) = q_rovers.get(rover_entity) {
                    ui.group(|ui| {
                        ui.label(format!("Editing: {}", name));
                        if let Ok(children) = q_children.get(rover_entity) {
                             ui.collapsing("Suspension Parameters", |ui| {
                                for child in children.iter() {
                                    inspect_suspension_recursive(ui, child, &q_children, &mut q_suspension);
                                }
                             });
                        }
                    });
                }
            }
        });
    }

    egui::Window::new("Celestial Control").default_width(320.0).show(ctx, |ui| {
        ui.heading("Mission Status");
        ui.separator();
        
        // Navigation Metadata
        if let Some((_, _, cam_gtf, _)) = q_camera.iter().next() {
            if let Some(target) = current_focus {
                if let Ok((_, target_gtf, body, name)) = q_bodies.get(target) {
                     let dist_m = cam_gtf.translation().distance(target_gtf.translation()) as f64;
                     let altitude_km = (dist_m - body.radius_m) / 1000.0;
                     
                     ui.group(|ui| {
                         ui.label(format!("TARGET: {}", name.as_str()));
                         ui.label(format!("RANGE: {:.1} km", dist_m / 1000.0));
                         if altitude_km < 100_000.0 {
                             ui.colored_label(egui::Color32::from_rgb(0, 255, 150), format!("ALTITUDE: {:.2} km", altitude_km));
                         }
                     });
                }
            }
        }

        ui.separator();
        // Landing Logic
        if !q_tiles.is_empty() {
             ui.group(|ui| {
                ui.label("READY FOR LANDING");
                if ui.button("L - SPAWN ROVER").clicked() {
                    for (_, _, cam_gtf, mut obs) in q_camera.iter_mut() {
                        let mut nearest_body = None;
                        let mut min_dist = f32::MAX;
                        for (body_ent, body_gtf, body, _) in q_bodies.iter() {
                            let dist = cam_gtf.translation().distance(body_gtf.translation());
                            if dist < min_dist {
                                min_dist = dist;
                                nearest_body = Some((body_ent, body_gtf, body));
                            }
                        }
                        if let Some((body_ent, body_gtf, body)) = nearest_body {
                            let rover_id = lunco_sim_celestial::spawn_rover_at_camera_surface(&mut commands, cam_gtf, body_gtf, body, body_ent);
                            obs.focus_target = Some(rover_id);
                            obs.distance = 50.0;
                        }
                    }
                }
             });
        }

        ui.separator();
        ui.label(format!("Julian Date: {:.4}", world_clock.epoch));
        ui.horizontal(|ui| { if ui.button(if world_clock.paused { "▶ Play" } else { "⏸ Pause" }).clicked() { world_clock.paused = !world_clock.paused; } });
        ui.horizontal_wrapped(|ui| { 
            let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
            for &m in &multipliers { 
               if ui.selectable_label(world_clock.speed_multiplier == m, format!("{}x", m)).clicked() { 
                   world_clock.speed_multiplier = m; 
               } 
            } 
        });
        
        ui.separator();
        ui.label("Focus Target:");
        for (_, _, _, mut obs) in q_camera.iter_mut() {
            ui.horizontal_wrapped(|ui| {
                for (entity, _, _, name) in q_bodies.iter() {
                    if ui.selectable_label(obs.focus_target == Some(entity), name.as_str()).clicked() { obs.focus_target = Some(entity); }
                }
                for (entity, name) in q_rovers.iter() {
                    if ui.selectable_label(obs.focus_target == Some(entity), name.as_str()).clicked() { obs.focus_target = Some(entity); }
                }
            });
        }
    });

    draw_sun_marker(contexts, &q_sun, &q_camera);
}

fn draw_sun_marker(mut contexts: EguiContexts, q_sun: &Query<&GlobalTransform, (With<CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>, q_camera: &Query<(Entity, &Camera, &GlobalTransform, &mut ObserverCamera)>) {
    let Some(sun_gtf) = q_sun.iter().next() else { return; };
    let Some((_, camera, cam_gtf, _)) = q_camera.iter().next() else { return; };
    let dir_to_sun = (sun_gtf.translation() - cam_gtf.translation()).normalize_or_zero();
    if let Ok(screen_pos) = camera.world_to_viewport(cam_gtf, cam_gtf.translation() + dir_to_sun * 100.0) {
        let Ok(ctx) = contexts.ctx_mut() else { return; };
        let painter = ctx.debug_painter();
        painter.circle_filled(egui::pos2(screen_pos.x, screen_pos.y), 10.0, egui::Color32::from_rgb(255, 255, 0));
        painter.text(egui::pos2(screen_pos.x, screen_pos.y + 15.0), egui::Align2::CENTER_TOP, "SUN", egui::FontId::proportional(14.0), egui::Color32::WHITE);
    }
}

fn inspect_suspension_recursive(ui: &mut egui::Ui, entity: Entity, q_children: &Query<&Children>, q_suspension: &mut Query<(Entity, &mut Suspension)>) {
    if let Ok((_e, mut susp)) = q_suspension.get_mut(entity) {
        ui.label(format!("Hub: {:?}", entity));
        ui.add(egui::Slider::new(&mut susp.rest_length, 0.1..=2.0).text("Rest Length"));
        ui.add(egui::Slider::new(&mut susp.spring_k, 1000.0..=100000.0).text("Spring K"));
        ui.add(egui::Slider::new(&mut susp.damping_c, 100.0..=10000.0).text("Damping C"));
        ui.separator();
    }
    if let Ok(children) = q_children.get(entity) { for child in children.iter() { inspect_suspension_recursive(ui, child, q_children, q_suspension); } }
}
