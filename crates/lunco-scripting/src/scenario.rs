//! Language-neutral scenario lifecycle — the runtime-agnostic driver.
//!
//! A *scenario* is a persistent per-entity program with native task/mission
//! policy plus lifecycle hooks (`on_start` / `on_tick` / `on_event` /
//! `on_stop`). EVERYTHING about *when*
//! those fire — scheduling, hot-reload on a generation bump, `on_event`
//! delivery, paused-simulation handling, despawn/detach teardown, diagnostics
//! reporting — is identical across languages. That orchestration lives here, in
//! [`ScenarioDriver`], free of any interpreter type.
//!
//! The only language-specific part is the *mechanics*: turning source into a
//! compiled program and calling a hook. That's the [`ScenarioRuntime`] trait —
//! one impl per language (Rhai lives in `lunco-scripting-rhai-runtime`; see the
//! Python TODO below). This mirrors
//! the [`lunco_scripting_bridge_core`] split: neutral core + thin per-language
//! binding.
//!
//! TODO(python scenarios): give Python lifecycle parity by implementing
//! `ScenarioRuntime` for a `PythonScenarioRuntime` (compile a module per entity;
//! map policy/lifecycle functions to module-level `task`/`mission` and
//! `on_start(me, ctx)`/`on_tick(me, ctx)`/`on_stop(me, ctx)`/`on_event(me, evt, ctx)`
//! functions via pyo3) and registering a `ScenarioDriver<PythonScenarioRuntime>`
//! + a `tick_python_scenarios` exclusive system. Python then gets hot-reload,
//! pause, on_stop teardown, and diagnostics FOR FREE from this driver — only the
//! ~5 trait methods are new. (The old input/output-dict execution path has been
//! removed; this hook model — with the `lunco.*` world verbs as the one way
//! Python reads/writes the world — is the only Python scenario model going
//! forward. There is currently NO Python scenario execution until this lands.)

#![cfg(any(feature = "rhai", feature = "python"))]

use bevy::prelude::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use lunco_api::registry::ApiEntityRegistry;
use lunco_command_contracts::SessionId;
use lunco_doc::{Diagnostic, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;
use lunco_telemetry_core::TelemetryEvent;

use crate::doc::ScenarioParameters;
use crate::doc::{ScriptLanguage, ScriptedModel};
use crate::ScriptRegistry;
use lunco_scripting_bridge_core as bridge_core;
use lunco_scripting_bridge_core::{ScenarioAudience, ValueBuilder};

/// Controls whether persistent scenario programs are allowed to execute their
/// lifecycle hooks.
///
/// Headless scene runners close this gate until the composed scene and its
/// readiness participants are admitted. Scenario policy can reference entities
/// other than its attached owner, so startup waits for the complete readiness
/// set. After admission, the driver idles programs whose own subtree is held.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenarioExecutionGate {
    /// Whether scenario attachment and lifecycle execution may proceed.
    pub enabled: bool,
}

impl Default for ScenarioExecutionGate {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A completed scene transaction whose participants have not all been released
/// by readiness policy yet. This is scene lifecycle state, not a second user
/// pause control.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScenarioReadinessArm(pub bool);

/// Monotonic scene replacement generation observed by scenario drivers. A
/// scenario with [`ScenarioReloadPolicy::Restart`] compares its last started
/// generation with this value and re-enters `on_start` after the new scene is
/// ready. The scene owner remains unaware of scripting-specific behavior.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScenarioSceneGeneration(pub u64);

pub fn close_scenarios_for_scene_transition(
    _trigger: On<lunco_core::SceneTransitionStarted>,
    mut gate: ResMut<ScenarioExecutionGate>,
    mut arm: ResMut<ScenarioReadinessArm>,
    inbox: Option<ResMut<ScriptEventInbox>>,
) {
    gate.enabled = false;
    arm.0 = false;
    if let Some(mut inbox) = inbox {
        inbox.reset();
    }
}

pub fn arm_scenarios_after_scene_composition(
    _trigger: On<lunco_core::SceneTransitionCompleted>,
    mut arm: ResMut<ScenarioReadinessArm>,
    mut generation: ResMut<ScenarioSceneGeneration>,
) {
    arm.0 = true;
    generation.0 = generation.0.saturating_add(1);
}

/// Open scenario lifecycle after all readiness holds for the completed scene
/// clear. Once opened it stays open: a later dynamically attached participant
/// must not rewind already-running lifecycle state.
pub fn open_scenarios_when_scene_ready(
    readiness: Option<Res<lunco_readiness::ReadinessState>>,
    mut gate: ResMut<ScenarioExecutionGate>,
    mut arm: ResMut<ScenarioReadinessArm>,
) {
    if !arm.0 {
        return;
    }
    let held = readiness
        .as_deref()
        .is_some_and(|state| state.world_hold || !state.held_entities.is_empty());
    if held {
        return;
    }
    gate.enabled = true;
    arm.0 = false;
    info!("[scenario] scene participants ready — lifecycle execution enabled");
}

/// Run condition for scenario lifecycle systems.
pub fn scenario_execution_enabled(gate: Option<Res<ScenarioExecutionGate>>) -> bool {
    gate.is_some_and(|gate| gate.enabled)
}

/// Run condition for the paused-simulation scenario pass.
///
/// `Time<Virtual>` owns pause and `SimTick` must be installed by the simulation
/// runtime. The paused pass keeps discrete lifecycle events responsive while
/// the fixed simulation clock is stopped; it never advances `on_tick`, task,
/// or mission work.
pub fn simulation_is_paused(
    time: Option<Res<Time<Virtual>>>,
    tick: Option<Res<lunco_core_runtime::SimTick>>,
) -> bool {
    tick.is_some() && time.is_some_and(|time| time.is_paused())
}

/// Run condition for continuous fixed-step scenario behavior. Every consumer
/// that mutates simulation state must use the same virtual-clock predicate as
/// the time spine and co-simulation master; otherwise a residual fixed overstep
/// can execute `on_tick` while the shared barrier is paused and advance Rhai
/// state without advancing `SimTick` or physics. The tick resource is required
/// because it is the causal ordering boundary for script execution and events.
pub fn simulation_is_running(
    time: Option<Res<Time<Virtual>>>,
    tick: Option<Res<lunco_core_runtime::SimTick>>,
) -> bool {
    tick.is_some() && lunco_time::simulation_is_running(time)
}

#[cfg(test)]
mod readiness_gate_tests {
    use super::*;
    use lunco_core::{SceneTransition, SceneTransitionCompleted, SceneTransitionStarted};
    use lunco_readiness::ReadinessState;

    fn transition_id(transition: SceneTransition) -> lunco_core::SceneTransitionId {
        lunco_core::SceneTransitionCoordinator::default().start(transition)
    }

    #[test]
    fn each_scene_transition_opens_once_after_readiness_clears() {
        let mut app = App::new();
        app.init_resource::<ScenarioExecutionGate>()
            .init_resource::<ScenarioReadinessArm>()
            .init_resource::<ScenarioSceneGeneration>()
            .init_resource::<ReadinessState>()
            .add_observer(close_scenarios_for_scene_transition)
            .add_observer(arm_scenarios_after_scene_composition)
            .add_systems(Update, open_scenarios_when_scene_ready);

        let clear = SceneTransition::clear();
        let clear_id = transition_id(clear.clone());
        app.world_mut().trigger(SceneTransitionStarted {
            id: clear_id,
            transition: clear.clone(),
        });
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(!app.world().resource::<ScenarioReadinessArm>().0);

        app.world_mut().trigger(SceneTransitionCompleted {
            id: clear_id,
            transition: clear,
        });
        app.world_mut().resource_mut::<ReadinessState>().world_hold = true;
        app.update();
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(app.world().resource::<ScenarioReadinessArm>().0);

        {
            let mut readiness = app.world_mut().resource_mut::<ReadinessState>();
            readiness.world_hold = false;
            readiness.held_entities = vec![Entity::from_raw_u32(12).unwrap()];
        }
        app.update();
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(app.world().resource::<ScenarioReadinessArm>().0);

        app.world_mut()
            .resource_mut::<ReadinessState>()
            .held_entities
            .clear();
        app.update();
        assert!(app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(!app.world().resource::<ScenarioReadinessArm>().0);

        // A participant arriving after scenario start cannot rewind lifecycle.
        app.world_mut().resource_mut::<ReadinessState>().world_hold = true;
        app.update();
        assert!(app.world().resource::<ScenarioExecutionGate>().enabled);

        // The next authoritative transition closes it again.
        let next = SceneTransition::load("next.usda", "");
        app.world_mut().trigger(SceneTransitionStarted {
            id: transition_id(next.clone()),
            transition: next,
        });
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
    }

    #[test]
    fn readiness_holds_only_scenarios_in_the_held_entity_subtree() {
        let mut world = World::new();
        let held_root = world.spawn_empty().id();
        let held_child = world.spawn(ChildOf(held_root)).id();
        let held_grandchild = world.spawn(ChildOf(held_child)).id();
        let independent = world.spawn_empty().id();

        assert!(scenario_owner_is_held(&world, held_root, &[held_root]));
        assert!(scenario_owner_is_held(
            &world,
            held_grandchild,
            &[held_root]
        ));
        assert!(!scenario_owner_is_held(&world, independent, &[held_root]));
        assert!(!scenario_owner_is_held(&world, held_grandchild, &[]));
    }
}

fn scenario_owner_is_held(world: &World, entity: Entity, held_roots: &[Entity]) -> bool {
    let mut current = entity;
    loop {
        if held_roots.contains(&current) {
            return true;
        }
        let Some(parent) = world.get::<ChildOf>(current).map(ChildOf::parent) else {
            return false;
        };
        if parent == current {
            return false;
        }
        current = parent;
    }
}

/// The session a scenario acts on behalf of — captured at attach from the wire
/// origin ([`lunco_core_session::SyncApplyGuard`]). `Some` only for a scenario
/// launched by a *remote* networked session; the driver sets it as the `cmd()`
/// authority for that entity's hooks, so a remote script can't exceed its
/// submitter's authority (design §3.4). Absent / `None` ⇒ host-trusted launch
/// (local, standalone, USD-embedded) → ungated, matching the open-by-default
/// substrate (and side-stepping the default-deny `SessionRbac` would apply where
/// no sessions are registered).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ScriptAuthority(pub Option<SessionId>);

/// Resolve whether this run is [`Attended`](ScenarioAudience::Attended) from
/// application facts. The UI adapter supplies window presence; headless hosts
/// can pass `false` without depending on Bevy's window subsystem.
pub fn scenario_audience(window_present: bool, override_value: Option<&str>) -> ScenarioAudience {
    match override_value {
        Some("1") | Some("true") => ScenarioAudience::Unattended,
        Some("0") | Some("false") => ScenarioAudience::Attended,
        _ if !window_present => ScenarioAudience::Unattended,
        _ => ScenarioAudience::Attended,
    }
}

/// Resolve the audience at startup for windowed hosts. The `window-audience`
/// feature is opt-in so the generic scripting host remains window-free in
/// headless builds.
#[cfg(feature = "window-audience")]
pub fn resolve_scenario_audience(
    windows: Query<(), With<Window>>,
    mut audience: ResMut<ScenarioAudience>,
) {
    *audience = scenario_audience(
        !windows.is_empty(),
        std::env::var("LUNCO_SCENARIO_UNATTENDED").ok().as_deref(),
    );
    info!("[scenario] audience: {:?}", *audience);
}

/// Where a scenario's lifecycle hooks execute. Default [`Host`](ScriptScope::Host):
/// a predicting client must not run sim-mutating scripts (they would double-apply
/// or fight replication — the same reason cosim/physics only step on the host).
///
/// - `Host` — host / standalone only (the safe default; all existing scenarios).
/// - `Client` — **client only**: presentation / HUD / camera / tutorial coaching.
///   Its `cmd()`s are restricted to the client-local surface
///   ([`lunco_core::ClientCommandPolicy`]); an authoritative command is dropped.
/// - `Both` — every peer (each peer still filtered by the same client-local rule
///   when it is the client).
///
/// Authored via a `// @scope host|client|both` directive on one of the first
/// lines of the script source, so it rides the same channel for API-attached
/// (`RunScenario`) and USD-embedded scenarios with no wire or schema change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptScope {
    #[default]
    Host,
    Client,
    Both,
    /// A declared peer scope was not recognized; the scenario is disabled.
    Unsupported,
}

impl ScriptScope {
    /// Whether a scenario with this scope should tick on the current peer.
    pub fn runs_on(self, is_client: bool) -> bool {
        match self {
            ScriptScope::Host => !is_client,
            ScriptScope::Client => is_client,
            ScriptScope::Both => true,
            ScriptScope::Unsupported => false,
        }
    }

    fn is_unsupported(self) -> bool {
        matches!(self, Self::Unsupported)
    }
}

/// Clock cycle requested by a persistent scenario.
///
/// The scenario host currently admits stateful hooks only to deterministic
/// simulation. This declaration makes that contract explicit without allowing
/// authored text to install or move systems between Rust schedules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptTiming {
    /// The fixed simulation cycle owned by the scenario runtime.
    #[default]
    Simulation,
    /// A timing value the scenario runtime does not support.
    Unsupported,
}

/// Parsed, source-revision-owned scheduling metadata for one scenario.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScenarioDirectives {
    /// Network peer on which this scenario may execute.
    pub scope: ScriptScope,
    /// Runtime cycle requested by this scenario.
    pub timing: ScriptTiming,
}

impl ScenarioDirectives {
    /// Parse known directives from the source preamble.
    ///
    /// Unknown scope and timing values are retained as unsupported metadata so
    /// the owner can skip this scenario and publish a document diagnostic.
    pub fn from_source(src: &str) -> Self {
        let mut directives = Self::default();
        for line in src.lines().take(24) {
            let t = line.trim_start();
            let Some(rest) = t.strip_prefix("//") else {
                continue;
            };
            let rest = rest.trim_start().trim_start_matches('!').trim_start();
            if let Some(value) = rest.strip_prefix("@scope") {
                directives.scope = match value.trim().to_ascii_lowercase().as_str() {
                    "host" => ScriptScope::Host,
                    "client" => ScriptScope::Client,
                    "both" => ScriptScope::Both,
                    _ => ScriptScope::Unsupported,
                };
            } else if let Some(value) = rest.strip_prefix("@timing") {
                directives.timing = match value.trim().to_ascii_lowercase().as_str() {
                    "simulation" => ScriptTiming::Simulation,
                    _ => ScriptTiming::Unsupported,
                };
            }
        }
        directives
    }

    fn is_supported(self) -> bool {
        !self.scope.is_unsupported() && self.timing != ScriptTiming::Unsupported
    }

    fn diagnostics(self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.scope.is_unsupported() {
            diagnostics.push(Diagnostic::error(
                "unknown scenario @scope directive; expected host, client, or both",
                None,
                None,
            ));
        }
        if self.timing == ScriptTiming::Unsupported {
            diagnostics.push(Diagnostic::error(
                "unsupported scenario @timing directive; expected simulation",
                None,
                None,
            ));
        }
        diagnostics
    }
}

/// The lifecycle hook points a scenario may define.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioHook {
    /// Once, after a (re)compile.
    Start,
    /// Every simulation step.
    Tick,
    /// At teardown (hot-reload swap, despawn, or detach).
    Stop,
}

/// Outcome of (re)compiling a scenario for an entity.
pub enum CompileOutcome {
    /// Parse/compile failed — no runnable program this tick. Fatal diagnostic.
    Failed(Diagnostic),
    /// Compiled. `top_level` carries a non-fatal init-time error if the
    /// top-level body ran but errored (the hooks still run).
    Ready {
        /// Init-time runtime error from running the top-level body, if any.
        top_level: Option<Diagnostic>,
    },
}

/// A read-only view of a running scenario's live state, for introspection. The
/// language-neutral half of the `ScriptInspect` query (the FSM half is on
/// [`ScenarioDriver`]); a backend fills this in from its compiled per-entity
/// program.
///
/// Generic over the value type `V` so the backend builds `state` *natively* via a
/// [`ValueBuilder`]. The API boundary can use its typed value builder and
/// serialize once for an external response; JSON is never an internal transform.
pub struct ScenarioSnapshot<V> {
    /// The scenario's per-entity state object (rhai `this`, future Python state),
    /// built into `V` by the caller's builder. The builder's `unit` if none.
    pub state: V,
    /// Which policy/lifecycle entrypoints the compiled program actually defines
    /// (`task` / `mission` / `on_start` / `on_tick` / `on_event` / `on_stop`).
    pub hooks: Vec<String>,
}

/// Live introspection of one entity's scenario: the neutral FSM state plus the
/// backend's [`ScenarioSnapshot`]. The data behind the `ScriptInspect` query —
/// lets an author/agent see a *running* scenario's state, not just its errors.
/// Generic over the value type `V` (see [`ScenarioSnapshot`]).
pub struct ScenarioIntrospection<V> {
    /// `ScriptDocument.generation` the live program was compiled from.
    pub generation: u64,
    /// Whether `on_start` has run for the current program.
    pub started: bool,
    /// Whether a compiled program is currently held.
    pub compiled: bool,
    /// Last-known host gid (the `self` hooks receive).
    pub gid: i64,
    /// The scenario's live state object as a native value `V` (the builder's
    /// `unit` if none).
    pub state: V,
    /// Policy/lifecycle entrypoints the program defines.
    pub hooks: Vec<String>,
}

/// A language backend that runs persistent per-entity scenarios. Supplies ONLY
/// the language mechanics — [`ScenarioDriver`] owns all lifecycle policy.
///
/// Each method is keyed by the host `entity`; the impl owns the per-entity
/// compiled state internally. `self_gid` is the host's `GlobalEntityId` when
/// one exists, or the telemetry bus' explicit global/local source `0` for a
/// local host (the `self` a hook receives), passed in so the impl never resolves
/// identity itself.
///
// TODO(hooks): evaluate folding this onto the `lunco-hooks` registry (the
// language-neutral internal-hook substrate that backs `MergePolicy` and
// `rbac.authorize`). NOT done yet — deliberately —
// because the two are different shapes and a forced migration is lateral churn on
// a working system with no clear win:
//   - `lunco-hooks` = STATELESS, GLOBAL policy functions keyed by a `HookId`
//     string (`HookValue in → HookValue out`, one call, no identity).
//   - `ScenarioRuntime` = STATEFUL, PER-ENTITY behaviour programs: a persistent
//     `this` state object, a Start/Tick/Stop lifecycle FSM (`ScenarioDriver`),
//     hot-reload/recompile, and replication — none of which the flat registry
//     models.
// They already share the SAME foundations (`bridge_core::ValueBuilder`, the
// mechanism/per-language-binding split), so nothing is duplicated by keeping them
// separate. Revisit ONLY if a concrete need appears — most likely "a scenario
// script should be able to CALL a registered hook", which is a small ADDITIVE
// bridge (invoke a `HookId` from a scenario verb), not a migration of this trait.
pub trait ScenarioRuntime: Send + Sync + 'static {
    /// (Re)compile `source` for `entity`, replacing any prior program and running
    /// its top-level init. The driver guarantees the previous program's
    /// `on_stop` has already been called before this. `params` is the validated,
    /// instance-owned launch context; the backend exposes it through its native
    /// value model.
    ///
    /// `asset_id` is the canonical id the source was loaded from
    /// (`twin://ep1/main.rhai`), or `None` when it is not file-backed. It is the
    /// script's IDENTITY, and a backend with a module system needs it to anchor
    /// relative imports (rhai: `AST::set_source`, which rhai hands to
    /// `ModuleResolver::resolve` as the importing script). Backends without one
    /// ignore it.
    fn compile(
        &mut self,
        entity: Entity,
        source: &str,
        params: &ScenarioParameters,
        asset_id: Option<&str>,
    ) -> CompileOutcome;

    /// Call a lifecycle hook for `entity` — a no-op if the scenario doesn't
    /// define it or has no compiled program. Returns a runtime-error diagnostic
    /// if the hook ran and failed.
    fn call_hook(
        &mut self,
        entity: Entity,
        hook: ScenarioHook,
        self_gid: i64,
    ) -> Option<Diagnostic>;

    /// Deliver one event to `entity`'s event hook (no-op if undefined).
    fn deliver_event(
        &mut self,
        entity: Entity,
        self_gid: i64,
        event: &TelemetryEvent,
    ) -> Option<Diagnostic>;

    /// Drop all per-entity state for `entity` (after its `on_stop`).
    fn forget(&mut self, entity: Entity);

    /// Read-only snapshot of `entity`'s running program — its live state object
    /// and the lifecycle hooks it defines — for the `ScriptInspect` query. The
    /// backend builds `state` into the caller's native value type via `builder`
    /// (so JSON only appears at an API seam, never as an internal hop). Default
    /// `None`: the backend exposes nothing inspectable.
    fn snapshot<B: ValueBuilder>(
        &self,
        _entity: Entity,
        _builder: &B,
    ) -> Option<ScenarioSnapshot<B::Value>> {
        None
    }

    /// Per-run global maintenance (e.g. hot-reload of shared modules). Runs once
    /// at the start of each driver pass, inside the World scope. Default: no-op.
    fn maintain(&mut self) {}

    /// Discard backend programs after a shared runtime contract changes.
    ///
    /// The neutral driver also drops its lifecycle bookkeeping. The next pass
    /// recompiles the still-attached scene programs against the new contract.
    /// Backends without compiled shared state can keep the default.
    fn invalidate(&mut self) {}
}

/// Neutral per-entity lifecycle bookkeeping — the FSM the driver owns. Kept
/// separate from the backend's compiled state so the policy stays language-free.
#[derive(Default)]
struct Fsm {
    /// `ScriptDocument.generation` the current program was compiled from.
    generation: u64,
    /// Document that owns the program and its lifecycle diagnostics.
    document_id: Option<u64>,
    /// Source revision most recently sent to the compiler, including a failed
    /// compile. A broken revision is terminal until the document changes; retrying
    /// it every fixed tick only floods the log and repeats work that cannot succeed.
    attempted_generation: Option<u64>,
    /// Parameter revision most recently sent to the compiler, including a
    /// failed compile. Parameters belong to the attached instance rather than
    /// the reusable source document.
    parameters_revision: u64,
    /// Whether `on_start` has run for the current program.
    started: bool,
    /// Whether the backend currently holds a compiled program for this entity.
    compiled: bool,
    /// Last-known host gid — so `on_stop` has a meaningful `self` after despawn.
    /// The derived default `0` is the telemetry bus' explicit global/no-entity
    /// source. A local script host has no GlobalEntityId, so `-1` would leak
    /// `u64::MAX` into emitted events.
    gid: i64,
    /// Scene generation at which this scenario last entered `on_start`.
    scene_generation: u64,
    /// Source revision whose scheduling metadata is cached below.
    directives_generation: Option<u64>,
    /// Parsed peer scope and execution timing for that source revision.
    directives: ScenarioDirectives,
    /// Source generation whose unsupported directives were diagnosed.
    directives_diagnostic_generation: Option<u64>,
}

fn publish_scenario_stop_error(
    world: &mut World,
    document_id: Option<u64>,
    diagnostic: Diagnostic,
) {
    let Some(raw) = document_id else {
        bevy::log::error!(
            "[scenario] on_stop failed without an owning script document: {}",
            diagnostic.message
        );
        return;
    };
    if let Some(mut diagnostics) = world.get_resource_mut::<DocumentDiagnostics>() {
        let document = DocumentId::new(raw);
        let mut current = diagnostics.diagnostics(document).to_vec();
        current.push(diagnostic);
        diagnostics.set_error(document, current);
    } else {
        bevy::log::error!(
            "[scenario] on_stop failed for script document {raw}, but document diagnostics are unavailable"
        );
    }
}

fn scenario_execution_context(
    world: &World,
    simulation_cycle: bool,
    scene_generation: Option<u64>,
) -> lunco_core::RuntimeExecutionContext {
    let (sequence, fixed_sample) = if simulation_cycle {
        (
            world
                .get_resource::<lunco_core_runtime::SimTick>()
                .map(|tick| tick.0),
            world
                .get_resource::<Time<Fixed>>()
                .map(|time| (time.timestep().as_secs_f64(), time.delta_secs_f64())),
        )
    } else {
        (None, None)
    };
    let cycle = if simulation_cycle {
        lunco_core::RuntimeCycle::Simulation
    } else {
        lunco_core::RuntimeCycle::Lifecycle
    };
    let (clock, time_seconds, delta_seconds) = if simulation_cycle {
        let timestep = fixed_sample.map(|(timestep, _)| timestep);
        let delta = fixed_sample.map(|(_, delta)| delta);
        (
            lunco_core::RuntimeClock::Simulation,
            sequence.zip(timestep).map(|(tick, dt)| tick as f64 * dt),
            delta,
        )
    } else {
        (lunco_core::RuntimeClock::None, None, None)
    };
    lunco_core::RuntimeExecutionContext {
        route: scene_generation.map(|generation| lunco_core::RuntimeRoute::twin(cycle, generation)),
        phase: lunco_core::RuntimePhase::Unclassified,
        clock,
        time_seconds,
        delta_seconds,
        sequence,
        producer: None,
    }
}

fn report_missing_scenario_generation(world: &mut World) {
    const DETAIL: &str = "scenario execution requires the active Twin scene generation";
    bevy::log::error!(target: "scripting", "{DETAIL}");
    if let Some(mut faults) = world.get_resource_mut::<lunco_core::RuntimeFaults>() {
        faults.raise(
            "scenario-generation-missing",
            None,
            "Rhai scenario driver",
            DETAIL,
        );
    }
}

/// Generic scenario runtime resource: a language backend `R` + the neutral FSM.
/// One instance per language (`ScenarioDriver<RhaiScenarioRuntime>`, …).
#[derive(Resource)]
pub struct ScenarioDriver<R: ScenarioRuntime> {
    /// The language backend (owns compiled per-entity programs).
    pub runtime: R,
    /// Per-entity lifecycle state.
    fsm: HashMap<Entity, Fsm>,
}

impl<R: ScenarioRuntime + Default> Default for ScenarioDriver<R> {
    fn default() -> Self {
        Self {
            runtime: R::default(),
            fsm: HashMap::new(),
        }
    }
}

impl<R: ScenarioRuntime> ScenarioDriver<R> {
    /// Invalidate every attached program after a shared runtime contract, such
    /// as the authored Rhai prelude, has changed. The scene entities remain
    /// attached; their programs are rebuilt on the next enabled pass.
    #[cfg(feature = "rhai")]
    pub fn invalidate(&mut self) {
        self.fsm.clear();
        self.runtime.invalidate();
    }

    /// Stop one scenario synchronously at an ownership boundary.
    ///
    /// The ordinary dead-entity path runs during the next driver tick. That is
    /// correct for an entity disappearing during normal ECS work, but it is too
    /// late for a scene transition: the old `on_stop` must run before the old
    /// scene is despawned, otherwise its cleanup commands can target the next
    /// scene. Scene and tutorial teardown call this method before removing the
    /// owning entity.
    pub fn stop_entity(world: &mut World, entity: Entity) {
        let Some(driver) = world.get_resource::<ScenarioDriver<R>>() else {
            return;
        };
        if !driver.fsm.contains_key(&entity) {
            return;
        }

        let mut stop_error = None;
        let mut document_id = None;
        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let Some(state) = driver.fsm.remove(&entity) else {
                return;
            };
            document_id = state.document_id;
            let generation = world
                .get_resource::<ScenarioSceneGeneration>()
                .map(|generation| generation.0);
            let context = scenario_execution_context(world, false, generation)
                .with_phase(lunco_core::RuntimePhase::Stop);
            let _scope = bridge_core::WorldScope::enter(world, context);
            // The entity is still present, but the scene/tutor owns the
            // transition. Its final cleanup is host-authoritative, matching
            // the normal despawn teardown path below.
            bridge_core::set_script_authority(None);
            bridge_core::set_script_client_local(false);
            if state.started && state.compiled {
                let _phase = bridge_core::ExecutionContextScope::enter(context);
                stop_error = driver
                    .runtime
                    .call_hook(entity, ScenarioHook::Stop, state.gid);
            }
            driver.runtime.forget(entity);
        });
        if let Some(diagnostic) = stop_error {
            publish_scenario_stop_error(world, document_id, diagnostic);
        }
    }

    /// Exclusive-system body: drive every non-paused `ScriptedModel { language }`
    /// through its lifecycle against the live World, including its fixed-step
    /// behavior.
    pub fn run(world: &mut World, language: ScriptLanguage) {
        Self::run_with_tick(world, language, true);
    }

    /// Exclusive-system body for a paused simulation pass.
    ///
    /// Lifecycle startup and queued discrete events remain responsive while
    /// `Time<Virtual>` stops the fixed simulation schedule. Continuous hooks,
    /// native tasks, and missions are intentionally left to [`Self::run`].
    pub fn run_without_simulation_tick(world: &mut World, language: ScriptLanguage) {
        Self::run_with_tick(world, language, false);
    }

    fn run_with_tick(world: &mut World, language: ScriptLanguage, run_tick: bool) {
        let Some(scene_generation) = world
            .get_resource::<ScenarioSceneGeneration>()
            .map(|generation| generation.0)
        else {
            report_missing_scenario_generation(world);
            return;
        };
        let pass_context = scenario_execution_context(world, run_tick, Some(scene_generation));
        // 1. Snapshot (entity, doc_id, gid, source revision, parameter revision),
        //    releasing every
        //    World borrow before we execute scripts. `live` = all THIS-LANGUAGE
        //    entities (incl. paused) — drives despawn/detach teardown.
        // (entity, doc_id, gid, generation, parameter revision, maybe (source,
        // parameters, asset-id), authority). Source+parameters+id are cloned only
        // when a (re)compile is due (see below) — not every tick — since they're
        // consumed solely by `runtime.compile`.
        type CompileInput = (String, ScenarioParameters, Option<String>);
        let mut diag_updates: Vec<(u64, Option<Vec<Diagnostic>>)> = Vec::new();
        let mut work: Vec<(
            Entity,
            u64,
            i64,
            u64,
            u64,
            Option<CompileInput>,
            Option<SessionId>,
            crate::doc::ScenarioReloadPolicy,
            u64,
            ScenarioDirectives,
        )> = Vec::new();
        // A predicting client only ticks scenarios scoped to run there
        // (`Client`/`Both`); the host ticks `Host`/`Both`. Read once — constant
        // for the whole pass.
        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let current_sim_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0);
        let held_roots = world
            .get_resource::<lunco_readiness::ReadinessState>()
            .map(|state| state.held_entities.clone())
            .unwrap_or_default();
        let live: HashSet<Entity>;
        {
            let mut q = world.query::<(Entity, &ScriptedModel, Option<&ScriptAuthority>)>();
            let models: Vec<(
                Entity,
                bool,
                Option<ScriptLanguage>,
                Option<u64>,
                Option<SessionId>,
                crate::doc::ScenarioReloadPolicy,
                u64,
            )> = q
                .iter(world)
                .map(|(e, m, auth)| {
                    (
                        e,
                        m.paused,
                        m.language,
                        m.document_id,
                        auth.and_then(|a| a.0),
                        m.reload_policy,
                        m.parameters_revision,
                    )
                })
                .collect();
            live = models
                .iter()
                .filter(|(_, _, l, _, _, _, _)| *l == Some(language))
                .map(|(e, ..)| *e)
                .collect();

            for (entity, paused, lang, doc_id, authority, reload_policy, parameters_revision) in
                models
            {
                if lang != Some(language) {
                    continue;
                }
                let Some(raw) = doc_id else { continue };
                let (generation, maybe_src, directives, directives_changed, prior_diag) = {
                    let registry = world.resource::<ScriptRegistry>();
                    let Some(host) = registry.documents.get(&DocumentId::new(raw)) else {
                        continue;
                    };
                    let doc = host.document();
                    if doc.language != language {
                        continue;
                    }
                    let generation = doc.generation;
                    let state = world
                        .get_resource::<ScenarioDriver<R>>()
                        .and_then(|driver| driver.fsm.get(&entity));
                    let directives_changed =
                        state.is_none_or(|state| state.directives_generation != Some(generation));
                    let directives = match state {
                        Some(state) if !directives_changed => state.directives,
                        _ => ScenarioDirectives::from_source(&doc.source),
                    };
                    let prior_diag = state.and_then(|state| state.directives_diagnostic_generation);
                    // Only (re)compilation reads source/params, so clone them ONLY when a
                    // recompile is actually due (first sight or generation bump) — otherwise
                    // the multi-KB source was cloned and dropped unused every tick. This
                    // A failed compile is also an attempted revision. Do not compile it
                    // again until the author changes the document: the diagnostic is
                    // already published and the same text cannot become valid by ticking.
                    let can_compile = directives.is_supported()
                        && !paused
                        && directives.scope.runs_on(is_client)
                        && !scenario_owner_is_held(world, entity, &held_roots);
                    let needs_recompile = can_compile
                        && state.is_none_or(|state| {
                            state.attempted_generation != Some(generation)
                                || state.parameters_revision != parameters_revision
                                || (reload_policy == crate::doc::ScenarioReloadPolicy::Restart
                                    && state.started
                                    && state.scene_generation != scene_generation)
                        });
                    let maybe_src = needs_recompile.then(|| {
                        let parameters = world
                            .get::<ScriptedModel>(entity)
                            .map(|model| model.parameters.clone())
                            .unwrap_or_default();
                        (doc.source.clone(), parameters, doc.asset_id.clone())
                    });
                    (
                        generation,
                        maybe_src,
                        directives,
                        directives_changed,
                        prior_diag,
                    )
                };

                if directives_changed {
                    if let Some(mut driver) = world.get_resource_mut::<ScenarioDriver<R>>() {
                        let state = driver.fsm.entry(entity).or_default();
                        state.directives_generation = Some(generation);
                        state.directives = directives;
                    }
                }

                if directives_changed && directives.is_supported() && prior_diag.is_some() {
                    if let Some(mut driver) = world.get_resource_mut::<ScenarioDriver<R>>() {
                        driver
                            .fsm
                            .entry(entity)
                            .or_default()
                            .directives_diagnostic_generation = None;
                    }
                    diag_updates.push((raw, None));
                }
                if !directives.is_supported() {
                    // Invalid authored routing/timing is local to this scenario.
                    // It bypasses peer, pause, and readiness gates so a changed
                    // invalid revision can stop an already-running program and
                    // publish its document error immediately.
                    let settled = world
                        .get_resource::<ScenarioDriver<R>>()
                        .and_then(|driver| driver.fsm.get(&entity))
                        .is_some_and(|state| {
                            state.directives_diagnostic_generation == Some(generation)
                                && !state.started
                                && !state.compiled
                        });
                    if settled {
                        continue;
                    }
                } else {
                    if paused {
                        continue;
                    }
                    if !directives.scope.runs_on(is_client) {
                        let active = world
                            .get_resource::<ScenarioDriver<R>>()
                            .and_then(|driver| driver.fsm.get(&entity))
                            .is_some_and(|state| state.started || state.compiled);
                        if !active {
                            continue;
                        }
                    }
                    // Readiness freezes a physical subtree. Scripts owned
                    // anywhere inside that subtree wait with it, while unrelated
                    // scenarios keep participating in the fixed step. A peer
                    // change still reaches the owner above to stop old state.
                    if directives.scope.runs_on(is_client)
                        && scenario_owner_is_held(world, entity, &held_roots)
                    {
                        continue;
                    }
                }

                let gid = world
                    .resource::<ApiEntityRegistry>()
                    .api_id_for(entity)
                    .map(|g| g.get() as i64)
                    .unwrap_or(0);
                work.push((
                    entity,
                    raw,
                    gid,
                    generation,
                    parameters_revision,
                    maybe_src,
                    authority,
                    reload_policy,
                    scene_generation,
                    directives,
                ));
            }
        }

        // Query/archetype order is not a simulation contract. `cmd()` is
        // synchronous inside each hook, so the actor order is observable by
        // later actors in this same pass. Addressable actors use their stable
        // API identity; local-only hosts (identity 0 in the script ABI) use
        // their stable-for-this-world ECS entity key as the final tie-breaker.
        work.sort_unstable_by(|a, b| {
            a.2.cmp(&b.2)
                .then_with(|| a.0.to_bits().cmp(&b.0.to_bits()))
        });

        // A fixed-step event is eligible after SimTickSet only when its recorded
        // tick precedes the current tick. That boundary determines when the
        // event becomes visible; events stamped at the current tick remain
        // queued until a later tick. The paused Update pass drains all
        // events to keep discrete lifecycle events responsive while simulation
        // is stopped. Drain before the no-work return so traffic cannot accrue.
        let mut events: Vec<TelemetryEvent> = world
            .get_resource_mut::<ScriptEventInbox>()
            .map(|mut inbox| {
                if inbox.faulted {
                    inbox.pending.clear();
                    Vec::new()
                } else if run_tick {
                    let Some(current_tick) = current_sim_tick else {
                        return Vec::new();
                    };
                    if inbox.pending.len() > 1 {
                        inbox.pending.sort_by(compare_telemetry_events);
                    }
                    let ready_count = inbox
                        .pending
                        .iter()
                        .take_while(|event| {
                            lunco_core_runtime::SimTick(current_tick)
                                .wrapping_diff(lunco_core_runtime::SimTick(event.sim_tick))
                                > 0
                        })
                        .count();
                    if ready_count == 0 {
                        Vec::new()
                    } else {
                        let mut ready = std::mem::take(&mut inbox.pending);
                        inbox.pending = ready.split_off(ready_count);
                        ready
                    }
                } else {
                    std::mem::take(&mut inbox.pending)
                }
            })
            .unwrap_or_default();
        if events.len() > 1 {
            events.sort_by(compare_telemetry_events);
        }

        // Run if there's work OR a tracked entity vanished (needs on_stop).
        let needs_teardown = world
            .get_resource::<ScenarioDriver<R>>()
            .is_some_and(|d| d.fsm.keys().any(|e| !live.contains(e)));
        if work.is_empty() && !needs_teardown && diag_updates.is_empty() {
            // No scenario can consume this batch. Preserve the allocation for
            // the next pass, but intentionally discard the events themselves.
            events.clear();
            if let Some(mut inbox) = world.get_resource_mut::<ScriptEventInbox>() {
                inbox.pending = events;
            }
            return;
        }

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let _scope = bridge_core::WorldScope::enter(world, pass_context);
            let _phase = bridge_core::ExecutionContextScope::enter(
                pass_context.with_phase(lunco_core::RuntimePhase::Preparation),
            );
            driver.runtime.maintain();
            let ScenarioDriver { runtime, fsm } = &mut *driver;

            // Live client hooks are restricted to the client-local command
            // surface. Invalid or newly rerouted scenarios may also reach this
            // pass solely to stop old state and publish their document diagnostic.
            bridge_core::set_script_client_local(is_client);

            for (
                entity,
                raw,
                gid,
                generation,
                parameters_revision,
                maybe_src,
                authority,
                reload_policy,
                scene_generation,
                directives,
                ) in work
            {
                // Gate this entity's hook `cmd()`s against the launching session
                // (§3.4). `None` for a host-trusted launch → ungated. Covers the
                // hot-reload `on_stop` below too (still inside this iteration).
                bridge_core::set_script_authority(authority);
                let st = fsm.entry(entity).or_default();
                st.document_id = Some(raw);
                st.directives_generation = Some(generation);
                st.directives = directives;
                let directive_invalid = !directives.is_supported();
                let runs_on_peer = directives.scope.runs_on(is_client);
                if directive_invalid || !runs_on_peer {
                    let scene_restart = reload_policy
                        == crate::doc::ScenarioReloadPolicy::Restart
                        && st.started
                        && st.scene_generation != scene_generation;
                    let stop_error = if st.started && st.compiled && !scene_restart {
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context.with_phase(lunco_core::RuntimePhase::Stop),
                        );
                        runtime.call_hook(entity, ScenarioHook::Stop, gid)
                    } else {
                        None
                    };
                    runtime.forget(entity);
                    st.gid = gid;
                    st.started = false;
                    st.compiled = false;
                    st.generation = generation;
                    if directive_invalid {
                        st.attempted_generation = Some(generation);
                    }
                    st.parameters_revision = parameters_revision;
                    st.scene_generation = scene_generation;
                    if directive_invalid {
                        if st.directives_diagnostic_generation != Some(generation)
                            || stop_error.is_some()
                        {
                            let mut diagnostics = directives.diagnostics();
                            diagnostics.extend(stop_error);
                            diag_updates.push((raw, Some(diagnostics)));
                            st.directives_diagnostic_generation = Some(generation);
                        }
                    } else if let Some(diagnostic) = stop_error {
                        diag_updates.push((raw, Some(vec![diagnostic])));
                        st.directives_diagnostic_generation = None;
                    } else if st.directives_diagnostic_generation.take().is_some() {
                        diag_updates.push((raw, None));
                    }
                    continue;
                }
                st.directives_diagnostic_generation = None;
                // Events accumulated before this program's first `on_start`
                // belong to the prior lifecycle state. Startup reads current
                // owner state directly; it must not replay a stale event batch.
                let receive_events = st.started && maybe_src.is_none();
                st.gid = gid;
                let mut recompiled = false;
                let mut compile_diag: Option<Diagnostic> = None;
                let mut transition_error: Option<Diagnostic> = None;
                let scene_restart = reload_policy
                    == crate::doc::ScenarioReloadPolicy::Restart
                    && st.started
                    && st.scene_generation != scene_generation;

                // (Re)compile on first sight or generation bump. Phase 1 provides
                // `maybe_src` exactly when this is due (Some ⟺ recompile), so the
                // presence of the source IS the gate — no per-tick source clone.
                if let Some((source, params, asset_id)) = &maybe_src {
                    recompiled = true;
                    st.attempted_generation = Some(generation);
                    st.parameters_revision = parameters_revision;
                    // Hot-reload teardown: the OUTGOING program cleans up first.
                    if scene_restart {
                        // The old scene is already gone. Discard the backend
                        // state directly instead of calling the outgoing
                        // program's cleanup against the replacement scene.
                        runtime.forget(entity);
                        st.compiled = false;
                    } else if st.started && st.compiled {
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context.with_phase(lunco_core::RuntimePhase::Stop),
                        );
                        transition_error = runtime.call_hook(entity, ScenarioHook::Stop, gid);
                    }
                    st.started = false;
                    st.scene_generation = scene_generation;
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        pass_context.with_phase(lunco_core::RuntimePhase::Preparation),
                    );
                    match runtime.compile(entity, source, params, asset_id.as_deref()) {
                        CompileOutcome::Failed(diag) => {
                            let program = world
                                .get::<lunco_core::ScenarioProgramPrim>(entity)
                                .map(|prim| prim.0.as_str())
                                .unwrap_or("<interactive scenario>");
                            error!(
                                "[scenario] {:?} compile failed for program {} on entity {entity:?}: {}",
                                language,
                                program,
                                diag.message,
                            );
                            st.compiled = false;
                            let mut diagnostics = Vec::new();
                            diagnostics.extend(transition_error.take());
                            diagnostics.push(diag);
                            diag_updates.push((raw, Some(diagnostics)));
                            continue;
                        }
                        CompileOutcome::Ready { top_level } => {
                            st.compiled = true;
                            st.generation = generation;
                            compile_diag = top_level;
                        }
                    }
                }

                // A failed source revision has no lifecycle. It remains latched
                // until its generation changes above, rather than being treated as
                // a started-but-empty program on subsequent ticks.
                if !st.compiled {
                    continue;
                }

                // Preserve each runtime failure from teardown and this pass.
                let mut runtime_errors = Vec::new();
                runtime_errors.extend(transition_error.take());
                if !st.started {
                    st.started = true;
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        pass_context.with_phase(lunco_core::RuntimePhase::Start),
                    );
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Start, gid) {
                        runtime_errors.push(d);
                    }
                }
                if receive_events {
                    for ev in &events {
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context
                                .with_phase(lunco_core::RuntimePhase::Event)
                                .with_producer(lunco_core::RuntimeProducerStamp::simulation(
                                    scene_generation,
                                    ev.sim_tick,
                                )),
                        );
                        if let Some(d) = runtime.deliver_event(entity, gid, ev) {
                            runtime_errors.push(d);
                        }
                    }
                }
                if run_tick {
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        pass_context.with_phase(lunco_core::RuntimePhase::Behavior),
                    );
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Tick, gid) {
                        runtime_errors.push(d);
                    }
                }

                // Authoritative commands a client-scoped scenario tried (and was
                // denied) this pass — collected in `bridge_core::cmd_value`. Surface
                // them as ONE per-scenario warning diagnostic, not a per-tick log:
                // the author sees, once, that a presentation-scoped script is
                // reaching for host-owned state. Warning severity → the scenario
                // still reports Ready (it compiled and ran fine).
                let dropped = bridge_core::take_script_rejects();

                // Publish status: any Error diagnostic → Error state; a warning-only
                // set stays Ready. Cleared to OK only when a (re)compile ran clean.
                let mut diags = Vec::new();
                diags.extend(compile_diag);
                diags.extend(runtime_errors);
                if !dropped.is_empty() {
                    diags.push(Diagnostic::warning(
                        format!(
                            "client-scoped scenario dropped authoritative command(s): {} — \
                             the host owns shared sim state. Move these to a host scenario, \
                             or drop the `// @scope client` directive.",
                            dropped.join(", ")
                        ),
                        None,
                        None,
                    ));
                }
                if !diags.is_empty() {
                    diag_updates.push((raw, Some(diags)));
                } else if recompiled {
                    diag_updates.push((raw, None));
                }
            }

            // Teardown: any tracked entity no longer live (despawned / detached)
            // gets a final on_stop, then its state is dropped. The entity (and its
            // ScriptAuthority) is gone, so its `cmd()`s run host-trusted — teardown
            // cleanup only, never ongoing behaviour.
            bridge_core::set_script_authority(None);
            bridge_core::set_script_client_local(false);
            let mut dead: Vec<Entity> = fsm.keys().copied().filter(|e| !live.contains(e)).collect();
            dead.sort_unstable_by(|a, b| {
                fsm.get(a)
                    .map(|state| state.gid)
                    .cmp(&fsm.get(b).map(|state| state.gid))
                    .then_with(|| a.to_bits().cmp(&b.to_bits()))
            });
            for entity in dead {
                if let Some(st) = fsm.remove(&entity) {
                    if st.started && st.compiled {
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context.with_phase(lunco_core::RuntimePhase::Stop),
                        );
                        if let Some(diagnostic) = runtime.call_hook(entity, ScenarioHook::Stop, st.gid) {
                            if let Some(raw) = st.document_id {
                                diag_updates.push((raw, Some(vec![diagnostic])));
                            } else {
                                bevy::log::error!("[scenario] on_stop failed without an owning script document: {}", diagnostic.message);
                            }
                        }
                    }
                    runtime.forget(entity);
                }
            }
        });

        if !diag_updates.is_empty() {
            let mut store = world.resource_mut::<DocumentDiagnostics>();
            for (raw, status) in diag_updates {
                match status {
                    // Severity-derived: an error-carrying set marks Error, a
                    // warning-only set (e.g. a dropped client-scoped command)
                    // stays Ready while still surfacing the notice.
                    Some(diags) => store.set_diagnostics(DocumentId::new(raw), diags),
                    None => store.set_ok(DocumentId::new(raw)),
                }
            }
        }

        // Keep the inbox's backing allocation hot without retaining delivered
        // payloads. This matters for control paths that publish one telemetry
        // event per fixed tick: the queue remains bounded and allocation-free
        // after the first burst.
        events.clear();
        if let Some(mut inbox) = world.get_resource_mut::<ScriptEventInbox>() {
            // Hooks can emit events while this pass is delivering the batch.
            // Those events are already in `inbox.pending` and belong to the
            // next pass; never replace them with the recycled delivered batch.
            // Reuse the old allocation only when no new event was produced.
            if inbox.pending.is_empty() {
                inbox.pending = events;
            }
        }
    }

    /// Live introspection of `entity`'s scenario: the neutral FSM state joined
    /// with the backend's [`ScenarioSnapshot`]. `None` if the driver isn't
    /// tracking this entity (no scenario, or it hasn't been driven yet). Powers
    /// the `ScriptInspect` query — the same data for any language `R`. `builder`
    /// chooses the value format (for example, the API's typed value builder).
    pub fn introspect<B: ValueBuilder>(
        &self,
        entity: Entity,
        builder: &B,
    ) -> Option<ScenarioIntrospection<B::Value>> {
        let fsm = self.fsm.get(&entity)?;
        let (state, hooks) = match self.runtime.snapshot(entity, builder) {
            Some(s) => (s.state, s.hooks),
            None => (builder.unit(), Vec::new()),
        };
        Some(ScenarioIntrospection {
            generation: fsm.generation,
            started: fsm.started,
            compiled: fsm.compiled,
            gid: fsm.gid,
            state,
            hooks,
        })
    }
}

// ── Event inbox (neutral) ───────────────────────────────────────────────────
//
// TODO(multi-agent coordination): the inbox below is untyped *broadcast* pub/sub
// (every scenario sees every TelemetryEvent on the next driver pass). Two follow-ups, only one
// of which is a scripting feature:
//   1. Shared BLACKBOARD (the real coordination primitive): a neutral
//      `Blackboard` resource (`HashMap<String, Value>`) + verbs `bb_set`/`bb_get`/
//      `bb_delete`/`bb_keys`, plus ONE atomic `bb_claim(key, gid) -> bool`
//      (compare-and-set — the only part scripts can't do race-free themselves) for
//      task allocation / resource claiming / formations. Tension to decide:
//      deterministic double-buffering (write N, visible N+1) vs immediate
//      visibility (which `bb_claim` needs). Build only when a scenario actually
//      needs agents to claim/share state — speculative until then.
//   2. ADDRESSED messaging is NOT a new channel: a `send(to_gid, name, value)` is
//      just `emit` with the recipient encoded + a filter in `on_event` on the
//      bus that already exists. Do NOT widen `TelemetryEvent` (the YAMCS sample
//      type) with routing fields. Sugar at best — skip until fan-out cost matters.
// Separately, REALISTIC inter-agent comms (latency / range / line-of-sight /
// relay) is a SIMULATION subsystem, not this substrate — scripts would send/recv
// over it via the command/query API and get real delays/dropouts back.

/// Maximum number of full telemetry events that may wait for one scenario pass.
///
/// The producer clock is not necessarily the fixed simulation clock: UI and
/// interaction events can arrive on `Update` while a causal participant holds
/// the fixed schedule. A bound is therefore required even though the driver
/// normally drains the inbox every pass. Exceeding it latches a visible
/// scene-scoped diagnostic and holds event delivery until the next scene.
pub const SCRIPT_EVENT_INBOX_CAPACITY: usize = 4096;

/// Pass-delayed inbox of `TelemetryEvent`s destined for scenario `on_event`
/// hooks. An observer ([`collect_script_events`]) clones every fired event here;
/// the driver drains it at the start of the next driver pass (the next fixed
/// simulation pass while running, or the next `Update` pass while paused).
/// Every event already carries the `SimTick` at which its producer ran, so the
/// pass boundary is simulation-time based rather than render-time based.
/// Delivery remains deterministic and language-neutral: order never depends on
/// system scheduling. If producers outrun the driver, the inbox refuses new
/// events and records the owning runtime diagnostic rather than growing until
/// the simulation becomes progressively slower or crashing the process.
#[derive(Resource, Debug)]
pub struct ScriptEventInbox {
    /// Events awaiting a causally later fixed tick or the next paused pass.
    pub pending: Vec<TelemetryEvent>,
    /// Whether the fixed-capacity boundary has been crossed.
    pub overflowed: bool,
    /// Number of events refused after the boundary was crossed.
    pub dropped: u64,
    /// Whether delivery is held after an overflow. The scene transition owner
    /// clears this before the replacement scene starts.
    pub faulted: bool,
}

impl Default for ScriptEventInbox {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(SCRIPT_EVENT_INBOX_CAPACITY),
            overflowed: false,
            dropped: 0,
            faulted: false,
        }
    }
}

impl ScriptEventInbox {
    /// Discard scene-owned events and reset this scene's overflow accounting.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.overflowed = false;
        self.dropped = 0;
    }

    /// Enqueue one event. The driver assigns canonical order at the pass boundary.
    ///
    /// `false` is an explicit overflow signal. The caller owns the policy for
    /// surfacing it; no event is silently evicted from the front of the queue.
    pub fn enqueue(&mut self, event: TelemetryEvent) -> bool {
        if self.faulted || self.pending.len() >= SCRIPT_EVENT_INBOX_CAPACITY {
            self.overflowed = true;
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.pending.push(event);
        true
    }

    /// Clear pending events and the latched collection fault at a scene
    /// replacement boundary.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.overflowed = false;
        self.dropped = 0;
        self.faulted = false;
    }
}

fn compare_telemetry_events(a: &TelemetryEvent, b: &TelemetryEvent) -> Ordering {
    a.sim_tick
        .cmp(&b.sim_tick)
        .then_with(|| a.source.cmp(&b.source))
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.severity.cmp(&b.severity))
        .then_with(|| a.sim_secs.total_cmp(&b.sim_secs))
        .then_with(|| a.timestamp.total_cmp(&b.timestamp))
        .then_with(|| compare_telemetry_values(&a.data, &b.data))
}

fn compare_telemetry_values(
    a: &lunco_telemetry_core::TelemetryValue,
    b: &lunco_telemetry_core::TelemetryValue,
) -> Ordering {
    use lunco_telemetry_core::TelemetryValue as Value;

    match (a, b) {
        (Value::F64(a), Value::F64(b)) => a.total_cmp(b),
        (Value::I64(a), Value::I64(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Array(a), Value::Array(b)) => {
            for (a, b) in a.iter().zip(b) {
                let order = compare_telemetry_values(a, b);
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.len().cmp(&b.len())
        }
        (Value::Map(a), Value::Map(b)) => {
            for ((a_key, a_value), (b_key, b_value)) in a.iter().zip(b) {
                let order = a_key
                    .cmp(b_key)
                    .then_with(|| compare_telemetry_values(a_value, b_value));
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.len().cmp(&b.len())
        }
        (Value::F64(_), _) => Ordering::Less,
        (_, Value::F64(_)) => Ordering::Greater,
        (Value::I64(_), _) => Ordering::Less,
        (_, Value::I64(_)) => Ordering::Greater,
        (Value::Bool(_), _) => Ordering::Less,
        (_, Value::Bool(_)) => Ordering::Greater,
        (Value::String(_), _) => Ordering::Less,
        (_, Value::String(_)) => Ordering::Greater,
        (Value::Array(_), _) => Ordering::Less,
        (_, Value::Array(_)) => Ordering::Greater,
    }
}

/// Observer: mirror live-scene `TelemetryEvent`s into the scenario inbox. Reuses
/// the existing telemetry bus — scenarios are just another subscriber.
///
/// Runs on EVERY peer while the scenario lifecycle gate is open. Events are not
/// queued while the gate is closed because no scenario can consume them and the
/// readiness gate also prevents the driver pass that drains the inbox. Scene
/// transitions clear any already-pending events before the outgoing scene is
/// replaced. Client-scoped scenarios (`// @scope client`) therefore see events
/// fired on the client; host-authoritative events reach them only when explicitly
/// replicated.
pub fn collect_script_events(
    trigger: On<TelemetryEvent>,
    gate: Res<ScenarioExecutionGate>,
    mut inbox: ResMut<ScriptEventInbox>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    // Scenario hooks cannot observe events while the scene readiness gate is
    // closed. Do not accumulate producer traffic during that hold: the fixed
    // and paused scenario passes are deliberately disabled until every
    // participant is admitted, so accepting events here would only fill the
    // bounded inbox before startup completes.
    if !gate.enabled {
        return;
    }
    if inbox.enqueue(trigger.event().clone()) {
        return;
    }
    // Surface the terminal signal only once. The first rejected event is enough
    // to identify the fault; additional events are counted without producing a
    // second message or another log line. The affected event stream is held and
    // the owning diagnostics plane exposes a recoverable scene-scoped verdict.
    if inbox.dropped == 1 {
        inbox.pending.clear();
        inbox.faulted = true;
        error!(
            "[scripting] telemetry event inbox overflowed at {} events; event delivery is held until the next scene",
            SCRIPT_EVENT_INBOX_CAPACITY
        );
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "scripting-telemetry",
                [lunco_core::RuntimeDiagnostic {
                    code: "telemetry-event-overflow".into(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "scripting-telemetry".into(),
                    subject: "scenario-event-inbox".into(),
                    message: format!(
                        "{} telemetry events exceeded the fixed inbox capacity; event delivery is held until the next scene",
                        SCRIPT_EVENT_INBOX_CAPACITY
                    ),
                }],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScriptEventInbox, SCRIPT_EVENT_INBOX_CAPACITY};
    use bevy::prelude::App;
    use lunco_telemetry_core::{Severity, TelemetryEvent, TelemetryValue};

    fn event(index: usize) -> TelemetryEvent {
        TelemetryEvent {
            name: format!("event:{index}"),
            source: 0,
            severity: Severity::Info,
            data: TelemetryValue::F64(index as f64),
            timestamp: index as f64,
            sim_secs: 0.0,
            sim_tick: 0,
        }
    }

    #[test]
    fn event_inbox_is_bounded_and_reports_overflow() {
        let mut inbox = ScriptEventInbox::default();
        for index in 0..SCRIPT_EVENT_INBOX_CAPACITY {
            assert!(inbox.enqueue(event(index)));
        }
        assert_eq!(inbox.pending.len(), SCRIPT_EVENT_INBOX_CAPACITY);
        assert!(!inbox.enqueue(event(SCRIPT_EVENT_INBOX_CAPACITY)));
        assert!(inbox.overflowed);
        assert_eq!(inbox.dropped, 1);
        assert_eq!(inbox.pending[0].name, "event:0");
    }

    #[test]
    fn collector_latches_a_diagnostic_without_exiting_the_app() {
        let mut app = App::new();
        app.init_resource::<super::ScenarioExecutionGate>()
            .init_resource::<ScriptEventInbox>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_observer(super::collect_script_events);

        for index in 0..=SCRIPT_EVENT_INBOX_CAPACITY {
            app.world_mut().trigger(event(index));
        }

        let inbox = app.world().resource::<ScriptEventInbox>();
        assert!(inbox.faulted);
        assert!(inbox.pending.is_empty());
        assert_eq!(inbox.dropped, 1);
        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert!(diagnostics
            .findings
            .iter()
            .any(|finding| finding.code == "telemetry-event-overflow"));
    }
}

#[cfg(test)]
mod lifecycle_readiness_tests {
    use super::*;
    use crate::doc::ScriptDocument;
    use std::sync::{Arc, Mutex};

    #[test]
    fn scenario_directives_bind_only_to_supported_peer_and_timing_values() {
        assert_eq!(
            ScenarioDirectives::from_source("fn on_start(me, ctx) {}"),
            ScenarioDirectives::default()
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @scope host\n// @timing simulation\n"),
            ScenarioDirectives::default()
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @scope client\n").scope,
            ScriptScope::Client
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @scope both\n").scope,
            ScriptScope::Both
        );

        let unknown_scope = ScenarioDirectives::from_source("// @scope clinet\n");
        assert_eq!(unknown_scope.scope, ScriptScope::Unsupported);
        assert_eq!(unknown_scope.timing, ScriptTiming::Simulation);
        assert_eq!(unknown_scope.diagnostics().len(), 1);

        let unknown_timing = ScenarioDirectives::from_source("// @timing presentation\n");
        assert_eq!(unknown_timing.scope, ScriptScope::Host);
        assert_eq!(unknown_timing.timing, ScriptTiming::Unsupported);
        assert_eq!(unknown_timing.diagnostics().len(), 1);

        let invalid =
            ScenarioDirectives::from_source("// @scope clinet\n// @timing presentation\n");
        assert_eq!(invalid.diagnostics().len(), 2);
    }

    #[test]
    fn missing_scene_generation_faults_and_skips_scenario_execution() {
        let mut world = World::new();
        world.init_resource::<lunco_core::RuntimeFaults>();

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);

        let fault = world
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .as_ref()
            .expect("missing owner generation is visible as a runtime fault");
        assert_eq!(fault.kind, "scenario-generation-missing");
    }

    #[test]
    fn unknown_scenario_directives_skip_runtime_and_publish_document_errors_once() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        world.spawn((
            ChildOf(owner),
            ScriptedModel {
                document_id: Some(72),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
        ));
        world.spawn((
            ChildOf(owner),
            ScriptedModel {
                document_id: Some(74),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver {
            runtime: RecordingRuntime(calls.clone(), contexts, Arc::new(Mutex::new(false))),
            fsm: HashMap::new(),
        });
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(72),
            ScriptDocument::new(
                72,
                ScriptLanguage::Rhai,
                "// @scope clinet\n// @timing presentation\n",
            ),
        );
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(74),
            ScriptDocument::new(
                74,
                ScriptLanguage::Rhai,
                "// @scope host\n// @timing simulation\n",
            ),
        );
        world.insert_resource(ApiEntityRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(ScenarioSceneGeneration::default());
        world.insert_resource(lunco_core_runtime::SimTick(4));

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Tick
            ]
        );
        let status = world
            .resource::<DocumentDiagnostics>()
            .get(DocumentId::new(72))
            .expect("invalid scope is visible to the document owner");
        assert_eq!(status.diagnostics.len(), 2);
        assert!(status.diagnostics[0]
            .message
            .contains("unknown scenario @scope"));
        assert!(status.diagnostics[1]
            .message
            .contains("unsupported scenario @timing"));

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(calls.lock().unwrap().len(), 4);
        let status = world
            .resource::<DocumentDiagnostics>()
            .get(DocumentId::new(72))
            .unwrap();
        assert_eq!(status.diagnostics.len(), 2);
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RecordedCall {
        Compile,
        Start,
        Tick,
        Stop,
        Event(String),
    }

    #[derive(Clone, Default)]
    struct RecordingRuntime(
        Arc<Mutex<Vec<RecordedCall>>>,
        Arc<Mutex<Vec<lunco_core::RuntimeExecutionContext>>>,
        Arc<Mutex<bool>>,
    );

    impl ScenarioRuntime for RecordingRuntime {
        fn compile(
            &mut self,
            _entity: Entity,
            _source: &str,
            _params: &ScenarioParameters,
            _asset_id: Option<&str>,
        ) -> CompileOutcome {
            self.0.lock().unwrap().push(RecordedCall::Compile);
            self.1
                .lock()
                .unwrap()
                .push(bridge_core::execution_context());
            CompileOutcome::Ready { top_level: None }
        }

        fn call_hook(
            &mut self,
            _entity: Entity,
            hook: ScenarioHook,
            _self_gid: i64,
        ) -> Option<Diagnostic> {
            self.1
                .lock()
                .unwrap()
                .push(bridge_core::execution_context());
            let call = match hook {
                ScenarioHook::Start => RecordedCall::Start,
                ScenarioHook::Tick => RecordedCall::Tick,
                ScenarioHook::Stop => RecordedCall::Stop,
            };
            self.0.lock().unwrap().push(call);
            if matches!(hook, ScenarioHook::Stop) && *self.2.lock().unwrap() {
                Some(Diagnostic::error("on_stop failed", None, None))
            } else {
                None
            }
        }

        fn deliver_event(
            &mut self,
            _entity: Entity,
            _self_gid: i64,
            event: &TelemetryEvent,
        ) -> Option<Diagnostic> {
            self.1
                .lock()
                .unwrap()
                .push(bridge_core::execution_context());
            self.0
                .lock()
                .unwrap()
                .push(RecordedCall::Event(event.name.clone()));
            None
        }

        fn forget(&mut self, _entity: Entity) {}
    }

    #[test]
    fn edited_directives_stop_the_old_program_and_skip_the_new_revision() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        world.spawn((
            ChildOf(owner),
            ScriptedModel {
                document_id: Some(73),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver {
            runtime: RecordingRuntime(
                calls.clone(),
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(true)),
            ),
            fsm: HashMap::new(),
        });
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(73),
            ScriptDocument::new(
                73,
                ScriptLanguage::Rhai,
                "// @scope host\n// @timing simulation\n",
            ),
        );
        world.insert_resource(ApiEntityRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(ScenarioSceneGeneration::default());
        world.insert_resource(lunco_core_runtime::SimTick(1));

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Tick
            ]
        );

        assert!(world
            .resource_mut::<ScriptRegistry>()
            .reload_external_source(
                DocumentId::new(73),
                "// @scope clinet\n// @timing presentation\n",
            ));
        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Tick,
                RecordedCall::Stop,
            ]
        );
        let diagnostics = world
            .resource::<DocumentDiagnostics>()
            .get(DocumentId::new(73))
            .expect("the edited source revision publishes its own diagnostics");
        assert_eq!(diagnostics.diagnostics.len(), 3);
        assert!(diagnostics.diagnostics[2]
            .message
            .contains("on_stop failed"));

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(calls.lock().unwrap().len(), 4);
        assert_eq!(
            world
                .resource::<DocumentDiagnostics>()
                .get(DocumentId::new(73))
                .unwrap()
                .diagnostics
                .len(),
            3
        );
    }

    fn event(name: &str, sim_tick: u64) -> TelemetryEvent {
        TelemetryEvent {
            name: name.into(),
            source: 1,
            severity: lunco_telemetry_core::Severity::Info,
            data: lunco_telemetry_core::TelemetryValue::Bool(true),
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick,
        }
    }

    #[test]
    fn held_owner_starts_after_release_without_replaying_pre_start_events() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        world.spawn((
            ChildOf(owner),
            ScriptedModel {
                document_id: Some(71),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver {
            runtime: RecordingRuntime(calls.clone(), contexts.clone(), Arc::new(Mutex::new(false))),
            fsm: HashMap::new(),
        });
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(71),
            ScriptDocument::new(71, ScriptLanguage::Rhai, ""),
        );
        world.insert_resource(ApiEntityRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(ScenarioSceneGeneration::default());
        world.insert_resource(lunco_core_runtime::SimTick(5));
        world.insert_resource(lunco_readiness::ReadinessState {
            world_hold: false,
            held_entities: vec![owner],
        });
        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("before_release", 4));

        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert!(calls.lock().unwrap().is_empty());

        world
            .resource_mut::<lunco_readiness::ReadinessState>()
            .held_entities
            .clear();
        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Tick
            ]
        );

        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("current_tick", 5));
        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert!(!calls
            .lock()
            .unwrap()
            .contains(&RecordedCall::Event("current_tick".into())));

        world.resource_mut::<lunco_core_runtime::SimTick>().0 = 6;
        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);

        let recorded_calls = calls.lock().unwrap();
        assert!(recorded_calls.contains(&RecordedCall::Event("current_tick".into())));
        assert!(!recorded_calls.contains(&RecordedCall::Event("before_release".into())));
        drop(recorded_calls);

        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("paused_update", 6));
        ScenarioDriver::<RecordingRuntime>::run_without_simulation_tick(
            &mut world,
            ScriptLanguage::Rhai,
        );
        assert!(calls
            .lock()
            .unwrap()
            .contains(&RecordedCall::Event("paused_update".into())));

        let contexts = contexts.lock().unwrap();
        assert!(contexts.iter().any(|context| {
            context
                .route
                .is_some_and(|route| route.cycle == lunco_core::RuntimeCycle::Simulation)
                && context.phase == lunco_core::RuntimePhase::Behavior
                && context.clock == lunco_core::RuntimeClock::Simulation
                && context.sequence == Some(6)
        }));
        assert!(contexts.iter().any(|context| {
            context
                .route
                .is_some_and(|route| route.cycle == lunco_core::RuntimeCycle::Simulation)
                && context.phase == lunco_core::RuntimePhase::Event
                && context.producer == Some(lunco_core::RuntimeProducerStamp::simulation(0, 5))
        }));
        assert!(contexts.iter().any(|context| {
            context
                .route
                .is_some_and(|route| route.cycle == lunco_core::RuntimeCycle::Lifecycle)
                && context.phase == lunco_core::RuntimePhase::Event
                && context.clock == lunco_core::RuntimeClock::None
                && context.sequence.is_none()
                && context.producer == Some(lunco_core::RuntimeProducerStamp::simulation(0, 6))
        }));
    }
}
