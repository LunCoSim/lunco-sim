use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_sim_physics::Suspension;
// use lunco_sim_rover_raycast::WheelRaycast; // Might need to check if this crate exists
use lunco_sim_core::RoverVessel;
use lunco_sim_celestial::CelestialClock;

pub struct LunCoSimUiPlugin;

impl Plugin for LunCoSimUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<SelectedRover>()
           // THE RECOVERY BRIDGE: This system catches raw Bevy Input events and 
           // manually injects them into Egui. This ensures the UI stays clickable 
           // on systems where the Window's transform-less state blocks automatic propagation.
           .add_systems(PreUpdate, manual_egui_input_bridge) 
           
           // CRITICAL: We move UI drawing to EguiPrimaryContextPass. 
           // This schedule runs after input but before rendering, which stabilizes 
           // the clicking behavior in complex multi-camera planetary simulations.
           .add_systems(EguiPrimaryContextPass, main_ui_system);
    }
}

#[derive(Resource, Default)]
struct SelectedRover {
    entity: Option<Entity>,
}

/// Manual Input Bridge: Collects Bevy RAW mouse events and injects them into Egui.
/// This bypasses the broken automatic input propagation in certain Bevy 0.18 environments.
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
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedRover>,
    q_rovers: Query<(Entity, &Name), With<RoverVessel>>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    // mut q_raycast_wheels: Query<(Entity, &mut WheelRaycast)>,
    q_children: Query<&Children>,
    world_clock: Option<ResMut<CelestialClock>>,
    q_sun: Query<&GlobalTransform, (With<lunco_sim_celestial::CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>,
    mut q_camera: Query<(Entity, &Camera, &GlobalTransform, &mut lunco_sim_celestial::ObserverCamera)>,
    q_bodies: Query<(Entity, &Name), With<lunco_sim_celestial::CelestialBody>>,
    mut frame_timer: Local<u32>,
) {
    *frame_timer += 1;
    if *frame_timer < 10 { return; }

    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let Some(mut world_clock) = world_clock else { return; };

    let mut show_rover_ui = false;
    if let Some((_ent, _cam, _gtf, obs)) = q_camera.iter().next() {
        if let Some(target) = obs.focus_target {
            if q_rovers.contains(target) {
                show_rover_ui = true;
            }
        }
    }

    if show_rover_ui {
        egui::Window::new("Rover Parameters").default_width(300.0).show(ctx, |ui| {
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

    egui::Window::new("Celestial Control").default_width(300.0).show(ctx, |ui| {
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

        ui.separator();
        ui.label("Focus Target:");
        if let Some((_, _, _, mut obs)) = q_camera.iter_mut().next() {
            ui.horizontal_wrapped(|ui| {
                for (entity, name) in q_bodies.iter() {
                    if ui.selectable_label(obs.focus_target == Some(entity), name.as_str()).clicked() {
                        obs.focus_target = Some(entity);
                    }
                }
                for (entity, name) in q_rovers.iter() {
                    if ui.selectable_label(obs.focus_target == Some(entity), name.as_str()).clicked() {
                        obs.focus_target = Some(entity);
                    }
                }
            });
            
            ui.separator();
            ui.label(format!("Camera Scale: {:.0} km", obs.distance / 1000.0));
            let dist_au = obs.distance / 1.496e11;
            if dist_au > 0.1 {
                ui.label(format!("({:.3} AU)", dist_au));
            }
        }
    });

    draw_sun_marker(contexts, &q_sun, &q_camera);
}

fn draw_sun_marker(
    mut contexts: EguiContexts,
    q_sun: &Query<&GlobalTransform, (With<lunco_sim_celestial::CelestialBody>, With<lunco_sim_celestial::SolarSystemRoot>)>,
    q_camera: &Query<(Entity, &Camera, &GlobalTransform, &mut lunco_sim_celestial::ObserverCamera)>,
) {
    let Some(sun_gtf) = q_sun.iter().next() else { return; };
    let Some((_cam_ent, camera, cam_gtf, _)) = q_camera.iter().next() else { return; };
    
    let sun_pos_abs = sun_gtf.translation();
    let cam_pos_abs = cam_gtf.translation();
    let dir_to_sun = (sun_pos_abs - cam_pos_abs).normalize_or_zero();
    
    if let Ok(screen_pos) = camera.world_to_viewport(cam_gtf, cam_pos_abs + dir_to_sun * 1000.0) {
        let Ok(ctx) = contexts.ctx_mut() else { return; };
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
