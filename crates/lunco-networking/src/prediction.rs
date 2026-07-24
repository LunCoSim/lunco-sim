//! Client-side netcode: snapshot interpolation, prediction, rollback,
//! reconciliation and correction smoothing over avian bodies.
//!
//! Split out of `lunco-sandbox-edit::commands`, which had fused two unrelated
//! subsystems in one file: the scene/document command layer (spawn / move / delete
//! / set-property / focus / shader) and *this* — the client half of the wire. The
//! netcode half never touched an editor symbol; its dependencies are `lunco-core`
//! (the session/identity substrate), `lunco-api`, `big_space` and
//! `avian3d`, all of which this crate already had. It belongs next to the wire that
//! feeds it (`sync.rs` produces the `IncomingSnapshots` this module consumes), not
//! next to the editor.
//!
//! One system stayed behind: `apply_replicated_spawns` (it instantiates from the
//! editor's spawn catalog). It runs FIRST, and the ordering across the new crate
//! boundary is preserved by [`lunco_core::NetcodeSet`] — see [`NetcodePredictionPlugin`].
//!
//! Compiled unconditionally (no `networking` feature gate): every dependency it
//! names is a non-optional dependency of this crate, and all of its systems are
//! self-guarding no-ops on host/standalone.

use avian3d::physics_transform::{Position, Rotation};
use avian3d::prelude::{
    AngularVelocity, Collisions, LinearVelocity, PhysicsSystems, RigidBody, SubstepCount,
};
use avian3d::schedule::{Physics, PhysicsSchedule, Substeps};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use std::collections::{HashMap, HashSet, VecDeque};

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
    // Host-loss quiescence: when the client has no host, zero proxy velocities so
    // nothing dead-reckons/glides off (the disconnected cosim ball otherwise
    // launched to ~-195 km). The kinematic pin then holds them at their last
    // replicated pose until reconnect; the driver is gated off in parallel.
    status: Res<lunco_core::NetStatus>,
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
        // being pinned `Kinematic`. Phase B: a free predicted prop
        // (`PredictedDynamic`, e.g. a ball you bump) is likewise excluded — it
        // runs local physics + state-reconcile.
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
    let frozen = !status.connected; // host lost → quiesce
    for (e, rb, lin, ang) in q.iter_mut() {
        // `RigidBody` is an immutable Avian component — replace it via `insert`.
        if !matches!(*rb, RigidBody::Kinematic) {
            commands.entity(e).try_insert(RigidBody::Kinematic);
        }
        if frozen {
            // No authority to follow — pin velocity to zero so the body holds.
            if let Some(mut l) = lin {
                l.0 = DVec3::ZERO;
            }
            if let Some(mut a) = ang {
                a.0 = DVec3::ZERO;
            }
        }
        // NOTE (Step 1): when connected, velocity is NOT zeroed here. The proxy's velocity is
        // now *driven* every fixed tick toward the snapshot curve by
        // `drive_kinematic_proxies` (closed loop — a resting body's curve is flat so
        // it commands v≈0 anyway), which is what lets the proxy's motion enter
        // contact resolution. Zeroing it here would fight that driver.
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

/// Shared interpolation playback clock, in the host-tick timebase (anchored to the
/// snapshot stream, NOT wall time). Was a pair of `Local`s private to
/// [`interpolate_proxies`]; promoted to a resource so the upcoming
/// `drive_kinematic_proxies` (FixedUpdate) and `interpolate_proxies` (render) read
/// **one** render instant — otherwise the physics-driven `RigidBody` proxies and
/// the Transform-written `RigidBody`-less proxies would sample two slightly
/// different clocks and drift apart.
///
/// `t` is the current render instant (seconds, host-tick timebase); `init` guards
/// the first-sample snap. Advanced once per fixed tick by
/// [`advance_playback_clock`].
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct ProxyPlaybackClock {
    pub t: f64,
    pub init: bool,
}

/// Advance the shared playback clock toward `newest_gen − INTERP_DELAY` by `dt`
/// seconds (snap on first sample / large desync, else ease). Returns
/// `(render_instant, snapped)` — `snapped == true` on the first sample or a large
/// desync, which the FixedUpdate driver turns into a teleport instead of a
/// velocity command. `None` if there are no samples yet. Pure given its args so it
/// is unit-testable. `newest_gen` is the freshest host generation time across all
/// buffered bodies (the busiest body anchors the clock — a resting body stops
/// emitting). Advanced once per fixed tick by `drive_kinematic_proxies`.
fn advance_playback_clock(
    clock: &mut ProxyPlaybackClock,
    newest_gen: f64,
    dt: f64,
) -> Option<(f64, bool)> {
    if !newest_gen.is_finite() {
        return None; // no samples yet
    }
    let target = newest_gen - INTERP_DELAY;
    let snapped = !clock.init || (target - clock.t).abs() > CLOCK_SNAP;
    if snapped {
        // First run, or a large desync (long stall / tick discontinuity): snap.
        clock.t = target;
        clock.init = true;
    } else {
        clock.t += dt;
        clock.t += (target - clock.t) * CLOCK_EASE;
        // Never render past the freshest sample we hold.
        if clock.t > newest_gen {
            clock.t = newest_gen;
        }
    }
    Some((clock.t, snapped))
}

/// If a kinematic proxy's `Position` is further than this (metres) from where its
/// curve says it should be *right now*, teleport it instead of trying to close the
/// gap with one tick of velocity (which would be a huge, contact-disrupting kick).
/// Covers first sight, a long stall, and authoritative discontinuities.
const PROXY_SNAP_DIST: f64 = 2.0;

/// Time constant (seconds) for easing a proxy's residual position/orientation error
/// onto its curve. The proxy moves at the curve's **feed-forward velocity** (the
/// host's authoritative chassis velocity) and this softly corrects the small
/// leftover error over ~TAU — instead of a deadbeat `(target−pos)/h` that demanded
/// the whole gap in one tick (~50 m/s, which jittered and tunneled through
/// contacts). ~0.08 s ≈ 5 ticks at 60 Hz: snappy enough to track, soft enough not
/// to spike.
const PROXY_CORRECT_TAU: f64 = 0.08;

/// Cap (m/s) on the soft-correction term so that a proxy near the snap threshold
/// (error approaching `PROXY_SNAP_DIST`) still corrects gently rather than with a
/// big velocity; gross errors are handled by the teleport branch, not this.
const PROXY_CORRECT_MAX: f64 = 4.0;

/// Absolute cap (m/s) on a proxy's commanded velocity. No rover moves this fast;
/// the cap exists so a *diverging authoritative body* (e.g. a runaway cosim
/// balloon whose host-side physics blew up — a separate, known bug) can't fling
/// its proxy across the scene at hundreds of m/s. Past this the body is far enough
/// off that the teleport branch reseats it instead.
const PROXY_MAX_SPEED: f64 = 50.0;

/// Angular velocity (rad/s, world axis-angle) that rotates `from` onto `to` in one
/// step of `h` seconds: `ω = axis · θ / h` where `q_err = to · from⁻¹`. Takes the
/// **shortest arc** (negate `q_err` if `w < 0`, since `q` and `−q` are the same
/// orientation but the naive angle would be the long way round). Returns zero for a
/// negligible rotation. Used by `drive_kinematic_proxies` to drive a kinematic
/// proxy's `AngularVelocity` so its orientation tracks the snapshot curve through
/// the solver (and its spin enters contact resolution) instead of being teleported.
fn ang_vel_to_track(from: Quat, to: Quat, h: f64) -> DVec3 {
    let mut q_err = to * from.inverse();
    if q_err.w < 0.0 {
        q_err = Quat::from_xyzw(-q_err.x, -q_err.y, -q_err.z, -q_err.w);
    }
    let w = q_err.w.clamp(-1.0, 1.0);
    let sin_half = (1.0 - w * w).sqrt();
    if sin_half < 1e-6 {
        return DVec3::ZERO; // no meaningful rotation this step
    }
    let angle = 2.0 * (w.acos() as f64); // total rotation, radians
    let axis = Vec3::new(q_err.x, q_err.y, q_err.z) / sin_half;
    axis.as_dvec3() * (angle / h)
}

/// Sample the interpolation curve for one body's buffer at host-tick time `t`.
/// Shared by the render path ([`interpolate_proxies`]) and the FixedUpdate
/// velocity driver (`drive_kinematic_proxies`, Step 1.4) so both read the **same**
/// target pose. Returns `(pos_world, rot, lv, av)` or `None` when the buffer holds
/// nothing usable (empty / all-future-and-no-bracket collapses to None).
///
/// Position uses **cubic Hermite** through the bracketing samples' positions *and
/// velocities* `(a.pos, a.lv) → (b.pos, b.lv)` — so a body that is turning/accel-
/// erating follows a smooth curve that honours the sampled velocity at each end,
/// instead of the straight chord a plain lerp draws (which under-shoots arcs and
/// kinks at every sample). Rotation stays slerp (orientation is scale-free; a
/// cubic quaternion spline isn't worth the cost at 20 Hz). `lv`/`av` are the
/// bracketing-start sample's velocities (animation hint + driver feed-forward).
///
/// Starvation (render_t past the newest sample) keeps the existing linear glide
/// along `a.lv`, capped by [`INTERP_MAX_EXTRAPOLATION`] (time) and
/// [`INTERP_MAX_EXTRAP_DIST`] (distance) — a single sample has no second point for
/// a cubic, and an unbounded cubic extrapolation would fly off.
fn sample_curve(buf: &VecDeque<InterpSample>, t: f64) -> Option<(DVec3, Quat, DVec3, DVec3)> {
    // Samples are time-ordered: `a` = latest at/just before t, `b` = first after.
    let mut a: Option<&InterpSample> = None;
    let mut b: Option<&InterpSample> = None;
    for s in buf.iter() {
        if s.gen_t <= t {
            a = Some(s);
        } else {
            b = Some(s);
            break;
        }
    }
    match (a, b) {
        (Some(a), Some(b)) => {
            let span = (b.gen_t - a.gen_t).max(1e-5);
            let s = (((t - a.gen_t) / span).clamp(0.0, 1.0)) as f64;
            // Cubic Hermite. Tangents are velocity·span (curve param is s∈[0,1],
            // ds = dt/span, so dp/ds = v·span). lv is units/sec.
            let p0 = a.pos_world;
            let p1 = b.pos_world;
            let m0 = a.lv.as_dvec3() * span;
            let m1 = b.lv.as_dvec3() * span;
            let s2 = s * s;
            let s3 = s2 * s;
            let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
            let h10 = s3 - 2.0 * s2 + s;
            let h01 = -2.0 * s3 + 3.0 * s2;
            let h11 = s3 - s2;
            let pos = p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11;
            let rot = a.rot.slerp(b.rot, s as f32);
            Some((pos, rot, a.lv.as_dvec3(), a.av.as_dvec3()))
        }
        // t before the oldest sample → snap to oldest.
        (None, Some(b)) => Some((b.pos_world, b.rot, b.lv.as_dvec3(), b.av.as_dvec3())),
        // Starved (t past the newest sample). Glide linearly along the sample's
        // velocity so a mover keeps going instead of freezing then snapping;
        // capped in time and distance so a stalled/diverging body can't fly off.
        (Some(a), None) => {
            let dt = (t - a.gen_t).clamp(0.0, INTERP_MAX_EXTRAPOLATION);
            let mut delta = a.lv.as_dvec3() * dt;
            let len = delta.length();
            if len > INTERP_MAX_EXTRAP_DIST {
                delta *= INTERP_MAX_EXTRAP_DIST / len;
            }
            Some((a.pos_world + delta, a.rot, a.lv.as_dvec3(), a.av.as_dvec3()))
        }
        (None, None) => None,
    }
}
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
    // Drain in place — the body writes only into `buffers` (a separate
    // resource), never back into `snaps`, so the `.collect::<Vec<_>>()`
    // was a pure-waste allocation per ingest (CQ-216).
    for s in snaps.0.drain(..) {
        let buf = buffers.0.entry(s.gid).or_default();
        let gen_t = s.tick as f64 * SECS_PER_TICK;
        // Drop out-of-order / duplicate snapshots. `SnapChannel` is
        // `UnorderedUnreliable`, so a stale connect-baseline (or a reordered
        // datagram) can arrive *after* a newer periodic snapshot. Appending it
        // would seat an older sample behind the newest one, corrupting the
        // bracket search in `sample_curve` and snapping the proxy backward.
        // `back()` is always the highest tick accepted so far (we only ever
        // push strictly-newer samples and prune from the front), so it is the
        // correct monotonic gate.
        if buf.back().is_some_and(|last| gen_t <= last.gen_t) {
            continue;
        }
        buf.push_back(InterpSample {
            gen_t,
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

/// L1: free a despawned proxy's interpolation buffer. `ingest_snapshots` inserts a
/// `VecDeque` per gid on first sight (`entry(gid).or_default()`), but the client
/// Despawn arm (`lunco_networking::sync`) only despawns the entity + cleans the
/// `ApiEntityRegistry` — it never touches `InterpBuffers`, which lives in this
/// crate. Without this the map leaks one ring per ever-seen gid; worse, once
/// interest-management churns proxies in/out, a gid that leaves then re-enters
/// replays its STALE pre-exit samples — the H3 monotonic gate only blocks ticks
/// older than `back()`, so old positions still bracket the fresh sample in
/// `interpolate_proxies` → a visible teleport on the visual-only path (the
/// `PROXY_SNAP_DIST` guard exists only on the physics `drive_kinematic_proxies`
/// path). Pruning on despawn fixes both. `RemovedComponents<GlobalEntityId>` yields
/// entities, not gids, and the despawned entity can no longer be queried, so cache
/// Entity→gid from `Added` — the same incremental pattern `broadcast_despawns` uses.
pub fn prune_interp_buffers_on_despawn(
    mut removed: RemovedComponents<lunco_core::GlobalEntityId>,
    q_added: Query<(Entity, &lunco_core::GlobalEntityId), Added<lunco_core::GlobalEntityId>>,
    mut known: Local<HashMap<Entity, u64>>,
    mut buffers: ResMut<InterpBuffers>,
) {
    for (entity, gid) in q_added.iter() {
        known.insert(entity, gid.get());
    }
    for entity in removed.read() {
        if let Some(gid) = known.remove(&entity) {
            buffers.0.remove(&gid);
        }
    }
}

/// Client: render replicated proxies that have **no `RigidBody`** by writing their
/// `Transform` straight from the interpolation curve, [`INTERP_DELAY`] in the past
/// — turning 20 Hz snapshots into smooth per-frame motion for non-physics bodies
/// (markers, visual-only props). A body with no fresh samples holds its last pose.
///
/// Bodies **with** a `RigidBody` are skipped here: as of Step 1 they are driven
/// through the solver by [`drive_kinematic_proxies`] (velocity toward the same
/// shared curve) and rendered from avian's `Position → Transform` writeback, so
/// their contact velocity is real and they push locally-predicted bodies crisply.
/// This system is **read-only** on [`ProxyPlaybackClock`]; the driver advances the
/// clock once per fixed tick so both paths sample one render instant.
///
/// (The body this client possesses, and free predicted props, are excluded via
/// `q_local_sim` — they're locally simulated + reconciled, not interpolated.)
pub fn interpolate_proxies(
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    buffers: Res<InterpBuffers>,
    // Shared playback clock, advanced in FixedUpdate by `drive_kinematic_proxies`.
    // Read-only here — this is the render projection of the same instant.
    clock: Res<ProxyPlaybackClock>,
    // Predict-own: the possessed rover is locally simulated + smooth-corrected
    // (`reconcile_owned_prediction`), so it must NOT be dragged back to the
    // `INTERP_DELAY`-old interpolated pose here. Phase B: a free predicted prop
    // (`PredictedDynamic`) is likewise locally simulated + state-reconciled, so it
    // is excluded too.
    q_local_sim: Query<
        (),
        Or<(
            With<lunco_core::OwnedLocally>,
            With<lunco_core::PredictedDynamic>,
        )>,
    >,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut Rotation>,
        // Animation motion hint: stamp the snapshot's authoritative chassis
        // velocity here for the wheel-spin model to read (see
        // [`lunco_core::ReplicatedChassisMotion`]).
        Option<&mut lunco_core::ReplicatedChassisMotion>,
        // Skip physics bodies — they're solver-driven by `drive_kinematic_proxies`.
        Has<RigidBody>,
    )>,
    // Insert the motion hint on first sight of a proxy that lacks it.
    mut commands: Commands,
) {
    if !clock.init {
        return; // clock not started (no samples ingested yet)
    }
    let render_t = clock.t;
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
        let Ok((mut tf, pos, rot, motion, has_rb)) = q.get_mut(e) else {
            continue;
        };
        if has_rb {
            continue; // physics-driven by `drive_kinematic_proxies`; rendered via writeback
        }

        // Shared curve evaluator (cubic-Hermite position + slerp rotation +
        // starvation glide). Returns the bracketing-start velocities for the
        // animation hint below.
        let Some((out_world, out_rot, lv, av)) = sample_curve(buf, render_t) else {
            continue;
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
        // `force_kinematic_proxies` zeros away). `lv`/`av` are the bracketing-start
        // sample's velocities from `sample_curve` (changes at the 20 Hz snapshot
        // rate — plenty for animation).
        let hint = lunco_core::ReplicatedChassisMotion { lin: lv, ang: av };
        match motion {
            Some(mut m) => *m = hint,
            None => {
                commands.entity(e).try_insert(hint);
            }
        }
    }
}

/// Client (FixedUpdate): drive each kinematic replicated proxy that has a
/// `RigidBody` **through the solver** by setting its `LinearVelocity` /
/// `AngularVelocity` toward the shared interpolation curve, instead of teleporting
/// its `Transform` each frame. This is the core of Step 1 (predict-and-smooth; design in git history):
///
/// * The proxy stays `RigidBody::Kinematic` (pinned by `force_kinematic_proxies`),
///   so the host stays authoritative — but now it carries a *real velocity* the
///   solver knows about. A locally-predicted body (your owned rover, a
///   `PredictedDynamic` prop) that rams it gets pushed crisply in the same step,
///   instead of interpenetrating and being shoved out by overlap-recovery alone
///   (the source of the contact buzz). Confirmed by the Step 1.1 avian probe.
/// * Each tick the velocity is recomputed toward the curve (closed loop), so error
///   cannot accumulate beyond one step — no balloon drift, and a resting body's
///   curve is flat ⇒ `v ≈ 0` ⇒ it sits still (no settled-rover blink).
///
/// Advances the shared [`ProxyPlaybackClock`] once here (its single advance site);
/// `interpolate_proxies` reads the same instant. Target pose is sampled one tick
/// **ahead** (`render_t + SECS_PER_TICK`) so that `v = (target − pos)/h` lands the
/// body on the curve after avian integrates this tick. Teleports instead of
/// commanding velocity when the clock snapped (first sample / large desync) or the
/// body is more than [`PROXY_SNAP_DIST`] off its current curve point — a one-tick
/// velocity to close a big gap would be a contact-disrupting kick.
pub fn drive_kinematic_proxies(
    role: Res<lunco_core::NetworkRole>,
    // Host-loss quiescence: with no authoritative snapshots arriving, driving
    // proxies off the starved curve would dead-reckon them off into space (the
    // disconnected cosim ball launched to ~-195 km). Stop driving when not
    // connected; `force_kinematic_proxies` freezes them (kinematic + zero vel).
    status: Res<lunco_core::NetStatus>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    buffers: Res<InterpBuffers>,
    mut clock: ResMut<ProxyPlaybackClock>,
    // Excluded: locally-simulated bodies (owned rover, predicted props) run their
    // own Dynamic step + reconcile, not curve-following.
    q_local_sim: Query<
        (),
        Or<(
            With<lunco_core::OwnedLocally>,
            With<lunco_core::PredictedDynamic>,
        )>,
    >,
    mut q: Query<
        (
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
            Option<&mut lunco_core::ReplicatedChassisMotion>,
        ),
        With<RigidBody>,
    >,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    if !status.connected {
        return; // host lost — freeze (see `force_kinematic_proxies`), don't dead-reckon
    }
    // Advance the shared clock once per fixed tick (its only advance site).
    let newest_gen = buffers
        .0
        .values()
        .filter_map(|b| b.back())
        .map(|s| s.gen_t)
        .fold(f64::NEG_INFINITY, f64::max);
    let Some((render_t, snapped)) = advance_playback_clock(&mut clock, newest_gen, SECS_PER_TICK)
    else {
        return; // no samples yet
    };
    for (gid, buf) in buffers.0.iter() {
        if buf.is_empty() {
            continue;
        }
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(*gid)) else {
            continue;
        };
        if q_local_sim.contains(e) {
            continue;
        }
        let Ok((mut pos, mut rot, mut lin, mut ang, motion)) = q.get_mut(e) else {
            continue; // not a RigidBody proxy (e.g. visual-only — handled by interpolate)
        };
        // Where the curve says this body is right now, plus its feed-forward
        // velocity (`lv`/`av` = the host's authoritative chassis velocity).
        let Some((here, here_rot, lv, av)) = sample_curve(buf, render_t) else {
            continue;
        };

        let off = (pos.0 - here).length();
        if snapped || off > PROXY_SNAP_DIST {
            // Teleport: seat pose, kill velocity. Covers first sight / long stall /
            // authoritative discontinuity — closing this gap with one tick of
            // velocity would be a violent kick into anything in contact.
            pos.0 = here;
            rot.0 = here_rot.as_dquat();
            lin.0 = DVec3::ZERO;
            ang.0 = DVec3::ZERO;
        } else {
            // Feed-forward curve velocity + soft position correction over TAU (NOT
            // deadbeat: the old `(target−pos)/h` commanded ~50 m/s → jitter +
            // contact tunnelling). The body moves at the host's real chassis speed
            // and the small residual error eases in.
            let mut corr = (here - pos.0) / PROXY_CORRECT_TAU;
            let cl = corr.length();
            if cl > PROXY_CORRECT_MAX {
                corr *= PROXY_CORRECT_MAX / cl;
            }
            let mut v = lv + corr;
            let vl = v.length();
            if vl > PROXY_MAX_SPEED {
                v *= PROXY_MAX_SPEED / vl; // backstop a diverging authoritative body
            }
            lin.0 = v;
            ang.0 = av + ang_vel_to_track(rot.0.as_quat(), here_rot, PROXY_CORRECT_TAU);
        }

        // Animation hint = authoritative chassis velocity (moved here from
        // `interpolate_proxies`, which no longer touches RigidBody proxies).
        let hint = lunco_core::ReplicatedChassisMotion { lin: lv, ang: av };
        match motion {
            Some(mut m) => *m = hint,
            None => {
                commands.entity(e).try_insert(hint);
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
/// TEST TOGGLE (`LUNCO_NO_PREDICT=1`): disable local physics-prediction of the
/// owned rover, so it follows the host authoritatively like every other body
/// (kinematic proxy via `drive_kinematic_proxies`). Used to validate the
/// "visual-prediction" direction before building the render-lead: if the wobble +
/// bad body-interactions vanish in follow mode, physics-prediction was the cause.
/// Read once (env is process-static).
fn no_local_predict() -> bool {
    use std::sync::OnceLock;
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("LUNCO_NO_PREDICT").as_deref() == Ok("1"))
}

/// Live-tunable settings for VISUAL PREDICTION (the owned rover follows the host
/// authoritatively for PHYSICS — no wobble, correct contacts — while
/// `lead_owned_rover_render` leads its RENDERED pose so it feels responsive at any
/// ping; see `project_predict_own_oscillation_cadence`). A resource, not consts, so
/// it can be tuned LIVE via the `SetVisualLead` command (no rebuild). Env vars seed
/// the defaults: `LUNCO_VISUAL_PREDICT=1` → `enabled`, `LUNCO_SIM_LATENCY_MS` →
/// `lead_secs` (the display lag to hide; in production this tracks measured RTT).
#[derive(Resource, Clone, Debug)]
pub struct VisualLeadSettings {
    /// Master: visual-prediction on (follow-authority physics + render-lead).
    pub enabled: bool,
    /// Yaw lead rate — rad/s at full steer.
    pub yaw_rate: f32,
    /// Forward lead speed — m/s at full throttle.
    pub speed: f32,
    /// Lead time (s): how far ahead of authority to lead the visual. 0 disables.
    pub lead_secs: f32,
}

impl Default for VisualLeadSettings {
    fn default() -> Self {
        let enabled = std::env::var("LUNCO_VISUAL_PREDICT").as_deref() == Ok("1");
        let lead_secs = std::env::var("LUNCO_SIM_LATENCY_MS")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.0)
            / 1000.0;
        // Gentle defaults — the lead is SMOOTHED (eased) so it never leaps; tune up
        // via `SetVisualLead` to taste.
        Self {
            enabled,
            yaw_rate: 0.5,
            speed: 4.0,
            lead_secs,
        }
    }
}

/// Per-gid SMOOTHED render-lead offset `(yaw_rad, forward_m)` — eased toward the
/// input-driven target each frame so the visual never leaps/snaps when you
/// tap/release throttle or steer (the abrupt-jump artifact of the first version:
/// a 300 ms lead applied instantly is ~1.8 m + ~12° in one frame). Client-only,
/// presentational.
#[derive(Resource, Default)]
struct VisualLeadState(std::collections::HashMap<u64, (f32, f32)>);

/// Live-tune [`VisualLeadSettings`] (all fields optional → set only what you pass):
/// `SetVisualLead {enabled?, yaw_rate?, speed?, lead_secs?}`. Lets you A/B the
/// render-lead strength while driving, no rebuild.
#[Command(default)]
pub struct SetVisualLead {
    #[serde(default)]
    #[reflect(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    #[reflect(default)]
    pub yaw_rate: Option<f32>,
    #[serde(default)]
    #[reflect(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    #[reflect(default)]
    pub lead_secs: Option<f32>,
}

/// Observer for [`SetVisualLead`] — apply the passed fields to the live resource.
#[on_command(SetVisualLead)]
pub fn on_set_visual_lead(trigger: On<SetVisualLead>, mut s: ResMut<VisualLeadSettings>) {
    if let Some(v) = cmd.enabled {
        s.enabled = v;
    }
    if let Some(v) = cmd.yaw_rate {
        s.yaw_rate = v;
    }
    if let Some(v) = cmd.speed {
        s.speed = v;
    }
    if let Some(v) = cmd.lead_secs {
        s.lead_secs = v;
    }
    info!(
        "[visual-lead] enabled={} yaw_rate={:.2} speed={:.2} lead_secs={:.3}",
        s.enabled, s.yaw_rate, s.speed, s.lead_secs
    );
}

pub fn maintain_owned_locally(
    role: Res<lunco_core::NetworkRole>,
    local: Res<lunco_core::LocalSession>,
    reg: Res<lunco_core::SessionRegistry>,
    // Prediction membership is **computability**, not ownership (Phase A;
    // design in git history): predict the owned rover only while THIS peer is
    // actively driving it. A possessed-but-idle rover is dominated by external
    // forces (another rover pushing it, cosim) the client can't reproduce, so it
    // must interpolate as a normal proxy — else it free-runs local physics with no
    // working correction ("pushed without contact").
    tick: Res<lunco_core::SimTick>,
    input_log: Res<lunco_core::OwnedInputLog>,
    // Freshest authoritative snapshot per gid — the seed for a newly-promoted
    // predicted body (see the promote arm). avian is deterministic
    // (`determinism_probe`), so aligning the prediction's START to authority makes
    // its trajectory track the host instead of running a constant INTERP_DELAY
    // behind (the offset the reconcile keeps chasing → the drive-fighting wobble).
    buffers: Res<InterpBuffers>,
    lead: Res<VisualLeadSettings>,
    mut commands: Commands,
    q: Query<
        (
            Entity,
            &lunco_core::GlobalEntityId,
            Has<lunco_core::OwnedLocally>,
        ),
        // Skip articulated wheels: they are never owned in the registry (only the
        // chassis gid is claimed), so this system would strip the `OwnedLocally`
        // that `propagate_owned_to_wheels` mirrors onto an owned rover's wheels.
        (
            With<lunco_core::NetReplicate>,
            Without<lunco_core::ArticulatedLink>,
        ),
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
        let last_active = input_log
            .0
            .get(&gid.get())
            .map_or(0, |l| l.last_active_tick);
        // `no_local_predict()` / `visual_predict()` force follow-authority mode:
        // never promote to a local `Dynamic` step (the wobble source) — the rover
        // stays a kinematic proxy. In `visual_predict` the render-lead adds back
        // responsiveness on the presentation layer only.
        let mine = predicts_locally(owns, last_active, tick.0, PREDICT_GRACE_TICKS)
            && !no_local_predict()
            && !lead.enabled;
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
                info!(
                    "[predict] promote owned rover gid={} -> Dynamic (last_active={}, now={})",
                    gid.get(),
                    last_active,
                    tick.0
                );
                commands
                    .entity(e)
                    .try_insert((lunco_core::OwnedLocally, RigidBody::Dynamic));
                // SEED FROM AUTHORITY (predict-alignment, Stage 1): overwrite the
                // INTERP_DELAY-stale interpolated pose the proxy carried with the
                // FRESHEST authoritative snapshot, so the deterministic prediction
                // starts where the host is — not ~2-3 ticks behind. Deferred through
                // a world closure so it lands after the `Dynamic` flip; `Position`/
                // `Rotation` are the physics truth the bridge syncs to `Transform`.
                if let Some(s) = buffers.0.get(&gid.get()).and_then(|b| b.back()).copied() {
                    let ent = e;
                    commands.queue(move |world: &mut World| {
                        let Ok(mut em) = world.get_entity_mut(ent) else {
                            return;
                        };
                        if let Some(mut p) = em.get_mut::<Position>() {
                            p.0 = s.pos_world;
                        }
                        if let Some(mut r) = em.get_mut::<Rotation>() {
                            r.0 = s.rot.as_dquat();
                        }
                        if let Some(mut lv) = em.get_mut::<LinearVelocity>() {
                            lv.0 = s.lv.as_dvec3();
                        }
                        if let Some(mut av) = em.get_mut::<AngularVelocity>() {
                            av.0 = s.av.as_dvec3();
                        }
                    });
                }
            }
            (false, true) => {
                info!(
                    "[predict] demote owned rover gid={} (idle/released)",
                    gid.get()
                );
                commands.entity(e).remove::<lunco_core::OwnedLocally>();
            }
            _ => {}
        }
    }
}

/// Client-only: mirror an [`lunco_core::ArticulatedVehicle`] chassis's
/// [`lunco_core::OwnedLocally`] state onto its wheels ([`lunco_core::ArticulatedLink`]).
///
/// With per-link replication the wheels carry [`lunco_core::NetReplicate`], so by
/// default a client would pin them `Kinematic` and snapshot-drive them
/// (`force_kinematic_proxies` / `drive_kinematic_proxies`). That is correct for a
/// *remote* rover (a fully pose-forced assembly), but WRONG for the rover this
/// client possesses and drives: its chassis runs local predicted physics
/// (`maintain_owned_locally`), and its wheels must run the **same** local physics
/// (real joints + drive motors) — otherwise the wheels of the rover you are
/// driving freeze while the chassis predicts.
///
/// `OwnedLocally` is the marker every proxy seam already keys off, so mirroring it
/// onto the wheels excludes them from the kinematic-proxy path
/// (`force_kinematic_proxies`' `Without<OwnedLocally>` + `drive_kinematic_proxies`'
/// `q_local_sim`) exactly like the chassis. Runs right after
/// `maintain_owned_locally`; one fixed/Update tick of latency on a possession flip
/// is imperceptible and self-corrects. Wheel→chassis is read from `ChildOf` (the
/// wheel keeps its chassis parent), so this needs no `lunco-usd-sim` types.
pub fn propagate_owned_to_wheels(
    role: Res<lunco_core::NetworkRole>,
    q_owned_chassis: Query<
        (),
        (
            With<lunco_core::OwnedLocally>,
            With<lunco_core::ArticulatedVehicle>,
        ),
    >,
    q_wheels: Query<
        (Entity, &ChildOf, Has<lunco_core::OwnedLocally>),
        With<lunco_core::ArticulatedLink>,
    >,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, child_of, has_marker) in q_wheels.iter() {
        let owned = q_owned_chassis.contains(child_of.parent());
        match (owned, has_marker) {
            (true, false) => {
                // Chassis just became owned: claim the wheel for local physics.
                // Restore `Dynamic` too — the wheel may have been pinned
                // `Kinematic` for many frames as a proxy (mirrors the chassis
                // restore in `maintain_owned_locally`).
                commands
                    .entity(e)
                    .try_insert((lunco_core::OwnedLocally, RigidBody::Dynamic));
            }
            (false, true) => {
                // Chassis released: hand the wheel back to the snapshot-driven
                // proxy path (`force_kinematic_proxies` re-pins it `Kinematic`).
                commands.entity(e).remove::<lunco_core::OwnedLocally>();
            }
            _ => {}
        }
    }
}

/// VISUAL PREDICTION (client, `LUNCO_VISUAL_PREDICT=1`): lead the owned rover's
/// RENDERED pose ahead of its authoritative pose by ~RTT, from the local drive
/// input, so driving feels responsive at any ping while physics stays 100%
/// host-authoritative (no wobble, correct contacts). Runs in `Last` — after ALL
/// transform propagation (incl. big_space) — and offsets `GlobalTransform` (the
/// render truth) for the owned rover AND its whole visual assembly (chassis +
/// wheel/mesh children). Recomputed fresh each frame from the *current* input, so
/// nothing accumulates, the sim (`Transform`/`Position`) is never touched, and when
/// you stop steering the lead decays to zero — easing onto authority with no snap.
fn lead_owned_rover_render(
    role: Res<lunco_core::NetworkRole>,
    local: Res<lunco_core::LocalSession>,
    reg: Res<lunco_core::SessionRegistry>,
    drive: Res<lunco_core::LocalDriveInput>,
    settings: Res<VisualLeadSettings>,
    time: Res<Time>,
    mut state: ResMut<VisualLeadState>,
    // Single-body (raycast) rovers ONLY: an articulated rover's wheels are separate
    // physics bodies with joints, so rigidly offsetting their `GlobalTransform`
    // fights the joint solver → jitter. Those stay follow-authority (no lead).
    q_rovers: Query<
        (Entity, &lunco_core::GlobalEntityId),
        (
            With<lunco_core::NetReplicate>,
            Without<lunco_core::ArticulatedLink>,
            Without<lunco_core::ArticulatedVehicle>,
        ),
    >,
    q_children: Query<&Children>,
    mut q_gt: Query<&mut GlobalTransform>,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) || !settings.enabled {
        return;
    }
    let lead = settings.lead_secs;
    if lead <= 0.0 {
        return;
    }
    // Ease the offset toward its target over ~TAU seconds (frame-rate independent),
    // so tapping/releasing input never leaps or snaps the visual.
    const TAU: f32 = 0.12;
    let alpha = 1.0 - (-time.delta_secs() / TAU).exp();
    for (e, gid) in q_rovers.iter() {
        if !reg.owns(local.0, gid.get()) {
            continue;
        }
        let (throttle, steer) = drive.0.get(&gid.get()).copied().unwrap_or((0.0, 0.0));
        // Target offset from current input; eased into the persistent smoothed value.
        let tgt_yaw = steer as f32 * settings.yaw_rate * lead;
        let tgt_dist = throttle as f32 * settings.speed * lead;
        let slot = state.0.entry(gid.get()).or_insert((0.0, 0.0));
        slot.0 += (tgt_yaw - slot.0) * alpha;
        slot.1 += (tgt_dist - slot.1) * alpha;
        let (yaw, dist) = *slot;
        // Below a hair of offset, skip (also lets a released rover settle exactly).
        if yaw.abs() < 1e-4 && dist.abs() < 1e-4 {
            continue;
        }
        let (c, fwd) = {
            let Ok(gt) = q_gt.get(e) else { continue };
            let fwd = (gt.rotation() * Vec3::NEG_Z)
                .with_y(0.0)
                .normalize_or_zero();
            (gt.translation(), fwd)
        };
        // World rigid delta: yaw about the rover's centre, then translate forward.
        let d = bevy::math::Affine3A::from_translation(fwd * dist)
            * bevy::math::Affine3A::from_translation(c)
            * bevy::math::Affine3A::from_rotation_y(yaw)
            * bevy::math::Affine3A::from_translation(-c);
        // Collect the assembly (rover + all VISUAL descendants) and offset each GT.
        let mut all = vec![e];
        let mut stack = vec![e];
        while let Some(cur) = stack.pop() {
            if let Ok(children) = q_children.get(cur) {
                for ch in children.iter() {
                    all.push(ch);
                    stack.push(ch);
                }
            }
        }
        for ent in all {
            if let Ok(mut gt) = q_gt.get_mut(ent) {
                *gt = GlobalTransform::from(d * gt.affine());
            }
        }
    }
}

/// HOST (FixedFirst): apply EXACTLY ONE buffered client input per fixed tick, in
/// seq order, to each remote-owned rover — so the host integrates the same input
/// sequence one-input-per-physics-step as the owning client predicted with. Without
/// this the host applied forwarded `SetPorts` at render cadence (`on_set_ports` in
/// `Update`, port-latched), so its rover saw a DIFFERENT number of drive steps than
/// the client's local prediction → the two deterministic sims diverged → the
/// reconcile had to fight it (the wobble). Runs before the drive reads the ports;
/// `on_set_ports`' later `Update` write is harmlessly overwritten next tick (the
/// consumer latches the last input, so it stays the port authority). Host-only.
fn apply_buffered_client_inputs(
    role: Res<lunco_core::NetworkRole>,
    mut buf: ResMut<lunco_core::BufferedClientInputs>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    ports: Res<lunco_core::ports::PortRegistry>,
    // The reconcile ack is stamped HERE, from the seq this tick actually integrated
    // (review N2) — see the comment in the loop.
    sessions: Res<lunco_core::SessionRegistry>,
    mut applied: ResMut<lunco_core::AppliedInputSeq>,
    mut commands: Commands,
) {
    if !role.is_host() {
        return;
    }
    let gids: Vec<u64> = buf
        .pending
        .keys()
        .chain(buf.last_writes.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for gid in gids {
        let Some(writes) = buf.next_for_tick(gid, 8) else {
            continue;
        };
        // THE ACK (review N2). `next_for_tick` advanced the per-gid cursor by at most
        // ONE seq — the input this fixed tick will integrate — so the cursor is the
        // honest "how far the authoritative sim has consumed your input" watermark.
        // Stamped even if the entity fails to resolve below: the input was consumed
        // either way, and a stalled ack would strand the owner's reconcile.
        // `record` also re-keys the slot to the current owner and rejects an
        // implausible seq (review N1).
        applied.record(gid, sessions.owner_of(gid), buf.cursor(gid));
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(gid)) else {
            continue;
        };
        let reg = ports.clone();
        commands.queue(move |world: &mut World| {
            for (port, value) in &writes {
                reg.write_port(world, e, port, *value);
            }
        });
    }
}

/// Cap on the predicted-state history ring (~2 s at 60 Hz). Only the recent tail
/// (the unacked window) is ever compared.
const MAX_PREDICTED_HISTORY: usize = 128;

/// How many ticks after the last nonzero local input a vessel stays in the
/// predicted set (`maintain_owned_locally`). Was 30 (~0.5 s), but a rover released
/// at speed COASTS for seconds; lapsing mid-coast hands it to the kinematic-proxy
/// path, which drags it back toward the `INTERP_DELAY`-stale curve (~0.3 m/frame
/// backward steps caught by the render-jitter detector) — a visible warp on every
/// key release. ~4 s covers a coast-to-stop; an external push on a *parked* owned
/// rover still renders correctly after the longer lapse, and during the grace a
/// push is locally computable anyway now that other vehicles are predicted
/// (Step 4). Coasting itself is deterministic zero-input physics — predictable.
const PREDICT_GRACE_TICKS: u64 = 240;

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

// ─────────────────────────── Deterministic rollback ───────────────────────────
//
// The proper fix for predict-own divergence. The old `reconcile_owned_prediction`
// BLENDS toward authority — a proportional controller that fights the live drive
// and, under a changing (steering) input, never settles: the post-turn wobble.
//
// Rollback instead RE-DERIVES the present from the authoritative past: snap the
// rover to the state the host actually had at the acked tick, then deterministically
// re-simulate every input we've sent since. avian is deterministic (`determinism_probe`),
// so the replay reproduces the host's trajectory exactly — the rover responds
// immediately to local input AND carries no accumulating error, at any ping.
//
// Validated headlessly by `rollback_probe` before wiring: on the real solver, a
// public-state-only restore + input replay reconverges to 0.24 mm steady-state
// (vs 102 m free-running). Crucially it needs NO solver warm-start/contact-cache
// restoration — which is what makes it implementable from a network snapshot.

/// Enable deterministic rollback (`LUNCO_ROLLBACK=1`). Default OFF: the shipped
/// path stays the current reconcile, so this cannot regress anything until chosen.
fn rollback_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LUNCO_ROLLBACK").map_or(false, |v| v == "1" || v == "true"))
}

/// Don't rollback for noise. Below this the prediction already matches authority,
/// and re-simulating would burn N physics steps to move the rover a few mm.
const ROLLBACK_POS_EPS: f64 = 0.02; // m
/// Rotation error (radians) that also justifies a re-simulation — heading error is
/// what actually compounds under drive, so it gets its own (tight) trigger.
const ROLLBACK_ROT_EPS: f64 = 0.01; // ~0.6°
/// Safety cap on replayed steps per correction. At 60 Hz this is ~0.5 s of unacked
/// input (≈500 ms RTT). Beyond it we snap without replay rather than stall the frame.
const MAX_REPLAY_STEPS: usize = 32;

/// Full physics state of ONE rigid body — everything a rollback restore needs, and
/// exactly what avian reconverges from (no warm-start/contact caches; see the probe).
#[derive(Clone, Copy)]
struct RbState {
    pos: DVec3,
    rot: DQuat,
    lv: DVec3,
    av: DVec3,
}

/// The owned rover's ENTIRE articulated assembly at one input `seq`.
///
/// Why the whole assembly and not just the chassis: `apply_net_replication` excludes
/// `ArticulatedLink`, so the wire carries the CHASSIS ONLY (the client rebuilds wheels
/// from it). Snapping just the chassis to authority would leave its four wheel bodies
/// behind and tear the revolute joints apart. So we keep a client-LOCAL history of every
/// link (zero wire cost) and restore the assembly as a RIGID BODY — chassis lands exactly
/// on authority while suspension compression, steer angle and wheel spin stay internally
/// consistent.
#[derive(Clone)]
struct AssemblyState {
    seq: u32,
    chassis: RbState,
    links: Vec<(Entity, RbState)>,
}

/// Per-gid ring of recent assembly states, keyed by the input seq that produced them.
#[derive(Resource, Default)]
pub struct AssemblyHistory(HashMap<u64, VecDeque<AssemblyState>>);

fn rb_state(p: &Position, r: &Rotation, lv: &LinearVelocity, av: &AngularVelocity) -> RbState {
    RbState {
        pos: p.0,
        rot: r.0,
        lv: lv.0,
        av: av.0,
    }
}

/// Record the owned rover's full assembly each fixed tick, keyed by the input seq
/// applied that tick — the history rollback rewinds into. `FixedPostUpdate` after
/// avian writeback (so it captures the post-step truth), and NOT during a replay.
pub fn record_assembly_state(
    input_log: Res<lunco_core::OwnedInputLog>,
    mut hist: ResMut<AssemblyHistory>,
    // The chassis: owned + articulated root. `propagate_owned_to_wheels` mirrors
    // `OwnedLocally` onto the wheels too, so links MUST be excluded here or each
    // wheel would be mistaken for a chassis.
    q_chassis: Query<
        (
            Entity,
            &lunco_core::GlobalEntityId,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
        ),
        (
            With<lunco_core::OwnedLocally>,
            Without<lunco_core::ArticulatedLink>,
        ),
    >,
    // Every rigid body in the rover, found by walking the subtree — NOT by assuming
    // the wheels are direct children of the body that carries the gid. That
    // assumption produced `links=0` in the live client: the rollback snapped the
    // chassis to authority, left the four wheel bodies behind (they were even
    // *restored* afterwards as part of the frozen set), tore the revolute joints,
    // and launched the rover tens of metres. Walk the real hierarchy instead.
    q_children: Query<&Children>,
    q_body: Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), With<RigidBody>>,
) {
    if !rollback_enabled() {
        return;
    }
    for (chassis_e, gid, p, r, lv, av) in q_chassis.iter() {
        let g = gid.get();
        let Some(seq) = input_log
            .0
            .get(&g)
            .and_then(|l| l.frames.back())
            .map(|f| f.seq)
        else {
            continue;
        };
        let ring = hist.0.entry(g).or_default();
        if ring.back().is_some_and(|s| s.seq == seq) {
            continue; // one record per input seq
        }
        let links = collect_assembly_links(chassis_e, &q_children, &q_body);
        ring.push_back(AssemblyState {
            seq,
            chassis: rb_state(p, r, lv, av),
            links,
        });
        while ring.len() > MAX_PREDICTED_HISTORY {
            ring.pop_front();
        }
    }
}

/// Every rigid body in the rover's subtree (wheels, bogies, any jointed part),
/// excluding the root itself. Breadth-first over `Children`, so it doesn't care how
/// deeply the USD prim hierarchy nests the links under the articulation root.
fn collect_assembly_links(
    root: Entity,
    q_children: &Query<&Children>,
    q_body: &Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), With<RigidBody>>,
) -> Vec<(Entity, RbState)> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = q_children
        .get(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    while let Some(e) = stack.pop() {
        if let Ok((p, r, lv, av)) = q_body.get(e) {
            out.push((e, rb_state(p, r, lv, av)));
        }
        if let Ok(children) = q_children.get(e) {
            stack.extend(children.iter());
        }
    }
    out
}

/// Advance physics exactly one deterministic tick, replaying `input` — the same
/// (actuation → solve) pair a live fixed tick performs, and nothing else.
///
/// Mirrors avian's `run_physics_schedule`: we cannot call that (it is a system inside
/// `FixedPostUpdate`) nor re-enter `FixedMain` (Bevy takes a schedule OUT of the world
/// to run it, so re-entrancy is impossible — and it would re-run every unrelated fixed
/// system N times per correction). Instead: run the mirrored actuation chain
/// (`RollbackReplay`), then step `PhysicsSchedule` by the fixed delta.
fn replay_one_tick(
    world: &mut World,
    ports: &lunco_core::ports::PortRegistry,
    chassis: Entity,
    input: &lunco_core::InputFrame,
) {
    // Feed the RECORDED input by writing the ports directly. Deliberately NOT a
    // `SetPorts` trigger: that would fire `record_control_input`, re-logging an input
    // we are merely re-simulating (and bumping the host ack bookkeeping).
    ports.write_port(world, chassis, "throttle", input.forward);
    ports.write_port(world, chassis, "steer", input.steer);
    ports.write_port(world, chassis, "brake", input.brake);

    let dt = world.resource::<Time<Fixed>>().delta();

    // Actuation runs on the FIXED clock, as it does live.
    *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();
    world.run_schedule(lunco_core::RollbackReplay);

    // Solve. Advance the physics + substep clocks exactly as avian's driver does,
    // then run the schedule (which includes the big_space bridge's Prepare/Writeback).
    world.resource_mut::<Time<Physics>>().advance_by(dt);
    let SubstepCount(substeps) = *world.resource::<SubstepCount>();
    world
        .resource_mut::<Time<Substeps>>()
        .advance_by(dt.div_f64(substeps as f64));
    *world.resource_mut::<Time>() = world.resource::<Time<Physics>>().as_generic();
    world.run_schedule(PhysicsSchedule);
}

/// **Deterministic rollback reconciliation** for the owned, locally-predicted rover.
///
/// On each snapshot that acks a NEW input seq: if our prediction at that seq diverged
/// from authority, rewind the whole assembly onto the authoritative state and re-simulate
/// every unacked input forward to the present. The rover therefore always shows a state
/// that is (a) derived from the host's truth and (b) already includes every input the
/// player has pressed since — immediate response, no accumulating error.
///
/// Exclusive, in `Update` (after `ingest_snapshots`, so the freshest ack is visible) —
/// it MUST live outside the fixed loop to run schedules at all.
pub fn rollback_owned_prediction(world: &mut World) {
    if !rollback_enabled() {
        return;
    }

    // ── Gather the owned chassis + its ack, without holding any borrows ──
    let mut owned: Vec<(Entity, u64)> = Vec::new();
    {
        let mut q = world.query_filtered::<(Entity, &lunco_core::GlobalEntityId), (
            With<lunco_core::OwnedLocally>,
            Without<lunco_core::ArticulatedLink>,
        )>();
        for (e, gid) in q.iter(world) {
            owned.push((e, gid.get()));
        }
    }
    if owned.is_empty() {
        return;
    }
    // Which owned bodies are articulated (have joints that a partial restore would tear).
    let articulated_set: HashSet<Entity> = {
        let mut q = world.query_filtered::<Entity, With<lunco_core::ArticulatedVehicle>>();
        q.iter(world).collect()
    };

    for (chassis, gid) in owned {
        // Authority + the highest input seq the host has applied for us.
        let Some(sample) = world
            .resource::<InterpBuffers>()
            .0
            .get(&gid)
            .and_then(|b| b.back())
            .copied()
        else {
            continue;
        };
        let ack = sample.last_input_seq;
        if ack == 0 {
            continue; // host hasn't applied any of our input yet
        }
        // STALE-ACK GUARD (review N1) — same reasoning as `reconcile_owned_prediction`:
        // an ack above the highest seq we ever minted belongs to the vessel's PREVIOUS
        // owner, and latching it as `last_reconciled` disables this path permanently.
        let next_seq = world
            .resource::<lunco_core::OwnedInputLog>()
            .0
            .get(&gid)
            .map_or(0, |l| l.next_seq);
        if ack > next_seq {
            continue;
        }
        // One rollback per new ack.
        {
            let mut hist = world.resource_mut::<PredictedStateLog>();
            let vlog = hist.0.entry(gid).or_default();
            if ack <= vlog.last_reconciled {
                continue;
            }
            vlog.last_reconciled = ack;
        }

        // The assembly as WE predicted it at the acked seq — the rewind target.
        let Some(pred) = world
            .resource::<AssemblyHistory>()
            .0
            .get(&gid)
            .and_then(|ring| ring.iter().find(|s| s.seq == ack))
            .cloned()
        else {
            continue; // no recorded assembly for that seq (just promoted) — nothing to rewind
        };

        // Authoritative chassis state (f64 absolute position — gap A).
        let auth = RbState {
            pos: sample.pos_world,
            rot: sample.rot.as_dquat(),
            lv: sample.lv.as_dvec3(),
            av: sample.av.as_dvec3(),
        };

        // ── Divergence test: did the prediction actually miss? ──
        let dpos = (auth.pos - pred.chassis.pos).length();
        let drot = auth.rot.angle_between(pred.chassis.rot);
        let diverged = dpos > ROLLBACK_POS_EPS || drot > ROLLBACK_ROT_EPS;

        // Inputs we've sent that the host hasn't acked — the ones to re-simulate.
        let unacked: Vec<lunco_core::InputFrame> = world
            .resource::<lunco_core::OwnedInputLog>()
            .0
            .get(&gid)
            .map(|l| l.frames.iter().filter(|f| f.seq > ack).copied().collect())
            .unwrap_or_default();

        // SAFETY GATE: an articulated rover whose links we failed to gather must NEVER
        // be rolled back. Seating the chassis alone while its wheels stay put tears the
        // revolute joints apart and launches the vehicle — the exact catastrophic
        // failure observed live (`links=0`). Better to leave the body uncorrected
        // (a wobble) than to destroy it.
        let articulated = articulated_set.contains(&chassis);
        if diverged && articulated && pred.links.is_empty() {
            warn!(
                "[rollback] gid={gid}: articulated rover has NO recorded links — refusing to \
                 roll back (would tear the joints). Skipping correction."
            );
            continue;
        }

        if diverged {
            // ── RIGID RE-FRAME: move the WHOLE assembly onto authority ──
            // The wire carries only the chassis, so derive the correction as a rigid
            // transform of the assembly: chassis snaps exactly to authority, and every
            // link is carried with it, preserving joint/suspension/steer/spin state.
            let d_rot = auth.rot * pred.chassis.rot.inverse();
            let mut restore: Vec<(Entity, RbState)> = Vec::with_capacity(pred.links.len() + 1);
            restore.push((chassis, auth));
            for (link_e, link) in &pred.links {
                restore.push((
                    *link_e,
                    RbState {
                        pos: auth.pos + d_rot * (link.pos - pred.chassis.pos),
                        rot: d_rot * link.rot,
                        lv: auth.lv + d_rot * (link.lv - pred.chassis.lv),
                        av: auth.av + d_rot * (link.av - pred.chassis.av),
                    },
                ));
            }

            // Freeze the rest of the world: save every other non-static body and restore
            // it after the replay. Re-simulation must not advance bodies that already
            // live on their own authoritative timeline (proxies) or double-step props.
            // They still act as colliders at their current pose, so contacts stay real.
            let mut frozen: Vec<(Entity, RbState)> = Vec::new();
            {
                let assembly: HashSet<Entity> = restore.iter().map(|(e, _)| *e).collect();
                let mut q = world.query::<(
                    Entity,
                    &RigidBody,
                    &Position,
                    &Rotation,
                    &LinearVelocity,
                    &AngularVelocity,
                )>();
                for (e, rb, p, r, lv, av) in q.iter(world) {
                    if matches!(*rb, RigidBody::Static) || assembly.contains(&e) {
                        continue;
                    }
                    frozen.push((e, rb_state(p, r, lv, av)));
                }
            }

            let steps = unacked.len().min(MAX_REPLAY_STEPS);
            let ports = world.resource::<lunco_core::ports::PortRegistry>().clone();
            let saved_time = *world.resource::<Time>();

            world.resource_mut::<lunco_core::RollbackInProgress>().0 = true;
            apply_states(world, &restore);
            for input in unacked.iter().take(steps) {
                replay_one_tick(world, &ports, chassis, input);
            }
            // Put the frozen world back exactly as it was (they moved under gravity /
            // their own velocity during the replay steps).
            apply_states(world, &frozen);
            world.resource_mut::<lunco_core::RollbackInProgress>().0 = false;
            *world.resource_mut::<Time>() = saved_time;

            debug!(
                "[rollback] gid={gid} ack={ack} dpos={dpos:.3}m drot={drot:.3}rad replayed={steps} \
                 (unacked={}) links={} frozen={}",
                unacked.len(),
                pred.links.len(),
                frozen.len()
            );
            if unacked.len() > MAX_REPLAY_STEPS {
                warn!(
                    "[rollback] gid={gid}: {} unacked inputs exceeds cap {MAX_REPLAY_STEPS} — \
                     snapped without full replay (latency too high?)",
                    unacked.len()
                );
            }
        }

        // Prune what the ack has retired, whether or not we rolled back.
        if let Some(il) = world
            .resource_mut::<lunco_core::OwnedInputLog>()
            .0
            .get_mut(&gid)
        {
            while il.frames.front().is_some_and(|f| f.seq <= ack) {
                il.frames.pop_front();
            }
        }
        if let Some(ring) = world.resource_mut::<AssemblyHistory>().0.get_mut(&gid) {
            while ring.front().is_some_and(|s| s.seq < ack) {
                ring.pop_front();
            }
        }
    }
}

/// Seat a batch of bodies' public physics state (the only state a rollback may touch).
fn apply_states(world: &mut World, states: &[(Entity, RbState)]) {
    for (e, s) in states {
        let Ok(mut em) = world.get_entity_mut(*e) else {
            continue;
        };
        if let Some(mut p) = em.get_mut::<Position>() {
            p.0 = s.pos;
        }
        if let Some(mut r) = em.get_mut::<Rotation>() {
            r.0 = s.rot;
        }
        if let Some(mut lv) = em.get_mut::<LinearVelocity>() {
            lv.0 = s.lv;
        }
        if let Some(mut av) = em.get_mut::<AngularVelocity>() {
            av.0 = s.av;
        }
    }
}

/// Client predict-own reconciliation (input-replay model, D2). GENERAL over any
/// owned, locally-predicted moving body — it keys off [`lunco_core::OwnedLocally`]
/// + gid and corrects an arbitrary dynamic body's Transform/Position/velocity; it
/// assumes nothing about "rover". (Only the *input* that drives the body, e.g.
/// a `SetPorts` throttle/steer write, is domain-specific — the predict-and-reconcile substrate is not.)
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
    // Desync detection (review N3): every ack feeds the per-body gauge.
    mut divergence: ResMut<lunco_core::DivergenceStats>,
    q_owned: Query<&lunco_core::GlobalEntityId, With<lunco_core::OwnedLocally>>,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut Rotation>,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
        Option<&mut PendingCorrection>,
    )>,
    mut commands: Commands,
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
        // STALE-ACK GUARD (review N1). An ack can only be ours if we have actually
        // MINTED that seq. A snapshot still carrying the PREVIOUS owner's watermark
        // — in flight, or sitting in `InterpBuffers`, when we took possession —
        // would otherwise be latched below as `last_reconciled`; every ack from our
        // own stream (which restarts at 1) is then `<=` it, so this system
        // early-returns FOREVER and the rover we are driving is never reconciled
        // again. The host now resets the watermark on re-possession
        // (`sync_applied_seq_owners`); this is the client-side half, and it is what
        // covers the in-flight window between the two.
        let next_seq = input_log.0.get(&g).map_or(0, |l| l.next_seq);
        if ack > next_seq {
            continue;
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
        let Ok((mut tf, pos, rot, lin, ang, off)) = q.get_mut(e) else {
            continue;
        };

        // Compare prediction-at-the-acked-seq vs authority-at-that-seq — the
        // apples-to-apples test that cancels the latency lead, so a correct
        // prediction is left alone (no rubber-band). Only divergence corrects.
        let decision = lunco_core::reconcile_decision(
            hs.pos,
            hs.rot,
            tf.translation,
            tf.rotation,
            sample.pos,
            sample.rot,
            lunco_core::ReconcileParams::default(),
        );
        // DESYNC GAUGE (review N3). The error at the acked seq IS the prediction
        // error (the latency lead cancels), so this is the honest per-body
        // divergence — recorded on every ack, InSync included, so the gauge shows
        // the healthy baseline too. A sustained metre says so out loud: before this
        // there was no way to observe a desync in the field at all.
        let err_m = (sample.pos - hs.pos).length();
        if divergence.observe(g, lunco_core::PredictionKind::Owned, err_m) {
            warn!(
                "[desync] owned gid={g:x} diverging: {err_m:.2} m at ack seq={ack} for \
                 {} consecutive acks (max {:.2} m). The prediction is not tracking the host.",
                divergence.warn_streak,
                divergence.bodies.get(&g).map_or(err_m, |b| b.max_m),
            );
        }
        // COMMON CASE: prediction matched authority → leave the body alone.
        if matches!(decision, lunco_core::Reconciliation::InSync) {
            continue;
        }
        match decision {
            lunco_core::Reconciliation::InSync => unreachable!(),
            // Park the correction as a residual; `drain_pending_corrections`
            // applies it to physics `Position`/`Rotation` a few cm/degrees per
            // fixed tick, which avian writeback + transform-interpolation render
            // smoothly. Writing the pose (or `Transform`) here instead popped the
            // body AND reset `bevy_transform_interpolation`'s easing — the
            // hold-the-key jitter.
            lunco_core::Reconciliation::Correct {
                pos: new_pos,
                rot: new_rot,
            } => {
                let dpos = new_pos - tf.translation;
                let drot = (new_rot * tf.rotation.inverse()).normalize();
                match off {
                    Some(mut pc) => {
                        pc.pos += dpos;
                        pc.rot = (drot * pc.rot).normalize();
                    }
                    None => {
                        commands.entity(e).try_insert(PendingCorrection {
                            pos: dpos,
                            rot: drot,
                        });
                    }
                }
            }
            // Gross desync: teleport semantics — seat pose directly (Transform
            // included; the interpolation easing-reset on a real teleport is
            // exactly what we want) and drop any queued residual.
            lunco_core::Reconciliation::Snap {
                pos: new_pos,
                rot: new_rot,
            } => {
                // The force-rebaseline. It used to be SILENT; it is now counted and
                // announced (review N3) — a snapping owned body is the loudest
                // symptom the netcode has, and it was invisible in the field.
                divergence.note_rebaseline(g);
                warn!(
                    "[desync] owned gid={g:x} REBASELINED (snap {:.1} m to authority at ack \
                     seq={ack}) — prediction grossly desynced",
                    (sample.pos - tf.translation).length()
                );
                tf.translation = new_pos;
                tf.rotation = new_rot;
                // TODO(multiplayer): deferred — singleplayer focus for now, RBAC
                // disabled for ease of debugging. This seats absolute f64 `Position`
                // from a cell-relative f32 pose (the owned compare ignores `s.cell`)
                // — wrong at any non-zero cell (e.g. moonbase). Revisit before
                // multiplayer hardening (REVIEW-2026-07-19.md PRED-1).
                if let Some(mut p) = pos {
                    p.0 = DVec3::new(new_pos.x as f64, new_pos.y as f64, new_pos.z as f64);
                }
                if let Some(mut r) = rot {
                    r.0 = new_rot.as_dquat();
                }
                if let Some(mut pc) = off {
                    *pc = PendingCorrection::default();
                }
            }
        }
        // Blend velocity HALFWAY to authoritative (not a full seat): the sample's
        // velocity is ~a snapshot-period stale, so fully seating it while
        // accelerating yanks the rover's speed backward every correction — felt as
        // a rhythmic hiccup while simply holding the throttle. Half-blending damps
        // divergence just as effectively across a few acks without the kick.
        let auth_lv = sample.lv;
        let auth_av = sample.av;
        if let Some(mut l) = lin {
            let auth = DVec3::new(auth_lv.x as f64, auth_lv.y as f64, auth_lv.z as f64);
            l.0 = (l.0 + auth) * 0.5;
        }
        if let Some(mut a) = ang {
            let auth = DVec3::new(auth_av.x as f64, auth_av.y as f64, auth_av.z as f64);
            a.0 = (a.0 + auth) * 0.5;
        }
    }
}

/// Client Phase B (design in git history): mark **every replicated free dynamic
/// prop** (a ball / crate / cone — whether runtime-spawned OR authored scene
/// content) as [`lunco_core::ContactPredictable`] — *eligible* to become a
/// locally-`Dynamic` [`lunco_core::PredictedDynamic`] body, but only transiently,
/// while an owned body is shoving it (`promote_contacting_proxies`). Until then it
/// stays a kinematic snapshot proxy, perfectly synced to authority. This is the
/// fix for the old "predict every prop the moment it's seen" design, whose N
/// permanently-Dynamic bodies drifted then piled into chaos (see
/// `ContactPredictable`'s doc). Bump a prop and it still yields live in the same
/// contact — the eligibility just defers the `Dynamic` flip to the contact window.
///
/// The cosim guard is now [`lunco_core::NotPredictable`] ALONE — stamped on every
/// cosim-driven / server-only body by `tag_cosim_opaque` and the USD net policy
/// (balloons / `CosimTarget`, whose forces are server-only and not locally
/// computable). That marker was added precisely so the structural
/// `SkipContentStamp` guard wouldn't have to be the only thing (see
/// `NotPredictable`'s doc) — so we no longer restrict to runtime spawns, which
/// had frozen plain scene-content physics props server-only. Wheeled vehicles
/// (a rover root, identified by its `ActuatorPorts`) and the possessed rover (`OwnedLocally`)
/// are excluded — they have their own paths. A `Static` prop is left alone.
/// Client-only.
pub fn maintain_predicted_dynamic(
    role: Res<lunco_core::NetworkRole>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            // Wheeled vehicles have their own predict path
            // (`maintain_predicted_vehicles`); a cosim-flown vessel is caught by the
            // `NotPredictable` guard below.
            //
            // `ActuatorPorts` is the wheeled-vehicle discriminator: only a rover root
            // (`PhysxVehicleContextAPI`) carries an actuator index. A lander/avatar has
            // no hardware actuator ports and so is not excluded here. (`DriveMix` would
            // select the same set, but lives in `lunco-mobility`, which this crate does
            // not depend on — and should not, for one query filter.)
            Without<lunco_core::ActuatorPorts>,
            Without<lunco_core::OwnedLocally>,
            // Stamp the eligibility marker at most once (a promoted body carries
            // both `ContactPredictable` and `PredictedDynamic`).
            Without<lunco_core::ContactPredictable>,
            // The cosim/server-only guard: a cosim-driven body (Modelica balloon,
            // CosimTarget, …) has forces we can't reproduce locally, so it must
            // never be predicted — it stays a kinematic, snapshot-driven proxy.
            // This is now the SOLE membership guard (the old `SkipContentStamp`
            // runtime-spawn restriction is dropped so authored scene props run
            // live physics too).
            Without<lunco_core::NotPredictable>,
        ),
    >,
    // If this peer later POSSESSES the prop, the owned (input-replay) path takes
    // over — drop BOTH the eligibility marker and any live promotion so neither the
    // contact-gate nor the free-body reconciler acts on it.
    q_demote: Query<
        Entity,
        (
            With<lunco_core::ContactPredictable>,
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
        // Mark eligible, but leave it a kinematic proxy: `promote_contacting_proxies`
        // flips it `Dynamic` only while an owned body is touching it.
        commands
            .entity(e)
            .try_insert(lunco_core::ContactPredictable);
    }
    for e in q_demote.iter() {
        commands.entity(e).remove::<(
            lunco_core::ContactPredictable,
            lunco_core::PredictedDynamic,
            ContactPredictLinger,
        )>();
    }
}

/// Client Step 4 (`PREDICT_AND_SMOOTH` §5): mark every **remote raycast rover**
/// (a rover you don't possess and don't own) as
/// [`lunco_core::ContactPredictable`] — *eligible* for the same transient
/// promotion as a free prop, so it **yields** the moment your owned rover shoves
/// it, then re-syncs.
///
/// Why not immovable proxies: with Step 1 alone a remote rover is a permanent
/// *kinematic* proxy. It can push your owned rover (its velocity enters contact)
/// but never yields to *being* pushed — you'd bounce off an immovable wall while
/// authority shows it moving away. Why not permanently `Dynamic` (the old Step 4):
/// N non-owned Dynamic rovers all free-running local physics against a stale curve
/// drifted then piled into chaos. The fix is the middle path — Dynamic *only while
/// you're touching it* (`promote_contacting_proxies`), one pusher at a time.
///
/// The contact-gate reuses `PredictedDynamic` for the live-promotion state (not a
/// separate marker): every predict-own seam already excludes it (kinematic pin /
/// drive / interpolate), and [`maintain_predicted_dynamic`]'s possession-demote
/// clears both markers when you possess the rover (its input-replay path takes
/// over). Cosim-flown vessels are safe — `tag_cosim_opaque` marks cosim-driven
/// bodies `NotPredictable`, excluded here. Articulated rovers are excluded too
/// (they flip if made single-body Dynamic) and stay pure kinematic proxies.
/// Client-only.
pub fn maintain_predicted_vehicles(
    role: Res<lunco_core::NetworkRole>,
    local: Res<lunco_core::LocalSession>,
    reg: Res<lunco_core::SessionRegistry>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &lunco_core::GlobalEntityId, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            // A wheeled vehicle = a rover root, which is exactly what carries an
            // `ActuatorPorts` actuator index. The `Without<NotPredictable>` guard below
            // additionally excludes cosim-flown vessels, so this resolves to exactly the
            // locally-simulated rovers. (A lander no longer even reaches this filter: it
            // has no actuator ports of its own.)
            With<lunco_core::ActuatorPorts>,
            Without<lunco_core::OwnedLocally>,
            // Stamp eligibility at most once (a promoted rover carries both).
            Without<lunco_core::ContactPredictable>,
            Without<lunco_core::NotPredictable>,
            // Articulated (Physical/joint) rovers must NOT be single-body
            // predicted: only the chassis is replicated, so making it Dynamic +
            // reconciling its pose each snapshot while the jointed wheels run
            // free injects joint energy → flip. They stay kinematic proxies
            // (chassis pose forced by snapshots, cannot flip), so they never
            // become contact-eligible. Raycast rovers are single bodies and
            // yield fine when a shove promotes them.
            Without<lunco_core::ArticulatedVehicle>,
        ),
    >,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, gid, rb) in q_add.iter() {
        if matches!(*rb, RigidBody::Static) {
            continue;
        }
        // NEVER contact-predict a rover THIS session owns: its prediction
        // membership belongs exclusively to Phase A (`maintain_owned_locally`,
        // OwnedLocally + input-replay). Phase A's drive-grace lapses between key
        // taps; without this guard the rover flapped OwnedLocally→PredictedDynamic
        // on every lapse, and the state-reconciler yanked the still-moving rover
        // back to a ~0.2 s-stale snapshot each time — a tap-driven sawtooth jitter
        // with no contact at all. An owned-but-idle rover falls back to the
        // kinematic proxy path (computability rule, Phase A), not to this marker.
        if reg.owns(local.0, gid.get()) {
            continue;
        }
        // Eligible only: stays a kinematic proxy until an owned body shoves it,
        // at which point `promote_contacting_proxies` flips it `Dynamic` to yield.
        commands
            .entity(e)
            .try_insert(lunco_core::ContactPredictable);
    }
}

/// Linger window (s) a contact-promoted body stays `Dynamic` after the last tick an
/// owned body was touching it. Contacts chatter — a rolling/bouncing shove makes
/// and breaks the manifold — so demoting the instant a touch drops would flip-flop
/// Kinematic↔Dynamic mid-bump. Holding it Dynamic briefly keeps the yield smooth,
/// then it demotes and re-syncs to authority.
const CONTACT_PREDICT_LINGER: f32 = 0.30;

/// Per-body countdown (seconds remaining) keeping a contact-promoted proxy
/// `Dynamic`. Re-armed to [`CONTACT_PREDICT_LINGER`] every tick an owned body is
/// touching it; drained otherwise. Removed together with `PredictedDynamic` on
/// demotion. Present only on client, only during a shove.
#[derive(Component, Clone, Copy, Debug)]
pub struct ContactPredictLinger(f32);

/// The contact-gate that makes the hybrid work (see [`lunco_core::ContactPredictable`]):
/// promote a `ContactPredictable` kinematic proxy to a locally-`Dynamic`
/// [`lunco_core::PredictedDynamic`] body **only while an [`lunco_core::OwnedLocally`]
/// body is touching it** (plus [`CONTACT_PREDICT_LINGER`]), then demote it back.
///
/// Non-owned bodies otherwise stay perfectly-synced kinematic proxies; the ONLY
/// interval one runs local dynamics is the brief window your owned rover is shoving
/// it, against exactly one pusher — so it yields crisply without the N-body free-run
/// that produced the old drift-then-chaos. On demotion the body loses
/// `PredictedDynamic`, so `force_kinematic_proxies` re-pins it `Kinematic` and
/// `drive_kinematic_proxies` re-seats it on the authoritative curve next frame.
///
/// Contact is read from avian's `Collisions` graph via the **rigid-body** entities
/// (`ContactPair::body{1,2}`), so it is robust to colliders living on child entities
/// (compound/wheel colliders). Only `OwnedLocally` bodies act as pushers, which
/// bounds promotion to one body at a time — a promoted body cannot cascade-promote a
/// pile. Registered in `Update` **before** `force_kinematic_proxies` reads the
/// marker; the chain's auto-inserted sync point applies the promote/demote command
/// before the kinematic-pin pass runs, so a promoted body is skipped by the pin the
/// same frame and a demoted one is re-pinned the same frame. Client-only.
pub fn promote_contacting_proxies(
    role: Res<lunco_core::NetworkRole>,
    time: Res<Time>,
    collisions: Collisions,
    q_owned: Query<(), With<lunco_core::OwnedLocally>>,
    q_eligible: Query<(), With<lunco_core::ContactPredictable>>,
    // Bodies currently promoted (Dynamic) that carry the linger countdown.
    mut q_promoted: Query<(Entity, &mut ContactPredictLinger), With<lunco_core::PredictedDynamic>>,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    // Which eligible proxies is an owned body touching this tick?
    let mut touched: HashSet<Entity> = HashSet::new();
    for pair in collisions.iter() {
        if !pair.is_touching() {
            continue;
        }
        // `body{1,2}` are the rigid-body entities behind each collider (None for a
        // colliderless static). `OwnedLocally` / `ContactPredictable` live on those
        // body entities, so match against them, not the collider entities.
        let (Some(b1), Some(b2)) = (pair.body1, pair.body2) else {
            continue;
        };
        let proxy = if q_owned.contains(b1) && q_eligible.contains(b2) {
            b2
        } else if q_owned.contains(b2) && q_eligible.contains(b1) {
            b1
        } else {
            continue; // not an owned↔eligible pair
        };
        touched.insert(proxy);
    }

    // Age already-promoted bodies: re-arm the linger if still shoved, else drain it
    // and demote when the window closes. Consume (`remove`) the touched entries here
    // so whatever remains in `touched` is a fresh promotion handled below — this also
    // avoids re-inserting `RigidBody::Dynamic` every tick (which would churn avian's
    // change detection) on a body that's already Dynamic.
    let dt = time.delta_secs();
    for (e, mut linger) in q_promoted.iter_mut() {
        if touched.remove(&e) {
            linger.0 = CONTACT_PREDICT_LINGER; // still shoved — re-arm
        } else {
            linger.0 -= dt;
            if linger.0 <= 0.0 {
                // Hand the body back to the kinematic proxy path.
                // `force_kinematic_proxies` (later in this chain) re-pins it
                // `Kinematic`; `drive_kinematic_proxies` re-seats it on the
                // authoritative curve (snapping if it drifted > 2 m).
                commands
                    .entity(e)
                    .remove::<(lunco_core::PredictedDynamic, ContactPredictLinger)>();
            }
        }
    }

    // Fresh promotions: eligible proxies newly shoved this tick (not already in
    // `q_promoted`). Inserting `PredictedDynamic` excludes the body from every
    // kinematic-proxy seam so it free-runs local physics and yields to the shove;
    // `reconcile_predicted_dynamic` keeps it from drifting past authority meanwhile.
    for e in touched {
        commands.entity(e).try_insert((
            lunco_core::PredictedDynamic,
            RigidBody::Dynamic,
            ContactPredictLinger(CONTACT_PREDICT_LINGER),
        ));
    }
}

/// Client Phase B: **state-based** reconciliation for [`lunco_core::PredictedDynamic`]
/// bodies (free props + remote rovers). Unlike the owned rover there is NO input
/// `seq` to replay, so we pull the body's CURRENT pose toward the authoritative
/// curve directly.
///
/// CONTINUOUS reconcile (revised 2026-06-26): runs EVERY fixed tick, in ABSOLUTE
/// WORLD space, against the same delayed `sample_curve` target the kinematic proxies
/// use (`pos_world`, f64). The previous design reconciled once per 20 Hz snapshot in
/// f32 render space, which (a) let a free Dynamic body re-settle/tip on terrain
/// between snapshots faster than the bounded correction could cancel → drift, and
/// (b) seated `Position` from a cell-relative render value on the teleport path →
/// non-origin-cell bodies collapsed toward the world origin (the pile-up). Working in
/// absolute world fixes both. Pose is held by a soft spring fed through
/// `PendingCorrection` (drained a bounded bit per tick — never a direct `Transform`
/// write); velocity is left to LOCAL physics except on a gross teleport, so contacts
/// and your push stay crisp. The `RECONCILE_EPS_*` dead-zone is the yield budget.
/// `FixedPostUpdate` after avian writeback; no-op on host/standalone.
/// Beyond this absolute-world position error (m) a predicted body has grossly
/// desynced (first sight / respawn / long stall) → hard-teleport to authority.
const RECONCILE_SNAP_DIST: f64 = 2.0;
/// Dead-zone (m / rad ≈5.7°): below this the body is left to local physics — this
/// tolerance IS the yield budget that lets a contact/push deviate the body crisply
/// without the spring fighting it. Tune up if collisions feel mushy, down if drift.
const RECONCILE_EPS_POS: f64 = 0.40;
const RECONCILE_EPS_ROT: f32 = 0.10;

pub fn reconcile_predicted_dynamic(
    role: Res<lunco_core::NetworkRole>,
    status: Res<lunco_core::NetStatus>,
    buffers: Res<InterpBuffers>,
    clock: Res<ProxyPlaybackClock>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    // Desync detection (review N3): free predicted bodies feed the same gauge as the
    // owned rover, so a drifting prop is observable instead of silently teleporting.
    mut divergence: ResMut<lunco_core::DivergenceStats>,
    q_pred: Query<&lunco_core::GlobalEntityId, With<lunco_core::PredictedDynamic>>,
    mut q: Query<(
        Option<&mut Position>,
        Option<&mut Rotation>,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
        Option<&mut PendingCorrection>,
    )>,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    if !status.connected {
        return; // host lost — freeze, don't chase a stale curve
    }
    // Read (don't advance) the shared playback clock that `drive_kinematic_proxies`
    // already stepped this tick: predicted bodies track the SAME delayed authoritative
    // curve as the kinematic proxies, so the two never disagree on where authority is.
    let render_t = clock.t;
    for gid in q_pred.iter() {
        let g = gid.get();
        let Some(buf) = buffers.0.get(&g) else {
            continue;
        };
        if buf.is_empty() {
            continue;
        }
        let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(g)) else {
            continue;
        };
        let Ok((pos, rot, lin, ang, off)) = q.get_mut(e) else {
            continue;
        };
        // Dynamic bodies always carry avian `Position`/`Rotation` (absolute, f64).
        let (Some(mut pos), Some(mut rot)) = (pos, rot) else {
            continue;
        };
        // Authoritative target in ABSOLUTE WORLD (`pos_world`), via the same cubic
        // curve the kinematic proxies follow. This is the fix for the earlier pile-up:
        // the old code compared/seated in f32 render space (cell-relative), which
        // collapsed non-origin-cell bodies toward the world origin. Everything here is
        // absolute, so each body converges to its OWN pose.
        let Some((here, here_rot, lv, av)) = sample_curve(buf, render_t) else {
            continue;
        };

        let err = here - pos.0; // DVec3, absolute world
        let dist = err.length();
        let cur_rot = rot.0.as_quat();
        let mut rot_err = (here_rot * cur_rot.inverse()).normalize();
        if rot_err.w < 0.0 {
            rot_err = -rot_err; // shortest arc
        }
        let angle = rot_err.to_axis_angle().1.abs();

        // DESYNC GAUGE (review N3) — same signal as the owned body, for the free
        // predicted set (props, bumped rocks, contact-gated remote rovers).
        if divergence.observe(g, lunco_core::PredictionKind::Free, dist as f32) {
            warn!(
                "[desync] free predicted gid={g:x} diverging: {dist:.2} m from authority for {} \
                 consecutive ticks — local physics is not reproducing the host",
                divergence.warn_streak,
            );
        }

        if dist > RECONCILE_SNAP_DIST {
            // Counted + announced: this teleport was silent before (review N3).
            divergence.note_rebaseline(g);
            debug!("[desync] free predicted gid={g:x} REBASELINED (teleport {dist:.1} m)");
            // Gross desync / first sight: teleport. Seat Position/Rotation directly
            // (NEVER `Transform` — avian writeback derives it; a Transform write here
            // resets `bevy_transform_interpolation` → the historical jitter) and seat
            // velocity so it stops diverging. Closing a >2 m gap with velocity would
            // be a violent kick into anything in contact.
            pos.0 = here;
            rot.0 = here_rot.as_dquat();
            if let Some(mut l) = lin {
                l.0 = lv;
            }
            if let Some(mut a) = ang {
                a.0 = av;
            }
            if let Some(mut pc) = off {
                *pc = PendingCorrection::default();
            }
        } else if dist > RECONCILE_EPS_POS || angle > RECONCILE_EPS_ROT {
            // OUT of the in-sync dead-zone but not far enough to snap.
            //
            // Feed-forward authoritative velocity to ALL predicted bodies here so
            // they dead-reckon smoothly between snapshots instead of sitting still
            // (0 local velocity) and drifting until they cross RECONCILE_SNAP_DIST
            // and teleport. This applies to free props (host-launched debris, a
            // ball mid-flight) just as much as to driven rovers — gating it on
            // `is_rover` left non-rover bodies stationary and snapping. A
            // host-moved prop leaves the dead-zone immediately, so it reaches this
            // branch and tracks authority velocity.
            if let Some(mut l) = lin {
                l.0 = lv;
            }
            if let Some(mut a) = ang {
                a.0 = av;
            }

            // Soft CONTINUOUS spring (every fixed tick): SET the residual to the
            // freshly-measured error; `drain_pending_corrections` eases a bounded bit
            // per tick into Position/Rotation (smooth, never a Transform write).
            let dpos = err.as_vec3();
            match off {
                Some(mut pc) => {
                    pc.pos = dpos;
                    pc.rot = rot_err;
                }
                None => {
                    commands.entity(e).try_insert(PendingCorrection {
                        pos: dpos,
                        rot: rot_err,
                    });
                }
            }
        }
        // else: within tolerance — leave the body entirely to local physics; any
        // residual `PendingCorrection` finishes draining and removes itself.
    }
}

/// Step 2 (revised): the residual reconcile correction, drained in **physics
/// space** a tick at a time by [`drain_pending_corrections`].
///
/// The TYPE now lives in `lunco_core::session` (review A6 — it and `SpawnEntity`
/// were the only two symbols `lunco-networking` needed from this 13.4k-LOC crate,
/// and that one edge dragged the whole editor closure into every networking build).
/// The producer (`reconcile_owned_prediction`) and the drain stay here; the
/// rationale for parking a correction instead of writing `Transform` is on the type.
/// Re-exported so existing call sites are unchanged.
pub use lunco_core::PendingCorrection;

/// Time-constant (s) for draining a pending correction: ~63% applied per
/// `CORRECTION_TAU`, ≈ fully in ~3×. Long enough to be invisible, short enough to
/// converge well before the next ack lands.
const CORRECTION_TAU: f64 = 0.12;

/// Per-tick cap on the drained position nudge (m). 2.5 cm at 64 Hz = up to
/// 1.6 m/s of correction capacity — far above the measured ~0.15 m/s divergence —
/// while each individual nudge stays far too small to disturb a contact.
const CORRECTION_MAX_POS_PER_TICK: f64 = 0.025;

/// Per-tick cap on the drained rotation nudge (rad, ~0.9°/tick ≈ 57°/s capacity).
const CORRECTION_MAX_ROT_PER_TICK: f64 = 0.016;

// `PendingCorrection::is_negligible` moved to `lunco_core::session` with the type
// (review A6) — an inherent impl must live in the type's own crate.

/// Drain each body's [`PendingCorrection`] into its avian `Position`/`Rotation`
/// in small per-tick steps (exp toward zero residual, hard per-tick caps).
/// `FixedUpdate` — the nudge flows through this tick's solve + writeback, so
/// `bevy_transform_interpolation` eases it at render rate like any other motion.
pub fn drain_pending_corrections(
    mut commands: Commands,
    mut q: Query<(Entity, &mut Position, &mut Rotation, &mut PendingCorrection)>,
) {
    let frac = 1.0 - (-SECS_PER_TICK / CORRECTION_TAU).exp(); // per-tick drain fraction
    for (e, mut pos, mut rot, mut pc) in q.iter_mut() {
        if pc.is_negligible() {
            commands.entity(e).remove::<PendingCorrection>();
            continue;
        }
        // Position: take `frac` of the residual, capped.
        let mut step = pc.pos.as_dvec3() * frac;
        let len = step.length();
        if len > CORRECTION_MAX_POS_PER_TICK {
            step *= CORRECTION_MAX_POS_PER_TICK / len;
        }
        pos.0 += step;
        pc.pos -= step.as_vec3();

        // Rotation: slerp a capped fraction of the residual toward identity.
        let angle = pc.rot.angle_between(Quat::IDENTITY) as f64;
        if angle > 1e-5 {
            let take = (frac * angle).min(CORRECTION_MAX_ROT_PER_TICK) / angle; // fraction of residual
            let applied = Quat::IDENTITY.slerp(pc.rot, take as f32);
            rot.0 = applied.as_dquat() * rot.0;
            pc.rot = (applied.inverse() * pc.rot).normalize();
        } else {
            pc.rot = Quat::IDENTITY;
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
/// The membership DECISION is declarative, derived from USD at load
/// (`lunco-usd-sim`'s `process_usd_sim_prims` → `derive`/`net_override_markers`):
/// structural markers (`ArticulatedVehicle`/`ArticulatedLink`) and any opt-out
/// (`NetExcluded`) / opacity (`NotPredictable`) come from the USD joint graph +
/// `lunco:net:*` attributes. This system only **applies the default**: every
/// non-static rigid body replicates unless USD excluded it. See
/// `crates/lunco-networking/USD_REPLICATION_POLICY.md`.
///
/// Why a re-asserting Update pass and not a one-shot at load: the avian `RigidBody`
/// component materialises a frame or more AFTER the USD prim entity exists (the
/// rover's cosim/flight-software re-inserts a `Dynamic` body after the asset loads —
/// see the `force_kinematic_proxies` note). Keying on the live `RigidBody` here, each
/// frame, catches it whenever it lands; `Without<NetReplicate>` makes the steady
/// state a no-op.
///
/// Excludes:
/// - **static** colliders (the ground) — never move;
/// - **runtime spawns** (`SkipContentStamp`) — already tagged at spawn time;
/// - **USD opt-outs** (`NetExcluded`) — `lunco:net:replicate = false` / `authority = "local"`;
/// - **articulated links** (`ArticulatedLink`, i.e. rover wheels) — NOT replicated; the
///   client reconstructs each wheel's pose from its chassis (rigid axle ⇒ fixed mount +
///   derived steer + cosmetic spin), saving ~4 wheel poses/tick/rover. See
///   `lunco-usd-sim::reconstruct_proxy_wheels` and USD_REPLICATION_POLICY.md. (Full
///   per-link replication remains available as a future USD opt-in.)
pub fn apply_net_replication(
    mut commands: Commands,
    q_candidates: Query<
        (Entity, &RigidBody),
        (
            With<lunco_core::GlobalEntityId>,
            Without<lunco_core::NetReplicate>,
            Without<lunco_core::NetExcluded>,
            Without<lunco_core::ArticulatedLink>,
            Without<lunco_core::SkipContentStamp>,
        ),
    >,
) {
    for (e, rb) in q_candidates.iter() {
        if matches!(*rb, RigidBody::Static) {
            continue;
        }
        commands.entity(e).try_insert(lunco_core::NetReplicate);
    }
}

/// Plugin that owns the client-netcode half of what `SpawnCommandPlugin` used to
/// register: snapshot ingest + interpolation, kinematic proxy driving, owned-rover
/// prediction / reconciliation / rollback, and correction smoothing.
///
/// `SpawnCommandPlugin` (lunco-sandbox-edit) keeps `apply_replicated_spawns`, the
/// first system of the old `Update` chain, because it spawns from the editor's
/// catalog. The chain's relative order survives the split via
/// [`lunco_core::NetcodeSet`]: sandbox-edit puts its system in
/// `NetcodeSet::InstantiateSpawns`, and everything here runs in `NetcodeSet::Predict`,
/// configured `.after()` it below. The internal order of the rest of the chain is
/// unchanged.
pub struct NetcodePredictionPlugin;

impl Plugin for NetcodePredictionPlugin {
    fn build(&self, app: &mut App) {
        // `SetVisualLead` — the one `#[Command]` this module owns. Registered the
        // same way every other command in this crate is (`register_commands!` →
        // `register_all_commands`), so the type lands in the registry alongside its
        // observer and stays reachable from the HTTP API / rhai / `discover_schema`.
        register_all_commands(app);
        // Render-lead visual prediction: live tunables + the per-gid eased
        // offsets they drive. Resources only — `SetVisualLead` itself is a
        // `#[Command]` like any other and comes in via `register_all_commands`.
        app.init_resource::<VisualLeadSettings>();
        app.init_resource::<VisualLeadState>();
        app.init_resource::<InterpBuffers>();
        app.init_resource::<PredictedStateLog>();
        app.init_resource::<ProxyPlaybackClock>();
        // The netcode `Update` pipeline now spans two crates: `apply_replicated_spawns`
        // (lunco-sandbox-edit) is the chain's first system and stays there, so the
        // ordering it used to get from `.chain()` is expressed as a set relation.
        app.configure_sets(
            Update,
            lunco_core::NetcodeSet::Predict.after(lunco_core::NetcodeSet::InstantiateSpawns),
        );
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
                maintain_owned_locally,
                // Mirror the chassis's `OwnedLocally` onto owned-rover wheels so
                // the rover you drive runs local physics on all links (not frozen
                // kinematic proxies). After `maintain_owned_locally` so it reads
                // the freshly-set chassis marker; before the kinematic-pin systems.
                propagate_owned_to_wheels,
                // Phase B: classify free predicted props BEFORE the interpolate /
                // kinematic-pin systems read the `PredictedDynamic` marker.
                maintain_predicted_dynamic,
                // Step 4: mark remote raycast rovers contact-eligible (like props),
                // so they yield to your push. After maintain_predicted_dynamic so the
                // possession-demote ordering is stable.
                maintain_predicted_vehicles,
                // Contact-gate: promote an eligible proxy to Dynamic only while an
                // owned body shoves it, demote it back otherwise. MUST run before
                // force_kinematic_proxies so the chain's sync point applies the
                // promote/demote before the kinematic-pin pass reads the marker.
                promote_contacting_proxies,
                ingest_snapshots,
                interpolate_proxies,
                force_kinematic_proxies,
                apply_net_replication,
                // L1: drop the interp ring of any proxy despawned this frame so the
                // map doesn't leak per gid and a re-entering gid can't replay stale
                // pre-exit samples. RemovedComponents-driven, order-independent.
                prune_interp_buffers_on_despawn,
            )
                .chain()
                .in_set(lunco_core::NetcodeSet::Predict),
        );
        // Step 1: velocity-drive kinematic RigidBody proxies toward the snapshot
        // curve in `FixedUpdate`, so it runs BEFORE avian's solver step
        // (`FixedPostUpdate`) and the commanded velocity enters this tick's contact
        // resolution. Reads `InterpBuffers` (filled by `ingest_snapshots` in the
        // prior frame's `Update` — one-frame latency, absorbed by `INTERP_DELAY`)
        // and is the sole advance site for `ProxyPlaybackClock`. No-op on
        // host/standalone (guards on `NetworkRole::Client`).
        app.add_systems(
            FixedUpdate,
            drive_kinematic_proxies.run_if(lunco_core::not_rolling_back),
        );
        // HOST: apply one buffered client input per fixed tick BEFORE the drive
        // reads the ports, so the host steps the client's input sequence in lockstep
        // (the divergence fix behind proper prediction+reconciliation).
        app.add_systems(FixedFirst, apply_buffered_client_inputs);
        // Visual prediction (`LUNCO_VISUAL_PREDICT=1`): lead the owned rover's
        // RENDERED pose in `Last` — after ALL transform propagation (incl.
        // big_space), before render extraction — so physics stays authoritative
        // while the visual anticipates. No-op unless the mode is on.
        app.add_systems(Last, lead_owned_rover_render);
        // Input-replay reconciliation (D2), in LOCKSTEP with physics —
        // `FixedPostUpdate` AFTER avian's writeback. `reconcile_owned_prediction` folds
        // in the authoritative ack (no-op in the common case → no rubber-band),
        // then `record_predicted_state` records this tick's pose keyed by the input
        // seq, so the NEXT ack can be compared apples-to-apples. Order matters:
        // reconcile first (may correct), then record the resulting pose.
        // `reconcile_owned_prediction` is the BLEND corrector. Under `LUNCO_ROLLBACK=1`
        // it is replaced wholesale by `rollback_owned_prediction` — running both would
        // have them fight over the same body (the blend nudging a trajectory that
        // rollback has already re-derived exactly).
        app.add_systems(
            FixedPostUpdate,
            (
                reconcile_owned_prediction.run_if(|| !rollback_enabled()),
                record_predicted_state,
                // Rollback's rewind target: the full assembly (chassis + every wheel),
                // because the wire replicates the chassis only.
                record_assembly_state,
            )
                .chain()
                .after(PhysicsSystems::Writeback)
                .run_if(lunco_core::not_rolling_back),
        );
        // Phase B: state-based reconcile for free predicted props (no input seq),
        // likewise after avian writeback. Independent of the owned-rover chain
        // above (acts on a disjoint set of bodies).
        app.add_systems(
            FixedPostUpdate,
            reconcile_predicted_dynamic
                .after(PhysicsSystems::Writeback)
                .run_if(lunco_core::not_rolling_back),
        );
        // Deterministic rollback. `Update`, after `ingest_snapshots` has landed the
        // freshest ack — and necessarily OUTSIDE the fixed loop, since it runs
        // schedules (`RollbackReplay` + `PhysicsSchedule`) itself, which is impossible
        // re-entrantly from within `FixedMain`. No-op unless `LUNCO_ROLLBACK=1`.
        app.init_resource::<AssemblyHistory>();
        app.add_systems(Update, rollback_owned_prediction.after(ingest_snapshots));
        // Step 2 (revised) correction smoothing: reconcilers PARK their correction
        // in `PendingCorrection`; this drain applies it to the physics pose a few
        // cm/deg per fixed tick, BEFORE the solve, so writeback + avian's
        // transform-interpolation render it smoothly. Game code never writes
        // `Transform` (which resets `bevy_transform_interpolation`'s easing — the
        // cause of the hold-the-key client jitter).
        app.add_systems(
            FixedUpdate,
            drain_pending_corrections.run_if(lunco_core::not_rolling_back),
        );
    }
}

// Generates `register_all_commands(app)` — every `#[Command]` this module owns,
// each wired type + observer together.
register_commands!(on_set_visual_lead);

#[cfg(test)]
mod tests {
    use super::{predicts_locally, PREDICT_GRACE_TICKS};

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
        assert!(predicts_locally(
            true,
            1_000,
            1_000 + PREDICT_GRACE_TICKS,
            PREDICT_GRACE_TICKS
        ));
    }

    #[test]
    fn owned_idle_past_grace_falls_back_to_interpolation() {
        // One tick past the grace window → demote to proxy/interpolation.
        assert!(!predicts_locally(
            true,
            1_000,
            1_001 + PREDICT_GRACE_TICKS,
            PREDICT_GRACE_TICKS
        ));
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

    /// Reconcile (owned rover): a Correct-class divergence must NOT pop the pose —
    /// it parks a [`PendingCorrection`] residual, and `drain_pending_corrections`
    /// then moves avian `Rotation` (the physics truth — interpolation re-derives
    /// `Transform` from it) toward authority in small per-tick steps. Direct
    /// `Transform` writes are forbidden: `bevy_transform_interpolation`
    /// (`interpolate_all()`) treats them as teleports and resets its easing,
    /// which was the hold-the-key client jitter.
    #[test]
    fn reconcile_correction_writes_avian_rotation() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.init_resource::<PredictedStateLog>();
        world.init_resource::<lunco_core::OwnedInputLog>();
        world.init_resource::<lunco_core::DivergenceStats>();

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

        // This client really did emit input seq 1 — the stale-ack guard (review N1)
        // only accepts an ack it could have produced.
        world
            .resource_mut::<lunco_core::OwnedInputLog>()
            .0
            .entry(gid)
            .or_default()
            .next_seq = 1;
        // We predicted `predicted` at input seq 1…
        world
            .resource_mut::<PredictedStateLog>()
            .0
            .entry(gid)
            .or_default()
            .ring
            .push_back(PredictedState {
                seq: 1,
                pos: Vec3::ZERO,
                rot: predicted,
            });
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

        // The reconciler must NOT have popped the pose (no direct writes)…
        let tf_rot = world.entity(e).get::<Transform>().unwrap().rotation;
        assert!(
            tf_rot.angle_between(predicted) < 1e-6,
            "Correct-class divergence must not pop Transform; got {tf_rot:?}"
        );
        // …instead it parked a rotation residual…
        let pc = world
            .entity(e)
            .get::<PendingCorrection>()
            .copied()
            .expect("reconcile should park a PendingCorrection");
        assert!(
            pc.rot.angle_between(Quat::IDENTITY) > 1e-3,
            "pending correction should carry the rotation error; got {pc:?}"
        );
        // …which the drain converges onto avian `Rotation` (physics truth) in
        // small per-tick steps, never exceeding the per-tick cap.
        let before = world.entity(e).get::<Rotation>().unwrap().0.as_quat();
        world.run_system_once(drain_pending_corrections).unwrap();
        let after = world.entity(e).get::<Rotation>().unwrap().0.as_quat();
        let step = after.angle_between(before);
        assert!(
            step > 1e-4,
            "drain should rotate avian Rotation toward authority; step={step}"
        );
        assert!(
            step <= CORRECTION_MAX_ROT_PER_TICK as f32 + 1e-4,
            "per-tick rotation nudge must respect the cap; step={step}"
        );
        // Draining repeatedly converges (residual shrinks monotonically).
        for _ in 0..600 {
            world.run_system_once(drain_pending_corrections).unwrap();
        }
        let settled = world.entity(e).get::<Rotation>().unwrap().0.as_quat();
        let target = predicted.slerp(authoritative, 0.3); // blend=0.3 nudge target
        assert!(
            settled.angle_between(target) < 0.01,
            "drained Rotation should reach the blended correction target; \
             got {settled:?} vs {target:?}"
        );
    }

    /// Proxy interpolation must likewise write avian `Rotation`.
    #[test]
    fn interpolate_proxy_writes_avian_rotation() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        // Clock is now an external resource (advanced in FixedUpdate by
        // `drive_kinematic_proxies`); seat it at the render instant directly. For
        // the single sample at gen_t 0, render_t = −INTERP_DELAY ⇒ snap-to-oldest.
        world.insert_resource(ProxyPlaybackClock {
            t: -INTERP_DELAY,
            init: true,
        });

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
        // newest_gen 0.50 − INTERP_DELAY 0.18 = 0.32 render instant (the clock is
        // external now; seat it where the old self-advancing clock would have eased
        // to on first sight).
        world.insert_resource(ProxyPlaybackClock {
            t: 0.32,
            init: true,
        });

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
            world
                .resource_mut::<IncomingSnapshots>()
                .0
                .push(SnapshotSample {
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
            world
                .resource::<InterpBuffers>()
                .0
                .get(&gid)
                .map(|b| b.len()),
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
        world.init_resource::<lunco_core::DivergenceStats>();
        // reconcile only runs as a connected Client; the clock is the render instant.
        world.insert_resource(lunco_core::NetworkRole::Client);
        world.insert_resource(lunco_core::NetStatus {
            connected: true,
            ..Default::default()
        });
        world.insert_resource(ProxyPlaybackClock { t: 0.5, init: true });

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

        // The snap branch seats avian `Position` (the physics truth — never
        // `Transform`, which writeback derives) and the authoritative velocity.
        let p = world.entity(e).get::<Position>().unwrap().0;
        let v = world.entity(e).get::<LinearVelocity>().unwrap().0;
        assert!(
            (p.x - 50.0).abs() < 1e-4,
            "should snap to authority, got {p:?}"
        );
        assert!(
            (v.x - 2.0).abs() < 1e-4,
            "velocity must be seated to authority, got {v:?}"
        );
    }

    /// Phase B: when a `PredictedDynamic` prop is already at authority (InSync), the
    /// reconcile leaves it COMPLETELY alone — no pose change and, crucially, NO
    /// velocity seating — so its local physics keeps running crisply between
    /// snapshots instead of being clamped to the authoritative velocity each frame.
    #[test]
    fn predicted_dynamic_in_sync_is_left_untouched() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.init_resource::<lunco_core::DivergenceStats>();
        world.insert_resource(lunco_core::NetworkRole::Client);
        world.insert_resource(lunco_core::NetStatus {
            connected: true,
            ..Default::default()
        });
        world.insert_resource(ProxyPlaybackClock { t: 0.5, init: true });

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
        // The gauge saw the (tiny) divergence — a healthy body is measured, not just
        // an unhealthy one, so the baseline is visible in the field (review N3).
        let stats = world.resource::<lunco_core::DivergenceStats>();
        assert_eq!(stats.bodies[&gid].kind, lunco_core::PredictionKind::Free);
        assert!(stats.bodies[&gid].last_m < 0.1);
        assert_eq!(stats.bodies[&gid].rebaselines, 0);
    }

    /// **The re-possession bug, client side (review N1).** A snapshot still carrying
    /// the PREVIOUS owner's input ack — in flight, or already sitting in
    /// `InterpBuffers`, when we took possession — must not be latched as
    /// `last_reconciled`. If it is, every ack from OUR seq stream (which restarts at
    /// 1) is `<=` it and this system early-returns forever: the rover we are driving
    /// is never reconciled again, and drifts without bound. The host resets the
    /// watermark on the handover; this guard covers the in-flight window between the
    /// two, and is what keeps a `Snap` reachable at all.
    #[test]
    fn stale_ack_from_a_previous_owner_does_not_kill_reconciliation() {
        let mut world = World::new();
        world.init_resource::<InterpBuffers>();
        world.init_resource::<PredictedStateLog>();
        world.init_resource::<lunco_core::OwnedInputLog>();
        world.init_resource::<lunco_core::DivergenceStats>();

        let gid = 0x00CC_0001u64;
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

        // WE have emitted exactly one input (seq 1) — we just possessed this rover.
        world
            .resource_mut::<lunco_core::OwnedInputLog>()
            .0
            .entry(gid)
            .or_default()
            .next_seq = 1;
        world
            .resource_mut::<PredictedStateLog>()
            .0
            .entry(gid)
            .or_default()
            .ring
            .push_back(PredictedState {
                seq: 1,
                pos: Vec3::ZERO,
                rot: Quat::IDENTITY,
            });

        // A stale snapshot arrives still advertising the PREVIOUS owner's seq 5000.
        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                gen_t: 0.0,
                pos: Vec3::new(100.0, 0.0, 0.0),
                rot: Quat::IDENTITY,
                pos_world: DVec3::new(100.0, 0.0, 0.0),
                lv: Vec3::ZERO,
                av: Vec3::ZERO,
                last_input_seq: 5000,
            });
        world.run_system_once(reconcile_owned_prediction).unwrap();

        // It must have been IGNORED — not latched as `last_reconciled`.
        assert_eq!(
            world.resource::<PredictedStateLog>().0[&gid].last_reconciled,
            0,
            "an ack above the highest seq we ever minted is not ours; latching it \
             disables this system permanently"
        );

        // Now OUR ack (seq 1) lands, with authority 100 m away → the Snap path, which
        // the stale ack would otherwise have made unreachable for the whole session.
        world
            .resource_mut::<InterpBuffers>()
            .0
            .entry(gid)
            .or_default()
            .push_back(InterpSample {
                gen_t: 0.1,
                pos: Vec3::new(100.0, 0.0, 0.0),
                rot: Quat::IDENTITY,
                pos_world: DVec3::new(100.0, 0.0, 0.0),
                lv: Vec3::ZERO,
                av: Vec3::ZERO,
                last_input_seq: 1,
            });
        world.run_system_once(reconcile_owned_prediction).unwrap();

        assert_eq!(
            world.resource::<PredictedStateLog>().0[&gid].last_reconciled,
            1
        );
        let p = world.entity(e).get::<Position>().unwrap().0;
        assert!(
            (p.x - 100.0).abs() < 1e-4,
            "the gross-desync snap must still fire for the new owner; got {p:?}"
        );
        // …and the rebaseline was counted + announced rather than being silent (N3).
        assert_eq!(
            world.resource::<lunco_core::DivergenceStats>().bodies[&gid].rebaselines,
            1
        );
    }
}

/// **Platform-semantics regression guard for `drive_kinematic_proxies`** (was the
/// Step-1 probe; kept because the velocity-drive design *depends* on these avian
/// 0.6.1 facts staying true across upgrades). Two questions the design rides on:
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
/// each `app.update()` is exactly one fixed tick. Asserts platform behavior, not
/// our code — if it ever fails after an avian bump, the velocity-drive needs review.
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
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                H,
            )));
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
        let vel = app
            .world()
            .entity(target)
            .get::<LinearVelocity>()
            .unwrap()
            .0;

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

/// Step 1.8 — pure-function tests for the curve evaluator and the angular-velocity
/// helper that `drive_kinematic_proxies` relies on. No solver, no app.
#[cfg(test)]
mod step1_curve_tests {
    use super::*;

    fn sample(gen_t: f64, pos: DVec3, lv: Vec3, rot: Quat) -> InterpSample {
        InterpSample {
            gen_t,
            pos: pos.as_vec3(),
            rot,
            pos_world: pos,
            lv,
            av: Vec3::ZERO,
            last_input_seq: 0,
        }
    }

    /// Hermite hits the sample positions exactly at the bracket endpoints.
    #[test]
    fn hermite_matches_endpoints() {
        let mut buf = VecDeque::new();
        buf.push_back(sample(
            0.0,
            DVec3::new(0.0, 0.0, 0.0),
            Vec3::X,
            Quat::IDENTITY,
        ));
        buf.push_back(sample(
            1.0,
            DVec3::new(5.0, 0.0, 0.0),
            Vec3::X,
            Quat::IDENTITY,
        ));

        let (p0, _, _, _) = sample_curve(&buf, 0.0).unwrap();
        let (p1, _, _, _) = sample_curve(&buf, 1.0).unwrap();
        assert!(
            (p0 - DVec3::new(0.0, 0.0, 0.0)).length() < 1e-9,
            "start: {p0:?}"
        );
        assert!(
            (p1 - DVec3::new(5.0, 0.0, 0.0)).length() < 1e-9,
            "end: {p1:?}"
        );
    }

    /// Constant velocity (`p1 = p0 + v·span`, equal end tangents) ⇒ Hermite is
    /// exactly the straight line: the midpoint is the geometric midpoint.
    #[test]
    fn hermite_constant_velocity_is_linear() {
        let v = Vec3::new(2.0, 0.0, 0.0);
        let mut buf = VecDeque::new();
        buf.push_back(sample(0.0, DVec3::ZERO, v, Quat::IDENTITY));
        buf.push_back(sample(1.0, DVec3::new(2.0, 0.0, 0.0), v, Quat::IDENTITY)); // p0 + v·1

        let (mid, _, _, _) = sample_curve(&buf, 0.5).unwrap();
        assert!(
            (mid - DVec3::new(1.0, 0.0, 0.0)).length() < 1e-9,
            "constant-v midpoint should be linear; got {mid:?}"
        );
    }

    /// Starved (t past newest sample) glides along velocity, distance-capped.
    #[test]
    fn starved_extrapolates_then_caps() {
        let mut buf = VecDeque::new();
        buf.push_back(sample(
            0.0,
            DVec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Quat::IDENTITY,
        ));

        // Small overshoot within both caps: linear glide = v·dt.
        let (p, _, _, _) = sample_curve(&buf, 0.1).unwrap();
        assert!(
            (p - DVec3::new(0.1, 0.0, 0.0)).length() < 1e-9,
            "glide: {p:?}"
        );

        // Far past: time cap (0.25) then distance cap (8.0) bound it — here time
        // cap binds first (1 m/s × 0.25 s = 0.25 m).
        let (far, _, _, _) = sample_curve(&buf, 100.0).unwrap();
        assert!(
            far.x <= INTERP_MAX_EXTRAP_DIST + 1e-9,
            "distance cap: {far:?}"
        );
        assert!((far.x - 0.25).abs() < 1e-9, "time cap should bind: {far:?}");
    }

    /// Empty buffer ⇒ nothing to sample.
    #[test]
    fn empty_buffer_is_none() {
        let buf: VecDeque<InterpSample> = VecDeque::new();
        assert!(sample_curve(&buf, 0.0).is_none());
    }

    /// ω = 0 when orientation already matches.
    #[test]
    fn ang_vel_identity_is_zero() {
        let q = Quat::from_rotation_y(0.7);
        let w = ang_vel_to_track(q, q, 1.0 / 64.0);
        assert!(w.length() < 1e-9, "no rotation ⇒ ω≈0; got {w:?}");
    }

    /// 90° about +Y over h ⇒ ω ≈ (0, (π/2)/h, 0).
    #[test]
    fn ang_vel_quarter_turn_about_y() {
        let h = 1.0 / 64.0;
        let w = ang_vel_to_track(
            Quat::IDENTITY,
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            h,
        );
        let expected = (std::f64::consts::FRAC_PI_2) / h;
        assert!(
            w.x.abs() < 1e-6 && w.z.abs() < 1e-6,
            "axis should be +Y; got {w:?}"
        );
        assert!(
            (w.y - expected).abs() < 1e-4,
            "ω.y expected {expected}; got {}",
            w.y
        );
    }

    /// `w < 0` branch: a quaternion equal to `−q` is the same orientation; the
    /// helper must take the SHORT arc (90°, +Y), not the long way (270°, −Y).
    #[test]
    fn ang_vel_takes_shortest_arc() {
        let h = 1.0 / 64.0;
        // −(90° about +Y): same orientation, but raw w = −cos(45°) < 0.
        let s = std::f32::consts::FRAC_PI_2 / 2.0; // 45°
        let neg = Quat::from_xyzw(0.0, -s.sin(), 0.0, -s.cos());
        let w = ang_vel_to_track(Quat::IDENTITY, neg, h);
        let expected = (std::f64::consts::FRAC_PI_2) / h; // short arc magnitude
        assert!(w.y > 0.0, "short arc should be +Y; got {w:?}");
        assert!(
            (w.y - expected).abs() < 1e-4,
            "ω.y short-arc expected {expected}; got {}",
            w.y
        );
    }
}
