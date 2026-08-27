//! Engine-side producers for the generic exposure registry.
//!
//! This module has no HTML, egui, or Flair dependency. It resolves authoritative
//! engine state for the currently possessed vessel and publishes a named
//! capability snapshot through `lunco_core::exposure::EngineExposures`. Any
//! consumer can read that snapshot: runtime HTML, egui, API, telemetry, or a
//! remote client. The producer is scheduled with the simulation, not with a
//! particular presentation surface.
//!
//! Continuous sources are invalidated by Bevy change ticks and coalesced to the
//! bounded exposure cadence. Static or paused scenes do not repeat the expensive
//! resolution work.

use avian3d::prelude::{AngularVelocity, ComputedCenterOfMass, LinearVelocity, Rotation};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_autopilot::Autopilot;
use lunco_celestial::link::LinkState;
use lunco_celestial::OrbitalViewPin;
use lunco_controller::ControllerLink;
use lunco_core::exposure::{EngineExposures, ExposureRefresh, ExposureWriter, EXPOSURE_UPDATE_HZ};
use lunco_core::{Avatar, CelestialBody, GlobalEntityId, LocalAvatar, TheLocalAvatar};
use lunco_cosim::SimComponent;
use lunco_mobility::WheelRaycast;
use lunco_scene_commands::SelectedEntities;
use lunco_signal::{SignalRef, SignalRegistry, SignalType};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdRead};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Optional progress resources projected into generic runtime surfaces.
///
/// The values stay domain-neutral after this boundary: HUI and egui consumers
/// receive the same named snapshot, while the terrain/networking crates retain
/// ownership of how progress is calculated.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RuntimeOverlayInputs<'w> {
    terrain: Option<Res<'w, lunco_terrain_surface::TerrainGenStatus>>,
    overlay: Option<Res<'w, lunco_terrain_surface::overlay::TerrainOverlayParams>>,
    #[cfg(feature = "networking")]
    scenario: Option<Res<'w, lunco_networking::scenario_sync::ScenarioDownloadStatus>>,
}

/// The small amount of edge state needed for seminar-grade runtime evidence.
///
/// These are event logs, not a second domain state store: the authoritative
/// vessel pose, terrain oracle, battery outputs, and overlay resource remain
/// owned by their existing systems.
#[derive(Default)]
pub(crate) struct SeminarExposureTrace {
    current_vessel: Option<Entity>,
    last_label: Option<String>,
    tipped: bool,
    max_slope_deg: Option<f32>,
    overlay: Option<lunco_terrain_surface::overlay::TerrainOverlayParams>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SeminarTraceInputs<'w, 's> {
    surface: lunco_terrain_surface::GridSurfaceQuery<'w, 's>,
    prims: Query<'w, 's, &'static lunco_usd::UsdPrimPath>,
    provenance: Query<'w, 's, &'static lunco_core::Provenance>,
}

/// Fallback amber threshold, for a vessel whose limits cannot be derived.
///
/// GENERIC on purpose: the real roll-over angle is `atan(half_track / com_height)`
/// and the real slip limit is `atan(μ)`, both properties of the AUTHORED vehicle.
/// A driven rover publishes exactly those through the generic exposure registry,
/// and the HTML HUD prefers them — see
/// `docs/architecture/58-vessel-envelope-and-routes.md`. These remain for the
/// unknown-vehicle case (a lander, a wheel-less body), where they are
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

/// One authored operator channel retained by the shared telemetry registry.
///
/// The producer intentionally carries no electrical, thermal, hydraulic, or
/// other domain vocabulary. A declaration participates in this compact surface
/// only when its authored USD prim has the standard `ui:displayName`; that
/// existing authoring field is the explicit operator-view membership and label.
/// The full public telemetry catalog remains available to the telemetry browser
/// and API whether or not a channel is promoted to this surface.
#[derive(Debug, Clone, PartialEq)]
struct PublicTelemetryValue {
    label: String,
    value: f64,
    unit: Option<String>,
}

/// What the HUD needs about the driven vessel, resolved at the bounded exposure
/// cadence after authoritative inputs change.
#[derive(PartialEq)]
struct DrivenVessel {
    entity: Entity,
    label: String,
    /// Explicit coordinate mode. A site scene never falls back to root-world
    /// coordinates when its frame is missing or ambiguous.
    pose: DrivenVesselPose,
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

/// Coordinate ownership for one driven vessel.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DrivenVesselPose {
    /// Canonical site/body-fixed coordinates in a site-anchored scene.
    Surface(lunco_celestial::SurfacePose),
    /// Explicit non-celestial sandbox coordinates.
    World {
        position: lunco_core::coords::GridPos,
        rotation: lunco_core::coords::GridRot,
    },
}

impl DrivenVesselPose {
    fn display_position(self) -> DVec3 {
        match self {
            Self::Surface(pose) => pose.site_position.0,
            Self::World { position, .. } => position.0,
        }
    }

    fn display_rotation(self) -> DQuat {
        match self {
            Self::Surface(pose) => pose.site_rotation,
            Self::World { rotation, .. } => rotation.0,
        }
    }

    fn geodetic(self) -> Option<lunco_celestial::Geodetic> {
        match self {
            Self::Surface(pose) => Some(pose.geodetic),
            Self::World { .. } => None,
        }
    }

    fn altitude(self) -> f64 {
        self.geodetic()
            .map_or_else(|| self.display_position().y, |geo| geo.height_m)
    }
}

/// The optional geographic datum is one system parameter so the HUD remains
/// below Bevy's flat system-parameter limit while retaining the authored site
/// coordinate readout.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GeodeticHud<'w, 's> {
    surface_pose: lunco_celestial::SurfacePoseQuery<'w, 's>,
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
#[derive(PartialEq)]
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

fn resolve_authored_telemetry(
    vessel: Entity,
    signals: &SignalRegistry,
    q_parents: &Query<&ChildOf>,
    q_channels: &Query<(
        Entity,
        &lunco_core::telemetry::Parameter,
        Option<&lunco_core::markers::Callsign>,
    )>,
) -> Vec<PublicTelemetryValue> {
    // `Parameter` is the existing authored recording declaration. It is the
    // complete, domain-neutral boundary for USD telemetry channels and for
    // channels authored through the generic command/script API. Modelica's
    // runtime catalog is intentionally not included: it is inspection state
    // until an author promotes a value through a `Parameter` declaration.
    //
    // The registry is still the sole value/history source. This query only
    // selects which authored declarations belong to the driven vessel; it
    // never reads a domain output map or interprets a producer name.
    let mut seen = HashSet::new();
    let mut values = q_channels
        .iter()
        .filter_map(|(channel_entity, parameter, callsign)| {
            let callsign = callsign?;
            if !parameter.enabled || parameter.name.is_empty() {
                return None;
            }
            let measured = parameter.target.unwrap_or(channel_entity);
            if !is_owned_by_vessel(measured, vessel, q_parents)
                || !seen.insert((measured, parameter.name.clone()))
            {
                return None;
            }
            let signal = SignalRef::new(measured, parameter.name.clone());
            if signals.signal_type(&signal) != Some(SignalType::Scalar)
                || !signals.is_active(&signal)
            {
                return None;
            }
            let value = signals
                .scalar_history(&signal)
                .and_then(|history| history.samples.back())?
                .value;
            let unit = (!parameter.unit.is_empty())
                .then_some(parameter.unit.clone())
                .or_else(|| signals.meta(&signal).and_then(|meta| meta.unit.clone()));
            Some(PublicTelemetryValue {
                // `ui:displayName` is the standard USD human-facing label and
                // is projected onto the channel as Callsign. It also makes
                // membership in this deliberately compact operator view
                // explicit; a channel without it remains in the full catalog.
                label: callsign.0.clone(),
                value,
                unit,
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|a, b| a.label.cmp(&b.label));
    values
}

/// Resolve the vessel the local avatar is driving, or `None` in free flight.
fn resolve_driven(
    local_avatar: &TheLocalAvatar,
    q_avatar: &Query<&ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
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
    surface_pose: &lunco_celestial::SurfacePoseQuery,
) -> Option<DrivenVessel> {
    let vessel = q_avatar.get(local_avatar.0?).ok()?.vessel_entity;
    let pose = match surface_pose.site_count() {
        0 => {
            let (position, rotation) =
                lunco_core::coords::world_pose(vessel, q_parents, q_grids, q_spatial).ok()?;
            DrivenVesselPose::World { position, rotation }
        }
        1 => DrivenVesselPose::Surface(surface_pose.get(vessel)?),
        _ => return None,
    };
    let rot = pose.display_rotation().as_quat();

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
        pose,
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

#[cfg(test)]
mod exposure_tests {
    use super::*;

    #[test]
    fn link_snapshot_publishes_explicit_unavailable_state() {
        let values = link_snapshot(None, "muted", "ok", "danger");
        assert_eq!(values.0, "none");
        assert_eq!(values.1, "NO LINK");
        assert!(!values.2.is_empty());
        assert!(!values.3.is_empty());
        assert_eq!(values.4, "—");
        assert_eq!(values.5, "none");
        assert_eq!(values.6, "muted");

        let no_peers = LinkInfo {
            connected: false,
            peer_label: String::new(),
            range_m: 0.0,
            elevation_deg: None,
            no_peers: true,
        };
        let values = link_snapshot(Some(&no_peers), "muted", "ok", "danger");
        assert_eq!(values.1, "NO PEERS");
        assert!(!values.2.is_empty());
        assert!(!values.3.is_empty());
        assert_eq!(values.4, "—");
        assert_eq!(values.5, "none");
    }

    #[test]
    fn telemetry_summary_preserves_authored_labels_units_and_values() {
        let values = [
            PublicTelemetryValue {
                label: "power.battery_soc".into(),
                value: 100.0,
                unit: Some("%".into()),
            },
            PublicTelemetryValue {
                label: "power.battery_discharge".into(),
                value: 327.5,
                unit: Some("W".into()),
            },
        ];

        assert_eq!(
            format_telemetry_summary(&values),
            "power.battery_soc 100.0 % | power.battery_discharge 327.5 W"
        );
    }

    #[test]
    fn camera_exposure_projects_only_authoritative_camera_facts() {
        let status = lunco_usd_bevy::camera_switch::CameraSelectionStatus {
            cameras: vec!["/World/Wide".into(), "/World/Close".into()],
            active_name: Some("/World/Close".into()),
            owner: lunco_usd_bevy::camera_switch::CameraSelectionOwner::User,
            avatar_available: true,
            director_available: true,
            last_error: None,
        };
        let mut exposures = EngineExposures::default();
        publish_camera_exposure(&mut exposures, &status);
        let surface = exposures
            .surfaces
            .get("camera-status")
            .expect("camera status exposure");
        assert!(surface.visible);
        assert_eq!(surface.properties["active_name"].render(), "/World/Close");
        assert!(!surface.properties.contains_key("mode"));
        assert!(!surface.properties.contains_key("camera_count"));
        assert!(!surface.properties.contains_key("owner"));
        assert!(!surface.properties.contains_key("error"));
    }

    #[test]
    fn camera_status_event_updates_the_retained_exposure() {
        let mut app = App::new();
        app.init_resource::<EngineExposures>()
            .insert_resource(lunco_usd_bevy::camera_switch::CameraSelectionStatus {
                active_name: Some("/World/Close".into()),
                ..default()
            })
            .add_observer(on_camera_selection_status_changed);

        app.world_mut()
            .trigger(lunco_usd_bevy::camera_switch::CameraSelectionStatusChanged);

        let exposures = app.world().resource::<EngineExposures>();
        assert_eq!(
            exposures.surfaces["camera-status"].properties["active_name"].render(),
            "/World/Close"
        );
    }
}

/// Cheap reactive invalidation in front of the expensive vessel resolver.
///
/// The queries only inspect Bevy change ticks. Continuous motion still marks the
/// HUD dirty, but the publisher below coalesces those changes to the presentation
/// cadence. Static scenes, paused simulations, and idle frames do not rebuild the
/// view model.
pub(crate) fn mark_exposure_dirty(
    q_avatar: Query<
        (),
        (
            With<LocalAvatar>,
            Or<(
                Changed<ControllerLink>,
                Changed<Avatar>,
                Changed<LocalAvatar>,
            )>,
        ),
    >,
    q_velocity: Query<(), Changed<LinearVelocity>>,
    q_spatial: Query<(), Or<(Changed<CellCoord>, Changed<Transform>, Changed<ChildOf>)>>,
    q_links: Query<(), Changed<LinkState>>,
    q_wheels: Query<(), Or<(Changed<WheelRaycast>, Changed<Transform>)>>,
    q_com: Query<(), Changed<ComputedCenterOfMass>>,
    q_sim: Query<(), Changed<SimComponent>>,
    q_autopilot: Query<(), Changed<Autopilot>>,
    q_bodies: Query<(), Or<(Added<CelestialBody>, Changed<CelestialBody>)>>,
    selected: Res<SelectedEntities>,
    orbital_pin: Option<Res<OrbitalViewPin>>,
    overlays: RuntimeOverlayInputs,
    mut refresh: ResMut<ExposureRefresh>,
) {
    let changed = !q_avatar.is_empty()
        || !q_velocity.is_empty()
        || !q_spatial.is_empty()
        || !q_links.is_empty()
        || !q_wheels.is_empty()
        || !q_com.is_empty()
        || !q_sim.is_empty()
        || !q_autopilot.is_empty()
        || !q_bodies.is_empty()
        || selected.is_changed()
        || orbital_pin.is_some_and(|pin| pin.is_changed());

    let overlay_changed = overlays
        .terrain
        .as_ref()
        .is_some_and(|status| status.is_changed());
    let overlay_changed = overlay_changed
        || overlays
            .overlay
            .as_ref()
            .is_some_and(|params| params.is_changed());
    #[cfg(feature = "networking")]
    let overlay_changed = overlay_changed
        || overlays
            .scenario
            .as_ref()
            .is_some_and(|status| status.is_changed());

    if changed || overlay_changed {
        refresh.dirty = true;
    }
}

/// Publish a vessel exposure namespace. The engine resolves its authoritative
/// state here, then writes only generic named values; the runtime UI layer owns
/// all template and style mechanics.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ExposureRuntime<'w, 's> {
    time: Res<'w, Time>,
    timer: Local<'s, ExposureTimer>,
    refresh: ResMut<'w, ExposureRefresh>,
    exposures: ResMut<'w, EngineExposures>,
    signals: Res<'w, SignalRegistry>,
    selected: Res<'w, SelectedEntities>,
    local_avatar: Res<'w, TheLocalAvatar>,
    bodies: Query<'w, 's, &'static CelestialBody>,
    angular_velocity: Query<'w, 's, &'static AngularVelocity>,
    rotation: Query<'w, 's, &'static Rotation>,
    orbital_pin: Option<Res<'w, OrbitalViewPin>>,
    canonical: NonSend<'w, CanonicalStages>,
}

/// Authoritative inputs for the driven-vessel projection.
///
/// Bevy's function-system adapter has a bounded number of direct system
/// parameters. Keeping these queries in one `SystemParam` preserves the
/// ownership boundary without making the publisher an unregistered plain
/// function when seminar tracing adds another input.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ExposureQueries<'w, 's> {
    avatar: Query<'w, 's, &'static ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
    name: Query<'w, 's, &'static Name>,
    callsign: Query<'w, 's, &'static lunco_core::markers::Callsign>,
    gid: Query<'w, 's, &'static GlobalEntityId>,
    velocity: Query<'w, 's, &'static LinearVelocity>,
    parents: Query<'w, 's, &'static ChildOf>,
    grids: Query<'w, 's, &'static Grid>,
    spatial: Query<'w, 's, (Option<&'static CellCoord>, &'static Transform)>,
    links: Query<'w, 's, (Entity, &'static LinkState)>,
    ids: Query<'w, 's, (Entity, &'static GlobalEntityId)>,
    wheels: Query<'w, 's, (Entity, &'static WheelRaycast, &'static Transform)>,
    com: Query<'w, 's, &'static ComputedCenterOfMass>,
    sim: Query<'w, 's, (Entity, &'static SimComponent)>,
    channels: Query<
        'w,
        's,
        (
            Entity,
            &'static lunco_core::telemetry::Parameter,
            Option<&'static lunco_core::markers::Callsign>,
        ),
    >,
    usd_paths: Query<'w, 's, (Entity, &'static lunco_usd::UsdPrimPath)>,
}

pub(crate) fn publish_exposure(
    queries: ExposureQueries,
    geo: GeodeticHud,
    mut runtime: ExposureRuntime,
    overlays: RuntimeOverlayInputs,
    trace_inputs: SeminarTraceInputs,
    mut seminar: Local<SeminarExposureTrace>,
) {
    if let Some(overlay) = overlays.overlay.as_deref() {
        if seminar.overlay != Some(*overlay) {
            info!(
                "[seminar] terrain overlay: enabled={} mode={} safe={:.1}° cliff={:.1}° opacity={:.2}",
                overlay.enabled,
                if overlay.lod_depth { "lod" } else { "slope" },
                overlay.safe_deg,
                overlay.cliff_deg,
                overlay.opacity,
            );
            seminar.overlay = Some(*overlay);
        }
    }

    let timer_finished = runtime.timer.0.tick(runtime.time.delta()).just_finished();
    if !runtime.refresh.dirty || (!timer_finished && !runtime.refresh.first_update) {
        return;
    }
    runtime.refresh.dirty = false;
    runtime.refresh.first_update = false;

    let Some(vessel) = resolve_driven(
        &runtime.local_avatar,
        &queries.avatar,
        &queries.name,
        &queries.callsign,
        &queries.gid,
        &queries.velocity,
        &queries.parents,
        &queries.grids,
        &queries.spatial,
        &queries.links,
        &queries.ids,
        &queries.wheels,
        &queries.com,
        &geo.surface_pose,
    ) else {
        seminar.current_vessel = None;
        seminar.last_label = None;
        seminar.tipped = false;
        seminar.max_slope_deg = None;
        {
            let mut ui = runtime.exposures.writer("driven-vessel");
            ui.visible(false);
        }
        publish_lunica_schema_exposure(
            &mut runtime.exposures,
            &runtime.selected,
            &queries.usd_paths,
            &runtime.canonical,
        );
        publish_control_exposures(
            &mut runtime.exposures,
            &queries.name,
            &queries.callsign,
            &queries.sim,
            &queries.parents,
            &queries.grids,
            &queries.velocity,
            &runtime.angular_velocity,
            &runtime.rotation,
            &queries.spatial,
            &queries.usd_paths,
            &runtime.canonical,
        );
        publish_celestial_capability(
            &mut runtime.exposures,
            &runtime.bodies,
            runtime.orbital_pin.as_deref(),
        );
        publish_runtime_overlay_exposures(&mut runtime.exposures, &overlays);
        return;
    };

    if seminar.current_vessel != Some(vessel.entity) {
        seminar.current_vessel = Some(vessel.entity);
        seminar.last_label = Some(vessel.label.clone());
        seminar.tipped = false;
        seminar.max_slope_deg = None;
        let prim = trace_inputs
            .prims
            .get(vessel.entity)
            .map(|prim| prim.path.as_str())
            .unwrap_or("<no USD prim>");
        let provenance = trace_inputs
            .provenance
            .get(vessel.entity)
            .map(|value| format!("{value:?}"))
            .unwrap_or_else(|_| "<no provenance>".to_owned());
        info!(
            "[seminar] driven vessel resolved: label={} prim={} provenance={}",
            vessel.label, prim, provenance
        );
    }

    let tip_over = vessel.tilt_deg >= vessel.danger_deg;
    if tip_over != seminar.tipped {
        if tip_over {
            warn!(
                "[seminar] tip-over threshold crossed: vessel={} tilt={:.1}° limit={:.1}°",
                vessel.label, vessel.tilt_deg, vessel.danger_deg
            );
        } else {
            info!(
                "[seminar] tip-over threshold cleared: vessel={} tilt={:.1}° limit={:.1}°",
                vessel.label, vessel.tilt_deg, vessel.danger_deg
            );
        }
        seminar.tipped = tip_over;
    }

    if let Some(slope_deg) = trace_inputs
        .surface
        .slope_at(
            lunco_core::coords::GridPos(vessel.pose.display_position()),
            1.0,
        )
        .map(|slope| slope.to_degrees() as f32)
    {
        let new_max = seminar
            .max_slope_deg
            .is_none_or(|previous| slope_deg > previous + 0.5);
        if new_max {
            seminar.max_slope_deg = Some(slope_deg);
            info!(
                "[seminar] terrain slope: vessel={} instantaneous={:.1}° maximum={:.1}°",
                vessel.label, slope_deg, slope_deg
            );
        }
    }

    let autopilot = geo
        .autopilots
        .iter()
        .any(|pilot| pilot.vessel == vessel.entity && pilot.engaged);
    {
        let mut ui = runtime.exposures.writer("driven-vessel");
        ui.visible(true);
        let telemetry = resolve_authored_telemetry(
            vessel.entity,
            &runtime.signals,
            &queries.parents,
            &queries.channels,
        );
        publish_vessel_values(&mut ui, &vessel, autopilot, &telemetry);
    }
    publish_lunica_schema_exposure(
        &mut runtime.exposures,
        &runtime.selected,
        &queries.usd_paths,
        &runtime.canonical,
    );
    publish_control_exposures(
        &mut runtime.exposures,
        &queries.name,
        &queries.callsign,
        &queries.sim,
        &queries.parents,
        &queries.grids,
        &queries.velocity,
        &runtime.angular_velocity,
        &runtime.rotation,
        &queries.spatial,
        &queries.usd_paths,
        &runtime.canonical,
    );
    publish_celestial_capability(
        &mut runtime.exposures,
        &runtime.bodies,
        runtime.orbital_pin.as_deref(),
    );
    publish_runtime_overlay_exposures(&mut runtime.exposures, &overlays);
}

struct ExposureTimer(Timer);

impl Default for ExposureTimer {
    fn default() -> Self {
        Self(Timer::from_seconds(
            1.0 / EXPOSURE_UPDATE_HZ,
            TimerMode::Repeating,
        ))
    }
}

fn publish_celestial_capability(
    exposures: &mut EngineExposures,
    bodies: &Query<&CelestialBody>,
    orbital_pin: Option<&OrbitalViewPin>,
) {
    let mut moon = false;
    let mut earth = false;
    for body in bodies.iter() {
        match body.ephemeris_id {
            301 => moon = true,
            399 => earth = true,
            _ => {}
        }
    }

    let active_body = orbital_pin.filter(|pin| pin.active).map(|pin| pin.body);
    let mut ui = exposures.writer("celestial-view");
    ui.visible(moon || earth);
    ui.property("body_moon_present", moon);
    ui.property("body_earth_present", earth);
    ui.property("active_body_id", f64::from(active_body.unwrap_or_default()));
}

/// Publish the authored flight-control summary for a selected schema root.
///
/// This is the recorder-safe counterpart of the workbench connection canvas.
/// It reads the composed stage, honours only typed schemaNode/schemaColumn/
/// schemaRow/schemaRole properties, and derives the signal list from real USD
/// attribute connections. The surface is deliberately presentation-only: it
/// cannot affect simulation state and it does not classify prims from paths.
fn publish_lunica_schema_exposure(
    exposures: &mut EngineExposures,
    selected: &SelectedEntities,
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) -> bool {
    let mut ui = exposures.writer("lunica-schema");
    ui.visible(false);
    ui.property("title", "FLIGHT CONTROL / CONNECTIONS");
    ui.property("scope", "Select an authored schema root");
    ui.property("count", "0 authored blocks");
    ui.property("note", "Typed USD nodes and their authored connections");
    for column in 0..4 {
        for row in 0..2 {
            ui.property(format!("slot_{column}_{row}_title"), "");
            ui.property(format!("slot_{column}_{row}_role"), "");
            ui.property(format!("slot_{column}_{row}_visible"), false);
        }
    }
    for index in 0..8 {
        ui.property(format!("wire_{index}"), "");
        ui.property(format!("wire_{index}_visible"), false);
    }

    let Some(root) = selected.primary() else {
        return false;
    };
    let Ok((_, root_path)) = q_paths.get(root) else {
        return false;
    };
    let Some(stage) = canonical.get(root_path.stage_handle.id()) else {
        return false;
    };
    let Ok(root_sdf) = SdfPath::new(&root_path.path) else {
        return false;
    };
    let view = stage.view();
    if view.boolean(&root_sdf, "lunco:ui:schemaRoot") != Some(true) {
        return false;
    }

    #[derive(Clone)]
    struct SchemaCard {
        path: String,
        title: String,
        role: String,
        column: i32,
        row: i32,
    }

    let root_prefix = format!("{}/", root_path.path.trim_end_matches('/'));
    let mut cards = Vec::new();
    for path in view.prim_paths() {
        let path_text = path.to_string();
        if path_text != root_path.path && !path_text.starts_with(&root_prefix) {
            continue;
        }
        if !view.is_active(&path) || view.boolean(&path, "lunco:ui:schemaNode") != Some(true) {
            continue;
        }
        let title = view
            .text(&path, "ui:displayName")
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                path_text
                    .rsplit('/')
                    .next()
                    .filter(|leaf| !leaf.is_empty())
                    .unwrap_or("USD block")
                    .to_owned()
            });
        cards.push(SchemaCard {
            path: path_text,
            title,
            role: view
                .text(&path, "lunco:ui:schemaRole")
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| "Connected USD block".to_owned()),
            column: view
                .scalar::<i32>(&path, "lunco:ui:schemaColumn")
                .unwrap_or(0),
            row: view.scalar::<i32>(&path, "lunco:ui:schemaRow").unwrap_or(0),
        });
    }
    cards.sort_by_key(|card| (card.column, card.row, card.path.clone()));
    if cards.is_empty() {
        return false;
    }

    let card_names: HashMap<String, String> = cards
        .iter()
        .map(|card| (card.path.clone(), card.title.clone()))
        .collect();
    for card in &cards {
        if (0..4).contains(&card.column) && (0..2).contains(&card.row) {
            ui.property(
                format!("slot_{}_{}_title", card.column, card.row),
                card.title.clone(),
            );
            ui.property(
                format!("slot_{}_{}_role", card.column, card.row),
                card.role.clone(),
            );
            ui.property(format!("slot_{}_{}_visible", card.column, card.row), true);
        }
    }

    let mut wires = BTreeSet::new();
    for card in &cards {
        let Ok(target) = SdfPath::new(&card.path) else {
            continue;
        };
        for attr in view.attr_names(&target) {
            if !attr.starts_with("inputs:") {
                continue;
            }
            for source in view.connections(&target, &attr) {
                let Some((source_prim, _)) = source.rsplit_once('.') else {
                    continue;
                };
                let Some(source_name) = card_names.get(source_prim) else {
                    continue;
                };
                wires.insert(format!("{source_name}  →  {}", card.title));
            }
        }
    }

    ui.visible(true);
    ui.property(
        "scope",
        root_path.path.rsplit('/').next().unwrap_or("schema"),
    );
    ui.property("count", format!("{} authored blocks", cards.len()));
    ui.property(
        "note",
        "Signals are the live USD connections between these blocks",
    );
    for (index, wire) in wires.into_iter().take(8).enumerate() {
        ui.property(format!("wire_{index}"), wire);
        ui.property(format!("wire_{index}_visible"), true);
    }
    true
}

/// Publish control responses for explicitly authored runtime surfaces.
///
/// A selected entity is not enough to opt into the compact lander card: the
/// surface is a scene-authored presentation contract. This keeps a rover
/// selection from accidentally publishing a lander-specific HUD.
fn publish_control_exposures(
    exposures: &mut EngineExposures,
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_vel: &Query<&LinearVelocity>,
    q_angvel: &Query<&AngularVelocity>,
    q_rotation: &Query<&Rotation>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) {
    let mut roots = authored_control_roots(q_paths, canonical);
    roots.sort_by_key(|(_, column)| *column);

    publish_selected_control_exposure(
        exposures,
        "lander-control-0",
        roots.first().and_then(|(entity, _)| *entity),
        q_name,
        q_callsign,
        q_sim,
        q_parents,
        q_grids,
        q_vel,
        q_angvel,
        q_rotation,
        q_spatial,
        q_paths,
        canonical,
    );
    publish_selected_control_exposure(
        exposures,
        "lander-control-1",
        roots.get(1).and_then(|(entity, _)| *entity),
        q_name,
        q_callsign,
        q_sim,
        q_parents,
        q_grids,
        q_vel,
        q_angvel,
        q_rotation,
        q_spatial,
        q_paths,
        canonical,
    );
}

/// Discover the roots that explicitly opt into a compact control card.
///
/// The roots are authored USD data, not a list of vehicle paths in Rust.  The
/// column is likewise an authored presentation hint, so a scene can place a
/// pair of cards without teaching the engine what a particular film calls its
/// vehicles.
fn authored_control_roots(
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) -> Vec<(Option<Entity>, i32)> {
    let mut roots = Vec::new();
    for (entity, prim_path) in q_paths.iter() {
        let Some(stage) = canonical.get(prim_path.stage_handle.id()) else {
            continue;
        };
        let Ok(path) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        let view = stage.view();
        if view.boolean(&path, "lunco:ui:controlHud") != Some(true) {
            continue;
        }
        let column = view
            .scalar::<i32>(&path, "lunco:ui:controlHudColumn")
            .unwrap_or(0);
        roots.push((Some(entity), column));
    }
    roots
}

/// Resolve the authored target and both entities' absolute positions. This keeps
/// the HUD honest across BigSpace cells, parent hierarchies, and floating-origin
/// recentering even when the generated GNC model does not expose estimator error
/// channels.
fn authored_target_positions(
    root: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) -> Option<(lunco_core::coords::GridPos, lunco_core::coords::GridPos)> {
    let (_, root_path) = q_paths.get(root).ok()?;
    let stage = canonical.get(root_path.stage_handle.id())?;
    let view = stage.view();
    let root_prefix = format!("{}/", root_path.path.trim_end_matches('/'));

    // The guidance boundary is selected by its authored schema column. Its
    // target is then read from the real USD connection, so neither a vehicle
    // path nor a target name is embedded in the producer.
    let mut guidance_paths: Vec<_> = view
        .prim_paths()
        .into_iter()
        .filter(|path| {
            let path_text = path.to_string();
            path_text.starts_with(&root_prefix)
                && view.is_active(path)
                && view.boolean(path, "lunco:ui:schemaNode") == Some(true)
                && view.scalar::<i32>(path, "lunco:ui:schemaColumn") == Some(0)
        })
        .collect();
    guidance_paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let guidance = match guidance_paths.as_slice() {
        [guidance] => guidance,
        [] => return None,
        _ => {
            warn!(
                "[control-hud] multiple schema column-0 guidance nodes under {}; target is ambiguous",
                root_path.path
            );
            return None;
        }
    };
    let target_source = view
        .connections(guidance, "inputs:target_x")
        .into_iter()
        .next()?;
    let (target_path, _) = target_source.rsplit_once('.')?;
    let mut target_entities = q_paths.iter().filter_map(|(entity, prim_path)| {
        (prim_path.stage_handle.id() == root_path.stage_handle.id()
            && prim_path.path == target_path)
            .then_some(entity)
    });
    let target_entity = target_entities.next()?;
    if target_entities.next().is_some() {
        warn!(
            "[control-hud] multiple ECS entities represent target {}; target is ambiguous",
            target_path
        );
        return None;
    }

    let root_position =
        lunco_core::coords::world_position(root, q_parents, q_grids, q_spatial).ok()?;
    let target_position =
        lunco_core::coords::world_position(target_entity, q_parents, q_grids, q_spatial).ok()?;
    Some((root_position, target_position))
}

fn publish_selected_control_exposure(
    exposures: &mut EngineExposures,
    namespace: &str,
    root: Option<Entity>,
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_vel: &Query<&LinearVelocity>,
    q_angvel: &Query<&AngularVelocity>,
    q_rotation: &Query<&Rotation>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) {
    let mut ui = exposures.writer(namespace);
    ui.visible(false);
    ui.property("vehicle", "No simulation selected");
    ui.property("status", "WAITING");
    ui.property("status_color", "var(--muted-color)");
    ui.property("ground_speed", "—");
    ui.property("lateral_speed", "—");
    ui.property("vertical_speed", "—");
    ui.property("vertical_direction", "—");
    ui.property("altitude", "—");
    ui.property("target_offset", "—");
    ui.property("predicted_impact", "—");
    ui.property("propellant", "—");
    ui.property("propellant_width", "0%");
    ui.property("roll", "—");
    ui.property("pitch", "—");
    ui.property("yaw", "—");
    ui.property("spin", "—");
    ui.property("rcs_activity", "0%");
    ui.property("rcs_activity_width", "0%");
    ui.property("rcs_axis", "OFF");
    ui.property("torque_x", "—");
    ui.property("torque_y", "—");
    ui.property("torque_z", "—");
    ui.property("main_engine", "—");
    ui.property("main_engine_width", "0%");
    ui.property("main_engine_color", "var(--muted-color)");

    let Some(root) = root else {
        return;
    };

    // The surface footprint is part of the recording composition. Mount it as
    // soon as the authored HUD root exists, before Modelica has published its
    // first values, so solver readiness cannot cause a large mid-shot layout pop.
    ui.visible(true);

    let vehicle = q_callsign
        .get(root)
        .ok()
        .map(|callsign| callsign.0.trim().to_owned())
        .filter(|label| !label.is_empty())
        .or_else(|| {
            q_name.get(root).ok().map(|name| {
                name.as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or("selected")
                    .to_owned()
            })
        })
        .unwrap_or_else(|| "selected".to_owned());
    ui.property("vehicle", vehicle.clone());
    ui.property("status", "INITIALIZING");

    let mut outputs = std::collections::HashMap::<String, f64>::new();
    let mut max_valve = 0.0_f64;
    let mut touchdown = 0.0_f64;
    for (entity, sim) in q_sim.iter() {
        if !is_owned_by_vessel(entity, root, q_parents) {
            continue;
        }
        let authored_outputs = authored_output_names(entity, q_paths, canonical);
        for (name, &value) in &sim.outputs {
            let is_public = authored_outputs
                .as_ref()
                .is_none_or(|authored| authored.contains(name));
            if !is_public {
                continue;
            }
            // Prefer the selected prim's own public output when a generated
            // wrapper also republishes the same name below it.
            if entity == root || !outputs.contains_key(name) {
                outputs.insert(name.clone(), value);
            }
            if name.ends_with("_valve") {
                max_valve = max_valve.max(value.clamp(0.0, 1.0));
            }
        }
        if let Some(&value) = sim.outputs.get("touchdown") {
            touchdown = touchdown.max(value);
        }
    }

    let has_control_outputs = outputs.contains_key("torque_x")
        || outputs.contains_key("torque_y")
        || outputs.contains_key("torque_z")
        || outputs.contains_key("engine_activity")
        || outputs.contains_key("thrust_n")
        || outputs.contains_key("propellant_mass")
        || max_valve > 0.0;
    if !has_control_outputs {
        return;
    }
    let main_activity = engine_firing_fraction(&outputs);
    let landing_handoff = outputs.get("landing_handoff").copied();
    let (rcs_axis, rcs_peak) = rcs_axis_label(&outputs);
    let flight_handoff = landing_handoff.is_some_and(|value| value >= 0.5);
    let status = if touchdown >= 0.5 {
        ("TOUCHDOWN", "var(--ok-color)")
    } else if flight_handoff {
        ("GEAR SETTLING", "var(--ok-color)")
    } else if max_valve > 0.01 {
        ("RCS FIRING", "var(--accent-color)")
    } else if main_activity > 0.01 {
        ("ENGINE FIRING", "var(--warm-color)")
    } else {
        ("ATTITUDE HOLD", "var(--ok-color)")
    };

    let motion = q_vel.get(root).ok().map(|velocity| {
        let v = velocity.0;
        let ground_speed = v.x.hypot(v.z);
        (ground_speed, v.x, v.y)
    });
    let attitude = q_rotation.get(root).ok().map(|rotation| {
        let (yaw, pitch, roll) = rotation.0.to_euler(EulerRot::YXZ);
        (roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees())
    });
    let spin = q_angvel.get(root).ok().map(|angular| angular.0.length());
    // A tipped lander can have a valid-looking zero range while its downward
    // beam has no usable surface return.  Exposing that as `0.0 m` makes the
    // HUD claim that the vehicle is on the ground during the attitude-recovery
    // phase.  Keep the sensor contract honest: a range is displayable only
    // when the Modelica altimeter reports usable vertical confidence.
    let altitude = outputs
        .get("range_m")
        .copied()
        .zip(outputs.get("range_confidence").copied())
        .and_then(|(range, confidence)| (confidence >= 0.5).then_some(range));
    let target_positions =
        authored_target_positions(root, q_parents, q_grids, q_spatial, q_paths, canonical);
    let target_offset_xy = target_positions.map(|(root_position, target_position)| {
        let offset = root_position.0 - target_position.0;
        (offset.x, offset.z)
    });
    let target_offset = target_offset_xy.map_or_else(
        || "—".to_owned(),
        |(x, z)| format!("X {x:+.1} · Z {z:+.1} m"),
    );
    let predicted_impact = match (
        outputs.get("predicted_landing_x"),
        outputs.get("predicted_landing_z"),
        outputs.get("predicted_landing_time"),
        target_positions,
    ) {
        (Some(&x), Some(&z), Some(&time), Some((_, target_position))) => {
            let target_x = target_position.0.x;
            let target_z = target_position.0.z;
            format!(
                "X {:+.1} · Z {:+.1} m / {time:.1}s",
                x - target_x,
                z - target_z,
            )
        }
        _ => "—".to_owned(),
    };
    let propellant_mass = outputs.get("propellant_mass").copied();
    let propellant_fraction = outputs
        .get("propellant_fraction")
        .copied()
        .map(|value| value.clamp(0.0, 1.0));

    ui.property("vehicle", vehicle);
    ui.property("status", status.0);
    ui.property("status_color", status.1);
    ui.property(
        "ground_speed",
        motion.map_or_else(|| "—".to_owned(), |(speed, _, _)| format!("{speed:.2} m/s")),
    );
    ui.property(
        "lateral_speed",
        motion.map_or_else(
            || "—".to_owned(),
            |(_, speed, _)| format!("{speed:+.2} m/s"),
        ),
    );
    ui.property(
        "vertical_speed",
        motion.map_or_else(
            || "—".to_owned(),
            // The model keeps the signed world-frame velocity.  The film-facing
            // card reports a rate magnitude and puts the sign into the adjacent
            // direction word, so "DOWN 1.82 m/s" is immediately readable.
            |(_, _, speed)| format!("{:.2} m/s", speed.abs()),
        ),
    );
    ui.property(
        "vertical_direction",
        motion.map_or_else(
            || "—".to_owned(),
            |(_, _, speed)| {
                if speed < -0.01 {
                    "DOWN".to_owned()
                } else if speed > 0.01 {
                    "UP".to_owned()
                } else {
                    "HOLD".to_owned()
                }
            },
        ),
    );
    ui.property(
        "altitude",
        altitude.map_or_else(|| "NO LOCK".to_owned(), |value| format!("{value:.1} m")),
    );
    ui.property("target_offset", target_offset);
    ui.property("predicted_impact", predicted_impact);
    ui.property(
        "propellant",
        match (propellant_fraction, propellant_mass) {
            (Some(fraction), Some(mass)) => format!("{:.0}% · {:.0} kg", fraction * 100.0, mass),
            (Some(fraction), None) => format!("{:.0}%", fraction * 100.0),
            (None, Some(mass)) => format!("{mass:.0} kg"),
            (None, None) => "—".to_owned(),
        },
    );
    ui.property(
        "propellant_width",
        propellant_fraction
            .map_or_else(|| "0%".to_owned(), |value| format!("{:.1}%", value * 100.0)),
    );
    ui.property(
        "roll",
        attitude.map_or_else(|| "—".to_owned(), |(roll, _, _)| format!("{roll:+.0}°")),
    );
    ui.property(
        "pitch",
        attitude.map_or_else(|| "—".to_owned(), |(_, pitch, _)| format!("{pitch:+.0}°")),
    );
    ui.property(
        "yaw",
        attitude.map_or_else(|| "—".to_owned(), |(_, _, yaw)| format!("{yaw:+.0}°")),
    );
    ui.property(
        "spin",
        spin.map_or_else(|| "—".to_owned(), |value| format!("{value:.2} rad/s")),
    );
    ui.property("rcs_activity", format!("{:.0}%", rcs_peak * 100.0));
    ui.property("rcs_activity_width", format!("{:.1}%", rcs_peak * 100.0));
    ui.property("rcs_axis", rcs_axis);
    ui.property(
        "torque_x",
        outputs
            .get("torque_x")
            .map_or_else(|| "—".to_owned(), |value| format!("{value:+.0} N·m")),
    );
    ui.property(
        "torque_y",
        outputs
            .get("torque_y")
            .map_or_else(|| "—".to_owned(), |value| format!("{value:+.0} N·m")),
    );
    ui.property(
        "torque_z",
        outputs
            .get("torque_z")
            .map_or_else(|| "—".to_owned(), |value| format!("{value:+.0} N·m")),
    );
    ui.property(
        "main_engine",
        if main_activity > 0.01 {
            format!("FIRING {:.0}%", main_activity * 100.0)
        } else {
            "OFF".to_owned()
        },
    );
    ui.property(
        "main_engine_width",
        format!("{:.1}%", main_activity * 100.0),
    );
    ui.property(
        "main_engine_color",
        if main_activity > 0.01 {
            "var(--warm-color)"
        } else {
            "var(--muted-color)"
        },
    );
}

/// Names on the prim's composed public co-simulation boundary.
///
/// This is derived from authored USD properties, not from solver variable
/// spelling. Generated networks can expose hundreds of member values in their
/// runtime `SimComponent`; only the root `outputs:*` properties are the public
/// contract a scene author chose.
fn authored_output_names(
    entity: Entity,
    q_paths: &Query<(Entity, &lunco_usd::UsdPrimPath)>,
    canonical: &CanonicalStages,
) -> Option<std::collections::HashSet<String>> {
    let (_, prim_path) = q_paths.get(entity).ok()?;
    let stage = canonical.get(prim_path.stage_handle.id())?;
    let path = SdfPath::new(&prim_path.path).ok()?;
    let names = stage
        .view()
        .attr_names(&path)
        .into_iter()
        .filter_map(|name| name.strip_prefix("outputs:").map(str::to_owned))
        .collect::<std::collections::HashSet<_>>();
    (!names.is_empty()).then_some(names)
}

/// Delivered main-engine thrust fraction for operator display.
///
/// A chamber can still move a trace amount of propellant while producing no
/// useful force. `ENGINE FIRING` therefore reports the physically delivered
/// thrust fraction whenever the authored engine boundary provides it. Older or
/// non-thrust actuators retain their explicit activity output as the fallback.
fn engine_firing_fraction(outputs: &std::collections::HashMap<String, f64>) -> f64 {
    let fraction = outputs
        .get("thrust_n")
        .zip(outputs.get("maximum_thrust_n"))
        .filter(|(_, maximum)| maximum.is_finite() && **maximum > f64::EPSILON)
        .map(|(thrust, maximum)| thrust / maximum)
        .or_else(|| outputs.get("engine_activity").copied())
        .unwrap_or(0.0);
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// RCS valve names are the authored actuator contract of AttitudeActuation:
/// `pitch_pos_a_valve`, `roll_neg_b_valve`, and so on. This is classification
/// of public model outputs, not a vehicle-path or film-specific heuristic.
fn rcs_axis_label(outputs: &std::collections::HashMap<String, f64>) -> (String, f64) {
    let mut axes = [("PITCH", 0.0_f64, ""), ("ROLL", 0.0, ""), ("YAW", 0.0, "")];
    for (name, value) in outputs {
        if !name.ends_with("_valve") {
            continue;
        }
        let axis = if name.starts_with("pitch_") {
            Some(0)
        } else if name.starts_with("roll_") {
            Some(1)
        } else if name.starts_with("yaw_") {
            Some(2)
        } else {
            None
        };
        let Some(axis) = axis else { continue };
        let opening = value.clamp(0.0, 1.0);
        if opening <= axes[axis].1 {
            continue;
        }
        let direction = if name.contains("_pos_") {
            "+"
        } else if name.contains("_neg_") {
            "-"
        } else {
            ""
        };
        axes[axis].1 = opening;
        axes[axis].2 = direction;
    }

    let peak = axes.iter().map(|(_, value, _)| *value).fold(0.0, f64::max);
    let active = axes
        .iter()
        .filter(|(_, value, _)| *value > 0.01)
        .map(|(axis, _, direction)| format!("{axis} {direction}"))
        .collect::<Vec<_>>();
    if active.is_empty() {
        ("OFF".to_owned(), peak)
    } else {
        (active.join(" · "), peak)
    }
}

#[cfg(test)]
mod control_exposure_tests {
    use super::*;

    #[test]
    fn engine_display_uses_delivered_thrust_over_stale_activity() {
        let outputs = std::collections::HashMap::from([
            ("thrust_n".to_owned(), 0.0),
            ("maximum_thrust_n".to_owned(), 93_000.0),
            ("engine_activity".to_owned(), 0.07),
        ]);
        assert_eq!(engine_firing_fraction(&outputs), 0.0);
    }

    #[test]
    fn engine_display_falls_back_to_explicit_activity_without_thrust_contract() {
        let outputs = std::collections::HashMap::from([("engine_activity".to_owned(), 0.4)]);
        assert_eq!(engine_firing_fraction(&outputs), 0.4);
    }
}

fn publish_runtime_overlay_exposures(
    exposures: &mut EngineExposures,
    overlays: &RuntimeOverlayInputs,
) {
    {
        let mut terrain = exposures.writer("terrain-progress");
        let terrain_active = overlays
            .terrain
            .as_deref()
            .is_some_and(|status| status.active && !status.user_dismissed);
        terrain.visible(terrain_active);
        if let Some(status) = overlays.terrain.as_deref() {
            let title = if status.site.is_empty() {
                status.phase.label().to_owned()
            } else {
                format!("{} — {}", status.phase.label(), status.site)
            };
            terrain.property("title", title);
            terrain.property("caption", status.phase.caption());
            terrain.property(
                "progress_width",
                status.fraction.map_or_else(
                    || "0%".to_owned(),
                    |fraction| format!("{:.1}%", fraction * 100.0),
                ),
            );
            terrain.property(
                "progress_text",
                status.fraction.map_or_else(
                    || "working…".to_owned(),
                    |fraction| format!("{:.0}%", fraction * 100.0),
                ),
            );
        }
    }

    #[cfg(feature = "networking")]
    {
        let mut scenario = exposures.writer("scenario-download");
        let active = overlays
            .scenario
            .as_deref()
            .is_some_and(|status| status.active);
        scenario.visible(active);
        if let Some(status) = overlays.scenario.as_deref() {
            scenario.property(
                "title",
                if status.name.is_empty() {
                    "Downloading scenario".to_owned()
                } else {
                    format!("Downloading {}", status.name)
                },
            );
            scenario.property(
                "asset_count",
                format!("{} / {} assets", status.assets_done, status.assets_total),
            );
            scenario.property(
                "progress_width",
                status.fraction().map_or_else(
                    || "0%".to_owned(),
                    |fraction| format!("{:.1}%", fraction * 100.0),
                ),
            );
            scenario.property(
                "progress_text",
                format!(
                    "{:.1} / {:.1} MB",
                    status.bytes_done as f64 / (1024.0 * 1024.0),
                    status.bytes_total as f64 / (1024.0 * 1024.0)
                ),
            );
        }
    }
}

/// Publish the current camera fact at the lifecycle boundary that changed it.
/// This observer is deliberately separate from the continuous vessel exposure
/// cadence: a camera switch must not be rediscovered by a per-tick poll.
pub(crate) fn on_camera_selection_status_changed(
    _trigger: On<lunco_usd_bevy::camera_switch::CameraSelectionStatusChanged>,
    status: Res<lunco_usd_bevy::camera_switch::CameraSelectionStatus>,
    mut exposures: ResMut<EngineExposures>,
) {
    publish_camera_exposure(&mut exposures, &status);
}

/// Seed the retained camera surface once when the host starts. Subsequent
/// updates arrive only through `CameraSelectionStatusChanged`.
pub(crate) fn publish_initial_camera_exposure(
    status: Res<lunco_usd_bevy::camera_switch::CameraSelectionStatus>,
    mut exposures: ResMut<EngineExposures>,
) {
    publish_camera_exposure(&mut exposures, &status);
}

fn publish_camera_exposure(
    exposures: &mut EngineExposures,
    status: &lunco_usd_bevy::camera_switch::CameraSelectionStatus,
) {
    let mut ui = exposures.writer("camera-status");
    ui.visible(true);
    // This boundary publishes only the authoritative current-camera fact.
    // Selection policy and presentation wording stay in Rhai/HUI.
    ui.property("active_name", status.active_name.as_deref().unwrap_or(""));
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
            "NO LINK".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "none".into(),
            muted.to_string(),
        );
    };

    if link.no_peers {
        return (
            "flex",
            "NO PEERS".into(),
            "—".into(),
            "—".into(),
            "—".into(),
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

/// Publish the formatted values for the vessel exposure namespace.
///
/// This is the only domain-specific part of the first producer. It emits generic
/// properties and CSS state variables; no HUI, Flair, egui, or Bevy UI component
/// is touched here.
fn publish_vessel_values(
    ui: &mut ExposureWriter<'_>,
    v: &DrivenVessel,
    autopilot: bool,
    telemetry: &[PublicTelemetryValue],
) {
    let tilt_color = if v.tilt_deg >= v.danger_deg {
        "var(--danger-color)"
    } else if v.tilt_deg >= v.caution_deg {
        "var(--caution-color)"
    } else {
        "var(--ok-color)"
    };
    let danger_width = (v.danger_deg - v.caution_deg).max(0.0) / 45.0 * 100.0;
    let limits = if v.limits_derived {
        format!("slip {:.0}° · tip {:.0}°", v.caution_deg, v.danger_deg)
    } else {
        "generic limits".into()
    };

    ui.property("tilt_color", tilt_color);
    ui.property("tilt_marker", percent(v.tilt_deg / 45.0 * 100.0));
    ui.property("caution_width", percent(v.caution_deg / 45.0 * 100.0));
    ui.property("danger_start", percent(v.caution_deg / 45.0 * 100.0));
    ui.property("danger_width", percent(danger_width));
    ui.property(
        "autopilot_color",
        if autopilot {
            "var(--accent-color)"
        } else {
            "var(--muted-color)"
        },
    );
    ui.property(
        "autopilot_label",
        if autopilot {
            "AUTOPILOT ON"
        } else {
            "AUTOPILOT"
        },
    );
    ui.property("label", v.label.clone());
    ui.property("tilt", format!("{:.0}°", v.tilt_deg));
    ui.property("tilt_limits", limits);
    ui.property(
        "tilt_status",
        if v.tilt_deg >= v.danger_deg {
            "DANGER"
        } else if v.tilt_deg >= v.caution_deg {
            "CAUTION"
        } else {
            "STABLE"
        },
    );
    ui.property(
        "speed",
        v.speed
            .map_or_else(|| "—".into(), |speed| format!("{speed:.1}")),
    );
    ui.property("altitude", format!("{:.1}", v.pose.altitude()));
    ui.property("roll", format!("{:+.0}°", v.roll_deg));
    ui.property("pitch", format!("{:+.0}°", v.pitch_deg));
    ui.property("heading", format!("{:.0}°", v.heading_deg));

    if let Some(geo) = v.pose.geodetic() {
        ui.property("geo_display", "flex");
        ui.property("local_display", "none");
        let lat = if geo.lat_deg >= 0.0 { "N" } else { "S" };
        let lon = if geo.lon_deg >= 0.0 { "E" } else { "W" };
        ui.property(
            "geographic",
            format!(
                "{:.4}° {lat}  ·  {:.4}° {lon}",
                geo.lat_deg.abs(),
                geo.lon_deg.abs()
            ),
        );
    } else {
        ui.property("geo_display", "none");
        ui.property("local_display", "flex");
        ui.property(
            "local_position",
            format!(
                "E {:+.0}  ·  N {:+.0}",
                v.pose.display_position().x,
                -v.pose.display_position().z
            ),
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
    ) = link_snapshot(
        v.link.as_ref(),
        "var(--muted-color)",
        "var(--ok-color)",
        "var(--danger-color)",
    );
    ui.property("comms_display", comms_display);
    ui.property("comms_status", comms_status);
    ui.property("comms_peer", comms_peer);
    ui.property("comms_range", comms_range);
    ui.property("comms_elevation", comms_elevation);
    ui.property("comms_los_display", comms_los_display);
    ui.property("comms_color", comms_color);

    if telemetry.is_empty() {
        ui.property("telemetry_display", "none");
        ui.property("telemetry_summary", "TELEMETRY UNAVAILABLE");
    } else {
        ui.property("telemetry_display", "flex");
        ui.property("telemetry_summary", format_telemetry_summary(telemetry));
    }
}

fn format_telemetry_summary(values: &[PublicTelemetryValue]) -> String {
    values
        .iter()
        .map(|value| {
            let unit = value
                .unit
                .as_deref()
                .filter(|unit| !unit.is_empty())
                .map_or_else(String::new, |unit| format!(" {unit}"));
            format!("{} {:.1}{}", value.label, value.value, unit)
        })
        .collect::<Vec<_>>()
        .join(" | ")
}
