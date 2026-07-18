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

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lunco_core::coords::world_pose;
use lunco_core::{on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue};
use lunco_hooks::HookValue;
use lunco_terrain_surface::{DemHeightField, SurfaceOracle};
use lunco_time::WorldTime;

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::{segment_hits_obb, segment_hits_sphere};
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
    /// Drop debounce: how many CONSECUTIVE recomputes a live link must read as
    /// severed before its verdict actually flips to down. Acquisition is never
    /// debounced — a link goes up the instant geometry closes.
    ///
    /// This is the fix for green↔red chatter at a grazing horizon: at a polar site
    /// the sight-line to a relay or to Earth skims the ridge, so the DEM march flickers
    /// hit/clear as the endpoint creeps and a raw verdict oscillates every recompute.
    /// Requiring N consecutive downs to drop turns that flicker into one honest LOS,
    /// the same way real AOS/LOS detection debounces contact chatter. Asymmetric on
    /// purpose: you want to SEE a link the moment it is available, but not lose it to a
    /// single-frame graze.
    ///
    /// `1` disables the debounce (flip immediately, the old behaviour). At the default
    /// 0.25 s cadence, `3` is a 0.75 s hold — below human notice, above the flicker.
    pub drop_debounce: u32,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self { interval_s: 0.25, drop_debounce: 3 }
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

/// Generic sight-line blocker: an oriented box that severs any link whose segment
/// passes through it. Authored as `lunco:occluder` on the geometry prim that does
/// the blocking — a wall, a habitat module, a lander body.
///
/// **Nothing here is "comms"** (doc 49 §1): this is a box that blocks a segment. A
/// sensor, radar, or sunlight domain composes over the same component. It is also
/// deliberately NOT a physics collider:
///
/// * Occlusion is a MATERIAL question, not a collision one. A radio-transparent
///   handrail has a collider and must not block; a radome that is radio-opaque may
///   have none. Deriving one from the other is wrong in both directions.
/// * Reading colliders would mean an avian `SpatialQuery` per node pair against the
///   full broadphase, and a link sweep that only works once physics is stepping —
///   where this is `segment_hits_obb` over a handful of authored prims at the
///   [`LinkConfig`] cadence, through a read-only `Query`, in a crate that must stay
///   render-free and headless. (Precision is not the argument: avian re-exports
///   `parry3d_f64`, so a collider cast here would be f64 too.)
///
/// The cost is that occlusion is OPT-IN: an untagged wall does not block. That is
/// intended — say which geometry is opaque.
///
/// # The box comes from UsdGeom `extent`
///
/// There is no invented size vocabulary here. The occluding box IS the prim's
/// **`extent`** — core UsdGeom's "three dimensional range measuring the geometric
/// extent of the authored gprim in its own local space", which every DCC authors
/// and computes already. `lunco:occluder` adds exactly one fact USD has no word
/// for ("this geometry is opaque to sight-lines"); the shape it names is standard.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct LinkOccluder {
    /// Half-size of the prim's UsdGeom `extent`, in its local space, BEFORE the
    /// prim's own scale. Defaults to a unit cube (`0.5`), so a `Cube` with
    /// `size = 1` and no authored extent — which is how `props/wall.usda` and the
    /// sandbox slabs are written — resolves to `scale/2`.
    pub half_extents: DVec3,
    /// Centre of that `extent` in local space. UsdGeom's extent is not required to
    /// be origin-centred, so an offset mesh occludes where it actually sits.
    pub center: DVec3,
}

impl Default for LinkOccluder {
    fn default() -> Self {
        // A unit cube: the identity that makes `scale` alone sufficient.
        Self { half_extents: DVec3::splat(0.5), center: DVec3::ZERO }
    }
}

impl LinkOccluder {
    /// The box in the prim's own frame with its `Transform` scale applied:
    /// `(centre, half_extents)`. The kernel then places it with the prim's
    /// grid-absolute pose.
    pub fn box_for(&self, scale: Vec3) -> (DVec3, DVec3) {
        let s = scale.as_dvec3();
        (self.center * s, (self.half_extents * s).abs())
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
    /// The peer's [`GlobalEntityId`](lunco_core::GlobalEntityId) — the project's
    /// stable entity reference: deterministic from the prim's asset+path, identical
    /// on every peer, and the same `u64` that `find()` returns to a script and the
    /// API speaks on the wire.
    ///
    /// Not a name and not a class. Both are labels: `class` is a shared ROLE (three
    /// DSN complexes all author `class = "earth"`), and a prim `Name` is unique only
    /// within its parent. Keying identity on either collapsed distinct stations onto
    /// one graph node. Names/classes still exist — as labels and routing groups, in
    /// [`LinksProvider`](crate::queries) — but identity is the GID.
    pub peer: u64,
    pub connected: bool,
    pub range_m: f64,
    /// One-way propagation delay, seconds — `range_m / c`. Published next to the
    /// range because for anything Earth↔Moon (1.28 s) the DELAY, not the range,
    /// is what the mission actually has to design around.
    pub light_time_s: f64,
    pub elevation_deg: f64,
    /// The PEER's authored `class`, denormalized here at solve time.
    ///
    /// Identity stays the GID above; this is the peer's ROLE, copied because the
    /// solver already holds both endpoints' [`LinkNode`] in the pairwise loop. It
    /// exists so a port read is a pure local component read: without it, resolving
    /// "which class is peer GID 7" from a `&World` port backend would mean a full
    /// entity scan per read. `None` for a peer whose node authored no class — such
    /// a peer is still a real link, it just has no port name.
    pub class: Option<String>,
}

/// The verdict seam consulted per pair. `ctx` (a [`HookValue`] map): `a`, `b`
/// (the peers' GIDs — the same ids `find()` returns), `name_a`, `name_b`,
/// `class_a`, `class_b`, `range_m`, `light_time_s`, `elev_a`, `elev_b`,
/// `min_elev_a`, `min_elev_b`, `occluded`, `occluded_by`, `terrain_blocked`,
/// `occluder_blocked`, `max_range_m`. Return bool. No hook → the builtin
/// range+mask+occlusion rule
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
/// AOS/LOS edges), keyed by GID pair.
#[derive(Resource, Default)]
pub(crate) struct LinkSolverState {
    last_jd: f64,
    /// The PUBLISHED up-set from the previous recompute — what consumers were told and
    /// what AOS/LOS edges fire against. Debounced, not raw.
    prev_up: HashSet<(u64, u64)>,
    /// Per-pair count of consecutive recomputes a currently-published-up link has read
    /// as severed. Reaches [`LinkConfig::drop_debounce`] → the drop publishes. Cleared
    /// the moment the link reads up again, so a flicker never accumulates.
    down_streak: HashMap<(u64, u64), u32>,
}

/// One resolved node, snapshotted so the world borrow is free for the terrain
/// query provider.
struct Node {
    entity: Entity,
    /// Stable identity — see [`LinkPeer::peer`].
    gid: u64,
    /// Human label (prim `Name`, else class) for logs and the verdict ctx. NEVER
    /// an identity: it is not unique and not guaranteed stable.
    label: String,
    node: LinkNode,
    pose: SolarFramePose,
}

/// The cadence-gated pairwise connectivity sweep. A REGULAR system on purpose:
/// it writes through `Commands`, so it adds NO extra command-flush sync point. An
/// earlier EXCLUSIVE version (to call the `TerrainRaycast` provider with
/// `&mut World`) inserted a sync point that interleaved with the twin/terrain
/// despawns and tripped avian's island bookkeeping (`island.body_count > 0`).
/// Terrain and box occlusion are instead read here through plain `Query`s — a
/// read-only component access, so the system stays non-exclusive (no `&mut World`,
/// no sync point, no avian interference).
///
/// # Frames
///
/// Every occlusion test runs in the **grid-absolute (BigSpace root) frame**, which
/// is what [`SolarFramePose::local`] is. Occluder and DEM poses therefore come from
/// [`world_pose`] (the cell-aware chain walk), **not** from `GlobalTransform` —
/// which is origin-RELATIVE and shifts by a whole cell every time the floating
/// origin moves. An earlier version inverted a `GlobalTransform` here, so terrain
/// occlusion was silently wrong by the floating origin's grid offset: zero near the
/// origin, kilometres out at a site like the moonbase. (Same failure family as the
/// wheel-raycast bug that `GridSpatialQuery` exists to prevent.) f64 throughout —
/// an f32 cast of a ~1.7e6 m lunar coordinate throws away decimetres.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_links(
    config: Option<Res<LinkConfig>>,
    world_time: Option<Res<WorldTime>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Option<Res<CelestialBodyRegistry>>,
    q_nodes: Query<(
        Entity,
        &LinkNode,
        &SolarFramePose,
        Option<&Name>,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    q_terrain: Query<(Entity, &DemHeightField)>,
    q_occluders: Query<(Entity, &LinkOccluder)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut q_state: Query<&mut LinkState>,
    mut state: Local<LinkSolverState>,
    mut commands: Commands,
) {
    let (Some(config), Some(world_time)) = (config, world_time) else {
        return;
    };
    // `1` (or a fat-fingered `0`) means "flip immediately" — the pre-debounce behaviour.
    let debounce = config.drop_debounce.max(1);
    let jd = world_time.epoch_jd;
    if q_nodes.iter().count() < 2 {
        return;
    }
    let advanced = (jd - state.last_jd).abs() * 86_400.0 >= config.interval_s;
    if !advanced && state.last_jd != 0.0 {
        return;
    }

    // A node with no GID yet is SKIPPED this sweep, not given a fallback id.
    // Identity is minted in `PostUpdate` (and a runtime-spawned instance takes an
    // extra frame to go `Provenance::Local` → `Derived`), so an absent GID means
    // "not yet", and the node joins the graph within a frame or two. Inventing a
    // name/index key instead would MIS-BIND — it diverges across peers and across a
    // reload, which is precisely the class of bug GIDs exist to prevent.
    let nodes: Vec<Node> = q_nodes
        .iter()
        .filter_map(|(e, n, p, name, gid)| {
            Some(Node {
                entity: e,
                gid: gid?.get(),
                label: node_label(n.class.as_deref(), name, e),
                node: n.clone(),
                pose: *p,
            })
        })
        .collect();
    if nodes.len() < 2 {
        // Fewer than two IDENTIFIED nodes — nothing to pair. Return WITHOUT
        // stamping `last_jd`: this frame did no work, so it must not consume the
        // cadence slot, or the frame in which identities finally mint would be
        // skipped and the graph would wait a further interval for no reason.
        return;
    }
    state.last_jd = jd;

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
    // Poses are grid-absolute (see the frame note above), matching `pose.local`.
    let terrains: Vec<(DVec3, DQuat, Arc<SurfaceOracle>)> = q_terrain
        .iter()
        .filter_map(|(e, hf)| {
            let (p, r) = world_pose(e, &q_parents, &q_grids, &q_spatial)?;
            Some((p, r, hf.0.clone()))
        })
        .collect();

    // Authored box occluders (walls, habitats, lander bodies) — same frame, same
    // snapshot discipline. Usually a handful of prims, often none.
    let occluders: Vec<(DVec3, DQuat, DVec3)> = q_occluders
        .iter()
        .filter_map(|(e, occ)| {
            let (p, r) = world_pose(e, &q_parents, &q_grids, &q_spatial)?;
            let (_, tf) = q_spatial.get(e).ok()?;
            let (center_local, half) = occ.box_for(tf.scale);
            // `world_pose` composes translation+rotation only, so the extent's own
            // (scaled) centre offset is placed by the prim's rotation here.
            Some((p + r * center_local, r, half))
        })
        .collect();

    // Pairwise verdicts.
    let mut per_node: HashMap<Entity, Vec<LinkPeer>> = HashMap::new();
    let mut up_now: HashSet<(u64, u64)> = HashSet::new();
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
            // Authored geometry between the endpoints (a wall, a habitat). Skipped
            // once something cheaper already severs — the verdict is the same and
            // the hook reads the first cause, not every cause.
            let occluder_blocked = cheap_ok
                && !terrain_blocked
                && occluder_blocks(a.pose.local, b.pose.local, &occluders);

            let ctx = HookValue::map([
                // Identity first (the ids `find()` speaks), labels alongside for a
                // policy that wants to read as prose.
                ("a", HookValue::Int(a.gid as i64)),
                ("b", HookValue::Int(b.gid as i64)),
                ("name_a", HookValue::str(a.label.clone())),
                ("name_b", HookValue::str(b.label.clone())),
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
                ("occluder_blocked", HookValue::Bool(occluder_blocked)),
                ("max_range_m", HookValue::Float(a.node.max_range_m.min(b.node.max_range_m))),
            ]);
            let builtin = cheap_ok && !terrain_blocked && !occluder_blocked;
            let raw = match lunco_hooks::invoke(LINK_HOOK, &[ctx]) {
                Some(Ok(v)) => v.as_bool().unwrap_or(builtin),
                _ => builtin,
            };

            // Asymmetric drop debounce (see `LinkConfig::drop_debounce`). Acquire the
            // instant geometry closes; drop only after N consecutive severed reads, so a
            // grazing-horizon flicker becomes one honest LOS instead of green↔red chatter.
            let key = pair_key(a.gid, b.gid);
            let was_up = state.prev_up.contains(&key);
            let connected = if raw {
                state.down_streak.remove(&key);
                true
            } else if !was_up {
                false
            } else {
                let n = state.down_streak.entry(key).or_insert(0);
                *n += 1;
                if *n >= debounce {
                    state.down_streak.remove(&key);
                    false
                } else {
                    // Hold the published link up through the flicker.
                    true
                }
            };

            if connected {
                up_now.insert(key);
            }
            let delay_s = light_time_s(range_m);
            // Each side records the OTHER's gid AND the other's class — a node's
            // peer list describes who it can see, so both fields name the far end.
            per_node.entry(a.entity).or_default().push(LinkPeer {
                peer: b.gid,
                connected,
                range_m,
                light_time_s: delay_s,
                elevation_deg: elev_a,
                class: b.node.class.clone(),
            });
            per_node.entry(b.entity).or_default().push(LinkPeer {
                peer: a.gid,
                connected,
                range_m,
                light_time_s: delay_s,
                elevation_deg: elev_b,
                class: a.node.class.clone(),
            });
        }
    }

    debug!("[link] recompute: {} nodes, {} links up", nodes.len(), up_now.len());

    // AOS/LOS edges vs the previous recompute.
    for key in up_now.difference(&state.prev_up) {
        commands.trigger(link_event("link.aos", *key, jd));
    }
    for key in state.prev_up.difference(&up_now) {
        commands.trigger(link_event("link.los", *key, jd));
    }
    state.prev_up = up_now;

    // Publish per-node state — update in place, else insert.
    for node in &nodes {
        let peers = per_node.remove(&node.entity).unwrap_or_default();
        if let Ok(mut st) = q_state.get_mut(node.entity) {
            st.peers = peers;
        } else {
            commands.entity(node.entity).try_insert(LinkState { peers });
        }
    }
}

/// A node's human LABEL — prim `Name`, else authored `class`, else the entity
/// index. For logs, the inspector, and the verdict ctx's `name_a`/`name_b`.
///
/// **This is not an identity and must never be used as one.** Identity is the GID
/// ([`LinkPeer::peer`]). A label is not unique (three DSN complexes all carry
/// `class = "earth"`; two prims under different parents can share a `Name`) and the
/// index fallback is stable neither across a reload nor across peers. Keying the
/// graph on a label is exactly the bug that made Madrid, Goldstone and Canberra
/// collapse into one node.
pub fn node_label(class: Option<&str>, name: Option<&Name>, e: Entity) -> String {
    if let Some(n) = name {
        return n.as_str().to_string();
    }
    match class {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => format!("node_{}", e.index()),
    }
}

/// Does any DEM's relief block the segment `a→b` (both grid-absolute, the frame
/// [`SolarFramePose::local`] is in)? The single-ray `los_hit` kernel over each
/// oracle, mirroring `TerrainRaycastProvider`. The march is capped to the terrain
/// footprint (`±half_extent`) because terrain can only occlude within its own
/// extent — this keeps the step at the DEM sample pitch even for a
/// surface↔satellite segment (otherwise millions of metres of empty march).
///
/// `terrains` carries each DEM's grid-absolute pose (see the frame note on
/// [`update_links`]). Scale is not composed: DEM surfaces are authored unscaled,
/// and a scaled heightfield would need the oracle's own spacing rescaled too.
fn terrain_blocks(a: DVec3, b: DVec3, terrains: &[(DVec3, DQuat, Arc<SurfaceOracle>)]) -> bool {
    if terrains.is_empty() {
        return false;
    }
    let d = b - a;
    let seg = d.length();
    if seg < 1e-3 {
        return false;
    }
    let dir = d / seg;
    for (pos, rot, oracle) in terrains {
        let he = oracle.half_extent() as f64;
        let max = seg.min(2.5 * he);
        // Into the DEM's own frame — a rigid inverse, kept in f64.
        let inv = rot.conjugate();
        let o = inv * (a - *pos);
        let dl = inv * dir;
        let hit = lunco_terrain_core::los_hit(
            oracle.as_ref(),
            [o.x, o.y, o.z],
            [dl.x, dl.y, dl.z],
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

/// Does any authored [`LinkOccluder`] box block the segment `a→b` (both
/// grid-absolute)? Pure analytic geometry — no march, no physics query.
/// `occluders` carries each box's grid-absolute `(center, rotation, half_extents)`.
fn occluder_blocks(a: DVec3, b: DVec3, occluders: &[(DVec3, DQuat, DVec3)]) -> bool {
    occluders
        .iter()
        .any(|(center, rot, he)| segment_hits_obb(a, b, *center, *rot, *he))
}

/// An undirected pair, ordered so `(a,b)` and `(b,a)` are the same edge.
fn pair_key(a: u64, b: u64) -> (u64, u64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// An AOS/LOS edge event. `source` is one endpoint's GID (so a per-entity
/// telemetry subscription sees it) and `data` names the pair as `"<gid>-<gid>"`,
/// GIDs rather than labels — a subscriber resolves them with the same `find()` /
/// `name(id)` it uses everywhere else.
fn link_event(name: &str, (a, b): (u64, u64), jd: f64) -> TelemetryEvent {
    TelemetryEvent {
        name: name.to_string(),
        source: a,
        severity: Severity::Info,
        data: TelemetryValue::String(format!("{a}-{b}")),
        timestamp: jd,
    }
}

/// Publish each link node's working peer as `SimComponent` **outputs**
/// [`LINK_RANGE_CONNECTOR`] / [`LINK_CONNECTED_CONNECTOR`], so an RF model
/// (`assets/models/CommsLink.mo`) receives real geometry through an ordinary
/// output→input wire.
///
/// The gravity/solar-bridge idiom (`lunco-environment`'s `inject_local_solar_into_cosim`):
/// cosim stays domain-agnostic, this domain system writes the value, and the USD wiring
/// is explicit. The kernel keeps publishing geometry and ONLY geometry — metres and a
/// verdict. Bits per second are the channel model's business, and the channel model is
/// authored. That split is the whole point of the comms retirement: core knows geometry,
/// "comms" is content.
///
/// ## Reduced by CLASS, because a Modelica port is one scalar
///
/// `LinkState` already exposes EVERY peer to everything that can hold a list — rhai
/// (`get(id, "LinkState.peers")`), the API, the UI. Cosim is the one consumer that
/// cannot: a Modelica port is a fixed scalar, so N peers must reduce to something.
///
/// The reduction is by **`class`** — the authored routing group, the one that already
/// exists precisely because identity is the wrong key here (three DSN complexes all
/// author `class = "earth"`). Per class we publish the nearest CONNECTED peer, falling
/// back to the nearest of that class when none is connected, so the model keeps a
/// plausible distance to hold its input filter steady across a dropout rather than
/// snapping to a fake value:
///
/// ```text
/// outputs:link_earth_range_m / link_earth_connected / link_earth_elevation_deg
/// outputs:link_relay_range_m / link_relay_connected / link_relay_elevation_deg
/// ```
///
/// **The AUTHOR picks the link, not this function.** A model keeps generic inputs and the
/// USD connection maps one to it:
///
/// ```usda
/// float inputs:link_range_m.connect = </…/Comms.outputs:link_relay_range_m>
/// ```
///
/// so the same `CommsLink.mo` models a relay uplink on one vehicle and direct-to-Earth on
/// the next, and a two-radio vehicle instantiates it twice. No policy in Rust, none in
/// the model.
///
/// This REPLACED a "nearest connected peer overall" reduction, which quietly hardcoded a
/// routing decision in Rust while routing lives in rhai (`links.rhai`'s hop BFS to
/// `class = "earth"`) — a second, dumber router that disagreed with the real one the
/// moment a rover relayed via satellite instead of the nearest station. Reducing per
/// class is a REDUCTION, not routing: it answers "my best link of this kind", which is
/// exactly what a point-to-point budget needs, and leaves multi-hop to the layer that
/// owns it.
///
/// Writes every solve (not change-driven) because a model's own output sync rewrites its
/// outputs map — same reasoning as the gravity and solar bridges.
fn best_per_class(state: &LinkState) -> std::collections::HashMap<String, &LinkPeer> {
    let mut best: std::collections::HashMap<String, &LinkPeer> = Default::default();
    for p in &state.peers {
        // A peer with no class is unreachable by an authored wire (there is no port
        // name for it) — `LinkState` still carries it for script/UI.
        let Some(class) = p.class.as_deref() else { continue };
        best.entry(sanitize_class(class))
            .and_modify(|cur| {
                let better = match (cur.connected, p.connected) {
                    (false, true) => true,
                    (true, false) => false,
                    // `total_cmp`-equivalent ordering: a NaN range must not panic.
                    _ => p.range_m < cur.range_m,
                };
                if better {
                    *cur = p;
                }
            })
            .or_insert(p);
    }
    best
}

/// The three scalars a class publishes, as `(suffix, value)`.
fn class_ports(p: &LinkPeer) -> [(&'static str, f64); 3] {
    [
        ("range_m", p.range_m),
        ("connected", if p.connected { 1.0 } else { 0.0 }),
        ("elevation_deg", p.elevation_deg),
    ]
}

/// Link state as first-class **ports**, read on demand.
///
/// This used to be a system that wrote `link_<class>_*` keys into every link node's
/// [`SimComponent::outputs`] — i.e. into the *Modelica* backend's private storage.
/// That had three costs, all gone now:
///
/// 1. **A scheduling contract.** Pushing a value one tick early or late is a real
///    bug, and it happened: with no ordering the model read `link_range_m = 0` and
///    solved a 1 m link at 145 dB SNR, so the writer had to be pinned to
///    `FixedUpdate` `.before(CosimSet::Propagate)`. A backend is PULLED at read
///    time, so there is no tick to publish into wrongly and no ordering to keep.
/// 2. **A Modelica model as a prerequisite.** The old query was
///    `(&LinkState, &mut SimComponent)`, so a link node without an authored model
///    published nothing at all. Ports now exist wherever `LinkState` does.
/// 3. **Silent clobbering.** A model whose own variable was named
///    `link_earth_range_m` had it overwritten every tick. Registration order is
///    precedence and Modelica registers first, so now the model wins its own name.
///
/// Outputs only: link geometry is computed by the solver and is not writable. The
/// per-class reduction (connected beats nearer, then nearest) is unchanged — it is
/// what lets a scalar Modelica port see an N-peer graph.
pub const LINK_PORT_BACKEND: lunco_core::ports::PortBackend = lunco_core::ports::PortBackend {
    list: |world, entity, out| {
        let Some(state) = world.get::<LinkState>(entity) else { return };
        for (class, p) in best_per_class(state) {
            for (suffix, value) in class_ports(p) {
                out.push(lunco_core::ports::PortRef {
                    name: format!("link_{class}_{suffix}"),
                    direction: lunco_core::ports::PortDirection::Out,
                    value,
                });
            }
        }
    },
    read_output: |world, entity, name| {
        let state = world.get::<LinkState>(entity)?;
        let rest = name.strip_prefix("link_")?;
        best_per_class(state).into_iter().find_map(|(class, p)| {
            let suffix = rest.strip_prefix(&class)?.strip_prefix('_')?;
            class_ports(p)
                .into_iter()
                .find_map(|(s, v)| (s == suffix).then_some(v))
        })
    },
    // Link geometry is solver-derived: there is no input to read and nothing to
    // write. Returning `None`/`false` is what lets the registry fall through to a
    // backend that DOES own the name.
    read_input: |_, _, _| None,
    write_input: |_, _, _, _| false,
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// A `class` is authored free text but a port name is an identifier, so fold anything
/// that is not `[a-z0-9_]` to `_`. `"earth"` → `link_earth_range_m`; `"Deep Space"` →
/// `link_deep_space_range_m`. Lossy by design and stable: the author reads the port name
/// off the class they wrote.
fn sanitize_class(class: &str) -> String {
    class
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A peer entry. `class` is denormalized onto the peer by the solver, so a port
    /// test needs no second entity — which is the point: reads are purely local.
    fn peer(gid: u64, class: &str, connected: bool, range_m: f64, elevation_deg: f64) -> LinkPeer {
        LinkPeer {
            peer: gid,
            connected,
            range_m,
            light_time_s: 0.0,
            elevation_deg,
            class: Some(class.into()),
        }
    }

    /// Read one port through the backend, exactly as the registry would.
    fn port(world: &World, entity: Entity, name: &str) -> Option<f64> {
        (LINK_PORT_BACKEND.read_output)(world, entity, name)
    }

    /// The reduction that lets a scalar Modelica port see an N-peer graph: per CLASS,
    /// the best peer — connected beats nearer, then nearest wins.
    ///
    /// This is what a hardcoded "nearest peer overall" got wrong: it would have handed
    /// the model the 100 m station and called the link up, when the vehicle's earth link
    /// is the 5 km one and the relay link is a separate signal entirely. Class-reduction
    /// keeps BOTH, and lets the author wire whichever the radio actually works.
    #[test]
    fn bridge_publishes_the_best_peer_per_class_not_one_global_winner() {
        let mut world = World::new();
        let e = world
            .spawn(LinkState {
                peers: vec![
                    // earth: nearest, but BLOCKED — must lose to a connected one.
                    peer(1, "earth", false, 100.0, 10.0),
                    // earth: farther but usable — the earth link we are working.
                    peer(2, "earth", true, 5000.0, 20.0),
                    // relay: a DIFFERENT link, must not compete with earth at all.
                    peer(3, "relay", true, 9000.0, 30.0),
                ],
            })
            .id();

        assert_eq!(
            port(&world, e, "link_earth_range_m"),
            Some(5000.0),
            "connected must beat nearer-but-blocked within a class"
        );
        assert_eq!(port(&world, e, "link_earth_connected"), Some(1.0));
        assert_eq!(
            port(&world, e, "link_relay_range_m"),
            Some(9000.0),
            "the relay link is its own signal — a nearer EARTH peer must not shadow it"
        );
        assert_eq!(port(&world, e, "link_relay_connected"), Some(1.0));
    }

    /// The capability the old push-bridge did NOT have: its query was
    /// `(&LinkState, &mut SimComponent)`, so a link node with no authored Modelica
    /// model published nothing. Ports live wherever `LinkState` does.
    #[test]
    fn ports_exist_without_any_modelica_model_on_the_entity() {
        let mut world = World::new();
        let e = world
            .spawn(LinkState { peers: vec![peer(1, "base", true, 674.0, -4.9)] })
            .id();

        assert_eq!(port(&world, e, "link_base_range_m"), Some(674.0));
        assert_eq!(port(&world, e, "link_base_connected"), Some(1.0));

        let mut listed = Vec::new();
        (LINK_PORT_BACKEND.list)(&world, e, &mut listed);
        assert_eq!(listed.len(), 3, "range + verdict + elevation, enumerable");
        assert!(listed
            .iter()
            .all(|p| p.direction == lunco_core::ports::PortDirection::Out));
    }

    /// A peer whose node authored no class has no port name, and must not be
    /// mistaken for one — nor drag a neighbouring class's read down with it.
    #[test]
    fn a_classless_peer_publishes_nothing() {
        let mut world = World::new();
        let e = world
            .spawn(LinkState {
                peers: vec![
                    LinkPeer {
                        peer: 1,
                        connected: true,
                        range_m: 10.0,
                        light_time_s: 0.0,
                        elevation_deg: 0.0,
                        class: None,
                    },
                    peer(2, "base", true, 674.0, -4.9),
                ],
            })
            .id();

        let mut listed = Vec::new();
        (LINK_PORT_BACKEND.list)(&world, e, &mut listed);
        assert_eq!(listed.len(), 3, "only the classed peer is addressable");
        assert_eq!(port(&world, e, "link_base_range_m"), Some(674.0));
    }

    /// An unknown name must return `None` rather than a wrong class's value, or the
    /// registry could not fall through to the backend that really owns it.
    #[test]
    fn unknown_names_do_not_resolve() {
        let mut world = World::new();
        let e = world
            .spawn(LinkState { peers: vec![peer(1, "base", true, 674.0, -4.9)] })
            .id();

        assert_eq!(port(&world, e, "link_earth_range_m"), None);
        assert_eq!(port(&world, e, "link_base_bogus"), None);
        assert_eq!(port(&world, e, "throttle"), None);
    }

    /// Every peer of a class blocked ⇒ publish the nearest one's real range with verdict
    /// 0. The model needs a plausible distance to hold its input filter steady across a
    /// dropout; the verdict is what gates the rate to zero, not a fabricated range.
    #[test]
    fn bridge_reports_range_with_a_down_verdict_when_every_peer_is_blocked() {
        let mut world = World::new();
        let e = world
            .spawn(LinkState {
                peers: vec![
                    peer(1, "earth", false, 800.0, 1.0),
                    peer(2, "earth", false, 200.0, 2.0),
                ],
            })
            .id();

        assert_eq!(port(&world, e, "link_earth_range_m"), Some(200.0));
        assert_eq!(
            port(&world, e, "link_earth_connected"),
            Some(0.0),
            "blocked geometry must reach the model as a DOWN verdict"
        );
    }

    /// A class is authored free text; a port name is an identifier.
    #[test]
    fn class_names_fold_to_port_identifiers() {
        assert_eq!(sanitize_class("earth"), "earth");
        assert_eq!(sanitize_class("Deep Space"), "deep_space");
        assert_eq!(sanitize_class("X-Band/2"), "x_band_2");
    }

    /// A link node with an explicit GID. Real nodes get theirs from `Provenance`
    /// in `PostUpdate`; here we mint one directly so the sweep has an identity to
    /// key on (a node without one is skipped by design — see `update_links`).
    fn node_gid(world: &mut World, gid: u64, class: &str, pos: DVec3, max_range: f64) -> Entity {
        world
            .spawn((
                lunco_core::GlobalEntityId::from_raw(gid),
                LinkNode { max_range_m: max_range, min_elevation_deg: -90.0, class: Some(class.into()) },
                SolarFramePose { pos, local: pos, up: DVec3::Y, body: 301 },
            ))
            .id()
    }

    /// The two-node scenes below all use these fixed GIDs.
    const GID_A: u64 = 1001;
    const GID_B: u64 = 1002;

    fn node(world: &mut World, class: &str, pos: DVec3, max_range: f64) -> Entity {
        // First node spawned in a test is A, every later one B — every test here
        // is a two-node scene.
        let gid = if world
            .query::<&LinkNode>()
            .iter(world)
            .next()
            .is_none()
        {
            GID_A
        } else {
            GID_B
        };
        node_gid(world, gid, class, pos, max_range)
    }

    // ── Terrain occlusion ────────────────────────────────────────────────────
    //
    // `terrain_blocks` is the function the whole radio-shadow feature rests on, and
    // it had NO test while every other verdict input had several. That is exactly
    // how its frame bug survived: the kernel inverted a `GlobalTransform` (which is
    // origin-RELATIVE) against a grid-absolute segment, so terrain occlusion was
    // wrong by the floating origin's cell offset — ZERO near the origin, kilometres
    // out at a real site. Any test that put its terrain at the origin would have
    // passed while the feature was broken in the field, so `terrain_at_a_real_site_*`
    // below deliberately does not.

    /// A DEM with a single north–south wall of height `h` in a band around x = 0.
    /// Flat everywhere else, so anything that blocks is the wall and nothing else.
    fn ridge_oracle(h: f64, half_extent: f32) -> Arc<SurfaceOracle> {
        use lunco_terrain_surface::HeightGrid;
        let res = 129usize;
        let mut grid = HeightGrid::new_flat(res, half_extent);
        let spacing = grid.spacing() as f64;
        for iz in 0..res {
            for ix in 0..res {
                // Sample position in the DEM's own frame, centred on 0.
                let x = -(half_extent as f64) + ix as f64 * spacing;
                if x.abs() <= 12.0 {
                    grid.heights[iz * res + ix] = h;
                }
            }
        }
        Arc::new(SurfaceOracle::new(Arc::new(grid), vec![]))
    }

    /// No terrain in the scene ⇒ nothing is ever blocked. An orbital-only scene must
    /// not pay for, or trip over, a march that has nothing to march.
    #[test]
    fn no_terrain_never_blocks() {
        assert!(!terrain_blocks(DVec3::new(-200.0, 2.0, 0.0), DVec3::new(200.0, 2.0, 0.0), &[]));
    }

    /// Flat ground between two raised endpoints does not block. Guards the margin:
    /// endpoints sit ABOVE the surface, and a self-occluding endpoint would make
    /// every link on a flat plain read as severed.
    #[test]
    fn flat_terrain_does_not_block() {
        use lunco_terrain_surface::HeightGrid;
        let flat = Arc::new(SurfaceOracle::new(Arc::new(HeightGrid::new_flat(65, 500.0)), vec![]));
        let terrains = [(DVec3::ZERO, DQuat::IDENTITY, flat)];
        assert!(!terrain_blocks(
            DVec3::new(-200.0, 2.0, 0.0),
            DVec3::new(200.0, 2.0, 0.0),
            &terrains
        ));
    }

    /// A ridge standing across the sight-line severs it.
    #[test]
    fn a_ridge_between_two_nodes_severs_the_link() {
        let terrains = [(DVec3::ZERO, DQuat::IDENTITY, ridge_oracle(40.0, 500.0))];
        assert!(terrain_blocks(
            DVec3::new(-200.0, 2.0, 0.0),
            DVec3::new(200.0, 2.0, 0.0),
            &terrains
        ));
    }

    /// …and the SAME ridge does not, once both endpoints clear it. This is the
    /// pair that proves the march reads height rather than merely noticing terrain
    /// exists — and it is the physics the lesson teaches: the way out is to climb.
    #[test]
    fn the_same_ridge_clears_once_the_link_is_above_it() {
        let terrains = [(DVec3::ZERO, DQuat::IDENTITY, ridge_oracle(40.0, 500.0))];
        assert!(!terrain_blocks(
            DVec3::new(-200.0, 120.0, 0.0),
            DVec3::new(200.0, 120.0, 0.0),
            &terrains
        ));
    }

    /// A ridge running PARALLEL to the segment, off to one side, does not block.
    /// (Segment along z at x = -200; the wall is the band around x = 0.)
    #[test]
    fn a_ridge_beside_the_segment_does_not_sever() {
        let terrains = [(DVec3::ZERO, DQuat::IDENTITY, ridge_oracle(40.0, 500.0))];
        assert!(!terrain_blocks(
            DVec3::new(-200.0, 2.0, -150.0),
            DVec3::new(-200.0, 2.0, 150.0),
            &terrains
        ));
    }

    /// **The frame test.** Terrain a long way from the BigSpace origin still blocks
    /// correctly, because its pose is grid-absolute and so is the segment.
    ///
    /// This is the regression for the actual bug. The old code took the DEM's pose
    /// from a `GlobalTransform` — origin-relative, and therefore off by the floating
    /// origin's cell offset. At the origin the offset is zero and everything looks
    /// fine; at a real site it is kilometres, and the march samples empty space far
    /// from the ridge and reports "clear" forever. Offsetting the terrain AND the
    /// endpoints by the same amount must change nothing: same geometry, same answer.
    #[test]
    fn terrain_at_a_real_site_still_blocks() {
        let site = DVec3::new(1_737_000.0, -412_500.0, 96_000.0); // lunar-scale, f64
        let terrains = [(site, DQuat::IDENTITY, ridge_oracle(40.0, 500.0))];
        assert!(
            terrain_blocks(site + DVec3::new(-200.0, 2.0, 0.0), site + DVec3::new(200.0, 2.0, 0.0), &terrains),
            "a ridge must block just the same 1700 km from the origin — if this fails, \
             the DEM pose and the segment are in different frames again"
        );
        // …and the above-it case must still clear, so the test above cannot pass by
        // the march simply hitting everything once it is far from the origin.
        assert!(!terrain_blocks(
            site + DVec3::new(-200.0, 120.0, 0.0),
            site + DVec3::new(200.0, 120.0, 0.0),
            &terrains
        ));
    }

    /// A ROTATED DEM is inverted correctly. The kernel applies `rot.conjugate()` to
    /// bring the segment into the DEM's own frame; get that backwards and the wall
    /// is tested 90° from where it stands.
    #[test]
    fn terrain_rotation_is_inverted_not_applied() {
        // Yaw the DEM 90° about Y. `from_rotation_y` maps DEM +X → world −Z, so the
        // wall — a slab perpendicular to the DEM's OWN x — now stands perpendicular
        // to world z, occupying |z| ≤ 12 across every x. The unrotated case is
        // therefore mirrored: the z-axis segment crosses it, and an x-axis segment
        // clears only if it sits OUTSIDE that band.
        let rot = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let terrains = [(DVec3::ZERO, rot, ridge_oracle(40.0, 500.0))];
        assert!(
            terrain_blocks(DVec3::new(0.0, 2.0, -200.0), DVec3::new(0.0, 2.0, 200.0), &terrains),
            "after a 90° yaw the wall stands across the z-axis segment"
        );
        assert!(
            !terrain_blocks(DVec3::new(-200.0, 2.0, 200.0), DVec3::new(200.0, 2.0, 200.0), &terrains),
            "…and an x-axis segment at z = 200 is well clear of the |z| ≤ 12 band"
        );
        // The mirror of the unrotated check, and the part that actually pins the
        // direction of the inverse: at z = 0 the x-axis segment lies INSIDE the
        // yawed slab for its whole length, so it must block. Apply the rotation the
        // wrong way round and this reads clear.
        assert!(
            terrain_blocks(DVec3::new(-200.0, 2.0, 0.0), DVec3::new(200.0, 2.0, 0.0), &terrains),
            "at z = 0 the x-axis segment runs THROUGH the yawed wall, not beside it"
        );
    }

    /// A degenerate (zero-length) segment never blocks — two nodes at the same point
    /// must not report themselves occluded.
    #[test]
    fn a_zero_length_segment_never_blocks() {
        let terrains = [(DVec3::ZERO, DQuat::IDENTITY, ridge_oracle(40.0, 500.0))];
        let p = DVec3::new(0.0, 1.0, 0.0); // sitting ON the ridge
        assert!(!terrain_blocks(p, p, &terrains));
    }

    /// Serialises every test in this module.
    ///
    /// `lunco_hooks` is a PROCESS-GLOBAL registry and `update_links` consults it on
    /// every pair. Rust runs tests in parallel threads within one binary, so while a
    /// hook test has `link.connected` registered, any other test's sweep sees it too
    /// and gets that forced verdict instead of the builtin — a flaky failure in a
    /// test that never mentions hooks. Guarding only the hook tests against each
    /// other is not enough; the shared resource is the registry, and every reader
    /// must take the lock.
    ///
    /// These tests are microseconds each, so serialising them costs nothing.
    fn link_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        // A panicking test poisons the mutex; the state it guards is the hook
        // registry, which each test re-establishes, so recover rather than cascade
        // one real failure into N spurious ones.
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn world_at_epoch(interval_s: f64) -> World {
        let mut world = World::new();
        world.insert_resource(lunco_time::WorldTime { epoch_jd: 2_451_545.0, ..Default::default() });
        // `drop_debounce: 1` = flip immediately, so every geometry test reads the raw
        // verdict on the sweep it runs. The debounce itself is exercised on its own.
        world.insert_resource(LinkConfig { interval_s, drop_debounce: 1 });
        world
    }

    #[test]
    fn kernel_pairs_nodes_and_writes_link_state() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "relay", DVec3::new(10.0, 0.0, 0.0), 1.0e12);

        world.run_system_once(update_links).unwrap();

        let sa = world.get::<LinkState>(a).expect("node a has LinkState");
        let peer = sa.peers.iter().find(|p| p.peer == GID_B).expect("a sees relay");
        assert!(peer.connected, "a clear 10 m link should be up: {peer:?}");
        assert!((peer.range_m - 10.0).abs() < 1e-6, "range {}", peer.range_m);
    }

    /// The green↔red chatter fix: a link that reads severed for a moment must NOT drop
    /// until it has read severed for `drop_debounce` consecutive recomputes, and a single
    /// re-acquire in that window resets the count. Acquisition itself is never delayed.
    #[test]
    fn a_dropout_debounces_but_a_reacquire_resets_it() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        world.resource_mut::<LinkConfig>().drop_debounce = 3;
        let a = node(&mut world, "rover", DVec3::ZERO, 100.0);
        let b = node(&mut world, "relay", DVec3::new(50.0, 0.0, 0.0), 100.0);

        let sys = world.register_system(update_links);
        let is_up = |w: &mut World| {
            w.run_system(sys).unwrap();
            w.get::<LinkState>(a).unwrap().peers.iter().find(|p| p.peer == GID_B).unwrap().connected
        };
        // Force the RAW verdict down by pulling the range past both nodes' max.
        let sever = |w: &mut World| { w.get_mut::<LinkNode>(a).unwrap().max_range_m = 1.0; w.get_mut::<LinkNode>(b).unwrap().max_range_m = 1.0; };
        let heal = |w: &mut World| { w.get_mut::<LinkNode>(a).unwrap().max_range_m = 100.0; w.get_mut::<LinkNode>(b).unwrap().max_range_m = 100.0; };

        assert!(is_up(&mut world), "50 m link within 100 m range is up");

        // Sever: held up through the debounce window (streak 1, then 2)…
        sever(&mut world);
        assert!(is_up(&mut world), "1st severed read: held up (streak 1/3)");
        assert!(is_up(&mut world), "2nd severed read: held up (streak 2/3)");
        // …one flicker back to up in the window RESETS the streak.
        heal(&mut world);
        assert!(is_up(&mut world), "re-acquired");
        sever(&mut world);
        assert!(is_up(&mut world), "streak restarts after a re-acquire (1/3), not 3/3");
        assert!(is_up(&mut world), "streak 2/3");
        // Third consecutive severed read finally publishes the drop.
        assert!(!is_up(&mut world), "3 consecutive severed reads ⇒ LOS");
    }

    /// **P5 regression — propagation delay is published, not silently dropped.**
    ///
    /// Before this, `grep -rn 'light_time\|speed_of_light\|299792' crates/`
    /// returned ZERO hits: the simulator whose reason to exist is lunar
    /// teleoperation did not model, or even mention, the 1.28 s that dominates
    /// it. A link at the mean Earth–Moon distance must report ~1.28 s.
    #[test]
    fn kernel_publishes_light_time_at_lunar_range() {
        let _g = link_lock();
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
            .find(|p| p.peer == GID_B)
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
        let _g = link_lock();
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
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        // `a` demands peers ≥ 30° above its local horizon (up = +Y).
        let a = world
            .spawn((
                lunco_core::GlobalEntityId::from_raw(GID_A),
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

    // ── Identity ─────────────────────────────────────────────────────────────

    /// The regression this exists for: `components/comms/ground_station.usda`
    /// authors `class = "earth"`, so a scene referencing it three times (Madrid,
    /// Goldstone, Canberra — `comms_demo_test.usda`) had all three collapse onto
    /// the key `"earth"`. Identity is now the GID, so same-class nodes stay
    /// distinct and a rover reports a separate link to each complex.
    #[test]
    fn same_class_nodes_stay_distinct_by_gid() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let rover = node_gid(&mut world, 7, "rover", DVec3::ZERO, 1.0e12);
        // Three stations, one shared role, three identities.
        for (i, gid) in [11_u64, 12, 13].iter().enumerate() {
            node_gid(
                &mut world,
                *gid,
                "earth",
                DVec3::new(100.0 * (i as f64 + 1.0), 0.0, 0.0),
                1.0e12,
            );
        }

        world.run_system_once(update_links).unwrap();

        let peers = &world.get::<LinkState>(rover).unwrap().peers;
        let mut ids: Vec<u64> = peers.iter().map(|p| p.peer).collect();
        ids.sort();
        assert_eq!(ids, vec![11, 12, 13], "each same-class station is its own node");
        // …and they are genuinely distinct links, not one repeated.
        let mut ranges: Vec<i64> = peers.iter().map(|p| p.range_m as i64).collect();
        ranges.sort();
        assert_eq!(ranges, vec![100, 200, 300], "each station keeps its own range");
    }

    /// A node whose identity has not been minted yet is SKIPPED, never given a
    /// fallback key. A name/index fallback would mis-bind across peers and across
    /// a reload — worse than waiting a frame.
    #[test]
    fn node_without_gid_is_skipped_not_faked() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node_gid(&mut world, GID_A, "rover", DVec3::ZERO, 1.0e12);
        // Same shape, but identity not yet assigned (the PostUpdate window).
        world.spawn((
            LinkNode { class: Some("station".into()), ..default() },
            SolarFramePose { pos: DVec3::new(10.0, 0.0, 0.0), local: DVec3::new(10.0, 0.0, 0.0), up: DVec3::Y, body: 301 },
        ));

        world.run_system_once(update_links).unwrap();

        // Fewer than two IDENTIFIED nodes ⇒ no graph at all this sweep.
        assert!(
            world.get::<LinkState>(a).is_none(),
            "a GID-less node must not be paired under an invented key"
        );
    }

    // ── Occluders ────────────────────────────────────────────────────────────
    //
    // The gap these close: before `LinkOccluder`, sight-lines were severed only by
    // celestial spheres and DEM relief, so a wall — the most legible obstacle there
    // is — did not block a link. "Drive the rover behind the wall and lose comms"
    // is the whole demo; these are the tests that make it real.

    /// Spawn a box occluder with explicit local half-extents (an authored UsdGeom
    /// `extent`) at `at`, unscaled.
    fn occluder(world: &mut World, at: DVec3, half: DVec3) -> Entity {
        world
            .spawn((
                LinkOccluder { half_extents: half, center: DVec3::ZERO },
                Transform::from_translation(at.as_vec3()),
            ))
            .id()
    }

    #[test]
    fn occluder_box_severs_link() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(20.0, 0.0, 0.0), 1.0e12);
        // A wall astride the segment at its midpoint.
        occluder(&mut world, DVec3::new(10.0, 0.0, 0.0), DVec3::new(1.0, 5.0, 5.0));

        world.run_system_once(update_links).unwrap();

        let peer = world.get::<LinkState>(a).unwrap().peers[0].clone();
        assert!(!peer.connected, "a wall across the sight-line must sever it: {peer:?}");
        // …and it is the OCCLUDER that severed it, not range or the elevation mask.
        assert!((peer.range_m - 20.0).abs() < 1e-6, "range unchanged: {}", peer.range_m);
    }

    /// The control for the test above: same nodes, same distance, box moved aside.
    /// Without this pair, a link that was down for an unrelated reason would look
    /// like working occlusion.
    #[test]
    fn occluder_beside_the_segment_does_not_sever() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(20.0, 0.0, 0.0), 1.0e12);
        // Same box, lifted well clear of the segment.
        occluder(&mut world, DVec3::new(10.0, 50.0, 0.0), DVec3::new(1.0, 5.0, 5.0));

        world.run_system_once(update_links).unwrap();

        assert!(
            world.get::<LinkState>(a).unwrap().peers[0].connected,
            "a box off the sight-line must not sever it"
        );
    }

    /// No authored `extent` ⇒ the unit-cube convention (scale/2). This is the path
    /// `props/wall.usda` takes — a `Cube` with `size = 1` and a scale, no extent —
    /// so tagging an existing prop is one line and no measurements.
    #[test]
    fn occluder_without_extent_derives_its_box_from_scale() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(20.0, 0.0, 0.0), 1.0e12);
        // Default (unit cube) scaled 2×10×10 ⇒ half-extents 1×5×5 — blocks, as above.
        world.spawn((
            LinkOccluder::default(),
            Transform {
                translation: Vec3::new(10.0, 0.0, 0.0),
                scale: Vec3::new(2.0, 10.0, 10.0),
                ..default()
            },
        ));

        world.run_system_once(update_links).unwrap();

        assert!(
            !world.get::<LinkState>(a).unwrap().peers[0].connected,
            "scale-derived box must occlude exactly like an authored extent"
        );
    }

    /// UsdGeom's `extent` need not be origin-centred, so a prim whose geometry sits
    /// off to one side must occlude where the geometry IS — not where its origin is.
    #[test]
    fn occluder_honours_an_offset_extent_centre() {
        let _g = link_lock();
        let (a_pos, b_pos) = (DVec3::ZERO, DVec3::new(20.0, 0.0, 0.0));

        // Extent centred 30 m up: the prim sits ON the segment, its geometry does not.
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", a_pos, 1.0e12);
        node(&mut world, "station", b_pos, 1.0e12);
        world.spawn((
            LinkOccluder {
                half_extents: DVec3::new(1.0, 2.0, 5.0),
                center: DVec3::new(0.0, 30.0, 0.0),
            },
            Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        ));
        world.run_system_once(update_links).unwrap();
        assert!(
            world.get::<LinkState>(a).unwrap().peers[0].connected,
            "geometry offset clear of the segment must not block, whatever the origin"
        );

        // Same prim, extent centred on the segment → blocks.
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", a_pos, 1.0e12);
        node(&mut world, "station", b_pos, 1.0e12);
        world.spawn((
            LinkOccluder {
                half_extents: DVec3::new(1.0, 2.0, 5.0),
                center: DVec3::ZERO,
            },
            Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        ));
        world.run_system_once(update_links).unwrap();
        assert!(
            !world.get::<LinkState>(a).unwrap().peers[0].connected,
            "the same box centred on the segment blocks"
        );
    }

    /// A rotated box occludes where its OBB actually is — not where its
    /// axis-aligned bounds would be. A thin wall turned edge-on to the link lets it
    /// through; the same wall turned across it does not.
    #[test]
    fn occluder_respects_rotation() {
        let _g = link_lock();
        let thin = DVec3::new(0.5, 5.0, 8.0); // a wall: thin in X, wide in Z
        let (a_pos, b_pos) = (DVec3::ZERO, DVec3::new(20.0, 0.0, 0.0));
        let center = DVec3::new(10.0, 0.0, 0.0);

        // Broadside: the wall's wide face spans the X-axis link → blocked.
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", a_pos, 1.0e12);
        node(&mut world, "station", b_pos, 1.0e12);
        world.spawn((
            LinkOccluder { half_extents: thin, center: DVec3::ZERO },
            Transform { translation: center.as_vec3(), ..default() },
        ));
        world.run_system_once(update_links).unwrap();
        assert!(
            !world.get::<LinkState>(a).unwrap().peers[0].connected,
            "broadside wall blocks"
        );

        // Same wall yawed 90°: now only its 0.5 m edge faces the link — but it is
        // still ON the segment, so it must STILL block. (Rotation must not be a
        // free pass; this pins that the OBB, not an AABB, is what we test.)
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", a_pos, 1.0e12);
        node(&mut world, "station", b_pos, 1.0e12);
        world.spawn((
            LinkOccluder { half_extents: thin, center: DVec3::ZERO },
            Transform {
                translation: center.as_vec3(),
                rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                ..default()
            },
        ));
        world.run_system_once(update_links).unwrap();
        assert!(
            !world.get::<LinkState>(a).unwrap().peers[0].connected,
            "yawed wall still straddles the segment → still blocks"
        );

        // Now slide the yawed wall along Z. Rotated, its Z half-extent is only
        // 0.5 m, so at z = 4 it is CLEAR — whereas an AABB test (which would keep
        // the 8 m Z extent) would wrongly still report a block.
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", a_pos, 1.0e12);
        node(&mut world, "station", b_pos, 1.0e12);
        world.spawn((
            LinkOccluder { half_extents: thin, center: DVec3::ZERO },
            Transform {
                translation: Vec3::new(10.0, 0.0, 4.0),
                rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                ..default()
            },
        ));
        world.run_system_once(update_links).unwrap();
        assert!(
            world.get::<LinkState>(a).unwrap().peers[0].connected,
            "yawed wall slid off the segment must NOT block (OBB, not AABB)"
        );
    }

    // ── The verdict seam ─────────────────────────────────────────────────────

    /// The whole scripting contract (doc 49 §4) was untested: nothing proved a
    /// registered `link.connected` hook actually overrides the builtin verdict.
    ///
    struct ConstHook(bool);
    impl lunco_hooks::ScriptHook for ConstHook {
        fn invoke(&self, _args: &[HookValue]) -> lunco_hooks::HookResult {
            Ok(HookValue::Bool(self.0))
        }
    }

    fn register_verdict(v: bool) {
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: LINK_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: Arc::new(ConstHook(v)),
        });
    }

    #[test]
    fn hook_verdict_overrides_builtin_in_both_directions() {
        let _g = link_lock();

        // A geometrically PERFECT link, refused by policy.
        register_verdict(false);
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(10.0, 0.0, 0.0), 1.0e12);
        world.run_system_once(update_links).unwrap();
        let down = world.get::<LinkState>(a).unwrap().peers[0].connected;

        // …and a link the builtin would REFUSE (out of range), allowed by policy.
        register_verdict(true);
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 5.0);
        node(&mut world, "station", DVec3::new(1000.0, 0.0, 0.0), 5.0);
        world.run_system_once(update_links).unwrap();
        let up = world.get::<LinkState>(a).unwrap().peers[0].connected;

        lunco_hooks::unregister(LINK_HOOK);

        assert!(!down, "hook returning false must sever a geometrically clear link");
        assert!(up, "hook returning true must raise a link the builtin would refuse");
    }

    /// The hook receives the geometry FACTS it is documented to receive. If a key
    /// is renamed or dropped, every authored policy silently falls back to the
    /// builtin — the same silent-no-op class as the wrong hook id.
    #[test]
    fn hook_ctx_carries_the_documented_keys() {
        let _g = link_lock();

        #[derive(Default)]
        struct Captor(std::sync::Mutex<Vec<String>>);
        impl lunco_hooks::ScriptHook for Captor {
            fn invoke(&self, args: &[HookValue]) -> lunco_hooks::HookResult {
                if let Some(HookValue::Map(entries)) = args.first() {
                    *self.0.lock().unwrap() =
                        entries.iter().map(|(k, _)| k.clone()).collect();
                }
                Ok(HookValue::Bool(true))
            }
        }
        let captor = Arc::new(Captor::default());
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: LINK_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: captor.clone(),
        });

        let mut world = world_at_epoch(0.0);
        node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(10.0, 0.0, 0.0), 1.0e12);
        world.run_system_once(update_links).unwrap();

        lunco_hooks::unregister(LINK_HOOK);

        let keys = captor.0.lock().unwrap().clone();
        for expected in [
            "a", "b", "class_a", "class_b", "range_m", "light_time_s", "elev_a", "elev_b",
            "min_elev_a", "min_elev_b", "occluded", "occluded_by", "terrain_blocked",
            "occluder_blocked", "max_range_m",
        ] {
            assert!(keys.contains(&expected.to_string()), "hook ctx missing '{expected}': {keys:?}");
        }
    }

    // ── AOS/LOS edges and cadence ────────────────────────────────────────────

    #[derive(Resource, Default)]
    struct SeenEvents(Vec<(String, String)>);

    fn watch_events(world: &mut World) {
        world.init_resource::<SeenEvents>();
        world.add_observer(|ev: On<TelemetryEvent>, mut seen: ResMut<SeenEvents>| {
            let data = match &ev.data {
                TelemetryValue::String(s) => s.clone(),
                _ => String::new(),
            };
            seen.0.push((ev.name.clone(), data));
        });
    }

    /// AOS/LOS must fire on TRANSITIONS only. A consumer that subscribes to
    /// `link.los` to hand control to autonomy gets re-triggered every recompute
    /// (4 Hz) if this regresses.
    #[test]
    fn aos_los_fire_once_per_transition() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        watch_events(&mut world);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        let b = node(&mut world, "station", DVec3::new(20.0, 0.0, 0.0), 1.0e12);
        let sys = world.register_system(update_links);

        // Link comes up → one AOS, and no second AOS while it stays up.
        world.run_system(sys).unwrap();
        world.run_system(sys).unwrap();
        assert_eq!(
            world.resource::<SeenEvents>().0.iter().filter(|(n, _)| n == "link.aos").count(),
            1,
            "AOS fires once on the rising edge, not per recompute: {:?}",
            world.resource::<SeenEvents>().0
        );

        // Drop a wall in → one LOS.
        occluder(&mut world, DVec3::new(10.0, 0.0, 0.0), DVec3::new(1.0, 5.0, 5.0));
        world.run_system(sys).unwrap();
        world.run_system(sys).unwrap();

        let seen = &world.resource::<SeenEvents>().0;
        assert_eq!(
            seen.iter().filter(|(n, _)| n == "link.los").count(),
            1,
            "LOS fires once on the falling edge: {seen:?}"
        );
        // The event names the pair by GID, ordered — the ids a subscriber can
        // resolve with `name(id)`, not labels it would have to match by string.
        assert!(
            seen.iter().any(|(n, d)| n == "link.los" && *d == format!("{GID_A}-{GID_B}")),
            "LOS carries the GID pair: {seen:?}"
        );
        let _ = (a, b);
    }

    /// The cadence gate (doc 49 §3) — the whole sweep, terrain march included, is
    /// meant to be skipped between intervals. Every previous test passed
    /// `interval_s = 0.0`, so nothing proved the gate worked at all.
    #[test]
    fn cadence_gate_skips_recompute_within_the_interval() {
        let _g = link_lock();
        // A 1 s interval with the clock held still ⇒ exactly one recompute.
        let mut world = world_at_epoch(1.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(10.0, 0.0, 0.0), 1.0e12);
        let sys = world.register_system(update_links);

        world.run_system(sys).unwrap();
        assert!(!world.get::<LinkState>(a).unwrap().peers.is_empty(), "first sweep runs");

        // Clobber the published state: a second sweep at the same epoch must NOT
        // rewrite it.
        world.get_mut::<LinkState>(a).unwrap().peers.clear();
        world.run_system(sys).unwrap();
        assert!(
            world.get::<LinkState>(a).unwrap().peers.is_empty(),
            "within the interval the sweep must be skipped entirely"
        );

        // Advance the clock past the interval → it recomputes.
        world.resource_mut::<WorldTime>().epoch_jd += 2.0 / 86_400.0;
        world.run_system(sys).unwrap();
        assert!(
            !world.get::<LinkState>(a).unwrap().peers.is_empty(),
            "past the interval the sweep must run again"
        );
    }

    /// `interval_s = 0` means "every tick" — the escape hatch the tests above and
    /// any step-locked consumer rely on.
    #[test]
    fn zero_cadence_recomputes_every_tick() {
        let _g = link_lock();
        let mut world = world_at_epoch(0.0);
        let a = node(&mut world, "rover", DVec3::ZERO, 1.0e12);
        node(&mut world, "station", DVec3::new(10.0, 0.0, 0.0), 1.0e12);
        let sys = world.register_system(update_links);

        world.run_system(sys).unwrap();
        world.get_mut::<LinkState>(a).unwrap().peers.clear();
        world.run_system(sys).unwrap();

        assert!(
            !world.get::<LinkState>(a).unwrap().peers.is_empty(),
            "interval 0 ⇒ recompute every tick"
        );
    }
}
