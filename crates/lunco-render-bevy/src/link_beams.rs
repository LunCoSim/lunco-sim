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
//! each a local [`Transform`] aimed at that peer — near = full span, far = a fixed stub
//! (a 384,000 km Earth beam would be off-screen and jitter). Direction is [`world_pose`]
//! (f64, cell-aware) both ends, so nothing jitters. Cloning a `Handle` is a cheap `Arc`
//! bump and Bevy GPU-batches shared-handle instances into one draw call — this scales to
//! a lidar's many rays unchanged.

use std::collections::{HashMap, HashSet};

use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::link::LinkState;
use lunco_core::coords::world_pose;
use lunco_core::programs::{ProgramDriverAppExt, ProgramDriverId};
use lunco_core::{GlobalEntityId, ScriptParams};

/// The `lunco:program:id` the beam part authors.
const DRIVER_ID: &str = "link_beams";

// Fallbacks when the part authors no `lunco:param:*`.
const DEF_WIDTH: f64 = 0.12;
const DEF_NEAR_M: f64 = 50_000.0;
const DEF_STUB: f64 = 20.0;

/// Tags a spawned beam with its peer and the state it currently shows, so the reconciler
/// can re-aim it, recolour it on a flip, or despawn it when the peer drops out.
#[derive(Component)]
struct LinkBeamInstance {
    peer: u64,
    up: bool,
}

type Look = (Mesh3d, MeshMaterial3d<StandardMaterial>);

/// The authored templates + tuning gathered for one node.
#[derive(Default)]
struct NodeBeams {
    up: Option<Look>,
    down: Option<Look>,
    width: f32,
    near_m: f64,
    stub: f32,
    mode: f64,
    show_down: bool,
}

pub(crate) fn build(app: &mut App) {
    app.register_program_driver(DRIVER_ID, drive_link_beams);
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
fn node_of(
    start: Entity,
    q_parents: &Query<&ChildOf>,
    q_state: &Query<&LinkState>,
) -> Option<Entity> {
    let mut e = start;
    loop {
        if q_state.get(e).is_ok() {
            return Some(e);
        }
        e = q_parents.get(e).ok()?.parent();
    }
}

#[allow(clippy::too_many_arguments)]
fn drive_link_beams(
    mut commands: Commands,
    // Each template IS a `ProgramDriverId` prim (an authored `Cylinder`): mesh + bound
    // material + params. There are two per node — `Up` and `Down`.
    q_templates: Query<(
        Entity,
        &ProgramDriverId,
        &Mesh3d,
        &MeshMaterial3d<StandardMaterial>,
        Option<&ScriptParams>,
    )>,
    q_state: Query<&LinkState>,
    q_ids: Query<(Entity, &GlobalEntityId)>,
    q_beams: Query<(Entity, &ChildOf, &LinkBeamInstance)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    // GID → entity, so a peer (named by identity in `LinkState`) resolves to something
    // `world_pose` can place.
    let ent_of: HashMap<u64, Entity> = q_ids.iter().map(|(e, g)| (g.get(), e)).collect();

    // Pass 1: gather the Up/Down templates + tuning per node.
    let mut nodes: HashMap<Entity, NodeBeams> = HashMap::new();
    for (tmpl, id, mesh, mat, params) in &q_templates {
        if id.0 != DRIVER_ID {
            continue;
        }
        let Some(node) = node_of(tmpl, &q_parents, &q_state) else { continue };
        let get = |k: &str, d: f64| params.and_then(|p| p.0.get(k).copied()).unwrap_or(d);
        let nb = nodes.entry(node).or_default();
        if get("state", 0.0) >= 0.5 {
            nb.down = Some((mesh.clone(), mat.clone()));
        } else {
            nb.up = Some((mesh.clone(), mat.clone()));
            nb.width = get("width", DEF_WIDTH) as f32;
            nb.near_m = get("nearM", DEF_NEAR_M);
            nb.stub = get("stubLen", DEF_STUB) as f32;
            nb.mode = get("mode", 0.0);
            nb.show_down = get("showDown", 0.0) >= 0.5;
        }
    }

    // Pass 2: reconcile one beam per wanted peer against what is already spawned.
    for (node, nb) in &nodes {
        let node = *node;
        let Ok(state) = q_state.get(node) else { continue };
        let Some(up) = nb.up.as_ref() else { continue };
        let show_down = nb.show_down && nb.down.is_some();

        // Which (peer, is_up) pairs to draw. `off` draws nothing; `active` keeps only the
        // nearest connected peer; `all` draws every connected peer, plus severed ones as
        // red when `showDown` is on.
        let mut wanted: Vec<(u64, bool)> = Vec::new();
        if nb.mode < 1.5 {
            if nb.mode >= 0.5 {
                if let Some(p) = state
                    .peers
                    .iter()
                    .filter(|p| p.connected)
                    .min_by(|a, b| a.range_m.total_cmp(&b.range_m))
                {
                    wanted.push((p.peer, true));
                }
            } else {
                for p in &state.peers {
                    if p.connected {
                        wanted.push((p.peer, true));
                    } else if show_down {
                        wanted.push((p.peer, false));
                    }
                }
            }
        }
        let wanted_ids: HashSet<u64> = wanted.iter().map(|(g, _)| *g).collect();

        let Some((npos, nrot)) = world_pose(node, &q_parents, &q_grids, &q_spatial) else {
            continue;
        };
        let nrot_inv = nrot.inverse();

        // Beams already spawned for this node, by peer.
        let existing: HashMap<u64, (Entity, bool)> = q_beams
            .iter()
            .filter(|(_, co, _)| co.parent() == node)
            .map(|(e, _, inst)| (inst.peer, (e, inst.up)))
            .collect();

        for (peer_gid, is_up) in wanted {
            let Some(&pe) = ent_of.get(&peer_gid) else { continue };
            let Some((ppos, _)) = world_pose(pe, &q_parents, &q_grids, &q_spatial) else {
                continue;
            };
            let world_dir = ppos - npos;
            let dist = world_dir.length();
            if dist < 1.0 {
                continue;
            }
            let dir_local = (nrot_inv * (world_dir / dist)).as_vec3();
            let len = if dist <= nb.near_m { dist as f32 } else { nb.stub };
            let tf = beam_transform(dir_local, len, nb.width);
            let (mesh, mat) = if is_up { up } else { nb.down.as_ref().unwrap() };

            match existing.get(&peer_gid) {
                Some(&(beam, was_up)) => {
                    commands.entity(beam).try_insert(tf);
                    if was_up != is_up {
                        // State flipped — swap to the other authored material.
                        commands.entity(beam).try_insert((
                            mesh.clone(),
                            mat.clone(),
                            LinkBeamInstance { peer: peer_gid, up: is_up },
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
                        LinkBeamInstance { peer: peer_gid, up: is_up },
                        ChildOf(node),
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
