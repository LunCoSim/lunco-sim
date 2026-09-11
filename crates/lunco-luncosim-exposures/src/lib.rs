//! Runtime projection for the generic exposure registry.
//!
//! This module has no HTML, egui, or Flair dependency. It resolves generic
//! runtime state and authored telemetry, then publishes named capability
//! snapshots through `lunco_core::exposure::EngineExposures`. Any consumer can
//! read that snapshot: runtime HTML, egui, API, telemetry, or a remote client.
//! Domain values and transformations remain in their authored owners.
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
use lunco_core::exposure::{
    EngineExposures, ExposureRefresh, ExposureValue, ExposureWriter, EXPOSURE_UPDATE_HZ,
};
use lunco_core::{
    Avatar, CelestialBody, GlobalEntityId, LocalAvatar, SceneMountState, TheLocalAvatar,
};
use lunco_cosim::{SimComponent, SimStatus};
use lunco_hooks::HookValue;
use lunco_mobility::WheelRaycast;
use lunco_scene_commands::SelectedEntities;
use lunco_signal::{SignalRef, SignalRegistry, SignalType};
use lunco_usd_bevy_core::read::UsdReadObject;
use lunco_usd_bevy_core::{CanonicalStages, UsdStageAsset};
use lunco_usd_bevy_scene::scene_root_ancestor;
use openusd::sdf::Path as SdfPath;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

/// Registers the renderer-independent runtime exposure projection.
///
/// The application owns composition policy, while this crate owns the one
/// production projection from authoritative ECS/domain state to the shared
/// [`lunco_core::exposure::EngineExposures`] registry. Keeping that projection
/// behind a plugin prevents the application composition root from recompiling
/// when exposure logic changes and keeps the headless server on the same path.
pub struct RuntimeExposuresPlugin;

impl Plugin for RuntimeExposuresPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, publish_initial_camera_exposure)
            .add_observer(on_camera_selection_status_changed)
            .add_systems(lunco_core::SceneTeardown, clear_scene_exposures)
            .add_systems(Update, mark_exposure_dirty)
            .add_systems(
                Update,
                publish_exposure
                    .after(mark_exposure_dirty)
                    .run_if(exposure_publish_due),
            );
    }
}

const LUNAR_MAP_SETTING_KEY: &str = "ui.lunar_map";
const RUNTIME_UI_VISIBILITY_HOOK: &str = "runtime.ui.visibility";
const RUNTIME_UI_PROPERTIES_HOOK: &str = "runtime.ui.properties";

/// Ask the active Twin's Rhai policy whether a subject-scoped surface is
/// visible. The engine passes one owned, typed fact map; no product or model
/// name is interpreted here.
fn runtime_ui_visibility(facts: &HookValue, surface_id: &str) -> bool {
    match lunco_hooks::invoke(RUNTIME_UI_VISIBILITY_HOOK, std::slice::from_ref(facts)) {
        Some(Ok(HookValue::Map(values))) => {
            let visible = values
                .iter()
                .find(|(key, _)| key == "visible")
                .and_then(|(_, value)| match value {
                    HookValue::Bool(value) => Some(*value),
                    _ => None,
                });
            match visible {
                Some(visible) => visible,
                _ => {
                    warn!(surface_id, "[runtime-ui] visibility policy must return a map with boolean `visible`; surface hidden");
                    false
                }
            }
        }
        Some(Ok(value)) => {
            warn!(
                surface_id,
                returned = ?value,
                "[runtime-ui] visibility policy must return a typed result map; surface hidden"
            );
            false
        }
        Some(Err(error)) => {
            warn!(
                surface_id,
                "[runtime-ui] visibility policy failed: {error}; surface hidden"
            );
            false
        }
        None => {
            warn!(
                surface_id,
                hook = RUNTIME_UI_VISIBILITY_HOOK,
                "[runtime-ui] no visibility policy is registered; surface hidden"
            );
            false
        }
    }
}

/// Ask the active Twin's Rhai policy for scalar presentation properties. The
/// exposure registry deliberately remains scalar because HUI/egui/API readers
/// share it; structured facts are consumed and flattened only at this policy
/// boundary.
fn runtime_ui_properties(facts: &HookValue, surface_id: &str) -> Vec<(String, ExposureValue)> {
    let Some(result) = lunco_hooks::invoke(RUNTIME_UI_PROPERTIES_HOOK, std::slice::from_ref(facts))
    else {
        warn!(
            surface_id,
            hook = RUNTIME_UI_PROPERTIES_HOOK,
            "[runtime-ui] no properties policy is registered; surface has no presentation values"
        );
        return Vec::new();
    };
    let result = match result {
        Ok(HookValue::Map(values)) => values,
        Ok(value) => {
            warn!(
                surface_id,
                returned = ?value,
                "[runtime-ui] properties policy must return a typed map; surface has no presentation values"
            );
            return Vec::new();
        }
        Err(error) => {
            warn!(surface_id, "[runtime-ui] properties policy failed: {error}");
            return Vec::new();
        }
    };
    result
        .into_iter()
        .filter_map(|(name, value)| {
            let value = match value {
                HookValue::Str(value) => ExposureValue::Text(value),
                HookValue::Bool(value) => ExposureValue::Bool(value),
                HookValue::Int(value) => ExposureValue::Number(value as f64),
                HookValue::Float(value) if value.is_finite() => ExposureValue::Number(value),
                _ => {
                    warn!(
                        surface_id,
                        property = name,
                        "[runtime-ui] ignored non-scalar policy property"
                    );
                    return None;
                }
            };
            Some((name, value))
        })
        .collect()
}

fn runtime_ui_facts(
    surface_id: &str,
    root: Option<Entity>,
    subject: Option<GlobalEntityId>,
    control_owner: &str,
    visibility_mode: &str,
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_catalog_id: &Query<&lunco_core::CatalogEntryId>,
    q_gid: &Query<&GlobalEntityId>,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
    q_vel: &Query<&LinearVelocity>,
    q_angvel: &Query<&AngularVelocity>,
    q_rotation: &Query<&Rotation>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
    telemetry: &[PublicTelemetryValue],
) -> HookValue {
    let label = root
        .map(|root| {
            lunco_core::entity_display_name(
                q_name.get(root).ok(),
                q_callsign.get(root).ok(),
                q_catalog_id.get(root).ok(),
            )
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "selected".to_owned());

    let status = root
        .and_then(|root| q_sim.get(root).ok())
        .map(|(_, sim)| sim_status_facts(&sim.status))
        .unwrap_or_else(|| ("unavailable".to_owned(), None));

    let position = root
        .and_then(|root| {
            lunco_core::coords::world_position(root, q_parents, q_grids, q_spatial).ok()
        })
        .map(|position| {
            HookValue::Array(
                [position.0.x, position.0.y, position.0.z]
                    .into_iter()
                    .map(HookValue::Float)
                    .collect(),
            )
        })
        .unwrap_or_else(|| HookValue::Array(Vec::new()));
    let velocity = root
        .and_then(|root| q_vel.get(root).ok())
        .map(|velocity| {
            HookValue::Array(
                [
                    velocity.0.x as f64,
                    velocity.0.y as f64,
                    velocity.0.z as f64,
                ]
                .into_iter()
                .map(HookValue::Float)
                .collect(),
            )
        })
        .unwrap_or_else(|| HookValue::Array(Vec::new()));
    let angular_velocity = root
        .and_then(|root| q_angvel.get(root).ok())
        .map(|velocity| {
            HookValue::Array(
                [
                    velocity.0.x as f64,
                    velocity.0.y as f64,
                    velocity.0.z as f64,
                ]
                .into_iter()
                .map(HookValue::Float)
                .collect(),
            )
        })
        .unwrap_or_else(|| HookValue::Array(Vec::new()));
    let rotation = root
        .and_then(|root| q_rotation.get(root).ok())
        .map(|rotation| {
            HookValue::Array(
                [
                    rotation.0.x as f64,
                    rotation.0.y as f64,
                    rotation.0.z as f64,
                    rotation.0.w as f64,
                ]
                .into_iter()
                .map(HookValue::Float)
                .collect(),
            )
        })
        .unwrap_or_else(|| HookValue::Array(Vec::new()));

    let mut participants = q_sim
        .iter()
        .filter(|(entity, _)| root.is_some_and(|root| is_owned_by_vessel(*entity, root, q_parents)))
        .map(|(entity, sim)| {
            let path = q_paths
                .get(entity)
                .map(|(_, path)| path.path.clone())
                .unwrap_or_default();
            let public_names = authored_output_names(entity, q_paths, stages, canonical);
            let outputs = sim
                .outputs
                .iter()
                .map(|(name, value)| (name.clone(), HookValue::Float(*value)))
                .collect::<Vec<_>>();
            let public_outputs = sim
                .outputs
                .iter()
                .filter(|(name, _)| {
                    public_names
                        .as_ref()
                        .is_some_and(|names| names.contains(*name))
                })
                .map(|(name, value)| (name.clone(), HookValue::Float(*value)))
                .collect::<Vec<_>>();
            let (status, error) = sim_status_facts(&sim.status);
            let gid = q_gid
                .get(entity)
                .ok()
                .map(|gid| HookValue::Int(gid.get() as i64))
                .unwrap_or(HookValue::Unit);
            (
                path.clone(),
                HookValue::map([
                    ("entity_gid", gid),
                    ("path", HookValue::str(path)),
                    ("model", HookValue::str(sim.model_name.clone())),
                    ("status", HookValue::str(status)),
                    ("error", error.map_or(HookValue::Unit, HookValue::str)),
                    ("inputs", scalar_hook_map(&sim.inputs)),
                    ("outputs", HookValue::Map(outputs)),
                    ("public_outputs", HookValue::Map(public_outputs)),
                ]),
            )
        })
        .collect::<Vec<_>>();
    participants.sort_by(|(a, _), (b, _)| a.cmp(b));

    let telemetry = telemetry
        .iter()
        .map(|value| {
            HookValue::map([
                ("label", HookValue::str(value.label.clone())),
                ("value", HookValue::Float(value.value)),
                (
                    "unit",
                    value
                        .unit
                        .as_ref()
                        .map_or(HookValue::Unit, |unit| HookValue::str(unit.clone())),
                ),
            ])
        })
        .collect();

    HookValue::map([
        ("surface_id", HookValue::str(surface_id)),
        (
            "subject_gid",
            subject
                .map(|gid| HookValue::Int(gid.get() as i64))
                .unwrap_or(HookValue::Unit),
        ),
        ("control_owner", HookValue::str(control_owner)),
        ("visibility_mode", HookValue::str(visibility_mode)),
        ("available", HookValue::Bool(root.is_some())),
        ("label", HookValue::str(label)),
        ("status", HookValue::str(status.0)),
        ("error", status.1.map_or(HookValue::Unit, HookValue::str)),
        ("position", position),
        ("velocity", velocity),
        ("angular_velocity", angular_velocity),
        ("rotation", rotation),
        ("telemetry", HookValue::Array(telemetry)),
        (
            "participants",
            HookValue::Array(participants.into_iter().map(|(_, facts)| facts).collect()),
        ),
    ])
}

fn scalar_hook_map(values: &HashMap<String, f64>) -> HookValue {
    let mut values = values
        .iter()
        .map(|(name, value)| (name.clone(), HookValue::Float(*value)))
        .collect::<Vec<_>>();
    values.sort_by(|(a, _), (b, _)| a.cmp(b));
    HookValue::Map(values)
}

fn sim_status_facts(status: &SimStatus) -> (String, Option<String>) {
    match status {
        SimStatus::Idle => ("idle".to_owned(), None),
        SimStatus::Compiling => ("compiling".to_owned(), None),
        SimStatus::Running => ("running".to_owned(), None),
        SimStatus::Paused => ("paused".to_owned(), None),
        SimStatus::Error(error) => ("error".to_owned(), Some(error.clone())),
    }
}

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
    current_surface: Option<String>,
    last_label: Option<String>,
    tipped: bool,
    max_slope_deg: Option<f32>,
    overlay: Option<lunco_terrain_surface::overlay::TerrainOverlayParams>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SeminarTraceInputs<'w, 's> {
    surface: lunco_terrain_surface::GridSurfaceQuery<'w, 's>,
    prims: Query<'w, 's, &'static lunco_usd_bevy_scene::UsdPrimPath>,
    provenance: Query<'w, 's, &'static lunco_core::Provenance>,
}

/// Fallback amber threshold for a body whose limits cannot be derived.
///
/// GENERIC on purpose: the real roll-over angle is `atan(half_track / com_height)`
/// and the real slip limit is `atan(μ)`, both properties of the AUTHORED vehicle.
/// A driven rover publishes exactly those through the generic exposure registry,
/// and a Twin policy may prefer them — see
/// `docs/architecture/58-vessel-envelope-and-routes.md`. These remain for the
/// unknown-body case (a wheel-less body), where they are
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

/// What a generic driven-body surface needs, resolved at the bounded exposure
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
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_catalog_id: &Query<&lunco_core::CatalogEntryId>,
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
    // Resolve both candidates through the shared entity label contract. The
    // source `Name` remains a full USD path, but it is never shown as the label.
    let peer_ent = q_ids
        .iter()
        .find(|(_, g)| g.get() == pick.peer)
        .map(|(e, _)| e);
    let peer_label = match peer_ent {
        Some(e) => {
            let own = q_name.get(e).ok().map(|n| {
                lunco_core::entity_display_name(
                    Some(n),
                    q_callsign.get(e).ok(),
                    q_catalog_id.get(e).ok(),
                )
            });
            let parent = q_parents.get(e).ok().and_then(|p| {
                let parent_entity = p.parent();
                let name = q_name.get(parent_entity).ok()?;
                Some(lunco_core::entity_display_name(
                    Some(name),
                    q_callsign.get(parent_entity).ok(),
                    q_catalog_id.get(parent_entity).ok(),
                ))
            });
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
    q_catalog_id: &Query<&lunco_core::CatalogEntryId>,
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
    let label = lunco_core::entity_display_name(
        q_name.get(vessel).ok(),
        q_callsign.get(vessel).ok(),
        q_catalog_id.get(vessel).ok(),
    );
    let label = if label.is_empty() {
        q_gid
            .get(vessel)
            .map(|g| format!("vessel #{}", g.get()))
            .unwrap_or_else(|_| "vessel".to_string())
    } else {
        label
    };

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
        link: resolve_link(
            vessel,
            q_links,
            q_parents,
            q_name,
            q_callsign,
            q_catalog_id,
            q_ids,
        ),
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
mod exposure_schedule_tests {
    use super::*;

    #[test]
    fn publisher_timer_runs_at_the_exposure_cadence() {
        let mut timer = None;

        assert!(!exposure_publish_due_at(
            &mut timer,
            Duration::from_millis(49)
        ));
        assert!(exposure_publish_due_at(
            &mut timer,
            Duration::from_millis(2)
        ));
    }
}

#[cfg(test)]
mod exposure_tests {
    use super::*;

    fn lunar_surface_pose(geo: lunco_celestial::Geodetic) -> lunco_celestial::SurfacePose {
        lunco_celestial::SurfacePose {
            site: Entity::from_bits(1),
            body: 301,
            site_position: lunco_celestial::SitePosition(DVec3::ZERO),
            site_rotation: DQuat::IDENTITY,
            body_fixed_position: lunco_celestial::BodyFixedPosition(DVec3::ZERO),
            body_fixed_rotation: DQuat::IDENTITY,
            geodetic: geo,
        }
    }

    #[test]
    fn lunar_map_projection_wraps_longitude_and_places_lunar_marker() {
        let projection = project_lunar_map(
            true,
            true,
            Some(301),
            Some(lunar_surface_pose(lunco_celestial::Geodetic::new(
                45.0, 180.0, 12.0,
            ))),
        );

        assert_eq!(projection.display, "flex");
        assert_eq!(projection.status, "SURFACE FIX");
        assert_eq!(projection.coordinates, "LAT +45.00° · LON +180.00°");
        assert_eq!(projection.altitude, "ALT +12 m");
        assert_eq!(projection.marker_display, "flex");
        assert_eq!(projection.marker_left, "0.00%");
        assert_eq!(projection.marker_top, "25.00%");
    }

    #[test]
    fn lunar_map_projection_hides_marker_without_a_valid_lunar_surface_pose() {
        let orbit = project_lunar_map(true, true, Some(301), None);
        assert_eq!(orbit.display, "flex");
        assert_eq!(orbit.status, "AWAITING LUNAR FIX");
        assert_eq!(orbit.marker_display, "none");

        let hidden = project_lunar_map(
            false,
            true,
            Some(301),
            Some(lunar_surface_pose(lunco_celestial::Geodetic::new(
                45.0, 180.0, 12.0,
            ))),
        );
        assert_eq!(hidden.display, "none");
        assert_eq!(hidden.status, "MOON UNAVAILABLE");
        assert_eq!(hidden.marker_display, "none");
    }

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
    fn camera_exposure_projects_authoritative_fact_and_compact_label() {
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
        assert_eq!(surface.properties["active_label"].render(), "Close");
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

    #[test]
    fn scene_teardown_hides_scene_surfaces_but_retains_camera_status() {
        let mut exposures = EngineExposures::default();
        exposures.writer("camera-status").visible(true);
        exposures.writer("subject-surface").visible(true);
        exposures.writer("secondary-surface").visible(true);
        let mut refresh = ExposureRefresh::default();
        refresh.first_update = false;
        refresh.clear_dirty();

        clear_scene_exposures_impl(&mut exposures, &mut refresh);

        assert!(exposures.surfaces["camera-status"].visible);
        assert!(!exposures.surfaces["subject-surface"].visible);
        assert!(!exposures.surfaces["secondary-surface"].visible);
        assert!(refresh.first_update);
        assert!(refresh.any_dirty());
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
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    stage_revision: Option<Res<lunco_usd_bevy_scene::UsdStageRevision>>,
    scene_mount: Res<SceneMountState>,
    overlays: RuntimeOverlayInputs,
    mut refresh: ResMut<ExposureRefresh>,
) {
    let driven_changed = !q_avatar.is_empty()
        || !q_velocity.is_empty()
        || !q_spatial.is_empty()
        || !q_links.is_empty()
        || !q_wheels.is_empty()
        || !q_com.is_empty()
        || !q_sim.is_empty()
        || !q_autopilot.is_empty();

    let schema_changed = selected.is_changed();
    let celestial_changed = !q_bodies.is_empty()
        || orbital_pin.is_some_and(|pin| pin.is_changed())
        || workspace
            .as_ref()
            .is_some_and(|workspace| workspace.is_changed());
    let authored_changed = stage_revision.is_some_and(|revision| revision.is_changed());
    let scene_mount_changed = scene_mount.is_changed();

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

    if driven_changed {
        refresh.driven_vessel_dirty = true;
        refresh.control_dirty = true;
        // The map is a projection of the same driven vessel pose. Keep its
        // snapshot on the shared bounded cadence so possession, scene handoff,
        // and continuous surface motion cannot leave a stale marker behind.
        refresh.celestial_dirty = true;
    }
    if schema_changed || authored_changed {
        refresh.schema_dirty = true;
    }
    if authored_changed || scene_mount_changed {
        refresh.control_dirty = true;
    }
    if celestial_changed {
        refresh.celestial_dirty = true;
    }
    if overlay_changed {
        refresh.overlay_dirty = true;
    }
}

/// Withdraw scene-derived surfaces synchronously at the replacement boundary.
///
/// Bevy defers scene entity despawns, while the retained UI consumes the last
/// exposure snapshot. Hiding these namespaces here prevents an outgoing HUD from
/// remaining visible until the next bounded publication; the next active scene
/// repopulates them through the normal first-update path. Camera status is an
/// application-owned retained fact and intentionally survives scene replacement.
pub(crate) fn clear_scene_exposures(
    mut exposures: ResMut<EngineExposures>,
    mut refresh: ResMut<ExposureRefresh>,
) {
    clear_scene_exposures_impl(&mut exposures, &mut refresh);
}

fn clear_scene_exposures_impl(exposures: &mut EngineExposures, refresh: &mut ExposureRefresh) {
    let namespaces = exposures
        .surfaces
        .keys()
        .filter(|namespace| namespace.as_str() != "camera-status")
        .cloned()
        .collect::<Vec<_>>();
    for namespace in namespaces {
        exposures.writer(&namespace).visible(false);
    }

    refresh.driven_vessel_dirty = true;
    refresh.control_dirty = true;
    refresh.schema_dirty = true;
    refresh.celestial_dirty = true;
    refresh.overlay_dirty = true;
    refresh.first_update = true;
}

/// Run the exposure publisher on its existing bounded cadence while preserving
/// the first publication at startup. The condition owns only scheduling; the
/// publisher still uses [`ExposureRefresh`] to decide whether its snapshot
/// actually needs rebuilding.
pub(crate) fn exposure_publish_due(
    time: Res<Time>,
    refresh: Res<ExposureRefresh>,
    mut timer: Local<Option<Timer>>,
) -> bool {
    if refresh.first_update {
        return true;
    }
    exposure_publish_due_at(&mut timer, time.delta()) && refresh.any_dirty()
}

fn exposure_publish_due_at(timer: &mut Option<Timer>, delta: Duration) -> bool {
    timer
        .get_or_insert_with(|| Timer::from_seconds(1.0 / EXPOSURE_UPDATE_HZ, TimerMode::Repeating))
        .tick(delta)
        .just_finished()
}

/// Publish a vessel exposure namespace. The engine resolves its authoritative
/// state here, then writes only generic named values; the runtime UI layer owns
/// all template and style mechanics.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ExposureRuntime<'w, 's> {
    refresh: ResMut<'w, ExposureRefresh>,
    exposures: ResMut<'w, EngineExposures>,
    signals: Res<'w, SignalRegistry>,
    selected: Res<'w, SelectedEntities>,
    scene_mount: Res<'w, SceneMountState>,
    local_avatar: Res<'w, TheLocalAvatar>,
    bodies: Query<'w, 's, &'static CelestialBody>,
    angular_velocity: Query<'w, 's, &'static AngularVelocity>,
    rotation: Query<'w, 's, &'static Rotation>,
    orbital_pin: Option<Res<'w, OrbitalViewPin>>,
    stages: Res<'w, Assets<UsdStageAsset>>,
    canonical: NonSend<'w, CanonicalStages>,
}

/// Authoritative inputs for the driven-body projection.
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
    catalog_id: Query<'w, 's, &'static lunco_core::CatalogEntryId>,
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
    usd_paths: Query<'w, 's, (Entity, &'static lunco_usd_bevy_scene::UsdPrimPath)>,
    entities: Query<'w, 's, Entity>,
    /// Preview descendants also carry `UsdPrimPath`, but are render-only
    /// projections and must never populate live operator exposures.
    scene_roots: Query<'w, 's, (), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
}

fn hide_runtime_surface(exposures: &mut EngineExposures, surface_id: &str) {
    let mut ui = exposures.writer(surface_id);
    ui.subject(None);
    ui.visible(false);
    ui.clear_properties();
}

pub(crate) fn publish_exposure(
    queries: ExposureQueries,
    geo: GeodeticHud,
    mut runtime: ExposureRuntime,
    overlays: RuntimeOverlayInputs,
    trace_inputs: SeminarTraceInputs,
    mut seminar: Local<SeminarExposureTrace>,
    mut runtime_surface_roots: Local<RuntimeSurfaceRootCache>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    stage_revision: Option<Res<lunco_usd_bevy_scene::UsdStageRevision>>,
) {
    if let Some(overlay) = overlays.overlay.as_deref() {
        if seminar.overlay != Some(*overlay) {
            info!(
                "[seminar] terrain diagnostic: enabled={} mode={} safe={:.1}° cliff={:.1}° opacity={:.2}",
                overlay.enabled,
                if overlay.lod_depth { "lod" } else { "slope" },
                overlay.safe_deg,
                overlay.cliff_deg,
                overlay.opacity,
            );
            seminar.overlay = Some(*overlay);
        }
    }

    if !runtime.refresh.any_dirty() && !runtime.refresh.first_update {
        return;
    }
    let first_update = runtime.refresh.first_update;
    // Authored surface metadata is part of the driven-body projection. A USD
    // edit can change its identity or visibility mode without changing pose,
    // so refresh the driven projection with the same invalidation boundary.
    let update_driven =
        first_update || runtime.refresh.driven_vessel_dirty || runtime.refresh.control_dirty;
    let update_control = first_update || runtime.refresh.control_dirty;
    let update_schema = first_update || runtime.refresh.schema_dirty;
    let update_celestial = first_update || runtime.refresh.celestial_dirty;
    let update_overlay = first_update || runtime.refresh.overlay_dirty;
    runtime.refresh.clear_dirty();
    runtime.refresh.first_update = false;

    if update_control {
        let revision = stage_revision.as_deref().map(|revision| revision.0);
        runtime_surface_roots.refresh(
            revision,
            &runtime.scene_mount,
            &queries.usd_paths,
            &queries.parents,
            &queries.scene_roots,
            &queries.entities,
            &runtime.stages,
            &runtime.canonical,
        );
        publish_runtime_surface_exposures(
            &mut runtime.exposures,
            &queries.name,
            &queries.callsign,
            &queries.catalog_id,
            &queries.gid,
            &queries.sim,
            &runtime.signals,
            &queries.channels,
            &queries.parents,
            &queries.grids,
            &queries.velocity,
            &runtime.angular_velocity,
            &runtime.rotation,
            &queries.spatial,
            &queries.usd_paths,
            &runtime.stages,
            &runtime.canonical,
            &runtime_surface_roots.roots,
            &runtime_surface_roots.retired_surface_ids,
            &runtime.local_avatar,
            &queries.avatar,
        );
        runtime_surface_roots.retired_surface_ids.clear();
    }

    if update_driven {
        let vessel = resolve_driven(
            &runtime.local_avatar,
            &queries.avatar,
            &queries.name,
            &queries.callsign,
            &queries.catalog_id,
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
        );

        if let Some(vessel) = vessel {
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
            if let Some(surface) = runtime_surface_roots
                .roots
                .iter()
                .find(|surface| surface.entity == vessel.entity)
            {
                if seminar.current_surface.as_deref() != Some(surface.surface_id.as_str()) {
                    if let Some(previous) =
                        seminar.current_surface.replace(surface.surface_id.clone())
                    {
                        hide_runtime_surface(&mut runtime.exposures, &previous);
                    }
                }
                let mut ui = runtime.exposures.writer(&surface.surface_id);
                let subject = queries.gid.get(vessel.entity).ok().copied();
                ui.subject(subject);
                let control_owner =
                    if locally_possesses(&runtime.local_avatar, &queries.avatar, vessel.entity) {
                        "local"
                    } else {
                        "none"
                    };
                let facts = runtime_ui_facts(
                    &surface.surface_id,
                    Some(vessel.entity),
                    subject,
                    control_owner,
                    &surface.visibility_mode,
                    &queries.name,
                    &queries.callsign,
                    &queries.catalog_id,
                    &queries.gid,
                    &queries.sim,
                    &queries.parents,
                    &queries.velocity,
                    &runtime.angular_velocity,
                    &runtime.rotation,
                    &queries.grids,
                    &queries.spatial,
                    &queries.usd_paths,
                    &runtime.stages,
                    &runtime.canonical,
                    &[],
                );
                ui.visible(runtime_ui_visibility(&facts, &surface.surface_id));
                let telemetry = resolve_authored_telemetry(
                    vessel.entity,
                    &runtime.signals,
                    &queries.parents,
                    &queries.channels,
                );
                publish_vessel_values(&mut ui, &vessel, autopilot, &telemetry);
            } else {
                if let Some(surface_id) = seminar.current_surface.take() {
                    hide_runtime_surface(&mut runtime.exposures, &surface_id);
                }
            }
        } else {
            seminar.current_vessel = None;
            seminar.last_label = None;
            seminar.tipped = false;
            seminar.max_slope_deg = None;
            if let Some(surface_id) = seminar.current_surface.take() {
                hide_runtime_surface(&mut runtime.exposures, &surface_id);
            }
        }
    }

    if update_schema {
        publish_lunica_schema_exposure(
            &mut runtime.exposures,
            &runtime.selected,
            &queries.usd_paths,
            &runtime.stages,
            &runtime.canonical,
        );
    }
    if update_celestial {
        publish_celestial_capability(
            &mut runtime.exposures,
            &runtime.bodies,
            runtime.orbital_pin.as_deref(),
            &runtime.local_avatar,
            &queries.avatar,
            &geo.surface_pose,
            workspace.as_deref(),
        );
    }
    if update_overlay {
        publish_runtime_overlay_exposures(&mut runtime.exposures, &overlays);
    }
}

fn publish_celestial_capability(
    exposures: &mut EngineExposures,
    bodies: &Query<&CelestialBody>,
    orbital_pin: Option<&OrbitalViewPin>,
    local_avatar: &TheLocalAvatar,
    avatars: &Query<&ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
    surface_pose: &lunco_celestial::SurfacePoseQuery,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
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
    let local_surface_pose = local_avatar
        .0
        .and_then(|avatar| avatars.get(avatar).ok())
        .and_then(|controller| surface_pose.get(controller.vessel_entity));
    let lunar_map = project_lunar_map(
        twin_setting_is_enabled(workspace, LUNAR_MAP_SETTING_KEY),
        moon,
        active_body,
        local_surface_pose,
    );
    let mut ui = exposures.writer("celestial-view");
    ui.visible(moon || earth);
    ui.property("body_moon_present", moon);
    ui.property("body_earth_present", earth);
    ui.property("active_body_id", f64::from(active_body.unwrap_or_default()));
    ui.property("map_display", lunar_map.display);
    ui.property("map_status", lunar_map.status);
    ui.property("map_coordinates", lunar_map.coordinates);
    ui.property("map_altitude", lunar_map.altitude);
    ui.property("map_marker_display", lunar_map.marker_display);
    ui.property("map_marker_left", lunar_map.marker_left);
    ui.property("map_marker_top", lunar_map.marker_top);
}

/// The domain-neutral snapshot required by the authored lunar map.
///
/// The map is an equirectangular presentation of the canonical body-fixed
/// geodetic pose. Longitude wraps at the authored atlas seam (-180/180°),
/// while latitude is clamped only at the physical poles. Invalid or
/// non-lunar poses never produce a marker, so the UI cannot display a stale or
/// fabricated location.
#[derive(Debug, PartialEq)]
struct LunarMapProjection {
    display: &'static str,
    status: &'static str,
    coordinates: String,
    altitude: String,
    marker_display: &'static str,
    marker_left: String,
    marker_top: String,
}

impl LunarMapProjection {
    fn hidden() -> Self {
        Self {
            display: "none",
            status: "MOON UNAVAILABLE",
            coordinates: "—".into(),
            altitude: "—".into(),
            marker_display: "none",
            marker_left: "0%".into(),
            marker_top: "0%".into(),
        }
    }
}

fn twin_setting_is_enabled(
    workspace: Option<&lunco_workspace::WorkspaceResource>,
    key: &str,
) -> bool {
    let Some(workspace) = workspace else {
        return false;
    };
    let Some(twin_id) = workspace.active_twin else {
        return false;
    };
    matches!(
        workspace
            .twin(twin_id)
            .and_then(|twin| twin.manifest.as_ref())
            .and_then(|manifest| manifest.setting(key)),
        Some(lunco_twin::TwinSettingValue::Bool(true))
    )
}

fn project_lunar_map(
    map_visible: bool,
    moon_present: bool,
    active_body: Option<i32>,
    pose: Option<lunco_celestial::SurfacePose>,
) -> LunarMapProjection {
    if !map_visible || !moon_present {
        return LunarMapProjection::hidden();
    }

    let Some(pose) = pose else {
        return LunarMapProjection {
            display: "flex",
            status: if active_body == Some(301) {
                "AWAITING LUNAR FIX"
            } else {
                "NO LUNAR FIX"
            },
            coordinates: "LAT — · LON —".into(),
            altitude: "ALT —".into(),
            marker_display: "none",
            marker_left: "0%".into(),
            marker_top: "0%".into(),
        };
    };

    let geo = pose.geodetic;
    if pose.body != 301
        || !geo.lat_deg.is_finite()
        || !geo.lon_deg.is_finite()
        || !geo.height_m.is_finite()
    {
        return LunarMapProjection {
            display: "flex",
            status: "NO LUNAR FIX",
            coordinates: "LAT — · LON —".into(),
            altitude: "ALT —".into(),
            marker_display: "none",
            marker_left: "0%".into(),
            marker_top: "0%".into(),
        };
    }

    let left = ((geo.lon_deg + 180.0).rem_euclid(360.0) / 360.0 * 100.0).clamp(0.0, 100.0);
    let top = ((90.0 - geo.lat_deg.clamp(-90.0, 90.0)) / 180.0 * 100.0).clamp(0.0, 100.0);
    LunarMapProjection {
        display: "flex",
        status: "SURFACE FIX",
        coordinates: format!("LAT {:+.2}° · LON {:+.2}°", geo.lat_deg, geo.lon_deg),
        altitude: format!("ALT {:+.0} m", geo.height_m),
        marker_display: "flex",
        marker_left: format!("{left:.2}%"),
        marker_top: format!("{top:.2}%"),
    }
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
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    stages: &Assets<UsdStageAsset>,
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
    let Some(stage_asset) = stages.get(&root_path.stage_handle) else {
        return false;
    };
    let (reader, _generation) = canonical.reader_for(root_path.stage_handle.id(), stage_asset);
    let reader: &dyn UsdReadObject = &reader;
    let Ok(root_sdf) = SdfPath::new(&root_path.path) else {
        return false;
    };
    if reader.boolean(&root_sdf, "lunco:ui:schemaRoot") != Some(true) {
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
    for path in reader.prim_paths() {
        let path_text = path.to_string();
        if path_text != root_path.path && !path_text.starts_with(&root_prefix) {
            continue;
        }
        if !reader.is_active(&path) || reader.boolean(&path, "lunco:ui:schemaNode") != Some(true) {
            continue;
        }
        let title = reader
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
            role: reader
                .text(&path, "lunco:ui:schemaRole")
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| "Connected USD block".to_owned()),
            column: reader.integer(&path, "lunco:ui:schemaColumn").unwrap_or(0),
            row: reader.integer(&path, "lunco:ui:schemaRow").unwrap_or(0),
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
        for attr in reader.attr_names(&target) {
            if !attr.starts_with("inputs:") {
                continue;
            }
            for source in reader.connections(&target, &attr) {
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

/// Cached authored runtime-surface topology. The USD revision is the authoritative
/// invalidation boundary; entity IDs remain valid until that same projection
/// changes, so a motion-only publication never scans every USD prim again.
#[derive(Default)]
pub(crate) struct RuntimeSurfaceRootCache {
    revision: Option<u64>,
    active_root: Option<Entity>,
    initialized: bool,
    roots: Vec<AuthoredRuntimeSurface>,
    retired_surface_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthoredRuntimeSurface {
    entity: Entity,
    surface_id: String,
    visibility_mode: String,
}

impl RuntimeSurfaceRootCache {
    fn refresh(
        &mut self,
        revision: Option<u64>,
        scene_mount: &SceneMountState,
        q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
        q_parents: &Query<&ChildOf>,
        q_scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
        q_entities: &Query<Entity>,
        stages: &Assets<UsdStageAsset>,
        canonical: &CanonicalStages,
    ) {
        let active_root = scene_mount.active_root();
        if self.initialized && self.revision == revision && self.active_root == active_root {
            return;
        }
        let previous = self
            .roots
            .iter()
            .map(|root| root.surface_id.clone())
            .collect::<HashSet<_>>();
        self.roots = authored_runtime_surfaces(
            scene_mount,
            q_paths,
            q_parents,
            q_scene_roots,
            q_entities,
            stages,
            canonical,
        );
        self.roots
            .sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
        let current = self
            .roots
            .iter()
            .map(|root| root.surface_id.clone())
            .collect::<HashSet<_>>();
        self.retired_surface_ids = previous.difference(&current).cloned().collect();
        self.revision = revision;
        self.active_root = active_root;
        self.initialized = true;
    }
}

/// Publish responses for explicitly authored runtime surfaces.
///
/// Surface identity and visibility mode come from composed USD. This keeps the
/// engine independent of a fixed set of product models, surface names, or
/// surface slots.
fn publish_runtime_surface_exposures(
    exposures: &mut EngineExposures,
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_catalog_id: &Query<&lunco_core::CatalogEntryId>,
    q_gid: &Query<&GlobalEntityId>,
    q_sim: &Query<(Entity, &SimComponent)>,
    signals: &SignalRegistry,
    q_channels: &Query<(
        Entity,
        &lunco_core::telemetry::Parameter,
        Option<&lunco_core::markers::Callsign>,
    )>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_vel: &Query<&LinearVelocity>,
    q_angvel: &Query<&AngularVelocity>,
    q_rotation: &Query<&Rotation>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
    roots: &[AuthoredRuntimeSurface],
    retired_surface_ids: &[String],
    local_avatar: &TheLocalAvatar,
    q_avatar: &Query<&ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
) {
    for surface_id in retired_surface_ids {
        let mut ui = exposures.writer(surface_id);
        ui.subject(None);
        ui.visible(false);
        ui.clear_properties();
    }

    for root in roots {
        let subject = q_gid.get(root.entity).ok().copied();
        let control_owner = if locally_possesses(local_avatar, q_avatar, root.entity) {
            "local"
        } else {
            "none"
        };
        let telemetry = resolve_authored_telemetry(root.entity, signals, q_parents, q_channels);
        publish_selected_control_exposure(
            exposures,
            &root.surface_id,
            Some(root.entity),
            subject,
            control_owner,
            &root.visibility_mode,
            &telemetry,
            q_name,
            q_callsign,
            q_catalog_id,
            q_gid,
            q_sim,
            q_parents,
            q_grids,
            q_vel,
            q_angvel,
            q_rotation,
            q_spatial,
            q_paths,
            stages,
            canonical,
        );
    }
}

fn publish_selected_control_exposure(
    exposures: &mut EngineExposures,
    namespace: &str,
    root: Option<Entity>,
    subject: Option<GlobalEntityId>,
    control_owner: &str,
    visibility_mode: &str,
    telemetry: &[PublicTelemetryValue],
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_catalog_id: &Query<&lunco_core::CatalogEntryId>,
    q_gid: &Query<&GlobalEntityId>,
    q_sim: &Query<(Entity, &SimComponent)>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_vel: &Query<&LinearVelocity>,
    q_angvel: &Query<&AngularVelocity>,
    q_rotation: &Query<&Rotation>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
) {
    let facts = runtime_ui_facts(
        namespace,
        root,
        subject,
        control_owner,
        visibility_mode,
        q_name,
        q_callsign,
        q_catalog_id,
        q_gid,
        q_sim,
        q_parents,
        q_vel,
        q_angvel,
        q_rotation,
        q_grids,
        q_spatial,
        q_paths,
        stages,
        canonical,
        telemetry,
    );
    let visible = runtime_ui_visibility(&facts, namespace);
    let properties = runtime_ui_properties(&facts, namespace);
    let mut ui = exposures.writer(namespace);
    ui.subject(subject);
    ui.visible(visible);
    ui.clear_properties();
    for (name, value) in properties {
        ui.property(name, value);
    }
}

fn locally_possesses(
    local_avatar: &TheLocalAvatar,
    q_avatar: &Query<&ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
    subject: Entity,
) -> bool {
    local_avatar
        .0
        .and_then(|avatar| q_avatar.get(avatar).ok())
        .is_some_and(|controller| controller.vessel_entity == subject)
}

/// Discover roots that explicitly opt into a runtime surface.
///
/// The surface ID and visibility mode are authored USD data, not a list of
/// vehicle paths or a slot convention in Rust. A Twin can bind any model to any
/// registered runtime surface without an engine change.
fn authored_runtime_surfaces(
    scene_mount: &SceneMountState,
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    q_parents: &Query<&ChildOf>,
    q_scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_entities: &Query<Entity>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
) -> Vec<AuthoredRuntimeSurface> {
    let mut roots = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_surface_ids = HashSet::new();
    for (entity, prim_path) in q_paths.iter() {
        if !is_active_scene_entity(entity, scene_mount, q_parents, q_scene_roots, q_entities) {
            continue;
        }
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim_path.stage_handle.id(), stage_asset);
        let reader: &dyn UsdReadObject = &reader;
        let Ok(path) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        let Some(surface_id) = reader
            .text(&path, "lunco:ui:surfaceId")
            .filter(|surface_id| !surface_id.trim().is_empty())
        else {
            continue;
        };
        if !accept_runtime_surface_identity(
            &mut seen_paths,
            prim_path.stage_handle.id(),
            &prim_path.path,
        ) {
            warn!(
                "[runtime-ui] duplicate ECS projection for authored surface root {}; keeping one active projection",
                prim_path.path
            );
            continue;
        }
        if !seen_surface_ids.insert(surface_id.clone()) {
            warn!(
                surface_id,
                "[runtime-ui] duplicate active surface identity; keeping the first authored root"
            );
            continue;
        }
        roots.push(AuthoredRuntimeSurface {
            entity,
            surface_id,
            visibility_mode: reader
                .text(&path, "lunco:ui:visibilityMode")
                .unwrap_or_else(|| "possessed".to_owned()),
        });
    }
    roots
}

/// Return true only for an entity owned by the active scene mount.
///
/// Preview prims intentionally retain ordinary USD components so they render,
/// but their root carries `UsdPreviewOnly` rather than `UsdSceneRoot`. The
/// mount state additionally invalidates outgoing roots before deferred despawn
/// and excludes additive document roots from operator-facing exposures.
fn is_active_scene_entity(
    entity: Entity,
    scene_mount: &SceneMountState,
    q_parents: &Query<&ChildOf>,
    q_scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_entities: &Query<Entity>,
) -> bool {
    scene_root_ancestor(entity, q_scene_roots, q_parents, q_entities)
        .is_ok_and(|root| is_active_scene_root(scene_mount, root))
}

fn is_active_scene_root(scene_mount: &SceneMountState, root: Option<Entity>) -> bool {
    root.is_some_and(|root| scene_mount.active_root() == Some(root))
}

fn accept_runtime_surface_identity(
    seen: &mut HashSet<(AssetId<UsdStageAsset>, String)>,
    stage_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    seen.insert((stage_id, path.to_owned()))
}

fn authored_output_names(
    entity: Entity,
    q_paths: &Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath)>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
) -> Option<std::collections::HashSet<String>> {
    let (_, prim_path) = q_paths.get(entity).ok()?;
    let stage_asset = stages.get(&prim_path.stage_handle)?;
    let (reader, _generation) = canonical.reader_for(prim_path.stage_handle.id(), stage_asset);
    let reader: &dyn UsdReadObject = &reader;
    let path = SdfPath::new(&prim_path.path).ok()?;
    let names = reader
        .attr_names(&path)
        .into_iter()
        .filter_map(|name| name.strip_prefix("outputs:").map(str::to_owned))
        .collect::<std::collections::HashSet<_>>();
    (!names.is_empty()).then_some(names)
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
    // Keep the full path as the authoritative fact for Rhai/diagnostics, and
    // derive one deterministic identity label for compact status surfaces.
    // Selection policy remains in Rhai/the typed camera command path.
    let active_name = status.active_name.as_deref().unwrap_or("");
    let labels = lunco_usd_bevy::camera_switch::camera_display_labels(&status.cameras);
    let active_label = status
        .active_name
        .as_ref()
        .and_then(|active| status.cameras.iter().position(|name| name == active))
        .and_then(|index| labels.get(index))
        .map(String::as_str)
        .unwrap_or(active_name);
    ui.property("active_name", active_name);
    ui.property("active_label", active_label);
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

/// Publish the generic derived values for the driven-body surface.
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
