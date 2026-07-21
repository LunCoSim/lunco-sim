//! Avatar UI panels — camera mode display and surface coordinates.

use bevy::prelude::*;
use bevy::math::DVec3;
use bevy_egui::{egui, EguiContexts};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchAppExt, tutorial_overlay::TutorialHud};

use lunco_core::{Avatar, GlobalEntityId, SessionProfiles, SessionRegistry};
use crate::RoverNameTagSettings;
use lunco_celestial::{CelestialBody, LocalGravityField, LeaveSurface};
use big_space::prelude::{CellCoord, Grid};
use lunco_controller::ControllerLink;

use crate::{SpringArmCamera, OrbitCamera, FreeFlightCamera, FrameBlend};

/// Semantic colour bucket for the camera-mode label.
///
/// The view-model can't carry an `egui::Color32` derived from the theme
/// (the theme is a render-time concern and may be absent headless), so the
/// producer records *which* palette slot the mode maps to and the panel
/// resolves it against the live [`lunco_theme::Theme`] at paint.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ModeColor {
    Yellow,
    Maroon,
    Blue,
    Peach,
    #[default]
    Text,
}

impl ModeColor {
    /// Resolve to a concrete colour from the active palette, or
    /// `PLACEHOLDER` (egui's default text colour) when headless / no theme.
    fn resolve(self, palette: Option<&lunco_theme::ColorPalette>) -> egui::Color32 {
        let Some(p) = palette else { return egui::Color32::PLACEHOLDER };
        match self {
            ModeColor::Yellow => p.yellow,
            ModeColor::Maroon => p.maroon,
            ModeColor::Blue => p.blue,
            ModeColor::Peach => p.peach,
            ModeColor::Text => p.text,
        }
    }
}

/// Surface-mode readout for the avatar's current body.
#[derive(Clone, Default)]
struct SurfaceInfo {
    /// The celestial body the avatar is on (for the `LeaveSurface` target).
    body: Option<Entity>,
    /// Local surface gravity, m/s².
    surface_g: f64,
    /// Geodetic lat (°), lon (°), altitude (m) — `None` until derivable.
    lat_lon_height: Option<(f64, f64, f64)>,
}

/// Change-driven view-model for [`AvatarStatusPanel`] (WP-8).
///
/// `AvatarStatusPanel::render` formerly ran ~8 world scans per frame
/// (avatar lookup, gravity field, lat/lon/height derivation across
/// `Transform`/`CellCoord`/`ChildOf`/`Grid`/`CelestialBody`, and four
/// camera-mode `single()` queries). [`populate_avatar_status_view`]
/// flattens all of that into derived data here; the panel becomes a thin
/// reader + deferred-action emitter. The readout is inherently live
/// (it tracks the avatar as it moves), so the producer runs every
/// `Update` — a deliberate 1-frame lag, no scans in paint.
#[derive(Resource, Default)]
pub struct AvatarStatusView {
    /// The avatar entity, if spawned (target of `LeaveSurface`).
    avatar: Option<Entity>,
    /// Surface readout, present only when on a body.
    surface: Option<SurfaceInfo>,
    /// Camera-mode palette slot.
    mode_color: ModeColor,
    /// Camera-mode label, e.g. `"SPRING ARM"`.
    mode_label: String,
    /// Secondary mode detail, e.g. `"Distance: 12.0 m"` (empty if none).
    mode_detail: String,
    /// The vessel the local avatar is currently controlling (`ControllerLink`
    /// target), or `None` when free-flying. Drives the "Driving: <vessel>" /
    /// "Free flight" readout.
    possessing_vessel: Option<Entity>,
    /// Display label for the possessed vessel (Name or gid), empty when free.
    possessed_label: String,
}

/// Avatar status panel — camera mode and surface coordinates.
pub struct AvatarStatusPanel;

impl Panel for AvatarStatusPanel {
    fn id(&self) -> PanelId { PanelId("avatar_status") }
    fn title(&self) -> String { "Telemetry".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Capture the palette up front: semantic status colours for this
        // panel come from the active Theme (falls back to plain white when
        // headless / no theme registered).
        let palette = ctx.resource::<lunco_theme::Theme>().map(|t| t.colors.clone());
        if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }
        // No theme (headless) → PLACEHOLDER lets egui use its default text colour.
        let body_color = palette
            .as_ref()
            .map(|p| p.peach)
            .unwrap_or(egui::Color32::PLACEHOLDER);
        // "Driving: <vessel>" readout uses the success semantic token (green),
        // resolved from `Theme.tokens` (§3.1) — not the raw palette.
        let success_color = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.success)
            .unwrap_or(egui::Color32::PLACEHOLDER);

        ui.heading("Avatar Status");
        ui.separator();

        // The panel is a pure reader of the change-driven view-model; if it
        // hasn't been produced yet there's nothing to draw.
        let Some(view) = ctx.resource::<AvatarStatusView>() else { return };

        // ── Possession readout ──
        // Step 6: surface "Driving: <vessel>" when the avatar's `ControllerLink`
        // targets a vessel, else "Free flight". The producer
        // (`populate_avatar_status_view`) resolves `ControllerLink` → label.
        if let Some(_vessel) = view.possessing_vessel {
            ui.horizontal(|ui| {
                ui.label("Driving:");
                ui.colored_label(success_color, &view.possessed_label);
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("Status:");
                ui.weak("Free flight");
            });
        }
        ui.separator();

        // ── Surface Mode Info ──
        // `leave_target` defers the `LeaveSurface` trigger until after the
        // `view` borrow ends, so `ctx.defer` is free to take `&mut`.
        let mut leave_target: Option<Entity> = None;
        if let Some(surface) = &view.surface {
            if let Some(body) = surface.body {
                ui.horizontal(|ui| {
                    ui.label("Surface Mode — Body:");
                    ui.colored_label(body_color, format!("{:?}", body));
                });
                ui.label(format!("Gravity: {:.3} m/s²", surface.surface_g));

                if let Some((lat, lon, height)) = surface.lat_lon_height {
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
                    leave_target = view.avatar;
                }
                ui.separator();
            }
        }

        // ── Camera Mode ──
        let mode_color = view.mode_color.resolve(palette.as_ref());
        let mode_label = view.mode_label.clone();
        let mode_detail = view.mode_detail.clone();
        ui.horizontal(|ui| {
            ui.label("Mode:");
            ui.colored_label(mode_color, &mode_label);
        });
        if !mode_detail.is_empty() {
            ui.label(&mode_detail);
        }

        // `view` borrow released above (its data was cloned out); emit the
        // deferred surface-leave intent now.
        if let Some(target) = leave_target {
            ctx.defer(move |world| {
                world.trigger(LeaveSurface { target });
            });
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

/// Producer for [`AvatarStatusView`]. Runs every `Update`: the surface
/// readout tracks the avatar's live position, so unlike a static browser
/// list there is no useful change-gate — but all reads are O(1)
/// single-entity lookups (or a `single()` on the camera), not the
/// per-frame scans the old in-paint code did.
pub fn populate_avatar_status_view(
    mut view: ResMut<AvatarStatusView>,
    palette: Option<Res<lunco_theme::Theme>>,
    gravity: Option<Res<LocalGravityField>>,
    avatars: Query<Entity, With<Avatar>>,
    avatar_pos: Query<(&Transform, &CellCoord, &ChildOf)>,
    grids: Query<&Grid>,
    bodies: Query<&CelestialBody>,
    blends: Query<&FrameBlend>,
    spring: Query<&SpringArmCamera>,
    orbit: Query<&OrbitCamera>,
    free_flight: Query<&FreeFlightCamera>,
    q_link: Query<&ControllerLink>,
    q_name: Query<&Name>,
    q_gid: Query<&GlobalEntityId>,
) {
    let _ = &palette; // colours resolved at paint; producer only records slots.
    let avatar_ent = avatars.iter().next();
    view.avatar = avatar_ent;

    // ── Possession readout ──
    // Resolve the avatar's `ControllerLink` target (the vessel it's driving) and
    // its display label. `None` → free flight.
    (view.possessing_vessel, view.possessed_label) = match avatar_ent {
        Some(av) => match q_link.get(av) {
            Ok(link) => {
                let v = link.vessel_entity;
                let label = q_name.get(v).ok().map(|n| n.as_str().to_string())
                    .or_else(|| q_gid.get(v).ok().map(|g| format!("vessel #{}", g.get())))
                    .unwrap_or_else(|| format!("{:?}", v));
                (Some(v), label)
            }
            Err(_) => (None, String::new()),
        },
        None => (None, String::new()),
    };

    // ── Surface readout ──
    view.surface = gravity.as_ref().and_then(|gf| {
        let body = gf.body_entity?;
        let lat_lon_height = avatar_ent.and_then(|ae| {
            compute_lat_lon_height(ae, body, &avatar_pos, &grids, &bodies)
        });
        Some(SurfaceInfo {
            body: Some(body),
            surface_g: gf.surface_g,
            lat_lon_height,
        })
    });

    // ── Camera mode ──
    let (color, label, detail) = if let Ok(blend) = blends.single() {
        let progress = (blend.t / blend.duration * 100.0).min(100.0) as i32;
        (ModeColor::Yellow, format!("TRANSITION ({}%)", progress), String::new())
    } else if let Ok(arm) = spring.single() {
        (ModeColor::Maroon, "SPRING ARM".to_string(), format!("Distance: {:.1} m", arm.distance))
    } else if let Ok(o) = orbit.single() {
        (ModeColor::Blue, "ORBIT".to_string(), format!("Distance: {:.1} m", o.distance))
    } else if free_flight.single().is_ok() {
        (ModeColor::Peach, "FREE FLIGHT".to_string(), String::new())
    } else {
        (ModeColor::Text, "UNKNOWN".to_string(), String::new())
    };
    view.mode_color = color;
    view.mode_label = label;
    view.mode_detail = detail;
}

/// Derive geodetic lat (°), lon (°), and altitude (m) for `avatar_ent` on
/// `body`, in the body's surface grid frame. Returns `None` when any of
/// the avatar's positional components, the parent grid, or the body's
/// `CelestialBody` are missing.
fn compute_lat_lon_height(
    avatar_ent: Entity,
    body: Entity,
    avatar_pos: &Query<(&Transform, &CellCoord, &ChildOf)>,
    grids: &Query<&Grid>,
    bodies: &Query<&CelestialBody>,
) -> Option<(f64, f64, f64)> {
    let (tf, cell, child_of) = avatar_pos.get(avatar_ent).ok()?;
    let tf_pos = tf.translation.as_dvec3();
    let parent = child_of.0;

    let grid = grids.get(parent).ok()?;
    let dummy_tf = Transform::from_translation(tf_pos.as_vec3());
    let body_local = grid.grid_position_double(cell, &dummy_tf);
    let dist = body_local.length();

    let body_comp = bodies.get(body).ok()?;
    let height = dist - body_comp.radius_m;
    let body_local_norm = if dist > 1e-6 { body_local / dist } else { DVec3::Y };
    let lat = body_local_norm.y.asin().to_degrees();
    let lon = body_local_norm.x.atan2(body_local_norm.z).to_degrees();
    Some((lat, lon, height))
}

fn trigger_tutorial_next(commands: &mut Commands) {
    commands.trigger(lunco_core::TelemetryEvent {
        name: "cmd:TutorialNext".to_string(),
        source: 0,
        severity: lunco_core::Severity::Info,
        data: lunco_core::TelemetryValue::Bool(true),
        timestamp: 0.0,
    });
}

fn on_possess_progress(
    _trigger: On<crate::commands::PossessVessel>,
    hud: Option<Res<TutorialHud>>,
    mut commands: Commands,
) {
    if hud.is_some_and(|h| h.tour.as_ref().and_then(|t| t.require.as_deref()) == Some("possess")) {
        trigger_tutorial_next(&mut commands);
    }
}

fn on_release_progress(
    _trigger: On<crate::commands::ReleaseVessel>,
    hud: Option<Res<TutorialHud>>,
    mut commands: Commands,
) {
    if hud.is_some_and(|h| h.tour.as_ref().and_then(|t| t.require.as_deref()) == Some("release")) {
        trigger_tutorial_next(&mut commands);
    }
}

fn check_tutorial_keyboard_progress(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    hud: Option<Res<TutorialHud>>,
    mut commands: Commands,
) {
    let Some(keys) = keys else { return; };
    let Some(hud) = hud else { return; };
    let Some(tour) = &hud.tour else { return; };
    let Some(require) = &tour.require else { return; };

    let mut done = false;
    match require.as_str() {
        "cycle" => {
            if keys.just_pressed(KeyCode::KeyC) {
                done = true;
            }
        }
        "fly" => {
            if keys.any_just_pressed([KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD]) {
                done = true;
            }
        }
        "release" => {
            if keys.just_pressed(KeyCode::Backspace) {
                done = true;
            }
        }
        _ => {}
    }

    if done {
        trigger_tutorial_next(&mut commands);
    }
}

/// Plugin that registers avatar UI panels.
pub struct AvatarUiPlugin;

impl Plugin for AvatarUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AvatarStatusView>();
        app.add_systems(Update, (populate_avatar_status_view, check_tutorial_keyboard_progress));
        app.register_panel(AvatarStatusPanel);
        app.add_observer(on_possess_progress);
        app.add_observer(on_release_progress);
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
    net_role: Option<Res<lunco_core::NetworkRole>>,
    q_camera: Query<(&Camera, &GlobalTransform), With<Avatar>>,
    q_rovers: Query<(&GlobalEntityId, &GlobalTransform)>,
) {
    // Solo suppression: name tags label OTHER players, who only exist on a wire.
    // In single-player — a `Standalone` role, *including* a session where a local
    // AI autopilot possesses a rover (still solo, not a networked peer) — hide
    // them entirely unless the user opts into `show_always`. (`NetworkRole` absent
    // ⇒ treat as Standalone/solo.)
    let networked = net_role.map(|r| r.is_networked()).unwrap_or(false);
    if !settings.show_always && !networked {
        return;
    }

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
        // TODO(theme): migrate to lunco-theme once the token set covers this.
        // Distance/idle-faded backing behind a player name tag over the 3D scene.
        // Blocked on the dep, as with the toasts below.
        painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha((150.0 * fade) as u8));
        painter.galley(top_left, galley, text_color);
    }
}

/// Draw active [`crate::ScreenNotifications`] toasts as a centered stack near the
/// top of the screen, newest at the bottom; each fades out over its final second.
///
/// ui-gated screen-space overlay (the scene has only a `Camera3d`, so a
/// world-anchored `Text2d` HUD never renders) — registered in the egui pass by
/// [`crate::LunCoAvatarPlugin`]. Mission scripts drive it through rhai `notify`.
pub fn draw_notifications(
    mut egui_ctx: EguiContexts,
    notes: Res<crate::ScreenNotifications>,
) {
    if notes.toasts.is_empty() {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("lunco_notifications"),
    ));
    let rect = ctx.content_rect();
    let cx = rect.center().x;
    let mut y = rect.top() + 56.0;
    let pad = egui::vec2(14.0, 8.0);

    for t in &notes.toasts {
        // Fully opaque until the final second, then linearly fade out.
        let fade = t.remaining.clamp(0.0, 1.0);
        let a = |base: f32| (base * fade) as u8;
        // TODO(theme): migrate to lunco-theme once the token set covers this.
        // Toast bg/fg per severity (success / warn / error / info) drawn over the
        // 3D viewport, each modulating alpha by `fade`. `tokens.success|warning|
        // error` cover the foregrounds; the dark tinted backgrounds have no token.
        // BLOCKED: `lunco-avatar` is reachable from `lunco-sandbox-server`, so it
        // must not gain an unconditional `lunco-theme` edge (bevy_egui -> wgpu).
        let (bg, fg) = match t.kind.as_str() {
            "success" => (
                egui::Color32::from_rgba_unmultiplied(28, 92, 44, a(225.0)),
                egui::Color32::from_rgba_unmultiplied(205, 255, 215, a(255.0)),
            ),
            "warn" => (
                egui::Color32::from_rgba_unmultiplied(120, 88, 20, a(225.0)),
                egui::Color32::from_rgba_unmultiplied(255, 232, 160, a(255.0)),
            ),
            "error" => (
                egui::Color32::from_rgba_unmultiplied(120, 32, 32, a(225.0)),
                egui::Color32::from_rgba_unmultiplied(255, 190, 190, a(255.0)),
            ),
            _ => (
                egui::Color32::from_rgba_unmultiplied(24, 36, 58, a(225.0)),
                egui::Color32::from_rgba_unmultiplied(196, 218, 255, a(255.0)),
            ),
        };

        let font = egui::FontId::proportional(16.0);
        let galley = painter.layout_no_wrap(t.text.clone(), font, fg);
        let size = galley.size();
        let top_left = egui::pos2(cx - size.x * 0.5, y);
        let bg_rect = egui::Rect::from_min_size(top_left, size).expand2(pad);
        painter.rect_filled(bg_rect, 6.0, bg);
        painter.galley(top_left, galley, fg);
        y += size.y + pad.y * 2.0 + 8.0;
    }
}
