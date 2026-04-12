//! Telemetry panel — WorkbenchPanel implementation.
//!
//! Shows avatar status, surface mode info, lat/lon/alt, camera mode,
//! and navigation buttons (Return to Orbit).

use bevy::prelude::*;
use bevy::math::DVec3;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use lunco_core::{Avatar, architecture::CommandMessage};
use lunco_celestial::{CelestialBody, LocalGravityField};
use lunco_avatar::{SpringArmCamera, OrbitCamera, FreeFlightCamera, FrameBlend};
use big_space::prelude::{CellCoord, Grid};

/// Telemetry panel — shows avatar status and surface coordinates.
pub struct TelemetryPanel;

impl WorkbenchPanel for TelemetryPanel {
    fn id(&self) -> &str { "telemetry" }
    fn title(&self) -> String { "Telemetry".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Telemetry requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.heading("Avatar Status");
        ui.separator();

        let gf = world.get_resource::<LocalGravityField>().map(|gf| (gf.body_entity, gf.surface_g));
        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        // ── Surface Mode Info + Return to Orbit ──
        if let Some(gf) = &gf {
            if let Some(body) = gf.0 {
                ui.horizontal(|ui| {
                    ui.label("Surface Mode — Body:");
                    ui.colored_label(egui::Color32::from_rgb(255, 180, 50), format!("{:?}", body));
                });
                ui.label(format!("Gravity: {:.3} m/s²", gf.1));

                let lat_lon_height = if let Some(avatar_ent) = avatar_ent {
                    compute_lat_lon_height(world, avatar_ent, body)
                } else { None };

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
                    if let Some(avatar_ent) = avatar_ent {
                        world.commands().trigger(CommandMessage {
                            id: 0, target: body, name: "LEAVE_SURFACE".to_string(),
                            args: Default::default(), source: avatar_ent,
                        });
                    }
                }
                ui.separator();
            }
        }

        // ── Camera Mode ──
        let mode_info = get_camera_mode_info(world);
        ui.horizontal(|ui| {
            ui.label("Mode:");
            ui.colored_label(mode_info.0, &mode_info.1);
        });
        if !mode_info.2.is_empty() {
            ui.label(&mode_info.2);
        }

        ui.separator();
        ui.label("WASD: move");
        ui.label("QE: Up/Down");
        ui.label("SHIFT: Speed boost");
        ui.label("SCROLL or +/-: zoom (Spring/Orbit)");
        ui.label("Right-Click: rotate");
        ui.label("SPACE: pause/unpause");
    }
}

fn compute_lat_lon_height(world: &mut World, avatar_ent: Entity, body: Entity) -> Option<(f64, f64, f64)> {
    // Query avatar data, copy it out to avoid borrow conflicts
    let avatar_data: Option<(DVec3, CellCoord, Entity)> = {
        let mut q = world.query::<(&Transform, &CellCoord, &ChildOf)>();
        q.get(world, avatar_ent).ok().map(|(tf, cell, child_of)| {
            (tf.translation.as_dvec3(), *cell, child_of.0)
        })
    };
    let (tf_pos, cell, parent) = avatar_data?;

    // Query grid
    let grid_data: Option<DVec3> = {
        let mut grid_q = world.query::<&Grid>();
        grid_q.get(world, parent).ok().map(|grid| {
            let dummy_tf = Transform::from_translation(tf_pos.as_vec3());
            grid.grid_position_double(&cell, &dummy_tf)
        })
    };
    let body_local = grid_data?;
    let dist = body_local.length();

    // Query body
    let mut body_q = world.query::<&CelestialBody>();
    let Ok(body_comp) = body_q.get(world, body) else { return None };

    let height = dist - body_comp.radius_m;
    let body_local_norm = if dist > 1e-6 { body_local / dist } else { DVec3::Y };
    let lat = body_local_norm.y.asin().to_degrees();
    let lon = body_local_norm.x.atan2(body_local_norm.z).to_degrees();
    Some((lat, lon, height))
}

fn get_camera_mode_info(world: &mut World) -> (egui::Color32, String, String) {
    let mut blend_q = world.query::<&FrameBlend>();
    if let Ok(blend) = blend_q.single(world) {
        let progress = (blend.t / blend.duration * 100.0).min(100.0) as i32;
        return (egui::Color32::from_rgb(200, 200, 50), format!("TRANSITION ({}%)", progress), String::new());
    }

    let mut spring_q = world.query::<&SpringArmCamera>();
    if let Ok(arm) = spring_q.single(world) {
        return (egui::Color32::from_rgb(255, 100, 50), "SPRING ARM".to_string(), format!("Distance: {:.1} m", arm.distance));
    }

    let mut orbit_q = world.query::<&OrbitCamera>();
    if let Ok(orbit) = orbit_q.single(world) {
        return (egui::Color32::from_rgb(100, 150, 255), "ORBIT".to_string(), format!("Distance: {:.1} m", orbit.distance));
    }

    let mut ff_q = world.query::<&FreeFlightCamera>();
    if ff_q.single(world).is_ok() {
        return (egui::Color32::from_rgb(255, 200, 50), "FREE FLIGHT".to_string(), String::new());
    }

    (egui::Color32::WHITE, "UNKNOWN".to_string(), String::new())
}
