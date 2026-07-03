//! Autopilot = a user with a specialty (spec 034).
//!
//! An autopilot is **not** a bespoke control layer and has nothing to do with the
//! human avatar. It is an ordinary *actor*: a session in the shared
//! [`lunco_core::SessionRegistry`] that carries the [`AuthorityRole::AiAgent`] role
//! and drives its vessel from a behaviour instead of a keyboard. Fully **headless**
//! — no rendering/UI/avatar dependency, so a `--no-ui` server runs it identically.
//!
//! Control authority is just vessel *ownership*, so the model is inherently
//! **multi-actor**: any number of vessels, each owned by a different session (some
//! human, some autopilot), no central arbiter. Each [`Autopilot`] owns one vessel
//! and drives only that vessel while it owns it:
//!
//! - **Engage** → register the `AiAgent` session and `claim` the vessel.
//! - **Drive** → each fixed tick, an engaged autopilot that still `owns` its vessel
//!   emits one `SetPorts` (the single writer that tick).
//! - **Yield** → if ownership transfers, `owns` goes false and it stops writing.
//!   Losing ownership IS the disengage signal. The human keyboard yields
//!   symmetrically (`lunco_controller::drive_from_bindings` drives only what it
//!   owns), so the two never fight — the rover-jitter root cause is gone.
//!
//! ## Behaviour = a [`lunco_behavior`] tree, authored as data
//!
//! The *what to do* is a behaviour tree ([`AutopilotBehavior`]). The tree STRUCTURE
//! (sequence waypoints, fallbacks, when-to-brake) is the **glue**, authored as DATA
//! ([`BehaviorSpec`]) — so rhai/JSON can define it and **hot-swap it on the fly**
//! (the [`SetAutopilotBehavior`] command replaces the tree at runtime). The leaf
//! primitives are **Rust** ([`nav_setpoint`] steering math) — computation stays in
//! Rust; rhai stays glue-only. With no behaviour attached, the autopilot falls back
//! to constant `throttle`/`steer` setpoints.

use bevy::prelude::*;
use lunco_behavior::{Action, BoxNode, Node, Parallel, ParallelPolicy, Repeat, Selector, Sequence, Status};
use lunco_core::session::{AuthorityRole, SessionRbac, UserSession};
use lunco_core::{Command, on_command, register_commands};
use lunco_core::{GlobalEntityId, NetworkRole, SessionId, SessionRegistry};
use lunco_cosim::SetPorts;
use serde::Deserialize;

/// Base of the reserved `SessionId` band for local autopilots. Kept clear of
/// [`SessionId::LOCAL`] (`0`, the human/host) and of host-minted client ids
/// (allocated low, per connection). A fleet offsets by index within this band, so
/// each autopilot is a distinct actor with its own ownership.
pub const AUTOPILOT_SESSION_BASE: u64 = 0xA0_7000;

/// Session id for the autopilot at `index` within the reserved band.
pub fn autopilot_session(index: u64) -> SessionId {
    SessionId(AUTOPILOT_SESSION_BASE + index)
}

/// An autonomous driver for `vessel`, acting under its own [`AiAgent`]
/// [`session`](Self::session). Add it (with `engaged = true`) to any entity — its
/// session possesses `vessel` and drives it. Many can coexist, each owning a
/// different vessel. Attach an [`AutopilotBehavior`] to the same entity for
/// tree-driven navigation; without one it drives the constant setpoints below.
///
/// [`AiAgent`]: AuthorityRole::AiAgent
#[derive(Component, Debug, Clone)]
pub struct Autopilot {
    /// The vessel this autopilot drives.
    pub vessel: Entity,
    /// The autopilot's own session identity (role `AiAgent`). Distinct per actor —
    /// see [`autopilot_session`].
    pub session: SessionId,
    /// Whether the autopilot currently holds + drives the vessel. Set true to
    /// engage. It self-clears if the claim is refused (vessel already owned).
    pub engaged: bool,
    /// Constant forward setpoint `[-1, 1]`, used when no [`AutopilotBehavior`].
    pub throttle: f64,
    /// Constant steer setpoint `[-1, 1]`, used when no [`AutopilotBehavior`].
    pub steer: f64,
}

impl Autopilot {
    /// An engaged autopilot for `vessel`, actor `index`, driving straight forward
    /// at `throttle` (no behaviour tree).
    pub fn forward(vessel: Entity, index: u64, throttle: f64) -> Self {
        Self { vessel, session: autopilot_session(index), engaged: true, throttle, steer: 0.0 }
    }
}

// ── Behaviour tree ───────────────────────────────────────────────────────────

/// Per-tick bridge the behaviour tree reads/writes: the vessel's world pose in,
/// the desired setpoint out. A leaf reads `pos`/`fwd` and writes `out`.
pub struct DriveCtx {
    /// Vessel world position.
    pub pos: Vec3,
    /// Vessel forward (unit).
    pub fwd: Vec3,
    /// Current mission time in seconds — [`lunco_time::WorldTime::sim_secs`], the one
    /// clock (never Bevy's `Time`). Time-based leaves such as [`WaitNode`] latch a
    /// deadline against it, so they freeze whenever the sim is paused or warped.
    pub now: f64,
    /// Setpoint the leaves write: `(throttle, steer, brake)` in `[-1, 1]`.
    pub out: (f64, f64, f64),
}

/// Data description of an autopilot behaviour tree — authored as rhai/JSON DATA
/// (the glue), compiled by [`build_tree`] into a [`lunco_behavior`] tree whose
/// leaves are this crate's Rust primitives. Being data, it is dynamic: swap it (via
/// [`SetAutopilotBehavior`]) to change or hot-reload behaviour at runtime.
///
/// JSON shape (internally tagged by `kind`), e.g.:
/// ```json
/// {"kind":"sequence","children":[
///   {"kind":"drive_to","target":[10,0,0],"speed":0.6,"radius":2.0},
///   {"kind":"drive_to","target":[10,0,10]}
/// ]}
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BehaviorSpec {
    /// Run children in order; fail on first failure, succeed when all succeed.
    Sequence {
        /// Ordered children.
        children: Vec<BehaviorSpec>,
    },
    /// Fallback: the first child that doesn't fail. Pair with [`Arrived`](Self::Arrived)
    /// guards for real branching (e.g. "if arrived brake, else drive").
    Selector {
        /// Ordered alternatives.
        children: Vec<BehaviorSpec>,
    },
    /// Tick every child each tick, resolving by `require` (all succeed / any
    /// succeeds). Use for "do X while monitoring Y" — e.g. drive while a watchdog
    /// condition races to abort.
    Parallel {
        /// `all` (default) = succeed when all children succeed, fail on any failure;
        /// `one` = succeed as soon as any child succeeds.
        #[serde(default)]
        require: ParallelRequire,
        /// Concurrent children.
        children: Vec<BehaviorSpec>,
    },
    /// Repeat a child forever (a patrol loop wraps its `sequence` in this).
    Forever {
        /// The repeated subtree.
        child: Box<BehaviorSpec>,
    },
    /// Repeat a child until it has succeeded `times` times, then succeed. A child
    /// failure fails the repeat.
    Repeat {
        /// Number of successful completions before the repeat itself succeeds.
        times: usize,
        /// The repeated subtree.
        child: Box<BehaviorSpec>,
    },
    /// Navigate toward a world point; `Success` within `radius` (and brakes there).
    DriveTo {
        /// World-space goal `[x, y, z]`.
        target: [f32; 3],
        /// Cruise speed `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
        /// Arrival radius (world units).
        #[serde(default = "default_radius")]
        radius: f32,
    },
    /// Loop a list of waypoints forever, driving to each in turn and optionally
    /// dwelling (braked) at each for `dwell` seconds. Sugar for
    /// `forever(sequence([drive_to, wait?]...))` — the common patrol pattern as one
    /// node. See [`build_tree`].
    Patrol {
        /// Ordered world-space waypoints `[[x, y, z], ...]`.
        waypoints: Vec<[f32; 3]>,
        /// Cruise speed `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
        /// Arrival radius per waypoint (world units).
        #[serde(default = "default_radius")]
        radius: f32,
        /// Seconds to hold (braked) at each waypoint before moving on (`0` = none).
        #[serde(default)]
        dwell: f64,
    },
    /// Condition leaf: `Success` when within `radius` of `target`, else `Failure`.
    /// The guard that makes [`Selector`](Self::Selector) fallbacks meaningful.
    Arrived {
        /// World-space point to test against.
        target: [f32; 3],
        /// Radius that counts as "arrived".
        #[serde(default = "default_radius")]
        radius: f32,
    },
    /// Hold position (braked) for `seconds`, then `Success`. Resets its timer when a
    /// parent restarts it, so it dwells correctly each patrol lap.
    Wait {
        /// Seconds to hold.
        #[serde(default = "default_wait")]
        seconds: f64,
    },
    /// Drive constant `throttle`/`steer` (always `Running`).
    Cruise {
        /// Forward setpoint `[-1, 1]`.
        #[serde(default)]
        throttle: f64,
        /// Steer setpoint `[-1, 1]`.
        #[serde(default)]
        steer: f64,
    },
    /// Hold the brakes (`Success`).
    Brake,
}

/// Completion rule for a [`BehaviorSpec::Parallel`], mapped to
/// [`ParallelPolicy`]. Defaults to `all`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelRequire {
    /// Succeed when all children succeed; fail on the first failure.
    #[default]
    All,
    /// Succeed as soon as any child succeeds; fail when all fail.
    One,
}

impl From<ParallelRequire> for ParallelPolicy {
    fn from(r: ParallelRequire) -> Self {
        match r {
            ParallelRequire::All => ParallelPolicy::RequireAll,
            ParallelRequire::One => ParallelPolicy::RequireOne,
        }
    }
}

fn default_speed() -> f64 {
    0.6
}
fn default_radius() -> f32 {
    3.0
}
fn default_wait() -> f64 {
    1.0
}

/// Compile a [`BehaviorSpec`] (rhai/JSON data) into a tickable tree. The composite
/// nodes come from the [`lunco_behavior`] kernel; the leaves are this crate's Rust
/// primitives (steering math). `Send + Sync` throughout, so the tree lives in a
/// [`Component`].
pub fn build_tree(spec: &BehaviorSpec) -> BoxNode<DriveCtx> {
    match spec {
        BehaviorSpec::Sequence { children } => {
            Box::new(Sequence::new(children.iter().map(build_tree).collect()))
        }
        BehaviorSpec::Selector { children } => {
            Box::new(Selector::new(children.iter().map(build_tree).collect()))
        }
        BehaviorSpec::Parallel { require, children } => {
            Box::new(Parallel::new((*require).into(), children.iter().map(build_tree).collect()))
        }
        BehaviorSpec::Forever { child } => Box::new(Repeat::forever(build_tree(child))),
        BehaviorSpec::Repeat { times, child } => Box::new(Repeat::times(*times, build_tree(child))),
        BehaviorSpec::DriveTo { target, speed, radius } => {
            leaf_drive_to(Vec3::from_array(*target), *speed, *radius)
        }
        BehaviorSpec::Patrol { waypoints, speed, radius, dwell } => {
            build_patrol(waypoints, *speed, *radius, *dwell)
        }
        BehaviorSpec::Arrived { target, radius } => {
            leaf_arrived(Vec3::from_array(*target), *radius)
        }
        BehaviorSpec::Wait { seconds } => Box::new(WaitNode::new(*seconds)),
        BehaviorSpec::Cruise { throttle, steer } => leaf_cruise(*throttle, *steer),
        BehaviorSpec::Brake => leaf_brake(),
    }
}

/// Compile a [`BehaviorSpec::Patrol`] to `forever(sequence([drive_to (+ wait?)…]))`.
/// Each waypoint becomes a `drive_to`; a non-zero `dwell` appends a braked
/// [`WaitNode`] so the rover pauses before moving on. The whole leg-sequence loops
/// forever — the `forever` resets it each lap, which resets the waits' timers.
fn build_patrol(waypoints: &[[f32; 3]], speed: f64, radius: f32, dwell: f64) -> BoxNode<DriveCtx> {
    let legs: Vec<BoxNode<DriveCtx>> = waypoints
        .iter()
        .map(|wp| {
            let drive = leaf_drive_to(Vec3::from_array(*wp), speed, radius);
            if dwell > 0.0 {
                Box::new(Sequence::new(vec![drive, Box::new(WaitNode::new(dwell))])) as BoxNode<DriveCtx>
            } else {
                drive
            }
        })
        .collect();
    Box::new(Repeat::forever(Box::new(Sequence::new(legs))))
}

/// Leaf: steer toward `target` (Rust nav math); `Success` once within `radius`.
fn leaf_drive_to(target: Vec3, speed: f64, radius: f32) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let (throttle, steer, brake, arrived) = nav_setpoint(ctx.pos, ctx.fwd, target, speed, radius);
        ctx.out = (throttle, steer, brake);
        if arrived {
            Status::Success
        } else {
            Status::Running
        }
    }))
}

/// Leaf: constant setpoint, never terminates.
fn leaf_cruise(throttle: f64, steer: f64) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        ctx.out = (throttle, steer, 0.0);
        Status::Running
    }))
}

/// Leaf: brake, `Success`.
fn leaf_brake() -> BoxNode<DriveCtx> {
    Box::new(Action::new(|ctx: &mut DriveCtx| {
        ctx.out = (0.0, 0.0, 1.0);
        Status::Success
    }))
}

/// Condition leaf: `Success` if within `radius` of `target`, else `Failure`. Reads
/// the pose only — writes no setpoint — so a [`Selector`] can branch on it without
/// disturbing the drive command chosen by the taken branch.
fn leaf_arrived(target: Vec3, radius: f32) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        if (target - ctx.pos).length() < radius {
            Status::Success
        } else {
            Status::Failure
        }
    }))
}

/// Leaf: hold the brakes for `seconds` of **mission time**, then `Success`. On the
/// first tick it latches a deadline `now + seconds` against [`DriveCtx::now`]
/// ([`lunco_time::WorldTime::sim_secs`]) — so a paused or warped sim pauses the wait
/// too, because that clock stops. Stateful, so — unlike an [`Action`] closure, whose
/// default `reset` is a no-op — it implements [`Node`] directly to clear the deadline
/// when a parent (e.g. a patrol `forever`) restarts it, dwelling afresh each lap.
pub struct WaitNode {
    seconds: f64,
    /// Mission-time instant to complete at; `None` until the first tick latches it.
    deadline: Option<f64>,
}

impl WaitNode {
    /// A wait leaf that holds for `seconds` of mission time.
    pub fn new(seconds: f64) -> Self {
        Self { seconds, deadline: None }
    }
}

impl Node<DriveCtx> for WaitNode {
    fn tick(&mut self, ctx: &mut DriveCtx) -> Status {
        ctx.out = (0.0, 0.0, 1.0); // hold position while waiting
        let deadline = *self.deadline.get_or_insert(ctx.now + self.seconds);
        if ctx.now >= deadline {
            Status::Success
        } else {
            Status::Running
        }
    }

    fn reset(&mut self) {
        self.deadline = None;
    }
}

/// The autopilot's compiled behaviour tree. Insert/replace this component to change
/// or hot-swap behaviour at runtime (see [`SetAutopilotBehavior`]).
#[derive(Component)]
pub struct AutopilotBehavior(pub BoxNode<DriveCtx>);

impl AutopilotBehavior {
    /// Compile from a [`BehaviorSpec`] (typically deserialized from rhai/JSON data).
    pub fn new(spec: &BehaviorSpec) -> Self {
        Self(build_tree(spec))
    }

    /// Compile from a JSON spec string — the on-the-fly / rhai-authored form.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str::<BehaviorSpec>(json)
            .map(|s| Self::new(&s))
            .map_err(|e| e.to_string())
    }
}

/// Steering math (Rust): from the vessel's world pose and a goal, return
/// `(throttle, steer, brake, arrived)` in `[-1, 1]`. Steer toward the goal on the
/// yaw plane; hard-turn when it's behind; ease the throttle down while poorly
/// aligned so it pivots onto the goal instead of arcing past; brake + `arrived`
/// within `radius`. COMPUTATION, so it lives in Rust — rhai is glue-only. Steering
/// is a *relative* direction, so it's invariant to the floating-origin offset.
pub fn nav_setpoint(pos: Vec3, fwd: Vec3, target: Vec3, speed: f64, radius: f32) -> (f64, f64, f64, bool) {
    let to = target - pos;
    let dist = to.length();
    if dist < radius {
        return (0.0, 0.0, 1.0, true); // arrived → brake
    }
    let to = to / dist; // unit direction to goal
    let fwd = fwd.normalize_or_zero();
    // Yaw-plane cross `(forward × to).y` and forward/goal alignment `dot`. Skid mix
    // is `left = drive + steer`, so `+steer` yaws right; we steer `-cy` to turn
    // toward the goal (matches the prelude `steer_to` sign convention).
    let cy = fwd.z * to.x - fwd.x * to.z;
    let dot = fwd.dot(to);
    let steer: f32 = if dot < 0.0 {
        if cy >= 0.0 { -1.0 } else { 1.0 } // goal behind → hard turn toward its side
    } else {
        (-cy * 2.5).clamp(-1.0, 1.0)
    };
    let throttle = speed * (0.25 + 0.75 * dot as f64).clamp(0.25, 1.0);
    (throttle, steer as f64, 0.0, false)
}

// ── Systems ──────────────────────────────────────────────────────────────────

/// Host/standalone: when an [`Autopilot`] appears, register its `AiAgent` session
/// (so the role lattice + authorization treat it as a first-class user) and
/// `claim` its vessel. A refused claim (vessel already owned by another actor)
/// leaves it disengaged rather than fighting for control.
pub fn setup_autopilot_session(
    role: Res<NetworkRole>,
    mut q: Query<&mut Autopilot, Added<Autopilot>>,
    q_gid: Query<&GlobalEntityId>,
    mut rbac: ResMut<SessionRbac>,
    mut registry: ResMut<SessionRegistry>,
) {
    // Run on the authoritative peer (Host or single-player Standalone), never a
    // Client — ownership is the host's to assign. `is_host()` alone would skip the
    // idle single-player sandbox, which is `Standalone`.
    if matches!(*role, NetworkRole::Client) {
        return;
    }
    for mut ap in &mut q {
        // Register the autopilot exactly like a connecting user: authenticated, with
        // a host-issued token so `SessionRbac::is_authorized` treats it as trusted.
        rbac.sessions.entry(ap.session.0).or_insert_with(|| UserSession {
            session_id: ap.session,
            username: format!("autopilot-{}", ap.session.0),
            role: AuthorityRole::AiAgent,
            authenticated: true,
            token: Some("autopilot-local".to_string()),
        });
        let Ok(gid) = q_gid.get(ap.vessel) else {
            warn!("[autopilot] vessel {:?} has no GlobalEntityId; cannot engage", ap.vessel);
            ap.engaged = false;
            continue;
        };
        match registry.claim(ap.session, gid.get()) {
            Ok(()) => {
                ap.engaged = true;
                info!("[autopilot] session {} engaged, owns entity {}", ap.session, gid.get());
            }
            Err(cur) => {
                ap.engaged = false;
                warn!(
                    "[autopilot] session {} could not claim entity {} (owned by {cur}); staying disengaged",
                    ap.session,
                    gid.get()
                );
            }
        }
    }
}

/// Fixed-tick driver: every engaged autopilot that still **owns** its vessel emits
/// one `SetPorts`. If it has an [`AutopilotBehavior`] tree, tick it against the
/// vessel's world pose to get the setpoint (glue in the tree, math in the Rust
/// leaves); otherwise use the constant setpoints. Losing ownership makes `owns`
/// false, so it stops writing with no disengage polling — the symmetric self-gate
/// to the human's yield in `drive_from_bindings`.
pub fn drive_autopilots(
    registry: Res<SessionRegistry>,
    world_time: Res<lunco_time::WorldTime>,
    mut q: Query<(&Autopilot, Option<&mut AutopilotBehavior>)>,
    q_gid: Query<&GlobalEntityId>,
    q_xf: Query<&GlobalTransform>,
    mut commands: Commands,
) {
    let now = world_time.sim_secs; // the one clock — waits freeze under pause/warp
    for (ap, behavior) in &mut q {
        if !ap.engaged {
            continue;
        }
        let Ok(gid) = q_gid.get(ap.vessel) else { continue };
        if !registry.owns(ap.session, gid.get()) {
            continue; // lost the vessel → stop driving (one writer per tick)
        }

        let (throttle, steer, brake) = match (behavior, q_xf.get(ap.vessel).ok()) {
            (Some(mut tree), Some(xf)) => {
                let mut ctx = DriveCtx {
                    pos: xf.translation(),
                    fwd: xf.forward().as_vec3(),
                    now,
                    out: (ap.throttle, ap.steer, 0.0),
                };
                tree.0.tick(&mut ctx);
                ctx.out
            }
            _ => (ap.throttle, ap.steer, 0.0),
        };

        commands.trigger(SetPorts {
            target: ap.vessel,
            writes: vec![
                ("throttle".to_string(), throttle),
                ("steer".to_string(), steer),
                ("brake".to_string(), brake),
            ],
            seq: 0,
            tick: 0,
        });
    }
}

// ── Live behaviour authoring (rhai / API) ────────────────────────────────────

/// Engage an autopilot on `vessel`: spawn an [`Autopilot`] actor (its `AiAgent`
/// session claims the vessel next tick) and, if `spec_json` is non-empty, attach a
/// behaviour tree. The create-an-autopilot seam for the API / rhai:
/// `cmd("EngageAutopilot", #{ vessel: v, throttle: 0.5 })` or with a
/// `spec_json` [`BehaviorSpec`] to navigate.
#[Command]
pub struct EngageAutopilot {
    /// The vessel to put under autopilot.
    pub vessel: Entity,
    /// Actor index within the reserved session band (distinct autopilots differ).
    #[serde(default)]
    #[reflect(default)]
    pub index: u64,
    /// Constant forward setpoint used when no behaviour tree is given (default 0.5).
    #[serde(default)]
    #[reflect(default)]
    pub throttle: f64,
    /// Optional JSON [`BehaviorSpec`]; when present the autopilot navigates via the
    /// behaviour tree instead of the constant `throttle`.
    #[serde(default)]
    #[reflect(default)]
    pub spec_json: String,
}

#[on_command(EngageAutopilot)]
fn on_engage_autopilot(cmd: EngageAutopilot, mut commands: Commands) {
    let throttle = if cmd.throttle != 0.0 { cmd.throttle } else { 0.5 };
    let mut e = commands.spawn(Autopilot::forward(cmd.vessel, cmd.index, throttle));
    if !cmd.spec_json.is_empty() {
        match AutopilotBehavior::from_json(&cmd.spec_json) {
            Ok(b) => {
                e.insert(b);
            }
            Err(err) => warn!("[autopilot] EngageAutopilot: bad spec: {err}"),
        }
    }
    info!("[autopilot] engaging on vessel {:?} (index {})", cmd.vessel, cmd.index);
}

/// Set (or hot-swap) an autopilot's behaviour tree from a JSON [`BehaviorSpec`].
/// The dynamic authoring seam: a rhai scenario `cmd("SetAutopilotBehavior", #{
/// vessel: v, spec_json: "{...}" })` defines or replaces a vessel's behaviour at
/// runtime — different autopilots, updated on the fly, no rebuild. The tree is
/// data, so authoring is glue; the leaves it names are the Rust primitives.
#[Command]
pub struct SetAutopilotBehavior {
    /// The vessel whose autopilot should adopt the behaviour.
    pub vessel: Entity,
    /// A JSON [`BehaviorSpec`] (see its docs for the shape).
    pub spec_json: String,
}

#[on_command(SetAutopilotBehavior)]
fn on_set_autopilot_behavior(
    cmd: SetAutopilotBehavior,
    q: Query<(Entity, &Autopilot)>,
    mut commands: Commands,
) {
    let behavior = match AutopilotBehavior::from_json(&cmd.spec_json) {
        Ok(b) => b,
        Err(e) => {
            warn!("[autopilot] SetAutopilotBehavior: bad spec: {e}");
            return;
        }
    };
    match q.iter().find(|(_, ap)| ap.vessel == cmd.vessel) {
        Some((entity, _)) => {
            commands.entity(entity).insert(behavior);
            info!("[autopilot] behaviour updated for vessel {:?}", cmd.vessel);
        }
        None => warn!("[autopilot] SetAutopilotBehavior: no autopilot owns vessel {:?}", cmd.vessel),
    }
}

register_commands!(on_engage_autopilot, on_set_autopilot_behavior);

/// Headless-safe plugin: engage autopilots on spawn, drive them each fixed tick,
/// and accept live behaviour updates. No rendering/UI/avatar dependency.
pub struct AutopilotPlugin;

impl Plugin for AutopilotPlugin {
    fn build(&self, app: &mut App) {
        // The behaviour tree reads mission time from `WorldTime`; guarantee the
        // time spine is present (guarded — harmless where usd-bevy/celestial/etc.
        // already added it). Headless-safe: `TimePlugin` is resources + schedule
        // steps, no rendering.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.add_systems(Update, setup_autopilot_session);
        app.add_systems(FixedUpdate, drive_autopilots);
        register_all_commands(app);
    }
}
