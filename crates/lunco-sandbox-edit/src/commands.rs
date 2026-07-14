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
use bevy::math::{DQuat, DVec3};
use avian3d::prelude::{
    AngularVelocity, Collisions, LinearVelocity, PhysicsSystems, RigidBody, SubstepCount,
};
use avian3d::schedule::{PhysicsSchedule, Physics, Substeps};
use avian3d::physics_transform::{Position, Rotation};
use big_space::prelude::Grid;
use lunco_core::{on_command, register_commands, Command};
use lunco_obstacle_field::ObstacleFieldRoot;
// Appearance INTENT (render-free). `SetObjectProperty`'s PBR keys mutate `PbrLook`
// and its shader keys mutate `ShaderLook`; the render binders re-materialise on
// `Changed<PbrLook>` / `Changed<ShaderLook>`. This file names no material type —
// see `docs/architecture/render-decoupling.md`.
use lunco_materials::{ParamSchema, ParamType, ParamValue, ShaderLook};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::{UsdOp, LayerId};
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use std::collections::{HashMap, HashSet, VecDeque};
use crate::catalog::{SpawnCatalog, SpawnSource, spawn_usd_entry};

/// Spawn an entity from the catalog at a given world position.
///
/// The TYPE lives in `lunco_core::commands` (review A6): the networking crate
/// declares this command's wire channel and needs nothing but the type, so keeping
/// the definition in core is what let `lunco-networking` drop its dependency on
/// this crate. The HANDLER (`on_spawn_entity_command`) stays here, with the
/// catalog it spawns from. Re-exported so existing call sites are unchanged.
pub use lunco_core::SpawnEntity;

/// Detach a joint by despawning it.
#[Command(reflect_default)]
pub struct DetachJoint {
    /// The joint entity to despawn.
    pub target: Entity,
    /// Persistent (default) authors the joint's removal into the scene's runtime
    /// layer — so it journals, syncs, and survives reload — before despawning.
    /// Interactive just pops the live joint (a throwaway test), no journal. See
    /// [`lunco_core::EditIntent`]. Omitted by API callers → `Persistent`.
    #[serde(default)]
    pub intent: lunco_core::EditIntent,
}

impl Default for DetachJoint {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            intent: lunco_core::EditIntent::Persistent,
        }
    }
}


/// Force a re-scan of project USD files into the spawn catalog. Picks up
/// `*.usda` dropped into an already-open Twin mid-session (twin-open is
/// auto-scanned; this covers new files after that). Idempotent.
#[Command(default)]
pub struct RescanSpawnCatalog {}

/// Observer for [`RescanSpawnCatalog`].
#[on_command(RescanSpawnCatalog)]
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

/// Observer that handles DetachJoint commands — despawns the live joint entity in
/// BOTH modes (the visible effect). Persistence is a decoupled observer below.
#[on_command(DetachJoint)]
pub fn on_detach_joint(
    trigger: On<DetachJoint>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if let Ok(mut entity) = commands.get_entity(cmd.target) {
        entity.try_despawn();
        info!("DETACH_JOINT: despawned joint entity {:?} ({:?})", cmd.target, cmd.intent);
    }
}

// ── Dock release, as an actuator on the normal intent→port machinery ─────────

/// Dock/release actuator. A vessel exposes a `release` command PORT; when it rises
/// past 0.5 the fixed joint attaching this vessel to another body is detached, once.
/// Driven exactly like throttle/steer: `Release` intent (KeyG) → the `_LanderControl`
/// profile's `release`→`release` binding → `SetPorts` → this port. Replaces the old
/// hardcoded G-to-detach special case; it works for any possessed vessel + dock joint.
#[derive(bevy::prelude::Component, Default, bevy::prelude::Reflect)]
#[reflect(Component)]
pub struct ReleaseActuator {
    /// Commanded release 0..1, written by the control binding.
    pub cmd: f32,
    /// Edge latch so a held key detaches only once.
    latched: bool,
}

/// Port backend exposing `release` on any entity carrying a [`ReleaseActuator`].
const RELEASE_BACKEND: lunco_core::ports::PortBackend = lunco_core::ports::PortBackend {
    list: |w, e, out| {
        if let Some(a) = w.get::<ReleaseActuator>(e) {
            out.push(lunco_core::ports::PortRef {
                name: "release".to_string(),
                direction: lunco_core::ports::PortDirection::InOut,
                value: a.cmd as f64,
            });
        }
    },
    read_output: |w, e, n| {
        if n != "release" { return None; }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    read_input: |w, e, n| {
        if n != "release" { return None; }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    write_input: |w, e, n, v| {
        if n != "release" { return false; }
        if let Some(mut a) = w.get_mut::<ReleaseActuator>(e) {
            a.cmd = v as f32;
            return true;
        }
        false
    },
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Register [`RELEASE_BACKEND`] once the `PortRegistry` exists (after the cosim
/// builtins). `Option` so an app without cosim doesn't panic.
fn register_release_backend(reg: Option<ResMut<lunco_core::ports::PortRegistry>>) {
    if let Some(mut reg) = reg {
        reg.register(RELEASE_BACKEND);
    }
}

/// Give the CONTROL entity of every control-bound vessel a [`ReleaseActuator`], so
/// its `release` port is where the control binding actually writes. (A USD prim can
/// spawn several entities sharing one `UsdPrimPath` — the control/model entity vs the
/// physics-body entity a joint references; the binding targets the former, so the
/// actuator must live there. `joint_release_system` bridges to the joint by path.)
fn attach_release_actuator(
    mut commands: Commands,
    q: Query<Entity, (Added<lunco_core::ControlBinding>, Without<ReleaseActuator>)>,
) {
    for e in &q {
        // `try_insert`: scene-load churn (or a doc-backed reload) can despawn a
        // just-added ControlBinding entity before this deferred insert applies —
        // a plain `insert` then panics on the invalid entity. Same despawn-safe
        // idiom as gizmo/hardware/terrain-surface.
        commands.entity(e).try_insert(ReleaseActuator::default());
    }
}

/// Edge-detect the `release` command → detach the fixed joint attaching this vessel
/// to another body. The principled generalization of the old G-to-detach: any
/// possessed vessel, any dock joint, no per-scene name matching.
fn joint_release_system(
    mut vessels: Query<(&mut ReleaseActuator, &UsdPrimPath)>,
    joints: Query<(Entity, &avian3d::prelude::FixedJoint)>,
    body_paths: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    for (mut act, vpath) in &mut vessels {
        if act.cmd > 0.5 {
            if !act.latched {
                act.latched = true;
                // Bridge control-entity → physics-body by shared USD path: detach any
                // fixed joint whose bodies resolve to this vessel's prim path.
                for (je, j) in &joints {
                    let hit = [j.body1, j.body2].into_iter().any(|b| {
                        body_paths.get(b).is_ok_and(|p| p.path == vpath.path)
                    });
                    if hit {
                        info!("RELEASE: vessel {} detaching joint {je:?}", vpath.path);
                        // Runtime undock (a live physics action, not an authored scene
                        // edit) → Interactive so it doesn't journal.
                        commands.trigger(DetachJoint {
                            target: je,
                            intent: lunco_core::EditIntent::Interactive,
                        });
                    }
                }
            }
        } else {
            act.latched = false;
        }
    }
}

/// Persist a **`Persistent`** `DetachJoint` into the active USD document's runtime
/// overlay by authoring a `RemovePrim` — so the detachment journals, syncs, and
/// survives reload. Decoupled from [`on_detach_joint`] (which does the live
/// despawn), mirroring [`persist_move_to_runtime_layer`]: same active-doc +
/// ownership guard, same `LayerId::runtime()` target. `Interactive` detaches are
/// throwaway (no journal), so this early-returns for them.
pub fn persist_detach_to_runtime_layer(
    trigger: On<DetachJoint>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if !cmd.intent.is_persistent() {
        return;
    }
    let Some((doc, path)) =
        authorable_prim(cmd.target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };

    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
    });
}

/// Observer that handles SpawnEntity commands.
#[on_command(SpawnEntity)]
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
    role: Res<lunco_core::NetworkRole>,
    dem: Query<(&GlobalTransform, &lunco_terrain_surface::stream_viz::DemHeightField)>,
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

    // Terrain-fit the drop height: snap to the DEM surface (+ the asset's spawn
    // lift) when streamed terrain covers this (x,z), so an API / headless / rhai
    // spawn lands ON the surface instead of free-falling when the collider under
    // the drop point hasn't baked yet. No DEM here (a flat scene, or an intentional
    // altitude) → the position is used exactly as given. The GUI palette path does
    // its own richer footprint fit before triggering, so its position arrives fitted.
    let mut position = cmd.position;
    if let Some(y) = lunco_terrain_surface::stream_viz::dem_ground_height(
        dem.iter(),
        position.x as f64,
        position.z as f64,
    ) {
        position.y = y as f32 + entry.spawn_lift;
    }

    info!("SPAWN_ENTITY: {} at {:?}", cmd.entry_id, position);

    let rotation = cmd.rotation.unwrap_or(Quat::IDENTITY);
    let result = spawn_usd_entry(&mut commands, &asset_server, entry, position, rotation, grid);

    // Networked identity (gap G2): a runtime instance gets a server-allocated
    // unique id (SkipContentStamp → assign_global_entity_ids mints
    // Authoritative, never colliding `Content`), is marked for transform
    // replication, and records what to replicate so the host can broadcast the
    // spawn to clients.
    commands.entity(result.root_entity).try_insert((
        lunco_core::SkipContentStamp,
        lunco_core::NetReplicate,
        lunco_core::NetSpawn {
            entry_id: cmd.entry_id.clone(),
            position,
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
    // Drain in place — the loop body touches only `commands`/`catalog`/
    // `asset_server`, never `pending`, so the old `.collect::<Vec<_>>()`
    // was a pure-waste allocation (CQ-216).
    for job in pending.0.drain(..) {
        let Some(entry) = catalog.get(&job.entry_id) else {
            warn!("REPL_SPAWN: unknown entry '{}'", job.entry_id);
            continue;
        };
        let pos = job.position;
        let result = spawn_usd_entry(&mut commands, &asset_server, entry, pos, Quat::IDENTITY, grid);
        // Pin the host id; mark runtime instance + replication target. Forced
        // Kinematic by `force_kinematic_proxies` so snapshots drive it.
        commands.entity(result.root_entity).try_insert((
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
        Self { enabled, yaw_rate: 0.5, speed: 4.0, lead_secs }
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
                    gid.get(), last_active, tick.0
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
                        let Ok(mut em) = world.get_entity_mut(ent) else { return };
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
                info!("[predict] demote owned rover gid={} (idle/released)", gid.get());
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
            let fwd = (gt.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
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
    *ON.get_or_init(|| {
        std::env::var("LUNCO_ROLLBACK").map_or(false, |v| v == "1" || v == "true")
    })
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
    RbState { pos: p.0, rot: r.0, lv: lv.0, av: av.0 }
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
    q_body: Query<
        (&Position, &Rotation, &LinearVelocity, &AngularVelocity),
        With<RigidBody>,
    >,
) {
    if !rollback_enabled() {
        return;
    }
    for (chassis_e, gid, p, r, lv, av) in q_chassis.iter() {
        let g = gid.get();
        let Some(seq) = input_log.0.get(&g).and_then(|l| l.frames.back()).map(|f| f.seq) else {
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
        let mut q = world.query_filtered::<
            (Entity, &lunco_core::GlobalEntityId),
            (With<lunco_core::OwnedLocally>, Without<lunco_core::ArticulatedLink>),
        >();
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
        let Ok(mut em) = world.get_entity_mut(*e) else { continue };
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
                            .try_insert(PendingCorrection { pos: dpos, rot: drot });
                    }
                }
            }
            // Gross desync: teleport semantics — seat pose directly (Transform
            // included; the interpolation easing-reset on a real teleport is
            // exactly what we want) and drop any queued residual.
            lunco_core::Reconciliation::Snap { pos: new_pos, rot: new_rot } => {
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
/// (a `FlightSoftware` control surface) and the possessed rover (`OwnedLocally`)
/// are excluded — they have their own paths. A `Static` prop is left alone.
/// Client-only.
pub fn maintain_predicted_dynamic(
    role: Res<lunco_core::NetworkRole>,
    mut commands: Commands,
    q_add: Query<
        (Entity, &RigidBody),
        (
            With<lunco_core::NetReplicate>,
            // Wheeled vehicles (a `FlightSoftware` control surface) have their
            // own predict path (`maintain_predicted_vehicles`); a cosim-flown
            // vessel that also carries `FlightSoftware` is already caught by the
            // `NotPredictable` guard below.
            Without<lunco_fsw::FlightSoftware>,
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
        commands.entity(e).try_insert(lunco_core::ContactPredictable);
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
/// (a `FlightSoftware` vehicle you don't possess and don't own) as
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
            // A wheeled vehicle = has a `FlightSoftware` control surface. The
            // `Without<NotPredictable>` guard below excludes cosim-flown vessels
            // (a lander carries `FlightSoftware` too but is `NotPredictable`),
            // so this resolves to exactly the locally-simulated rovers.
            With<lunco_fsw::FlightSoftware>,
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
        commands.entity(e).try_insert(lunco_core::ContactPredictable);
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
    mut q_promoted: Query<
        (Entity, &mut ContactPredictLinger),
        With<lunco_core::PredictedDynamic>,
    >,
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
                        commands
                            .entity(e)
                            .try_insert(PendingCorrection { pos: dpos, rot: rot_err });
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
#[on_command(MoveEntity)]
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
    q_rb: Query<&RigidBody>,
    q_marker: Query<&JustMovedKinematic>,
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
    // replaces it. The original kind is stashed on the marker and
    // restored by `clear_kinematic_pulse_velocity` one tick later —
    // a move stream (gizmo drag) must keep the FIRST capture, or the
    // second move would capture the Kinematic we just inserted and
    // the body would stay Kinematic forever (the pre-2026-07-11 bug:
    // a moved rover hovered in mid-air permanently).
    let restore = match q_marker.get(target) {
        Ok(marker) => marker.restore,
        Err(_) => q_rb
            .get(target)
            .ok()
            .copied()
            .filter(|rb| !matches!(rb, RigidBody::Kinematic)),
    };
    if q_rb.get(target).is_ok() {
        commands.entity(target).try_insert(RigidBody::Kinematic);
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
    commands.entity(target).try_insert(JustMovedKinematic { restore });

    info!(
        "MOVE_ENTITY: {:?} → ({:.3}, {:.3}, {:.3})",
        cmd.entity_id, cmd.translation.x, cmd.translation.y, cmd.translation.z
    );
}

/// Persist a runtime move into the active USD document's **runtime** layer
/// (Phase C4b producer). Observes `MoveEntity` alongside the physics handler
/// [`on_move_entity_command`] but is fully decoupled from it — it touches no
/// physics state.
///
/// Persistence is **guarded to authored-scene entities**: it fires only when the
/// moved entity carries a [`UsdPrimPath`] whose prim is owned by the active USD
/// document (present in its base or runtime layer). Palette/sim spawns that
/// aren't part of the authored scene are skipped, so this never authors stray
/// opinions for entities the document doesn't know about. The op targets the
/// runtime layer, so the move round-trips through the Twin journal and renders
/// via the composed view, while Save stays base-only.
pub fn persist_move_to_runtime_layer(
    trigger: On<MoveEntity>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    let Some((doc, path)) = authorable_prim(target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };

    let v = cmd.translation;
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path,
            value: [v.x as f64, v.y as f64, v.z as f64],
        },
    });
}

// ─────────────────────────────────────────────────────────────────────
// Document history — THE history
//
// The 3D editor has no private undo stack. Every editor mutation is
// authored as a `UsdOp` (the persisters above), so its history is the
// document's history: Lamport-ordered, op+inverse, journaled, networked.
// `UndoDocument`/`RedoDocument` are the generic verbs; each domain observes them
// and acts only on documents its own registry owns. USD's observers live in
// `lunco-usd` (the crate that owns `UsdDocumentRegistry`) — NOT here, so that a
// headless binary with documents but no 3D editor can still undo. The editor's
// only job is to bind the key.
// ─────────────────────────────────────────────────────────────────────

/// Ctrl+Z → undo, Ctrl+Shift+Z / Ctrl+Y → redo, on the **active document**.
///
/// The editor's edits are document ops, so this is the same history the Inspector, the
/// journal and every networked peer see — there is no second, in-memory editor stack to
/// disagree with it.
///
/// Ignored while egui holds the keyboard, so Ctrl+Z in a text field (the rhai editor, a
/// name box) edits the text instead of silently reverting the scene.
pub fn handle_undo_input(
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    if !keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let redo = keys.just_pressed(KeyCode::KeyY) || (shift && keys.just_pressed(KeyCode::KeyZ));
    let undo = !shift && keys.just_pressed(KeyCode::KeyZ);
    if !undo && !redo {
        return;
    }

    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else {
        info!("[undo] no active document — nothing to undo");
        return;
    };
    if redo {
        commands.trigger(RedoDocument { doc });
    } else {
        commands.trigger(UndoDocument { doc });
    }
}

/// The preamble EVERY persister repeats: resolve the active USD document, resolve the
/// entity's prim path, and ownership-guard it against that document.
///
/// Factored out because the duplication was load-bearing: each `persist_*` observer
/// re-derived this by hand, and the ones that forgot to (the transform gizmo, the
/// Inspector's delete) simply mutated the ECS and never reached the document — which
/// is exactly why a gizmo drag used to be invisible to save, undo, the journal and the
/// network. If an edit path can call this, it has no excuse not to author.
///
/// Returns `None` when there is no active USD document (headless, a Modelica doc, no
/// scene), when the entity is not USD-backed, or when its prim belongs to some other
/// document.
pub fn authorable_prim(
    entity: Entity,
    q_prim: &Query<&UsdPrimPath>,
    usd_registry: &UsdDocumentRegistry,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> Option<(lunco_doc::DocumentId, String)> {
    let doc = workspace?.0.active_document?;
    let host = usd_registry.host(doc)?;
    let prim = q_prim.get(entity).ok()?;
    let prim_sdf = lunco_usd_bevy::SdfPath::new(&prim.path).ok()?;
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    owned.then(|| (doc, prim.path.clone()))
}

// ─────────────────────────────────────────────────────────────────────
// DeleteEntity — removal, authored
// ─────────────────────────────────────────────────────────────────────

/// Delete an entity from the scene.
///
/// The typed verb for "remove this", replacing the ad-hoc `world.despawn(entity)` the
/// Inspector used to do in two places. A bare despawn is invisible to the document:
/// the prim survives in the layer, so the deletion never journals, never replicates,
/// never persists, and the next projection can bring the entity straight back.
///
/// This despawns AND (via [`persist_delete_to_runtime_layer`]) authors a `RemovePrim`
/// — which is what makes deletion undoable, because the document hands back an
/// `AddPrim` inverse for free.
// Plain `#[Command]`, not `#[Command(default)]`: `default` derives `Default`, and
// `Entity` has none — the same reason `DetachJoint` above is plain.
#[Command]
pub struct DeleteEntity {
    /// Entity to remove.
    pub target: Entity,
    /// `Persistent` (the default) authors the removal into the document; an
    /// `Interactive` delete is live-only and does not journal.
    #[serde(default)]
    #[reflect(default)]
    pub intent: lunco_core::EditIntent,
}

/// Live leg: despawn the entity and drop it from the selection.
#[on_command(DeleteEntity)]
pub fn on_delete_entity(
    trigger: On<DeleteEntity>,
    mut selected: ResMut<crate::SelectedEntities>,
    mut commands: Commands,
) {
    let _ = trigger;
    commands.entity(cmd.target).try_despawn();
    selected.entities.retain(|e| *e != cmd.target);
}

/// Authoring leg: remove the prim, so the deletion persists, journals, replicates —
/// and undoes. Same shape as every other `persist_*` observer.
pub fn persist_delete_to_runtime_layer(
    trigger: On<DeleteEntity>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if !cmd.intent.is_persistent() {
        return;
    }
    let Some((doc, path)) =
        authorable_prim(cmd.target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
    });
}

/// Persist a `SetObjectProperty` **shader-param tune** into the active USD
/// document's **runtime overlay** (#4 — non-destructive layer tuning).
///
/// [`on_set_object_property`] mutates the live [`ShaderLook`] for immediate
/// feedback but writes nothing back to USD, so a tweak (e.g. a terrain
/// `weight_albedo`) is lost on reload. This decoupled observer authors the same
/// edit as a `SetAttribute` into `LayerId::runtime()` — the session overlay that
/// composes over the base layer and rides the Twin journal / `.lunco/runtime`
/// sidecar, while **Save stays base-only** (the authored `.usda` is never
/// dirtied). It mirrors [`persist_move_to_runtime_layer`]: same ownership guard,
/// same runtime target, fully decoupled from the live-mutation handler.
///
/// Scope: **scalar** params (covers every layer `weight_*` and roughness knob) on
/// entities carrying a [`ShaderLook`] whose prim the active document owns.
/// Colors/vectors and PBR props stay live-only for now.
///
/// The "is this a shader prim?" guard is a CLASSIFICATION query, so it asks the
/// *intent* (`With<ShaderLook>`), not the bound material: the intent exists headless
/// too, where nothing ever binds a `ShaderMaterial`.
pub fn persist_property_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_shader: Query<(), With<ShaderLook>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    // Not shader *params*: `shader` swaps the material (no USD reader — the
    // `shaderPath` attribute was deliberately vetoed, so it stays live-only) and
    // `visible` is authored as standard `token visibility` by
    // [`persist_wheel_and_pbr_to_runtime_layer`]. Disjoint, so neither is
    // double-authored.
    if matches!(cmd.property.as_str(), "shader" | "visible") {
        return;
    }
    // Parse the value into a typed USD attribute. A single float persists as
    // `float`; three comma-separated floats persist as a `color3f` vector — the
    // shape shader colours/vectors (`cell_a`, `tint`, …) take. `read_authored_params`
    // reads BOTH back on reload (vec3 first, then scalar), so both round-trip with
    // no loader change. Any other arity (or a non-numeric value) stays live-only.
    let parts: Vec<&str> = cmd.value.split(',').collect();
    let floats: Vec<f32> = parts.iter().filter_map(|s| s.trim().parse::<f32>().ok()).collect();
    if floats.len() != parts.len() {
        return;
    }
    let (type_name, value) = match floats.len() {
        1 => ("float".to_string(), floats[0].to_string()),
        3 => (
            "color3f".to_string(),
            format!("({}, {}, {})", floats[0], floats[1], floats[2]),
        ),
        _ => return,
    };
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };

    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    // Only shader-look prims (the layer-tuning case) — not PBR ones.
    if q_shader.get(target).is_err() {
        return;
    }
    let Ok(prim) = q_prim.get(target) else { return };

    // Ownership guard: only author for prims the active document actually holds
    // (base or runtime), so palette/sim spawns never get stray opinions.
    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else { return };
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    if !owned {
        return;
    }

    // Author under `primvars:` with the snake_case field name — the same contract
    // `read_authored_params` reads back (which now normalizes camelCase too).
    let name = format!("primvars:{}", lunco_materials::to_snake_case(&cmd.property));
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: prim.path.clone(),
            name,
            type_name,
            value,
        },
    });
}

/// One wheel-dynamics parameter — **the** single source of truth for it.
///
/// A wheel param has exactly three facets and they must never drift apart:
/// the names `SetObjectProperty` accepts for it, the live `WheelRaycast` field
/// it sets, and the USD attribute `lunco_usd_sim` reads back onto that field on
/// load. Two hand-synced tables (a `name → setter` match and a separate
/// `name → attr` match) had already drifted — `slip_stiffness` / `friction_mu`
/// were settable but not persistable, so tuning them was silently lost on
/// reload. One row per param makes that structurally impossible: a field cannot
/// exist in one table and not the other, because there is only one table.
pub(crate) struct WheelParam {
    /// Accepted `SetObjectProperty` names — the Rust field name first, USD-style
    /// aliases after (`radius`, `spring_stiffness`, …).
    pub aliases: &'static [&'static str],
    /// Live setter on `WheelRaycast`. Non-capturing closures coerce to `fn`.
    pub set: fn(&mut lunco_mobility::WheelRaycast, f64),
    /// The USD attribute the loader reads back into this field (`float`).
    pub usd_attr: &'static str,
}

/// Every wheel-dynamics parameter `SetObjectProperty` can tune. Each row's
/// `usd_attr` is a name `lunco_usd_sim`'s wheel loader actually reads, so every
/// tune round-trips through the runtime layer on reload.
pub(crate) const WHEEL_PARAMS: &[WheelParam] = &[
    WheelParam {
        aliases: &["drive_torque", "drive_torque_max"],
        set: |w, v| w.drive_torque_max = v,
        usd_attr: "physxVehicleEngine:peakTorque",
    },
    WheelParam {
        aliases: &["brake_torque", "brake_torque_max"],
        set: |w, v| w.brake_torque_max = v,
        usd_attr: "physxVehicleWheel:maxBrakeTorque",
    },
    WheelParam {
        aliases: &["slip_stiffness"],
        set: |w, v| w.slip_stiffness = v,
        usd_attr: "physxVehicleTire:longitudinalStiffness",
    },
    WheelParam {
        aliases: &["bearing_damping", "damping_rate"],
        set: |w, v| w.bearing_damping = v,
        usd_attr: "physxVehicleWheel:dampingRate",
    },
    WheelParam {
        aliases: &["friction_mu", "friction"],
        set: |w, v| w.friction_mu = v,
        usd_attr: "lunco:frictionCoefficient",
    },
    WheelParam {
        aliases: &["mass"],
        set: |w, v| w.mass = v,
        usd_attr: "physics:mass",
    },
    WheelParam {
        aliases: &["moi", "moment_of_inertia"],
        set: |w, v| w.moment_of_inertia = v,
        usd_attr: "physxVehicleWheel:moi",
    },
    WheelParam {
        aliases: &["wheel_radius", "radius"],
        set: |w, v| w.wheel_radius = v,
        usd_attr: "physxVehicleWheel:radius",
    },
    WheelParam {
        aliases: &["rest_length"],
        set: |w, v| w.rest_length = v,
        usd_attr: "physxVehicleSuspension:restLength",
    },
    WheelParam {
        aliases: &["spring_k", "spring_stiffness"],
        set: |w, v| w.spring_k = v,
        usd_attr: "physxVehicleSuspension:springStiffness",
    },
    WheelParam {
        aliases: &["damping_c", "spring_damping"],
        set: |w, v| w.damping_c = v,
        usd_attr: "physxVehicleSuspension:springDamping",
    },
];

/// Look a `SetObjectProperty` property name up in [`WHEEL_PARAMS`], or `None`
/// if it isn't a wheel field. Both the live-mutation path and the USD-authoring
/// path go through this one lookup.
pub(crate) fn wheel_param(name: &str) -> Option<&'static WheelParam> {
    WHEEL_PARAMS.iter().find(|p| p.aliases.contains(&name))
}

/// Persist a `SetObjectProperty` **wheel-dynamics**, **visibility** or **PBR
/// base-colour** tune into the active USD document's runtime overlay — the
/// counterpart of [`persist_property_to_runtime_layer`] for the property classes
/// it skips (it guards to shader-material prims). Fully decoupled + disjoint: it
/// authors for wheel-param names (via [`wheel_param`]), `visible` (standard USD
/// `token visibility`) or `base_color` on a PBR prim — all of
/// which the loader already reads back, so they round-trip on reload and ride the
/// Twin journal. Ownership-guarded and no-op without an active USD doc, like
/// every other persister.
pub fn persist_wheel_and_pbr_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    // "Is this a PBR (non-shader) prim?" — the `PbrLook` *intent*, which exists
    // headless as well as in a render build (the bound `StandardMaterial` is the
    // binder's business and this crate may not name it).
    q_std_mat: Query<(), With<PbrLook>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // Route the property to a USD attribute the loader reads back.
    let authored: Option<(String, &str, String)> =
        if let Some(param) = wheel_param(&cmd.property) {
            // Wheel dynamics → the single `WHEEL_PARAMS` row's USD attribute.
            cmd.value
                .trim()
                .parse::<f32>()
                .ok()
                .map(|v| (param.usd_attr.to_string(), "float", v.to_string()))
        } else if cmd.property == "visible" {
            // Visibility → standard USD `token visibility`, which the prim
            // instantiator already reads back (`inherited` / `invisible`), so a
            // hide survives reload instead of being a live-only ECS `Visibility`
            // write. A `token` literal is QUOTED in USD.
            let hidden = matches!(cmd.value.trim(), "false" | "0" | "hidden");
            let tok = if hidden { "invisible" } else { "inherited" };
            Some(("visibility".to_string(), "token", format!("\"{tok}\"")))
        } else if cmd.property == "base_color" {
            // PBR base colour → `primvars:displayColor` (the loader reads it back
            // into the prim's `PbrLook`). Linear r,g,b (drop any alpha).
            let f: Vec<f32> = cmd
                .value
                .split(',')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect();
            // ARRAY-valued: `UsdGeomGprim` declares `color3f[] primvars:displayColor`
            // with `constant` interpolation — one element for the whole prim. A
            // scalar `color3f` here is a type mismatch every other DCC falls back
            // to grey on.
            (f.len() >= 3).then(|| {
                (
                    "primvars:displayColor".to_string(),
                    "color3f[]",
                    format!("[({}, {}, {})]", f[0], f[1], f[2]),
                )
            })
        } else {
            None
        };
    let Some((name, type_name, value)) = authored else { return };

    // `base_color` only applies to PBR prims; wheel params resolve
    // regardless (the guard is cheap and disjoint from the shader persister).
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    if cmd.property == "base_color" && q_std_mat.get(target).is_err() {
        return;
    }
    let Ok(prim) = q_prim.get(target) else { return };

    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else { return };
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    if !owned {
        return;
    }

    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: prim.path.clone(),
            name,
            type_name: type_name.to_string(),
            value,
        },
    });
}

/// Persist a `SetEnvironmentLight` sun tweak into the active USD document's
/// runtime overlay — the environment twin of [`persist_property_to_runtime_layer`].
///
/// [`lunco_environment::on_set_environment_light`] mutates the live
/// `DirectionalLight` for immediate feedback but writes nothing back to USD, so a
/// sun tweak is lost on reload. This decoupled observer authors the changed
/// fields as `SetAttribute`s onto the sun's `DistantLight` prim in
/// `LayerId::runtime()`, using the SAME attribute names the loader
/// (`lunco_usd_bevy::light`) already reads back — so illuminance / colour /
/// shadow-range knobs round-trip on reload and ride the Twin journal like every
/// other USD edit. (Live peer-sync then follows the USD projection, exactly as
/// the move / property persisters do — no bespoke light broadcast.)
///
/// Scope: the fields with an existing loader reader. Sun **direction** (needs a
/// rotation-authoring op — there is no `SetRotate` yet) and the render-only knobs
/// (exposure / bloom / earthshine / ambient — no `DistantLight` attribute reads
/// them back yet) stay live-only for now.
///
/// Targets every non-earthshine `DistantLight` the active document owns
/// (`SetEnvironmentLight` itself is global). Ownership-guarded like the other
/// persisters; no-op when no USD doc is active (headless).
pub fn persist_environment_light_to_runtime_layer(
    trigger: On<lunco_environment::SetEnvironmentLight>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_sun: Query<
        (&UsdPrimPath, &Transform),
        (
            With<lunco_usd_bevy::UsdAuthoredLight>,
            With<DirectionalLight>,
            Without<lunco_environment::Earthshine>,
        ),
    >,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };

    // Collect only the fields that HAVE a matching loader reader, so every attr
    // authored here round-trips on reload (name, USD type, USD-literal value).
    let mut attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some(lux) = cmd.illuminance {
        attrs.push(("inputs:intensity", "float", lux.to_string()));
    }
    if let Some([r, g, b]) = cmd.sun_color {
        attrs.push(("inputs:color", "color3f", format!("({r}, {g}, {b})")));
    }
    if let Some(v) = cmd.shadow_max_distance {
        attrs.push(("lunco:shadow:maxDistance", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_first_cascade_bound {
        attrs.push(("lunco:shadow:firstCascadeFarBound", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_depth_bias {
        attrs.push(("lunco:shadow:depthBias", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_normal_bias {
        attrs.push(("lunco:shadow:normalBias", "float", v.to_string()));
    }
    // Direction changes when yaw or pitch is specified.
    let direction_changed = cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some();
    if attrs.is_empty() && !direction_changed {
        return;
    }

    for (prim, tf) in &q_sun {
        // Ownership guard: only author for suns the active document actually
        // holds (base or runtime), so an engine-fallback sun never gets opinions.
        let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else {
            continue;
        };
        let owned = host.document().data().spec(&prim_sdf).is_some()
            || host.document().runtime_data().spec(&prim_sdf).is_some();
        if !owned {
            continue;
        }
        for (name, type_name, value) in &attrs {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: prim.path.clone(),
                    name: (*name).to_string(),
                    type_name: (*type_name).to_string(),
                    value: value.clone(),
                },
            });
        }
        // Sun direction → `xformOp:rotateXYZ` via the new `SetRotate` op. Compute
        // the SAME final orientation the live handler does — YXZ yaw/pitch, the
        // unspecified axis kept from the current transform — then express it as
        // Euler XYZ **degrees** for USD. (Reading `cur` from the transform is
        // order-independent w.r.t. the live handler: a specified axis overrides
        // `cur`; an unspecified one the live handler leaves unchanged, so `cur`
        // is the same value either way.) Uses the runtime-overlay layer, exactly
        // like `persist_move_to_runtime_layer` does for translate.
        if direction_changed {
            let (cur_yaw, cur_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let yaw = cmd.sun_yaw.unwrap_or(cur_yaw);
            let pitch = cmd.sun_pitch.unwrap_or(cur_pitch);
            let quat = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            let (rx, ry, rz) = quat.to_euler(EulerRot::XYZ);
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetRotate {
                    edit_target: LayerId::runtime(),
                    path: prim.path.clone(),
                    value: [
                        rx.to_degrees() as f64,
                        ry.to_degrees() as f64,
                        rz.to_degrees() as f64,
                    ],
                },
            });
        }
    }

    // Render knobs (exposure / bloom / ambient / earthshine) have no natural
    // light-prim home — they apply to global/camera state — so per the schema
    // decision they persist onto a dedicated `LuncoEnvironment` settings prim
    // (a singleton under the default prim). A projector in `lunco-sandbox` reads
    // them back on stage change and applies them, so the light loader stays pure.
    let mut env_attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some(v) = cmd.exposure_ev100 {
        env_attrs.push(("lunco:env:exposureEv100", "float", v.to_string()));
    }
    if let Some(v) = cmd.bloom_intensity {
        env_attrs.push(("lunco:env:bloomIntensity", "float", v.to_string()));
    }
    if let Some(v) = cmd.ambient_brightness {
        env_attrs.push(("lunco:env:ambientBrightness", "float", v.to_string()));
    }
    if let Some(v) = cmd.earthshine_illuminance {
        env_attrs.push(("lunco:env:earthshineIntensity", "float", v.to_string()));
    }
    if let Some([r, g, b]) = cmd.earthshine_color {
        env_attrs.push((
            "lunco:env:earthshineColor",
            "color3f",
            format!("({r}, {g}, {b})"),
        ));
    }
    if !env_attrs.is_empty() {
        let parent_path = lunco_usd_bevy::layer_default_prim(host.document().data())
            .map(|p| format!("/{p}"))
            .unwrap_or_else(|| "/".to_string());
        let env_path = if parent_path == "/" {
            "/Environment".to_string()
        } else {
            format!("{parent_path}/Environment")
        };
        // Ensure the settings prim exists, but only author `AddPrim` when it's
        // actually absent (else every render tweak would journal a redundant
        // AddPrim). Idempotent thereafter — SetAttribute overwrites in place.
        let exists = lunco_usd_bevy::SdfPath::new(&env_path)
            .ok()
            .map(|sdf| {
                host.document().data().spec(&sdf).is_some()
                    || host.document().runtime_data().spec(&sdf).is_some()
            })
            .unwrap_or(false);
        if !exists {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::AddPrim {
                    edit_target: LayerId::runtime(),
                    parent_path,
                    name: "Environment".to_string(),
                    type_name: Some(lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE.to_string()),
                    reference: None,
                },
            });
        }
        for (name, type_name, value) in &env_attrs {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: env_path.clone(),
                    name: (*name).to_string(),
                    type_name: (*type_name).to_string(),
                    value: value.clone(),
                },
            });
        }
    }
}

/// Persist a runtime **spawn** into the active USD document's runtime layer
/// (Phase C4b producer). Observes `SpawnEntity` alongside the ECS spawn handler
/// [`on_spawn_entity_command`] but is fully decoupled from it — it touches no
/// world/entity state.
///
/// A spawn is recorded as a runtime prim that **`references` the spawned asset**
/// (`AddPrim{edit_target: runtime, reference}`) plus its drop position
/// (`SetTranslate{edit_target: runtime}`). The reference + transform compose
/// into the document's rendered/serialized view and ride the Twin journal, so
/// the spawn survives in session history and the composed scene — while Save
/// stays base-only (the runtime layer is never written to disk). Persisting is
/// gated to when a USD document is active; palette spawns with no active doc
/// (e.g. a headless server) are skipped.
pub fn persist_spawn_to_runtime_layer(
    trigger: On<SpawnEntity>,
    catalog: Res<SpawnCatalog>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    role: Res<lunco_core::NetworkRole>,
    // Monotonic per-session disambiguator for spawn prim names (the runtime
    // layer isn't persisted, so session scope is enough).
    mut spawn_seq: Local<u32>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    // Single-instantiation: `on_spawn_entity_command` ALWAYS directly instantiates
    // the spawn as an ECS entity (non-client). If we ALSO author it into the doc's
    // runtime layer here, the twin projection re-instantiates it as a SECOND entity
    // — the "double-instantiation" (two overlapping rovers; id-reuse then clobbers
    // one on doc reload). In a `Standalone` session there is no networked/web client
    // that needs the journal-authored copy, so the direct ECS spawn is the sole,
    // authoritative instance — skip persistence and let it be the only rover. (In a
    // `Host` session the runtime-layer op is still the channel a web client learns
    // the spawn from, so persistence stays; de-duplicating THAT double needs the
    // networking-aware fix and is handled separately.)
    if matches!(*role, lunco_core::NetworkRole::Standalone) {
        return;
    }
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };
    let Some(entry) = catalog.get(&cmd.entry_id) else { return };
    // The asset this spawn references (the only `SpawnSource` variant today).
    let SpawnSource::UsdFile(asset_path) = &entry.source;

    // Parent under the document's default prim (scene root) when it has one,
    // else at the stage root. `stage_default_prim` returns the bare prim name.
    let parent_path = lunco_usd_bevy::layer_default_prim(host.document().data())
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());

    // Unique, valid USD identifier for the spawn prim.
    *spawn_seq += 1;
    let stem: String = cmd
        .entry_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = format!("{stem}_{}", *spawn_seq);
    let prim_path = if parent_path == "/" {
        format!("/{name}")
    } else {
        format!("{parent_path}/{name}")
    };

    let v = cmd.position;
    // 1) Author the referenced spawn prim into the runtime layer.
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path,
            name,
            type_name: None,
            reference: Some(asset_path.clone()),
        },
    });
    // 2) Record its drop position (applied after the AddPrim above).
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path: prim_path,
            value: [v.x as f64, v.y as f64, v.z as f64],
        },
    });
}

/// Marker inserted on a kinematic body that just received a
/// `MoveEntity` (or analogous teleport) with a one-tick velocity
/// pulse. [`clear_kinematic_pulse_velocity`] zeros that velocity
/// the frame after the pulse so the body doesn't drift.
#[derive(Component)]
pub struct JustMovedKinematic {
    /// The body kind to put back after the pulse tick — the Kinematic
    /// forced by `on_move_entity_command` is only "for the duration of
    /// the move". `None` = the body was already Kinematic (or has no
    /// RigidBody): restore nothing.
    pub restore: Option<RigidBody>,
}

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
    mut q: Query<(Entity, &mut LinearVelocity, &JustMovedKinematic)>,
) {
    for (e, mut vel, marker) in q.iter_mut() {
        vel.0 = DVec3::ZERO;
        // Put the pre-move body kind back ("for the duration of the move").
        // Re-inserting RigidBody goes through avian's replace hook, which
        // wakes the island — a body released in mid-air falls.
        if let Some(kind) = marker.restore {
            commands.entity(e).try_insert(kind);
        }
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
/// - `shader` → author a [`ShaderLook`] for that `.wgsl` (asset path); the render
///   binder turns it into a material.
/// - any parameter named by the shader's `Material` struct (e.g. `albedo`,
///   `wedge_count`, `cell_a`) → set that named value on the entity's `ShaderLook`
///   (requires `shader` set first, or a USD shader material). The shader's
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

/// The `SetObjectProperty` PBR keys [`PbrLook`] can express.
///
/// These go through the **appearance-intent component**, not a material asset:
/// mutating `PbrLook` is enough, because `lunco-render-bevy`'s `Changed<PbrLook>`
/// binder re-materialises the entity. There is no longer an `Assets<StandardMaterial>`
/// fallback — an in-place asset write would have been actively wrong anyway (the
/// binder's handles are *shared by look*, so it would bleed onto every other entity
/// that looks the same), and naming the material would drag `bevy_pbr` (wgpu, naga)
/// into the headless server that links this file.
const PBR_LOOK_KEYS: &[&str] = &[
    "base_color",
    "emissive",
    "metallic",
    "roughness",
    "perceptual_roughness",
    "reflectance",
    "alpha",
    "opacity",
    "unlit",
    "double_sided",
];

/// Apply one PBR property addressed by `SetObjectProperty` to a [`PbrLook`] —
/// appearance **intent**, no material asset touched.
///
/// Value formats: colors are comma-separated **linear** `r,g,b[,a]` in 0..1 (so they
/// round-trip the Inspector's `color_edit_button_rgb`); scalars a single float;
/// booleans `true`/`1`/`yes`/`on`. Only the keys in [`PBR_LOOK_KEYS`] are understood;
/// anything else returns `false`.
fn apply_pbr_look(look: &mut PbrLook, key: &str, value: &str) -> bool {
    let f: Vec<f32> = value
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    let parse_bool = |v: &str| matches!(v.trim(), "true" | "1" | "yes" | "on");
    match key {
        "base_color" => {
            if f.len() < 3 {
                return false;
            }
            let a = f.get(3).copied().unwrap_or(look.base_color.alpha);
            look.base_color = LinearRgba::new(f[0], f[1], f[2], a);
        }
        "emissive" => {
            if f.len() < 3 {
                return false;
            }
            look.emissive = LinearRgba::new(f[0], f[1], f[2], f.get(3).copied().unwrap_or(1.0));
        }
        "metallic" => {
            let Some(v) = f.first() else { return false };
            look.metallic = v.clamp(0.0, 1.0);
        }
        "roughness" | "perceptual_roughness" => {
            let Some(v) = f.first() else { return false };
            look.perceptual_roughness = v.clamp(0.0, 1.0);
        }
        "reflectance" => {
            let Some(v) = f.first() else { return false };
            look.reflectance = v.clamp(0.0, 1.0);
        }
        "alpha" | "opacity" => {
            let Some(v) = f.first() else { return false };
            let v = v.clamp(0.0, 1.0);
            look.base_color.alpha = v;
            look.alpha = if v >= 1.0 { SurfaceAlpha::Opaque } else { SurfaceAlpha::Blend };
        }
        "unlit" => look.unlit = parse_bool(value),
        "double_sided" => look.double_sided = parse_bool(value),
        _ => return false,
    }
    true
}

/// The reflected parameter schema of a shader **asset path**.
///
/// Read straight out of the loaded WGSL source (`Material` struct + `//!@`
/// annotations) rather than off a material — the schema is a property of the
/// *asset*, and reading it this way keeps the shader-param paths render-free.
/// `None` while the shader is still loading (or if it declares no `Material`), in
/// which case callers infer the type from the value's arity, exactly as the old
/// material path did with its empty default schema.
fn shader_schema(
    path: &str,
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
) -> Option<ParamSchema> {
    let handle = asset_server.load::<bevy::shader::Shader>(path.to_string());
    let src = match &shaders.get(&handle)?.source {
        bevy::shader::Source::Wgsl(s) => s.as_ref().to_string(),
        _ => return None,
    };
    ParamSchema::parse(&src)
}

/// Parse one `SetObjectProperty` value into a typed [`ParamValue`] for `key`.
///
/// Same grammar (and the same type resolution) as the former
/// `lunco_materials::apply_param`: the field's type comes from the shader's
/// reflected schema when it is known, else from the value's arity; a vector field
/// takes `r,g,b` and is stored as a `Vec4` with alpha 1, which is what
/// `ShaderMaterial::set_color` did and what the shader's uniform block expects.
fn shader_param_value(schema: Option<&ParamSchema>, key: &str, value: &str) -> Option<ParamValue> {
    let ty = schema.and_then(|s| s.field(key)).map(|f| f.ty).unwrap_or_else(|| {
        match value.split(',').filter(|s| !s.trim().is_empty()).count() {
            0 | 1 => ParamType::F32,
            2 => ParamType::Vec2,
            3 => ParamType::Vec3,
            _ => ParamType::Vec4,
        }
    });
    match ty {
        ParamType::Vec3 | ParamType::Vec4 => {
            let n: Vec<f32> =
                value.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
            (n.len() >= 3).then(|| ParamValue::Vec4([n[0], n[1], n[2], 1.0]))
        }
        _ => ParamValue::parse(ty, value),
    }
}

/// Give `target` a [`ShaderLook`] for `shader_path`, carrying over any params it
/// already had (so swapping the `.wgsl` keeps tuned values — what cloning the old
/// `ShaderMaterial` as a template used to do).
///
/// Drops the [`PbrLook`] intent: an entity that carries both draws twice, because
/// each binder materialises its own. See `lunco-render-bevy`'s caller contract.
pub(crate) fn author_shader_look(
    commands: &mut Commands,
    target: Entity,
    existing: Option<&ShaderLook>,
    shader_path: &str,
) {
    let mut look = existing.cloned().unwrap_or_default();
    look.shader = shader_path.to_string();
    commands.entity(target).remove::<PbrLook>().try_insert(look);
    commands.queue(move |world: &mut World| drop_bound_pbr_material(world, target));
}

/// Drop the concrete PBR material a render build already bound to `e`.
///
/// Removing the [`PbrLook`] *intent* stops the binder re-materialising the entity,
/// but the `MeshMaterial3d<StandardMaterial>` it inserted earlier stays put — and a
/// mesh carrying that AND the shader material draws twice. That component is
/// `bevy_pbr`'s and this crate may not name it (render-decoupling rule), so it is
/// resolved out of the type registry instead (`MaterialPlugin` registers it, and it
/// is `#[reflect(Component)]`).
///
/// No-op headless and in tests, where nothing ever bound a material — and a no-op the
/// day `lunco-render-bevy` grows an `On<Remove, PbrLook>` observer that unbinds its
/// own material, which is where this really belongs.
pub(crate) fn drop_bound_pbr_material(world: &mut World, e: Entity) {
    let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else { return };
    let reflect_component = {
        let reg = registry.read();
        reg.get_with_short_type_path("MeshMaterial3d<StandardMaterial>")
            .and_then(|r| r.data::<bevy::ecs::reflect::ReflectComponent>())
            .cloned()
    };
    let Some(rc) = reflect_component else { return };
    if let Ok(mut entity) = world.get_entity_mut(e) {
        rc.remove(&mut entity);
    }
}

/// Observer for [`SetObjectProperty`].
#[on_command(SetObjectProperty)]
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    shaders: Res<Assets<bevy::shader::Shader>>,
    mut q_look: Query<&mut PbrLook>,
    mut q_shader_look: Query<&mut ShaderLook>,
    q_mesh: Query<(), With<Mesh3d>>,
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
    if let Some(param) = wheel_param(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!("SET_PROPERTY: '{}' expects a number, got '{}'", cmd.property, cmd.value);
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        (param.set)(&mut wheel, value);
        info!("SET_PROPERTY: wheel {} {} = {}", cmd.entity_id, cmd.property, value);
        return;
    }

    match cmd.property.as_str() {
        "shader" => {
            // Preserve existing uniforms if the object already has a shader look,
            // so swapping the .wgsl keeps tuned params.
            let existing = q_shader_look.get(target).ok().cloned();
            author_shader_look(&mut commands, target, existing.as_ref(), &cmd.value);
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
        // PBR properties — for props/rovers on a plain surface rather than a custom
        // shader. Explicit arm ([`PBR_LOOK_KEYS`]) so these names never get stolen by
        // the shader-param fallback below.
        //
        // The edit is a mutation of the entity's `PbrLook` *intent* component: the
        // render binder's `Changed<PbrLook>` system re-materialises it, so "edit the
        // material" is just "mutate a component" — no asset handles, and it works
        // headless (the intent is in the world; nothing binds it). A mesh with no
        // intent yet (a glTF import that brought its own material) is ADOPTED into an
        // intent, which is the only render-free way to keep this command working on
        // it; note that adoption starts from `PbrLook::default()`, so the import's own
        // textures are not carried over.
        key if PBR_LOOK_KEYS.contains(&key) => {
            if let Ok(mut look) = q_look.get_mut(target) {
                if apply_pbr_look(&mut look, key, &cmd.value) {
                    info!("SET_PROPERTY: {} look {} = {}", cmd.entity_id, cmd.property, cmd.value);
                } else {
                    warn!("SET_PROPERTY: bad value '{}' for pbr '{}'", cmd.value, cmd.property);
                }
                return;
            }
            if q_mesh.get(target).is_err() {
                warn!("SET_PROPERTY: entity {} has no PbrLook / mesh", cmd.entity_id);
                return;
            }
            let mut look = PbrLook::default();
            if apply_pbr_look(&mut look, key, &cmd.value) {
                commands.entity(target).try_insert(look);
                info!("SET_PROPERTY: {} adopted a PbrLook, {} = {}", cmd.entity_id, cmd.property, cmd.value);
            } else {
                warn!("SET_PROPERTY: bad value '{}' for pbr '{}'", cmd.value, cmd.property);
            }
        }
        key => {
            // param/color → set the named value on the entity's shader look. The
            // binder swaps in the material for the new look (`Changed<ShaderLook>`).
            let Ok(mut look) = q_shader_look.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no shader look — set 'shader' first", cmd.entity_id);
                return;
            };
            // USD authors params camelCase, WGSL declares them snake_case.
            let name = lunco_materials::to_snake_case(key);
            let schema = shader_schema(&look.shader, &asset_server, &shaders);
            match shader_param_value(schema.as_ref(), &name, &cmd.value) {
                Some(v) => {
                    look.values.insert(name, v);
                }
                None => warn!("SET_PROPERTY: unknown property '{}'", key),
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

/// A focus request recorded by [`on_focus_entity_by_id`] and applied by
/// [`apply_pending_focus`] at the start of the NEXT frame (`First` schedule).
///
/// The command observer fires wherever the API dispatcher happens to sit in
/// the frame — including BETWEEN transform-propagation passes, where the
/// target's and the avatar's `GlobalTransform`s are momentarily in different
/// conventions (a site-anchored scene re-bases the solar hierarchy every
/// tick). Doing the math there teleported the avatar ~1e11 m into empty space
/// ("click on Earth → everything vanishes"). In `First`, nothing has written a
/// transform yet this frame, so last frame's fully-propagated GTs are
/// mutually consistent by construction.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PendingFocus {
    pub target: Entity,
    pub distance: f32,
}

/// Observer: validate + record the focus; all spatial math happens in
/// [`apply_pending_focus`].
#[on_command(FocusEntityById)]
pub fn on_focus_entity_by_id(
    trigger: On<FocusEntityById>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("FOCUS_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    commands.insert_resource(PendingFocus { target, distance: cmd.distance });
    info!("FOCUS_ENTITY: queued focus on {target:?} at {} m", cmd.distance);
}

/// Applies a [`PendingFocus`] with frame-consistent transforms (`First`
/// schedule — see the type doc).
pub fn apply_pending_focus(
    pending: Option<Res<PendingFocus>>,
    q_target: Query<&GlobalTransform>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            &GlobalTransform,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_celestial: Query<(), With<lunco_celestial::CelestialBody>>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    let Some(pending) = pending else { return };
    let (target, distance) = (pending.target, pending.distance);
    // Celestial bodies are ORBIT-scale targets: hand them to the avatar's
    // `FocusTarget` flow (OrbitCamera flies to the body's grid — doc 47
    // Phase 6 — with sunlit-side arrival). Local framing stays for
    // metre-scale subjects (wheels, rovers, props).
    if q_celestial.get(target).is_ok() {
        commands.remove_resource::<PendingFocus>();
        commands.trigger(lunco_avatar::FocusTarget { avatar: None, target });
        info!("FOCUS_ENTITY: celestial target {target:?} → orbit focus");
        return;
    }
    // Local framing from an orbital view: deactivate the mode and let
    // `orbital_exit_restore_system` migrate the camera back to the parked
    // surface pose this frame. `PendingFocus` is deliberately NOT consumed —
    // the framing re-runs next frame from the restored state, where the GT
    // delta math below is surface-convention again (running it now would
    // compute a pose in the CELESTIAL grid the camera is still parented to,
    // which the restore would then clobber).
    if let Some(pin) = orbital_pin.as_mut() {
        if pin.active {
            pin.active = false;
            info!("FOCUS_ENTITY: leaving orbital view first; focus retries next frame");
            return;
        }
    }
    commands.remove_resource::<PendingFocus>();
    let cmd = FocusEntityById { entity_id: 0, distance };
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
    let Some((avatar_ent, mut tf, mut cell, child_of, avatar_gt, ff_opt)) = q_avatar.iter_mut().next() else {
        warn!("FOCUS_ENTITY: no Avatar entity in the scene (count={avatar_count})");
        return;
    };
    // Work in the avatar→target DELTA, not the target's absolute
    // `GlobalTransform`. Both GTs are read in the same instant so whatever
    // convention/origin big_space happens to be mid-way through this frame
    // (site-anchored scenes re-base every tick) cancels in the difference —
    // reading the target GT alone teleported the avatar 1e11 m into empty
    // space when the observer fired between propagation passes. The delta is
    // applied to the avatar's LOCAL translation, which is valid because the
    // avatar's parent grid (WorldGrid) is unrotated wrt render space.
    let delta = target_gt.translation() - avatar_gt.translation();
    let dist = if cmd.distance > 0.1 { cmd.distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see
    // the spoke face) plus a little up and forward. (Celestial targets never
    // reach here — they take the orbit-focus early return above.)
    let dir = Vec3::new(1.0, 0.4, 0.25).normalize();
    let offset = dir * dist;
    // Grid-frame absolute target = camera's CELL-AWARE position + GT delta.
    // A previous orbit focus leaves the avatar cells away from the scene;
    // `tf.translation` alone is only the cell remainder there. Re-split the
    // final pose through the grid so a local focus also RESETS the cell (for
    // scene-scale positions `translation_to_grid` returns cell (0,0,0) + the
    // plain translation — the historical single-cell convention).
    if let Ok(grid) = q_grids.get(child_of.parent()) {
        let target_abs = grid.grid_position_double(&cell, &tf) + delta.as_dvec3();
        let (new_cell, new_translation) = grid.translation_to_grid(target_abs + offset.as_dvec3());
        *cell = new_cell;
        tf.translation = new_translation;
    } else {
        tf.translation = tf.translation + delta + offset;
    }
    // Aim back along the framing offset (camera → target).
    let d = (-offset).normalize();
    let (yaw, pitch) = ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin());
    match ff_opt {
        // Free-flight rebuilds rotation from yaw/pitch every frame (YXZ euler), so
        // when it's present we must set those rather than the Transform rotation.
        Some(mut ff) => {
            ff.yaw = yaw;
            ff.pitch = pitch;
        }
        // Non-freeflight camera mode (orbit/spring/surface): the framing is
        // AUTHORITATIVE — leaving the old mode attached lets its system fly
        // the camera right back (an OrbitCamera on Earth reclaimed the camera
        // one frame after "focus rover" and the view never returned). Strip
        // the mode and reinstate free flight at the computed aim.
        None => {
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            commands
                .entity(avatar_ent)
                .remove::<lunco_avatar::OrbitCamera>()
                .remove::<lunco_avatar::OrbitFrameSample>()
                .remove::<lunco_avatar::SunlitArrival>()
                .remove::<lunco_avatar::SpringArmCamera>()
                .remove::<lunco_avatar::ChaseCamera>()
                .remove::<lunco_avatar::SurfaceCamera>()
                .remove::<lunco_avatar::SurfaceRelativeMode>()
                .remove::<lunco_avatar::FrameBlend>()
                .try_insert(lunco_avatar::FreeFlightCamera { yaw, pitch, damping: None });
        }
    }
    info!(
        "FOCUS_ENTITY: framed api_id={} at {:.1} m (avatars={avatar_count})",
        cmd.entity_id, dist
    );
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance.
///
/// Authoritative: whatever camera mode the avatar is in (orbit focus on a
/// planet, spring-arm follow, surface mode), this strips it and reinstates a
/// `FreeFlightCamera` at the requested pose — an API client asking for a
/// specific view must always get it. `eye` is split into cell + local
/// translation through the avatar's parent grid, so it lands in the scene
/// frame even when a previous orbit focus left the camera cells away.
#[Command(default)]
pub struct SetCameraLookAt {
    pub eye: Vec3,
    pub target: Vec3,
}

/// Observer for [`SetCameraLookAt`].
#[on_command(SetCameraLookAt)]
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_world_grid: Query<Entity, With<lunco_core::WorldGrid>>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    let cmd = trigger.event();
    let Some((entity, mut tf, mut cell, child_of, ff_opt)) = q_avatar.iter_mut().next() else {
        warn!("SET_CAMERA: no Avatar entity in the scene");
        return;
    };
    // An explicit camera pose is a SURFACE-frame request: leave any orbital
    // view first so `eye`/`target` mean scene coordinates again.
    let was_orbital = orbital_pin.as_mut().is_some_and(|pin| {
        let a = pin.active;
        if a {
            pin.active = false;
        }
        a
    });
    if was_orbital {
        // The camera flew to the focused body's grid; bring it home in one
        // atomic migration — raw cell/translation writes below would be
        // interpreted in the CELESTIAL grid's frame. Removing the marker
        // keeps `orbital_exit_restore_system` from overriding this pose.
        commands
            .entity(entity)
            .remove::<lunco_avatar::OrbitalViewCamera>();
        if let Some(root) = q_world_grid.iter().next() {
            if let Ok(grid) = q_grids.get(root) {
                let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
                lunco_core::attach::migrate_to_grid(
                    &mut commands,
                    entity,
                    root,
                    new_cell,
                    Transform::from_translation(new_translation).with_rotation(tf.rotation),
                );
            }
        }
    } else if let Ok(grid) = q_grids.get(child_of.parent()) {
        let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
        *cell = new_cell;
        tf.translation = new_translation;
    } else {
        tf.translation = cmd.eye;
    }
    let look = cmd.target - cmd.eye;
    let (yaw, pitch) = if look.length() > 1e-4 {
        let d = look.normalize();
        ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin())
    } else {
        let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
        (y, p)
    };
    if let Some(mut ff) = ff_opt {
        ff.yaw = yaw;
        ff.pitch = pitch;
    } else {
        commands
            .entity(entity)
            .remove::<lunco_avatar::OrbitCamera>()
            .remove::<lunco_avatar::OrbitFrameSample>()
            .remove::<lunco_avatar::SunlitArrival>()
            .remove::<lunco_avatar::SpringArmCamera>()
            .remove::<lunco_avatar::ChaseCamera>()
            .remove::<lunco_avatar::SurfaceCamera>()
            .remove::<lunco_avatar::SurfaceRelativeMode>()
            .remove::<lunco_avatar::FrameBlend>()
            .try_insert(lunco_avatar::FreeFlightCamera { yaw, pitch, damping: None });
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
#[on_command(ReloadShader)]
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
#[on_command(SetShaderSource)]
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut registry: ResMut<crate::shader_doc::ShaderRegistry>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
) {
    let ev = trigger.event();
    if ev.path.is_empty() || ev.source.is_empty() {
        warn!("SET_SHADER_SOURCE: empty path or source");
        return;
    }
    // Record the edit into the Twin journal (`DomainKind::Shader`) via the shader
    // document registry — so it SYNCS + PERSISTS like a rhai/Modelica edit, not
    // just a local `Assets<Shader>` poke. Skip recording when this arrived from the
    // wire (`SyncApplyGuard` set): the originating peer already journaled it, and
    // the journal replay leg applies + hot-reloads it here — re-recording would
    // duplicate the entry.
    if guard.map_or(true, |g| g.0.is_none()) {
        registry.apply_source(&ev.path, ev.source.clone());
    }
    // Hot-reload: `load` returns the handle every material already holds, so
    // overwriting that asset id propagates the recompile to them.
    let handle = asset_server.load::<bevy::shader::Shader>(ev.path.clone());
    let shader = bevy::shader::Shader::from_wgsl(ev.source.clone(), ev.path.clone());
    let _ = shaders.insert(handle.id(), shader);
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
    q_look: &Query<&ShaderLook>,
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
                // Intent, not material: the binder loads the same `asset_path` we
                // just inserted the compiled source under, so it renders at once.
                author_shader_look(commands, ent, q_look.get(ent).ok(), &asset_path);
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
#[on_command(CreateShader)]
pub fn on_create_shader(
    trigger: On<CreateShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
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
        &q_look,
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
#[on_command(ImportShader)]
pub fn on_import_shader(
    trigger: On<ImportShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
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
            &q_look,
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
#[on_command(RescanShaders)]
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
#[on_command(DeleteShader)]
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

/// Replace the flat USD-authored ground once an obstacle field exists: author a
/// `RemovePrim` op so the `Ground` prim leaves the active stage (the Twin document
/// stays consistent — a reload won't re-spawn it), and despawn its ECS projection
/// immediately so the generated heightfield is the only ground collider (also on
/// the headless server, where no viewport rebuild fires from the doc edit).
///
/// Lives here (not in the pure `lunco-obstacle-field` generator) because authoring
/// stage ops needs USD access. Change-driven via [`obstacle_field_scene_changed`]:
/// it scans only on frames where a field or a USD prim was just added, never
/// per-frame for the app lifetime.
fn remove_legacy_ground_prim(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    registry: Res<UsdDocumentRegistry>,
    ground: Query<(Entity, &UsdPrimPath)>,
) {
    for (entity, prim) in &ground {
        // The USD loader names entities by full prim path (e.g.
        // `/SandboxScene/Ground`); match the leaf. Generated terrain carries no
        // `UsdPrimPath`, so it can never match here.
        if prim.path.rsplit('/').next() != Some("Ground") {
            continue;
        }
        if let Some(doc) = doc_for_stage(&prim.stage_handle, &asset_server, &registry) {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::RemovePrim {
                    edit_target: LayerId::root(),
                    path: prim.path.clone(),
                },
            });
        }
        commands.entity(entity).try_despawn();
    }
}

/// Run condition for [`remove_legacy_ground_prim`]: a field exists and either it
/// or a USD prim was just added this frame. Handles both spawn orderings (field
/// before ground, ground before field) while keeping the system off every other
/// frame.
fn obstacle_field_scene_changed(
    fields_now: Query<(), With<ObstacleFieldRoot>>,
    fields_added: Query<(), Added<ObstacleFieldRoot>>,
    prims_added: Query<(), Added<UsdPrimPath>>,
) -> bool {
    !fields_now.is_empty() && (!fields_added.is_empty() || !prims_added.is_empty())
}

/// Resolve the open document that owns `stage_handle` by matching the stage asset
/// path against the registry (headless-safe — no viewport dependency).
fn doc_for_stage(
    stage_handle: &Handle<UsdStageAsset>,
    asset_server: &AssetServer,
    registry: &UsdDocumentRegistry,
) -> Option<DocumentId> {
    let asset_path = asset_server.get_path(stage_handle.id())?;
    let path_str = asset_path.path().to_string_lossy().into_owned();
    registry.ids().find(|id| {
        registry.host(*id).is_some_and(|h| match h.document().origin() {
            DocumentOrigin::File { path, .. } => path.to_string_lossy().ends_with(&path_str),
            _ => false,
        })
    })
}

// Generates `register_all_commands(app)` — every `#[Command]` this module owns,
// each wired type + observer together. `persist_*_to_runtime_layer` are NOT here:
// they are additional observers on the same verbs (the journaling/runtime-layer
// leg), not the command handlers, so they stay plain `add_observer`s.
register_commands!(
    on_create_shader,
    on_delete_entity,
    on_delete_shader,
    on_detach_joint,
    on_focus_entity_by_id,
    on_import_shader,
    on_move_entity_command,
    on_reload_shader,
    on_rescan_shaders,
    on_rescan_spawn_catalog,
    on_set_camera_look_at,
    on_set_object_property,
    on_set_shader_source,
    on_set_visual_lead,
    on_spawn_entity_command,
);

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        // Every `#[Command]` this crate owns — type + observer in one call, so a
        // verb can't end up observable-but-unconstructible (the old split wired
        // the observer by hand and then patched the type registry separately, and
        // whenever the second half was forgotten the command silently vanished
        // from the HTTP API / rhai / `discover_schema`).
        register_all_commands(app);
        // Dock release as an actuator on the intent→port machinery (replaces the
        // hardcoded G-to-detach): register the `release` port backend, attach a
        // ReleaseActuator to every control-bound vessel, and edge-detect → detach.
        app.register_type::<ReleaseActuator>();
        app.add_systems(Startup, register_release_backend);
        app.add_systems(Update, (attach_release_actuator, joint_release_system));
        // Persist a Persistent DetachJoint into the active doc's runtime layer.
        app.add_observer(persist_detach_to_runtime_layer);
        app.add_observer(persist_delete_to_runtime_layer);
        app.add_systems(
            Update,
            remove_legacy_ground_prim.run_if(obstacle_field_scene_changed),
        );
        // C4b: persist authored-scene moves into the active doc's runtime layer.
        app.add_observer(persist_move_to_runtime_layer);
        // C4b: persist palette/API spawns as referenced runtime-layer prims.
        app.add_observer(persist_spawn_to_runtime_layer);
        // #4: persist scalar shader-param tunes into the active doc's runtime
        // overlay (non-destructive; Save stays base-only). Decoupled from the
        // live-mutation handler above, like the move/spawn persisters.
        app.add_observer(persist_property_to_runtime_layer);
        // #15: persist wheel-dynamics tunes (suspension/drive → physxVehicle*) and
        // PBR base_color (→ primvars:displayColor) — the classes the shader-param
        // persister skips. Disjoint property sets, so both observers coexist.
        app.add_observer(persist_wheel_and_pbr_to_runtime_layer);
        // #14: persist a `SetEnvironmentLight` sun tweak (illuminance / colour /
        // shadow range) as `SetAttribute`s on the sun's DistantLight prim, using
        // the names the loader already reads back — so it round-trips + journals.
        app.add_observer(persist_environment_light_to_runtime_layer);
        // Applies the recorded focus at frame start, when last frame's fully
        // propagated GlobalTransforms are mutually consistent (see PendingFocus).
        app.add_systems(bevy::app::First, apply_pending_focus);
        // NOTE: `SelectEntity`/`on_select_entity` are editor-only (they drive the
        // Inspector highlight + gizmo) and live in the `ui`-gated `selection`
        // module; `SandboxEditPlugin` registers them. The headless server has no
        // selection, so they're absent here by design.
        // Render-lead visual prediction: live tunables + the per-gid eased
        // offsets they drive. Resources only — `SetVisualLead` itself is a
        // `#[Command]` like any other and comes in via `register_all_commands`.
        app.init_resource::<VisualLeadSettings>();
        app.init_resource::<VisualLeadState>();
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
                .chain(),
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

#[cfg(test)]
mod tests {
    use super::{predicts_locally, PREDICT_GRACE_TICKS};

    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            target: bevy::prelude::Entity::PLACEHOLDER,
            entry_id: "test".to_string(),
            position: bevy::prelude::Vec3::ZERO,
            rotation: None,
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

    // ── C4b: move-transform → runtime-layer persistence ─────────────────

    /// Build a headless app with the runtime-move producer wired and an active
    /// USD document containing `/World`, plus a sim entity bound to `prim_path`
    /// under api id `api_id`. Returns `(app, doc_id)`.
    fn app_with_runtime_producer(
        prim_path: &str,
        api_id: u64,
    ) -> (bevy::prelude::App, lunco_doc::DocumentId) {
        use bevy::prelude::*;
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // UsdCommandsPlugin inserts UsdDocumentRegistry + the `on_apply_usd_op`
        // observer that processes the `ApplyUsdOp` our producer dispatches.
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(persist_move_to_runtime_layer);

        let doc = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::DocumentOrigin::untitled("Scene.usda".to_string()),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));

        let ent = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: Handle::default(),
                path: prim_path.to_string(),
            })
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(ent, lunco_core::GlobalEntityId::from_raw(api_id));
        app.update();
        (app, doc)
    }

    #[test]
    fn move_of_authored_prim_persists_to_runtime_layer() {
        use bevy::prelude::*;
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: Vec3::new(3.0, 4.0, 5.0),
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        let world = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        // The move landed in the RUNTIME layer...
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`scalar`)
        // instead of the legacy `prim_attribute_value`, matching production (doc 21).
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "authored-scene move persists to the runtime layer"
        );
        // ...and the base layer (what Save writes) stays clean.
        let attr = lunco_usd_bevy::SdfPath::new("/World.xformOp:translate").unwrap();
        assert!(docu.data().spec(&attr).is_none(), "base layer untouched");
        assert!(!docu.source().contains("xformOp:translate"), "save excludes runtime move");
    }

    // ── A10: ONE wheel-param table ──────────────────────────────────────

    /// The whole point of collapsing the two hand-synced tables: a wheel
    /// property cannot be settable-but-not-persistable (the drift that lost
    /// `slip_stiffness` / `friction_mu` tunes on every reload). One row = one
    /// param = a setter AND a USD attribute, always.
    #[test]
    fn every_wheel_param_has_both_a_setter_and_a_usd_attr() {
        use super::*;
        use std::collections::HashSet;

        assert!(!WHEEL_PARAMS.is_empty());
        let mut seen_alias: HashSet<&str> = HashSet::new();
        let mut seen_attr: HashSet<&str> = HashSet::new();
        for p in WHEEL_PARAMS {
            assert!(!p.aliases.is_empty(), "a param with no name is unreachable");
            assert!(!p.usd_attr.is_empty(), "every param must round-trip through USD");
            assert!(seen_attr.insert(p.usd_attr), "duplicate USD attr {}", p.usd_attr);
            for a in p.aliases {
                assert!(seen_alias.insert(a), "duplicate alias {a}");
                // Both consumers (live setter + USD persister) resolve through the
                // SAME lookup, so a name that sets a field always has an attr.
                let row = wheel_param(a).expect("alias resolves");
                assert_eq!(row.usd_attr, p.usd_attr);
            }
        }

        // The two names the old split tables disagreed about are now complete.
        for name in ["slip_stiffness", "friction_mu", "mass"] {
            let row = wheel_param(name).expect("wheel param exists");
            assert!(!row.usd_attr.is_empty(), "{name} persists to USD");
        }
        assert!(wheel_param("not_a_wheel_field").is_none());

        // Setters write the field they claim.
        let mut w = lunco_mobility::WheelRaycast::default();
        (wheel_param("slip_stiffness").unwrap().set)(&mut w, 1234.0);
        (wheel_param("friction_mu").unwrap().set)(&mut w, 0.5);
        assert_eq!(w.slip_stiffness, 1234.0);
        assert_eq!(w.friction_mu, 0.5);
    }

    // ── A8: one history — the document's ────────────────────────────────

    /// Ctrl+Z routes to `UndoDocument`, which pops the USD document's last op
    /// and applies its inverse. The editor keeps no private undo stack, so the
    /// journal and the editor can no longer disagree.
    #[test]
    fn undo_document_reverts_the_last_usd_op() {
        use bevy::prelude::*;
        use super::*;
        use lunco_doc::Document;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        // USD's half of the generic verb now lives in `lunco-usd` (see the note above
        // `handle_undo_input`), so the test wires the real observer from there.
        app.add_observer(lunco_usd::commands::on_undo_usd_document);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: Vec3::new(3.0, 4.0, 5.0),
        });
        for _ in 0..3 {
            app.update();
        }
        let world_path = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        let gen_after_move = {
            let reg = app.world().resource::<UsdDocumentRegistry>();
            let docu = reg.host(doc).unwrap().document();
            assert_eq!(
                docu.runtime_data()
                    .prim_attribute_value::<[f64; 3]>(&world_path, "xformOp:translate"),
                Some([3.0, 4.0, 5.0])
            );
            docu.generation()
        };

        // The editor's undo verb — the SAME one the journal / other domains use.
        app.world_mut().trigger(UndoDocument { doc });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).unwrap().document();
        assert!(
            docu.generation() > gen_after_move,
            "undo applies an inverse op (history moves forward, state moves back)"
        );
        assert_ne!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world_path, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "the move is undone in the document, not just in ECS"
        );
    }

    #[test]
    fn move_of_unowned_entity_is_skipped() {
        use bevy::prelude::*;
        use super::*;
        use lunco_doc::Document;

        // Entity bound to a prim the document does NOT contain (e.g. a palette
        // spawn referencing an external asset).
        let (mut app, doc) = app_with_runtime_producer("/PaletteSpawn", 7);
        app.world_mut().trigger(MoveEntity {
            entity_id: 7,
            translation: Vec3::new(1.0, 2.0, 3.0),
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        // No op authored — the ownership guard skipped a non-document entity.
        assert_eq!(docu.generation(), 0, "un-owned entity move authors nothing");
        assert!(docu
            .runtime_data()
            .spec(&lunco_usd_bevy::SdfPath::new("/PaletteSpawn").unwrap())
            .is_none());
    }

    // ── C4b: spawn → referenced runtime-layer prim ──────────────────────

    #[test]
    fn spawn_persists_referenced_prim_to_runtime_layer() {
        use bevy::prelude::*;
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        // Persistence is a HOST behaviour: the observer deliberately skips
        // `Standalone` (there the direct ECS spawn is the sole instance, and
        // authoring it into the runtime layer too would double-instantiate it).
        // A real app always has the role; this hand-built one must supply it or
        // the observer fails `Res` validation.
        app.insert_resource(lunco_core::NetworkRole::Host);
        app.add_observer(persist_spawn_to_runtime_layer);

        // Catalog with one spawnable asset.
        let mut catalog = SpawnCatalog::default();
        catalog.add_unique(crate::catalog::SpawnableEntry {
            id: "test_rover".into(),
            display_name: "Test Rover".into(),
            category: "Rovers".into(),
            source: SpawnSource::UsdFile("vessels/rovers/test_rover.usda".into()),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });
        app.insert_resource(catalog);

        // Active USD doc whose default prim is /World.
        let doc = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::DocumentOrigin::untitled("Scene.usda".to_string()),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));
        app.update();

        // Spawn it at a drop position.
        let grid = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(SpawnEntity {
            target: grid,
            entry_id: "test_rover".into(),
            position: Vec3::new(2.0, 0.0, 7.0),
            rotation: None,
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        let prim = lunco_usd_bevy::SdfPath::new("/World/test_rover_1").unwrap();
        // The referenced spawn prim landed under the default prim, in RUNTIME...
        assert!(docu.runtime_data().spec(&prim).is_some(), "spawn prim authored in runtime layer");
        assert!(docu.data().spec(&prim).is_none(), "base layer untouched by spawn");
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`scalar`)
        // instead of the legacy `prim_attribute_value`, matching production (doc 21).
        assert_eq!(
            docu.runtime_data().prim_attribute_value::<[f64; 3]>(&prim, "xformOp:translate"),
            Some([2.0, 0.0, 7.0]),
            "spawn drop position recorded in runtime layer"
        );
        // ...rides into the composed view as a resolvable reference...
        let composed = docu.composed_source();
        assert!(
            composed.contains("@vessels/rovers/test_rover.usda@"),
            "composed view must carry the spawn reference:\n{composed}"
        );
        // ...and is excluded from Save (base only).
        assert!(!docu.source().contains("test_rover"), "spawn leaked into save:\n{}", docu.source());
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
        world.init_resource::<lunco_core::DivergenceStats>();
        // reconcile only runs as a connected Client; the clock is the render instant.
        world.insert_resource(lunco_core::NetworkRole::Client);
        world.insert_resource(lunco_core::NetStatus { connected: true, ..Default::default() });
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
        world.init_resource::<lunco_core::DivergenceStats>();
        world.insert_resource(lunco_core::NetworkRole::Client);
        world.insert_resource(lunco_core::NetStatus { connected: true, ..Default::default() });
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
            .push_back(PredictedState { seq: 1, pos: Vec3::ZERO, rot: Quat::IDENTITY });

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

        assert_eq!(world.resource::<PredictedStateLog>().0[&gid].last_reconciled, 1);
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
