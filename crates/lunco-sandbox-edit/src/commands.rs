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
        // Phase B: a free predicted prop (`PredictedDynamic`, e.g. a ball you
        // bump) is likewise excluded — it runs local physics + state-reconcile.
        (
            With<lunco_core::NetReplicate>,
            Without<lunco_core::OwnedLocally>,
            Without<lunco_core::PredictedDynamic>,
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

/// One buffered transform sample for client-side interpolation, stamped with the
/// host's **generation time** (its `SimTick` × [`SECS_PER_TICK`]), NOT the local
/// receipt time. Tick-stamping is what makes interpolation robust to bursty /
/// render-throttled delivery: when the sending peer's window is unfocused, several
/// 20 Hz snapshots arrive in one frame, but they carry distinct host ticks, so the
/// bracket search below still spaces them correctly instead of collapsing them to
/// one effective sample (which produced the visible proxy "jumps").
#[derive(Clone, Copy)]
struct InterpSample {
    /// Host generation time in seconds (`tick × SECS_PER_TICK`). The
    /// interpolation/extrapolation clock (`render_t`) lives in this same timebase.
    gen_t: f64,
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
const INTERP_DELAY: f64 = 0.18;
// Seconds per host `SimTick` — the shared fixed-step period (every app's
// `Time::<Fixed>` is built from `lunco_core::FIXED_HZ`). Snapshot ticks are
// multiplied by this to place each sample on the interpolation timebase.
use lunco_core::SECS_PER_TICK;
/// Per-frame easing of the playback clock toward its target (`newest_gen −
/// INTERP_DELAY`). The clock advances at real time between snapshots and is gently
/// nudged so it tracks the host's tick stream without stepping. ~0.1 ⇒ smooth
/// correction of small drift; large desyncs snap (see [`CLOCK_SNAP`]).
const CLOCK_EASE: f64 = 0.1;
/// If the playback clock is more than this far from its target (seconds), snap
/// instead of easing — e.g. first sample, a long stall, or a tick discontinuity.
const CLOCK_SNAP: f64 = 1.0;
/// Cap per-body history (seconds of buffer at 20 Hz; only the recent tail is read).
const INTERP_MAX_SAMPLES: usize = 16;
/// When the buffer starves (`render_t` past the newest sample — common at 20 Hz
/// with network jitter), extrapolate along the newest sample's velocity for up to
/// this long instead of freezing the body and snapping to the next snapshot. This
/// is what turns a fast mover's ~0.6 m teleport-stutter into smooth motion. Capped
/// so a body whose updates genuinely stopped doesn't fly off.
const INTERP_MAX_EXTRAPOLATION: f64 = 0.25;
/// Hard cap on how far (metres) extrapolation may move a starved proxy, so a
/// diverging/runaway authoritative body can't be flung across the scene. Set
/// GENEROUS: a real rover at ~30 m/s over the 0.25 s time cap legitimately needs
/// ~7 m, so a tight cap (the old 0.5) clipped normal motion and CAUSED ~1 m
/// snap-jumps. This only backstops a catastrophic body (e.g. the diverging
/// cosim balloon), which is a separate bug.
const INTERP_MAX_EXTRAP_DIST: f64 = 8.0;

/// Client: file each incoming snapshot into its per-entity interpolation buffer,
/// stamped with the host's **generation time** (`tick × SECS_PER_TICK`), NOT local
/// receipt time. Keying on the host tick means a burst of snapshots that arrive in
/// the same frame (sender render-throttled while unfocused) still land at distinct,
/// correctly-spaced times in the buffer — so [`interpolate_proxies`] brackets them
/// smoothly instead of collapsing the burst into one sample (the proxy "jumps").
pub fn ingest_snapshots(
    mut snaps: ResMut<lunco_core::IncomingSnapshots>,
    mut buffers: ResMut<InterpBuffers>,
) {
    if snaps.0.is_empty() {
        return;
    }
    for s in snaps.0.drain(..).collect::<Vec<_>>() {
        let buf = buffers.0.entry(s.gid).or_default();
        buf.push_back(InterpSample {
            gen_t: s.tick as f64 * SECS_PER_TICK,
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
    // REAL (wall-clock) time advances the playback clock BETWEEN snapshots so a
    // paused client still renders smooth motion. The clock itself lives in the
    // host-tick timebase (anchored to the snapshot stream), not wall time directly.
    time: Res<Time<bevy::time::Real>>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    buffers: Res<InterpBuffers>,
    // Predict-own: the possessed rover is locally simulated + smooth-corrected
    // (`reconcile_owned_prediction`), so it must NOT be dragged back to the
    // `INTERP_DELAY`-old interpolated pose here. Phase B: a free predicted prop
    // (`PredictedDynamic`) is likewise locally simulated + state-reconciled, so it
    // is excluded too.
    q_local_sim: Query<(), Or<(With<lunco_core::OwnedLocally>, With<lunco_core::PredictedDynamic>)>>,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut Rotation>,
        // Animation motion hint: the proxy's local avian velocity is zeroed by
        // `force_kinematic_proxies`, so stamp the snapshot's authoritative chassis
        // velocity here for the wheel-spin model to read (see
        // [`lunco_core::ReplicatedChassisMotion`]).
        Option<&mut lunco_core::ReplicatedChassisMotion>,
    )>,
    // Insert the motion hint on first sight of a proxy that lacks it.
    mut commands: Commands,
    // Playback clock in the host-tick timebase: advances at real time and is eased
    // toward `newest_gen − INTERP_DELAY` so it tracks the host's tick stream without
    // stepping. This is what makes interpolation robust to bursty delivery — the
    // render instant is decoupled from when packets happen to arrive.
    mut playback: Local<f64>,
    mut clock_init: Local<bool>,
) {
    // Newest host generation time across ALL bodies drives the shared clock: the
    // busiest body anchors it (a resting body stops emitting, so keying the clock
    // off its own buffer would stall playback).
    let newest_gen = buffers
        .0
        .values()
        .filter_map(|b| b.back())
        .map(|s| s.gen_t)
        .fold(f64::NEG_INFINITY, f64::max);
    if !newest_gen.is_finite() {
        return; // no samples yet
    }
    let target = newest_gen - INTERP_DELAY;
    if !*clock_init || (target - *playback).abs() > CLOCK_SNAP {
        // First run, or a large desync (long stall / tick discontinuity): snap.
        *playback = target;
        *clock_init = true;
    } else {
        *playback += time.delta_secs() as f64;
        *playback += (target - *playback) * CLOCK_EASE;
        // Never render past the freshest sample we hold.
        if *playback > newest_gen {
            *playback = newest_gen;
        }
    }
    let render_t = *playback;
    for (gid, buf) in buffers.0.iter() {
        if buf.is_empty() {
            continue;
        }
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(*gid)) else {
            continue;
        };
        if q_local_sim.contains(e) {
            continue; // locally simulated (owned rover or predicted prop), not interpolated
        }
        let Ok((mut tf, pos, rot, motion)) = q.get_mut(e) else {
            continue;
        };

        // Samples are time-ordered: `a` = latest at/just before render_t,
        // `b` = first after it.
        let mut a: Option<&InterpSample> = None;
        let mut b: Option<&InterpSample> = None;
        for s in buf.iter() {
            if s.gen_t <= render_t {
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
                let span = (b.gen_t - a.gen_t).max(1e-5);
                let alpha = ((render_t - a.gen_t) / span).clamp(0.0, 1.0);
                (
                    a.pos_world.lerp(b.pos_world, alpha),
                    a.rot.slerp(b.rot, alpha as f32),
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
                let dt = (render_t - a.gen_t).clamp(0.0, INTERP_MAX_EXTRAPOLATION);
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

        // Deliver the authoritative chassis velocity for LOCAL wheel animation.
        // This is the "sync the motion, derive the animation" boundary: we stamp
        // the host's replicated velocity onto a read-only hint that the wheel-spin
        // model reads — we do NOT write avian `LinearVelocity`, because a velocity
        // on a kinematic body would make it glide between snapshots (the very drift
        // `force_kinematic_proxies` zeros away). The nearest bracketing sample's
        // velocity is plenty for animation (it changes at the 20 Hz snapshot rate).
        let (lv, av) = match (a, b) {
            (Some(s), _) | (None, Some(s)) => (s.lv.as_dvec3(), s.av.as_dvec3()),
            (None, None) => (DVec3::ZERO, DVec3::ZERO),
        };
        let hint = lunco_core::ReplicatedChassisMotion { lin: lv, ang: av };
        match motion {
            Some(mut m) => *m = hint,
            None => {
                commands.entity(e).insert(hint);
            }
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
    // Prediction membership is **computability**, not ownership (Phase A,
    // `PREDICTION_MEMBERSHIP.md`): predict the owned rover only while THIS peer is
    // actively driving it. A possessed-but-idle rover is dominated by external
    // forces (another rover pushing it, cosim) the client can't reproduce, so it
    // must interpolate as a normal proxy — else it free-runs local physics with no
    // working correction ("pushed without contact").
    tick: Res<lunco_core::SimTick>,
    input_log: Res<lunco_core::OwnedInputLog>,
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
        // Owned AND actively driven within the grace window. Grace gives
        // hysteresis so a brief gap between key taps doesn't flip the body
        // Kinematic↔Dynamic; when it does lapse the body cleanly returns to
        // interpolation (`force_kinematic_proxies` re-pins it).
        let owns = reg.owns(local.0, gid.get());
        let last_active = input_log.0.get(&gid.get()).map_or(0, |l| l.last_active_tick);
        let mine = predicts_locally(owns, last_active, tick.0, PREDICT_GRACE_TICKS);
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

/// How many ticks after the last nonzero local input a vessel stays in the
/// predicted set (`maintain_owned_locally`). ~0.5 s at 60 Hz — long enough to
/// bridge gaps between key taps (hysteresis on the Dynamic↔Kinematic flip),
/// short enough that an idle/parked owned rover promptly falls back to
/// interpolation so an external push renders correctly.
const PREDICT_GRACE_TICKS: u64 = 30;

/// Pure prediction-membership predicate (Phase A): a client predicts a body
/// locally iff it **owns** it AND it **drove it** within the grace window.
/// Extracted so the ownership × input-recency × tick logic is unit-tested without
/// an avian/render build. `last_active=0` = never driven → never predicted.
fn predicts_locally(owns: bool, last_active: u64, now: u64, grace: u64) -> bool {
    owns && last_active != 0 && now.saturating_sub(last_active) <= grace
}
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

/// Client Phase B (`PREDICTION_MEMBERSHIP.md`): designate **runtime-spawned free
/// dynamic props** (a ball / crate you bump with a rover) as
/// [`lunco_core::PredictedDynamic`], so they run local physics + state-reconcile
/// instead of being kinematic-pinned + interpolated — giving a crisp local
/// rover↔prop collision in the same frame.
///
/// Restricting to [`lunco_core::SkipContentStamp`] (runtime spawns, D4
/// `Authoritative`) is the **cosim guard**: balloons / `CosimTarget` are scene
/// CONTENT (never `SkipContentStamp`), so they can't be caught here — and they
/// must NOT be predicted, their motion is server-only (Gap C). Rovers
/// (`RoverVessel`) and the possessed rover (`OwnedLocally`) are excluded too — they
/// have their own paths. Flips the body to `Dynamic` (a freshly-spawned prop may
/// already have been pinned `Kinematic` by `force_kinematic_proxies` the prior
/// frame); a `Static` prop is left alone. Client-only.
pub fn maintain_predicted_dynamic(
    role: Res<lunco_core::NetworkRole>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            With<lunco_core::SkipContentStamp>,
            Without<lunco_core::RoverVessel>,
            Without<lunco_core::OwnedLocally>,
            Without<lunco_core::PredictedDynamic>,
            // §6 opaque guard: a cosim-driven body (server-only forces) is never
            // locally computable, so it must never be predicted even if it somehow
            // arrived as a runtime spawn. Belt-and-suspenders with the structural
            // `SkipContentStamp` guard above (cosim props are scene content).
            Without<lunco_core::NotPredictable>,
        ),
    >,
    // If this peer later POSSESSES the prop, the owned (input-replay) path takes
    // over — drop the free-body marker so the two reconcilers don't both act on it.
    q_demote: Query<
        Entity,
        (
            With<lunco_core::PredictedDynamic>,
            With<lunco_core::OwnedLocally>,
        ),
    >,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, rb) in q_add.iter() {
        if matches!(*rb, RigidBody::Static) {
            continue; // a static prop has no dynamics worth predicting
        }
        commands
            .entity(e)
            .insert((lunco_core::PredictedDynamic, RigidBody::Dynamic));
    }
    for e in q_demote.iter() {
        commands.entity(e).remove::<lunco_core::PredictedDynamic>();
    }
}

/// Client Phase B: **state-based** reconciliation for [`lunco_core::PredictedDynamic`]
/// free props. Unlike the owned rover there is NO input `seq` to replay, so we
/// compare the body's CURRENT pose directly against the latest authoritative
/// snapshot — `predicted == current` in [`lunco_core::reconcile_decision`], which
/// reduces the decision to "how far is the body from authority right now":
/// `InSync` → leave the local physics alone (crisp contact); a small divergence
/// eases in; a gross one snaps; velocity is seated to authoritative so it stops
/// re-diverging.
///
/// Fires at most ONCE per new snapshot (tracked by host gen-time per gid), NOT
/// every frame: between snapshots the prop runs free local physics. Nudging it
/// every frame would collapse it onto the authoritative pose and destroy the very
/// local-prediction crispness this exists to provide. `FixedPostUpdate` after
/// avian writeback; no-op on host/standalone (no `PredictedDynamic`).
pub fn reconcile_predicted_dynamic(
    buffers: Res<InterpBuffers>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_pred: Query<&lunco_core::GlobalEntityId, With<lunco_core::PredictedDynamic>>,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut Rotation>,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
    )>,
    // Last host gen-time reconciled per gid, so we act once per fresh snapshot.
    mut last_handled: Local<HashMap<u64, f64>>,
) {
    for gid in q_pred.iter() {
        let g = gid.get();
        let Some(sample) = buffers.0.get(&g).and_then(|b| b.back()).copied() else {
            continue;
        };
        if last_handled.get(&g).is_some_and(|&t| t >= sample.gen_t) {
            continue; // no fresh snapshot since last reconcile — let local physics run
        }
        last_handled.insert(g, sample.gen_t);
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(g)) else {
            continue;
        };
        let Ok((mut tf, pos, rot, lin, ang)) = q.get_mut(e) else {
            continue;
        };
        let (new_pos, new_rot) = match lunco_core::reconcile_decision(
            tf.translation,
            tf.rotation,
            tf.translation,
            tf.rotation,
            sample.pos,
            sample.rot,
            lunco_core::ReconcileParams::default(),
        ) {
            lunco_core::Reconciliation::InSync => continue,
            lunco_core::Reconciliation::Correct { pos, rot } => (pos, rot),
            lunco_core::Reconciliation::Snap { pos, rot } => (pos, rot),
        };
        tf.translation = new_pos;
        tf.rotation = new_rot;
        if let Some(mut p) = pos {
            p.0 = DVec3::new(new_pos.x as f64, new_pos.y as f64, new_pos.z as f64);
        }
        // Write avian's f64 `Rotation` too, else writeback re-derives Transform from
        // the stale `Rotation` next tick and clobbers the correction.
        if let Some(mut r) = rot {
            r.0 = new_rot.as_dquat();
        }
        // Seat velocity to authoritative so the prop stops re-diverging next tick.
        if let Some(mut l) = lin {
            l.0 = DVec3::new(sample.lv.x as f64, sample.lv.y as f64, sample.lv.z as f64);
        }
        if let Some(mut a) = ang {
            a.0 = DVec3::new(sample.av.x as f64, sample.av.y as f64, sample.av.z as f64);
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
    /// API-stable global entity ID (the `api_id` from `ListEntities`),
    /// resolved to a Bevy `Entity` in the observer via `ApiEntityRegistry`.
    ///
    /// Deliberately `u64`, not `Entity` — this is "**Pattern B**". The
    /// type-driven id codec (`crates/lunco-networking/PH2_ID_CODEC.md`)
    /// auto-converts only `Entity`-typed fields, so a `u64` field opts out and
    /// is resolved here instead. NOT migrated to `Entity` because this command
    /// is `#[Command(default)]`, which derives `Default`, and `Entity` has no
    /// `Default`. Leaving it `u64` is a cleanliness leftover, not a
    /// names/correctness issue — the codec no longer keys off field names at
    /// all, so this `u64` is simply ignored by it. (An earlier comment here
    /// blamed the resolver "dropping the generation"; that was stale — the
    /// codec preserves index+generation via `Entity::to_bits()`.)
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

// ─────────────────────────────────────────────────────────────────────
// SetObjectProperty — ONE general verb to set any property on an object
// ─────────────────────────────────────────────────────────────────────

/// Set a property on a scene object at runtime (live override — not persisted
/// to USD). One general command instead of many narrow ones; new properties
/// just add a `match` arm. Drive it from curl after a screenshot to iterate:
///
/// ```jsonc
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/spin_reveal.wgsl"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"param0","value":"12"}}   // wedge count
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"colorA","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → load that `.wgsl` (asset path) and bind it via `UsdShaderMaterial`.
/// - `param0`..`param7`, `colorA`/`color`/`colorB`/`colorC` → update the object's
///   shader uniforms in place (requires `shader` to have been set first, or a
///   USD `usd_shader` material).
/// - `visible` → `true`/`false` toggles `Visibility`.
/// - Per-wheel tire-spin dynamics (target a single wheel entity by its `api_id`):
///   `drive_torque`, `brake_torque`, `slip_stiffness`, `bearing_damping`,
///   `friction_mu`, `mass`, `moi`, `wheel_radius`, `rest_length`, `spring_k`,
///   `damping_c` → set that `f64` field on the wheel's `WheelRaycast` live.
///   Each wheel is its own entity, so this gives independent per-wheel control.
#[Command(default)]
pub struct SetObjectProperty {
    /// API-stable global entity ID (the `api_id` from `ListEntities`), same
    /// resolution path as [`MoveEntity`] — `u64` "Pattern B", resolved in the
    /// observer; see [`MoveEntity`]'s `entity_id` for why it stays `u64`.
    pub entity_id: u64,
    /// Property name (see struct docs).
    pub property: String,
    /// Value; comma-separated `r,g,b` for colors, a single float for params,
    /// an asset path for `shader`, `true`/`false` for `visible`.
    pub value: String,
}

/// Maps a `SetObjectProperty` property name to a setter on `WheelRaycast`, or
/// `None` if the name isn't a wheel field. Non-capturing closures coerce to
/// `fn` pointers, so this stays a cheap lookup table. Accepts both the Rust
/// field names and the USD-style aliases (`radius`, `spring_stiffness`, …).
fn wheel_param_setter(name: &str) -> Option<fn(&mut lunco_mobility::WheelRaycast, f64)> {
    use lunco_mobility::WheelRaycast as W;
    Some(match name {
        "drive_torque" | "drive_torque_max" => |w: &mut W, v| w.drive_torque_max = v,
        "brake_torque" | "brake_torque_max" => |w: &mut W, v| w.brake_torque_max = v,
        "slip_stiffness" => |w: &mut W, v| w.slip_stiffness = v,
        "bearing_damping" | "damping_rate" => |w: &mut W, v| w.bearing_damping = v,
        "friction_mu" | "friction" => |w: &mut W, v| w.friction_mu = v,
        "mass" => |w: &mut W, v| w.mass = v,
        "moi" | "moment_of_inertia" => |w: &mut W, v| w.moment_of_inertia = v,
        "wheel_radius" | "radius" => |w: &mut W, v| w.wheel_radius = v,
        "rest_length" => |w: &mut W, v| w.rest_length = v,
        "spring_k" | "spring_stiffness" => |w: &mut W, v| w.spring_k = v,
        "damping_c" | "spring_damping" => |w: &mut W, v| w.damping_c = v,
        _ => return None,
    })
}

/// Observer for [`SetObjectProperty`].
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    q_mat: Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    mut q_vis: Query<&mut Visibility>,
    mut q_wheel: Query<&mut lunco_mobility::WheelRaycast>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SET_PROPERTY: no api_id={} in registry", cmd.entity_id);
        return;
    };

    // Per-wheel tire-spin dynamics. Each wheel is its own entity, so addressing
    // a single `api_id` sets the field on just that wheel — independent control.
    if let Some(setter) = wheel_param_setter(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!("SET_PROPERTY: '{}' expects a number, got '{}'", cmd.property, cmd.value);
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        setter(&mut wheel, value);
        info!("SET_PROPERTY: wheel {} {} = {}", cmd.entity_id, cmd.property, value);
        return;
    }

    match cmd.property.as_str() {
        "shader" => {
            // Preserve existing uniforms if the object already has a
            // ShaderMaterial, so swapping the .wgsl keeps tuned params.
            let template = q_mat
                .get(target)
                .ok()
                .and_then(|m| materials.get(&m.0))
                .cloned()
                .unwrap_or_default();
            let shader = asset_server.load(&cmd.value);
            let handle = materials.add(lunco_materials::build_shader_material(shader, template));
            commands
                .entity(target)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(handle));
            info!("SET_PROPERTY: {} shader = {}", cmd.entity_id, cmd.value);
        }
        "visible" => {
            let Ok(mut vis) = q_vis.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no Visibility", cmd.entity_id);
                return;
            };
            let v = cmd.value.trim();
            *vis = if matches!(v, "false" | "0" | "hidden") {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
        key => {
            // param/color → mutate the live shader material's uniforms in place.
            let Ok(m) = q_mat.get(target) else {
                warn!("SET_PROPERTY: entity {} has no usd_shader material — set 'shader' first", cmd.entity_id);
                return;
            };
            let Some(mat) = materials.get_mut(&m.0) else { return };
            if !lunco_materials::apply_param(mat, key, &cmd.value) {
                warn!("SET_PROPERTY: unknown property '{}'", key);
            }
        }
    }
}

/// Point the free-flight avatar camera at an entity (by API id), from a fixed
/// side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
/// automated screenshots) frame a subject — e.g. a wheel — without hand-driving
/// the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
/// [`MoveEntity`]/[`SetObjectProperty`].
#[Command(default)]
pub struct FocusEntityById {
    /// API id from `ListEntities` — `u64` "Pattern B", resolved in the observer
    /// via `ApiEntityRegistry`; see [`MoveEntity`]'s `entity_id` for why it
    /// stays `u64` and isn't auto-converted by the id codec.
    pub entity_id: u64,
    /// Camera distance from the target, metres. `<= 0` → default 6.
    pub distance: f32,
}

/// Observer that aims the avatar at the requested entity.
pub fn on_focus_entity_by_id(
    trigger: On<FocusEntityById>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_target: Query<&GlobalTransform>,
    mut q_avatar: Query<(&mut Transform, &mut lunco_avatar::FreeFlightCamera), With<lunco_core::Avatar>>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("FOCUS_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let Ok(target_gt) = q_target.get(target) else {
        warn!("FOCUS_ENTITY: target {:?} has no GlobalTransform", target);
        return;
    };
    let Ok((mut tf, mut ff)) = q_avatar.single_mut() else {
        warn!("FOCUS_ENTITY: no Avatar with FreeFlightCamera in the scene");
        return;
    };
    // In big_space, `GlobalTransform` is expressed relative to the floating
    // origin — which is the avatar — so this IS the avatar→target vector in
    // render space (grid-aligned, unit scale).
    // `GlobalTransform.translation()` is the target's absolute world position in
    // this sandbox (the avatar's CellCoord is 0, so local == world).
    let target_pos = target_gt.translation();
    let dist = if cmd.distance > 0.1 { cmd.distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see the
    // spoke face) plus a little up and forward.
    let offset = Vec3::new(1.0, 0.4, 0.25).normalize() * dist;
    // Set the avatar to target + offset (absolute, like MoveEntity).
    tf.translation = target_pos + offset;
    // Camera forward = (camera → target) = -offset. The freeflight system rebuilds
    // rotation from yaw/pitch every frame (YXZ euler), so we must set those.
    let d = (-offset).normalize();
    ff.yaw = (-d.x).atan2(-d.z);
    ff.pitch = d.y.clamp(-1.0, 1.0).asin();
    info!("FOCUS_ENTITY: framed api_id={} at {:.1} m", cmd.entity_id, dist);
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance. Sets the
/// `FreeFlightCamera` yaw/pitch (the camera system rebuilds rotation from those
/// each frame), so the aim sticks.
#[Command(default)]
pub struct SetCameraLookAt {
    pub eye: Vec3,
    pub target: Vec3,
}

/// Observer for [`SetCameraLookAt`].
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_avatar: Query<(&mut Transform, &mut lunco_avatar::FreeFlightCamera), With<lunco_core::Avatar>>,
) {
    let cmd = trigger.event();
    let Ok((mut tf, mut ff)) = q_avatar.single_mut() else {
        warn!("SET_CAMERA: no Avatar with FreeFlightCamera in the scene");
        return;
    };
    tf.translation = cmd.eye;
    let look = cmd.target - cmd.eye;
    if look.length() > 1e-4 {
        let d = look.normalize();
        ff.yaw = (-d.x).atan2(-d.z);
        ff.pitch = d.y.clamp(-1.0, 1.0).asin();
    }
    info!(
        "SET_CAMERA: eye=({:.2},{:.2},{:.2}) target=({:.2},{:.2},{:.2})",
        cmd.eye.x, cmd.eye.y, cmd.eye.z, cmd.target.x, cmd.target.y, cmd.target.z
    );
}

/// Force-reload shader assets from disk so live WGSL edits apply without
/// restarting the app. Bypasses the file watcher (unreliable in this build):
/// calls [`AssetServer::reload`], which re-runs the loader and triggers
/// dependent material pipelines to rebuild. Empty `path` → reload the standard
/// `assets/shaders/*` set; otherwise reload just that path (e.g.
/// `"shaders/wheel.wgsl"`).
#[Command(default)]
pub struct ReloadShader {
    pub path: String,
}

/// Observer for [`ReloadShader`].
pub fn on_reload_shader(trigger: On<ReloadShader>, asset_server: Res<AssetServer>) {
    let p = trigger.event().path.trim().to_string();
    let paths: Vec<String> = if p.is_empty() {
        ["shaders/wheel.wgsl", "shaders/balloon.wgsl", "shaders/solar_panel.wgsl"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![p]
    };
    for path in paths {
        // Owned `String` → `AssetPath<'static>`, so the queued reload doesn't
        // borrow the (short-lived) trigger.
        asset_server.reload(path.clone());
        info!("RELOAD_SHADER: {}", path);
    }
}

/// Replace a shader asset's WGSL **source in place** from text sent over the
/// API, recompiling it live without touching disk or restarting. Overwrites the
/// `Shader` asset currently at `path` (e.g. `"shaders/wheel.wgsl"`), so every
/// material using it re-specializes its pipeline next frame. Compile/validation
/// outcome surfaces in the render log (naga errors on a bad shader). Pairs with
/// [`ReloadShader`] (disk) — this one is for pushing edits directly.
#[Command(default)]
pub struct SetShaderSource {
    /// Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`.
    pub path: String,
    /// New WGSL source text.
    pub source: String,
}

/// Observer for [`SetShaderSource`].
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
) {
    let ev = trigger.event();
    if ev.path.is_empty() || ev.source.is_empty() {
        warn!("SET_SHADER_SOURCE: empty path or source");
        return;
    }
    // `load` returns the handle the materials are already using (the asset is
    // loaded), so overwriting that asset id propagates to them.
    let handle = asset_server.load::<bevy::shader::Shader>(ev.path.clone());
    let shader = bevy::shader::Shader::from_wgsl(ev.source.clone(), ev.path.clone());
    shaders.insert(handle.id(), shader);
    info!(
        "SET_SHADER_SOURCE: recompiled {} from {} bytes of WGSL",
        ev.path,
        ev.source.len()
    );
}

/// Plugin that registers SPAWN_ENTITY / MOVE_ENTITY / SET_OBJECT_PROPERTY /
/// FOCUS_ENTITY_BY_ID / SET_CAMERA_LOOK_AT / RELOAD_SHADER / SET_SHADER_SOURCE
/// command observers and the kinematic-pulse cleanup system.
pub struct SpawnCommandPlugin;

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_spawn_entity_command);
        app.add_observer(on_move_entity_command);
        app.add_observer(on_set_object_property);
        app.add_observer(on_focus_entity_by_id);
        app.add_observer(on_set_camera_look_at);
        app.add_observer(on_reload_shader);
        app.add_observer(on_set_shader_source);
        // Register with AppTypeRegistry so the reflection-based HTTP executor
        // (`get_with_short_type_path`) can construct it from `{"command":"SetObjectProperty",...}`.
        app.register_type::<SetObjectProperty>();
        app.register_type::<FocusEntityById>();
        app.register_type::<SetCameraLookAt>();
        app.register_type::<ReloadShader>();
        app.register_type::<SetShaderSource>();
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
        // Kept in `Update`: the snapshot ingest reads what `drain_wire_inbox`
        // produces, which rides the lightyear ferry (also Update). Smoothness under
        // a render-throttled sender does NOT come from rescheduling these — it comes
        // from `gather_snapshot` generating tick-stamped snapshots at a steady 20 Hz
        // in `FixedUpdate` and `interpolate_proxies` playing them back in tick-space.
        app.add_systems(
            Update,
            (
                apply_replicated_spawns,
                maintain_owned_locally,
                // Phase B: classify free predicted props BEFORE the interpolate /
                // kinematic-pin systems read the `PredictedDynamic` marker.
                maintain_predicted_dynamic,
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
        // Phase B: state-based reconcile for free predicted props (no input seq),
        // likewise after avian writeback. Independent of the owned-rover chain
        // above (acts on a disjoint set of bodies).
        app.add_systems(
            FixedPostUpdate,
            reconcile_predicted_dynamic.after(PhysicsSystems::Writeback),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{predicts_locally, PREDICT_GRACE_TICKS};

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

    // Phase A: prediction membership = ownership ∧ recent local input.
    #[test]
    fn not_owned_never_predicts() {
        // Even with fresh input, a body this peer does not own is never predicted.
        assert!(!predicts_locally(false, 100, 100, PREDICT_GRACE_TICKS));
    }

    #[test]
    fn owned_but_never_driven_interpolates() {
        // The bug case: possessed (owned) but `last_active=0` (never driven by us,
        // e.g. it's being pushed by another rover) → NOT predicted → interpolated.
        assert!(!predicts_locally(true, 0, 1_000, PREDICT_GRACE_TICKS));
    }

    #[test]
    fn owned_and_actively_driving_predicts() {
        // Driven this very tick, and anywhere inside the grace window.
        assert!(predicts_locally(true, 1_000, 1_000, PREDICT_GRACE_TICKS));
        assert!(predicts_locally(true, 1_000, 1_000 + PREDICT_GRACE_TICKS, PREDICT_GRACE_TICKS));
    }

    #[test]
    fn owned_idle_past_grace_falls_back_to_interpolation() {
        // One tick past the grace window → demote to proxy/interpolation.
        assert!(!predicts_locally(true, 1_000, 1_001 + PREDICT_GRACE_TICKS, PREDICT_GRACE_TICKS));
    }

    #[test]
    fn tick_reset_does_not_falsely_predict() {
        // `saturating_sub` guards a client SimTick that jumped backwards (clock
        // discontinuity): now < last_active must not underflow into a huge value
        // that reads as "recent". It clamps to 0 → treated as just-driven, which
        // is the safe/benign direction (predict, then the next real input resets).
        assert!(predicts_locally(true, 1_000, 5, PREDICT_GRACE_TICKS));
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
                gen_t: 0.0,
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
                gen_t: 0.0,
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

    /// The bursty-delivery fix: a batch of snapshots that all arrive in the SAME
    /// frame (sender render-throttled while unfocused) must still interpolate
    /// smoothly, because each carries its host `SimTick` and is keyed in tick-space
    /// — not the local receipt time (which would be identical for the whole burst
    /// and collapse it to one effective sample → the visible proxy "jump").
    ///
    /// We push 7 samples in one `ingest_snapshots` call (one frame), positioned
    /// linearly along the host-tick timebase, then run `interpolate_proxies` once
    /// and assert the rendered pose is a true mid-bracket lerp, not a snap to an
    /// endpoint.
    #[test]
    fn bursty_snapshots_interpolate_in_tick_space() {
        use lunco_core::{IncomingSnapshots, SnapshotSample};

        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.init_resource::<IncomingSnapshots>();
        world.insert_resource(Time::<bevy::time::Real>::default());

        let gid = 0x00AB_0003u64;
        let e = world
            .spawn((
                Transform::default(),
                Position::default(),
                Rotation::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::NetReplicate,
            ))
            .id();
        registry_with(&mut world, e, gid);

        // 7 snapshots at 20 Hz (3 host ticks apart at 60 Hz), spanning gen_t
        // 0.20‥0.50 s, with absolute X moving linearly at 100 m per second of
        // tick-time (so X == gen_t × 100). ALL queued before a single ingest →
        // they arrive as one burst at identical local receipt time.
        let identity_r = [0.0, 0.0, 0.0, 1.0];
        for k in 0..7u64 {
            let tick = 12 + k * 3; // 12,15,…,30  ⇒ gen_t 0.20,0.25,…,0.50
            let gen_t = tick as f64 * SECS_PER_TICK;
            let x = (gen_t * 100.0) as f32;
            world.resource_mut::<IncomingSnapshots>().0.push(SnapshotSample {
                gid,
                tick,
                t: [x, 0.0, 0.0],
                r: identity_r,
                lv: [100.0, 0.0, 0.0], // unused here (we bracket, never extrapolate)
                av: [0.0; 3],
                last_input_seq: 0,
                pos: [gen_t * 100.0, 0.0, 0.0],
                cell: [0; 3],
            });
        }

        // One frame: the whole burst lands in the buffer at once.
        world.run_system_once(ingest_snapshots).unwrap();
        assert_eq!(
            world.resource::<InterpBuffers>().0.get(&gid).map(|b| b.len()),
            Some(7),
            "all 7 burst samples must be distinct buffer entries"
        );

        world.run_system_once(interpolate_proxies).unwrap();

        // newest_gen = 0.50; render_t = 0.50 − INTERP_DELAY(0.18) = 0.32, which
        // brackets the samples at gen_t 0.30 (x=30) and 0.35 (x=35):
        //   alpha = (0.32 − 0.30)/0.05 = 0.4  ⇒  x = 30 + 0.4·5 = 32.
        // A receipt-time-keyed buffer would have collapsed the burst and snapped to
        // an endpoint (x=20 or x=50) instead.
        let x = world.entity(e).get::<Transform>().unwrap().translation.x;
        assert!(
            (x - 32.0).abs() < 0.1,
            "expected mid-bracket lerp x≈32 (proof of tick-space interpolation), got {x}"
        );
    }

    /// Phase B: a `PredictedDynamic` prop that has grossly diverged from authority
    /// is hard-snapped to the authoritative pose AND has its velocity seated, so it
    /// stops re-diverging.
    #[test]
    fn predicted_dynamic_snaps_far_body_and_seats_velocity() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();

        let gid = 0x00BB_0001u64;
        let e = world
            .spawn((
                Transform::default(), // at origin
                Position::default(),
                Rotation::default(),
                LinearVelocity::default(),
                AngularVelocity::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::PredictedDynamic,
            ))
            .id();
        registry_with(&mut world, e, gid);

        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                gen_t: 0.5,
                pos: Vec3::new(50.0, 0.0, 0.0), // ≫ snap_pos (6.0) from origin
                rot: Quat::IDENTITY,
                pos_world: DVec3::new(50.0, 0.0, 0.0),
                lv: Vec3::new(2.0, 0.0, 0.0),
                av: Vec3::ZERO,
                last_input_seq: 0,
            });

        world.run_system_once(reconcile_predicted_dynamic).unwrap();

        let p = world.entity(e).get::<Transform>().unwrap().translation;
        let v = world.entity(e).get::<LinearVelocity>().unwrap().0;
        assert!((p.x - 50.0).abs() < 1e-4, "should snap to authority, got {p:?}");
        assert!((v.x - 2.0).abs() < 1e-4, "velocity must be seated to authority, got {v:?}");
    }

    /// Phase B: when a `PredictedDynamic` prop is already at authority (InSync), the
    /// reconcile leaves it COMPLETELY alone — no pose change and, crucially, NO
    /// velocity seating — so its local physics keeps running crisply between
    /// snapshots instead of being clamped to the authoritative velocity each frame.
    #[test]
    fn predicted_dynamic_in_sync_is_left_untouched() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();

        let gid = 0x00BB_0002u64;
        let local_vel = DVec3::new(5.0, 0.0, 0.0); // the prop's own local velocity
        let e = world
            .spawn((
                Transform::default(), // at origin
                Position::default(),
                Rotation::default(),
                LinearVelocity(local_vel),
                AngularVelocity::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::PredictedDynamic,
            ))
            .id();
        registry_with(&mut world, e, gid);

        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                gen_t: 0.5,
                pos: Vec3::new(0.05, 0.0, 0.0), // within eps_pos (0.25) of the body
                rot: Quat::IDENTITY,
                pos_world: DVec3::new(0.05, 0.0, 0.0),
                lv: Vec3::ZERO, // authority says 0 — must NOT overwrite local 5.0
                av: Vec3::ZERO,
                last_input_seq: 0,
            });

        world.run_system_once(reconcile_predicted_dynamic).unwrap();

        let v = world.entity(e).get::<LinearVelocity>().unwrap().0;
        assert!(
            (v.x - 5.0).abs() < 1e-9,
            "InSync must NOT seat velocity — local physics keeps running; got {v:?}"
        );
    }
}

/// **Throwaway probe — PREDICT_AND_SMOOTH_TODO Step 1.1.** Confirms avian 0.6.1
/// kinematic semantics before we commit to velocity-driven remote proxies
/// (`drive_kinematic_proxies`). Two questions the whole Step-1 design rides on:
///
///  * **P1** — a `Kinematic` body with `LinearVelocity = v` advances `Position`
///    by exactly `v·h` per fixed tick. If true we can steer a proxy purely by
///    *setting velocity* each tick (`v = (target − pos)/h`) instead of teleporting
///    its Transform, so its motion is a real velocity the solver knows about.
///  * **P2** — that velocity **enters contact resolution**: a kinematic body
///    moving into a `Dynamic` body *pushes* it. This is the payoff — a remote
///    rover proxy driven by velocity will shove a locally-predicted prop (ball /
///    crate) crisply in the same frame, instead of interpenetrating and being
///    resolved by overlap-pushout alone (the source of the contact buzz).
///
/// Runs the real solver headlessly (no window) with `SubstepCount(12)` to match
/// the app. Deterministic stepping via [`TimeUpdateStrategy::ManualDuration`] so
/// each `app.update()` is exactly one fixed tick. **Delete this whole module once
/// Step 1 lands** — it asserts platform behavior, not our code.
#[cfg(test)]
mod avian_kinematic_probe {
    use super::*;
    use avian3d::prelude::{Collider, Gravity, PhysicsPlugins, SubstepCount};
    use bevy::asset::AssetApp;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    const HZ: f64 = 64.0;
    const H: f64 = 1.0 / HZ;

    fn headless_physics_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::transform::TransformPlugin)
            // avian's collider cache touches Mesh assets + `AssetEvent<Mesh>`;
            // register them so its systems' message readers validate headless.
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<bevy::mesh::Mesh>()
            // avian's `bevy_diagnostic`/`debug-plugin` features insert their
            // diagnostics resources (e.g. `ColliderTreeDiagnostics`) only when
            // bevy's `DiagnosticsPlugin` is present.
            .add_plugins(bevy::diagnostic::DiagnosticsPlugin)
            .add_plugins(PhysicsPlugins::default())
            .insert_resource(SubstepCount(12))
            // No gravity — isolate kinematic integration / contact push from fall.
            .insert_resource(Gravity(avian3d::math::Vector::ZERO))
            // Fixed step == H, and advance virtual time by exactly H per update:
            // one — and only one — physics tick per `app.update()`, no wall clock.
            .insert_resource(Time::<Fixed>::from_hz(HZ))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(H)));
        // `app.run()` calls these; bare `app.update()` does not. avian inserts its
        // diagnostics resources (`ColliderTreeDiagnostics`, …) in `finish`.
        app.finish();
        app.cleanup();
        app
    }

    fn step(app: &mut App, ticks: usize) {
        for _ in 0..ticks {
            app.update();
        }
    }

    /// P1: a kinematic body advances `Position` by exactly `v·h` **per fixed
    /// tick**. Measured as the steady-state delta across a span of K ticks (after
    /// a few warmup ticks) — this isolates the per-tick integration rate and
    /// sidesteps the one-tick spawn/prepare lag (the first `update()` syncs
    /// Transform→Position without integrating, so absolute Position == v·h·(N−1)).
    /// The per-tick *rate* is the invariant `drive_kinematic_proxies` relies on.
    #[test]
    fn kinematic_advances_position_by_v_times_h() {
        let mut app = headless_physics_app();
        let v = avian3d::math::Vector::new(2.0, 0.0, 0.0); // 2 m/s +x
        let e = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Position::default(),
                Rotation::default(),
                LinearVelocity(v),
                AngularVelocity::default(),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();

        step(&mut app, 4); // warmup past the spawn/prepare tick
        let p0 = app.world().entity(e).get::<Position>().unwrap().0;
        let k = 10;
        step(&mut app, k);
        let p1 = app.world().entity(e).get::<Position>().unwrap().0;

        let expected = v * (H * k as f64);
        // Tolerance ~1e-6 m: `SubstepCount(12)` splits h into integer-nanosecond
        // substeps (15625000/12 truncates), losing ~4 ns/tick → ~8 nm/tick of
        // integrated time. So the advance is v·h modulo that substep rounding —
        // exact for our purposes (nanometres over a 60 Hz tick).
        assert!(
            ((p1 - p0) - expected).length() < 1e-6,
            "P1: kinematic should advance v·h per tick; {k}-tick delta expected \
             {expected:?}, got {:?}",
            p1 - p0
        );
    }

    /// P2: a kinematic pusher moving +x, starting *clear* of a dynamic target,
    /// drives that target +x once it makes contact (and the target gains +x
    /// velocity). Starting separated rules out penetration-recovery as the cause.
    #[test]
    fn kinematic_velocity_pushes_dynamic_body() {
        let mut app = headless_physics_app();
        // Two unit spheres (r=0.5 → contact at centre-distance 1.0). Pusher at x=0
        // moving +x at 3 m/s; target at x=1.2 (0.2 m clear gap). Over 30 ticks the
        // pusher travels ~1.4 m, so it reaches and shoves the target.
        let _pusher = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Collider::sphere(0.5),
                Position(avian3d::math::Vector::new(0.0, 0.0, 0.0)),
                Rotation::default(),
                LinearVelocity(avian3d::math::Vector::new(3.0, 0.0, 0.0)),
                AngularVelocity::default(),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let target = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::sphere(0.5),
                Position(avian3d::math::Vector::new(1.2, 0.0, 0.0)),
                Rotation::default(),
                LinearVelocity::default(),
                AngularVelocity::default(),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();

        let x0 = app.world().entity(target).get::<Position>().unwrap().0.x;
        step(&mut app, 30);
        let pos = app.world().entity(target).get::<Position>().unwrap().0;
        let vel = app.world().entity(target).get::<LinearVelocity>().unwrap().0;

        assert!(
            pos.x > x0 + 0.1,
            "P2: kinematic pusher should drive the dynamic target +x via contact; \
             x0={x0}, now={}",
            pos.x
        );
        assert!(
            vel.x > 0.0,
            "P2: dynamic target should gain +x velocity from the kinematic contact; got {vel:?}"
        );
    }
}
