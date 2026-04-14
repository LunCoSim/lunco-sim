//! Mission Control panel — single unified panel for time, bodies, spacecraft, rovers, and actions.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use chrono::TimeZone;

use lunco_core::{Avatar, RoverVessel, Spacecraft, CelestialClock, PossessVessel, ReleaseVessel, FocusTarget, TeleportToSurface, LeaveSurface};
use lunco_celestial::CelestialBody;

/// Mission Control panel — everything in one place.
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

        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        // ── Time Control ──
        ui.heading("Time Control");
        if let Some(clock) = world.get_resource::<CelestialClock>() {
            ui.label(format!("JD: {:.4}", clock.epoch));
            ui.label(format!("UTC: {}", jd_to_utc_string(clock.epoch)));
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
                            world.commands().trigger(FocusTarget { avatar: av, target: *entity });
                        }
                    }
                    if ui.small_button("🌕 Surface").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(TeleportToSurface {
                                target: av,
                                body_entity: entity.to_bits(),
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
                            world.commands().trigger(FocusTarget { avatar: av, target: *entity });
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
                            world.commands().trigger(FocusTarget { avatar: av, target: *entity });
                        }
                    }
                    if ui.small_button("🚗 Possess").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(PossessVessel { avatar: av, target: *entity });
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
                world.commands().trigger(ReleaseVessel { target: av });
            }

            // Return to Orbit — show when avatar is in surface mode
            let on_surface = {
                let mut q = world.query::<&lunco_avatar::SurfaceCamera>();
                q.iter(world).next().is_some()
            };
            if on_surface {
                if ui.button("🏠 Return to Orbit").clicked() {
                    let target = world.get_resource::<lunco_celestial::LocalGravityField>()
                        .and_then(|gf| gf.body_entity);
                    if target.is_some() {
                        world.commands().trigger(LeaveSurface { target: av });
                    }
                }
            }
        }

        ui.separator();
        ui.label("Double-click entities in Inspector to focus.");
        ui.label("WASD: move  |  QE: Up/Down");
        ui.label("Right-Click: rotate  |  SPACE: pause");
    }
}

fn jd_to_utc_string(jd: f64) -> String {
    let j2000 = 2451545.0;
    let days_since_j2000 = (jd - j2000) as i64;
    let base = chrono::Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap()
        + chrono::Duration::days(days_since_j2000);
    base.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
