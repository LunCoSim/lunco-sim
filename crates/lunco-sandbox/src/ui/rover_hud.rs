//! Driver HUD for the View perspective — the cockpit overlay for whatever vessel
//! the local avatar is currently possessing.
//!
//! Two floating clusters, painted only while something is actually being driven
//! (free-flight shows nothing, so the plain sandbox viewport stays clean). Both sit
//! along the BOTTOM edge, flanking the viewport centre — the thing the driver is
//! actually looking at — rather than boxing it in from three sides:
//!
//! - **ATTITUDE** (bottom-left) — the tilt gauge, SPEED as the hero number, then
//!   roll/pitch as one line of fine print. Tilt is the number that matters on a
//!   slope: it is what puts a rover on its roof.
//! - **NAV + COMMS** (bottom-right) — the vessel's name, ALT as the hero number,
//!   then E/N/heading as one line, and the live link home.
//!
//! ONE hero readout per cluster, centred and large; everything else is a compact
//! inline row. A HUD of equal-weight rows makes the driver read all of it to find
//! the one number the moment is about — speed while flying, altitude while
//! landing — and on camera it reads as a debug dump rather than an instrument.
//!
//! The key-press legend deliberately lives NOWHERE here: `lunco-workbench`'s
//! `input_overlay` already paints it centre-screen, and a second copy in this
//! panel said the same thing twice while pushing the numbers into the margins.
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

use avian3d::prelude::{ComputedCenterOfMass, LinearVelocity};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::link::LinkState;
use lunco_controller::ControllerLink;
use lunco_mobility::WheelRaycast;
use lunco_core::{Avatar, GlobalEntityId};

/// Fallback amber threshold, for a vessel whose limits cannot be derived.
///
/// GENERIC on purpose: the real roll-over angle is `atan(half_track / com_height)`
/// and the real slip limit is `atan(μ)`, both properties of the AUTHORED vehicle.
/// A rover now publishes exactly those as [`VesselEnvelope`], and the HUD prefers
/// them — see `docs/architecture/58-vessel-envelope-and-routes.md`. These remain
/// for the unknown-vehicle case (a lander, a wheel-less body), where they are
/// honest "meaningful slope" / "slope that rolls things" bands spanning the range
/// real lunar rovers cared about (Lunokhod-1 drove to ~32° operationally, with a
/// 45° auto-brake cut-out).
///
/// They must NOT be used for a wheeled rover. Against the Summer Space School
/// ladder these generic bands are *inverted*: the awful tier slips at 21.8° (only
/// just amber) while the easy tier screams red at 30° with 22° of margin left —
/// the driver most at risk got the mildest warning.
const FALLBACK_CAUTION_TILT_DEG: f32 = 20.0;
/// Fallback red threshold. See [`FALLBACK_CAUTION_TILT_DEG`].
const FALLBACK_DANGER_TILT_DEG: f32 = 30.0;

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
    /// Amber threshold — this vessel's own slip limit when derivable, else the
    /// generic fallback. See [`FALLBACK_CAUTION_TILT_DEG`].
    caution_deg: f32,
    /// Red threshold — this vessel's own tip limit when derivable.
    danger_deg: f32,
    /// True when the bands above came from the vessel rather than the fallback,
    /// so the gauge can say which it is showing. A driver reading a limit needs to
    /// know whether it is *their* limit.
    limits_derived: bool,
}

/// The tilt bands to paint, in degrees: (amber, red).
///
/// Pure arithmetic over the vessel's authored parts, kept as a free function so the
/// derivation can be tested without a World — and so it stays obviously cheap.
/// It is NOT cached anywhere: `atan` of a min is not worth a stored component, and
/// a stored copy could go stale against the tire it derives from, which is the
/// exact failure this is meant to remove.
///
/// * amber = slip limit = `atan(min μ)`. **min, not mean** — a vehicle slips at its
///   weakest contact, and averaging would flatter a rover with one bald tire.
/// * red = tip limit = `atan(half_track / CoM-height-above-contact)`.
///
/// `com_above_contact <= 0` has no finite tip angle (CoM at or below the contact
/// plane), so red falls back rather than reporting ~90°, which would read as
/// "extremely stable" when the truth is "this model does not apply".
///
/// See `docs/architecture/58-vessel-envelope-and-routes.md`.
fn tilt_bands(min_mu: f64, half_track: f64, com_above_contact: f64) -> (f32, f32) {
    let caution = min_mu.max(0.0).atan().to_degrees() as f32;
    let danger = if com_above_contact > 1e-3 && half_track > 1e-3 {
        (half_track / com_above_contact).atan().to_degrees() as f32
    } else {
        FALLBACK_DANGER_TILT_DEG
    };
    // Never let amber sit above red: the easy tier slips at 52.4°, past its own
    // fallback red, and a gauge whose bands cross is worse than a generic one.
    (caution, danger.max(caution))
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
    // Site anchor + body radius, resolved once by the caller. `None` in a scene
    // with no site frame — the peer still gets a name and a range.
    // coordinates, exactly as the billboards degrade.
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
    //
    // The candidate names stay borrowed until the winner is picked — this runs
    // every frame per driven vessel, and eagerly copying both names allocated two
    // Strings to throw one away. Exactly one allocation happens now, on the
    // branch that survives.
    // ⚠ `Name` on a USD-spawned entity is the FULL PRIM PATH
    // (`Name::new(child_path.to_string())`, `lunco-usd-bevy`), not the leaf. The
    // owner-substitution below used to compare the whole path against "Antenna",
    // which never matched — so the driver read `/Traverse/Base/Antenna` where the
    // code intended `Base`. Truncate to the leaf FIRST, then decide.
    let leaf = |n: &str| n.rsplit('/').next().unwrap_or(n).to_string();

    let peer_ent = q_ids.iter().find(|(_, g)| g.get() == pick.peer).map(|(e, _)| e);
    let peer_label = match peer_ent {
        Some(e) => {
            let own = q_name.get(e).ok().map(|n| leaf(n.as_str()));
            let parent = q_parents
                .get(e)
                .ok()
                .and_then(|p| q_name.get(p.parent()).ok())
                .map(|n| leaf(n.as_str()));
            match (own, parent) {
                // An "Antenna"/"Comms" node under a named structure reads better as
                // its owner; anything else keeps its own name.
                (Some(o), Some(p)) if o == "Antenna" || o == "Comms" => p,
                (Some(o), _) => o,
                (None, Some(p)) => p,
                (None, None) => format!("#{}", pick.peer),
            }
        }
        None => format!("#{}", pick.peer),
    };

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
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_gid: &Query<&GlobalEntityId>,
    q_vel: &Query<&LinearVelocity>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_links: &Query<(Entity, &LinkState)>,
    q_ids: &Query<(Entity, &GlobalEntityId)>,
    q_wheels: &Query<(Entity, &WheelRaycast, &Transform)>,
    q_com: &Query<&ComputedCenterOfMass>,
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

    // The HUD title is the ship's NAME, not its address: prefer the USD
    // `ui:displayName` (ingested as `Callsign`) over the `Name` component,
    // which carries the prim path and reads as plumbing on camera.
    let label = q_callsign
        .get(vessel)
        .map(|c| c.0.clone())
        .or_else(|_| q_name.get(vessel).map(|n| n.as_str().to_string()))
        .or_else(|_| q_gid.get(vessel).map(|g| format!("vessel #{}", g.get())))
        .unwrap_or_else(|_| "vessel".to_string());

    // Derive this vessel's own bands from its wheels, at the point of use. Six
    // wheels, a min and an atan — cheaper per frame than the layout of the panel
    // it labels, and with no cached copy that could disagree with the tire.
    //
    // Wheels hang under the chassis (often via a suspension link), so match by
    // ancestry rather than by direct parentage — the same walk `resolve_link` uses
    // to find a radio.
    let mut min_mu = f64::MAX;
    let mut half_track: f64 = 0.0;
    // Contact plane: the lowest point any tire touches, in chassis-local space.
    let mut contact_y = f64::MAX;
    let mut wheels = 0usize;
    for (wheel, w, t) in q_wheels.iter() {
        // Wheel pose in CHASSIS space: the wheel's own `Transform` is local to
        // its PARENT, which for a suspension-linked wheel is the link, not the
        // chassis — so compose each intermediate link's transform on the way up.
        let mut e = wheel;
        let mut owned = false;
        let mut p = t.translation;
        for _ in 0..8 {
            let Ok(parent) = q_parents.get(e) else { break };
            e = parent.parent();
            if e == vessel {
                owned = true;
                break;
            }
            let Ok((_, link_t)) = q_spatial.get(e) else { break };
            p = link_t.transform_point(p);
        }
        if !owned {
            continue;
        }
        wheels += 1;
        min_mu = min_mu.min(w.friction_mu);
        half_track = half_track.max((p.x as f64).abs());
        contact_y = contact_y.min(p.y as f64 - w.wheel_radius);
    }

    // No wheels ⇒ not a ground vehicle (a lander, a free camera): keep the honest
    // generic bands rather than inventing limits for a vehicle model that does not
    // apply.
    let (caution_deg, danger_deg, limits_derived) = if wheels > 0 {
        let com_above_contact = q_com
            .get(vessel)
            .map(|c| c.0.y - contact_y)
            .unwrap_or(f64::NAN);
        let (c, d) = tilt_bands(min_mu, half_track, com_above_contact);
        (c, d, true)
    } else {
        (
            FALLBACK_CAUTION_TILT_DEG,
            FALLBACK_DANGER_TILT_DEG,
            false,
        )
    };

    Some(DrivenVessel {
        label,
        pos,
        tilt_deg,
        roll_deg,
        pitch_deg,
        heading_deg,
        speed: q_vel.get(vessel).ok().map(|v| v.length() as f32),
        link: resolve_link(vessel, q_links, q_parents, q_name, q_ids),
        caution_deg,
        danger_deg,
        limits_derived,
    })
}

#[cfg(test)]
mod tilt_band_tests {
    use super::*;

    /// The three tiers from the Summer Space School twin's `SURVEY.md` ladder,
    /// with the shipped `six_wheel_rover.usda` geometry: wheels at x = ±1.0,
    /// y = −0.15, radius 0.4, so the contact plane sits at y = −0.55.
    ///
    /// Pinned deliberately. If these drift, either the derivation broke or the
    /// survey needs re-checking, and both want a human to look.
    #[test]
    fn bands_reproduce_the_surveyed_rover_ladder() {
        // easy: cleated μ=1.3, CoM −0.25 ⇒ 0.30 m above contact
        let (slip, tip) = tilt_bands(1.3, 1.0, -0.25 - -0.55);
        assert!((slip - 52.4).abs() < 0.1, "easy slip {slip}");
        assert!((tip - 73.3).abs() < 0.1, "easy tip {tip}");

        // medium: worn μ=0.5, CoM −0.05 ⇒ 0.50 m above contact
        let (slip, tip) = tilt_bands(0.5, 1.0, -0.05 - -0.55);
        assert!((slip - 26.6).abs() < 0.1, "medium slip {slip}");
        assert!((tip - 63.4).abs() < 0.1, "medium tip {tip}");

        // awful: bald μ=0.4, CoM +0.45 ⇒ 1.00 m above contact
        let (slip, tip) = tilt_bands(0.4, 1.0, 0.45 - -0.55);
        assert!((slip - 21.8).abs() < 0.1, "awful slip {slip}");
        assert!((tip - 45.0).abs() < 0.1, "awful tip {tip}");
    }

    /// The generic bands are *inverted* against this ladder — the awful tier slips
    /// at 21.8°, only just past a 20° amber, while the easy tier would scream red
    /// at 30° with 22° of margin left. This test states the defect the derivation
    /// exists to fix, so nobody restores the constants thinking they were fine.
    #[test]
    fn generic_bands_would_mislead_both_extremes() {
        let (awful_slip, _) = tilt_bands(0.4, 1.0, 1.0);
        assert!(
            awful_slip > FALLBACK_CAUTION_TILT_DEG,
            "awful rover slips at {awful_slip}, generic amber is {FALLBACK_CAUTION_TILT_DEG} — \
             it would still read 'caution' while already sliding"
        );
        let (easy_slip, _) = tilt_bands(1.3, 1.0, 0.30);
        assert!(
            easy_slip > FALLBACK_DANGER_TILT_DEG,
            "easy rover slips at {easy_slip}, generic red is {FALLBACK_DANGER_TILT_DEG} — \
             it would read 'danger' with {} deg of real margin left",
            easy_slip - FALLBACK_DANGER_TILT_DEG
        );
    }

    /// CoM at or below the contact plane: fall back rather than report ~90°.
    #[test]
    fn tip_band_falls_back_when_com_is_at_the_contact_plane() {
        let (_, tip) = tilt_bands(0.5, 1.0, 0.0);
        assert_eq!(tip, FALLBACK_DANGER_TILT_DEG);
    }

    /// Amber must never sit above red, or the gauge draws crossed bands.
    #[test]
    fn amber_never_exceeds_red() {
        // Easy tier on a very stable chassis: slip 52.4° vs a tip of ~45°.
        let (slip, tip) = tilt_bands(1.3, 1.0, 1.0);
        assert!(tip >= slip, "bands crossed: slip {slip}, tip {tip}");
    }
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
            // Instrument cyan, NOT the theme's accent. The theme accent is a
            // saturated violet tuned for editor chrome (buttons, selection);
            // on a flight HUD over grey regolith it reads as a UI toy and is
            // tiring to look at for a whole descent. Cyan is the aviation
            // convention for "live number", holds contrast against both the
            // dark panel and a blown-out sunlit background, and matches the
            // campaign's title cards so film and instrument agree.
            accent: egui::Color32::from_rgb(0x7F, 0xD4, 0xFF),
            value: k.text,
            muted: k.text_subdued,
        }
    }

    /// Colour for a tilt reading. Success → warning → error, matching the arc zones.
    fn tilt(&self, tilt_deg: f32, caution_deg: f32, danger_deg: f32) -> egui::Color32 {
        if tilt_deg >= danger_deg {
            self.danger
        } else if tilt_deg >= caution_deg {
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

    // Band edges come from the VESSEL, so the dial reads in this rover's terms.
    // Clamped to the dial's span: the easy tier slips at 52.4°, past the end of a
    // 45° dial, and the honest rendering of that is a dial with no red on it — not
    // an arc drawn off the edge. The dial stays a fixed 45° across tiers on
    // purpose, so a driver who switches rovers compares like with like.
    let caution = v.caution_deg.clamp(0.0, MAX_DEG);
    let danger = v.danger_deg.clamp(caution, MAX_DEG);

    // Bands, mirrored left and right of vertical.
    for sign in [-1.0_f32, 1.0] {
        arc(&painter, 0.0, sign * caution, pal.band_ok, 5.0);
        arc(&painter, sign * caution, sign * danger, pal.band_caution, 5.0);
        arc(&painter, sign * danger, sign * MAX_DEG, pal.band_danger, 5.0);
    }

    // Needle: lean direction follows roll, magnitude is total tilt (so a purely
    // pitched-up rover still reads its tilt, it just does not lean).
    let lean = v.roll_deg.signum() * v.tilt_deg.min(MAX_DEG);
    let a = to_screen(lean);
    let tip = egui::pos2(centre.x + (radius - 8.0) * a.cos(), centre.y + (radius - 8.0) * a.sin());
    // Colour against the UNCLAMPED limits — the reading must go amber at the real
    // slip angle even when that sits past the end of the dial.
    let col = pal.tilt(v.tilt_deg, v.caution_deg, v.danger_deg);
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
    // Name the limits the bands are drawn from. A driver reading a coloured arc
    // needs to know whether it is THEIR rover's limit or a generic one — without
    // this the same dial means different things on different vehicles and looks
    // identical.
    painter.text(
        egui::pos2(centre.x, centre.y - 2.0),
        egui::Align2::CENTER_CENTER,
        if v.limits_derived {
            format!("slip {:.0}° · tip {:.0}°", v.caution_deg, v.danger_deg)
        } else {
            "generic limits".to_string()
        },
        egui::FontId::proportional(8.0),
        pal.muted,
    );
}

/// The HERO readout: small caps label over a large monospace value with its
/// unit — for the one or two numbers a beat is ABOUT (speed while flying,
/// altitude while landing). Everything else stays in [`readout`] fine print.
fn hero_readout(ui: &mut egui::Ui, label: &str, value: String, unit: &str, color: egui::Color32) {
    // Value and unit are ONE laid-out text run, not two widgets in a
    // horizontal strip. `vertical_centered` centres each CHILD it lays out, and
    // a horizontal strip claims the panel's full width — so its contents stayed
    // hard left while the caption above them centred, which is exactly the
    // "numbers aren't centred" misalignment. One galley has a real width, so
    // centring it centres what you see.
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &value,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::monospace(26.0),
            color,
            ..Default::default()
        },
    );
    if !unit.is_empty() {
        job.append(
            unit,
            4.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.0),
                color: color.linear_multiply(0.55),
                // Sit the unit on the big number's baseline instead of the top
                // of its line box, where it floated level with the digits' caps.
                valign: egui::Align::BOTTOM,
                ..Default::default()
            },
        );
    }
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(label)
                .weak()
                .size(9.0)
                .extra_letter_spacing(1.5),
        );
        ui.add_space(1.0);
        ui.label(job);
    });
}

/// Several label/value pairs on ONE centred line — the compact idiom for the
/// secondary numbers (roll/pitch, E/N/heading). A stack of single-number rows
/// costs a panel's whole height to say very little; inline pairs keep the same
/// information under the hero readout without competing with it.
fn inline_pairs(ui: &mut egui::Ui, pairs: &[(&str, String)], pal: &Palette) {
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, (label, value)) in pairs.iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new("·").weak().size(10.0));
                }
                ui.label(egui::RichText::new(*label).weak().size(9.0));
                ui.label(
                    egui::RichText::new(value)
                        .color(pal.value)
                        .monospace()
                        .size(11.0),
                );
            }
        });
    });
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

/// ATTITUDE cluster (bottom-left) + NAV/COMMS cluster (bottom-right).
/// Both early-out in free flight.
// The driver HUD is NOT gated. It paints whenever a vessel is possessed.
//
// It describes the VEHICLE — attitude, speed, position, wheel state — which is
// information you want whenever a vehicle is being driven, including in recorded
// footage. It was briefly put behind a `SetHud` opt-in along with the key-press
// readout; that conflated two different things. The thing that needs gating is the
// INPUT OVERLAY (`lunco-workbench`'s `input_overlay`), which shows which keys the
// operator is holding: that answers a question only a teaching context asks, and is
// noise everywhere else. Vehicle state is not chrome; operator state is.

pub(crate) fn draw_rover_hud(
    mut egui_ctx: EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_name: Query<&Name>,
    q_callsign: Query<&lunco_core::markers::Callsign>,
    q_gid: Query<&GlobalEntityId>,
    q_vel: Query<&LinearVelocity>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_links: Query<(Entity, &LinkState)>,
    q_ids: Query<(Entity, &GlobalEntityId)>,
    q_wheels: Query<(Entity, &WheelRaycast, &Transform)>,
    q_com: Query<&ComputedCenterOfMass>,
) {
    let Some(theme) = theme else { return };
    let pal = Palette::of(&theme);
    let Some(v) = resolve_driven(
        &q_avatar, &q_name, &q_callsign, &q_gid, &q_vel, &q_parents, &q_grids, &q_spatial,
        &q_links, &q_ids, &q_wheels, &q_com,
    ) else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

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
                    // SPEED is the number a pilot flies by — hero-sized, not a
                    // row in the fine print.
                    match v.speed {
                        Some(s) => hero_readout(ui, "SPEED", format!("{s:.1}"), "m/s", pal.accent),
                        None => hero_readout(ui, "SPEED", "—".into(), "", pal.muted),
                    }
                    ui.separator();
                    // Roll and pitch on ONE line. Two full label/value rows for
                    // two small angles doubled the panel's height for numbers
                    // nobody reads digit-by-digit — the gauge above already
                    // shows attitude; these are the fine print under it.
                    inline_pairs(ui, &[
                        ("R", format!("{:+.0}°", v.roll_deg)),
                        ("P", format!("{:+.0}°", v.pitch_deg)),
                    ], &pal);
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
                    ui.label(egui::RichText::new(&v.label).strong().size(15.0));
                    ui.label(egui::RichText::new("site frame · metres").weak().size(9.0));
                    ui.separator();
                    // ALT is the landing's hero number: what the narration and
                    // the audience track all the way to touchdown.
                    hero_readout(ui, "ALT", format!("{:.1}", v.pos.y), "m", pal.accent);
                    ui.separator();
                    // Position and heading on ONE line — three stacked rows of
                    // one number each was most of this panel's height.
                    inline_pairs(ui, &[
                        ("E", format!("{:+.0}", v.pos.x)),
                        ("N", format!("{:+.0}", -v.pos.z)),
                        ("HDG", format!("{:.0}°", v.heading_deg)),
                    ], &pal);

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
                            // Range and elevation on ONE line, and the peer's own
                            // survey coordinates on NONE.
                            //
                            // The `at lat,lon` / `alt` rows were two of this panel's
                            // eight, spent restating something that does not change
                            // and is already on the station's billboard — while the
                            // peer's `alt` sat directly under the driver's own ALT
                            // hero in the same column, two different altitudes
                            // labelled almost identically. That is the duplication:
                            // not the same fact twice, but a fact about SOMEWHERE
                            // ELSE dressed to look like a fact about here.
                            //
                            // What a driver needs from a link is whether it closes,
                            // to whom, how far and how high — which is what is left.
                            inline_pairs(ui, &[
                                ("range", range),
                                ("elev", format!("{:+.0}°", link.elevation_deg)),
                            ], &pal);
                            if !link.connected {
                                ui.label(
                                    egui::RichText::new("no line of sight — autonomy only")
                                        .color(pal.danger)
                                        .size(9.0),
                                );
                            }
                        }
                    }

                    // NO key legend here. The live key-press readout is its
                    // own centre-screen overlay (`lunco-workbench`'s
                    // `input_overlay`), so repeating W/A/S/D in this panel
                    // showed the same information twice and pushed the numbers
                    // that matter — position, comms — into the fine print.
                });
        });
}
