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
use lunco_behavior::{
    Action, BoxNode, Force, Invert, Node, Parallel, ParallelPolicy, ReactiveSelector,
    ReactiveSequence, Repeat, Retry, Selector, Sequence, Status,
};
use lunco_core::session::{AuthorityRole, SessionRbac, UserSession};
use lunco_core::{Ack, Command, OpId, on_command, register_commands};

/// BehaviorTree.CPP v4 XML ⇄ tree-JSON codec (Groot2 / ROS interop).
pub mod btcpp_xml;
/// Behaviour trees authored as USD prims (one prim per node) — the source of truth
/// for a mission. `AutopilotBehaviorSpec` is derived from them, never authored.
pub mod usd_tree;
use lunco_core::{GlobalEntityId, NetworkRole, SessionId, SessionRegistry};
use lunco_cosim::SetPorts;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

/// Live kinematic state of a track/follow/intercept target this tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct TargetState {
    /// World position.
    pub pos: Vec3,
    /// World velocity (units/s), finite-differenced across ticks — zero on the first
    /// sighting and while the sim is paused. Lead-pursuit ([`BehaviorSpec::Intercept`])
    /// aims ahead along this; plain [`BehaviorSpec::Follow`] ignores it.
    pub vel: Vec3,
}

/// Live kinematic state of candidate targets this tick, keyed by [`GlobalEntityId`]
/// (the api_id a scenario names). Built once per tick by [`drive_autopilots`] and
/// shared into every [`DriveCtx`]; the [`BehaviorSpec::Follow`] / [`BehaviorSpec::Intercept`]
/// leaves resolve their target through it, so they track a mover.
pub type TargetStates = std::collections::HashMap<u64, TargetState>;

/// Distance (world units) to the nearest collider along the forward ray-fan cast
/// this tick by [`sense_clearance`] — the vessel's obstacle sensor. `None` on a lane
/// means clear to [`range`](Self::range). Populated only where physics (avian) runs;
/// a headless server has it, a physics-free harness leaves it all-clear.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clearance {
    /// Nearest hit straight ahead.
    pub ahead: Option<f32>,
    /// Nearest hit on the forward-left probe (heading rotated `+spread` about up).
    pub left: Option<f32>,
    /// Nearest hit on the forward-right probe (heading rotated `-spread`).
    pub right: Option<f32>,
    /// Sensor range the probes were cast to (a lane at this distance reads clear).
    pub range: f32,
}

/// Per-vessel [`Clearance`] from the last [`sense_clearance`] pass, keyed by the
/// **vessel** [`Entity`] — filled for every controlled vessel (human- or
/// autopilot-driven), since the sensor is the rover's. A resource (not a component)
/// so the raycasting system and its consumers stay decoupled: `sense_clearance`
/// fills it (only when avian is present), [`drive_autopilots`] reads it into each
/// [`DriveCtx`] (and a HUD could read it for a human), defaulting to all-clear.
#[derive(Resource, Default)]
pub struct ClearanceField(pub std::collections::HashMap<Entity, Clearance>);

/// Per-tick bridge the behaviour tree reads/writes: the vessel's world pose in,
/// the desired setpoint out. A leaf reads `pos`/`fwd` (and, for tracking leaves,
/// `targets`) and writes `out`.
pub struct DriveCtx {
    /// The autopilot's own vessel [`GlobalEntityId`], so sensing leaves can exclude
    /// themselves from the [`targets`](Self::targets) snapshot (a vessel isn't its own
    /// obstacle). `0` when unknown.
    pub self_gid: u64,
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
    /// Live kinematic state of other entities this tick, keyed by [`GlobalEntityId`].
    /// Tracking leaves ([`BehaviorSpec::Follow`]/[`BehaviorSpec::Intercept`]) look their
    /// moving target up here. Cheap `Arc` clone per autopilot; empty for pose-only
    /// leaves. See [`TargetStates`].
    pub targets: std::sync::Arc<TargetStates>,
    /// Forward obstacle-sensor readings from this tick's physics ray-fan (see
    /// [`Clearance`]). All-clear where physics isn't running. Read by the raycast
    /// leaves `path_blocked` / `steer_clear`.
    pub clearance: Clearance,
    /// Outgoing tool calls queued this tick by [`BehaviorSpec::RunTool`] leaves.
    /// Leaves have no ECS access, so they push here; `drive_autopilots` drains
    /// the queue after the tick and re-emits each as a [`ToolFired`] event.
    /// Reset to empty every tick when the `DriveCtx` is constructed.
    pub fired: Vec<ToolInvocation>,
}

// `ToolInvocation` and `ToolFired` are the shared tool-call vocabulary. They
// live in `lunco-core` (see `lunco_core::tools`) so that handler crates can
// observe `ToolFired` without depending on this crate — instruments must not
// depend on the driver. Re-exported here so existing `lunco_autopilot::`
// references keep resolving.
pub use lunco_core::tools::{ToolFired, ToolInvocation};

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
///
/// `Serialize` so the UI can round-trip a spec (read → append a checkpoint →
/// re-emit via `SetAutopilotBehavior`) without a separate JSON mirror.

/// One stop on a [`BehaviorSpec::Patrol`] loop. Carries a world position plus
/// optional per-waypoint tuning (dwell) and **arrival actions** — the tools to
/// fire when the vessel reaches this waypoint (e.g. `take_photo`).
///
/// This is the declarative home for "fire a tool at a patrol waypoint": instead
/// of authoring a hand-composed `sequence[arrived, run_tool]` tree in rhai, a
/// mission just lists `on_arrival` actions on the waypoint and the patrol
/// engine injects them into the compiled tree (see `build_patrol`). rhai/JSON
/// authors *data*, not trees.
///
/// **Backward-compat serde:** a bare `[x, y, z]` array deserializes into a
/// waypoint at that position with no actions and no dwell, so the legacy
/// `waypoints: [[x,y,z], ...]` shape (used by `patrol.rhai` and Ctrl+LMB)
/// keeps working unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct PatrolWaypoint {
    /// World-space position `[x, y, z]`.
    pub pos: [f32; 3],
    /// Seconds to hold (braked) at this waypoint before its actions + departure.
    /// `None` → inherit the patrol's top-level `dwell`. Defaults to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dwell: Option<f64>,
    /// Actions to run on arrival (after any dwell), in order. Empty by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_arrival: Vec<WaypointAction>,
}

impl PatrolWaypoint {
    /// A bare waypoint at `pos` with no actions and no per-waypoint dwell.
    pub fn at(pos: [f32; 3]) -> Self {
        Self { pos, dwell: None, on_arrival: Vec::new() }
    }
}

/// An action fired when a patrol vessel arrives at a waypoint. Today only
/// `RunTool` (fire a named tool via the `ToolFired` event); the enum is
/// extensible for future core actions (sample, transmit, …) without reshaping
/// the patrol data again.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaypointAction {
    /// Fire a named tool call (same shape as [`BehaviorSpec::RunTool`]).
    RunTool {
        /// Tool name (convention `family::verb`, e.g. `"science::take_photo"`).
        tool: String,
        /// Opaque args string forwarded verbatim to the tool's handler.
        #[serde(default)]
        args: String,
    },
}

// Backward-compat: a bare `[x, y, z]` array deserializes to a no-action
// `PatrolWaypoint`. Lets legacy `waypoints: [[x,y,z], ...]` JSON / rhai keep
// working after the `Vec<[f32;3]>` → `Vec<PatrolWaypoint>` type change.
//
// NOTE: this dual-shape (array vs object) handling is JSON-only — it peeks at
// `serde_json::Value` to pick the branch, so it won't work with bincode or other
// self-describing formats. That's fine here: `BehaviorSpec`'s only wire path is
// JSON (rhai `to_json` / the HTTP API / USD metadata), never bincode. If a
// binary format is ever needed, give `PatrolWaypoint` two explicit
// `#[serde(deserialize_with = …)]` arms or a newtype wrapper instead.
impl<'de> serde::Deserialize<'de> for PatrolWaypoint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        struct Full {
            pos: [f32; 3],
            #[serde(default)]
            dwell: Option<f64>,
            #[serde(default)]
            on_arrival: Vec<WaypointAction>,
        }
        // Two accepted shapes:
        //  1. `[x, y, z]`            — legacy bare array (no actions, no dwell).
        //  2. `{pos: [...], dwell?, on_arrival?}` — full struct.
        let v = serde_json::Value::deserialize(d)?;
        if v.is_array() {
            // Bare-array legacy form.
            let p: [f32; 3] = serde_json::from_value(v).map_err(D::Error::custom)?;
            return Ok(PatrolWaypoint::at(p));
        }
        let f: Full = serde_json::from_value(v).map_err(D::Error::custom)?;
        Ok(PatrolWaypoint { pos: f.pos, dwell: f.dwell, on_arrival: f.on_arrival })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
        /// Ordered waypoints. Each carries a position + optional per-waypoint
        /// dwell and arrival actions (e.g. `take_photo`). Accepts the legacy
        /// `[[x,y,z], ...]` bare-array shape (no actions) for backward compat
        /// with existing rhai/JSON — see [`PatrolWaypoint`]'s serde impl.
        waypoints: Vec<PatrolWaypoint>,
        /// Cruise speed `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
        /// Arrival radius per waypoint (world units).
        #[serde(default = "default_radius")]
        radius: f32,
        /// Default seconds to hold (braked) at each waypoint before moving on
        /// (`0` = none). Overridden by a waypoint's own `dwell` when set.
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
    /// Turn in place to face `target` (steer only, no throttle); `Success` once the
    /// heading is within `tolerance` degrees. Aim before driving, point an
    /// instrument/antenna, or orient a lander.
    Face {
        /// World point to face `[x, y, z]`.
        target: [f32; 3],
        /// Heading tolerance in degrees that counts as "facing".
        #[serde(default = "default_face_tol")]
        tolerance: f64,
    },
    /// Always succeed immediately — a no-op / placeholder (e.g. a `selector`
    /// fallthrough or a stubbed branch).
    Succeed,
    /// Always fail immediately — a placeholder / forced-failure branch.
    Fail,

    // ── Reactive composites (re-evaluate guards every tick) ──────────────────
    /// Like [`Sequence`](Self::Sequence) but re-ticks from the first child every
    /// tick, so guard children stay live: `reactive_sequence([arrived_inverted?,
    /// drive_to])` keeps driving *while* not-arrived and re-checks each frame.
    ReactiveSequence {
        /// Ordered children (typically guard(s) then an action).
        children: Vec<BehaviorSpec>,
    },
    /// Like [`Selector`](Self::Selector) but re-ticks from the highest priority
    /// child every tick, so a higher option preempts a lower one mid-run — the
    /// priority arbiter (e.g. avoid-if-blocked, else cruise).
    ReactiveSelector {
        /// Ordered alternatives, highest priority first.
        children: Vec<BehaviorSpec>,
    },

    // ── Decorators (single-child wrappers) ───────────────────────────────────
    /// Negate a child's result (`Success` ↔ `Failure`) — turn a condition into its
    /// opposite, e.g. `invert(arrived)` = "not yet arrived".
    Invert {
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Force any terminal result of the child to `Success` — a best-effort step that
    /// must never fail its parent sequence.
    ForceSuccess {
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Force any terminal result of the child to `Failure` — force an abort.
    ForceFailure {
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Retry the child on `Failure` up to `times` attempts, then give up
    /// (`Failure`); a child `Success` ends it early. The failure-side mirror of
    /// [`Repeat`](Self::Repeat) — re-attempt a flaky maneuver before conceding.
    Retry {
        /// Number of failures tolerated before the retry itself fails.
        times: usize,
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Run the child but abort with `Failure` if it stays `Running` longer than
    /// `seconds` of mission time (a child terminal before then passes through). The
    /// watchdog for "attempt X for N seconds, else fall back". On timeout it brakes.
    Timeout {
        /// Mission-time budget in seconds.
        #[serde(default = "default_wait")]
        seconds: f64,
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Follow a **moving** entity (named by its [`GlobalEntityId`]/api_id): steer
    /// toward its *live* position every tick, holding station within `radius`.
    /// Unlike [`DriveTo`](Self::DriveTo) it never finishes — it stays `Running` while
    /// the target is resolvable, and `Failure` (braking) if the target vanishes, so a
    /// fallback branch can take over.
    Follow {
        /// [`GlobalEntityId`] (api_id) of the entity to follow.
        target: u64,
        /// Cruise speed `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
        /// Station-keeping radius — hold this far off the target (world units).
        #[serde(default = "default_follow_radius")]
        radius: f32,
    },
    /// Lead-pursuit a **moving** entity: aim `lead` seconds *ahead* of the target
    /// along its velocity (so you cut it off instead of chasing its tail), and
    /// `Success` on contact — within `radius` of its actual position. `Failure`
    /// (braking) if the target vanishes. Unlike [`Follow`](Self::Follow) (open-ended
    /// station-keeping) this is a catch-it pursuit that finishes.
    Intercept {
        /// [`GlobalEntityId`] (api_id) of the entity to intercept.
        target: u64,
        /// Cruise speed `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
        /// Contact radius that counts as intercepted (world units).
        #[serde(default = "default_radius")]
        radius: f32,
        /// Seconds to lead the target — how far ahead along its velocity to aim.
        #[serde(default = "default_lead")]
        lead: f64,
    },
    /// Condition leaf: `Success` if another known vessel is within `distance` and
    /// inside a forward cone of `cone` degrees (something's in the way), else
    /// `Failure`. Vessel-vs-vessel proximity sensing off the `targets` snapshot
    /// (self excluded) — pair with a `reactive_selector` for "stop/steer if blocked,
    /// else drive". Writes no setpoint.
    ObstacleAhead {
        /// How far ahead to look (world units).
        #[serde(default = "default_obstacle_dist")]
        distance: f32,
        /// Full width of the forward detection cone, in degrees.
        #[serde(default = "default_cone")]
        cone: f64,
    },
    /// Condition leaf: `Success` if the vessel's heading is within `tolerance`
    /// degrees of `target`, else `Failure`. The read-only guard counterpart to
    /// [`Face`](Self::Face) — gate a drive on being pointed the right way.
    Facing {
        /// World point to test the heading against.
        target: [f32; 3],
        /// Heading tolerance in degrees.
        #[serde(default = "default_face_tol")]
        tolerance: f64,
    },
    /// Hold position (braked) and stay `Running` forever — a "stay put" action, e.g.
    /// under a `parallel`/`reactive_sequence` while a guard keeps holding. Unlike
    /// [`Brake`](Self::Brake) (which `Success`es and lets a sequence advance), `hold`
    /// never finishes.
    Hold,
    /// Decorator: run the child, but after it `Success`es block re-entry (return
    /// `Failure`) for `seconds` of mission time — rate-limits an action so it can't
    /// re-fire every tick (fire a thruster, drop a marker, re-plan). `Running`/
    /// `Failure` pass through and set no cooldown.
    Cooldown {
        /// Mission-time lockout after each success, in seconds.
        #[serde(default = "default_wait")]
        seconds: f64,
        /// The wrapped subtree.
        child: Box<BehaviorSpec>,
    },
    /// Condition leaf: `Success` if the forward physics **raycast** hits a collider
    /// within `distance` (the path ahead is blocked by terrain/geometry), else
    /// `Failure`. Reads the [`Clearance`] sensor filled by [`sense_clearance`], so it
    /// works headless. Writes no setpoint.
    PathBlocked {
        /// How near ahead a hit counts as blocking (world units).
        #[serde(default = "default_obstacle_dist")]
        distance: f32,
    },
    /// Reactive obstacle avoidance driven by the forward ray-fan ([`Clearance`]):
    /// drive at `speed` when the path is clear, otherwise steer toward the more open
    /// side and ease off, braking if boxed in. Always `Running` (a behaviour, not a
    /// goal) — compose it under a `reactive_selector` behind a `path_blocked` guard,
    /// or run it as the default action.
    SteerClear {
        /// Cruise speed when clear `[0, 1]`.
        #[serde(default = "default_speed")]
        speed: f64,
    },
    /// Fire a named tool call once per activation (e.g. `science::take_photo`
    /// at a patrol waypoint). One-shot: latches `Success` after the first tick
    /// and won't re-fire until the tree's [`Node::reset`] clears it (which the
    /// `repeat` / `cooldown` decorators drive). The `tool` string names the
    /// tool; `args` is an opaque payload the tool's observer interprets
    /// (typically JSON) — the core stays tool-agnostic. The fired call is
    /// queued on [`DriveCtx::fired`] and re-emitted as a [`ToolFired`] event by
    /// `drive_autopilots`, since leaves have no ECS access.
    RunTool {
        /// Tool name (convention `family::verb`, e.g. `"science::take_photo"`).
        tool: String,
        /// Opaque args string forwarded verbatim to the tool's observer.
        #[serde(default)]
        args: String,
    },
}

/// Completion rule for a [`BehaviorSpec::Parallel`], mapped to
/// [`ParallelPolicy`]. Defaults to `all`.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
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
fn default_face_tol() -> f64 {
    8.0
}
fn default_follow_radius() -> f32 {
    5.0
}
fn default_lead() -> f64 {
    1.0
}
fn default_obstacle_dist() -> f32 {
    6.0
}
fn default_cone() -> f64 {
    60.0
}

/// Compile a [`BehaviorSpec`] (rhai/JSON data) into a tickable tree. The composite
/// nodes come from the [`lunco_behavior`] kernel; the leaves are this crate's Rust
/// primitives (steering math). `Send + Sync` throughout, so the tree lives in a
/// [`Component`].
pub fn build_tree(spec: &BehaviorSpec) -> BoxNode<DriveCtx> {
    match spec {
        BehaviorSpec::Sequence { children } => {
            Box::new(Sequence::new(build_sequence_children(children)))
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
        BehaviorSpec::Face { target, tolerance } => {
            leaf_face(Vec3::from_array(*target), *tolerance)
        }
        BehaviorSpec::Succeed => Box::new(Action::new(|_: &mut DriveCtx| Status::Success)),
        BehaviorSpec::Fail => Box::new(Action::new(|_: &mut DriveCtx| Status::Failure)),
        BehaviorSpec::ReactiveSequence { children } => {
            Box::new(ReactiveSequence::new(build_sequence_children(children)))
        }
        BehaviorSpec::ReactiveSelector { children } => {
            Box::new(ReactiveSelector::new(children.iter().map(build_tree).collect()))
        }
        BehaviorSpec::Invert { child } => Box::new(Invert::new(build_tree(child))),
        BehaviorSpec::ForceSuccess { child } => Box::new(Force::succeed(build_tree(child))),
        BehaviorSpec::ForceFailure { child } => Box::new(Force::fail(build_tree(child))),
        BehaviorSpec::Retry { times, child } => Box::new(Retry::times(*times, build_tree(child))),
        BehaviorSpec::Timeout { seconds, child } => {
            Box::new(TimeoutNode::new(*seconds, build_tree(child)))
        }
        BehaviorSpec::Follow { target, speed, radius } => leaf_follow(*target, *speed, *radius),
        BehaviorSpec::Intercept { target, speed, radius, lead } => {
            leaf_intercept(*target, *speed, *radius, *lead)
        }
        BehaviorSpec::ObstacleAhead { distance, cone } => leaf_obstacle_ahead(*distance, *cone),
        BehaviorSpec::Facing { target, tolerance } => {
            leaf_facing(Vec3::from_array(*target), *tolerance)
        }
        BehaviorSpec::Hold => Box::new(Action::new(|ctx: &mut DriveCtx| {
            ctx.out = (0.0, 0.0, 1.0);
            Status::Running
        })),
        BehaviorSpec::Cooldown { seconds, child } => {
            Box::new(CooldownNode::new(*seconds, build_tree(child)))
        }
        BehaviorSpec::PathBlocked { distance } => leaf_path_blocked(*distance),
        BehaviorSpec::SteerClear { speed } => leaf_steer_clear(*speed),
        BehaviorSpec::RunTool { tool, args } => {
            Box::new(RunToolNode::new(tool.clone(), args.clone()))
        }
    }
}

/// Half-angle of the forward-left/right obstacle probes, radians (~30°). Shared by
/// [`sense_clearance`] (which casts the probes) and [`leaf_steer_clear`] (which
/// recomputes the probe directions to steer toward the open one), so they agree.
const PROBE_SPREAD: f32 = std::f32::consts::FRAC_PI_6;
/// How far the obstacle probes are cast (world units).
const SENSOR_RANGE: f32 = 20.0;
/// World-up offsets (m) from the vessel origin at which each lane is probed — a small
/// fan of heights (low wheels → chassis → mast) so a body-height obstacle is detected
/// well before a single centre-height ray would just graze it.
const PROBE_HEIGHTS: [f32; 3] = [-0.2, 0.4, 1.0];

/// Compile a [`BehaviorSpec::Patrol`] to `forever(sequence([drive_to (+ wait?)…]))`.
/// Each waypoint becomes a `drive_to`; a non-zero `dwell` appends a braked
/// [`WaitNode`] so the rover pauses before moving on. The whole leg-sequence loops
/// forever — the `forever` resets it each lap, which resets the waits' timers.
///
/// Each waypoint's `on_arrival` actions are appended after its (optional) wait,
/// so a waypoint like `{pos:[...], on_arrival:[run_tool("take_photo")]}` compiles
/// to `sequence[drive_to, wait?, run_tool]` — the declarative home for
/// "fire a tool at a patrol waypoint" (no rhai tree-composition needed).
fn build_patrol(waypoints: &[PatrolWaypoint], speed: f64, radius: f32, dwell: f64) -> BoxNode<DriveCtx> {
    let legs: Vec<BoxNode<DriveCtx>> = waypoints
        .iter()
        .map(|wp| {
            // Arrival latch shared by this waypoint's `drive_to` and its arrival
            // tools: the drive leaf arms it while en route, firing consumes it, so
            // a tool fires once per genuine arrival instead of every tick the
            // rover sits parked in the radius (see `RunToolNode::arm`). Starts
            // ARMED so the first arrival fires even if the rover is engaged while
            // already standing on the waypoint.
            let arm = (!wp.on_arrival.is_empty()).then(|| Arc::new(AtomicBool::new(true)));
            let drive =
                leaf_drive_to_arming(Vec3::from_array(wp.pos), speed, radius, arm.clone());
            // Per-waypoint dwell overrides the patrol's top-level default.
            let wp_dwell = wp.dwell.unwrap_or(dwell);
            let mut steps: Vec<BoxNode<DriveCtx>> = vec![drive];
            if wp_dwell > 0.0 {
                steps.push(Box::new(WaitNode::new(wp_dwell)));
            }
            // Arrival actions (tools to fire) run after the dwell, in order.
            for act in &wp.on_arrival {
                match act {
                    WaypointAction::RunTool { tool, args } => {
                        let mut node = RunToolNode::new(tool.clone(), args.clone());
                        if let Some(a) = &arm {
                            node = node.armed_by(a.clone());
                        }
                        steps.push(Box::new(node));
                    }
                }
            }
            if steps.len() == 1 {
                steps.pop().unwrap()
            } else {
                Box::new(Sequence::new(steps)) as BoxNode<DriveCtx>
            }
        })
        .collect();
    Box::new(Repeat::forever(Box::new(Sequence::new(legs))))
}

/// Compile a sequence's children, wiring the **arrival latch** from each `drive_to`
/// to the `run_tool` leaves that follow it in the same sequence.
///
/// `sequence[drive_to, run_tool]` under a `forever` is the hand-authored (rhai/USD)
/// spelling of a patrol leg, and it hits exactly the tick-rate re-fire that
/// [`RunToolNode::arm`] describes: `Sequence` resets its children the moment it
/// completes, so a rover parked inside the drive_to radius completes the whole
/// sequence every tick and re-fires the tool at 60 Hz. [`build_patrol`] arms its own
/// legs; this is the same guarantee for every other way of writing one.
///
/// The rule: a `run_tool` fires on the ARRIVAL of the nearest preceding `drive_to`
/// in its sequence. A `run_tool` with no preceding `drive_to` is ungated (nothing
/// re-activates it at tick rate).
fn build_sequence_children(children: &[BehaviorSpec]) -> Vec<BoxNode<DriveCtx>> {
    let mut out: Vec<BoxNode<DriveCtx>> = Vec::with_capacity(children.len());
    let mut arm: Option<Arc<AtomicBool>> = None;
    for child in children {
        match child {
            BehaviorSpec::DriveTo { target, speed, radius } => {
                // Starts ARMED so the first arrival fires even when the vessel is
                // already standing on the target.
                let a = Arc::new(AtomicBool::new(true));
                arm = Some(a.clone());
                out.push(leaf_drive_to_arming(
                    Vec3::from_array(*target),
                    *speed,
                    *radius,
                    Some(a),
                ));
            }
            BehaviorSpec::RunTool { tool, args } => {
                let mut node = RunToolNode::new(tool.clone(), args.clone());
                if let Some(a) = &arm {
                    node = node.armed_by(a.clone());
                }
                out.push(Box::new(node));
            }
            other => out.push(build_tree(other)),
        }
    }
    out
}

/// Leaf: steer toward `target` (Rust nav math); `Success` once within `radius`.
fn leaf_drive_to(target: Vec3, speed: f64, radius: f32) -> BoxNode<DriveCtx> {
    leaf_drive_to_arming(target, speed, radius, None)
}

/// [`leaf_drive_to`], plus the arrival latch for this waypoint's arrival tools.
///
/// Every tick this leaf runs while *outside* `radius` — i.e. it is genuinely
/// driving to the waypoint — it arms `arm`. The waypoint's [`RunToolNode`]s
/// consume that latch when they fire. A rover parked inside the radius never runs
/// an outside-radius tick, so it never re-arms, so its tools cannot re-fire — see
/// [`RunToolNode::arm`] for why the node's own latch can't carry this.
fn leaf_drive_to_arming(
    target: Vec3,
    speed: f64,
    radius: f32,
    arm: Option<Arc<AtomicBool>>,
) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let (throttle, steer, brake, arrived) = nav_setpoint(ctx.pos, ctx.fwd, target, speed, radius);
        ctx.out = (throttle, steer, brake);
        if arrived {
            Status::Success
        } else {
            // Away from the waypoint → the next arrival is a real one.
            if let Some(a) = &arm {
                a.store(true, Ordering::Relaxed);
            }
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

/// Condition leaf: `Success` if another known vessel sits within `distance` and
/// inside the forward cone of `cone_deg` degrees — a proximity sense over the
/// `targets` snapshot, excluding `self_gid` (a vessel isn't its own obstacle).
/// Writes no setpoint. Vessel-vs-vessel only (no terrain/collider raycast — that
/// needs a spatial-query provider the headless crate deliberately doesn't pull in).
fn leaf_obstacle_ahead(distance: f32, cone_deg: f64) -> BoxNode<DriveCtx> {
    let cos_half = (cone_deg * 0.5).to_radians().cos() as f32;
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let fwd = ctx.fwd.normalize_or_zero();
        for (gid, st) in ctx.targets.iter() {
            if *gid == ctx.self_gid {
                continue; // not my own obstacle
            }
            let to = st.pos - ctx.pos;
            let d = to.length();
            if d < 1e-3 || d > distance {
                continue;
            }
            if fwd.dot(to / d) >= cos_half {
                return Status::Success; // something is ahead within the cone
            }
        }
        Status::Failure
    }))
}

/// Condition leaf: `Success` if the forward obstacle ray (see [`Clearance::ahead`])
/// hits within `distance` — the path ahead is blocked. Reads the physics-raycast
/// sensor, so it works headless; all-clear (Failure) where physics isn't running.
fn leaf_path_blocked(distance: f32) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        if ctx.clearance.ahead.is_some_and(|d| d <= distance) {
            Status::Success
        } else {
            Status::Failure
        }
    }))
}

/// Action leaf: reactive obstacle avoidance from the forward ray-fan ([`Clearance`]).
/// Clear ahead → drive straight at `speed`. Blocked → steer toward the more open of
/// the two side probes (recomputing their directions from [`PROBE_SPREAD`] so the
/// steer sign matches [`nav_setpoint`]) and ease the throttle down; brake if boxed
/// in on all three probes. Always `Running`.
fn leaf_steer_clear(speed: f64) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let c = ctx.clearance;
        let range = if c.range > 0.0 { c.range } else { SENSOR_RANGE };
        let ahead = c.ahead.unwrap_or(range);
        // Mostly clear ahead → just go.
        if ahead >= range * 0.9 {
            ctx.out = (speed, 0.0, 0.0);
            return Status::Running;
        }
        let left = c.left.unwrap_or(range);
        let right = c.right.unwrap_or(range);
        // Boxed in on every probe → stop rather than grind forward.
        if ahead < 2.0 && left < 2.0 && right < 2.0 {
            ctx.out = (0.0, 0.0, 1.0);
            return Status::Running;
        }
        // Steer toward the more open side. Recompute that probe's world direction and
        // reuse the nav yaw-cross so the steer sign is consistent with drive_to.
        let fwd = ctx.fwd.normalize_or_zero();
        let open = if left >= right { PROBE_SPREAD } else { -PROBE_SPREAD };
        let to = Quat::from_rotation_y(open) * fwd;
        let cy = fwd.z * to.x - fwd.x * to.z;
        let steer = (-cy * 2.5).clamp(-1.0, 1.0) as f64;
        // Ease throttle with how much room is ahead (never below a crawl).
        let throttle = speed * (ahead / range).clamp(0.2, 1.0) as f64;
        ctx.out = (throttle, steer, 0.0);
        Status::Running
    }))
}

/// Condition leaf: `Success` if the vessel's heading is within `tolerance_deg`
/// degrees of `target`, else `Failure`. Read-only guard counterpart to
/// [`leaf_face`] — gate a drive on being pointed the right way.
fn leaf_facing(target: Vec3, tolerance_deg: f64) -> BoxNode<DriveCtx> {
    let align_dot = tolerance_deg.to_radians().cos();
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let to = target - ctx.pos;
        let dist = to.length();
        if dist < 1e-3 {
            return Status::Success;
        }
        if ctx.fwd.normalize_or_zero().dot(to / dist) as f64 >= align_dot {
            Status::Success
        } else {
            Status::Failure
        }
    }))
}

/// Leaf: follow a **moving** entity by its [`GlobalEntityId`]. Each tick it looks
/// the target's live position up in [`DriveCtx::targets`] and steers toward it with
/// the shared [`nav_setpoint`] math, holding station (braking) within `radius`.
/// Never terminates with `Success` — following is open-ended, so it stays `Running`
/// while the target resolves and returns `Failure` (braking) if it drops out of the
/// map (despawned / out of scope), letting a fallback branch take the wheel.
fn leaf_follow(target_gid: u64, speed: f64, radius: f32) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| match ctx.targets.get(&target_gid) {
        Some(st) => {
            // Reuse the drive math; within `radius` it returns brake+arrived, which
            // for a follow means "hold station here", so we stay Running regardless.
            let (throttle, steer, brake, _arrived) = nav_setpoint(ctx.pos, ctx.fwd, st.pos, speed, radius);
            ctx.out = (throttle, steer, brake);
            Status::Running
        }
        None => {
            ctx.out = (0.0, 0.0, 1.0); // target lost → brake and let a fallback take over
            Status::Failure
        }
    }))
}

/// Leaf: lead-pursuit a **moving** entity — aim `lead` seconds ahead of the target
/// along its velocity ([`TargetState::vel`]) so the pursuer cuts it off rather than
/// tailing it, driving toward that predicted point. `Success` on **contact** (within
/// `radius` of the target's *actual* position — a catch-it pursuit that finishes,
/// unlike open-ended [`leaf_follow`]); `Failure` (braking) if the target vanishes.
fn leaf_intercept(target_gid: u64, speed: f64, radius: f32, lead: f64) -> BoxNode<DriveCtx> {
    Box::new(Action::new(move |ctx: &mut DriveCtx| match ctx.targets.get(&target_gid) {
        Some(st) => {
            let aim = st.pos + st.vel * lead as f32; // predicted lead point
            let (throttle, steer, brake, _) = nav_setpoint(ctx.pos, ctx.fwd, aim, speed, radius);
            ctx.out = (throttle, steer, brake);
            // Done when we reach the TARGET itself (not the lead point) within radius.
            if (st.pos - ctx.pos).length() < radius {
                Status::Success
            } else {
                Status::Running
            }
        }
        None => {
            ctx.out = (0.0, 0.0, 1.0); // target lost → brake, let a fallback take over
            Status::Failure
        }
    }))
}

/// Leaf: turn in place to face `target` — steer toward it with **no throttle**, so
/// the skid rover pivots without translating; `Success` once the heading is within
/// `tolerance_deg` degrees of the target. Uses the same yaw-plane steering sign as
/// [`nav_setpoint`]. Relative direction, so floating-origin invariant.
fn leaf_face(target: Vec3, tolerance_deg: f64) -> BoxNode<DriveCtx> {
    let align_dot = tolerance_deg.to_radians().cos();
    Box::new(Action::new(move |ctx: &mut DriveCtx| {
        let to = target - ctx.pos;
        let dist = to.length();
        if dist < 1e-3 {
            ctx.out = (0.0, 0.0, 0.0);
            return Status::Success; // sitting on the target: nothing to face
        }
        let to = to / dist;
        let fwd = ctx.fwd.normalize_or_zero();
        let dot = fwd.dot(to) as f64;
        if dot >= align_dot {
            ctx.out = (0.0, 0.0, 0.0); // aligned → release, hold heading
            return Status::Success;
        }
        let cy = fwd.z * to.x - fwd.x * to.z;
        let steer: f32 = if dot < 0.0 {
            if cy >= 0.0 { -1.0 } else { 1.0 } // target behind → hard pivot toward it
        } else {
            (-cy * 2.5).clamp(-1.0, 1.0)
        };
        ctx.out = (0.0, steer as f64, 0.0); // steer only — pivot in place
        Status::Running
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

/// Leaf that fires a named tool call once per activation. Stateful (not a plain
/// `Action::new`) so it doesn't re-fire every tick: a `fired` latch gates the
/// push into [`DriveCtx::fired`], and [`Node::reset`] clears it so a `repeat` /
/// `cooldown` decorator can re-arm it. Returns `Success` after firing — the
/// natural status for a one-shot inside a `Sequence` ("drive to waypoint → take
/// photo → drive on").
pub struct RunToolNode {
    tool: String,
    args: String,
    /// `true` once the tool has been queued this activation. Cleared by `reset`.
    fired: bool,
    /// Arrival latch shared with this waypoint's `drive_to` leaf (see
    /// [`build_patrol`]). `Some` only on the patrol path.
    ///
    /// The `fired` latch alone is NOT enough to stop repeat fires: `Sequence`
    /// resets its children the moment it completes, and `Repeat::forever` resets
    /// the whole lap — so a patrol whose rover is already parked inside the
    /// waypoint radius completes a lap EVERY TICK (`drive_to` succeeds
    /// immediately), re-arming `fired` and queueing the tool at tick rate. A
    /// one-waypoint patrol — exactly what a single Ctrl+LMB checkpoint builds,
    /// with the default `dwell` of 0 — would fire 60 screenshots a second.
    ///
    /// The fix is to fire on the arrival EDGE, not while parked: `drive_to` sets
    /// this flag on every tick it is *outside* the radius (i.e. actually driving
    /// there), and firing consumes it. Parked ⇒ never re-armed ⇒ never re-fires;
    /// a real patrol lap drives away and back, so it fires once per lap.
    arm: Option<Arc<AtomicBool>>,
}

impl RunToolNode {
    /// A one-shot leaf that queues `tool` (with `args`) on the next tick. Fires on
    /// every activation — use [`armed_by`](Self::armed_by) for the patrol path,
    /// where "activation" can recur at tick rate.
    pub fn new(tool: String, args: String) -> Self {
        Self { tool, args, fired: false, arm: None }
    }

    /// Gate firing on the shared arrival latch of the waypoint's `drive_to` leaf,
    /// so the tool fires once per genuine arrival rather than once per tick while
    /// parked. See [`RunToolNode::arm`].
    pub fn armed_by(mut self, arm: Arc<AtomicBool>) -> Self {
        self.arm = Some(arm);
        self
    }
}

impl Node<DriveCtx> for RunToolNode {
    fn tick(&mut self, ctx: &mut DriveCtx) -> Status {
        // Hold position while/after firing — a tool call is not a drive command.
        ctx.out = (0.0, 0.0, 1.0);
        if !self.fired {
            // Consume the arrival latch. Ungated (`None`) means "fire on every
            // activation" — the standalone/`RunTool`-spec use, where nothing
            // re-activates the node at tick rate.
            let armed = self
                .arm
                .as_ref()
                .map_or(true, |a| a.swap(false, Ordering::Relaxed));
            if armed {
                ctx.fired.push(ToolInvocation {
                    tool: self.tool.clone(),
                    args: self.args.clone(),
                });
            }
            // Latch either way: an unarmed pass is a no-op, not a retry, and the
            // leaf must still report Success so the patrol moves on.
            self.fired = true;
        }
        Status::Success
    }

    fn reset(&mut self) {
        self.fired = false;
    }
}

/// Decorator: run `child`, but abort with `Failure` if it stays `Running` past
/// `seconds` of **mission time** (a child terminal before then passes straight
/// through). The clock is [`DriveCtx::now`], so — like [`WaitNode`] — the budget
/// freezes under pause/warp. This is a *domain* decorator (it needs the context's
/// clock, and it brakes on timeout), so it lives here, not in the clock-free kernel.
pub struct TimeoutNode {
    seconds: f64,
    child: BoxNode<DriveCtx>,
    /// Mission-time instant to abort at; `None` until the first tick latches it.
    deadline: Option<f64>,
}

impl TimeoutNode {
    /// A watchdog that fails `child` if it runs longer than `seconds` of mission time.
    pub fn new(seconds: f64, child: BoxNode<DriveCtx>) -> Self {
        Self { seconds, child, deadline: None }
    }
}

impl Node<DriveCtx> for TimeoutNode {
    fn tick(&mut self, ctx: &mut DriveCtx) -> Status {
        let deadline = *self.deadline.get_or_insert(ctx.now + self.seconds);
        if ctx.now >= deadline {
            self.reset();
            ctx.out = (0.0, 0.0, 1.0); // timed out → brake instead of the last command
            return Status::Failure;
        }
        match self.child.tick(ctx) {
            Status::Running => Status::Running,
            terminal => {
                self.reset();
                terminal
            }
        }
    }

    fn reset(&mut self) {
        self.deadline = None;
        self.child.reset();
    }
}

/// Decorator: run `child`, but after each `Success` block re-entry (return
/// `Failure`) for `seconds` of **mission time** — a rate limiter, so a one-shot
/// action (fire a thruster, drop a marker, re-plan) can't re-fire every tick.
/// `Running`/`Failure` pass through and arm no cooldown. Clock is [`DriveCtx::now`],
/// so the lockout freezes under pause/warp.
pub struct CooldownNode {
    seconds: f64,
    child: BoxNode<DriveCtx>,
    /// Mission-time instant the child may next run; `0.0` = ready now.
    ready_at: f64,
}

impl CooldownNode {
    /// A rate limiter that locks `child` out for `seconds` after each success.
    pub fn new(seconds: f64, child: BoxNode<DriveCtx>) -> Self {
        Self { seconds, child, ready_at: 0.0 }
    }
}

impl Node<DriveCtx> for CooldownNode {
    fn tick(&mut self, ctx: &mut DriveCtx) -> Status {
        if ctx.now < self.ready_at {
            return Status::Failure; // still cooling down — don't even tick the child
        }
        match self.child.tick(ctx) {
            Status::Success => {
                self.ready_at = ctx.now + self.seconds;
                self.child.reset();
                Status::Success
            }
            other => other, // Running / Failure: no cooldown armed
        }
    }

    fn reset(&mut self) {
        self.ready_at = 0.0;
        self.child.reset();
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

/// The **source** [`BehaviorSpec`] an autopilot's tree was compiled from, mirrored
/// onto the **vessel** entity (not the autopilot actor) so the UI / gizmo can
/// read the waypoints back for visualization and interactive editing (Ctrl+LMB
/// append, right-click delete) without reverse-engineering the opaque compiled
/// [`AutopilotBehavior`] tree. [` BehaviorSpec `] is `Serialize`, so the UI
/// round-trips a spec (read → mutate → re-emit via `SetAutopilotBehavior`) with
/// no separate JSON mirror.
///
/// Lives next to [`AutopilotBehavior`] (which stays on the autopilot actor
/// entity) — set in [`on_engage_autopilot`] / [`on_set_autopilot_behavior`].
/// Cleared when an autopilot disengages / when [`ClearAutopilotBehavior`] fires.
#[derive(Component, Debug, Clone)]
pub struct AutopilotBehaviorSpec(pub BehaviorSpec);

impl AutopilotBehaviorSpec {
    /// Construct from a parsed spec.
    pub fn new(spec: BehaviorSpec) -> Self {
        Self(spec)
    }

    /// Parse from JSON (the same shape [`SetAutopilotBehavior`] takes).
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str::<BehaviorSpec>(json)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// Serialize back to JSON for re-emitting via `SetAutopilotBehavior`.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.0).map_err(|e| e.to_string())
    }

    /// Borrowed view of the patrol waypoints, if the spec's top-level node is a
    /// [`BehaviorSpec::Patrol`]. `None` for non-patrol trees (the UI hides the
    /// checkpoint list in that case). This is the read path the path-line gizmo
    /// and the Ctrl+LMB "append" both use.
    pub fn patrol_waypoints(&self) -> Option<&[PatrolWaypoint]> {
        match &self.0 {
            BehaviorSpec::Patrol { waypoints, .. } => Some(waypoints.as_slice()),
            _ => None,
        }
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
    mut commands: Commands,
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
                // Fire the SAME possession signal a human does (the avatar mirrors
                // native possession to `cmd:PossessVessel`), so scenario waits like
                // `wait_for("cmd:PossessVessel")` / `requires_event:"cmd:PossessVessel"`
                // fire for an autopilot claim too — controller-uniform. `source` is the
                // vessel gid so a scenario can filter by which vessel.
                commands.trigger(lunco_core::TelemetryEvent {
                    name: "cmd:PossessVessel".into(),
                    source: gid.get(),
                    severity: lunco_core::Severity::Info,
                    data: lunco_core::TelemetryValue::String("PossessVessel".into()),
                    timestamp: 0.0,
                });
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

/// Per-system memory of last tick's target positions + mission time, so
/// [`drive_autopilots`] can finite-difference a world velocity for lead-pursuit
/// ([`BehaviorSpec::Intercept`]). Held in a `Local`, so it is private to the system.
#[derive(Default)]
pub struct PrevTargets {
    poses: std::collections::HashMap<u64, Vec3>,
    now: f64,
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
    q_targets: Query<(&GlobalEntityId, &GlobalTransform)>,
    clearances: Option<Res<ClearanceField>>,
    mut prev: Local<PrevTargets>,
    mut commands: Commands,
) {
    if q.is_empty() {
        return; // no autopilots → skip the per-tick target snapshot entirely
    }
    let now = world_time.sim_secs; // the one clock — waits freeze under pause/warp
    // One snapshot of every identifiable entity's world pose + a finite-difference
    // velocity (this pos minus last tick's, over the mission-clock delta), shared
    // (cheap Arc clone) into each autopilot's ctx so `follow`/`intercept` can track a
    // mover. Velocity is zero on first sighting and under pause (dt == 0).
    let dt = (now - prev.now).max(0.0);
    let states: TargetStates = q_targets
        .iter()
        .map(|(gid, xf)| {
            let pos = xf.translation();
            let vel = if dt > 1e-6 {
                prev.poses.get(&gid.get()).map(|&p| (pos - p) / dt as f32).unwrap_or(Vec3::ZERO)
            } else {
                Vec3::ZERO
            };
            (gid.get(), TargetState { pos, vel })
        })
        .collect();
    prev.poses = states.iter().map(|(k, s)| (*k, s.pos)).collect();
    prev.now = now;
    let targets = std::sync::Arc::new(states);
    for (ap, behavior) in &mut q {
        if !ap.engaged {
            continue;
        }
        let Ok(gid) = q_gid.get(ap.vessel) else { continue };
        if !registry.owns(ap.session, gid.get()) {
            continue; // lost the vessel → stop driving (one writer per tick)
        }

        let (throttle, steer, brake, mut fired) = match (behavior, q_xf.get(ap.vessel).ok()) {
            (Some(mut tree), Some(xf)) => {
                let clearance = clearances
                    .as_ref()
                    .and_then(|c| c.0.get(&ap.vessel).copied())
                    .unwrap_or_default();
                let mut ctx = DriveCtx {
                    self_gid: gid.get(),
                    pos: xf.translation(),
                    fwd: xf.forward().as_vec3(),
                    now,
                    out: (ap.throttle, ap.steer, 0.0),
                    targets: targets.clone(),
                    clearance,
                    fired: Vec::new(),
                };
                tree.0.tick(&mut ctx);
                (ctx.out.0, ctx.out.1, ctx.out.2, ctx.fired)
            }
            _ => (ap.throttle, ap.steer, 0.0, Vec::new()),
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

        // Re-emit any tool calls queued by `RunTool` leaves this tick. Leaves
        // can't reach ECS, so they push into `DriveCtx::fired`; this is the one
        // place with `Commands` access to fan them out as `ToolFired` events.
        // `vessel_gid` (the api_id) is passed so a handler can feed it back
        // through a command's Entity field — the reflect-dispatch resolver maps
        // api_id → local Entity (NOT Entity::to_bits, which is a different u64).
        for inv in fired.drain(..) {
            commands.trigger(ToolFired {
                vessel: ap.vessel,
                vessel_gid: gid.get(),
                tool: inv.tool,
                args: inv.args,
            });
        }
    }
}

/// Collect `root` and all of its descendants into `out` — the entity set a raycast
/// must exclude so a vessel never senses its own chassis/wheel colliders.
fn collect_hierarchy(root: Entity, q_children: &Query<&Children>, out: &mut Vec<Entity>) {
    out.push(root);
    if let Ok(children) = q_children.get(root) {
        for &c in children {
            collect_hierarchy(c, q_children, out);
        }
    }
}

/// Headless obstacle sensing — a **rover** capability, independent of who drives it.
/// For every **controlled vessel** (owned by *any* session — a human or an autopilot,
/// since an autopilot is just a user with a specialty), cast a forward ray-fan from
/// the **vessel's** pose (ahead + `±`[`PROBE_SPREAD`], at each [`PROBE_HEIGHTS`]) via
/// avian [`SpatialQuery`], excluding the vessel's own hierarchy, and record the
/// nearest hit per lane into [`ClearanceField`] keyed by the vessel entity.
/// `drive_autopilots` reads it for an autopilot's vessel; a human-driven rover gets
/// the same readings (for a HUD / driver-assist). Gated on the physics pipeline
/// existing, so a physics-free app just skips it (leaves then read all-clear).
pub fn sense_clearance(
    spatial: avian3d::prelude::SpatialQuery,
    q_children: Query<&Children>,
    registry: Res<SessionRegistry>,
    q_vessels: Query<(Entity, &GlobalEntityId, &GlobalTransform)>,
    mut field: ResMut<ClearanceField>,
) {
    use avian3d::prelude::SpatialQueryFilter;
    field.0.clear();
    for (vessel, gid, xf) in &q_vessels {
        // Sense only vessels someone is actually driving (human OR autopilot). An
        // idle, unowned vessel has no consumer for its clearance, so skip it.
        if registry.owner_of(gid.get()).is_none() {
            continue;
        }
        // Level forward: drop the pitch so the probe skims a horizontal plane instead
        // of aiming into the ground (downhill) or the sky (uphill).
        let f = xf.forward().as_vec3();
        let fwd = Vec3::new(f.x, 0.0, f.z).normalize_or_zero();
        if fwd == Vec3::ZERO {
            continue;
        }
        let mut excluded = Vec::new();
        collect_hierarchy(vessel, &q_children, &mut excluded);
        let filter = SpatialQueryFilter::from_excluded_entities(excluded);
        let base = xf.translation();
        // Cast each lane at every body height, keep the nearest hit across them.
        let lane = |dir: Vec3| -> Option<f32> {
            let d = Dir3::new(dir).ok()?;
            PROBE_HEIGHTS
                .iter()
                .filter_map(|h| {
                    let origin = (base + Vec3::Y * *h).as_dvec3();
                    spatial.cast_ray(origin, d, SENSOR_RANGE as f64, true, &filter).map(|hit| hit.distance as f32)
                })
                .reduce(f32::min)
        };
        field.0.insert(
            vessel,
            Clearance {
                ahead: lane(fwd),
                left: lane(Quat::from_rotation_y(PROBE_SPREAD) * fwd),
                right: lane(Quat::from_rotation_y(-PROBE_SPREAD) * fwd),
                range: SENSOR_RANGE,
            },
        );
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
fn on_engage_autopilot(
    trigger: On<EngageAutopilot>,
    q: Query<(Entity, &Autopilot)>,
    mut commands: Commands,
) {
    let throttle = if cmd.throttle != 0.0 { cmd.throttle } else { 0.5 };
    // Re-engaging a vessel that ALREADY has an actor must reuse it. Spawning a
    // second one leaves two autopilots driving one vessel — both writing
    // `SetPorts` every tick (e.g. a stale Brake tree fighting a fresh patrol),
    // last-writer-wins. Reachable from the Command Deck, rhai and the API.
    let existing = q.iter().find(|(_, ap)| ap.vessel == cmd.vessel).map(|(e, _)| e);
    let mut e = match existing {
        Some(actor) => {
            let mut ec = commands.entity(actor);
            ec.insert(Autopilot::forward(cmd.vessel, cmd.index, throttle));
            // Drop any tree from the previous engage: an empty `spec_json` means
            // "constant forward throttle", which only holds if no stale
            // `AutopilotBehavior` is left attached to out-vote it.
            ec.try_remove::<AutopilotBehavior>();
            ec
        }
        None => commands.spawn(Autopilot::forward(cmd.vessel, cmd.index, throttle)),
    };
    if !cmd.spec_json.is_empty() {
        match AutopilotBehavior::from_json(&cmd.spec_json) {
            Ok(b) => {
                e.insert(b);
                // Mirror the source spec onto the VESSEL entity (not the
                // autopilot actor) so the UI / path-line gizmo can read the
                // waypoints back without walking the autopilot→vessel link.
                let spec = AutopilotBehaviorSpec::from_json(&cmd.spec_json);
                if let Ok(s) = spec {
                    commands.entity(cmd.vessel).try_insert(s);
                }
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
    trigger: On<SetAutopilotBehavior>,
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
            // Mirror the source spec onto the vessel (read path for UI/gizmo).
            if let Ok(spec) = AutopilotBehaviorSpec::from_json(&cmd.spec_json) {
                commands.entity(cmd.vessel).try_insert(spec);
            }
            info!("[autopilot] behaviour updated for vessel {:?}", cmd.vessel);
        }
        None => warn!("[autopilot] SetAutopilotBehavior: no autopilot owns vessel {:?}", cmd.vessel),
    }
}

/// Clear the patrol (or any behaviour) on `vessel` and stop it: sets the
/// autopilot's behaviour to [`BehaviorSpec::Brake`] AND removes the
/// [`AutopilotBehaviorSpec`] mirror from the vessel, so the path-line gizmo /
/// Command Deck stop showing checkpoints. The single canonical "stop & clear"
/// verb — replaces the hand-built `SetAutopilotBehavior` + `Brake`-JSON dance
/// that was duplicated in the Command Deck, the right-click menu, and the
/// delete-last-waypoint path (§4.2 — one input shape, every surface).
#[Command]
pub struct ClearPatrol {
    /// Vessel whose patrol to clear.
    pub vessel: Entity,
}

#[on_command(ClearPatrol)]
fn on_clear_patrol(
    _trigger: On<ClearPatrol>,
    q: Query<(Entity, &Autopilot)>,
    mut commands: Commands,
) {
    // Brake the compiled tree directly — we have the typed `BehaviorSpec::Brake`
    // value, so no JSON round-trip (unlike SetAutopilotBehavior, which receives
    // a wire string and must parse).
    let brake = AutopilotBehavior::new(&BehaviorSpec::Brake);
    match q.iter().find(|(_, ap)| ap.vessel == cmd.vessel) {
        Some((entity, _)) => {
            commands.entity(entity).insert(brake);
        }
        None => warn!("[autopilot] ClearPatrol: no autopilot owns vessel {:?}", cmd.vessel),
    }
    // Remove the source-spec mirror so the UI/gizmo stop showing checkpoints.
    // `try_remove`: ClearPatrol can legitimately arrive for an already-despawned
    // vessel (the panel defers the trigger a frame), and a plain `remove` would
    // panic at flush.
    commands.entity(cmd.vessel).try_remove::<AutopilotBehaviorSpec>();
    info!("[autopilot] ClearPatrol: vessel {:?} cleared + braked", cmd.vessel);
}

/// Disengage the autopilot on `vessel` WITHOUT clearing its patrol: replaces
/// the live behaviour with [`BehaviorSpec::Brake`] (the vessel stops) but
/// LEAVES the [`AutopilotBehaviorSpec`] mirror intact so the patrol survives a
/// later re-engage. Distinct from [`ClearPatrol`] (which wipes the patrol data
/// too) — the Command Deck "Disengage" button wants this one (pause driving,
/// keep the route).
#[Command]
pub struct DisengageAutopilot {
    /// Vessel whose autopilot to disengage (brake, keep patrol data).
    pub vessel: Entity,
}

#[on_command(DisengageAutopilot)]
fn on_disengage_autopilot(
    _trigger: On<DisengageAutopilot>,
    q: Query<(Entity, &Autopilot)>,
    mut registry: ResMut<SessionRegistry>,
    mut commands: Commands,
) {
    // Despawning the actor — not just braking it — is what makes "disengaged"
    // true for everyone: `Autopilot` is the presence signal the UI reads
    // (`autopilot_engaged = any actor whose vessel == v`), so an actor left alive
    // holding a Brake would pin the Command Deck on "Disengage" forever, with
    // "Engage" unreachable. The vessel's `AutopilotBehaviorSpec` mirror lives on
    // the VESSEL, not the actor, so the patrol survives for a later re-engage.
    match q.iter().find(|(_, ap)| ap.vessel == cmd.vessel) {
        Some((entity, ap)) => {
            // Release the AiAgent claim, or the vessel stays owned by a session
            // whose actor is gone — `may_possess` would then deny the human the
            // very vessel the UI reports as disengaged.
            let freed = registry.release_session(ap.session);
            commands.entity(entity).despawn();
            info!(
                "[autopilot] DisengageAutopilot: vessel {:?} disengaged (patrol kept, {} claim(s) freed)",
                cmd.vessel,
                freed.len()
            );
        }
        None => warn!("[autopilot] DisengageAutopilot: no autopilot owns vessel {:?}", cmd.vessel),
    }
}

/// Export a behaviour tree (JSON [`BehaviorSpec`]) to BehaviorTree.CPP v4 XML —
/// the format Groot2 edits and ROS/Nav2 runs. The result is returned in the Ack
/// (`xml`), so a rhai scenario or the API can convert a tree to a portable,
/// editable file. Round-trips with [`ImportBehaviorXml`].
#[Command]
pub struct ExportBehaviorXml {
    /// A JSON [`BehaviorSpec`] (the same shape [`SetAutopilotBehavior`] takes).
    pub spec_json: String,
}

#[on_command(ExportBehaviorXml)]
fn on_export_behavior_xml(_t: On<ExportBehaviorXml>) -> Result<Ack, String> {
    let value: serde_json::Value =
        serde_json::from_str(&cmd.spec_json).map_err(|e| format!("ExportBehaviorXml: {e}"))?;
    let xml = btcpp_xml::value_to_xml(&value)?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "xml": xml });
    Ok(ack)
}

/// Import a BehaviorTree.CPP v4 XML tree back to a JSON [`BehaviorSpec`] — the
/// inverse of [`ExportBehaviorXml`]. The JSON is returned in the Ack (`spec_json`)
/// ready to feed [`SetAutopilotBehavior`] / [`EngageAutopilot`].
#[Command]
pub struct ImportBehaviorXml {
    /// A BehaviorTree.CPP v4 XML document.
    pub xml: String,
}

#[on_command(ImportBehaviorXml)]
fn on_import_behavior_xml(_t: On<ImportBehaviorXml>) -> Result<Ack, String> {
    let value = btcpp_xml::xml_to_value(&cmd.xml)?;
    let spec_json = serde_json::to_string(&value).map_err(|e| format!("ImportBehaviorXml: {e}"))?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "spec_json": spec_json });
    Ok(ack)
}

register_commands!(
    on_engage_autopilot,
    on_set_autopilot_behavior,
    on_clear_patrol,
    on_disengage_autopilot,
    on_export_behavior_xml,
    on_import_behavior_xml
);

/// Tunable defaults for an interactively-authored patrol (§3 — no magic numbers at
/// the call sites). Domain tuning, so it lives with the autopilot rather than the
/// editor: rhai, the API and the UI all read the same knobs.
///
/// Per-waypoint / per-mission values authored in USD or the BT.CPP XML always win;
/// these are only the fallback the editor reaches for when a mission does not say.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PatrolDefaults {
    /// Cruise speed between waypoints.
    pub speed: f64,
    /// Arrival radius (m) — within this, the route advances.
    pub radius: f32,
    /// Dwell at each waypoint (s).
    pub dwell: f64,
    /// Throttle `EngageAutopilot` starts a patrol at.
    pub engage_throttle: f64,
}

impl Default for PatrolDefaults {
    fn default() -> Self {
        Self { speed: 0.6, radius: 3.0, dwell: 0.0, engage_throttle: 0.6 }
    }
}

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
        // A `run_tool` leaf fires `ToolFired`; without the dispatcher observing
        // it the event is dropped on the floor and the tool never runs. Guard-add
        // it here so the producer can never ship without its consumer.
        // Headless-safe (an observer, no rendering).
        if !app.is_plugin_added::<lunco_tools_bevy::ToolDispatchPlugin>() {
            app.add_plugins(lunco_tools_bevy::ToolDispatchPlugin);
        }
        app.init_resource::<ClearanceField>();
        app.init_resource::<PatrolDefaults>();
        // Missions authored as BT.CPP XML + USD waypoint prims. The spec on the vessel
        // is DERIVED from them; dragging a waypoint pin recompiles the route. See
        // `usd_tree`.
        app.init_asset::<usd_tree::BehaviorXmlAsset>()
            .init_asset_loader::<usd_tree::BehaviorXmlLoader>();
        app.add_systems(
            Update,
            (
                usd_tree::load_behavior_xml_assets,
                usd_tree::compile_behavior_xml,
            ),
        );
        app.add_systems(Update, setup_autopilot_session);
        // Sense obstacles (physics raycast) before driving, so `path_blocked` /
        // `steer_clear` see this tick's clearance. Gated on the avian spatial pipeline
        // so a physics-free app just skips sensing (leaves read all-clear).
        app.add_systems(
            FixedUpdate,
            (
                sense_clearance
                    .run_if(resource_exists::<avian3d::collider_tree::ColliderTrees>)
                    .before(drive_autopilots),
                drive_autopilots,
            ),
        );
        register_all_commands(app);
    }
}
