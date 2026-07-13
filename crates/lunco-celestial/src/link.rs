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
//!
//! # Propagation delay
//!
//! Every link publishes [`LinkPeer::light_time_s`] = `range_m / c`
//! ([`SPEED_OF_LIGHT_M_PER_S`]) alongside its range. Earth↔Moon one-way light
//! time is **1.28 s** (2.56 s round trip) — the dominant constraint on the
//! teleoperation scenarios this simulator exists to study, so it is a
//! first-class output rather than something each consumer re-derives.
//!
//! # What the geometry does NOT model (say it, don't leave it silent)
//!
//! Ephemeris positions are **geometric at the epoch**: the state of every body
//! and node is taken at the same instant `jd`.
//!
//! - **No light-time correction.** Range/elevation/occlusion are computed
//!   between simultaneous positions, not between a receiver now and a
//!   transmitter 1.28 s ago. The two differ by the emitter's motion over the
//!   light time: ~1 km for the Moon about the Earth (≈ 1 km/s), which is
//!   3e-6 rad at lunar range — far below any antenna beamwidth here, and it does
//!   NOT accumulate. `light_time_s` gives consumers the DELAY (which matters
//!   enormously); the geometry is uncorrected (which does not).
//! - **No stellar aberration.** ≈ 20.5″ (1e-4 rad) — negligible for lighting and
//!   pointing at these beamwidths.
//! - **No relativistic delay** (Shapiro ≈ tens of ns) and no ionospheric or
//!   tropospheric path delay.
//!
//! If this simulator ever grows radiometric navigation or two-way ranging
//! residuals, those are the terms to add, and they belong here.

use bevy::math::{DVec3, Vec3A};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lunco_core::{on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue};
use lunco_hooks::HookValue;
use lunco_terrain_surface::{DemHeightField, SurfaceOracle};
use lunco_time::WorldTime;

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::segment_hits_sphere;
use crate::pose::SolarFramePose;
use crate::registry::CelestialBodyRegistry;

/// Speed of light in vacuum, m/s — the SI definition (exact).
pub const SPEED_OF_LIGHT_M_PER_S: f64 = 299_792_458.0;

/// One-way propagation delay over `range_m`, seconds.
///
/// Earth↔Moon ≈ 1.28 s; a lunar surface relay hop ≈ microseconds.
#[inline]
pub fn light_time_s(range_m: f64) -> f64 {
    range_m / SPEED_OF_LIGHT_M_PER_S
}

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
/// (or subscribe to the AOS/LOS events); routing is authored over this. Reflect so
/// the inspector / API `query_entity` can read it and the [`LinkRoute`] query can
/// walk the topology.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct LinkState {
    pub peers: Vec<LinkPeer>,
}

#[derive(Debug, Clone, Reflect)]
pub struct LinkPeer {
    pub peer: String,
    pub connected: bool,
    pub range_m: f64,
    /// One-way propagation delay, seconds — `range_m / c`. Published next to the
    /// range because for anything Earth↔Moon (1.28 s) the DELAY, not the range,
    /// is what the mission actually has to design around.
    pub light_time_s: f64,
    pub elevation_deg: f64,
}

/// The verdict seam consulted per pair. `ctx` (a [`HookValue`] map): `a`, `b`,
/// `class_a`, `class_b`, `range_m`, `light_time_s`, `elev_a`, `elev_b`,
/// `min_elev_a`, `min_elev_b`, `occluded`, `occluded_by`, `terrain_blocked`,
/// `max_range_m`. Return bool. No hook → the builtin range+mask+occlusion rule
/// (which does NOT gate on delay — a policy that refuses links slower than some
/// latency budget is exactly the kind of thing this hook is for).
pub const LINK_HOOK: &str = "link.connected";

/// Set the connectivity recompute cadence at runtime (any client / language).
#[Command(default)]
pub struct SetLinkCadence {
    pub interval_s: f64,
}

#[on_command(SetLinkCadence)]
fn on_set_link_cadence(trigger: On<SetLinkCadence>, mut config: ResMut<LinkConfig>) {
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
/// Terrain occlusion is instead read here through a plain `Query` over each DEM's
/// [`DemHeightField`] oracle — a read-only component access, so the system stays
/// non-exclusive (no `&mut World`, no sync point, no avian interference).
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_links(
    config: Option<Res<LinkConfig>>,
    world_time: Option<Res<WorldTime>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Option<Res<CelestialBodyRegistry>>,
    q_nodes: Query<(Entity, &LinkNode, &SolarFramePose, Option<&Name>)>,
    q_terrain: Query<(&GlobalTransform, &DemHeightField)>,
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
            // A body we cannot place cannot occlude anything. Skipping it is right; placing it
            // at the Sun's centre (the old behaviour) would have it eclipse everything.
            .filter_map(|b| {
                let p = eph.provider.global_position(b.ephemeris_id, jd)?;
                Some((b.name.clone(), ecliptic_to_bevy(p), b.radius_m))
            })
            .map(|(n, p, r)| (n, p.raw(), r))
            .collect(),
        _ => Vec::new(),
    };

    // DEM oracles for terrain LOS — snapshotted (Arc-shared, cheap) so the loop
    // owns no world borrow. Empty when no terrain is loaded (orbital-only scenes).
    let terrains: Vec<(GlobalTransform, Arc<SurfaceOracle>)> =
        q_terrain.iter().map(|(gt, hf)| (*gt, hf.0.clone())).collect();

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
            // Terrain relief (a rille rim / hill between the endpoints) shadows the
            // link. March the DEM in the site-local frame — `SolarFramePose::local`
            // IS the terrain oracle frame (see `pose.rs`). Skipped when the analytic
            // body check already severs, and cheap when no terrain is loaded.
            let terrain_blocked = cheap_ok && terrain_blocks(a.pose.local, b.pose.local, &terrains);

            let ctx = HookValue::map([
                ("a", HookValue::str(a.name.clone())),
                ("b", HookValue::str(b.name.clone())),
                ("class_a", HookValue::str(a.node.class.clone().unwrap_or_default())),
                ("class_b", HookValue::str(b.node.class.clone().unwrap_or_default())),
                ("range_m", HookValue::Float(range_m)),
                ("light_time_s", HookValue::Float(light_time_s(range_m))),
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
            let delay_s = light_time_s(range_m);
            per_node.entry(a.entity).or_default().push(LinkPeer {
                peer: b.name.clone(),
                connected,
                range_m,
                light_time_s: delay_s,
                elevation_deg: elev_a,
            });
            per_node.entry(b.entity).or_default().push(LinkPeer {
                peer: a.name.clone(),
                connected,
                range_m,
                light_time_s: delay_s,
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
        self.name = node_key(self.node.class.as_deref(), name, e);
        self
    }
}

/// The stable identifier a node is known by in [`LinkState::peers`] and the
/// routing graph: authored `class` first, else the entity `Name`, else a synthetic
/// `node_<index>`. The kernel and [`LinkRoute`](crate::queries) MUST agree on this.
pub fn node_key(class: Option<&str>, name: Option<&Name>, e: Entity) -> String {
    match class {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => name
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| format!("node_{}", e.index())),
    }
}

/// Does any DEM's relief block the segment `a→b` (both in the site-local / terrain
/// frame)? The single-ray `los_hit` kernel over each oracle, mirroring
/// `TerrainRaycastProvider`. The march is capped to the terrain footprint
/// (`±half_extent`) because terrain can only occlude within its own extent — this
/// keeps the step at the DEM sample pitch even for a surface↔satellite segment
/// (otherwise millions of metres of empty march).
fn terrain_blocks(a: DVec3, b: DVec3, terrains: &[(GlobalTransform, Arc<SurfaceOracle>)]) -> bool {
    if terrains.is_empty() {
        return false;
    }
    let d = b - a;
    let seg = d.length();
    if seg < 1e-3 {
        return false;
    }
    let dir = d / seg;
    for (gt, oracle) in terrains {
        let he = oracle.half_extent() as f64;
        let max = seg.min(2.5 * he);
        let inv = gt.affine().inverse();
        let o = inv.transform_point3(Vec3::new(a.x as f32, a.y as f32, a.z as f32));
        let dl = (inv.matrix3 * Vec3A::new(dir.x as f32, dir.y as f32, dir.z as f32))
            .normalize_or_zero();
        if dl.length_squared() < 0.5 {
            continue;
        }
        let hit = lunco_terrain_core::los_hit(
            oracle.as_ref(),
            [o.x as f64, o.y as f64, o.z as f64],
            [dl.x as f64, dl.y as f64, dl.z as f64],
            max,
            he,
            oracle.spacing().max(0.5) as f64,
            0.05, // endpoints sit above the surface — don't let them self-occlude
        );
        if hit.is_some() {
            return true;
        }
    }
    false
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

    /// **P5 regression — propagation delay is published, not silently dropped.**
    ///
    /// Before this, `grep -rn 'light_time\|speed_of_light\|299792' crates/`
    /// returned ZERO hits: the simulator whose reason to exist is lunar
    /// teleoperation did not model, or even mention, the 1.28 s that dominates
    /// it. A link at the mean Earth–Moon distance must report ~1.28 s.
    #[test]
    fn kernel_publishes_light_time_at_lunar_range() {
        const EARTH_MOON_M: f64 = 384_400_000.0;
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "earth_dsn", DVec3::ZERO, 1.0e12);
        node(&mut world, "lunar_relay", DVec3::new(EARTH_MOON_M, 0.0, 0.0), 1.0e12);

        world.run_system_once(update_links).unwrap();

        let peer = world
            .get::<LinkState>(a)
            .unwrap()
            .peers
            .iter()
            .find(|p| p.peer == "lunar_relay")
            .cloned()
            .expect("earth sees the relay");

        assert!(
            (peer.light_time_s - 1.282).abs() < 0.005,
            "Earth↔Moon one-way light time must be ~1.28 s, got {:.4} s",
            peer.light_time_s
        );
        // …and it is exactly range/c, not an approximation.
        assert!((peer.light_time_s - peer.range_m / SPEED_OF_LIGHT_M_PER_S).abs() < 1e-12);
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
