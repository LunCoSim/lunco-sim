//! Comms connectivity geometry (docs 36 + 43): pairwise sight-lines between
//! antenna-flagged entities — range, local elevation, analytic body-sphere
//! occlusion — published as ports and AOS/LOS telemetry events.
//!
//! Comms is a DOMAIN, not a kernel (doc 38): this module is the domain-neutral
//! geometry substrate only. Which prims are antennas, their masks/ranges, and
//! link dynamics (Modelica/rhai) are authored content.
//!
//! All math runs in the **solar frame** (Bevy axes, meters, heliocentric — see
//! [`crate::geo`]) and needs NO big_space hierarchy: body centers come from
//! the ephemeris, anchored antennas from geodesy, orbiting antennas from
//! Kepler elements, and scene-local antennas through the site frame (the
//! scene-root [`GeodeticAnchor`] + [`SiteAnchor`]). Headless-safe.

use bevy::math::DVec3;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use big_space::prelude::{CellCoord, Grid};

use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};
use lunco_core::{Severity, TelemetryEvent, TelemetryValue};
use lunco_time::WorldTime;

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::{
    solar_position_of_geodetic, solar_tangent_frame, GeodeticAnchor, LocalTangentFrame, SiteAnchor,
};
use crate::kepler::KeplerOrbit;
use crate::registry::{BodyDescriptor, CelestialBodyRegistry};

/// Hook id consulted per antenna pair for the link verdict. The authored rule
/// lives in `assets/scripting/policy/comms_link.rhai` (entry
/// `link_connected(ctx)`); scenarios may override via `register_hook`. `ctx`
/// map: a/b (names), range_m, elev_a/b, min_elev_a/b, occluded, occluded_by,
/// max_range_m. Return bool. No hook registered → builtin Rust rule.
pub const COMMS_LINK_HOOK: &str = "comms.link.connected";

/// Marks an entity as a comms antenna endpoint (USD: `lunco:comms:antenna`).
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct CommsAntenna {
    /// Maximum usable link range in meters (USD: `lunco:comms:maxRangeM`).
    pub max_range_m: f64,
    /// Elevation mask in degrees for surface antennas
    /// (USD: `lunco:comms:minElevationDeg`). Ignored for orbiting antennas.
    pub min_elevation_deg: f64,
    /// Stable peer identity used in port names (`comms:<id>:*`). Authored via
    /// `lunco:comms:id`, or derived by the USD bridge (parent prim's leaf when
    /// the antenna prim has a generic name like "Comms" — keeps two rovers'
    /// antennas distinct). Falls back to the entity `Name`.
    pub id: Option<String>,
}

impl Default for CommsAntenna {
    fn default() -> Self {
        // The elevation mask is OPT-IN (−90 = disabled): geometry/occlusion
        // alone governs by default. A 0° default wrongly severs short
        // surface-to-surface links (a mast base a meter lower than the rover
        // antenna reads a negative elevation at 15 m range yet has clear
        // line-of-sight). Ground stations author a realistic 5°.
        Self { max_range_m: 1.0e12, min_elevation_deg: -90.0, id: None }
    }
}

/// One antenna's view of a peer, refreshed by [`update_comms_links`].
#[derive(Debug, Clone, PartialEq, Reflect)]
pub struct PeerLink {
    pub peer: String,
    pub peer_entity: Entity,
    pub connected: bool,
    pub range_m: f64,
    /// Elevation of the peer above this antenna's local horizon (surface
    /// antennas only).
    pub elevation_deg: Option<f64>,
    /// Name of the registry body blocking the sight-line, if any.
    pub occluded_by: Option<String>,
}

/// Per-antenna link state; the comms [`PortBackend`] reads this.
/// Ports: `comms:<peer>:connected|range_m|elevation_deg`,
/// `comms:route_earth:connected|hops`.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct CommsLinkState {
    pub peers: Vec<PeerLink>,
    /// Relay hops to reach any Earth-anchored antenna (0 = is one), if a
    /// connected route exists.
    pub earth_hops: Option<u32>,
}

/// A resolved pairwise sight-line (for UI/overlays; entities in solar frame).
#[derive(Debug, Clone, Reflect)]
pub struct SightLine {
    pub a: Entity,
    pub b: Entity,
    pub a_name: String,
    pub b_name: String,
    pub range_m: f64,
    pub elevation_a_deg: Option<f64>,
    pub elevation_b_deg: Option<f64>,
    pub occluded_by: Option<String>,
    pub connected: bool,
}

/// All sight-lines + solar-frame antenna positions at the last update.
#[derive(Resource, Debug, Default)]
pub struct CommsLinks {
    pub epoch_jd: f64,
    pub links: Vec<SightLine>,
    pub positions: Vec<(Entity, DVec3)>,
    /// Site tangent frame (solar) used for scene-local antennas, if anchored.
    pub site: Option<SiteFrameSnapshot>,
}

/// The site frame captured at the last comms update.
#[derive(Debug, Clone, Copy)]
pub struct SiteFrameSnapshot {
    pub body: i32,
    pub frame: LocalTangentFrame,
}

/// How an antenna's solar position was resolved.
enum AntennaPose {
    /// Anchored to a body: position + local up (elevation mask applies).
    Surface { pos: DVec3, up: DVec3, body: i32 },
    /// Free-flying (Kepler orbit): no elevation mask.
    Orbit { pos: DVec3 },
}

impl AntennaPose {
    fn pos(&self) -> DVec3 {
        match self {
            AntennaPose::Surface { pos, .. } | AntennaPose::Orbit { pos } => *pos,
        }
    }
    fn up(&self) -> Option<DVec3> {
        match self {
            AntennaPose::Surface { up, .. } => Some(*up),
            AntennaPose::Orbit { .. } => None,
        }
    }
}

/// Segment–sphere occlusion: does the open interior of `p1→p2` dip inside the
/// sphere at `center` with radius `radius_m`? Endpoints on (or above) the
/// surface never occlude themselves: the closest-approach parameter clamps to
/// the segment ends, which sit at ≥ radius. A small margin absorbs float noise
/// for horizon-grazing links.
pub fn segment_hits_sphere(p1: DVec3, p2: DVec3, center: DVec3, radius_m: f64) -> bool {
    let d = p2 - p1;
    let len_sq = d.length_squared();
    if len_sq < 1.0 {
        return false;
    }
    let t = ((center - p1).dot(d) / len_sq).clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return false;
    }
    let closest = p1 + d * t;
    (closest - center).length() < radius_m - 0.5
}

/// An antenna's position in the site's local scene frame (East=+X, Up=+Y,
/// North=−Z — the frame the terrain height oracle / `TerrainRaycast` use), or
/// `Unit` when the scene has no site anchor. Far endpoints (Earth/orbit) still
/// map to a valid, large local point, so a segment march from a surface
/// endpoint toward it carries the correct local direction.
fn local_hook(site: &Option<SiteFrameSnapshot>, solar: DVec3) -> lunco_hooks::HookValue {
    match site {
        Some(s) => {
            let l = s.frame.from_frame(solar);
            lunco_hooks::HookValue::Array(vec![
                lunco_hooks::HookValue::Float(l.x),
                lunco_hooks::HookValue::Float(l.y),
                lunco_hooks::HookValue::Float(l.z),
            ])
        }
        None => lunco_hooks::HookValue::Unit,
    }
}

/// Whether a pose sits on the site body (a surface antenna within the DEM's
/// world) — lets a terrain constraint skip pure space-to-space links that the
/// local relief can never occlude.
fn pose_on_site(pose: &AntennaPose, site: &Option<SiteFrameSnapshot>) -> bool {
    matches!((pose, site), (AntennaPose::Surface { body, .. }, Some(s)) if *body == s.body)
}

fn sanitize_port_token(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

/// Recomputes pairwise link state. Gated: runs on antenna topology change or
/// when the epoch advances ≥ 0.25 s (rover motion rides the same cadence).
#[allow(clippy::too_many_arguments)]
pub fn update_comms_links(
    world_time: Option<Res<WorldTime>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Option<Res<CelestialBodyRegistry>>,
    q_antennas: Query<(
        Entity,
        &CommsAntenna,
        Option<&GeodeticAnchor>,
        Option<&KeplerOrbit>,
        Option<&Name>,
    )>,
    q_changed: Query<
        (),
        Or<(
            Added<CommsAntenna>,
            Changed<CommsAntenna>,
            Changed<GeodeticAnchor>,
            Changed<KeplerOrbit>,
        )>,
    >,
    mut removed: RemovedComponents<CommsAntenna>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_states: Query<&CommsLinkState>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut links: ResMut<CommsLinks>,
    mut commands: Commands,
    mut last_jd: Local<f64>,
) {
    let (Some(world_time), Some(ephemeris), Some(registry)) = (world_time, ephemeris, registry)
    else {
        return;
    };
    if q_antennas.is_empty() {
        return;
    }

    let jd = world_time.epoch_jd;
    let topology_changed = !q_changed.is_empty() || removed.read().next().is_some();
    let epoch_advanced = (jd - *last_jd).abs() * 86_400.0 >= 0.25;
    if !topology_changed && !epoch_advanced && *last_jd != 0.0 {
        return;
    }
    *last_jd = jd;

    let body_of = |naif: i32| -> Option<&BodyDescriptor> {
        registry.bodies.iter().find(|b| b.ephemeris_id == naif)
    };
    let body_center = |naif: i32| -> DVec3 {
        ecliptic_to_bevy(ephemeris.provider.global_position(naif, jd))
    };

    // Site frame: the scene-root anchor, if authored.
    let site = q_site.iter().next().and_then(|anchor| {
        let desc = body_of(anchor.body)?;
        Some(SiteFrameSnapshot {
            body: anchor.body,
            frame: solar_tangent_frame(desc, &anchor.geodetic, body_center(anchor.body), jd),
        })
    });

    // Resolve every antenna's solar pose.
    let mut poses: Vec<(Entity, String, &CommsAntenna, AntennaPose)> = Vec::new();
    for (entity, antenna, anchor, orbit, name) in q_antennas.iter() {
        let label = antenna
            .id
            .clone()
            .or_else(|| name.map(|n| n.as_str().to_string()))
            .unwrap_or_else(|| format!("antenna_{}", entity.index()));
        let pose = if let Some(anchor) = anchor {
            let Some(desc) = body_of(anchor.body) else {
                warn!("[comms] {label}: unknown anchor body {}", anchor.body);
                continue;
            };
            let center = body_center(anchor.body);
            let pos = solar_position_of_geodetic(desc, &anchor.geodetic, center, jd);
            let up = (pos - center).normalize_or_zero();
            AntennaPose::Surface { pos, up, body: anchor.body }
        } else if let Some(orbit) = orbit {
            let Some(desc) = body_of(orbit.body) else {
                warn!("[comms] {label}: unknown orbit body {}", orbit.body);
                continue;
            };
            let pos = body_center(orbit.body) + orbit.elements.position_bevy_m(desc.gm, jd);
            AntennaPose::Orbit { pos }
        } else if let Some(site) = &site {
            // Scene-local: entity position in the local scene, mapped through
            // the site tangent frame.
            let Ok((cell, tf)) = q_spatial.get(entity) else { continue };
            let cell = cell.copied().unwrap_or_default();
            let local = lunco_core::coords::world_position_seeded(
                entity, &cell, tf, &q_parents, &q_grids, &q_spatial,
            );
            AntennaPose::Surface {
                pos: site.frame.to_frame(local),
                up: site.frame.up,
                body: site.body,
            }
        } else {
            // No way to place this antenna on a body — scene is unanchored.
            continue;
        };
        poses.push((entity, label, antenna, pose));
    }

    // Pairwise sight-lines.
    let mut new_links: Vec<SightLine> = Vec::new();
    for i in 0..poses.len() {
        for j in (i + 1)..poses.len() {
            let (ea, na, aa, pa) = &poses[i];
            let (eb, nb, ab, pb) = &poses[j];
            let (p1, p2) = (pa.pos(), pb.pos());
            let d = p2 - p1;
            let range_m = d.length();
            let dir = if range_m > 1e-6 { d / range_m } else { DVec3::ZERO };

            let elevation = |up: Option<DVec3>, dir: DVec3| -> Option<f64> {
                up.map(|u| u.dot(dir).clamp(-1.0, 1.0).asin().to_degrees())
            };
            let elev_a = elevation(pa.up(), dir);
            let elev_b = elevation(pb.up(), -dir);

            let mut occluded_by = None;
            for body in registry.bodies.iter().filter(|b| b.radius_m > 0.0) {
                if segment_hits_sphere(p1, p2, body_center(body.ephemeris_id), body.radius_m) {
                    occluded_by = Some(body.name.clone());
                    break;
                }
            }

            // The link RULE is policy, not kernel (doc 38): the
            // `comms.link.connected` hook decides — normally the authored
            // rhai rule (`assets/scripting/policy/comms_link.rhai`, entry
            // `link_connected(ctx)`), overridable per scenario via
            // `register_hook(...)`. The builtin range+mask+occlusion rule is
            // only the fallback for a missing/broken script. Same
            // None→builtin pattern as `rbac.authorize` / MergePolicy.
            let builtin = || {
                let in_range = range_m <= aa.max_range_m.min(ab.max_range_m);
                let mask_a = elev_a.map(|e| e >= aa.min_elevation_deg).unwrap_or(true);
                let mask_b = elev_b.map(|e| e >= ab.min_elevation_deg).unwrap_or(true);
                in_range && mask_a && mask_b && occluded_by.is_none()
            };
            let ctx = lunco_hooks::HookValue::map([
                ("a", lunco_hooks::HookValue::str(na.clone())),
                ("b", lunco_hooks::HookValue::str(nb.clone())),
                ("range_m", lunco_hooks::HookValue::Float(range_m)),
                // Orbiting antennas have no local horizon: elevation reads 90.
                ("elev_a", lunco_hooks::HookValue::Float(elev_a.unwrap_or(90.0))),
                ("elev_b", lunco_hooks::HookValue::Float(elev_b.unwrap_or(90.0))),
                ("min_elev_a", lunco_hooks::HookValue::Float(aa.min_elevation_deg)),
                ("min_elev_b", lunco_hooks::HookValue::Float(ab.min_elevation_deg)),
                ("occluded", lunco_hooks::HookValue::Bool(occluded_by.is_some())),
                (
                    "occluded_by",
                    lunco_hooks::HookValue::str(occluded_by.clone().unwrap_or_default()),
                ),
                (
                    "max_range_m",
                    lunco_hooks::HookValue::Float(aa.max_range_m.min(ab.max_range_m)),
                ),
                // Terrain-frame handles: local scene positions + on-site gate, so
                // a constraint can call `query("TerrainRaycast", …)` itself. This
                // is what makes the constraint set extensible over the GEOMETRY
                // engine (STK-plugin style), not just the flattened scalars.
                ("a_on_site", lunco_hooks::HookValue::Bool(pose_on_site(pa, &site))),
                ("b_on_site", lunco_hooks::HookValue::Bool(pose_on_site(pb, &site))),
                ("a_local", local_hook(&site, p1)),
                ("b_local", local_hook(&site, p2)),
            ]);
            let connected = match lunco_hooks::invoke(COMMS_LINK_HOOK, &[ctx]) {
                Some(Ok(v)) => match v.as_bool() {
                    Some(b) => b,
                    None => {
                        warn!("[comms] {COMMS_LINK_HOOK} returned non-bool {v:?}; using builtin rule");
                        builtin()
                    }
                },
                Some(Err(e)) => {
                    warn!("[comms] {COMMS_LINK_HOOK} hook error: {e:?}; using builtin rule");
                    builtin()
                }
                None => builtin(),
            };

            new_links.push(SightLine {
                a: *ea,
                b: *eb,
                a_name: na.clone(),
                b_name: nb.clone(),
                range_m,
                elevation_a_deg: elev_a,
                elevation_b_deg: elev_b,
                occluded_by,
                connected,
            });
        }
    }

    // Earth-route BFS over connected links (sources: Earth-anchored antennas).
    let mut hops: HashMap<Entity, u32> = poses
        .iter()
        .filter(|(_, _, _, pose)| matches!(pose, AntennaPose::Surface { body: 399, .. }))
        .map(|(e, ..)| (*e, 0u32))
        .collect();
    loop {
        let mut grew = false;
        for link in new_links.iter().filter(|l| l.connected) {
            for (from, to) in [(link.a, link.b), (link.b, link.a)] {
                if let Some(&d) = hops.get(&from) {
                    if !hops.contains_key(&to) {
                        hops.insert(to, d + 1);
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }

    // Write per-antenna state + emit AOS/LOS edges vs the previous state.
    for (entity, name, _, _) in &poses {
        let mut peers = Vec::new();
        for link in &new_links {
            let (peer_entity, peer, elev) = if link.a == *entity {
                (link.b, link.b_name.clone(), link.elevation_a_deg)
            } else if link.b == *entity {
                (link.a, link.a_name.clone(), link.elevation_b_deg)
            } else {
                continue;
            };
            peers.push(PeerLink {
                peer,
                peer_entity,
                connected: link.connected,
                range_m: link.range_m,
                elevation_deg: elev,
                occluded_by: link.occluded_by.clone(),
            });
        }
        let state = CommsLinkState { peers, earth_hops: hops.get(entity).copied() };

        if let Ok(prev) = q_states.get(*entity) {
            let prev_connected: HashSet<&str> = prev
                .peers
                .iter()
                .filter(|p| p.connected)
                .map(|p| p.peer.as_str())
                .collect();
            for peer in &state.peers {
                let was = prev_connected.contains(peer.peer.as_str());
                if peer.connected != was {
                    commands.trigger(TelemetryEvent {
                        name: if peer.connected { "comms.aos" } else { "comms.los" }.to_string(),
                        source: 0,
                        severity: Severity::Info,
                        data: TelemetryValue::String(format!("{name}<->{}", peer.peer)),
                        timestamp: jd,
                    });
                }
            }
        }
        commands.entity(*entity).insert(state);
    }

    links.epoch_jd = jd;
    links.positions = poses.iter().map(|(e, _, _, pose)| (*e, pose.pos())).collect();
    links.links = new_links;
    links.site = site;
}

// ─────────────────────────────────────────────────────────────────────────────
// Ports backend: comms:<peer>:connected|range_m|elevation_deg + route ports
// ─────────────────────────────────────────────────────────────────────────────

fn comms_list(world: &World, entity: Entity, out: &mut Vec<PortRef>) {
    let Some(state) = world.get::<CommsLinkState>(entity) else { return };
    for peer in &state.peers {
        let token = sanitize_port_token(&peer.peer);
        out.push(PortRef {
            name: format!("comms:{token}:connected"),
            direction: PortDirection::Out,
            value: if peer.connected { 1.0 } else { 0.0 },
        });
        out.push(PortRef {
            name: format!("comms:{token}:range_m"),
            direction: PortDirection::Out,
            value: peer.range_m,
        });
        if let Some(elev) = peer.elevation_deg {
            out.push(PortRef {
                name: format!("comms:{token}:elevation_deg"),
                direction: PortDirection::Out,
                value: elev,
            });
        }
    }
    out.push(PortRef {
        name: "comms:route_earth:connected".to_string(),
        direction: PortDirection::Out,
        value: if state.earth_hops.is_some() { 1.0 } else { 0.0 },
    });
    out.push(PortRef {
        name: "comms:route_earth:hops".to_string(),
        direction: PortDirection::Out,
        value: state.earth_hops.map(|h| h as f64).unwrap_or(-1.0),
    });
}

fn comms_read_output(world: &World, entity: Entity, name: &str) -> Option<f64> {
    let state = world.get::<CommsLinkState>(entity)?;
    let rest = name.strip_prefix("comms:")?;
    if rest == "route_earth:connected" {
        return Some(if state.earth_hops.is_some() { 1.0 } else { 0.0 });
    }
    if rest == "route_earth:hops" {
        return Some(state.earth_hops.map(|h| h as f64).unwrap_or(-1.0));
    }
    let (peer_token, field) = rest.rsplit_once(':')?;
    let peer = state
        .peers
        .iter()
        .find(|p| sanitize_port_token(&p.peer) == peer_token)?;
    match field {
        "connected" => Some(if peer.connected { 1.0 } else { 0.0 }),
        "range_m" => Some(peer.range_m),
        "elevation_deg" => peer.elevation_deg,
        _ => None,
    }
}

const COMMS_BACKEND: PortBackend = PortBackend {
    list: comms_list,
    read_output: comms_read_output,
    read_input: |_, _, _| None,
    write_input: |_, _, _, _| false,
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Comms connectivity: components, the link-update system, and the ports
/// backend. Headless-safe; needs `WorldTime` (lunco-time) and — for real
/// geometry — a real `EphemerisResource` (add `EphemerisPlugin`). Inserts the
/// default body registry / NoOp ephemeris only if absent, mirroring
/// `CelestialPlugin`.
pub struct CommsPlugin;

impl Plugin for CommsPlugin {
    fn build(&self, app: &mut App) {
        if app.world().get_resource::<CelestialBodyRegistry>().is_none() {
            app.insert_resource(CelestialBodyRegistry::default_system());
        }
        if app.world().get_resource::<EphemerisResource>().is_none() {
            app.insert_resource(EphemerisResource {
                provider: std::sync::Arc::new(crate::ephemeris::NoOpEphemerisProvider),
            });
        }
        app.register_type::<CommsAntenna>();
        app.register_type::<CommsLinkState>();
        app.register_type::<GeodeticAnchor>();
        app.register_type::<SiteAnchor>();
        app.register_type::<KeplerOrbit>();
        app.init_resource::<CommsLinks>();

        let mut registry = app
            .world_mut()
            .get_resource_or_insert_with(PortRegistry::default);
        registry.register(COMMS_BACKEND);

        app.add_systems(Update, update_comms_links);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_sphere_blocks_through_center_but_not_grazing_out() {
        let r = 1737.0e3;
        // Antipodal surface points: blocked.
        assert!(segment_hits_sphere(
            DVec3::new(r, 0.0, 0.0),
            DVec3::new(-r, 0.0, 0.0),
            DVec3::ZERO,
            r
        ));
        // Surface point looking outward at +5° elevation toward a far target:
        // closest approach clamps to the endpoint → clear.
        let up = DVec3::X;
        let dir = (up * 5f64.to_radians().sin()
            + DVec3::Y * 5f64.to_radians().cos())
        .normalize();
        assert!(!segment_hits_sphere(up * r, up * r + dir * 1.0e9, DVec3::ZERO, r));
        // Same target at −5°: the segment dips below the surface → blocked.
        let dir_down = (-up * 5f64.to_radians().sin()
            + DVec3::Y * 5f64.to_radians().cos())
        .normalize();
        assert!(segment_hits_sphere(up * r, up * r + dir_down * 1.0e9, DVec3::ZERO, r));
    }

    #[test]
    fn port_token_sanitizes() {
        assert_eq!(sanitize_port_token("DSS Madrid-63"), "dss_madrid_63");
        assert_eq!(sanitize_port_token("RelaySat"), "relaysat");
    }
}
