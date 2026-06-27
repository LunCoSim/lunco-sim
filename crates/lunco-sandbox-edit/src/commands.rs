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
use crate::catalog::{SpawnCatalog, spawn_usd_entry};

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

/// Force a re-scan of project USD files into the spawn catalog. Picks up
/// `*.usda` dropped into an already-open Twin mid-session (twin-open is
/// auto-scanned; this covers new files after that). Idempotent.
#[Command(default)]
pub struct RescanSpawnCatalog {}

/// Observer for [`RescanSpawnCatalog`].
pub fn on_rescan_spawn_catalog(
    _trigger: On<RescanSpawnCatalog>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<crate::catalog::SpawnCatalog>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        let n = crate::catalog::scan_usd_into_catalog(roots, &mut catalog);
        info!("RESCAN_SPAWN_CATALOG: +{n} USD asset(s)");
    }
}

/// Observer that handles SpawnEntity commands.
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
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

    let result = spawn_usd_entry(&mut commands, &asset_server, entry, cmd.position, grid);

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
        let result = spawn_usd_entry(&mut commands, &asset_server, entry, pos, grid);
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
            commands.entity(e).insert(RigidBody::Kinematic);
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
    q_local_sim: Query<(), Or<(With<lunco_core::OwnedLocally>, With<lunco_core::PredictedDynamic>)>>,
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
                commands.entity(e).insert(hint);
            }
        }
    }
}

/// Client (FixedUpdate): drive each kinematic replicated proxy that has a
/// `RigidBody` **through the solver** by setting its `LinearVelocity` /
/// `AngularVelocity` toward the shared interpolation curve, instead of teleporting
/// its `Transform` each frame. This is the core of Step 1 (`PREDICT_AND_SMOOTH`):
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
    q_local_sim: Query<(), Or<(With<lunco_core::OwnedLocally>, With<lunco_core::PredictedDynamic>)>>,
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
        // Skip articulated wheels: they are never owned in the registry (only the
        // chassis gid is claimed), so this system would strip the `OwnedLocally`
        // that `propagate_owned_to_wheels` mirrors onto an owned rover's wheels.
        (With<lunco_core::NetReplicate>, Without<lunco_core::ArticulatedLink>),
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
        (With<lunco_core::OwnedLocally>, With<lunco_core::ArticulatedVehicle>),
    >,
    q_wheels: Query<(Entity, &ChildOf, Has<lunco_core::OwnedLocally>), With<lunco_core::ArticulatedLink>>,
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
                    .insert((lunco_core::OwnedLocally, RigidBody::Dynamic));
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
            lunco_core::Reconciliation::Correct { pos: new_pos, rot: new_rot } => {
                let dpos = new_pos - tf.translation;
                let drot = (new_rot * tf.rotation.inverse()).normalize();
                match off {
                    Some(mut pc) => {
                        pc.pos += dpos;
                        pc.rot = (drot * pc.rot).normalize();
                    }
                    None => {
                        commands
                            .entity(e)
                            .insert(PendingCorrection { pos: dpos, rot: drot });
                    }
                }
            }
            // Gross desync: teleport semantics — seat pose directly (Transform
            // included; the interpolation easing-reset on a real teleport is
            // exactly what we want) and drop any queued residual.
            lunco_core::Reconciliation::Snap { pos: new_pos, rot: new_rot } => {
                tf.translation = new_pos;
                tf.rotation = new_rot;
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

/// Client Phase B (`PREDICTION_MEMBERSHIP.md`): designate **every replicated free
/// dynamic prop** (a ball / crate / cone — whether runtime-spawned OR authored
/// scene content) as [`lunco_core::PredictedDynamic`], so it runs local physics +
/// state-reconcile instead of being kinematic-pinned + interpolated. That makes
/// physics feel **live on the client**: bump a prop and it moves in the same
/// frame, then the snapshot reconcile corrects any drift.
///
/// The cosim guard is now [`lunco_core::NotPredictable`] ALONE — stamped on every
/// cosim-driven / server-only body by `tag_cosim_opaque` and the USD net policy
/// (balloons / `CosimTarget`, whose forces are server-only and not locally
/// computable). That marker was added precisely so the structural
/// `SkipContentStamp` guard wouldn't have to be the only thing (see
/// `NotPredictable`'s doc) — so we no longer restrict to runtime spawns, which
/// had frozen plain scene-content physics props server-only. Rovers
/// (`RoverVessel`) and the possessed rover (`OwnedLocally`) are excluded — they
/// have their own paths. Flips the body to `Dynamic` (it may already have been
/// pinned `Kinematic` by `force_kinematic_proxies` the prior frame); a `Static`
/// prop is left alone. Client-only.
pub fn maintain_predicted_dynamic(
    role: Res<lunco_core::NetworkRole>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            Without<lunco_core::RoverVessel>,
            Without<lunco_core::OwnedLocally>,
            Without<lunco_core::PredictedDynamic>,
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

/// Client Step 4 (`PREDICT_AND_SMOOTH` §5, predict-all-vehicles): designate every
/// **remote rover** (`RoverVessel` you don't possess) as
/// [`lunco_core::PredictedDynamic`] too — locally `Dynamic`, state-reconciled to
/// authority each snapshot — reusing the Phase B machinery wholesale.
///
/// Why: with Step 1 a remote rover is a *kinematic* proxy. It can push your owned
/// rover (its velocity enters contact), but it never **yields** to *being* pushed —
/// so when YOU drive into it, your predicted rover bounces off an immovable wall
/// while authority shows that rover yielding and moving away → reconcile fights the
/// mismatch → the "client push" jitter. Making it locally `Dynamic` means it yields
/// in the same step you push it, matching authority → crisp mutual push.
///
/// Reuses `PredictedDynamic` (not a separate marker): every predict-own seam
/// already excludes it (kinematic pin / drive / interpolate), and
/// [`maintain_predicted_dynamic`]'s possession-demote already removes the marker
/// when you possess the rover (its input-replay path takes over). Cosim rovers are
/// safe — `tag_cosim_opaque` only marks non-`RoverVessel` bodies `NotPredictable`,
/// which we still exclude here. Between snapshots the rover dead-reckons on the
/// authoritative velocity seated by `reconcile_predicted_dynamic`; held-input
/// feed-forward (Step 3) would sharpen the actively-driven-remote case but is not
/// needed for the push fix. Client-only.
pub fn maintain_predicted_vehicles(
    role: Res<lunco_core::NetworkRole>,
    local: Res<lunco_core::LocalSession>,
    reg: Res<lunco_core::SessionRegistry>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &lunco_core::GlobalEntityId, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            With<lunco_core::RoverVessel>,
            Without<lunco_core::OwnedLocally>,
            Without<lunco_core::PredictedDynamic>,
            Without<lunco_core::NotPredictable>,
            // Articulated (Physical/joint) rovers must NOT be single-body
            // predicted: only the chassis is replicated, so making it Dynamic +
            // reconciling its pose each snapshot while the jointed wheels run
            // free injects joint energy → flip. They stay kinematic proxies
            // (chassis pose forced by snapshots, cannot flip). Raycast rovers are
            // single bodies and predict fine.
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
        // NEVER vehicle-predict a rover THIS session owns: its prediction
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
        commands
            .entity(e)
            .insert((lunco_core::PredictedDynamic, RigidBody::Dynamic));
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
        let Some(buf) = buffers.0.get(&g) else { continue };
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

        if dist > RECONCILE_SNAP_DIST {
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
            // Soft CONTINUOUS spring (every fixed tick): SET the residual to the
            // freshly-measured error; `drain_pending_corrections` eases a bounded bit
            // per tick into Position/Rotation (smooth, never a Transform write).
            // Velocity is LEFT to local physics so contacts/your push produce real,
            // crisp response — the dead-zone above is the yield budget. This is what
            // makes a remote body both stay synced AND interact.
            let dpos = err.as_vec3();
            match off {
                Some(mut pc) => {
                    pc.pos = dpos;
                    pc.rot = rot_err;
                }
                None => {
                    commands
                        .entity(e)
                        .insert(PendingCorrection { pos: dpos, rot: rot_err });
                }
            }
        }
        // else: within tolerance — leave the body entirely to local physics; any
        // residual `PendingCorrection` finishes draining and removes itself.
    }
}

/// Step 2 (revised): residual reconcile correction, drained in **physics space**
/// a tick at a time by [`drain_pending_corrections`].
///
/// The first Step-2 design (a decaying offset written onto the render `Transform`
/// in `PostUpdate`) was architecturally wrong for this app: the sandbox enables
/// `PhysicsInterpolationPlugin::interpolate_all()`, so `bevy_transform_interpolation`
/// owns every body's `Transform` at render rate — and treats ANY external
/// `Transform` write as a teleport, resetting its easing. Our offset writer
/// therefore *disabled* interpolation for the corrected body (≈ continuously while
/// driving, since corrections land every ~1–2 s and the offset decayed for ~1 s)
/// → the rover rendered at raw 64 Hz steps = the persistent "jitters while just
/// holding the key" the host never shows.
///
/// Correct composition: never touch `Transform` from game code. Park the
/// correction here and let the drain system nudge `Position`/`Rotation` by a tiny
/// bounded amount each FIXED tick; writeback + interpolation then render it
/// perfectly smoothly with no second writer anywhere.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PendingCorrection {
    /// Remaining position delta to apply (world metres).
    pub pos: Vec3,
    /// Remaining orientation delta (applied as `rot * current`).
    pub rot: Quat,
}

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

impl PendingCorrection {
    pub fn is_negligible(&self) -> bool {
        self.pos.length_squared() < 1e-8 && self.rot.angle_between(Quat::IDENTITY) < 1e-4
    }
}

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
///  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wgsl"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"cell_a","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → load that `.wgsl` (asset path) and bind it as a `ShaderMaterial`.
/// - any parameter named by the shader's `Material` struct (e.g. `albedo`,
///   `wedge_count`, `cell_a`) → update that shader uniform in place by name
///   (requires `shader` set first, or a USD shader material). The material's
///   reflected schema resolves the type; colours are `r,g,b`.
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

/// Apply one `StandardMaterial` (PBR) property addressed by `SetObjectProperty`.
///
/// Value formats: colors are comma-separated **linear** `r,g,b[,a]` in 0..1 (so
/// they round-trip the Inspector's `color_edit_button_rgb`); scalars a single
/// float; booleans `true`/`1`/`yes`/`on`. Returns `false` if the value didn't
/// parse so the caller can warn.
fn apply_pbr_param(mat: &mut StandardMaterial, key: &str, value: &str) -> bool {
    let f: Vec<f32> = value
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    let parse_bool = |v: &str| matches!(v.trim(), "true" | "1" | "yes" | "on");
    match key {
        "base_color" => {
            if f.len() < 3 { return false; }
            let a = f.get(3).copied().unwrap_or_else(|| mat.base_color.to_linear().alpha);
            mat.base_color = Color::LinearRgba(LinearRgba::new(f[0], f[1], f[2], a));
        }
        "emissive" => {
            if f.len() < 3 { return false; }
            mat.emissive = LinearRgba::new(f[0], f[1], f[2], f.get(3).copied().unwrap_or(1.0));
        }
        "metallic" => { let Some(v) = f.first() else { return false }; mat.metallic = v.clamp(0.0, 1.0); }
        "roughness" | "perceptual_roughness" => {
            let Some(v) = f.first() else { return false };
            mat.perceptual_roughness = v.clamp(0.0, 1.0);
        }
        "reflectance" => { let Some(v) = f.first() else { return false }; mat.reflectance = v.clamp(0.0, 1.0); }
        "alpha" | "opacity" => {
            let Some(v) = f.first() else { return false };
            let v = v.clamp(0.0, 1.0);
            let mut lin = mat.base_color.to_linear();
            lin.alpha = v;
            mat.base_color = Color::LinearRgba(lin);
            mat.alpha_mode = if v >= 1.0 { AlphaMode::Opaque } else { AlphaMode::Blend };
        }
        "unlit" => mat.unlit = parse_bool(value),
        "double_sided" => {
            let b = parse_bool(value);
            mat.double_sided = b;
            mat.cull_mode = if b { None } else { Some(bevy::render::render_resource::Face::Back) };
        }
        _ => return false,
    }
    true
}

/// Observer for [`SetObjectProperty`].
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    q_mat: Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    q_std_mat: Query<&MeshMaterial3d<StandardMaterial>>,
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
        // StandardMaterial (PBR) properties — for props/rovers that use the
        // default bevy material rather than a custom `ShaderMaterial`. Mutates
        // the live asset in place (same immediate-feedback path as the shader
        // params below). Explicit arms so these names never get stolen by the
        // shader-param fallback.
        "base_color" | "emissive" | "metallic" | "roughness" | "perceptual_roughness"
        | "reflectance" | "alpha" | "opacity" | "unlit" | "double_sided" => {
            let Ok(m) = q_std_mat.get(target) else {
                warn!("SET_PROPERTY: entity {} has no StandardMaterial", cmd.entity_id);
                return;
            };
            let Some(mat) = std_materials.get_mut(&m.0) else { return };
            if apply_pbr_param(mat, cmd.property.as_str(), &cmd.value) {
                info!("SET_PROPERTY: {} pbr {} = {}", cmd.entity_id, cmd.property, cmd.value);
            } else {
                warn!("SET_PROPERTY: bad value '{}' for pbr '{}'", cmd.value, cmd.property);
            }
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
    mut q_avatar: Query<
        (&mut Transform, Option<&mut lunco_avatar::FreeFlightCamera>),
        With<lunco_core::Avatar>,
    >,
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
    // Tolerate 0/≥1 avatars robustly. `single_mut()` errored when the avatar was
    // momentarily in a non-freeflight camera mode (FreeFlightCamera removed by
    // possess/follow/orbit) OR when more than one Avatar existed (USD avatar +
    // fallback) — both surfaced as "no Avatar" and killed double-click focus.
    // Take the first avatar; the FreeFlightCamera is now optional.
    let avatar_count = q_avatar.iter().count();
    let Some((mut tf, ff_opt)) = q_avatar.iter_mut().next() else {
        warn!("FOCUS_ENTITY: no Avatar entity in the scene (count={avatar_count})");
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
    let d = (-offset).normalize();
    match ff_opt {
        // Free-flight rebuilds rotation from yaw/pitch every frame (YXZ euler), so
        // when it's present we must set those rather than the Transform rotation.
        Some(mut ff) => {
            ff.yaw = (-d.x).atan2(-d.z);
            ff.pitch = d.y.clamp(-1.0, 1.0).asin();
        }
        // Non-freeflight camera mode (orbit/spring/possessed): set the look
        // rotation directly. Best-effort — a mode that re-derives rotation from
        // its own target may override it, but the avatar still flies to the frame.
        None => {
            tf.look_at(target_pos, Vec3::Y);
        }
    }
    info!(
        "FOCUS_ENTITY: framed api_id={} at {:.1} m (avatars={avatar_count})",
        cmd.entity_id, dist
    );
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

// ─────────────────────────────────────────────────────────────────────────
// Live shader authoring — create from a template, import any `.wgsl` from the
// computer into the open Twin, and discover shaders dropped in the Twin folder.
// All persist into `<twin>/shaders/<name>.wgsl` (fallback `assets/shaders/`),
// register into the picker [`ShaderCatalog`], and can apply to an entity — no
// restart. The created/imported shaders are PBR-compatible self-describing
// shaders (see [`lunco_materials::shader_template`]).
// ─────────────────────────────────────────────────────────────────────────

/// The asset path a shader named `stem` would be installed at: under the
/// primary open Twin (`twin://<name>/shaders/<stem>.wgsl`) or the engine library
/// (`shaders/<stem>.wgsl`) when no Twin is open. Mirrors [`install_shader`]'s
/// destination logic so callers (e.g. the Inspector) can predict the path.
pub fn shader_asset_path_for(
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    stem: &str,
) -> String {
    match twin_roots.and_then(|t| t.primary()) {
        Some((name, _)) => format!("twin://{name}/shaders/{stem}.wgsl"),
        None => format!("shaders/{stem}.wgsl"),
    }
}

/// Sanitise a free-text name into a safe lowercase file stem (`[a-z0-9_]`,
/// trimmed of leading/trailing `_`). Empty input → `"shader"`.
pub fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "shader".to_string() } else { out }
}

/// Core of [`CreateShader`]/[`ImportShader`]: validate the WGSL is a
/// prop-pickable dynamic shader, persist it into the open Twin (fallback
/// `assets/shaders/`), insert it live into [`Assets<Shader>`] so it renders
/// this frame, register it in the picker [`ShaderCatalog`], and optionally bind
/// it to `target` (API id; 0 = none). Returns the asset path on success.
#[allow(clippy::too_many_arguments)]
fn install_shader(
    stem: &str,
    source: &str,
    target: u64,
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    asset_server: &AssetServer,
    shaders: &mut Assets<bevy::shader::Shader>,
    catalog: &mut lunco_materials::ShaderCatalog,
    registry: &lunco_api::registry::ApiEntityRegistry,
    materials: &mut Assets<lunco_materials::ShaderMaterial>,
    q_mat: &Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    commands: &mut Commands,
) -> Option<String> {
    // Gate: must be a self-describing `Material` shader whose only engine field
    // (if any) is `sun_vis`. Otherwise it would render black / can't be driven.
    if !lunco_materials::is_prop_pickable_source(source) {
        warn!(
            "INSTALL_SHADER: '{stem}' is not a prop-pickable dynamic shader \
             (needs a `Material` struct; engine fields limited to `sun_vis`) — skipped"
        );
        return None;
    }

    // Destination: the primary open Twin's `shaders/` dir (portable, persists
    // with the Twin under a `twin://` asset path), else the engine library.
    let (asset_path, disk_path): (String, std::path::PathBuf) =
        match twin_roots.and_then(|t| t.primary()) {
            Some((name, root)) => (
                format!("twin://{name}/shaders/{stem}.wgsl"),
                root.join("shaders").join(format!("{stem}.wgsl")),
            ),
            None => (
                format!("shaders/{stem}.wgsl"),
                std::path::PathBuf::from("assets/shaders").join(format!("{stem}.wgsl")),
            ),
        };

    // Persist to disk (native). Non-fatal on failure — the in-memory insert
    // below still makes it usable this session.
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = disk_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&disk_path, source) {
            Ok(()) => info!("INSTALL_SHADER: wrote {}", disk_path.display()),
            Err(e) => warn!("INSTALL_SHADER: write {} failed: {e}", disk_path.display()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = &disk_path;

    // Insert the compiled source live under the asset path, so any material
    // bound to it renders immediately (no disk round-trip / watcher wait).
    let shader_handle = asset_server.load::<bevy::shader::Shader>(asset_path.clone());
    let shader = bevy::shader::Shader::from_wgsl(source.to_string(), asset_path.clone());
    let _ = shaders.insert(shader_handle.id(), shader);

    // Make it pickable.
    catalog.add(asset_path.clone());

    // Optionally apply to a target entity (preserve any existing shader params).
    if target != 0 {
        let gid = lunco_core::GlobalEntityId::from_raw(target);
        match registry.resolve(&gid) {
            Some(ent) => {
                let template = q_mat
                    .get(ent)
                    .ok()
                    .and_then(|m| materials.get(&m.0))
                    .cloned()
                    .unwrap_or_default();
                let mat_handle =
                    materials.add(lunco_materials::build_shader_material(shader_handle.clone(), template));
                commands
                    .entity(ent)
                    .remove::<MeshMaterial3d<StandardMaterial>>()
                    .insert(MeshMaterial3d(mat_handle));
                info!("INSTALL_SHADER: applied {asset_path} to entity {target}");
            }
            None => warn!("INSTALL_SHADER: target id {target} not in registry"),
        }
    }

    info!("INSTALL_SHADER: registered {asset_path}");
    Some(asset_path)
}

/// Create a new dynamic shader from a built-in template (or supplied WGSL),
/// persist it into the open Twin (`<twin>/shaders/<name>.wgsl`, or
/// `assets/shaders/` when no Twin is open), register it in the picker, and
/// optionally bind it to a target entity — all live, no restart.
///
/// ```json
/// {"command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
/// {"command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
/// ```
#[Command(default)]
pub struct CreateShader {
    /// Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`).
    pub name: String,
    /// Template id when `source` is empty: `"solid"` (default) or `"checker"`.
    pub template: String,
    /// Full WGSL source. Empty → generate from `template`.
    pub source: String,
    /// API id of an entity to apply the new shader to. `0` = create only.
    pub target: u64,
}

/// Observer for [`CreateShader`].
#[allow(clippy::too_many_arguments)]
pub fn on_create_shader(
    trigger: On<CreateShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut materials: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    q_mat: Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    let stem = sanitize_stem(&ev.name);
    let source = if ev.source.trim().is_empty() {
        lunco_materials::shader_template(&ev.template, &stem)
    } else {
        ev.source.clone()
    };
    install_shader(
        &stem,
        &source,
        ev.target,
        twin_roots.as_deref(),
        &asset_server,
        &mut shaders,
        &mut catalog,
        &registry,
        &mut materials,
        &q_mat,
        &mut commands,
    );
}

/// Import an existing `.wgsl` file from anywhere on disk INTO the open Twin
/// (copies it to `<twin>/shaders/<name>.wgsl`), registers it in the picker, and
/// optionally binds it to a target entity. The file must be a prop-pickable
/// dynamic shader (a `Material` struct; engine fields limited to `sun_vis`).
///
/// ```json
/// {"command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
/// ```
#[Command(default)]
pub struct ImportShader {
    /// Filesystem path of the `.wgsl` to import (absolute or cwd-relative).
    pub source_path: String,
    /// Optional new stem; empty → keep the source file's own stem.
    pub name: String,
    /// API id of an entity to apply the imported shader to. `0` = import only.
    pub target: u64,
}

/// Observer for [`ImportShader`].
#[allow(clippy::too_many_arguments, unused_variables, unused_mut)]
pub fn on_import_shader(
    trigger: On<ImportShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut materials: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    q_mat: Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    #[cfg(target_arch = "wasm32")]
    {
        warn!("IMPORT_SHADER: importing from a local file is native-only");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let src = match std::fs::read_to_string(&ev.source_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("IMPORT_SHADER: read '{}' failed: {e}", ev.source_path);
                return;
            }
        };
        let stem = if ev.name.trim().is_empty() {
            std::path::Path::new(&ev.source_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(sanitize_stem)
                .unwrap_or_else(|| "shader".to_string())
        } else {
            sanitize_stem(&ev.name)
        };
        install_shader(
            &stem,
            &src,
            ev.target,
            twin_roots.as_deref(),
            &asset_server,
            &mut shaders,
            &mut catalog,
            &registry,
            &mut materials,
            &q_mat,
            &mut commands,
        );
    }
}

/// Rescan the open Twins' `shaders/` folders (and `assets/shaders`) and register
/// any prop-pickable `.wgsl` into the picker [`ShaderCatalog`]. Lets you drop a
/// shader file into a Twin and pick it up without restarting.
#[Command(default)]
pub struct RescanShaders {}

/// THE shader scanner: register every project `*.wgsl` (engine library + open
/// Twins) into the picker catalog via the shared `lunco_assets::discovery`
/// walk — the same single scanner the spawn catalog uses for `*.usda`. No
/// filter: the picker lists all shaders and flags any whose `@engine` inputs a
/// part can't provide. Idempotent (`add` dedups). Returns the count added.
pub fn scan_wgsl_into_catalog(
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut lunco_materials::ShaderCatalog,
) -> usize {
    let mut n = 0;
    for a in lunco_assets::discovery::list_assets(roots, "wgsl") {
        if catalog.add(a.asset_path) {
            n += 1;
        }
    }
    n
}

/// Populate BOTH catalogs (USD → spawn, WGSL → shaders) from the project. The
/// single scan entry point, driven by [`maintain_catalogs`] (Startup + on
/// Twin-set change) and the manual rescan commands — never a per-frame walk.
pub fn scan_all_catalogs(
    roots: &lunco_assets::twin_source::TwinRoots,
    spawn: &mut crate::catalog::SpawnCatalog,
    shaders: &mut lunco_materials::ShaderCatalog,
) {
    let s = crate::catalog::scan_usd_into_catalog(roots, spawn);
    let w = scan_wgsl_into_catalog(roots, shaders);
    if s > 0 || w > 0 {
        info!("CATALOG_SCAN: +{s} USD, +{w} shader(s)");
    }
}

/// The ONE catalog-population system. Scans the engine library once, then
/// re-scans whenever the set of open Twins changes (so a freshly-opened Twin's
/// files appear) — twin-open is async, so a guarded `Update` check is more
/// robust than racing the `TwinAdded` observer that registers the twin root.
/// It only *walks the disk* on first run and on change; every other frame it
/// early-returns after a cheap name-set comparison (no per-frame rescan).
pub fn maintain_catalogs(
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut spawn: ResMut<crate::catalog::SpawnCatalog>,
    mut shaders: ResMut<lunco_materials::ShaderCatalog>,
    mut last_twins: Local<Vec<String>>,
    mut did_first_scan: Local<bool>,
) {
    let Some(roots) = twin_roots.as_deref() else { return };
    let names = roots.names();
    if *did_first_scan && names == *last_twins {
        return;
    }
    *did_first_scan = true;
    *last_twins = names;
    scan_all_catalogs(roots, &mut spawn, &mut shaders);
}

/// Observer for [`RescanShaders`] — manual full re-scan of the shader catalog.
pub fn on_rescan_shaders(
    _trigger: On<RescanShaders>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        let n = scan_wgsl_into_catalog(roots, &mut catalog);
        info!("RESCAN_SHADERS: +{n} shader(s)");
    }
}

/// Resolve a shader **asset path** to its **disk path**: `twin://<name>/<rel>` →
/// `<twin_root>/<rel>`; an engine path like `shaders/foo.wgsl` → `assets/<path>`.
#[cfg(not(target_arch = "wasm32"))]
fn asset_path_to_disk(
    path: &str,
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
) -> Option<std::path::PathBuf> {
    if let Some(rest) = path.strip_prefix("twin://") {
        let mut it = rest.splitn(2, '/');
        let name = it.next()?;
        let rel = it.next()?;
        Some(twin_roots?.root_of(name)?.join(rel))
    } else {
        Some(std::path::PathBuf::from("assets").join(path))
    }
}

/// Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
/// its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
/// Entities currently using it keep their in-memory material for the session.
///
/// ```json
/// {"command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
/// ```
#[Command(default)]
pub struct DeleteShader {
    /// Asset path to remove (`twin://name/shaders/x.wgsl` or `shaders/x.wgsl`).
    pub path: String,
}

/// Observer for [`DeleteShader`].
#[allow(unused_variables)]
pub fn on_delete_shader(
    trigger: On<DeleteShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    let path = trigger.event().path.trim().to_string();
    if path.is_empty() {
        warn!("DELETE_SHADER: empty path");
        return;
    }
    let removed = catalog.remove(&path);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(disk) = asset_path_to_disk(&path, twin_roots.as_deref()) {
        match std::fs::remove_file(&disk) {
            Ok(()) => info!("DELETE_SHADER: removed {path} ({})", disk.display()),
            Err(e) => warn!("DELETE_SHADER: unregistered {path}, file remove failed: {e}"),
        }
    }
    if !removed {
        warn!("DELETE_SHADER: '{path}' was not in the catalog");
    }
}

/// Plugin that registers SPAWN_ENTITY / MOVE_ENTITY / SET_OBJECT_PROPERTY /
/// FOCUS_ENTITY_BY_ID / SET_CAMERA_LOOK_AT / RELOAD_SHADER / SET_SHADER_SOURCE /
/// CREATE_SHADER / IMPORT_SHADER / RESCAN_SHADERS / DELETE_SHADER command
/// observers and the kinematic-pulse cleanup + twin shader auto-scan systems.
pub struct SpawnCommandPlugin;

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_spawn_entity_command);
        app.add_observer(on_move_entity_command);
        app.add_observer(on_rescan_spawn_catalog);
        app.add_observer(on_set_object_property);
        app.add_observer(on_focus_entity_by_id);
        // NOTE: `SelectEntity`/`on_select_entity` are editor-only (they drive the
        // Inspector highlight + gizmo) and live in the `ui`-gated `selection`
        // module; `SandboxEditPlugin` registers them. The headless server has no
        // selection, so they're absent here by design.
        app.add_observer(on_set_camera_look_at);
        app.add_observer(on_reload_shader);
        app.add_observer(on_set_shader_source);
        app.add_observer(on_create_shader);
        app.add_observer(on_import_shader);
        app.add_observer(on_rescan_shaders);
        app.add_observer(on_delete_shader);
        // Register with AppTypeRegistry so the reflection-based HTTP executor
        // (`get_with_short_type_path`) can construct it from `{"command":"SetObjectProperty",...}`.
        // SpawnEntity/MoveEntity have observers above but were missing from the
        // type registry, so the API couldn't construct them (absent from
        // `discover_schema`). Register them so MCP/HTTP clients can spawn from the
        // catalog and teleport entities exactly like the in-app palette/gizmo.
        app.register_type::<SpawnEntity>();
        app.register_type::<MoveEntity>();
        app.register_type::<RescanSpawnCatalog>();
        app.register_type::<SetObjectProperty>();
        app.register_type::<FocusEntityById>();
        app.register_type::<SetCameraLookAt>();
        app.register_type::<ReloadShader>();
        app.register_type::<SetShaderSource>();
        app.register_type::<CreateShader>();
        app.register_type::<ImportShader>();
        app.register_type::<RescanShaders>();
        app.register_type::<DeleteShader>();
        // THE single catalog-population system: scans project USD → spawn
        // catalog and WGSL → shader catalog via the shared `lunco_assets`
        // discovery walk, once at first run and again only when the open-Twin
        // set changes (guarded — no per-frame disk walk). Replaces the old
        // per-catalog scanners (`populate_dynamic_spawn_catalog`,
        // `auto_scan_twin_shaders`, `discover_shaders`).
        app.add_systems(Update, maintain_catalogs);
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        app.init_resource::<InterpBuffers>();
        app.init_resource::<PredictedStateLog>();
        app.init_resource::<ProxyPlaybackClock>();
        // Resources this plugin's OWN systems read, so it stands alone without the
        // UI-layer `SandboxEditPlugin` / the render-layer `ShaderMaterialPlugin`
        // (e.g. a headless `--no-ui` server that adds only `SpawnCommandPlugin`).
        // `init_resource` is idempotent, so when those plugins also init these it's
        // a harmless no-op:
        //   - `SpawnCatalog`   — read by `maintain_catalogs` + `apply_replicated_spawns`;
        //   - `SelectedEntity` — read by `on_select_entity`;
        //   - `ShaderCatalog`  — read by `maintain_catalogs` (per-frame) + the shader
        //     command observers. Lives in `lunco_materials`; an empty one is fine on
        //     a server (shader discovery populates it but nothing renders it).
        app.init_resource::<crate::catalog::SpawnCatalog>();
        app.init_resource::<crate::SelectedEntities>();
        app.init_resource::<lunco_materials::ShaderCatalog>();
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
                // Mirror the chassis's `OwnedLocally` onto owned-rover wheels so
                // the rover you drive runs local physics on all links (not frozen
                // kinematic proxies). After `maintain_owned_locally` so it reads
                // the freshly-set chassis marker; before the kinematic-pin systems.
                propagate_owned_to_wheels,
                // Phase B: classify free predicted props BEFORE the interpolate /
                // kinematic-pin systems read the `PredictedDynamic` marker.
                maintain_predicted_dynamic,
                // Step 4: predict remote rovers too (reuses PredictedDynamic), so
                // they yield to your push. Must run before the kinematic/interpolate
                // systems read the marker; after maintain_predicted_dynamic so the
                // possession-demote ordering is stable.
                maintain_predicted_vehicles,
                ingest_snapshots,
                interpolate_proxies,
                force_kinematic_proxies,
                apply_net_replication,
            )
                .chain(),
        );
        // Step 1: velocity-drive kinematic RigidBody proxies toward the snapshot
        // curve in `FixedUpdate`, so it runs BEFORE avian's solver step
        // (`FixedPostUpdate`) and the commanded velocity enters this tick's contact
        // resolution. Reads `InterpBuffers` (filled by `ingest_snapshots` in the
        // prior frame's `Update` — one-frame latency, absorbed by `INTERP_DELAY`)
        // and is the sole advance site for `ProxyPlaybackClock`. No-op on
        // host/standalone (guards on `NetworkRole::Client`).
        app.add_systems(FixedUpdate, drive_kinematic_proxies);
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
        // Step 2 (revised) correction smoothing: reconcilers PARK their correction
        // in `PendingCorrection`; this drain applies it to the physics pose a few
        // cm/deg per fixed tick, BEFORE the solve, so writeback + avian's
        // transform-interpolation render it smoothly. Game code never writes
        // `Transform` (which resets `bevy_transform_interpolation`'s easing — the
        // cause of the hold-the-key client jitter).
        app.add_systems(FixedUpdate, drain_pending_corrections);
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
        world.insert_resource(ProxyPlaybackClock { t: -INTERP_DELAY, init: true });

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
        world.insert_resource(ProxyPlaybackClock { t: 0.32, init: true });

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
        buf.push_back(sample(0.0, DVec3::new(0.0, 0.0, 0.0), Vec3::X, Quat::IDENTITY));
        buf.push_back(sample(1.0, DVec3::new(5.0, 0.0, 0.0), Vec3::X, Quat::IDENTITY));

        let (p0, _, _, _) = sample_curve(&buf, 0.0).unwrap();
        let (p1, _, _, _) = sample_curve(&buf, 1.0).unwrap();
        assert!((p0 - DVec3::new(0.0, 0.0, 0.0)).length() < 1e-9, "start: {p0:?}");
        assert!((p1 - DVec3::new(5.0, 0.0, 0.0)).length() < 1e-9, "end: {p1:?}");
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
        buf.push_back(sample(0.0, DVec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY));

        // Small overshoot within both caps: linear glide = v·dt.
        let (p, _, _, _) = sample_curve(&buf, 0.1).unwrap();
        assert!((p - DVec3::new(0.1, 0.0, 0.0)).length() < 1e-9, "glide: {p:?}");

        // Far past: time cap (0.25) then distance cap (8.0) bound it — here time
        // cap binds first (1 m/s × 0.25 s = 0.25 m).
        let (far, _, _, _) = sample_curve(&buf, 100.0).unwrap();
        assert!(far.x <= INTERP_MAX_EXTRAP_DIST + 1e-9, "distance cap: {far:?}");
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
        let w = ang_vel_to_track(Quat::IDENTITY, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), h);
        let expected = (std::f64::consts::FRAC_PI_2) / h;
        assert!(w.x.abs() < 1e-6 && w.z.abs() < 1e-6, "axis should be +Y; got {w:?}");
        assert!((w.y - expected).abs() < 1e-4, "ω.y expected {expected}; got {}", w.y);
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
        assert!((w.y - expected).abs() < 1e-4, "ω.y short-arc expected {expected}; got {}", w.y);
    }
}
