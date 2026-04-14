//! Celestial UI panels — time control and celestial body browser.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use lunco_core::{Avatar, CelestialBody, CelestialClock};
use crate::commands::TeleportToSurface;
use chrono::TimeZone;

/// Celestial time control panel.
pub struct CelestialTimePanel;

impl WorkbenchPanel for CelestialTimePanel {
    fn id(&self) -> &str { "celestial_time" }
    fn title(&self) -> String { "Time Control".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Time Control requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.heading("Epoch & UTC Time");
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
    }
}

/// Celestial bodies browser panel.
pub struct CelestialBodiesPanel;

impl WorkbenchPanel for CelestialBodiesPanel {
    fn id(&self) -> &str { "celestial_bodies" }
    fn title(&self) -> String { "Celestial Bodies".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Celestial Bodies requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        let mut body_q = world.query::<(Entity, &Name, &CelestialBody)>();
        let bodies: Vec<(Entity, String, String)> = body_q.iter(world)
            .map(|(e, n, body)| (e, n.as_str().to_string(), format!("{:.0} km", body.radius_m / 1000.0)))
            .collect();

        for (entity, name, radius) in &bodies {
            ui.horizontal(|ui| {
                ui.label(format!("{} ({})", name, radius));
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
    }
}

/// Converts Julian Date to a human-readable UTC string.
fn jd_to_utc_string(jd: f64) -> String {
    let j2000 = 2451545.0;
    let days_since_j2000 = (jd - j2000) as i64;
    let base = chrono::Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap()
        + chrono::Duration::days(days_since_j2000);
    base.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// Plugin that registers celestial UI panels.
pub struct CelestialUiPlugin;

impl Plugin for CelestialUiPlugin {
    fn build(&self, app: &mut App) {
        use bevy_workbench::WorkbenchApp;
        app.register_panel(CelestialTimePanel);
        app.register_panel(CelestialBodiesPanel);
    }
}
