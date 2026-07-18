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
//! - **NAV + COMMS + CONTROLS** (bottom-right) — position and heading in the stable
//!   root frame, the live link home, plus the drive inputs.
//!
//! COMMS reads the generic link kernel (`lunco_celestial::link`, doc 49) — real
//! range/elevation/occlusion, never a scripted flag. It is the driver-facing half of
//! the same state `ss3_radio_shadow.rhai` turns into a tele-op refusal: when this
//! says NO LINK, commands genuinely cannot reach the vessel, so the readout has to
//! answer "why is it not responding" without the student going to a panel for it.
//! Shown only for a vessel that carries a link node — see `resolve_link`.
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
use lunco_celestial::link::LinkState;
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
    /// Live comms link, or `None` for a vessel carrying no link node at all.
    link: Option<LinkInfo>,
}

/// The one link the driver actually cares about: can I be commanded right now,
/// and by whom.
///
/// A node may have many peers; the HUD shows ONE. Choosing the nearest CONNECTED
/// peer (falling back to the nearest severed one) matches how
/// `inject_link_state_into_cosim` reduces a class to a single set of ports, so the
/// HUD and the cosim ports never disagree about which peer is "the" link.
struct LinkInfo {
    connected: bool,
    /// Peer prim name, or a GID fallback if the peer has no `Name`.
    peer_label: String,
    range_m: f64,
    elevation_deg: f64,
    /// True when the node has no peers at all — a different failure from "severed":
    /// nothing to talk to, rather than something in the way.
    no_peers: bool,
}

/// Find the driven vessel's link node and reduce it to one headline peer.
///
/// The link node is usually NOT the vessel entity: scenes author the radio as a
/// CHILD prim (`/Traverse/Rover/Comms` in the school twin), because the antenna has
/// its own pose and the vessel is the thing commands address. So walk descendants
/// rather than reading `LinkState` off the vessel and concluding "no comms".
fn resolve_link(
    vessel: Entity,
    q_links: &Query<(Entity, &LinkState)>,
    q_parents: &Query<&ChildOf>,
    q_name: &Query<&Name>,
    q_ids: &Query<(Entity, &GlobalEntityId)>,
) -> Option<LinkInfo> {
    // Depth cap: a radio hangs a hop or two under its vessel. This also makes the
    // walk terminate on a malformed hierarchy instead of spinning.
    const MAX_DEPTH: usize = 8;
    let owned_by_vessel = |mut e: Entity| {
        if e == vessel {
            return true;
        }
        for _ in 0..MAX_DEPTH {
            let Ok(parent) = q_parents.get(e) else { return false };
            e = parent.parent();
            if e == vessel {
                return true;
            }
        }
        false
    };

    let (_, state) = q_links.iter().find(|(e, _)| owned_by_vessel(*e))?;

    if state.peers.is_empty() {
        return Some(LinkInfo {
            connected: false,
            peer_label: "—".into(),
            range_m: 0.0,
            elevation_deg: 0.0,
            no_peers: true,
        });
    }

    // Nearest connected peer, else nearest peer at all.
    let pick = state
        .peers
        .iter()
        .filter(|p| p.connected)
        .min_by(|a, b| a.range_m.total_cmp(&b.range_m))
        .or_else(|| state.peers.iter().min_by(|a, b| a.range_m.total_cmp(&b.range_m)))?;

    // `LinkPeer` names its peer by GID (identity survives despawn/reload; an Entity
    // would not), so resolve GID → entity → `Name` for a label the driver can read.
    // Same GID→entity resolution `link_beams` does to aim a beam at its peer.
    //
    // Prefer the peer's PARENT name when the peer is an antenna child: the driver
    // thinks in terms of "Base", not "Antenna".
    let peer_label = q_ids
        .iter()
        .find(|(_, g)| g.get() == pick.peer)
        .map(|(e, _)| {
            let own = q_name.get(e).ok().map(|n| n.as_str().to_string());
            let parent = q_parents
                .get(e)
                .ok()
                .and_then(|p| q_name.get(p.parent()).ok())
                .map(|n| n.as_str().to_string());
            match (own, parent) {
                // An "Antenna"/"Comms" node under a named structure reads better as
                // its owner; anything else keeps its own name.
                (Some(o), Some(p)) if o == "Antenna" || o == "Comms" => p,
                (Some(o), _) => o,
                (None, Some(p)) => p,
                (None, None) => format!("#{}", pick.peer),
            }
        })
        .unwrap_or_else(|| format!("#{}", pick.peer));

    Some(LinkInfo {
        connected: pick.connected,
        peer_label,
        range_m: pick.range_m,
        elevation_deg: pick.elevation_deg,
        no_peers: false,
    })
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
    q_links: &Query<(Entity, &LinkState)>,
    q_ids: &Query<(Entity, &GlobalEntityId)>,
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
        link: resolve_link(vessel, q_links, q_parents, q_name, q_ids),
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
/// Whether the driver HUD is painted. **Off by default.**
///
/// The HUD is chrome: it belongs to a scene that wants to teach the controls, not to
/// every scene that happens to possess a vessel. Offline recording captures the whole
/// window, so a HUD that appears merely because something is possessed is baked into
/// the footage — and a scripted pilot possesses the vessel exactly as a human does,
/// which is what made it show up uninvited.
///
/// A scene opts in with `cmd("SetHud", #{ rover: true })`, so possession decides
/// *control* and the scene decides *presentation*.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HudSettings {
    /// Paint the driver cockpit (attitude + nav/controls) while possessing.
    pub rover: bool,
}

/// Show or hide the driver HUD. Fields left `None` are unchanged.
#[lunco_core::Command(default)]
pub struct SetHud {
    /// Paint the driver cockpit while possessing a vessel.
    pub rover: Option<bool>,
}

#[lunco_core::on_command(SetHud)]
pub(crate) fn on_set_hud(trigger: On<SetHud>, mut hud: ResMut<HudSettings>) {
    if let Some(rover) = trigger.event().rover {
        hud.rover = rover;
    }
}

// Registered from `SandboxUiPlugin`, which is itself the `ui` feature's plugin — so
// the verb exists exactly when the HUD it controls does, with no `cfg` at the call
// site. Visibility is a RUNTIME setting (`HudSettings`), never a build flag: the same
// binary shows the HUD for a scene that asks for it and stays clean for one that
// does not.
lunco_core::register_commands!(on_set_hud);

pub(crate) fn draw_rover_hud(
    mut egui_ctx: EguiContexts,
    hud: Res<HudSettings>,
    theme: Option<Res<lunco_theme::Theme>>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_intent: Query<&ActionState<UserIntent>, With<Avatar>>,
    q_name: Query<&Name>,
    q_gid: Query<&GlobalEntityId>,
    q_vel: Query<&LinearVelocity>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_links: Query<(Entity, &LinkState)>,
    q_ids: Query<(Entity, &GlobalEntityId)>,
) {
    if !hud.rover {
        return;
    }
    let Some(theme) = theme else { return };
    let pal = Palette::of(&theme);
    let Some(v) = resolve_driven(
        &q_avatar, &q_name, &q_gid, &q_vel, &q_parents, &q_grids, &q_spatial, &q_links,
        &q_ids,
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

                    // COMMS — only for a vessel that actually carries a link node.
                    // A rover with no radio shows nothing rather than a permanent
                    // "NO LINK", which would read as a fault instead of an absence.
                    if let Some(link) = &v.link {
                        ui.separator();
                        let (status, color) = if link.no_peers {
                            ("NO PEERS", pal.muted)
                        } else if link.connected {
                            ("LINK", pal.ok)
                        } else {
                            ("NO LINK", pal.danger)
                        };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("COMMS").weak().size(9.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(status)
                                            .color(color)
                                            .strong()
                                            .size(11.0),
                                    );
                                },
                            );
                        });
                        if !link.no_peers {
                            readout(
                                ui,
                                "peer",
                                link.peer_label.clone(),
                                if link.connected { pal.value } else { pal.muted },
                            );
                            // Range stays legible across the whole span the kernel
                            // covers: metres on a site, km once a peer is orbital or
                            // on another body (Earth is ~384,000 km out).
                            let range = if link.range_m >= 10_000.0 {
                                format!("{:.0} km", link.range_m / 1000.0)
                            } else {
                                format!("{:.0} m", link.range_m)
                            };
                            readout(ui, "range", range, pal.value);
                            readout(
                                ui,
                                "elev",
                                format!("{:+.0}°", link.elevation_deg),
                                pal.value,
                            );
                            if !link.connected {
                                ui.label(
                                    egui::RichText::new("no line of sight — autonomy only")
                                        .color(pal.danger)
                                        .size(9.0),
                                );
                            }
                        }
                    }

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
