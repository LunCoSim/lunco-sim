//! Mission Control panel — WorkbenchPanel implementation.
//!
//! Restores the old "Mission Control" egui window as a docked panel.
//! Provides time management, entity navigation, focus/possession/surface controls.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use lunco_core::{Avatar, RoverVessel, Spacecraft, architecture::CommandMessage};
use lunco_celestial::{CelestialClock, CelestialBody};
use lunco_avatar::OrbitCamera;

/// Mission Control panel — time, entities, and navigation controls.
pub struct MissionControl;

impl WorkbenchPanel for MissionControl {
    fn id(&self) -> &str { "mission_control" }
    fn title(&self) -> String { "Mission Control".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Mission Control requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        // ── Time Control ──
        ui.heading("Epoch & UTC Time");
        if let Some(clock) = world.get_resource::<CelestialClock>() {
            ui.label(format!("JD: {:.4}", clock.epoch));
            ui.label(format!("UTC: {}", lunco_celestial::jd_to_utc_string(clock.epoch)));
        }
        if let Some(mut clock) = world.get_resource_mut::<CelestialClock>() {
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
        }
        ui.separator();

        // Find avatar entity for commands
        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        // ── Celestial Bodies ──
        ui.collapsing("Celestial Bodies", |ui| {
            let mut body_q = world.query::<(Entity, &Name, &CelestialBody)>();
            let bodies: Vec<(Entity, String, String)> = body_q.iter(world)
                .map(|(e, n, body)| (e, n.as_str().to_string(), format!("{:.0} km", body.radius_m / 1000.0)))
                .collect();
            for (entity, name, radius) in &bodies {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", name, radius));
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(CommandMessage {
                                id: 0, target: *entity, name: "FOCUS".to_string(),
                                args: Default::default(), source: av,
                            });
                        }
                    }
                    if ui.small_button("🌕 Surface").clicked() {
                        if let Some(av) = avatar_ent {
                            let args: Vec<f64> = vec![entity.to_bits() as f64];
                            world.commands().trigger(CommandMessage {
                                id: 0, target: *entity, name: "TELEPORT_SURFACE".to_string(),
                                args: args.into_iter().collect(), source: av,
                            });
                        }
                    }
                });
            }
        });

        // ── Spacecraft ──
        ui.collapsing("Spacecraft", |ui| {
            let mut sc_q = world.query::<(Entity, &Name)>();
            let scs: Vec<(Entity, String)> = sc_q.iter(world)
                .filter(|(e, _)| world.get::<Spacecraft>(*e).is_some())
                .map(|(e, n)| (e, n.as_str().to_string()))
                .collect();
            for (entity, name) in &scs {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(CommandMessage {
                                id: 0, target: *entity, name: "FOCUS".to_string(),
                                args: Default::default(), source: av,
                            });
                        }
                    }
                });
            }
        });

        // ── Rovers ──
        ui.collapsing("Rovers", |ui| {
            let mut rover_q = world.query::<(Entity, &Name)>();
            let rovers: Vec<(Entity, String)> = rover_q.iter(world)
                .filter(|(e, _)| world.get::<RoverVessel>(*e).is_some())
                .map(|(e, n)| (e, n.as_str().to_string()))
                .collect();
            for (entity, name) in &rovers {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(CommandMessage {
                                id: 0, target: *entity, name: "FOCUS".to_string(),
                                args: Default::default(), source: av,
                            });
                        }
                    }
                    if ui.small_button("🚗 Possess").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(CommandMessage {
                                id: 0, target: *entity, name: "POSSESS".to_string(),
                                args: Default::default(), source: av,
                            });
                        }
                    }
                });
            }
        });

        // ── Quick Actions ──
        ui.separator();
        ui.heading("Quick Actions");
        if let Some(av) = avatar_ent {
            if ui.button("🚀 Release (Free Fly)").clicked() {
                world.commands().trigger(CommandMessage {
                    id: 0, target: av, name: "RELEASE".to_string(),
                    args: Default::default(), source: Entity::PLACEHOLDER,
                });
            }

            // Return to Orbit — show when camera is in OrbitCamera mode
            let orbit_target_body = {
                let mut orbit_q = world.query::<&OrbitCamera>();
                orbit_q.iter(world).next().map(|o| o.target)
            };
            if let Some(body_ent) = orbit_target_body {
                if ui.button("🏠 Return to Orbit").clicked() {
                    world.commands().trigger(CommandMessage {
                        id: 0, target: body_ent, name: "LEAVE_SURFACE".to_string(),
                        args: Default::default(), source: av,
                    });
                }
            }
        }

        ui.separator();
        ui.label("Double-click entities in Inspector to focus.");
        ui.label("WASD: move  |  QE: Up/Down");
        ui.label("Right-Click: rotate  |  SPACE: pause");
    }
}
