//! Generic celestial geometry queries — domain-free spatial answers exposed to
//! the API / scripting surface via `query(name, params)`, the same mechanism as
//! the terrain providers (`TerrainHeight` / `TerrainRaycast`).
//!
//! These are the geometry substrate that AUTHORED subsystems (comms, solar,
//! thermal) compose over; **none of them names a domain** (doc 49). They reuse the
//! ephemeris, the body registry, and the analytic `segment_hits_sphere` occlusion —
//! so a link-availability or sun-exposure rule is authored in rhai over
//! `query("Occultation", …)` / `query("Links", …)` with no comms or solar Rust.

use bevy::ecs::query::QueryState;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::{ApiQueryError, ApiQueryResult, api_param_u64};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_core::GlobalEntityId;
use lunco_time::CelestialTime;

use crate::link::node_label;
use lunco_celestial::CelestialBodyRegistry;
use lunco_celestial::coords::ecliptic_to_bevy;
use lunco_celestial::ephemeris::EphemerisResource;
use lunco_celestial::geo::segment_hits_sphere;
use lunco_celestial_spatial_core::{LinkNode, LinkState, WifiNode, WifiState};

/// Read a `[x,y,z]` array or `{x,y,z}` map into a solar-frame [`DVec3`].
fn parse_point(v: Option<&ApiValue>) -> Option<DVec3> {
    let v = v?;
    if let ApiValue::Array(a) = v {
        if a.len() < 3 {
            return None;
        }
        return Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?));
    }
    Some(DVec3::new(
        v.get("x")?.as_f64()?,
        v.get("y")?.as_f64()?,
        v.get("z")?.as_f64()?,
    ))
}

/// `Occultation` — does a celestial body block the segment `origin→target`?
/// Analytic ray–sphere over the body registry at the current epoch, in the
/// solar frame (metres). This is the generic occlusion primitive a comms link
/// or a sun-exposure test composes; it knows nothing about antennas.
///
/// params: `{ origin:[x,y,z], target:[x,y,z] }` (also accepts `{x,y,z}` maps).
/// returns: `{ occluded, by }` — `by` = blocking body name, or null when clear.
/// Clear (`occluded:false`) when no ephemeris/registry/clock is present.
pub struct OccultationProvider;

impl ApiQueryProvider for OccultationProvider {
    fn name(&self) -> &'static str {
        "Occultation"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let (Some(o), Some(t)) = (
            parse_point(params.get("origin")),
            parse_point(params.get("target")),
        ) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "Occultation: `origin` and `target` [x,y,z] required",
            ));
        };
        let Some(jd) = world
            .get_resource::<CelestialTime>()
            .map(|time| time.epoch_jd)
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "Occultation: CelestialTime is not installed",
            ));
        };
        let Some(eph) = world.get_resource::<EphemerisResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "Occultation: ephemeris is not installed",
            ));
        };
        let Some(reg) = world.get_resource::<CelestialBodyRegistry>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "Occultation: celestial body registry is not installed",
            ));
        };
        let mut by: Option<String> = None;
        for b in reg.bodies.iter().filter(|b| b.radius_m > 0.0) {
            // A body we cannot place cannot block a line of sight.
            let Some(p) = eph.provider.global_position(b.ephemeris_id, jd) else {
                continue;
            };
            let center = ecliptic_to_bevy(p).raw();
            if segment_hits_sphere(o, t, center, b.radius_m) {
                by = Some(b.name.clone());
                break;
            }
        }
        Ok(Some(api_value!({ "occluded": by.is_some(), "by": by })))
    }
}

/// `BodyPosition` — solar-frame position + radius of a registry body at the
/// current epoch, so an authored subsystem can compute range / direction /
/// elevation itself.
///
/// params: `{ body: <NAIF id> }`. returns: `{ found, pos:[x,y,z], radius }`.
pub struct BodyPositionProvider;

impl ApiQueryProvider for BodyPositionProvider {
    fn name(&self) -> &'static str {
        "BodyPosition"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(naif) = params.get("body").and_then(ApiValue::as_i64) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "BodyPosition: `body` (NAIF id) required",
            ));
        };
        let Ok(naif) = i32::try_from(naif) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "BodyPosition: `body` must fit a signed 32-bit NAIF identifier",
            ));
        };
        let Some(jd) = world
            .get_resource::<CelestialTime>()
            .map(|time| time.epoch_jd)
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "BodyPosition: CelestialTime is not installed",
            ));
        };
        let Some(eph) = world.get_resource::<EphemerisResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "BodyPosition: ephemeris is not installed",
            ));
        };
        let Some(radius) = world.get_resource::<CelestialBodyRegistry>().and_then(|r| {
            r.bodies
                .iter()
                .find(|b| b.ephemeris_id == naif)
                .map(|b| b.radius_m)
        }) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("BodyPosition: NAIF {naif} is not in the celestial body registry"),
            ));
        };
        let Some(p) = eph.provider.global_position(naif, jd) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("no ephemeris for NAIF {naif}"),
            ));
        };
        let p = ecliptic_to_bevy(p).raw();
        Ok(Some(api_value!({
            "found": true,
            "pos": api_value!([p.x, p.y, p.z]),
            "radius": radius,
        })))
    }
}

/// `SolarPose` — an entity's solar-frame position (and local up, for surface
/// points) from its `GeodeticAnchor` (ground stations) or `KeplerOrbit`
/// (satellites). Domain-free celestial placement: an authored subsystem uses it
/// to compute range / direction / elevation, then composes `Occultation` /
/// `TerrainRaycast`. Generalizes cleanly to LEO / lunar-orbit relays — a
/// satellite is just a `KeplerOrbit` endpoint.
///
/// params: `{ entity: <gid> }`. returns: `{ found, kind, body, pos:[x,y,z], up }`
/// — `up` is `[x,y,z]` for a surface point, null for an orbit. **Scene-local**
/// antennas (placed through the site frame with no own anchor/orbit) need the
/// big_space system context, so they resolve via the pose SYSTEM, not this query
/// (`{found:false, reason:"pose_unavailable"}` here). The pose system is the
/// sole owner of the f64-to-grid-local conversion; this query never recomputes
/// an alternate pose from anchor/orbit components.
pub struct SolarPoseProvider;

impl ApiQueryProvider for SolarPoseProvider {
    fn name(&self) -> &'static str {
        "SolarPose"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(gid) = api_param_u64(params, "entity") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "SolarPose: `entity` (gid) required",
            ));
        };
        let not_found = || Ok(Some(api_value!({ "found": false })));
        let Some(target) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(gid)))
        else {
            return not_found();
        };
        // Prefer the system-written pose — it covers scene-local prims (a moving
        // rover antenna) that this read-only path can't resolve. Falls through to
        // the inline anchor/orbit compute for entities not yet posed.
        if let Some(p) = world.get::<crate::pose::SolarFramePose>(target).copied() {
            // `kind` and `up` are one decision, taken once: a free-flyer has no
            // vertical, and says so with a null rather than a zero vector a caller
            // could mistake for a direction.
            let (kind, up) = match p.horizon {
                crate::pose::Horizon::Surface { up, .. } => {
                    ("surface", api_value!([up.x, up.y, up.z]))
                }
                crate::pose::Horizon::Free { .. } => ("orbit", ApiValue::Unit),
            };
            return Ok(Some(api_value!({
                "found": true,
                "kind": kind,
                "body": p.body(),
                "pos": api_value!([p.pos.x, p.pos.y, p.pos.z]),
                "local": api_value!([p.local.x, p.local.y, p.local.z]),
                "up": up,
            })));
        }
        Ok(Some(api_value!({
            "found": false,
            "reason": "pose_unavailable",
        })))
    }
}

/// `Links` — a snapshot of the live connectivity graph the kernel maintains, as
/// plain DATA for scripts to route over. No traversal here: the core stays a pure
/// cadence-gated geometry sweep, and reachability / preferred-path routing is
/// authored in rhai over this snapshot **on demand** (at decision time — not per
/// tick), which is where connectivity policy belongs.
///
/// Nodes are identified by **GID** — the same `u64` `find()` returns, the API puts
/// on the wire, and every other entity-shaped surface here speaks. Names and
/// classes are labels carried alongside.
///
/// params: none. returns
/// ```text
/// { nodes:  [{ id, name, class }],           # id = GID
///   adj:    { "<gid>": [gid,…] },            # UP links only; keys stringified (JSON)
///   edges:  [{ a, b, range_m, light_time_s }],   # deduped, undirected; a/b = GIDs
///   groups: { class: [gid,…] },              # authored roles → their members
///   owners: { "<gid>": [gid,…] } }           # containers → the nodes inside them
/// ```
/// `light_time_s` is the one-way propagation delay (1.28 s Earth↔Moon).
///
/// `groups` is what keeps ROLE routing working now that identity is per-node:
/// three DSN complexes have three distinct GIDs and all three appear under
/// `groups["earth"]`, so "can this rover reach Earth?" is a question about the
/// group, while "what is Madrid's range?" is a question about the node. Keying the
/// graph on the shared class collapsed the three into one and made only the last
/// one answerable. See [`LinkPeer::peer`](lunco_celestial_spatial_core::LinkPeer::peer).
///
/// `owners` is the same idea for CONTAINMENT. A link node sits where the link
/// geometry physically is — on the dish's feed phase centre, so the beam, the RF
/// state and the range verdict share one transform — which is several levels below
/// the prim a scene author names. `find("/Rover/Comms")` returns the antenna
/// assembly's GID; the node's own GID belongs to
/// `Comms/YawHead/DishGimbal/DishHead/LinkAperture`. Publishing each node's
/// ancestor chain lets a GID denote the node it contains, so addressing follows the
/// hierarchy the scene actually authors instead of the mechanism's internals. An
/// ancestor holding two antennas denotes both, exactly as a class denotes a group.
pub struct LinksProvider;

impl ApiQueryProvider for LinksProvider {
    fn name(&self) -> &'static str {
        "Links"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let mut nodes: Vec<ApiValue> = Vec::new();
        let mut adj = Vec::new();
        let mut edges: Vec<ApiValue> = Vec::new();
        // class → the GIDs that carry it, so a role stays routable now that
        // identity is per-node (see the type doc).
        let mut groups: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
        // ancestor GID → the nodes beneath it (see the type doc). Filled after the
        // sweep, from the entities collected below.
        let mut owners: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
        let mut node_entities: Vec<(Entity, u64)> = Vec::new();
        let Some(mut q) = QueryState::<(
            Entity,
            Option<&Name>,
            &LinkNode,
            &LinkState,
            &GlobalEntityId,
        )>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "Links: ECS query is unavailable",
            ));
        };
        for (e, name, node, state, gid) in q.iter(world) {
            let id = gid.get();
            node_entities.push((e, id));
            let label = node_label(node.class.as_deref(), name, e);
            if let Some(class) = node.class.as_deref().filter(|c| !c.is_empty()) {
                groups.entry(class.to_string()).or_default().push(id);
            }
            let mut peers: Vec<u64> = Vec::new();
            for peer in state.peers.iter().filter(|p| p.connected) {
                if !peers.contains(&peer.peer) {
                    peers.push(peer.peer);
                }
                // Dedup the undirected edge (each pair is listed from both sides).
                if id <= peer.peer {
                    edges.push(api_value!({
                        "a": id, "b": peer.peer, "range_m": peer.range_m,
                        "light_time_s": peer.light_time_s,
                    }));
                }
            }
            // JSON object keys are strings, so the adjacency is keyed by the GID
            // stringified; `links.rhai` converts once, in `neighbours()`.
            adj.push((id.to_string(), api_value!(peers)));
            nodes.push(api_value!({
                "id": id,
                "name": label,
                "class": node.class.clone().unwrap_or_default(),
            }));
        }
        // Containment, walked once per node from the node up to the scene root.
        // Only ancestors that carry a GID are recorded: a GID is what `find()`
        // returns, so an ancestor without one is not addressable and has nothing to
        // resolve from.
        for (entity, id) in node_entities {
            let mut cur = entity;
            while let Some(child_of) = world.get::<ChildOf>(cur) {
                let parent = child_of.parent();
                if let Some(pgid) = world.get::<GlobalEntityId>(parent) {
                    owners.entry(pgid.get().to_string()).or_default().push(id);
                }
                cur = parent;
            }
        }
        let groups = groups
            .into_iter()
            .map(|(class, ids)| (class, api_value!(ids)))
            .collect::<Vec<_>>();
        let owners = owners
            .into_iter()
            .map(|(owner, ids)| (owner, api_value!(ids)))
            .collect::<Vec<_>>();
        Ok(Some(api_value!({
            "nodes": nodes,
            "adj": ApiValue::map(adj),
            "edges": edges,
            "groups": ApiValue::map(groups),
            "owners": ApiValue::map(owners),
        })))
    }
}

/// `WifiLinks` — the separate short-range radio graph. It is projected from
/// raw link geometry and never reads the direct-link `Links` policy state.
pub struct WifiLinksProvider;

impl ApiQueryProvider for WifiLinksProvider {
    fn name(&self) -> &'static str {
        "WifiLinks"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let mut nodes = Vec::new();
        let mut adj = Vec::new();
        let mut edges = Vec::new();
        let Some(mut q) =
            QueryState::<(&GlobalEntityId, &WifiNode, &WifiState, Option<&Name>)>::try_new(world)
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "WifiLinks: ECS query is unavailable",
            ));
        };
        for (gid, wifi, state, name) in q.iter(world) {
            let id = gid.get();
            let peers: Vec<u64> = state
                .peers
                .iter()
                .filter(|peer| peer.connected)
                .map(|peer| peer.peer)
                .collect();
            for peer in state.peers.iter().filter(|peer| peer.connected) {
                if id <= peer.peer {
                    edges.push(api_value!({
                        "a": id,
                        "b": peer.peer,
                        "range_m": peer.range_m,
                        "light_time_s": peer.light_time_s,
                    }));
                }
            }
            adj.push((id.to_string(), api_value!(peers)));
            nodes.push(api_value!({
                "id": id,
                "name": name.map(|name| name.as_str()).unwrap_or_default(),
                "max_range_m": wifi.max_range_m,
            }));
        }
        Ok(Some(api_value!({
            "nodes": nodes,
            "adj": ApiValue::map(adj),
            "edges": edges,
        })))
    }
}

/// Register the generic celestial geometry providers into the [`ApiQueryRegistry`]
/// (init-if-absent, mirroring `register_terrain_queries`). Called from
/// [`CelestialPlugin`](crate::CelestialPlugin) — these are generic geometry that
/// authored subsystems (comms/solar/thermal) compose over, not a domain.
pub fn register_celestial_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(OccultationProvider);
    reg.register(BodyPositionProvider);
    reg.register(SolarPoseProvider);
    reg.register(LinksProvider);
    reg.register(WifiLinksProvider);
}
