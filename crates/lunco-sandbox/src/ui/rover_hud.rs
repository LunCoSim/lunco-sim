//! Driver HUD for the View perspective — the cockpit overlay for whatever vessel
//! the local avatar is currently possessing.
//!
//! Two floating clusters, painted only while something is actually being driven
//! (free-flight shows nothing, so the plain sandbox viewport stays clean). Both sit
//! along the BOTTOM edge, flanking the viewport centre — the thing the driver is
//! actually looking at — rather than boxing it in from three sides:
//!
//! - **ATTITUDE** (bottom-left) — tilt, roll, pitch, speed. Tilt is the number
//!   that matters on a slope: it is what puts a rover on its roof.
//! - **NAV + CONTROLS** (bottom-right) — position and heading in the stable root
//!   frame, plus the live drive inputs.
//!
//! TRANSPORT (pause + rate) is deliberately NOT here: the workbench toolbar already
//! owns the pause button and the same `TimeTransport` authority, and it explicitly
//! avoids a second transport row. The rate buttons were added next to it there.
//!
//! These are raw `egui::Area`s rather than registered Workbench `Panel`s on
//! purpose. The View perspective is full-screen 3D with no dock, and
//! `PanelSlot::Floating` is a declared-but-unimplemented placeholder
//! (`lunco-workbench/src/panel.rs`), so a floating HUD has exactly one sanctioned
//! shape today — the one `view_mode.rs`, `draw_notifications` and
//! `draw_waypoint_overlay` all already use.
//!
//! FRAME: pose comes from [`lunco_core::coords::world_pose`], which walks the cell
//! chain and applies ancestor grid rotation. A camera-relative `GlobalTransform` is
//! floating-origin-relative and useless for geography — see the same note on
//! `mode_exposure`. In a site-anchored scene the root frame IS site-ENU metres
//! (East +X, Up +Y, North −Z), which is the frame the survey and any route
//! waypoints are already expressed in.

use avian3d::prelude::LinearVelocity;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use big_space::prelude::{CellCoord, Grid};
use leafwing_input_manager::prelude::ActionState;
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, GlobalEntityId, UserIntent};

/// Tilt (degrees from local up) past which the readout goes amber.
///
/// GENERIC, and deliberately not a per-vehicle limit: the real roll-over angle is
/// `atan(half_track / com_height)` and the real slip limit is `atan(μ)`, both of
/// which are properties of the AUTHORED vehicle, not of this crate. A rover that
/// wants its own arcs should publish them; until it does, these are honest
/// "you are on a meaningful slope" / "you are on a slope that rolls things"
/// thresholds, spanning the range real lunar rovers actually cared about
/// (Lunokhod-1 drove to ~32° operationally, with a 45° auto-brake cut-out).
const CAUTION_TILT_DEG: f32 = 20.0;
/// Tilt past which the readout goes red. See [`CAUTION_TILT_DEG`].
const DANGER_TILT_DEG: f32 = 30.0;

/// What the HUD needs about the driven vessel, resolved once per frame.
struct DrivenVessel {
    label: String,
    /// Metres, root frame (site-ENU in a site-anchored scene).
    pos: DVec3,
    /// Degrees from local up. The tip-over-relevant number.
    tilt_deg: f32,
    roll_deg: f32,
    pitch_deg: f32,
    /// Compass degrees, 0 = North (−Z), clockwise through East (+X).
    heading_deg: f32,
    /// Metres/second, or `None` for a body avian is not integrating.
    speed: Option<f32>,
}

/// Resolve the vessel the local avatar is driving, or `None` in free flight.
fn resolve_driven(
    q_avatar: &Query<&ControllerLink, With<Avatar>>,
    q_name: &Query<&Name>,
    q_gid: &Query<&GlobalEntityId>,
    q_vel: &Query<&LinearVelocity>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DrivenVessel> {
    let vessel = q_avatar.iter().next()?.vessel_entity;
    let (pos, rot) = lunco_core::coords::world_pose(vessel, q_parents, q_grids, q_spatial)?;
    let rot = rot.as_quat();

    // Local up = world up. Over a 1 km site the body's curvature contributes
    // d²/2R ≈ 0.3 m of sag, i.e. ~0.03° of tilt — far below the gauge's
    // resolution. A multi-km traverse would need the real local up (away from
    // the body centre), which is what `mode_exposure` computes.
    let up = rot * Vec3::Y;
    let tilt_deg = up.dot(Vec3::Y).clamp(-1.0, 1.0).acos().to_degrees();

    // Bevy convention: forward is −Z, right is +X.
    let forward = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;
    let pitch_deg = forward.y.clamp(-1.0, 1.0).asin().to_degrees();
    let roll_deg = right.y.clamp(-1.0, 1.0).asin().to_degrees();

    // Compass heading: North is −Z, East is +X.
    let heading_deg = forward.x.atan2(-forward.z).to_degrees().rem_euclid(360.0);

    let label = q_name
        .get(vessel)
        .map(|n| n.as_str().to_string())
        .or_else(|_| q_gid.get(vessel).map(|g| format!("vessel #{}", g.get())))
        .unwrap_or_else(|_| "vessel".to_string());

    Some(DrivenVessel {
        label,
        pos,
        tilt_deg,
        roll_deg,
        pitch_deg,
        heading_deg,
        speed: q_vel.get(vessel).ok().map(|v| v.length() as f32),
    })
}

/// The HUD's palette — every colour resolved from the ACTIVE theme's semantic
/// design tokens, never a literal RGB.
///
/// This matters beyond tidiness. The first cut hardcoded a dark-theme palette, and
/// under the light theme the readouts came out near-white on near-white: legible
/// only if you already knew where to look. Reading the tokens means the HUD tracks
/// whatever theme is active (and any future theme) for free, and the
/// intent → colour mapping stays in one place — `lunco_theme::DesignTokens`.
///
/// The gauge bands needed an inactive-track colour for warning/error; only
/// `success_subdued` existed, so `warning_subdued`/`error_subdued` were added to
/// the tokens alongside it rather than mixed locally.
///
/// SOURCE MATTERS: the tokens come from the `Res<Theme>` RESOURCE, not from
/// `lunco_theme::active(ctx)`. `active()` reads a per-frame copy that something has
/// to have published into the egui context with `store_active` — and the only caller
/// in the whole repo is the Modelica canvas. Everywhere else `active()` silently
/// returns `Theme::dark()`, so reading it here produced dark-theme text on the light
/// theme's panel: white on white. `Res<Theme>` is the authority every other panel
/// (e.g. `terrain_tools`) already reads.
struct Palette {
    ok: egui::Color32,
    caution: egui::Color32,
    danger: egui::Color32,
    band_ok: egui::Color32,
    band_caution: egui::Color32,
    band_danger: egui::Color32,
    accent: egui::Color32,
    value: egui::Color32,
    muted: egui::Color32,
    cap_idle: egui::Color32,
}

impl Palette {
    fn of(theme: &lunco_theme::Theme) -> Self {
        let k = &theme.tokens;
        Self {
            ok: k.success,
            caution: k.warning,
            danger: k.error,
            band_ok: k.success_subdued,
            band_caution: k.warning_subdued,
            band_danger: k.error_subdued,
            accent: k.accent,
            value: k.text,
            muted: k.text_subdued,
            cap_idle: k.surface_sunken,
        }
    }

    /// Colour for a tilt reading. Success → warning → error, matching the arc zones.
    fn tilt(&self, tilt_deg: f32) -> egui::Color32 {
        if tilt_deg >= DANGER_TILT_DEG {
            self.danger
        } else if tilt_deg >= CAUTION_TILT_DEG {
            self.caution
        } else {
            self.ok
        }
    }
}

/// Paint the attitude gauge: a half-dial from 0° to 45° of tilt, with the
/// caution/danger bands drawn as arc segments and the live reading as a needle.
fn attitude_gauge(ui: &mut egui::Ui, v: &DrivenVessel, pal: &Palette) {
    const MAX_DEG: f32 = 45.0;
    let size = egui::vec2(150.0, 84.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    let centre = egui::pos2(rect.center().x, rect.bottom() - 10.0);
    let radius = 62.0;

    // Dial angles: tilt 0 at straight up (−90° in egui screen space, y down),
    // sweeping symmetrically to ±MAX_DEG. We draw the right half only and
    // mirror the reading, so the needle leans the way the vehicle leans.
    let to_screen = |deg: f32| -> f32 { (-90.0 + deg * (90.0 / MAX_DEG)).to_radians() };
    let arc = |painter: &egui::Painter, from: f32, to: f32, color: egui::Color32, w: f32| {
        let steps = 24;
        let pts: Vec<egui::Pos2> = (0..=steps)
            .map(|i| {
                let t = from + (to - from) * (i as f32 / steps as f32);
                let a = to_screen(t);
                egui::pos2(centre.x + radius * a.cos(), centre.y + radius * a.sin())
            })
            .collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(w, color)));
    };

    // Bands, mirrored left and right of vertical.
    for sign in [-1.0_f32, 1.0] {
        arc(&painter, 0.0, sign * CAUTION_TILT_DEG, pal.band_ok, 5.0);
        arc(
            &painter,
            sign * CAUTION_TILT_DEG,
            sign * DANGER_TILT_DEG,
            pal.band_caution,
            5.0,
        );
        arc(
            &painter,
            sign * DANGER_TILT_DEG,
            sign * MAX_DEG,
            pal.band_danger,
            5.0,
        );
    }

    // Needle: lean direction follows roll, magnitude is total tilt (so a purely
    // pitched-up rover still reads its tilt, it just does not lean).
    let lean = v.roll_deg.signum() * v.tilt_deg.min(MAX_DEG);
    let a = to_screen(lean);
    let tip = egui::pos2(centre.x + (radius - 8.0) * a.cos(), centre.y + (radius - 8.0) * a.sin());
    let col = pal.tilt(v.tilt_deg);
    painter.line_segment([centre, tip], egui::Stroke::new(2.5, col));
    painter.circle_filled(centre, 4.0, col);

    painter.text(
        egui::pos2(centre.x, centre.y - 34.0),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}°", v.tilt_deg),
        egui::FontId::proportional(22.0),
        col,
    );
    painter.text(
        egui::pos2(centre.x, centre.y - 16.0),
        egui::Align2::CENTER_CENTER,
        "TILT",
        egui::FontId::proportional(9.0),
        pal.muted,
    );
}

/// A dim label / bright value row — the readout idiom used throughout the HUD.
fn readout(ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).weak().size(10.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(color).monospace().size(12.0));
        });
    });
}

/// One key cap, lit while its intent is live. The legend doubles as an input
/// monitor: a student who cannot make the rover move can see at a glance whether
/// the key is even reaching the sim (vs. the vessel refusing to drive).
fn key_cap(ui: &mut egui::Ui, label: &str, active: bool, pal: &Palette) {
    let (bg, fg) = if active {
        (pal.accent, pal.value)
    } else {
        (pal.cap_idle, pal.muted)
    };
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::monospace(11.0),
        fg,
    );
    let size = egui::vec2(galley.size().x + 10.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, bg);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        fg,
    );
}

/// ATTITUDE cluster (bottom-left) + NAV/CONTROLS cluster (bottom-right).
/// Both early-out in free flight.
pub(crate) fn draw_rover_hud(
    mut egui_ctx: EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_intent: Query<&ActionState<UserIntent>, With<Avatar>>,
    q_name: Query<&Name>,
    q_gid: Query<&GlobalEntityId>,
    q_vel: Query<&LinearVelocity>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    let Some(theme) = theme else { return };
    let pal = Palette::of(&theme);
    let Some(v) = resolve_driven(
        &q_avatar, &q_name, &q_gid, &q_vel, &q_parents, &q_grids, &q_spatial,
    ) else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    // Live intents, straight off the avatar's leafwing state — the same signal the
    // controller turns into throttle/steer/brake port writes, read one hop earlier.
    let held = |i: UserIntent| q_intent.iter().next().is_some_and(|s| s.pressed(&i));

    egui::Area::new(egui::Id::new("rover_hud_attitude"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(14.0, -32.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(150.0);
                    attitude_gauge(ui, &v, &pal);
                    ui.separator();
                    readout(ui, "roll", format!("{:+.0}°", v.roll_deg), pal.value);
                    readout(ui, "pitch", format!("{:+.0}°", v.pitch_deg), pal.value);
                    match v.speed {
                        Some(s) => readout(ui, "speed", format!("{s:.2} m/s"), pal.accent),
                        None => readout(ui, "speed", "—".into(), pal.muted),
                    }
                });
        });

    egui::Area::new(egui::Id::new("rover_hud_nav"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -32.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(168.0);
                    ui.label(egui::RichText::new(&v.label).strong().size(12.0));
                    ui.label(egui::RichText::new("site frame · metres").weak().size(9.0));
                    ui.separator();
                    readout(ui, "E", format!("{:+.1}", v.pos.x), pal.value);
                    readout(ui, "N", format!("{:+.1}", -v.pos.z), pal.value);
                    readout(ui, "elev", format!("{:.1}", v.pos.y), pal.value);
                    readout(ui, "hdg", format!("{:.0}°", v.heading_deg), pal.accent);
                    ui.separator();
                    ui.label(egui::RichText::new("CONTROLS").weak().size(9.0));
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        key_cap(ui, "W", held(UserIntent::MoveForward), &pal);
                        key_cap(ui, "A", held(UserIntent::MoveLeft), &pal);
                        key_cap(ui, "S", held(UserIntent::MoveBackward), &pal);
                        key_cap(ui, "D", held(UserIntent::MoveRight), &pal);
                        key_cap(ui, "SPC", held(UserIntent::Action), &pal);
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("WASD drive · SPACE brake · G release")
                            .weak()
                            .size(9.0),
                    );
                });
        });
}
