//! Driver HUD for the View perspective — the cockpit overlay for whatever vessel
//! the local avatar is currently possessing.
//!
//! Two floating clusters, presented only while something is actually being driven
//! (free-flight shows nothing, so the plain luncosim viewport stays clean). Both sit
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
//! The HUD is a retained Bevy UI tree rather than a Workbench `Panel`. The View
//! perspective is full-screen 3D with no dock, and `PanelSlot::Floating` is a
//! declared-but-unimplemented placeholder (`lunco-workbench/src/panel.rs`), so
//! the template is attached directly to the app's UI world and hidden outside
//! this perspective.
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
use bevy_egui::PrimaryEguiContext;
use bevy_flair::prelude::{InlineStyle, StyleSheet, Styled};
use bevy_hui::prelude::{
    CompileContextEvent, HtmlNode, HtmlStyle, HtmlTemplate, TemplateProperties, UiId,
};
use big_space::prelude::{CellCoord, Grid};
use lunco_autopilot::Autopilot;
use lunco_celestial::link::LinkState;
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, GlobalEntityId};
use lunco_mobility::WheelRaycast;

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

use lunco_cosim::SimComponent;

/// Information about a driven vessel's energy budget/battery.
#[derive(Debug, Clone)]
struct EnergyInfo {
    /// State of charge in percent (0.0 ..= 100.0).
    soc_pct: f32,
    /// Total remaining energy in Wh, if capacity is known.
    energy_wh: Option<f32>,
    /// Pack capacity in Wh, if known.
    capacity_wh: Option<f32>,
}

/// Information about a driven vessel's motor temperatures.
#[derive(Debug, Clone)]
struct ThermalInfo {
    temp_left_k: Option<f32>,
    temp_right_k: Option<f32>,
}

impl ThermalInfo {
    fn max_temp_k(&self) -> f32 {
        match (self.temp_left_k, self.temp_right_k) {
            (Some(l), Some(r)) => l.max(r),
            (Some(l), None) => l,
            (None, Some(r)) => r,
            (None, None) => 250.0,
        }
    }
}

/// What the HUD needs about the driven vessel, resolved once per frame.
struct DrivenVessel {
    entity: Entity,
    label: String,
    /// Metres, root frame (site-ENU in a site-anchored scene).
    pos: DVec3,
    /// Geographic position resolved from the authored site anchor, when the
    /// scene has a geodetic frame. Plain sandboxes retain their ENU readout.
    geo: Option<lunco_celestial::Geodetic>,
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
    /// Energy/battery status, or `None` if vehicle has no battery telemetry.
    energy: Option<EnergyInfo>,
    /// Thermal status, or `None` if vehicle has no thermal telemetry.
    thermal: Option<ThermalInfo>,
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

/// The optional geographic datum is one system parameter so the HUD remains
/// below Bevy's flat system-parameter limit while retaining the authored site
/// coordinate readout.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GeodeticHud<'w, 's> {
    site:
        Query<'w, 's, &'static lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    bodies: Option<Res<'w, lunco_celestial::CelestialBodyRegistry>>,
    /// Kept inside this aggregate system parameter so the HUD stays under
    /// Bevy's flat system-parameter limit.
    autopilots: Query<'w, 's, &'static Autopilot>,
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
    /// `None` when the peer has no horizon to be measured against (an orbiting
    /// relay). The row shows an em dash — a driver reading "+0°" would believe the
    /// dish is on the horizon.
    elevation_deg: Option<f64>,
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
            let Ok(parent) = q_parents.get(e) else {
                return false;
            };
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
            elevation_deg: None,
            no_peers: true,
        });
    }

    // Nearest connected peer, else nearest peer at all.
    let pick = state
        .peers
        .iter()
        .filter(|p| p.connected)
        .min_by(|a, b| a.range_m.total_cmp(&b.range_m))
        .or_else(|| {
            state
                .peers
                .iter()
                .min_by(|a, b| a.range_m.total_cmp(&b.range_m))
        })?;

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

    let peer_ent = q_ids
        .iter()
        .find(|(_, g)| g.get() == pick.peer)
        .map(|(e, _)| e);
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

fn is_owned_by_vessel(entity: Entity, vessel: Entity, q_parents: &Query<&ChildOf>) -> bool {
    if entity == vessel {
        return true;
    }
    let mut curr = entity;
    for _ in 0..8 {
        let Ok(parent) = q_parents.get(curr) else {
            break;
        };
        curr = parent.parent();
        if curr == vessel {
            return true;
        }
    }
    false
}

fn resolve_energy(
    vessel: Entity,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
) -> Option<EnergyInfo> {
    for (ent, sim) in q_sim.iter() {
        if !is_owned_by_vessel(ent, vessel, q_parents) {
            continue;
        }
        let raw_soc = sim
            .outputs
            .get("soc")
            .or_else(|| sim.outputs.get("soc_out"))
            .or_else(|| sim.outputs.get("SOC"))
            .or_else(|| sim.outputs.get("battery_soc"))
            .copied();

        if let Some(raw_soc) = raw_soc {
            let soc_frac = if raw_soc <= 1.0 {
                raw_soc.max(0.0)
            } else {
                (raw_soc / 100.0).clamp(0.0, 1.0)
            };
            let soc_pct = (soc_frac * 100.0) as f32;

            let capacity_wh = capacity_wh(sim);

            let energy_wh = capacity_wh.map(|cap| (soc_frac as f32) * cap);

            return Some(EnergyInfo {
                soc_pct,
                energy_wh,
                capacity_wh,
            });
        }
    }
    None
}

fn capacity_wh(sim: &SimComponent) -> Option<f32> {
    sim.parameters
        .get("capacity_wh")
        .or_else(|| sim.inputs.get("capacity_wh"))
        .copied()
        .map(|value| value as f32)
}

fn resolve_thermal(
    vessel: Entity,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
) -> Option<ThermalInfo> {
    for (ent, sim) in q_sim.iter() {
        if !is_owned_by_vessel(ent, vessel, q_parents) {
            continue;
        }
        let tl = sim
            .outputs
            .get("temp_left")
            .or_else(|| sim.outputs.get("tl"))
            .copied()
            .map(|v| v as f32);
        let tr = sim
            .outputs
            .get("temp_right")
            .or_else(|| sim.outputs.get("tr"))
            .copied()
            .map(|v| v as f32);
        if tl.is_some() || tr.is_some() {
            return Some(ThermalInfo {
                temp_left_k: tl,
                temp_right_k: tr,
            });
        }
    }
    None
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
    q_sim: &Query<(Entity, &SimComponent)>,
    site: Option<&lunco_celestial::GeodeticAnchor>,
    bodies: Option<&lunco_celestial::CelestialBodyRegistry>,
) -> Option<DrivenVessel> {
    let vessel = q_avatar.iter().next()?.vessel_entity;
    let (pos, rot) = lunco_core::coords::world_pose(vessel, q_parents, q_grids, q_spatial)?;
    let pos = pos.0;
    let rot = rot.0.as_quat();

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
            let Ok((_, link_t)) = q_spatial.get(e) else {
                break;
            };
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
        (FALLBACK_CAUTION_TILT_DEG, FALLBACK_DANGER_TILT_DEG, false)
    };

    Some(DrivenVessel {
        entity: vessel,
        label,
        pos,
        geo: site.and_then(|site| hud_geodetic(site, bodies?, pos)),
        tilt_deg,
        roll_deg,
        pitch_deg,
        heading_deg,
        speed: q_vel.get(vessel).ok().map(|v| v.length() as f32),
        link: resolve_link(vessel, q_links, q_parents, q_name, q_ids),
        energy: resolve_energy(vessel, q_sim, q_parents),
        thermal: resolve_thermal(vessel, q_sim, q_parents),
        caution_deg,
        danger_deg,
        limits_derived,
    })
}

/// Convert the vessel's site-ENU position into the body's standard geodetic
/// coordinates. The site anchor names the datum; the body registry owns radius.
fn hud_geodetic(
    site: &lunco_celestial::GeodeticAnchor,
    bodies: &lunco_celestial::CelestialBodyRegistry,
    pos: DVec3,
) -> Option<lunco_celestial::Geodetic> {
    let body = bodies
        .bodies
        .iter()
        .find(|body| body.ephemeris_id == site.body)?;
    Some(lunco_celestial::geo::local_to_geodetic(
        &site.geodetic,
        body.radius_m,
        pos,
    ))
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

#[cfg(test)]
mod energy_thermal_tests {
    use super::*;

    #[test]
    fn thermal_info_max_temp() {
        let t = ThermalInfo {
            temp_left_k: Some(290.0),
            temp_right_k: Some(310.0),
        };
        assert_eq!(t.max_temp_k(), 310.0);

        let t_empty = ThermalInfo {
            temp_left_k: None,
            temp_right_k: None,
        };
        assert_eq!(t_empty.max_temp_k(), 250.0);
    }

    #[test]
    fn energy_capacity_uses_only_explicit_watt_hours() {
        let sim = SimComponent {
            parameters: [
                ("capacity".to_string(), 83.33),
                ("capacity_wh".to_string(), 2_000.0),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert_eq!(capacity_wh(&sim), Some(2_000.0));

        let amp_hours_only = SimComponent {
            parameters: [("capacity".to_string(), 83.33)].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(capacity_wh(&amp_hours_only), None);
    }
}

/// Root entity for the runtime-authored HUD template.
#[derive(Component)]
pub(crate) struct RoverHudRoot;

/// Render-free presentation snapshot consumed by the template adapter.
///
/// The resolver remains here for now because this is the first migration. The
/// renderer only sees this bounded snapshot and never receives an ECS query.
#[derive(Resource, Default)]
pub(crate) struct RoverHudView {
    driven: Option<DrivenVessel>,
    autopilot: bool,
}

/// Spawn the View HUD as an external HUI template with an inherited Flair
/// stylesheet. The entity stays alive across possession and perspective changes;
/// visibility is controlled by the presentation adapter below.
pub(crate) fn spawn_rover_hud(mut commands: Commands, server: Res<AssetServer>) {
    let template: Handle<HtmlTemplate> = server.load("ui/rover_hud.html");
    let stylesheet: Handle<StyleSheet> = server.load("ui/rover_hud.css");

    commands.spawn((
        Node::default(),
        HtmlNode(template),
        Styled::new(stylesheet),
        InlineStyle::default(),
        RoverHudRoot,
        Visibility::Hidden,
    ));
}

/// Keep the retained HUD on its stable window-targeting UI camera. Bevy otherwise
/// selects the highest-order window camera, but the scene camera is replaced
/// during possession and perspective changes.
pub(crate) fn bind_hud_to_camera(
    mut commands: Commands,
    cameras: Query<Entity, With<PrimaryEguiContext>>,
    roots: Query<(Entity, Option<&UiTargetCamera>), With<RoverHudRoot>>,
) {
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    for (entity, target) in &roots {
        if target.is_none_or(|target| target.entity() != camera) {
            commands.entity(entity).insert(UiTargetCamera(camera));
        }
    }
}

/// HUI exposes stable UiIds rather than Bevy Names. Flair selectors follow the
/// Bevy Name component for CSS id selectors, so bridge only this template's IDs
/// after HUI has built its nodes.
pub(crate) fn attach_hui_names(
    mut commands: Commands,
    ids: Query<(Entity, &UiId), (With<Node>, Without<Name>)>,
) {
    for (entity, id) in &ids {
        if id.id().starts_with("rover-hud-") {
            commands.entity(entity).insert(Name::new(id.id().clone()));
        }
    }
}

/// HUI's inline-style system writes its cached HTML attributes every frame.
/// This template deliberately has no HUI style attributes: Flair owns the
/// external CSS and must be the final writer of Bevy UI components. Remove the
/// empty HUI style components after HUI has built the tree so its default
/// `HtmlStyle` values cannot overwrite the stylesheet on the next frame.
pub(crate) fn hand_hud_styling_to_flair(
    mut commands: Commands,
    nodes: Query<(Entity, &UiId), With<HtmlStyle>>,
) {
    for (entity, id) in &nodes {
        if id.id().starts_with("rover-hud-") {
            commands.entity(entity).remove::<HtmlStyle>();
        }
    }
}

/// Publish the driven-vessel view model. This system contains the old HUD's
/// domain resolution, but no presentation calls or layout knowledge.
pub(crate) fn publish_rover_hud_view(
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
    q_sim: Query<(Entity, &SimComponent)>,
    geo: GeodeticHud,
    mut view: ResMut<RoverHudView>,
) {
    let Some(vessel) = resolve_driven(
        &q_avatar,
        &q_name,
        &q_callsign,
        &q_gid,
        &q_vel,
        &q_parents,
        &q_grids,
        &q_spatial,
        &q_links,
        &q_ids,
        &q_wheels,
        &q_com,
        &q_sim,
        geo.site.iter().next(),
        geo.bodies.as_deref(),
    ) else {
        view.driven = None;
        view.autopilot = false;
        return;
    };

    view.autopilot = geo
        .autopilots
        .iter()
        .any(|pilot| pilot.vessel == vessel.entity);
    view.driven = Some(vessel);
}

fn set_property(properties: &mut TemplateProperties, key: &str, value: impl Into<String>) {
    let value = value.into();
    if properties.get(key).map(String::as_str) != Some(value.as_str()) {
        properties.set(key, &value);
    }
}

fn set_css_var(style: &mut InlineStyle, key: &str, value: impl Into<String>) {
    let value = value.into();
    if style.get(key) != Some(value.as_str()) {
        style.set(key.to_string(), value);
    }
}

fn css_color(color: bevy_egui::egui::Color32) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        color.r(),
        color.g(),
        color.b(),
        f32::from(color.a()) / 255.0
    )
}

fn percent(value: f32) -> String {
    format!("{:.2}%", value.clamp(0.0, 100.0))
}

fn link_snapshot(
    link: Option<&LinkInfo>,
    muted: &str,
    ok: &str,
    danger: &str,
) -> (&'static str, String, String, String, String, String, String) {
    let Some(link) = link else {
        return (
            "none",
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            muted.to_string(),
        );
    };

    if link.no_peers {
        return (
            "flex",
            "NO PEERS".into(),
            String::new(),
            String::new(),
            String::new(),
            "none".into(),
            muted.to_string(),
        );
    }

    let range = if link.range_m >= 10_000.0 {
        format!("{:.0} km", link.range_m / 1000.0)
    } else {
        format!("{:.0} m", link.range_m)
    };
    let elevation = link
        .elevation_deg
        .map_or_else(|| "—".to_string(), |e| format!("{e:+.0}°"));
    let los_display = if link.connected { "none" } else { "flex" };
    let color = if link.connected { ok } else { danger };

    (
        "flex",
        if link.connected { "LINK" } else { "NO LINK" }.into(),
        link.peer_label.clone(),
        range,
        elevation,
        los_display.into(),
        color.to_string(),
    )
}

/// Apply the snapshot to the external template. All layout and visual component
/// writes remain in the authored CSS; this adapter only supplies text and root
/// custom properties for the stylesheet.
pub(crate) fn apply_rover_hud_view(
    mut commands: Commands,
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
    view: Res<RoverHudView>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut roots: Query<
        (
            Entity,
            &mut Visibility,
            &mut TemplateProperties,
            &InlineStyle,
        ),
        With<RoverHudRoot>,
    >,
) {
    let in_view = layout.is_some_and(|layout| {
        layout.active_perspective() == Some(lunco_workbench::PerspectiveId("sandbox_view"))
    });

    for (entity, mut visibility, mut properties, existing_style) in &mut roots {
        let driven = view.driven.is_some();
        if !driven || !in_view {
            *visibility = Visibility::Hidden;
        } else {
            *visibility = Visibility::Visible;
        }

        let Some(v) = view.driven.as_ref() else {
            continue;
        };
        if !in_view {
            continue;
        }

        let previous_properties = properties.0.clone();
        let mut style = existing_style.clone();

        let tokens = theme.as_deref().map(|theme| &theme.tokens);
        let panel_background = tokens.map_or_else(
            || "rgba(24,24,37,0.92)".into(),
            |tokens| css_color(tokens.overlay_backdrop),
        );
        let panel_border = tokens.map_or_else(
            || "rgba(127,212,255,0.55)".into(),
            |tokens| css_color(tokens.overlay_border),
        );
        let text = tokens.map_or_else(
            || "rgba(205,214,244,1.0)".into(),
            |tokens| css_color(tokens.text),
        );
        let muted = tokens.map_or_else(
            || "rgba(166,173,200,1.0)".into(),
            |tokens| css_color(tokens.text_subdued),
        );
        let ok = tokens.map_or_else(
            || "rgba(166,227,161,1.0)".into(),
            |tokens| css_color(tokens.success),
        );
        let caution = tokens.map_or_else(
            || "rgba(249,226,175,1.0)".into(),
            |tokens| css_color(tokens.warning),
        );
        let danger = tokens.map_or_else(
            || "rgba(243,139,168,1.0)".into(),
            |tokens| css_color(tokens.error),
        );
        let accent = "rgba(127,212,255,1.0)".to_string();

        let tilt_color = if v.tilt_deg >= v.danger_deg {
            &danger
        } else if v.tilt_deg >= v.caution_deg {
            &caution
        } else {
            &ok
        };
        let danger_width = (v.danger_deg - v.caution_deg).max(0.0) / 45.0 * 100.0;
        let limits = if v.limits_derived {
            format!("slip {:.0}° · tip {:.0}°", v.caution_deg, v.danger_deg)
        } else {
            "generic limits".into()
        };

        set_css_var(&mut style, "--panel-background", panel_background);
        set_css_var(&mut style, "--panel-border", panel_border);
        set_css_var(&mut style, "--text-color", text.clone());
        set_css_var(&mut style, "--muted-color", muted.clone());
        set_css_var(&mut style, "--accent-color", accent);
        set_css_var(&mut style, "--ok-color", ok.clone());
        set_css_var(&mut style, "--caution-color", caution.clone());
        set_css_var(&mut style, "--danger-color", danger.clone());
        set_css_var(&mut style, "--tilt-color", tilt_color.clone());
        set_css_var(
            &mut style,
            "--tilt-marker",
            percent(v.tilt_deg / 45.0 * 100.0),
        );
        set_css_var(
            &mut style,
            "--caution-width",
            percent(v.caution_deg / 45.0 * 100.0),
        );
        set_css_var(
            &mut style,
            "--danger-start",
            percent(v.caution_deg / 45.0 * 100.0),
        );
        set_css_var(&mut style, "--danger-width", percent(danger_width));
        set_css_var(
            &mut style,
            "--autopilot-display",
            if view.autopilot { "flex" } else { "none" },
        );
        set_property(&mut properties, "label", v.label.clone());
        set_property(&mut properties, "tilt", format!("{:.0}°", v.tilt_deg));
        set_property(&mut properties, "tilt_limits", limits);
        set_property(
            &mut properties,
            "tilt_status",
            if v.tilt_deg >= v.danger_deg {
                "DANGER"
            } else if v.tilt_deg >= v.caution_deg {
                "CAUTION"
            } else {
                "STABLE"
            },
        );
        set_property(
            &mut properties,
            "speed",
            v.speed
                .map_or_else(|| "—".into(), |speed| format!("{speed:.1}")),
        );
        set_property(&mut properties, "altitude", format!("{:.1}", v.pos.y));
        set_property(&mut properties, "roll", format!("{:+.0}°", v.roll_deg));
        set_property(&mut properties, "pitch", format!("{:+.0}°", v.pitch_deg));
        set_property(&mut properties, "heading", format!("{:.0}°", v.heading_deg));

        if let Some(geo) = v.geo {
            set_css_var(&mut style, "--geo-display", "flex");
            set_css_var(&mut style, "--local-display", "none");
            let lat = if geo.lat_deg >= 0.0 { "N" } else { "S" };
            let lon = if geo.lon_deg >= 0.0 { "E" } else { "W" };
            set_property(
                &mut properties,
                "geographic",
                format!(
                    "{:.4}° {lat}  ·  {:.4}° {lon}",
                    geo.lat_deg.abs(),
                    geo.lon_deg.abs()
                ),
            );
        } else {
            set_css_var(&mut style, "--geo-display", "none");
            set_css_var(&mut style, "--local-display", "flex");
            set_property(
                &mut properties,
                "local_position",
                format!("E {:+.0}  ·  N {:+.0}", v.pos.x, -v.pos.z),
            );
        }

        let (
            comms_display,
            comms_status,
            comms_peer,
            comms_range,
            comms_elevation,
            comms_los_display,
            comms_color,
        ) = link_snapshot(v.link.as_ref(), &muted, &ok, &danger);
        set_css_var(&mut style, "--comms-display", comms_display);
        set_property(&mut properties, "comms_status", comms_status);
        set_property(&mut properties, "comms_peer", comms_peer);
        set_property(&mut properties, "comms_range", comms_range);
        set_property(&mut properties, "comms_elevation", comms_elevation);
        set_css_var(&mut style, "--comms-los-display", comms_los_display);
        set_css_var(&mut style, "--comms-color", comms_color);

        if let Some(energy) = &v.energy {
            let power_color = if energy.soc_pct > 30.0 {
                &ok
            } else if energy.soc_pct > 15.0 {
                &caution
            } else {
                &danger
            };
            let detail = energy.energy_wh.map_or_else(
                || format!("charge {:.1}%", energy.soc_pct),
                |wh| {
                    let energy_str = if wh >= 1000.0 {
                        format!("{:.2} kWh", wh / 1000.0)
                    } else {
                        format!("{:.0} Wh", wh)
                    };
                    let capacity = energy.capacity_wh.map_or_else(String::new, |cap| {
                        if cap >= 1000.0 {
                            format!(" · cap {:.1} kWh", cap / 1000.0)
                        } else {
                            format!(" · cap {:.0} Wh", cap)
                        }
                    });
                    format!("{energy_str}{capacity}")
                },
            );
            set_css_var(&mut style, "--power-display", "flex");
            set_css_var(&mut style, "--power-color", power_color.clone());
            set_property(
                &mut properties,
                "power_value",
                format!("{:.0}%", energy.soc_pct),
            );
            set_property(&mut properties, "power_detail", detail);
        } else {
            set_css_var(&mut style, "--power-display", "none");
        }

        if let Some(thermal) = &v.thermal {
            let max_temp_k = thermal.max_temp_k();
            let thermal_color = if max_temp_k > 350.0 {
                &danger
            } else if max_temp_k > 310.0 {
                &caution
            } else {
                &ok
            };
            let detail = match (thermal.temp_left_k, thermal.temp_right_k) {
                (Some(left), Some(right)) => format!("L {:.0} K  ·  R {:.0} K", left, right),
                _ => String::new(),
            };
            set_css_var(&mut style, "--thermal-display", "flex");
            set_css_var(&mut style, "--thermal-color", thermal_color.clone());
            set_property(
                &mut properties,
                "thermal_value",
                format!("{:.0}°C", max_temp_k - 273.15),
            );
            set_property(&mut properties, "thermal_detail", detail);
        } else {
            set_css_var(&mut style, "--thermal-display", "none");
        }

        if &style != existing_style {
            commands.entity(entity).insert(style);
        }
        if properties.0 != previous_properties {
            commands.trigger(CompileContextEvent { entity });
        }
    }
}
