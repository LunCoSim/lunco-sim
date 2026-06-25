//! Avatar UI panels — camera mode display and surface coordinates.

use bevy::prelude::*;
use bevy::math::DVec3;
use bevy_egui::{egui, EguiContexts};
use lunco_workbench::{Panel, PanelId, PanelSlot, WorkbenchAppExt};

use lunco_core::{Avatar, GlobalEntityId, SessionProfiles, SessionRegistry};
use crate::RoverNameTagSettings;
use lunco_celestial::{CelestialBody, LocalGravityField, LeaveSurface};
use big_space::prelude::{CellCoord, Grid};

use crate::{SpringArmCamera, OrbitCamera, FreeFlightCamera, FrameBlend};

/// Avatar status panel — camera mode and surface coordinates.
pub struct AvatarStatusPanel;

impl Panel for AvatarStatusPanel {
    fn id(&self) -> PanelId { PanelId("avatar_status") }
    fn title(&self) -> String { "Telemetry".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Capture the palette up front: semantic status colours for this
        // panel come from the active Theme (falls back to plain white when
        // headless / no theme registered).
        let palette = world.get_resource::<lunco_theme::Theme>().map(|t| t.colors.clone());
        if let Some(theme) = world.get_resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }
        // No theme (headless) → PLACEHOLDER lets egui use its default text colour.
        let body_color = palette
            .as_ref()
            .map(|p| p.peach)
            .unwrap_or(egui::Color32::PLACEHOLDER);

        ui.heading("Avatar Status");
        ui.separator();

        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        // ── Surface Mode Info ──
        let gf = world.get_resource::<LocalGravityField>().map(|gf| (gf.body_entity, gf.surface_g));
        if let Some(gf) = &gf {
            if let Some(body) = gf.0 {
                ui.horizontal(|ui| {
                    ui.label("Surface Mode — Body:");
                    ui.colored_label(body_color, format!("{:?}", body));
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
                        world.commands().trigger(LeaveSurface { target: avatar_ent });
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
    let avatar_data: Option<(DVec3, CellCoord, Entity)> = {
        let mut q = world.query::<(&Transform, &CellCoord, &ChildOf)>();
        q.get(world, avatar_ent).ok().map(|(tf, cell, child_of)| {
            (tf.translation.as_dvec3(), *cell, child_of.0)
        })
    };
    let (tf_pos, cell, parent) = avatar_data?;

    let grid_data: Option<DVec3> = {
        let mut grid_q = world.query::<&Grid>();
        grid_q.get(world, parent).ok().map(|grid| {
            let dummy_tf = Transform::from_translation(tf_pos.as_vec3());
            grid.grid_position_double(&cell, &dummy_tf)
        })
    };
    let body_local = grid_data?;
    let dist = body_local.length();

    let mut body_q = world.query::<&CelestialBody>();
    let Ok(body_comp) = body_q.get(world, body) else { return None };

    let height = dist - body_comp.radius_m;
    let body_local_norm = if dist > 1e-6 { body_local / dist } else { DVec3::Y };
    let lat = body_local_norm.y.asin().to_degrees();
    let lon = body_local_norm.x.atan2(body_local_norm.z).to_degrees();
    Some((lat, lon, height))
}

fn get_camera_mode_info(world: &mut World) -> (egui::Color32, String, String) {
    // Status colours come from the active Theme palette; fall back to the
    // original literals when headless / no theme registered.
    let palette = world.get_resource::<lunco_theme::Theme>().map(|t| t.colors.clone());
    // No theme (headless) → PLACEHOLDER lets egui use its default text colour.
    let c = |pick: fn(&lunco_theme::ColorPalette) -> egui::Color32| {
        palette.as_ref().map(pick).unwrap_or(egui::Color32::PLACEHOLDER)
    };

    let mut blend_q = world.query::<&FrameBlend>();
    if let Ok(blend) = blend_q.single(world) {
        let progress = (blend.t / blend.duration * 100.0).min(100.0) as i32;
        return (c(|p| p.yellow), format!("TRANSITION ({}%)", progress), String::new());
    }

    let mut spring_q = world.query::<&SpringArmCamera>();
    if let Ok(arm) = spring_q.single(world) {
        return (c(|p| p.maroon), "SPRING ARM".to_string(), format!("Distance: {:.1} m", arm.distance));
    }

    let mut orbit_q = world.query::<&OrbitCamera>();
    if let Ok(orbit) = orbit_q.single(world) {
        return (c(|p| p.blue), "ORBIT".to_string(), format!("Distance: {:.1} m", orbit.distance));
    }

    let mut ff_q = world.query::<&FreeFlightCamera>();
    if ff_q.single(world).is_ok() {
        return (c(|p| p.peach), "FREE FLIGHT".to_string(), String::new());
    }

    (c(|p| p.text), "UNKNOWN".to_string(), String::new())
}

/// Plugin that registers avatar UI panels.
pub struct AvatarUiPlugin;

impl Plugin for AvatarUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(AvatarStatusPanel);
    }
}

/// Draw a floating name tag above every possessed rover, in screen space.
///
/// Registered in the egui pass by [`crate::LunCoAvatarPlugin`] (ui-gated) so it
/// composites on top of the 3D viewport regardless of camera setup. Each rover's
/// world position (plus a vertical offset) is projected through the active avatar
/// camera; rovers behind the camera or off the near plane are skipped
/// (`world_to_viewport` returns `Err`).
pub fn draw_rover_name_tags(
    mut egui_ctx: EguiContexts,
    registry: Res<SessionRegistry>,
    profiles: Res<SessionProfiles>,
    settings: Res<RoverNameTagSettings>,
    q_camera: Query<(&Camera, &GlobalTransform), With<Avatar>>,
    q_rovers: Query<(&GlobalEntityId, &GlobalTransform)>,
) {
    // The avatar camera is the one rendering this client's viewport. Without it
    // (e.g. headless / pre-spawn) there is nothing to project against.
    let Some((camera, cam_gtf)) = q_camera.iter().next() else { return };

    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    let [tr, tg, tb, _] = settings.text_color.to_srgba().to_u8_array();
    let origin = ctx.content_rect().min.to_vec2();
    let cam_pos = cam_gtf.translation();

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("rover_name_tags"),
    ));

    for (gid, gtf) in q_rovers.iter() {
        let Some(session) = registry.owner_of(gid.get()) else { continue };

        // Distance from the camera drives both size and fade. Cull past the
        // max so we don't paint a swarm of pinpoint tags across the horizon.
        let distance = gtf.translation().distance(cam_pos);
        if distance > settings.max_distance {
            continue;
        }

        // Size scales inversely with distance (`reference / distance`): a rover
        // at `reference_distance` renders at the nominal `font_size`, closer ones
        // grow, farther ones shrink. Clamp so it never balloons or vanishes.
        let scale = (settings.reference_distance / distance.max(0.01)).clamp(0.35, 2.5);
        let font_size = (settings.font_size * scale).max(7.0);

        // Fade linearly from `reference_distance` out to `max_distance`.
        let fade = if distance <= settings.reference_distance {
            1.0
        } else {
            let span = (settings.max_distance - settings.reference_distance).max(0.01);
            (1.0 - (distance - settings.reference_distance) / span).clamp(0.0, 1.0)
        };

        let name = profiles
            .profiles
            .get(&session.0)
            .cloned()
            .unwrap_or_else(|| format!("Player {}", session.0));

        let world = gtf.translation() + Vec3::Y * settings.vertical_offset;
        let Ok(viewport) = camera.world_to_viewport(cam_gtf, world) else { continue };
        let pos = egui::pos2(viewport.x, viewport.y) + origin;

        let text_color =
            egui::Color32::from_rgba_unmultiplied(tr, tg, tb, (255.0 * fade) as u8);
        let font = egui::FontId::proportional(font_size);

        // Anchor the text centered just above the projected point, with a
        // semi-transparent backing so it stays legible over bright terrain.
        let galley = painter.layout_no_wrap(name, font, text_color);
        let size = galley.size();
        let top_left = pos - egui::vec2(size.x * 0.5, size.y);
        let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(4.0, 2.0));
        painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha((150.0 * fade) as u8));
        painter.galley(top_left, galley, text_color);
    }
}
