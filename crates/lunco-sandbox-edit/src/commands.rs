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
use avian3d::physics_transform::{Position, Rotation};
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
    /// f32 render-space pose (cell-relative). Used by `reconcile_owned_prediction`
    /// to compare against the f32 predicted-Transform history (apples-to-apples).
    pos: Vec3,
    rot: Quat,
    /// Authoritative **absolute** position (avian f64 `Position`, gap A) — the
    /// remote-proxy interpolation seats `Position` from this so lunar/orbital-scale
    /// bodies keep f64 precision instead of collapsing to the f32 `pos`.
    pos_world: DVec3,
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
/// instant to interpolate between (≈3–4 snapshots at the 20 Hz default). Higher
/// = smoother under jitter (less buffer starvation → less reliance on
/// extrapolation) but more visible lag on the bodies you watch. 0.18 keeps a
/// fast-moving proxy reliably bracketed by two real samples so it lerps instead
/// of extrapolate-then-snapping (~1 m jumps under the old 0.12).
const INTERP_DELAY: f32 = 0.18;
/// Cap per-body history (seconds of buffer at 20 Hz; only the recent tail is read).
const INTERP_MAX_SAMPLES: usize = 16;
/// When the buffer starves (`render_t` past the newest sample — common at 20 Hz
/// with network jitter), extrapolate along the newest sample's velocity for up to
/// this long instead of freezing the body and snapping to the next snapshot. This
/// is what turns a fast mover's ~0.6 m teleport-stutter into smooth motion. Capped
/// so a body whose updates genuinely stopped doesn't fly off.
const INTERP_MAX_EXTRAPOLATION: f32 = 0.25;
/// Hard cap on how far (metres) extrapolation may move a starved proxy, so a
/// diverging/runaway authoritative body can't be flung across the scene. Set
/// GENEROUS: a real rover at ~30 m/s over the 0.25 s time cap legitimately needs
/// ~7 m, so a tight cap (the old 0.5) clipped normal motion and CAUSED ~1 m
/// snap-jumps. This only backstops a catastrophic body (e.g. the diverging
/// cosim balloon), which is a separate bug.
const INTERP_MAX_EXTRAP_DIST: f64 = 8.0;

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
            pos_world: DVec3::from_array(s.pos),
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
    mut q: Query<(&mut Transform, Option<&mut Position>, Option<&mut Rotation>)>,
    // NET_DIAG-only: count how often the buffer starves (we extrapolate) vs.
    // brackets two samples (true interpolation), to confirm the starvation cause.
    mut diag_sec: Local<f32>,
    mut diag_bracket: Local<u32>,
    mut diag_extrap: Local<u32>,
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
        let Ok((mut tf, pos, rot)) = q.get_mut(e) else {
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

        // Interpolate the absolute position in f64 (gap A) so far-from-origin
        // bodies don't lose precision; rotation lerps in f32 (orientation is
        // scale-free).
        let (out_world, out_rot) = match (a, b) {
            (Some(a), Some(b)) => {
                *diag_bracket += 1;
                let span = (b.t_recv - a.t_recv).max(1e-5);
                let alpha = ((render_t - a.t_recv) / span).clamp(0.0, 1.0);
                (
                    a.pos_world.lerp(b.pos_world, alpha as f64),
                    a.rot.slerp(b.rot, alpha),
                )
            }
            // render_t before the oldest sample → snap to oldest.
            (None, Some(b)) => (b.pos_world, b.rot),
            // Starved (render_t past the newest sample). Holding the pose here is
            // what made a moving body freeze then snap ~0.6 m when the next
            // snapshot landed. Extrapolate along the sample's velocity instead, so
            // it keeps gliding; a body genuinely at rest has lv≈0 → this is still a
            // hold. Capped at INTERP_MAX_EXTRAPOLATION so a stalled body doesn't
            // drift away.
            (Some(a), None) => {
                *diag_extrap += 1;
                let dt = (render_t - a.t_recv).clamp(0.0, INTERP_MAX_EXTRAPOLATION) as f64;
                let mut delta = a.lv.as_dvec3() * dt;
                // Hard distance clamp: a body whose authoritative physics is
                // diverging (or whose velocity sample is stale) must not be flung
                // metres away by extrapolation. Bound the glide so the worst case
                // is a small offset, not a teleport.
                let len = delta.length();
                if len > INTERP_MAX_EXTRAP_DIST {
                    delta *= INTERP_MAX_EXTRAP_DIST / len;
                }
                (a.pos_world + delta, a.rot)
            }
            (None, None) => continue,
        };

        // Seat the precise f64 physics `Position`; the f32 render `Transform` is
        // its projection (cell-relative — identical to absolute while cells stay
        // 0; once recentering is enabled this must subtract the body's cell origin).
        tf.translation = out_world.as_vec3();
        tf.rotation = out_rot;
        if let Some(mut p) = pos {
            p.0 = out_world;
        }
        // Also write avian's f64 `Rotation` (the physics truth), not just the f32
        // `Transform.rotation`. Without this, avian's writeback re-derives Transform
        // from the un-updated `Rotation` next frame and CLOBBERS the interpolated
        // orientation → the proxy's rotation fights/jitters (very visible on a body
        // that's turning). Position already sticks because we write `Position` above.
        if let Some(mut r) = rot {
            r.0 = out_rot.as_dquat();
        }
    }

    // NET_DIAG: report how often we had to extrapolate (buffer starved) vs.
    // interpolate between two real samples. High starvation % confirms the
    // 20 Hz cadence / INTERP_DELAY is too tight and was causing the snaps.
    *diag_sec += time.delta_secs();
    if *diag_sec >= 1.0 {
        *diag_sec = 0.0;
        let (b, e) = (*diag_bracket, *diag_extrap);
        *diag_bracket = 0;
        *diag_extrap = 0;
        if (b + e) > 0 && std::env::var("NET_DIAG").is_ok() {
            info!(
                "[net-diag interp] bracketed={b} extrapolated/starved={e} ({}% starved)",
                100 * e / (b + e)
            );
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

/// Cap on the predicted-state history ring (~2 s at 60 Hz). Only the recent tail
/// (the unacked window) is ever compared.
const MAX_PREDICTED_HISTORY: usize = 128;
// The reconciliation thresholds (eps_pos / eps_rot / snap_pos / blend) and the
// decision geometry live in `lunco_core::reconcile_decision` /
// `ReconcileParams::default()` — the single source of truth shared by this live
// system and the `reconcile` unit tests (no avian/render build needed to test).

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
        Option<&mut Rotation>,
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

        // Resolve the body so we can read its present pose (the correction is
        // expressed relative to "now") and mutate it.
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(g)) else {
            continue;
        };
        let Ok((mut tf, pos, rot, lin, ang)) = q.get_mut(e) else {
            continue;
        };

        // Compare prediction-at-the-acked-seq vs authority-at-that-seq — the
        // apples-to-apples test that cancels the latency lead, so a correct
        // prediction is left alone (no rubber-band). Only divergence corrects.
        let (new_pos, new_rot) = match lunco_core::reconcile_decision(
            hs.pos,
            hs.rot,
            tf.translation,
            tf.rotation,
            sample.pos,
            sample.rot,
            lunco_core::ReconcileParams::default(),
        ) {
            // COMMON CASE: prediction matched authority → leave the body alone.
            lunco_core::Reconciliation::InSync => continue,
            lunco_core::Reconciliation::Correct { pos, rot } => (pos, rot),
            lunco_core::Reconciliation::Snap { pos, rot } => (pos, rot),
        };
        tf.translation = new_pos;
        tf.rotation = new_rot;
        if let Some(mut p) = pos {
            p.0 = DVec3::new(new_pos.x as f64, new_pos.y as f64, new_pos.z as f64);
        }
        // Write avian's f64 `Rotation` too — otherwise avian's writeback re-derives
        // `Transform.rotation` from the stale f64 `Rotation` next frame and the
        // correction is lost, so the owned rover's orientation fights the physics
        // every frame (the "two systems fighting" jitter when steering).
        if let Some(mut r) = rot {
            r.0 = new_rot.as_dquat();
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

// ── Networking diagnostics (opt-in via `NET_DIAG` env) ─────────────────────────

/// Last post-physics pose of each replicated body, for the jump diagnostic.
#[derive(Resource, Default)]
pub struct NetDiagLastPose(HashMap<u64, (Vec3, Quat)>);

/// A single-frame jump larger than this (metres) is logged as a blow-up.
const NET_DIAG_JUMP_THRESH: f32 = 0.3;

/// DIAGNOSTIC (opt-in: set `NET_DIAG=1`). Pins down the rover-vs-rover collision
/// explosion (symptom #3) and quantifies rest jitter (#1/#2). Runs in
/// `FixedPostUpdate` AFTER avian writeback, so it sees the **raw physics result**
/// for each replicated body *before* `interpolate_proxies` (Update) re-pins the
/// proxies — that's the only place the physics-induced displacement is visible.
///
/// For any `NetReplicate` body that moved more than [`NET_DIAG_JUMP_THRESH`] in a
/// single fixed step it logs: role (host/client), gid, ownership, **RigidBody
/// kind**, and **speed**. That triple is the discriminator:
/// - proxy jump with `rb=Kinematic, |v|≈0`  → an interpolation/teleport conflict
///   (a Kinematic body repositioned into a Dynamic one → explosive contact);
/// - proxy jump with `rb=Kinematic, |v|≫0`  → physics gave a kinematic body
///   velocity (it shouldn't have one);
/// - owned jump with `rb=Dynamic,  |v|≫0`   → the predicted own-rover itself is
///   being launched by the contact solver.
///
/// Also emits a once-per-second per-body rest-jitter line (max single-step delta
/// seen that second) so the sub-centimetre #1/#2 jitter is quantified, not just
/// the metre-scale blow-ups.
pub fn net_diag_pose_jumps(
    role: Res<lunco_core::NetworkRole>,
    time: Res<Time>,
    mut last: ResMut<NetDiagLastPose>,
    mut sec: Local<f32>,
    mut max_step: Local<HashMap<u64, f32>>,
    mut max_rot: Local<HashMap<u64, f32>>,
    q: Query<
        (
            &lunco_core::GlobalEntityId,
            &Transform,
            &RigidBody,
            Option<&LinearVelocity>,
            Has<lunco_core::OwnedLocally>,
        ),
        With<lunco_core::NetReplicate>,
    >,
) {
    for (gid, tf, rb, lin, owned) in q.iter() {
        let g = gid.get();
        let now = tf.translation;
        let now_rot = tf.rotation;
        if let Some((prev, prev_rot)) = last.0.get(&g) {
            let d = (now - *prev).length();
            // Single-step rotation delta (degrees) — catches the "two systems
            // fighting" orientation jitter that a translation-only metric misses.
            let mut dq = now_rot * prev_rot.inverse();
            if dq.w < 0.0 {
                dq = -dq;
            }
            let dded = dq.to_axis_angle().1.abs().to_degrees();
            let m = max_step.entry(g).or_insert(0.0);
            if d > *m {
                *m = d;
            }
            let mr = max_rot.entry(g).or_insert(0.0);
            if dded > *mr {
                *mr = dded;
            }
            if d > NET_DIAG_JUMP_THRESH {
                let v = lin.map(|l| l.0.length()).unwrap_or(0.0);
                warn!(
                    "[net-diag {:?}] gid={:#x} STEP-JUMP {:.3}m owned={} rb={:?} |v|={:.2}",
                    *role, g, d, owned, rb, v
                );
            }
        }
        last.0.insert(g, (now, now_rot));
    }
    *sec += time.delta_secs();
    if *sec >= 1.0 {
        *sec = 0.0;
        for (g, m) in max_step.iter() {
            let mr = max_rot.get(g).copied().unwrap_or(0.0);
            if *m > 1e-4 || mr > 0.05 {
                info!(
                    "[net-diag {:?}] gid={:#x} max single-step Δ this sec = {:.4}m  Δrot = {:.3}°",
                    *role, g, m, mr
                );
            }
        }
        max_step.clear();
        max_rot.clear();
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
        // Kept in `Update` (the snapshot ingest reads what `drain_wire_inbox`
        // produces, which rides the lightyear ferry — also Update). Splitting the
        // ingest/interpolate across schedules is the right end-state but only once
        // the lightyear IO itself ticks in FixedUpdate.
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
        // Opt-in collision/jitter diagnostic (set `NET_DIAG=1`). Samples the raw
        // post-physics pose so it can tell a teleport-into-dynamic blow-up from a
        // contact-solver launch. Ordered after reconcile so its writes are
        // included in the sampled pose.
        if std::env::var("NET_DIAG").is_ok() {
            app.init_resource::<NetDiagLastPose>();
            app.add_systems(
                FixedPostUpdate,
                net_diag_pose_jumps
                    .after(PhysicsSystems::Writeback)
                    .after(record_predicted_state),
            );
        }
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

/// Headless integration tests for the networked-pose write path. They run the
/// real `reconcile_owned_prediction` / `interpolate_proxies` systems against a
/// hand-built `World` (no GPU, no `PhysicsPlugins`) — so they execute at full
/// speed and are immune to the ~1 FPS GUI-thrash that makes on-screen
/// verification on a memory-constrained machine unreliable.
///
/// The invariant under test is the one whose violation produced the "two systems
/// fighting" turning jitter: a corrected/interpolated orientation must land on
/// avian's f64 `Rotation` (the physics truth), not only the f32
/// `Transform.rotation` — otherwise avian's writeback re-derives Transform from
/// the stale `Rotation` next tick and clobbers the correction.
#[cfg(test)]
mod pose_write_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn registry_with(world: &mut World, e: Entity, gid: u64) {
        let mut reg = lunco_api::registry::ApiEntityRegistry::default();
        reg.assign(e, lunco_core::GlobalEntityId::from_raw(gid));
        world.insert_resource(reg);
    }

    /// Reconcile (owned rover) must write avian `Rotation`, not just `Transform`.
    #[test]
    fn reconcile_correction_writes_avian_rotation() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.init_resource::<PredictedStateLog>();
        world.init_resource::<lunco_core::OwnedInputLog>();

        let gid = 0x00AB_CDEFu64;
        let predicted = Quat::IDENTITY; // == Transform::default().rotation
        let authoritative = Quat::from_rotation_y(0.5); // 0.5 rad ≫ eps_rot (0.03)

        let e = world
            .spawn((
                Transform::default(),
                Position::default(),
                Rotation::default(),
                LinearVelocity::default(),
                AngularVelocity::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::OwnedLocally,
                lunco_core::NetReplicate,
            ))
            .id();
        registry_with(&mut world, e, gid);

        // We predicted `predicted` at input seq 1…
        world
            .resource_mut::<PredictedStateLog>()
            .0
            .entry(gid)
            .or_default()
            .ring
            .push_back(PredictedState { seq: 1, pos: Vec3::ZERO, rot: predicted });
        // …and the host acks seq 1 with a divergent authoritative orientation.
        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                t_recv: 0.0,
                pos: Vec3::ZERO,
                rot: authoritative,
                pos_world: DVec3::ZERO,
                lv: Vec3::ZERO,
                av: Vec3::ZERO,
                last_input_seq: 1,
            });

        world.run_system_once(reconcile_owned_prediction).unwrap();

        let tf_rot = world.entity(e).get::<Transform>().unwrap().rotation;
        let avian_rot = world.entity(e).get::<Rotation>().unwrap().0.as_quat();
        // The correction moved orientation off the (identity) prediction…
        assert!(
            tf_rot.angle_between(predicted) > 1e-3,
            "reconcile should have corrected rotation; got {tf_rot:?}"
        );
        // …and avian's f64 Rotation matches Transform (the bug = divergence here).
        assert!(
            tf_rot.angle_between(avian_rot) < 1e-4,
            "avian Rotation {avian_rot:?} must match Transform.rotation {tf_rot:?}"
        );
    }

    /// Proxy interpolation must likewise write avian `Rotation`.
    #[test]
    fn interpolate_proxy_writes_avian_rotation() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.insert_resource(Time::<bevy::time::Real>::default());

        let gid = 0x00AB_0002u64;
        let target = Quat::from_rotation_y(0.8);

        let e = world
            .spawn((
                Transform::default(),
                Position::default(),
                Rotation::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::NetReplicate, // NOT OwnedLocally → treated as a proxy
            ))
            .id();
        registry_with(&mut world, e, gid);

        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                t_recv: 0.0,
                pos: Vec3::ZERO,
                rot: target,
                pos_world: DVec3::ZERO,
                lv: Vec3::ZERO,
                av: Vec3::ZERO,
                last_input_seq: 0,
            });

        world.run_system_once(interpolate_proxies).unwrap();

        let tf_rot = world.entity(e).get::<Transform>().unwrap().rotation;
        let avian_rot = world.entity(e).get::<Rotation>().unwrap().0.as_quat();
        assert!(
            tf_rot.angle_between(target) < 1e-4,
            "proxy Transform should take the sample rotation; got {tf_rot:?}"
        );
        assert!(
            tf_rot.angle_between(avian_rot) < 1e-4,
            "proxy avian Rotation {avian_rot:?} must match Transform.rotation {tf_rot:?}"
        );
    }
}
