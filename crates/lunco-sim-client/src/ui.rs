use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_sim_core::{RoverVessel, Vessel};
use lunco_sim_celestial::{CelestialClock, ObserverCamera, CelestialBody, ActiveCamera};
use lunco_sim_controller::{ControllerLink, SpaceSystemAction, get_default_input_map};
use lunco_sim_physics::Suspension;

pub struct LunCoSimUiPlugin;

impl Plugin for LunCoSimUiPlugin {
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

#[derive(Resource, Default)]
struct SelectedEntity {
    entity: Option<Entity>,
}

#[derive(Resource, Default)]
struct PendingSpawn {
    request: Option<lunco_sim_celestial::SurfaceClickEvent>,
}

fn on_surface_click(
    trigger: On<lunco_sim_celestial::SurfaceClickEvent>,
    mut pending: ResMut<PendingSpawn>,
) {
    let ev = trigger.event();
    pending.request = Some(lunco_sim_celestial::SurfaceClickEvent {
        planet: ev.planet,
        click_pos_local: ev.click_pos_local,
        surface_normal: ev.surface_normal,
    });
}

fn on_rover_click(
    trigger: On<lunco_sim_celestial::RoverClickEvent>,
    mut selected: ResMut<SelectedEntity>,
) {
    selected.entity = Some(trigger.event().rover);
}

fn main_ui_system(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedEntity>,
    mut pending: ResMut<PendingSpawn>,
    mut clock: ResMut<CelestialClock>,
    q_rovers: Query<(Entity, &Name, &Vessel), With<RoverVessel>>,
    q_bodies: Query<(Entity, &Name, &CelestialBody)>,
    mut q_camera: Query<(Entity, &mut ObserverCamera), With<ActiveCamera>>,
    q_children: Query<&Children>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut commands: Commands,
    mut scroll_res: ResMut<lunco_sim_celestial::CameraScroll>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // Capture scroll before Egui potentially consumes it (if not over window)
    if !ctx.is_pointer_over_area() {
        scroll_res.delta = ctx.input(|i| i.raw_scroll_delta.y);
    }

    egui::Window::new("Mission Control").show(ctx, |ui| {
        ui.heading("Epoch & UTC Time");
        ui.label(format!("JD: {:.4}", clock.epoch));
        ui.label(format!("UTC: {}", clock.to_utc_string()));
        
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

        ui.separator();
        ui.collapsing("Celestial Bodies", |ui| {
            for (entity, name, _) in q_bodies.iter() {
                if ui.selectable_label(selected.entity == Some(entity), format!("{}", name)).clicked() {
                    selected.entity = Some(entity);
                }
            }
        });

        ui.collapsing("Local Vessels", |ui| {
            for (entity, name, _) in q_rovers.iter() {
                 if ui.selectable_label(selected.entity == Some(entity), format!("{}", name)).clicked() {
                     selected.entity = Some(entity);
                 }
            }
        });

        if let Some(target) = selected.entity {
            ui.separator();
            ui.heading("Selection Details");
            ui.label(format!("ID: {:?}", target));
            
            if ui.button("Focus Camera").clicked() {
                for (_, mut obs) in q_camera.iter_mut() {
                    obs.focus_target = Some(target);
                }
            }

            if q_rovers.contains(target) {
                if ui.button("Take Control (Possess)").clicked() {
                    for (cam_ent, mut obs) in q_camera.iter_mut() {
                        obs.focus_target = Some(target);
                        obs.distance = 10.0;
                        commands.entity(cam_ent).insert((
                            ControllerLink { vessel_entity: target },
                            lunco_sim_core::OrbitState::default(),
                        ));
                    }
                    commands.entity(target).insert((
                        leafwing_input_manager::prelude::ActionState::<SpaceSystemAction>::default(),
                        get_default_input_map(),
                    ));
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
                 let mesh_handle = meshes.add(Sphere::new(0.5).mesh().build());
                 let rover = lunco_sim_physics::spawn_joint_ackermann_rover(
                    &mut commands,
                    mesh_handle,
                    spawn_req.click_pos_local.as_vec3() + spawn_req.surface_normal * 1.5,
                    "Lunar Explorer",
                    Color::Srgba(bevy::color::palettes::basic::BLUE),
                 );
                 commands.entity(rover).set_parent_in_place(spawn_req.planet);
                 pending.request = None;
                 info!("Spawned rover at surface interaction point.");
             }
             if ui.button("Cancel").clicked() {
                 pending.request = None;
             }
        }
    });

    egui::Window::new("Telemetry").anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0]).show(ctx, |ui| {
        for (_, obs) in q_camera.iter() {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                match obs.mode {
                    lunco_sim_celestial::ObserverMode::Orbital => ui.colored_label(egui::Color32::from_rgb(100, 150, 255), "ORBITAL"),
                    lunco_sim_celestial::ObserverMode::Flyby => ui.colored_label(egui::Color32::from_rgb(255, 200, 50), "FLYBY"),
                    lunco_sim_celestial::ObserverMode::Surface => ui.colored_label(egui::Color32::from_rgb(50, 255, 100), "SURFACE"),
                };
            });
            ui.label(format!("Alt: {:.3} km", obs.altitude / 1000.0));
            if obs.mode == lunco_sim_celestial::ObserverMode::Orbital {
                ui.label(format!("Orbital Dist: {:.0} km", obs.distance / 1000.0));
            } else {
                ui.label(format!("Center Offset: {:.0} m", obs.local_flyby_pos.length()));
            }
        }
        ui.label("SCROLL or +/- to zoom");
        ui.label("Right-Click rotate");
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
