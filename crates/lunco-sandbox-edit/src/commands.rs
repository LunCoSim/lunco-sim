//! Command handlers for sandbox-edit world manipulation.
//!
//! - `SpawnEntity` — spawn from the catalog at a world position.
//! - `MoveEntity` — teleport an entity to an absolute world position.
//!   Mirrors what the gizmo does on drag: swap to Kinematic, update
//!   Transform/Position/LinearVelocity, so joint constraints
//!   propagate the move to coupled bodies. Lets API clients
//!   (MCP tools, automated tests) drive entity motion exactly the
//!   way a human would with the gizmo.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{AngularVelocity, LinearVelocity, PhysicsSystems, RigidBody};
use avian3d::physics_transform::Position;
use big_space::prelude::Grid;
use lunco_core::Command;
use std::collections::{HashMap, VecDeque};
use crate::catalog::{SpawnCatalog, spawn_procedural, spawn_usd_entry};

/// Spawn an entity from the catalog at a given world position.
#[Command]
pub struct SpawnEntity {
    /// The grid entity to spawn under.
    pub target: Entity,
    /// The catalog entry ID (e.g. "ball_dynamic", "skid_rover").
    pub entry_id: String,
    /// World-space position (x, y, z).
    pub position: Vec3,
}

/// Observer that handles SpawnEntity commands.
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
    role: Res<lunco_core::NetworkRole>,
) {
    let cmd = trigger.event();

    // On a pure client, spawning is the host's job: the command is captured and
    // sent to the host, which spawns the authoritative rover and replicates it
    // back (arriving via `apply_replicated_spawns`). Don't spawn locally, or the
    // client would get a duplicate with no server identity.
    if matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }

    let entry = match catalog.get(&cmd.entry_id) {
        Some(e) => e,
        None => {
            warn!("SPAWN_ENTITY: unknown entry '{}'", cmd.entry_id);
            return;
        }
    };

    // Prefer the requested grid; fall back to the first grid (a wire-applied
    // spawn may carry a grid id that doesn't resolve on this peer).
    let grid = match q_grids.get(cmd.target).ok().or_else(|| q_grids.iter().next()) {
        Some(g) => g,
        None => {
            warn!("SPAWN_ENTITY: no grid to spawn under");
            return;
        }
    };

    info!("SPAWN_ENTITY: {} at {:?}", cmd.entry_id, cmd.position);

    let result = match entry.source {
        crate::catalog::SpawnSource::Procedural(_) => {
            spawn_procedural(&mut commands, &mut meshes, &mut materials, entry, cmd.position, grid)
        }
        crate::catalog::SpawnSource::UsdFile(_) => {
            spawn_usd_entry(&mut commands, &asset_server, entry, cmd.position, grid)
        }
    };

    // Networked identity (gap G2): a runtime instance gets a server-allocated
    // unique id (SkipContentStamp → assign_global_entity_ids mints
    // Authoritative, never colliding `Content`), is marked for transform
    // replication, and records what to replicate so the host can broadcast the
    // spawn to clients.
    commands.entity(result.root_entity).insert((
        lunco_core::SkipContentStamp,
        lunco_core::NetReplicate,
        lunco_core::NetSpawn {
            entry_id: cmd.entry_id.clone(),
            position: cmd.position,
        },
    ));
}

/// Client: instantiate rovers the host has replicated to us (M1 content
/// reconstruction — geometry loads locally, pinned to the host-allocated id).
/// No-op on host/standalone (queue stays empty).
pub fn apply_replicated_spawns(
    mut pending: ResMut<lunco_core::PendingReplicatedSpawns>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
) {
    if pending.0.is_empty() {
        return;
    }
    // Wait until a grid exists (scene still loading) — keep the queue.
    let Some(grid) = q_grids.iter().next() else {
        return;
    };
    for job in pending.0.drain(..).collect::<Vec<_>>() {
        let Some(entry) = catalog.get(&job.entry_id) else {
            warn!("REPL_SPAWN: unknown entry '{}'", job.entry_id);
            continue;
        };
        let pos = job.position;
        let result = match entry.source {
            crate::catalog::SpawnSource::Procedural(_) => {
                spawn_procedural(&mut commands, &mut meshes, &mut materials, entry, pos, grid)
            }
            crate::catalog::SpawnSource::UsdFile(_) => {
                spawn_usd_entry(&mut commands, &asset_server, entry, pos, grid)
            }
        };
        // Pin the host id; mark runtime instance + replication target. Forced
        // Kinematic by `force_kinematic_proxies` so snapshots drive it.
        commands.entity(result.root_entity).insert((
            lunco_core::GlobalEntityId::from_raw(job.gid),
            lunco_core::SkipContentStamp,
            lunco_core::NetReplicate,
        ));
    }
}

/// Client: force replicated proxies to `Kinematic` so the host-authoritative
/// transform (applied via snapshots) is not fought by local physics
/// integration — and, crucially, so the proxy does **not** free-fall under
/// gravity while the host is idle (snapshots pause under `only_if_changed`).
///
/// Re-asserts every frame rather than keying on `Changed<RigidBody>`: the USD
/// rover's cosim/flight-software re-inserts a `Dynamic` body *after* the asset
/// loads, which a one-shot `Changed` filter races and misses — leaving the
/// proxy dynamic and sinking through the floor. The `!Kinematic` guard makes
/// the steady state a no-op.
pub fn force_kinematic_proxies(
    role: Res<lunco_core::NetworkRole>,
    mut commands: Commands,
    mut q: Query<
        (
            Entity,
            &RigidBody,
            Option<&mut LinearVelocity>,
            Option<&mut AngularVelocity>,
        ),
        // Predict-own: the rover this client possesses (`OwnedLocally`) is
        // excluded — it runs its own avian step as a `Dynamic` body instead of
        // being pinned `Kinematic`, so its velocities are NOT zeroed here.
        (
            With<lunco_core::NetReplicate>,
            Without<lunco_core::OwnedLocally>,
        ),
    >,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, rb, lin, ang) in q.iter_mut() {
        // `RigidBody` is an immutable Avian component — replace it via `insert`.
        if !matches!(*rb, RigidBody::Kinematic) {
            commands.entity(e).insert(RigidBody::Kinematic);
        }
        // A kinematic body keeps gliding at whatever velocity it carried when it
        // turned kinematic (a settled rover micro-jitters → glides → the next
        // snapshot snaps it back → *blinking*; a balloon glides smoothly →
        // drifts off). Zero it every frame so the proxy HOLDS the last snapshot
        // between updates instead of running its own physics.
        if let Some(mut l) = lin {
            l.0 = DVec3::ZERO;
        }
        if let Some(mut a) = ang {
            a.0 = DVec3::ZERO;
        }
    }
}

/// One buffered transform sample for client-side interpolation, stamped with
/// the local receipt time.
#[derive(Clone, Copy)]
struct InterpSample {
    t_recv: f32,
    pos: Vec3,
    rot: Quat,
    /// Authoritative velocities from the snapshot (owned-rover prediction uses
    /// these; remote interpolation ignores them).
    lv: Vec3,
    av: Vec3,
    /// Highest input seq the host applied for this gid as of this snapshot (the
    /// reconcile ack). 0 = none.
    last_input_seq: u32,
}

/// Per-[`lunco_core::GlobalEntityId`] ring of recent snapshot samples. The
/// client renders replicated bodies by interpolating ~[`INTERP_DELAY`] in the
/// past instead of hard-snapping to each 20 Hz snapshot (which looked like
/// teleport "jumps"). Client-only; stays empty on host/standalone.
#[derive(Resource, Default)]
pub struct InterpBuffers(HashMap<u64, VecDeque<InterpSample>>);

/// Render this far behind real time so two samples normally bracket the render
/// instant to interpolate between (≈2–3 snapshots at the 20 Hz default). Higher
/// = smoother under jitter but more visible lag on the bodies you watch.
const INTERP_DELAY: f32 = 0.12;
/// Cap per-body history (seconds of buffer at 20 Hz; only the recent tail is read).
const INTERP_MAX_SAMPLES: usize = 16;

/// Client: file each incoming snapshot into its per-entity interpolation buffer,
/// stamped with local receipt time. Replaces the old hard-set so motion is
/// smoothed by [`interpolate_proxies`] rather than teleported.
pub fn ingest_snapshots(
    // REAL (wall-clock) time, not the virtual clock — the client may run paused
    // (no local sim), which would freeze `Time<Virtual>` and stall interpolation.
    time: Res<Time<bevy::time::Real>>,
    mut snaps: ResMut<lunco_core::IncomingSnapshots>,
    mut buffers: ResMut<InterpBuffers>,
) {
    if snaps.0.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    for s in snaps.0.drain(..).collect::<Vec<_>>() {
        let buf = buffers.0.entry(s.gid).or_default();
        buf.push_back(InterpSample {
            t_recv: now,
            pos: Vec3::from(s.t),
            rot: Quat::from_array(s.r),
            lv: Vec3::from(s.lv),
            av: Vec3::from(s.av),
            last_input_seq: s.last_input_seq,
        });
        while buf.len() > INTERP_MAX_SAMPLES {
            buf.pop_front();
        }
    }
}

/// Client: drive each replicated proxy from its interpolation buffer, rendering
/// [`INTERP_DELAY`] in the past and lerp/slerping between the two bracketing
/// samples — turning 20 Hz snapshots into smooth per-frame motion. Writes both
/// `Transform` and avian `Position` (so the physics transform-sync doesn't
/// overwrite it). A body with no fresh samples holds its last pose, so a rover
/// at rest sits still instead of snapping.
///
/// (Currently interpolates *every* replicated body, including the one this
/// client possesses — that one is smooth but ~`INTERP_DELAY` behind your input.
/// Client-side prediction for the possessed rover is the follow-up that makes
/// your own vessel crisp; everyone else's stays interpolated.)
pub fn interpolate_proxies(
    // Must share the same clock `ingest_snapshots` stamps with — REAL time, so a
    // paused client still renders smooth replicated motion.
    time: Res<Time<bevy::time::Real>>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    buffers: Res<InterpBuffers>,
    // Predict-own: the possessed rover is locally simulated + smooth-corrected
    // (`correct_owned_prediction`), so it must NOT be dragged back to the
    // `INTERP_DELAY`-old interpolated pose here.
    q_owned: Query<(), With<lunco_core::OwnedLocally>>,
    mut q: Query<(&mut Transform, Option<&mut Position>)>,
) {
    let render_t = time.elapsed_secs() - INTERP_DELAY;
    for (gid, buf) in buffers.0.iter() {
        if buf.is_empty() {
            continue;
        }
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(*gid)) else {
            continue;
        };
        if q_owned.contains(e) {
            continue; // predict-own: my vessel is driven locally, not interpolated
        }
        let Ok((mut tf, pos)) = q.get_mut(e) else {
            continue;
        };

        // Samples are time-ordered: `a` = latest at/just before render_t,
        // `b` = first after it.
        let mut a: Option<&InterpSample> = None;
        let mut b: Option<&InterpSample> = None;
        for s in buf.iter() {
            if s.t_recv <= render_t {
                a = Some(s);
            } else {
                b = Some(s);
                break;
            }
        }

        let (out_pos, out_rot) = match (a, b) {
            (Some(a), Some(b)) => {
                let span = (b.t_recv - a.t_recv).max(1e-5);
                let alpha = ((render_t - a.t_recv) / span).clamp(0.0, 1.0);
                (a.pos.lerp(b.pos, alpha), a.rot.slerp(b.rot, alpha))
            }
            // render_t before the oldest sample → snap to oldest; past the
            // newest (starved / at rest) → hold newest.
            (None, Some(b)) => (b.pos, b.rot),
            (Some(a), None) => (a.pos, a.rot),
            (None, None) => continue,
        };

        tf.translation = out_pos;
        tf.rotation = out_rot;
        if let Some(mut p) = pos {
            p.0 = DVec3::new(out_pos.x as f64, out_pos.y as f64, out_pos.z as f64);
        }
    }
}

/// Client predict-own classifier: keep the [`lunco_core::OwnedLocally`] marker
/// in sync with the authoritative ownership table. This is the **single** place
/// that decides which replicated body this peer predicts locally (the rover it
/// possesses) versus interpolates as a remote proxy — every other predict-own
/// seam just reads the marker.
///
/// Client-only: on host/standalone every body is simulated authoritatively, so
/// no per-body marker is wanted (and `reg` would mark the host's *own* rovers,
/// not remote-owned ones — wrong meaning there). Ownership flips here (steal /
/// release) flow to all seams at once: losing the marker re-pins the body
/// `Kinematic` + re-interpolates it next frame; gaining it flips it `Dynamic`.
pub fn maintain_owned_locally(
    role: Res<lunco_core::NetworkRole>,
    local: Res<lunco_core::LocalSession>,
    reg: Res<lunco_core::SessionRegistry>,
    mut commands: Commands,
    q: Query<
        (Entity, &lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>),
        With<lunco_core::NetReplicate>,
    >,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, gid, has_marker) in q.iter() {
        let mine = reg.owns(local.0, gid.get());
        match (mine, has_marker) {
            (true, false) => {
                // Gaining ownership: mark it AND restore `Dynamic`. The marker
                // only *excludes* the body from `force_kinematic_proxies`; it
                // does NOT un-pin a body that was already forced `Kinematic`
                // (the common case: a replicated proxy is pinned for many frames
                // before this peer possesses it). Without this insert the rover
                // stays `Kinematic`, mobility's per-chassis guard skips it, and
                // predict-own is inert. Losing ownership needs no counterpart —
                // `force_kinematic_proxies` re-pins `Kinematic` + zeros velocity.
                commands
                    .entity(e)
                    .insert((lunco_core::OwnedLocally, RigidBody::Dynamic));
            }
            (false, true) => {
                commands.entity(e).remove::<lunco_core::OwnedLocally>();
            }
            _ => {}
        }
    }
}

/// Below this positional error (m) between our prediction-at-the-acked-seq and the
/// authoritative state, the prediction is "correct" and we do **nothing**. This is
/// the whole point of input-replay reconciliation: we compare at the SAME input
/// `seq` (not against the latency-delayed pose), so the legitimate latency lead
/// never triggers a pull — no rubber-band. Only a genuine divergence past this
/// tolerance (numerical drift accumulation, or a misprediction) is reconciled.
const RECONCILE_EPS_POS: f32 = 0.25;
/// Angular tolerance (rad, ≈1.7°) for the same "prediction correct" test.
const RECONCILE_EPS_ROT: f32 = 0.03;
/// Beyond this positional divergence (m) the prediction has grossly desynced
/// (teleport / respawn / long stall) — hard-snap fully to authority.
const RECONCILE_SNAP_POS: f32 = 6.0;
/// Cap on the predicted-state history ring (~2 s at 60 Hz). Only the recent tail
/// (the unacked window) is ever compared.
const MAX_PREDICTED_HISTORY: usize = 128;
/// P5 render-smoothing: fraction of a misprediction's POSE error applied per ack;
/// the rest eases in over the next few snapshots, so the (rare) correction slides
/// rather than jumps. Velocity is seated fully regardless, so the body stops
/// re-diverging and the residual converges in ~3–4 acks.
const RECONCILE_CORRECT_BLEND: f32 = 0.3;

/// One recorded predicted state of the owned rover after the fixed step that
/// applied input `seq`. Compared against the authoritative snapshot acking that
/// same `seq` — apples-to-apples, so the latency lead cancels.
#[derive(Clone, Copy)]
struct PredictedState {
    seq: u32,
    pos: Vec3,
    rot: Quat,
}

/// Per-vessel predicted-state history + the highest seq we've reconciled. Keyed
/// by [`lunco_core::GlobalEntityId`] raw `u64`. Client-only; empty otherwise.
#[derive(Default)]
struct BodyPredictionLog {
    ring: VecDeque<PredictedState>,
    last_reconciled: u32,
}

/// History of the owned rover's predicted poses, keyed by gid.
#[derive(Resource, Default)]
pub struct PredictedStateLog(HashMap<u64, BodyPredictionLog>);

/// Client predict-own: record the owned rover's post-step pose each fixed tick,
/// keyed by the input `seq` applied that tick (from [`lunco_core::OwnedInputLog`]).
/// Runs in `FixedPostUpdate` after avian writeback, after [`reconcile_owned_prediction`]
/// so the history reflects any correction. Reads `Transform` (post-writeback =
/// the avian pose, f32) so it never touches avian's f64 `Rotation` component.
pub fn record_predicted_state(
    input_log: Res<lunco_core::OwnedInputLog>,
    mut hist: ResMut<PredictedStateLog>,
    q: Query<(&lunco_core::GlobalEntityId, &Transform), With<lunco_core::OwnedLocally>>,
) {
    for (gid, tf) in q.iter() {
        let g = gid.get();
        // The seq the controller emitted for this fixed tick (newest input frame).
        let Some(seq) = input_log
            .0
            .get(&g)
            .and_then(|l| l.frames.back())
            .map(|f| f.seq)
        else {
            continue;
        };
        let vlog = hist.0.entry(g).or_default();
        if vlog.ring.back().is_some_and(|s| s.seq == seq) {
            continue; // already recorded this seq (multiple FixedUpdates, one input)
        }
        vlog.ring.push_back(PredictedState {
            seq,
            pos: tf.translation,
            rot: tf.rotation,
        });
        while vlog.ring.len() > MAX_PREDICTED_HISTORY {
            vlog.ring.pop_front();
        }
    }
}

/// Client predict-own reconciliation (input-replay model, D2). GENERAL over any
/// owned, locally-predicted moving body — it keys off [`lunco_core::OwnedLocally`]
/// + gid and corrects an arbitrary dynamic body's Transform/Position/velocity; it
/// assumes nothing about "rover". (Only the *input* that drives the body, e.g.
/// `DriveRover`, is domain-specific — the predict-and-reconcile substrate is not.)
///
/// On each snapshot that acks a NEW input `seq` for an owned body, compare what we
/// predicted at that seq (`PredictedStateLog`) to the authoritative state. **If
/// they agree (the common case) — do nothing**: the body runs purely on its own
/// physics, crisp and smooth, with no backward tug, because the comparison is at
/// the same seq so the latency lead cancels. Only a genuine divergence is
/// reconciled, by applying it to the *present* (the error at the acked seq ≈ the
/// error now, over the ~3–6 unacked ticks) and seating velocity to authoritative
/// so the body stops re-diverging. Acked input frames + stale history are pruned.
///
/// Runs in `FixedPostUpdate` after avian writeback. No-op on host/standalone
/// (no `OwnedLocally`, empty buffers). Replaces the old continuous-correction
/// rubber-band.
pub fn reconcile_owned_prediction(
    buffers: Res<InterpBuffers>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut hist: ResMut<PredictedStateLog>,
    mut input_log: ResMut<lunco_core::OwnedInputLog>,
    q_owned: Query<&lunco_core::GlobalEntityId, With<lunco_core::OwnedLocally>>,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
    )>,
) {
    for gid in q_owned.iter() {
        let g = gid.get();
        // Newest snapshot = authoritative state + the highest input seq the host
        // has applied for this rover.
        let Some(sample) = buffers.0.get(&g).and_then(|b| b.back()).copied() else {
            continue;
        };
        let ack = sample.last_input_seq;
        if ack == 0 {
            continue; // host hasn't applied any of our inputs yet
        }
        let Some(vlog) = hist.0.get_mut(&g) else {
            continue;
        };
        if ack <= vlog.last_reconciled {
            continue; // already handled this ack
        }

        let predicted = vlog.ring.iter().find(|s| s.seq == ack).copied();
        vlog.last_reconciled = ack;
        // Prune history strictly older than the ack; keep `ack` itself as the
        // anchor for the next comparison.
        while vlog.ring.front().is_some_and(|s| s.seq < ack) {
            vlog.ring.pop_front();
        }
        if let Some(il) = input_log.0.get_mut(&g) {
            while il.frames.front().is_some_and(|f| f.seq <= ack) {
                il.frames.pop_front();
            }
        }

        let Some(hs) = predicted else {
            continue; // no recorded prediction for that seq — can't compare
        };
        let auth_pos = sample.pos;
        let auth_rot = sample.rot;
        let err_pos = auth_pos - hs.pos;
        let dist = err_pos.length();
        let mut err_rot = auth_rot * hs.rot.inverse();
        if err_rot.w < 0.0 {
            err_rot = -err_rot; // shortest arc
        }
        let angle = err_rot.to_axis_angle().1.abs();

        // COMMON CASE: prediction matched authority → leave the rover alone.
        if dist < RECONCILE_EPS_POS && angle < RECONCILE_EPS_ROT {
            continue;
        }

        // MISPREDICTION: correct the present by the divergence + seat velocity.
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(g)) else {
            continue;
        };
        let Ok((mut tf, pos, lin, ang)) = q.get_mut(e) else {
            continue;
        };
        let (new_pos, new_rot) = if dist > RECONCILE_SNAP_POS {
            (auth_pos, auth_rot) // gross desync — hard-snap fully to authority
        } else {
            // P5: ease the correction in over a few acks (velocity seated fully
            // below) so a rare misprediction slides rather than jumps.
            (
                tf.translation + err_pos * RECONCILE_CORRECT_BLEND,
                (Quat::IDENTITY.slerp(err_rot, RECONCILE_CORRECT_BLEND) * tf.rotation).normalize(),
            )
        };
        tf.translation = new_pos;
        tf.rotation = new_rot;
        if let Some(mut p) = pos {
            p.0 = DVec3::new(new_pos.x as f64, new_pos.y as f64, new_pos.z as f64);
        }
        // Seat velocity to authoritative so the body stops re-diverging next tick.
        let auth_lv = sample.lv;
        let auth_av = sample.av;
        if let Some(mut l) = lin {
            l.0 = DVec3::new(auth_lv.x as f64, auth_lv.y as f64, auth_lv.z as f64);
        }
        if let Some(mut a) = ang {
            a.0 = DVec3::new(auth_av.x as f64, auth_av.y as f64, auth_av.z as f64);
        }
    }
}

/// Server-authoritative state sync: tag the scene's **top-level dynamic /
/// kinematic** physics bodies (the cosim balloons, the cosim target, free
/// cubes, rover chassis) as [`NetReplicate`] so they ride the snapshot channel.
///
/// Runs on BOTH peers and keys off deterministic USD identity — the same prim
/// derives the same `GlobalEntityId` on host and client (`Provenance::Content`),
/// so each peer tags the same set with no coordination. On the host they become
/// snapshot SOURCES (`gather_snapshot`); on the client `force_kinematic_proxies`
/// pins them kinematic and `apply_incoming_snapshots` drives them. Single-player
/// (`Standalone`) tags them too but nothing serializes — harmless.
///
/// Excludes:
/// - **static** colliders (the ground) — never move;
/// - **runtime spawns** (`SkipContentStamp`) — already tagged at spawn time;
/// - **nested** bodies (parent is itself a rigid body, e.g. rover wheels) — a
///   child-local snapshot would fight `apply_incoming_snapshots`' world-space
///   avian `Position` write; only top-level bodies have local≈world. (Full
///   articulated per-body pose replication is a follow-up.)
pub fn tag_networked_physics(
    mut commands: Commands,
    q_candidates: Query<
        (Entity, &RigidBody, Option<&ChildOf>),
        (
            With<lunco_core::GlobalEntityId>,
            Without<lunco_core::NetReplicate>,
            Without<lunco_core::SkipContentStamp>,
        ),
    >,
    q_bodies: Query<(), With<RigidBody>>,
) {
    for (e, rb, parent) in q_candidates.iter() {
        if matches!(*rb, RigidBody::Static) {
            continue;
        }
        if let Some(p) = parent {
            if q_bodies.contains(p.parent()) {
                continue; // nested body (e.g. rover wheel) — skip for now
            }
        }
        commands.entity(e).insert(lunco_core::NetReplicate);
    }
}

/// Move an existing entity to an absolute world-space position.
///
/// Programmatic equivalent of grabbing the entity with the gizmo and
/// dragging it. The handler:
/// 1. Switches the body to `RigidBody::Kinematic` (if it has a
///    `RigidBody`) so Avian treats the new pose as authoritative
///    rather than fighting back via integration.
/// 2. Writes `Transform.translation` for renderer + scene-graph.
/// 3. Writes Avian's `Position` for the joint/contact solver.
/// 4. Sets a one-tick `LinearVelocity` consistent with the move so
///    any joint coupled to a dynamic body propagates the motion.
///
/// Designed for automated tests / MCP tool clients that need to
/// drive the world without a mouse. Single-shot — body type stays
/// Kinematic until another command (or a gizmo drag-end) restores it.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID (the `api_id` from `ListEntities`).
    /// Resolved to a Bevy `Entity` inside the observer via
    /// `ApiEntityRegistry`. Using `u64` rather than `Entity` here is
    /// deliberate — the API's typed-command resolver only forwards
    /// the entity index, dropping the generation, which makes a
    /// `target: Entity` field lookup fail for any entity whose
    /// generation is non-zero.
    pub entity_id: u64,
    /// Target world-space translation.
    pub translation: Vec3,
}

/// Observer for `MoveEntity`.
pub fn on_move_entity_command(
    trigger: On<MoveEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut LinearVelocity>,
    )>,
    q_has_rb: Query<(), With<RigidBody>>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("MOVE_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let Ok((mut tf, pos_opt, lin_vel_opt)) = q.get_mut(target) else {
        warn!("MOVE_ENTITY: entity {:?} (api_id={}) has no Transform", target, cmd.entity_id);
        return;
    };

    let prev = tf.translation;
    tf.translation = cmd.translation;

    // Force the body to Kinematic for the duration of the move so
    // Avian treats the new pose as authoritative. RigidBody is an
    // immutable Avian component (no `&mut` access) — `insert`
    // replaces it.
    if q_has_rb.get(target).is_ok() {
        commands.entity(target).insert(RigidBody::Kinematic);
    }

    if let Some(mut pos) = pos_opt {
        pos.0 = DVec3::new(
            cmd.translation.x as f64,
            cmd.translation.y as f64,
            cmd.translation.z as f64,
        );
    }

    // **Joint-propagation pulse**: set `LinearVelocity` to a one-tick
    // velocity equal to (delta / dt). Avian's joint constraint solver
    // operates on velocities — without this, kinematic teleports
    // don't drag joint-coupled dynamic bodies along. Position is
    // still set above so the body lands exactly where requested;
    // the velocity is purely a signal to the solver.
    //
    // The `JustMovedKinematic` marker (below) tells
    // `clear_kinematic_pulse_velocity` to zero the velocity after
    // exactly one physics tick. Without that follow-up, the body
    // would keep drifting at this velocity each tick.
    let dt = time.delta_secs().max(1.0 / 240.0) as f64;
    let delta = cmd.translation - prev;
    if let Some(mut lin_vel) = lin_vel_opt {
        lin_vel.0 = DVec3::new(
            delta.x as f64 / dt,
            delta.y as f64 / dt,
            delta.z as f64 / dt,
        );
    }
    commands.entity(target).insert(JustMovedKinematic);

    info!(
        "MOVE_ENTITY: {:?} → ({:.3}, {:.3}, {:.3})",
        cmd.entity_id, cmd.translation.x, cmd.translation.y, cmd.translation.z
    );
}

/// Marker inserted on a kinematic body that just received a
/// `MoveEntity` (or analogous teleport) with a one-tick velocity
/// pulse. [`clear_kinematic_pulse_velocity`] zeros that velocity
/// the frame after the pulse so the body doesn't drift.
#[derive(Component)]
pub struct JustMovedKinematic;

/// Zeros the `LinearVelocity` of bodies marked with
/// [`JustMovedKinematic`], **after one physics tick has consumed
/// the velocity** for joint propagation.
///
/// Schedule: `FixedPostUpdate`. Bevy's main schedule order is
/// `RunFixedMainLoop` (FixedUpdate cycle) → `Update`. So when a
/// `MoveEntity` observer fires in Frame N's `Update` and sets
/// LinearVelocity + marker, the velocity must persist through the
/// *next* fixed-tick physics step (Frame N+1 `FixedUpdate`) before
/// being zeroed. Running this in `FixedPostUpdate` (which fires
/// after every `FixedUpdate` step) does exactly that:
///
/// - Frame N `Update`: `MoveEntity` sets velocity + inserts marker.
/// - Frame N+1 `FixedUpdate`: physics runs WITH the velocity;
///   Avian's joint solver sees the kinematic body moving and
///   propagates the motion through joints to coupled dynamic bodies.
/// - Frame N+1 `FixedPostUpdate`: this system runs, zeros velocity,
///   removes marker.
/// - Frame N+2 `FixedUpdate`: physics with velocity = 0; body
///   settled at its new position, no drift.
pub fn clear_kinematic_pulse_velocity(
    mut commands: Commands,
    mut q: Query<(Entity, &mut LinearVelocity), With<JustMovedKinematic>>,
) {
    for (e, mut vel) in q.iter_mut() {
        vel.0 = DVec3::ZERO;
        commands.entity(e).remove::<JustMovedKinematic>();
    }
}

/// Plugin that registers SPAWN_ENTITY / MOVE_ENTITY command observers
/// and the kinematic-pulse cleanup system.
pub struct SpawnCommandPlugin;

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_spawn_entity_command);
        app.add_observer(on_move_entity_command);
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        app.init_resource::<InterpBuffers>();
        app.init_resource::<PredictedStateLog>();
        // Networking: instantiate host-replicated spawns, buffer + interpolate
        // proxies from snapshots, and keep proxies kinematic. All no-op in
        // single-player. Order matters:
        // - `maintain_owned_locally` classifies my possessed rover BEFORE the
        //   interpolate / kinematic-pin systems read the `OwnedLocally` marker;
        // - `ingest_snapshots` before `interpolate_proxies` so the freshest
        //   sample is available the same frame it arrives;
        // - `correct_owned_prediction` AFTER `force_kinematic_proxies` so the
        //   smooth correction it writes to the owned (Dynamic) body isn't
        //   clobbered the same frame.
        app.add_systems(
            Update,
            (
                apply_replicated_spawns,
                maintain_owned_locally,
                ingest_snapshots,
                interpolate_proxies,
                force_kinematic_proxies,
                tag_networked_physics,
            )
                .chain(),
        );
        // Input-replay reconciliation (D2), in LOCKSTEP with physics —
        // `FixedPostUpdate` AFTER avian's writeback. `reconcile_owned_prediction` folds
        // in the authoritative ack (no-op in the common case → no rubber-band),
        // then `record_predicted_state` records this tick's pose keyed by the input
        // seq, so the NEXT ack can be compared apples-to-apples. Order matters:
        // reconcile first (may correct), then record the resulting pose.
        app.add_systems(
            FixedPostUpdate,
            (reconcile_owned_prediction, record_predicted_state)
                .chain()
                .after(PhysicsSystems::Writeback),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            target: bevy::prelude::Entity::PLACEHOLDER,
            entry_id: "test".to_string(),
            position: bevy::math::Vec3::ZERO,
        };
        assert_eq!(cmd.entry_id, "test");
    }
}
