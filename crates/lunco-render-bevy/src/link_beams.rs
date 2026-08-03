//! The `link_beams` program driver — connectivity beams, authored like the altimeter's
//! `range_beam`.
//!
//! The look and tuning are USD: authored `Cylinder`s with bound emissive `Material`s and
//! `lunco:param:*` numbers (`assets/components/comms/link_beam.usda`, shared like
//! `wheel.usda`). This file adds no schema and no components — it reads params through the
//! generic [`ScriptParams`] (as `drive_range_beam` reads `width`) and instances the
//! authored prims.
//!
//! COLOUR BY STATE. The part authors one template per connectivity state — `Up` (green,
//! `param:state = 0`) and `Down` (red, `state = 1`). The driver clones the template that
//! matches each peer's live `connected` verdict, so the colour follows the state that the
//! `link.connected` rhai hook decides and `link.aos`/`link.los` announce.
//!
//! COUNT is the only thing that differs from the altimeter: a node has N peers, so the
//! driver clones the matching template's mesh + material handle once per peer and writes
//! each a local [`Transform`] aimed at that peer — near = full span, far = a stub ray
//! (a 384,000 km Earth beam would be off-screen and jitter). With a camera, the stub uses a
//! share of the distance to the endpoint nearest the camera, so the same link reads from a
//! surface camera and from an Earth-focus one without the remote endpoint drawing a
//! viewport-spanning ray. The authored metre length is the headless fallback. Direction is
//! [`world_pose`]
//! (f64, cell-aware) both ends, so nothing jitters. Cloning a `Handle` is a cheap `Arc`
//! bump and Bevy GPU-batches shared-handle instances into one draw call — this scales to
//! a lidar's many rays unchanged.

use std::collections::{HashMap, HashSet};

use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::link::LinkState;
use lunco_core::coords::{world_pose, GridPos};
use lunco_core::programs::{ProgramDriverAppExt, ProgramDriverId};
use lunco_core::{GlobalEntityId, ScriptParams};

/// The `info:id` the beam part authors.
const DRIVER_ID: &str = "link_beams";

// Fallbacks when the part authors no `lunco:param:*`.
const DEF_WIDTH: f64 = 0.12;
const DEF_NEAR_M: f64 = 50_000.0;
const DEF_STUB: f64 = 20.0;
/// Far-peer beam length as a fraction of the camera's distance to the nearer endpoint.
///
/// A fixed `stubLen` is a length in metres, so it only reads at ONE zoom. The
/// Earth↔Moon link is the case that breaks it: 100 km of ray is generous from a
/// surface camera and invisible from an Earth-focus camera parked ~19,000 km out
/// (≈3× Earth radius, where the auto-focus sits). The beam is a DIRECTION
/// indicator — "the link reaches that way" — so what has to stay constant is how
/// much of the SCREEN it crosses, not how many metres it spans. The endpoint nearest the
/// camera controls both endpoint-owned copies of the same link; applying a fixed metre
/// floor to the remote endpoint makes an Earth station draw a huge ray across a lunar view.
///
/// When a camera is available this replaces `stubLen`; `stubLen` remains the fallback for
/// headless rendering, where there is no view-relative scale to measure.
const DEF_STUB_CAM_FRAC: f64 = 0.35;

/// Tags a spawned beam with its peer and the state it currently shows, so the reconciler
/// can recolour it on a flip or despawn it when the peer drops out — and so
/// [`aim_link_beams`] can re-point it every frame without consulting anything else.
///
/// `peer_entity` is the whole reason the two systems can be separated. A beam's peer is
/// authored as a `GlobalEntityId`, and resolving a GID to an `Entity` means a map over
/// *every* GID-bearing entity in the world. Doing that per frame — which is what a single
/// fused system had to do — is O(entities) of pure lookup to re-aim a handful of beams.
/// Resolving once at reconcile time and caching it here makes aiming O(beams).
///
/// The cached `Entity` is only as good as the reconcile that produced it, so aiming treats
/// a stale one as "skip this frame" (see [`aim_link_beams`]) rather than trusting it.
#[derive(Component)]
pub struct LinkBeamInstance {
    pub peer: u64,
    pub up: bool,
    /// Resolved at reconcile time from `peer`. See the type docs.
    pub peer_entity: Entity,
    /// Per-node tuning, copied so aiming needs no template lookup.
    pub near_m: f64,
    pub stub: f32,
    /// See [`DEF_STUB_CAM_FRAC`]. 0 disables the camera term entirely.
    pub stub_cam_frac: f32,
    pub width: f32,
}

type Look = (Mesh3d, MeshMaterial3d<StandardMaterial>);

/// The authored templates + tuning gathered for one node.
#[derive(Default)]
struct NodeBeams {
    /// Authored phase-centre transform that owns the template geometry. This
    /// can differ from the entity carrying the link state (for example a
    /// radio wrapper around an antenna).
    origin: Option<Entity>,
    up: Option<Look>,
    down: Option<Look>,
    width: f32,
    near_m: f64,
    stub: f32,
    stub_cam_frac: f32,
    mode: f64,
    show_down: bool,
    earth_only: bool,
}

/// How long a beam to this peer should be.
///
/// A NEAR peer gets a real cylinder that lands on it. A FAR one gets a ray whose
/// camera-relative length is [`DEF_STUB_CAM_FRAC`]'s share of the distance to the
/// nearer endpoint, so the link reads at surface zoom and at globe zoom without
/// either being authored twice. The authored metre stub is used without a camera.
fn beam_len(
    dist: f64,
    near_m: f64,
    stub: f32,
    cam_frac: f32,
    camera_distances: Option<(f64, f64)>,
) -> f32 {
    if dist <= near_m {
        return dist as f32;
    }
    let camera_term = match camera_distances {
        Some((source, peer)) if cam_frac > 0.0 => {
            (source.min(peer) * cam_frac as f64).max(1.0) as f32
        }
        _ => return stub,
    };
    camera_term
}

/// World distances from the active camera to the source and peer, or `None` when
/// there is no active camera to measure against (headless, or a frame before one exists).
fn camera_distances<F: bevy::ecs::query::QueryFilter>(
    source_pos: GridPos,
    peer_pos: GridPos,
    q_cam: &Query<(&Camera, Entity), With<Camera3d>>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(f64, f64)> {
    let (_, cam) = q_cam.iter().find(|(c, _)| c.is_active)?;
    let (camera_pos, _) = world_pose(cam, q_parents, q_grids, q_spatial)?;
    Some((
        (camera_pos.0 - source_pos.0).length(),
        (camera_pos.0 - peer_pos.0).length(),
    ))
}

pub(crate) fn build(app: &mut App) {
    // TWO cadences, because the driver does two jobs that do not change together.
    //
    // Which beams should exist changes only when a link connects, drops, or is
    // re-tuned — a `LinkState`/template edit. Where each beam POINTS changes
    // whenever either endpoint moves, which is every frame.
    //
    // Fused into one system these shared the slower job's cadence: a full rebuild
    // of the GID→`Entity` map over every GID-bearing entity in the world, plus a
    // per-node template gather with a `Look` clone each, every frame — to almost
    // always conclude nothing had changed. Splitting lets each run at its own rate.
    //
    // `register_program_driver` takes `IntoScheduleConfigs`, so a driver may carry
    // run conditions and register as a tuple; the id is the registry key, not a
    // one-system limit.
    app.register_program_driver(
        DRIVER_ID,
        (
            reconcile_link_beams.run_if(link_topology_changed),
            aim_link_beams,
        ),
    );
}

/// Does the SET of beams possibly need to change this frame?
///
/// Deliberately structural only — this must not consult motion. A rover driving
/// changes where a beam points, not whether it exists, and gating aiming on this
/// would freeze beams mid-air.
///
/// `Local` forces one initial pass: a freshly-added system does not observe
/// pre-existing entities as `Changed`.
fn link_topology_changed(
    mut first: Local<bool>,
    q_state: Query<(), Changed<LinkState>>,
    q_templates: Query<(), Or<(Changed<ProgramDriverId>, Changed<ScriptParams>)>>,
    mut rm_state: RemovedComponents<LinkState>,
) -> bool {
    // Drain unconditionally — `fold`, not `any`, which would short-circuit and
    // leave the buffer to accumulate.
    let removed = rm_state.read().fold(false, |acc, _| acc | true);
    let run = !*first || removed || !q_state.is_empty() || !q_templates.is_empty();
    *first = true;
    run
}

/// Re-point every existing beam. Runs every frame — endpoints move constantly.
///
/// O(beams), and touches nothing else: the peer `Entity` and the tuning were
/// resolved by the reconciler and cached on [`LinkBeamInstance`].
fn aim_link_beams(
    mut q_beams: Query<(&ChildOf, &LinkBeamInstance, &mut Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<LinkBeamInstance>>,
    q_cam: Query<(&Camera, Entity), With<Camera3d>>,
) {
    for (co, inst, mut tf) in &mut q_beams {
        // A cached peer can outlive its entity between reconciles. Skip rather
        // than guess — the next reconcile either refreshes it or despawns us.
        let (Some((npos, nrot)), Some((ppos, _))) = (
            world_pose(co.parent(), &q_parents, &q_grids, &q_spatial),
            world_pose(inst.peer_entity, &q_parents, &q_grids, &q_spatial),
        ) else {
            continue;
        };
        let world_dir = ppos - npos;
        let dist = world_dir.length();
        if dist < 1.0 {
            continue;
        }
        let dir_local = (nrot.0.inverse() * (world_dir / dist)).as_vec3();
        // Measured per beam rather than cached on the instance: the camera moves
        // every frame, which is exactly the cadence this system already runs at.
        let camera_dists = camera_distances(npos, ppos, &q_cam, &q_parents, &q_grids, &q_spatial);
        let len = beam_len(
            dist,
            inst.near_m,
            inst.stub,
            inst.stub_cam_frac,
            camera_dists,
        );
        let want = beam_transform(dir_local, len, inst.width);
        // Compare-gated: an unconditional write marks the `Transform` `Changed`
        // and re-propagates the beam every frame even when both endpoints are
        // parked. When they move, the recomputed transform differs and writes.
        if *tf != want {
            *tf = want;
        }
    }
}

/// A unit +Y cylinder (Bevy `Cylinder` is centred on the origin) → a beam from the node
/// along `dir_local` for `len`, `half_width` thick.
fn beam_transform(dir_local: Vec3, len: f32, half_width: f32) -> Transform {
    Transform {
        translation: dir_local * (len * 0.5),
        rotation: Quat::from_rotation_arc(Vec3::Y, dir_local),
        scale: Vec3::new(half_width, len, half_width),
    }
}

/// Walk up from a beam template to the nearest ancestor that carries a `LinkState` — the
/// link node. Nesting-agnostic, so the part can sit under any wrapper.
fn node_of<F: bevy::ecs::query::QueryData>(
    start: Entity,
    q_parents: &Query<&ChildOf>,
    q_nodes: &Query<F>,
) -> Option<Entity> {
    let mut e = start;
    loop {
        // The authored LinkNode is the endpoint geometry's ownership fact.
        // LinkState is published later by the cadence-gated solver and may be
        // attached to a wrapper; using it here made a cloned beam originate at
        // the wrapper/chassis instead of at the antenna phase centre.
        if q_nodes.get(e).is_ok() {
            return Some(e);
        }
        e = q_parents.get(e).ok()?.parent();
    }
}

#[allow(clippy::too_many_arguments)]
fn reconcile_link_beams(
    mut commands: Commands,
    // Each template IS a `ProgramDriverId` prim (an authored `Cylinder`): mesh + bound
    // material + params. There are two per node — `Up` and `Down`.
    q_templates: Query<(
        Entity,
        &ProgramDriverId,
        &Mesh3d,
        &MeshMaterial3d<StandardMaterial>,
        Option<&ScriptParams>,
        &Visibility,
        &ChildOf,
    )>,
    q_state: Query<&LinkState>,
    q_nodes: Query<(&lunco_celestial::link::LinkNode, Option<&Name>)>,
    q_names: Query<&Name>,
    q_ids: Query<(Entity, &GlobalEntityId)>,
    q_beams: Query<(Entity, &ChildOf, &LinkBeamInstance)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_cam: Query<(&Camera, Entity), With<Camera3d>>,
) {
    // GID → entity, so a peer (named by identity in `LinkState`) resolves to something
    // `world_pose` can place.
    let ent_of: HashMap<u64, Entity> = q_ids.iter().map(|(e, g)| (g.get(), e)).collect();

    // Pass 1: gather the Up/Down templates + tuning per node.
    let mut nodes: HashMap<Entity, NodeBeams> = HashMap::new();
    for (tmpl, id, mesh, mat, params, vis, parent) in &q_templates {
        if id.0 != DRIVER_ID {
            continue;
        }
        // A TEMPLATE MUST NEVER DRAW. It is a unit cylinder (radius 1, height 1)
        // that exists only to carry a mesh + material + params for cloning, and it
        // sits at the link node's own origin — so when it renders it is a ~1 m
        // glowing green lozenge stuck to the antenna dish, once per link node.
        //
        // The part authors `lunco:placeholder = true` to say this, and the USD
        // loader maps that to `Visibility::Hidden`. That flag's DEFINED meaning,
        // though, is the glTF one — "primitive stand-in until an async payload
        // loads", paired with `GlbPlaceholder` and revealed by
        // `hide_glb_placeholder_meshes`. Borrowing it for "never draw, ever" makes
        // this driver's correctness depend on a flag that belongs to a different
        // feature and could reasonably change with it.
        //
        // The driver is what DEFINES a template — this query is the definition —
        // so the driver states the invariant itself. Authored intent stays in the
        // asset (and third-party tools still read it); the guarantee lives here.
        // Compare-gated: an unconditional insert re-marks `Visibility` changed and
        // re-propagates the subtree on every reconcile.
        if *vis != Visibility::Hidden {
            commands.entity(tmpl).try_insert(Visibility::Hidden);
        }
        let Some(node) = node_of(tmpl, &q_parents, &q_nodes) else {
            continue;
        };
        let origin = q_parents
            .get(parent.parent())
            .map(ChildOf::parent)
            .unwrap_or(parent.parent());
        let get = |k: &str, d: f64| params.and_then(|p| p.0.get(k).copied()).unwrap_or(d);
        let nb = nodes.entry(node).or_default();
        nb.origin = Some(origin);
        if get("state", 0.0) >= 0.5 {
            nb.down = Some((mesh.clone(), mat.clone()));
        } else {
            nb.up = Some((mesh.clone(), mat.clone()));
            nb.width = get("width", DEF_WIDTH) as f32;
            nb.near_m = get("nearM", DEF_NEAR_M);
            nb.stub = get("stubLen", DEF_STUB) as f32;
            nb.stub_cam_frac = get("stubCamFrac", DEF_STUB_CAM_FRAC) as f32;
            nb.mode = get("mode", 0.0);
            nb.show_down = get("showDown", 0.0) >= 0.5;
            nb.earth_only = get("earthOnly", 0.0) >= 0.5;
        }
    }

    // Pass 2: reconcile one beam per wanted peer against what is already spawned.
    for (node, nb) in &nodes {
        let node = *node;
        let Ok(state) = q_state.get(node) else {
            continue;
        };
        let Some(up) = nb.up.as_ref() else { continue };
        let source = nb.origin.unwrap_or(node);
        let show_down = nb.show_down && nb.down.is_some();

        // Which (peer, is_up) pairs to draw. `off` draws nothing; `active` keeps only the
        // nearest connected peer; `all` draws every connected peer, plus severed ones as
        // red when `showDown` is on.
        let is_earth_peer = |peer_gid: u64| -> bool {
            let Some(&pe) = ent_of.get(&peer_gid) else {
                return false;
            };
            if let Ok((node_info, name)) = q_nodes.get(pe) {
                if let Some(ref class) = node_info.class {
                    if class.eq_ignore_ascii_case("earth") {
                        return true;
                    }
                }
                if let Some(n) = name {
                    if n.as_str().to_ascii_lowercase().contains("earth")
                        || n.as_str().to_ascii_lowercase().contains("groundstation")
                    {
                        return true;
                    }
                }
            }
            // Also walk ancestors to check if any ancestor prim is a GroundStation or Earth node
            let mut curr = pe;
            while let Ok(child) = q_parents.get(curr) {
                curr = child.parent();
                if let Ok((node_info, _)) = q_nodes.get(curr) {
                    if let Some(ref class) = node_info.class {
                        if class.eq_ignore_ascii_case("earth") {
                            return true;
                        }
                    }
                }
                if let Ok(n) = q_names.get(curr) {
                    if n.as_str().to_ascii_lowercase().contains("earth")
                        || n.as_str().to_ascii_lowercase().contains("groundstation")
                    {
                        return true;
                    }
                }
            }
            false
        };

        let mut wanted: Vec<(u64, bool)> = Vec::new();
        if nb.mode < 1.5 {
            if nb.mode >= 0.5 {
                if let Some(p) = state
                    .peers
                    .iter()
                    .filter(|p| p.connected && (!nb.earth_only || is_earth_peer(p.peer)))
                    .min_by(|a, b| a.range_m.total_cmp(&b.range_m))
                {
                    wanted.push((p.peer, true));
                }
            } else {
                for p in &state.peers {
                    if nb.earth_only && !is_earth_peer(p.peer) {
                        continue;
                    }
                    if p.connected {
                        wanted.push((p.peer, true));
                    } else if show_down {
                        wanted.push((p.peer, false));
                    }
                }
            }
        }
        let wanted_ids: HashSet<u64> = wanted.iter().map(|(g, _)| *g).collect();

        let Some((npos, nrot)) = world_pose(source, &q_parents, &q_grids, &q_spatial) else {
            continue;
        };
        let nrot_inv = nrot.0.inverse();
        // Beams already spawned for this node, by peer.
        let existing: HashMap<u64, (Entity, bool)> = q_beams
            .iter()
            .filter(|(_, co, _)| co.parent() == source)
            .map(|(e, _, inst)| (inst.peer, (e, inst.up)))
            .collect();

        for (peer_gid, is_up) in wanted {
            let Some(&pe) = ent_of.get(&peer_gid) else {
                continue;
            };
            let Some((ppos, _)) = world_pose(pe, &q_parents, &q_grids, &q_spatial) else {
                continue;
            };
            let world_dir = ppos - npos;
            let dist = world_dir.length();
            if dist < 1.0 {
                continue;
            }
            let dir_local = (nrot_inv * (world_dir / dist)).as_vec3();
            let camera_dists =
                camera_distances(npos, ppos, &q_cam, &q_parents, &q_grids, &q_spatial);
            let len = beam_len(dist, nb.near_m, nb.stub, nb.stub_cam_frac, camera_dists);
            let tf = beam_transform(dir_local, len, nb.width);
            let (mesh, mat) = if is_up { up } else { nb.down.as_ref().unwrap() };

            match existing.get(&peer_gid) {
                Some(&(beam, was_up)) => {
                    // Refresh the cached peer + tuning on EVERY reconcile, not just on a
                    // flip: that cache is what `aim_link_beams` trusts, and a peer that
                    // was despawned and respawned keeps its GID but not its `Entity`.
                    commands.entity(beam).try_insert((
                        tf,
                        LinkBeamInstance {
                            peer: peer_gid,
                            up: is_up,
                            peer_entity: pe,
                            near_m: nb.near_m,
                            stub: nb.stub,
                            stub_cam_frac: nb.stub_cam_frac,
                            width: nb.width,
                        },
                    ));
                    if was_up != is_up {
                        // State flipped — swap to the other authored material.
                        commands.entity(beam).try_insert((
                            mesh.clone(),
                            mat.clone(),
                            LinkBeamInstance {
                                peer: peer_gid,
                                up: is_up,
                                peer_entity: pe,
                                near_m: nb.near_m,
                                stub: nb.stub,
                                stub_cam_frac: nb.stub_cam_frac,
                                width: nb.width,
                            },
                        ));
                    }
                }
                None => {
                    // `Visibility::Visible` so a beam shows even though the template it
                    // was cloned from is a hidden placeholder. `LowPrecisionRoot` because
                    // the node is a high-precision (cell-anchored) big_space entity, and a
                    // plain-Transform child of one must mark itself the root of a
                    // low-precision subtree — else big_space's hierarchy validator panics
                    // (see `trajectories.rs`).
                    commands.spawn((
                        mesh.clone(),
                        mat.clone(),
                        tf,
                        Visibility::Visible,
                        // A beam is emitted light, not matter — it must not cast a
                        // shadow. The authored `primvars:doNotCastShadows` sits on the
                        // template prim, but cloning copies only mesh+material, so stamp
                        // the marker explicitly on each instance.
                        bevy::light::NotShadowCaster,
                        big_space::grid::propagation::LowPrecisionRoot,
                        LinkBeamInstance {
                            peer: peer_gid,
                            up: is_up,
                            peer_entity: pe,
                            near_m: nb.near_m,
                            stub: nb.stub,
                            stub_cam_frac: nb.stub_cam_frac,
                            width: nb.width,
                        },
                        lunco_core::NoSelectionBounds,
                        // A beam is owned and churned by THIS reconciler — spawned when a
                        // peer appears, despawned when it drops. It is runtime detail, not
                        // scene content, which is precisely what `SystemManaged` marks.
                        //
                        // Without it a beam is indistinguishable from an authored entity,
                        // so every spawn trips `Added<Mesh3d>` in `scene_topology_changed`
                        // and rebuilds the whole entity tree — a `String` per named entity
                        // — to produce an identical list, since the beam has no `Name` and
                        // never appears in it. `NoSelectionBounds` alone does not say this:
                        // that marker is about picking, this one is about ownership.
                        lunco_core::SystemManaged,
                        ChildOf(source),
                    ));
                }
            }
        }

        for (peer, (beam, _)) in existing {
            if !wanted_ids.contains(&peer) {
                commands.entity(beam).try_despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::beam_len;

    #[test]
    fn far_stub_uses_the_nearest_camera_endpoint() {
        let local_source = beam_len(1.0e9, 50_000.0, 100_000.0, 0.35, Some((10.0, 1.0e9)));
        assert_eq!(local_source, 3.5);

        let earth_source = beam_len(1.0e9, 50_000.0, 100_000.0, 0.35, Some((1.0e9, 10.0)));
        assert_eq!(earth_source, 3.5);

        let globe_source = beam_len(
            1.0e9,
            50_000.0,
            100_000.0,
            0.35,
            Some((10_000_000.0, 20_000_000.0)),
        );
        assert_eq!(globe_source, 3_500_000.0);

        let headless = beam_len(1.0e9, 50_000.0, 100_000.0, 0.35, None);
        assert_eq!(headless, 100_000.0);
    }

    #[test]
    fn near_peer_keeps_its_real_span() {
        assert_eq!(
            beam_len(12.5, 50_000.0, 100_000.0, 0.35, Some((10.0, 20.0))),
            12.5
        );
    }

    #[test]
    fn camera_stub_has_a_visible_minimum() {
        assert_eq!(
            beam_len(1.0e9, 50_000.0, 100_000.0, 0.35, Some((0.0, 1.0e9))),
            1.0
        );
    }

    #[test]
    fn nonpositive_camera_fraction_uses_headless_stub() {
        assert_eq!(
            beam_len(1.0e9, 50_000.0, 100_000.0, 0.0, Some((10.0, 1.0e9))),
            100_000.0
        );
    }
}
