//! Generic connectivity kernel — the domain-free MECHANISM behind links.
//!
//! The heavy work lives here in Rust (and thus serves every scripting language):
//! a cadence-gated pairwise sweep over [`LinkNode`] entities that computes the
//! geometry — range, local elevation, analytic body occlusion, and terrain
//! occlusion (via the generic `TerrainRaycast` query) — then asks a
//! **language-neutral verdict hook** ([`LINK_HOOK`]) whether each pair is a usable
//! link. Scripts supply only that verdict (a pure boolean over precomputed
//! geometry — no loops, no queries), so a rhai / Python / Luau policy is minimal.
//!
//! Nothing here is "comms": nodes, links, and the verdict are generic. A comms
//! (or sensor, or relay) domain is authored on top — roles, routing, and naming
//! live in script over the [`LinkState`] this writes and the `link.aos`/`link.los`
//! events it emits.
//!
//! The recompute cadence is a **runtime parameter** ([`LinkConfig::interval_s`]),
//! tuned live via the [`SetLinkCadence`] command — never a build constant.

use bevy::math::DVec3;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use lunco_core::{on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue};
use lunco_hooks::HookValue;
use lunco_time::WorldTime;

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::segment_hits_sphere;
use crate::pose::SolarFramePose;
use crate::registry::CelestialBodyRegistry;

/// Runtime-tunable connectivity cadence. Links change slowly (bodies/rovers move
/// metres per second), so recompute at an interval, not every physics tick.
/// Change it live with [`SetLinkCadence`] — it is NOT a build constant.
#[derive(Resource, Debug, Clone, Reflect)]
#[reflect(Resource)]
pub struct LinkConfig {
    /// Seconds of sim time between recomputes (`0` = every tick).
    pub interval_s: f64,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self { interval_s: 0.25 }
    }
}

/// A generic connectivity endpoint. The pose system tracks it (so it has a
/// [`SolarFramePose`]); the kernel pairs it with every other node. `class` is an
/// authored role the verdict/routing policy reads — the core never interprets it.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct LinkNode {
    pub max_range_m: f64,
    pub min_elevation_deg: f64,
    pub class: Option<String>,
}

impl Default for LinkNode {
    fn default() -> Self {
        Self { max_range_m: 1.0e12, min_elevation_deg: -90.0, class: None }
    }
}

/// One node's resolved peer links, written by [`update_links`]. Consumers read it
/// (or subscribe to the AOS/LOS events); routing is authored over this.
#[derive(Component, Debug, Clone, Default)]
pub struct LinkState {
    pub peers: Vec<LinkPeer>,
}

#[derive(Debug, Clone)]
pub struct LinkPeer {
    pub peer: String,
    pub connected: bool,
    pub range_m: f64,
    pub elevation_deg: f64,
}

/// The verdict seam consulted per pair. `ctx` (a [`HookValue`] map): `a`, `b`,
/// `class_a`, `class_b`, `range_m`, `elev_a`, `elev_b`, `min_elev_a`,
/// `min_elev_b`, `occluded`, `occluded_by`, `terrain_blocked`, `max_range_m`.
/// Return bool. No hook → the builtin range+mask+occlusion rule.
pub const LINK_HOOK: &str = "link.connected";

/// Set the connectivity recompute cadence at runtime (any client / language).
#[Command(default)]
pub struct SetLinkCadence {
    pub interval_s: f64,
}

#[on_command(SetLinkCadence)]
fn on_set_link_cadence(cmd: SetLinkCadence, mut config: ResMut<LinkConfig>) {
    config.interval_s = cmd.interval_s.max(0.0);
    info!("[link] recompute cadence set to {} s", config.interval_s);
}

register_commands!(on_set_link_cadence);

/// Kernel scratch: last recompute epoch + the previous tick's live link set (for
/// AOS/LOS edges).
#[derive(Resource, Default)]
pub(crate) struct LinkSolverState {
    last_jd: f64,
    prev_up: HashSet<String>,
}

/// One resolved node, snapshotted so the world borrow is free for the terrain
/// query provider.
struct Node {
    entity: Entity,
    name: String,
    node: LinkNode,
    pose: SolarFramePose,
}

/// The cadence-gated pairwise connectivity sweep. A REGULAR system on purpose:
/// it writes through `Commands`, so it adds NO extra command-flush sync point. An
/// earlier EXCLUSIVE version (to call the `TerrainRaycast` provider with
/// `&mut World`) inserted a sync point that interleaved with the twin/terrain
/// despawns and tripped avian's island bookkeeping (`island.body_count > 0`).
/// Terrain occlusion therefore moves to the verdict policy / a future cached
/// oracle, not an in-loop world query.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_links(
    config: Option<Res<LinkConfig>>,
    world_time: Option<Res<WorldTime>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Option<Res<CelestialBodyRegistry>>,
    q_nodes: Query<(Entity, &LinkNode, &SolarFramePose, Option<&Name>)>,
    mut q_state: Query<&mut LinkState>,
    mut state: Local<LinkSolverState>,
    mut commands: Commands,
) {
    let (Some(config), Some(world_time)) = (config, world_time) else {
        return;
    };
    let jd = world_time.epoch_jd;
    if q_nodes.iter().count() < 2 {
        return;
    }
    let advanced = (jd - state.last_jd).abs() * 86_400.0 >= config.interval_s;
    if !advanced && state.last_jd != 0.0 {
        return;
    }
    state.last_jd = jd;

    let nodes: Vec<Node> = q_nodes
        .iter()
        .map(|(e, n, p, name)| {
            Node {
                entity: e,
                name: n.class.clone().unwrap_or_default(),
                node: n.clone(),
                pose: *p,
            }
            .named(name, e)
        })
        .collect();

    // Body centers for analytic occlusion.
    let bodies: Vec<(String, DVec3, f64)> = match (ephemeris.as_deref(), registry.as_deref()) {
        (Some(eph), Some(reg)) => reg
            .bodies
            .iter()
            .filter(|b| b.radius_m > 0.0)
            .map(|b| {
                (
                    b.name.clone(),
                    ecliptic_to_bevy(eph.provider.global_position(b.ephemeris_id, jd)),
                    b.radius_m,
                )
            })
            .collect(),
        _ => Vec::new(),
    };

    // Pairwise verdicts.
    let mut per_node: HashMap<Entity, Vec<LinkPeer>> = HashMap::new();
    let mut up_now: HashSet<String> = HashSet::new();
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let (a, b) = (&nodes[i], &nodes[j]);
            let d = b.pose.pos - a.pose.pos;
            let range_m = d.length();
            let dir = if range_m > 1e-6 { d / range_m } else { DVec3::ZERO };
            let elev = |up: DVec3, dir: DVec3| -> f64 {
                if up == DVec3::ZERO {
                    90.0
                } else {
                    up.dot(dir).clamp(-1.0, 1.0).asin().to_degrees()
                }
            };
            let elev_a = elev(a.pose.up, dir);
            let elev_b = elev(b.pose.up, -dir);

            let occluded_by = bodies
                .iter()
                .find(|(_, c, r)| segment_hits_sphere(a.pose.pos, b.pose.pos, *c, *r))
                .map(|(n, _, _)| n.clone());

            let cheap_ok = range_m <= a.node.max_range_m.min(b.node.max_range_m)
                && elev_a >= a.node.min_elevation_deg
                && elev_b >= b.node.min_elevation_deg
                && occluded_by.is_none();
            // Terrain occlusion is left to the verdict policy / a future cached
            // oracle so the kernel stays non-exclusive (an in-loop provider query
            // needed `&mut World` → an exclusive system → the avian island crash).
            let terrain_blocked = false;

            let ctx = HookValue::map([
                ("a", HookValue::str(a.name.clone())),
                ("b", HookValue::str(b.name.clone())),
                ("class_a", HookValue::str(a.node.class.clone().unwrap_or_default())),
                ("class_b", HookValue::str(b.node.class.clone().unwrap_or_default())),
                ("range_m", HookValue::Float(range_m)),
                ("elev_a", HookValue::Float(elev_a)),
                ("elev_b", HookValue::Float(elev_b)),
                ("min_elev_a", HookValue::Float(a.node.min_elevation_deg)),
                ("min_elev_b", HookValue::Float(b.node.min_elevation_deg)),
                ("occluded", HookValue::Bool(occluded_by.is_some())),
                ("occluded_by", HookValue::str(occluded_by.clone().unwrap_or_default())),
                ("terrain_blocked", HookValue::Bool(terrain_blocked)),
                ("max_range_m", HookValue::Float(a.node.max_range_m.min(b.node.max_range_m))),
            ]);
            let builtin = cheap_ok && !terrain_blocked;
            let connected = match lunco_hooks::invoke(LINK_HOOK, &[ctx]) {
                Some(Ok(v)) => v.as_bool().unwrap_or(builtin),
                _ => builtin,
            };

            if connected {
                up_now.insert(pair_key(&a.name, &b.name));
            }
            per_node.entry(a.entity).or_default().push(LinkPeer {
                peer: b.name.clone(),
                connected,
                range_m,
                elevation_deg: elev_a,
            });
            per_node.entry(b.entity).or_default().push(LinkPeer {
                peer: a.name.clone(),
                connected,
                range_m,
                elevation_deg: elev_b,
            });
        }
    }

    debug!("[link] recompute: {} nodes, {} links up", nodes.len(), up_now.len());

    // AOS/LOS edges vs the previous recompute.
    for key in up_now.difference(&state.prev_up) {
        commands.trigger(link_event("link.aos", key, jd));
    }
    for key in state.prev_up.difference(&up_now) {
        commands.trigger(link_event("link.los", key, jd));
    }
    state.prev_up = up_now;

    // Publish per-node state — update in place, else insert.
    for node in &nodes {
        let peers = per_node.remove(&node.entity).unwrap_or_default();
        if let Ok(mut st) = q_state.get_mut(node.entity) {
            st.peers = peers;
        } else {
            commands.entity(node.entity).insert(LinkState { peers });
        }
    }
}

impl Node {
    fn named(mut self, name: Option<&Name>, e: Entity) -> Self {
        if self.name.is_empty() {
            self.name = name
                .map(|n| n.as_str().to_string())
                .unwrap_or_else(|| format!("node_{}", e.index()));
        }
        self
    }
}

fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}<->{b}")
    } else {
        format!("{b}<->{a}")
    }
}

fn link_event(name: &str, key: &str, jd: f64) -> TelemetryEvent {
    TelemetryEvent {
        name: name.to_string(),
        source: 0,
        severity: Severity::Info,
        data: TelemetryValue::String(key.to_string()),
        timestamp: jd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn node(world: &mut World, class: &str, pos: DVec3, max_range: f64) -> Entity {
        world
            .spawn((
                LinkNode { max_range_m: max_range, min_elevation_deg: -90.0, class: Some(class.into()) },
                SolarFramePose { pos, local: pos, up: DVec3::Y, body: 301 },
            ))
            .id()
    }

    fn world_at_epoch(interval_s: f64) -> World {
        let mut world = World::new();
        world.insert_resource(lunco_time::WorldTime { epoch_jd: 2_451_545.0, ..Default::default() });
        world.insert_resource(LinkConfig { interval_s });
        world
    }

    #[test]
    fn kernel_pairs_nodes_and_writes_link_state() {
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "relay", DVec3::new(10.0, 0.0, 0.0), 1.0e12);

        world.run_system_once(update_links).unwrap();

        let sa = world.get::<LinkState>(a).expect("node a has LinkState");
        let peer = sa.peers.iter().find(|p| p.peer == "relay").expect("a sees relay");
        assert!(peer.connected, "a clear 10 m link should be up: {peer:?}");
        assert!((peer.range_m - 10.0).abs() < 1e-6, "range {}", peer.range_m);
    }

    #[test]
    fn kernel_breaks_link_beyond_range() {
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "a", DVec3::ZERO, 5.0); // max range 5 m
        node(&mut world, "b", DVec3::new(10.0, 0.0, 0.0), 5.0); // 10 m away → out
        world.run_system_once(update_links).unwrap();
        let sa = world.get::<LinkState>(a).unwrap();
        assert!(
            sa.peers.iter().all(|p| !p.connected),
            "beyond range → down: {:?}",
            sa.peers
        );
    }

    #[test]
    fn kernel_elevation_mask_severs_below_horizon() {
        let mut world = world_at_epoch(0.0);
        // `a` demands peers ≥ 30° above its local horizon (up = +Y).
        let a = world
            .spawn((
                LinkNode { max_range_m: 1.0e12, min_elevation_deg: 30.0, class: None },
                SolarFramePose { pos: DVec3::ZERO, local: DVec3::ZERO, up: DVec3::Y, body: 301 },
            ))
            .id();
        // `b` sits on the horizon (elevation 0°) → below the 30° mask.
        node(&mut world, "b", DVec3::new(10.0, 0.0, 0.0), 1.0e12);
        world.run_system_once(update_links).unwrap();
        let sa = world.get::<LinkState>(a).unwrap();
        assert!(sa.peers.iter().all(|p| !p.connected), "0° < 30° mask → down");
    }
}
