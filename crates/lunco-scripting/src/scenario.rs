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
use std::sync::{Mutex, mpsc};

use lunco_command_contracts::SessionId;
use lunco_doc::{Diagnostic, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;
use lunco_telemetry_core::TelemetryEvent;

use crate::ScriptRegistry;
use crate::doc::ScenarioParameters;
use crate::doc::{ScriptLanguage, ScriptedModel};
use lunco_scripting_bridge_core as bridge_core;
use lunco_scripting_bridge_core::{ScenarioAudience, ValueBuilder};

/// Controls whether persistent scenario programs are allowed to execute their
/// lifecycle hooks.
///
/// Scene transitions close this gate until the composed scene and its readiness
/// participants are admitted. Scenario policy can reference entities other
/// than its attached owner, so startup waits for the complete readiness set.
/// Ready programs run `on_start` before the first fixed tick; the driver then
/// idles programs whose own subtree is held.
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

/// Language-neutral causal and data-readiness requirements declared by one
/// prepared scenario. Modelica ids identify live participants that join the
/// fixed-step barrier. Directional read/write ids authorize direct
/// simulation-clock access to live entities; when one is a Modelica
/// participant, it also joins this scenario's barrier contribution. Owner keys
/// identify immutable inputs that must commit before initialization and the
/// first lifecycle hook may run. Broad API query snapshots are named explicitly
/// so the runtime can account for their reads without guessing from results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScenarioDependencyPlan {
    /// Modelica entities whose ports or events the scenario consumes or writes.
    pub modelica_entities: Vec<i64>,
    /// Live entities whose reflected components or ports the scenario reads.
    pub entity_reads: Vec<i64>,
    /// Live entities whose reflected components or ports the scenario writes.
    pub entity_writes: Vec<i64>,
    /// Broad API query providers whose snapshots the scenario reads during the
    /// Simulation clock. Entity-target and committed-scene queries use their
    /// provider-owned scopes instead.
    pub query_reads: Vec<String>,
    /// Owner-published inputs that must be ready before activation.
    pub required_inputs: Vec<lunco_core_runtime::SimulationDependencyKey>,
}

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
    _trigger: On<lunco_core::SceneTransitionCommitted>,
    mut arm: ResMut<ScenarioReadinessArm>,
) {
    arm.0 = true;
}

fn committed_scene_generation(world: &World) -> Option<u64> {
    world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()?
        .completed_generation()
}

fn has_scenario_models(world: &mut World, language: ScriptLanguage) -> bool {
    let mut models = world.query::<&ScriptedModel>();
    models
        .iter(world)
        .any(|model| model.language == Some(language))
}

fn has_scene_owned_scenario_models(world: &mut World, language: ScriptLanguage) -> bool {
    let mut models = world.query::<(
        &ScriptedModel,
        Option<&crate::TwinOwnedScript>,
        Option<&crate::SceneOwnedScript>,
    )>();
    models.iter(world).any(|(model, twin, scene)| {
        model.language == Some(language) && (twin.is_some() || scene.is_some())
    })
}

/// Stable actor identity comes directly from the source-owned component. The
/// API registry is an Update-synchronized lookup index and can lag a newly
/// projected entity at a lifecycle boundary.
fn scenario_actor_order_key(world: &World, entity: Entity) -> (Option<u64>, u64) {
    (
        world
            .get::<lunco_core::GlobalEntityId>(entity)
            .map(|identity| identity.get()),
        entity.to_bits(),
    )
}

/// Rhai's `me` and telemetry ABI reserves zero for a host with no global
/// identity. This sentinel is never used to order actors.
fn scenario_self_id(world: &World, entity: Entity) -> i64 {
    world
        .get::<lunco_core::GlobalEntityId>(entity)
        .map(|identity| identity.get() as i64)
        .unwrap_or(0)
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
    use lunco_core::{SceneTransition, SceneTransitionCommitted, SceneTransitionStarted};
    use lunco_readiness::ReadinessState;

    fn transition_id(transition: SceneTransition) -> lunco_core::SceneTransitionId {
        lunco_core::SceneTransitionCoordinator::default().start(transition)
    }

    #[test]
    fn each_scene_transition_opens_once_after_readiness_clears() {
        let mut app = App::new();
        app.init_resource::<ScenarioExecutionGate>()
            .init_resource::<ScenarioReadinessArm>()
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

        app.world_mut()
            .trigger(SceneTransitionCommitted { id: clear_id });
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

/// Which network peers execute a scenario's lifecycle hooks. Default
/// [`Host`](ScenarioPeerTarget::Host):
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
/// Authored via a `// @peer host|client|both` directive on one of the first
/// lines of the script source, so it rides the same channel for API-attached
/// (`RunScenario`) and USD-embedded scenarios with no wire or schema change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScenarioPeerTarget {
    #[default]
    Host,
    Client,
    Both,
    /// A declared peer target was not recognized; the scenario is disabled.
    Unsupported,
}

impl ScenarioPeerTarget {
    /// Whether this scenario should execute on the current network peer.
    pub fn runs_on_peer(self, is_client: bool) -> bool {
        match self {
            ScenarioPeerTarget::Host => !is_client,
            ScenarioPeerTarget::Client => is_client,
            ScenarioPeerTarget::Both => true,
            ScenarioPeerTarget::Unsupported => false,
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
    /// Network peer or peers on which this scenario may execute.
    pub peer_target: ScenarioPeerTarget,
    /// Runtime cycle requested by this scenario.
    pub timing: ScriptTiming,
    /// Whether an unrecognized scenario metadata directive was authored.
    pub unsupported_directive: bool,
}

impl ScenarioDirectives {
    /// Parse known directives from the source preamble.
    ///
    /// Unknown peer targets, timing values, and metadata directives are
    /// retained as unsupported metadata so the owner can skip this scenario and
    /// publish a document diagnostic.
    pub fn from_source(src: &str) -> Self {
        let mut directives = Self::default();
        for line in src.lines().take(24) {
            let t = line.trim_start();
            let Some(rest) = t.strip_prefix("//") else {
                continue;
            };
            let rest = rest.trim_start().trim_start_matches('!').trim_start();
            let mut fields = rest.split_whitespace();
            let directive = fields.next().unwrap_or_default();
            let value = fields.next().unwrap_or_default();
            let has_extra_value = fields.next().is_some();
            match directive {
                "@peer" => {
                    directives.peer_target = if has_extra_value {
                        ScenarioPeerTarget::Unsupported
                    } else {
                        match value.to_ascii_lowercase().as_str() {
                            "host" => ScenarioPeerTarget::Host,
                            "client" => ScenarioPeerTarget::Client,
                            "both" => ScenarioPeerTarget::Both,
                            _ => ScenarioPeerTarget::Unsupported,
                        }
                    };
                }
                "@timing" => {
                    directives.timing = if !has_extra_value && value == "simulation" {
                        ScriptTiming::Simulation
                    } else {
                        ScriptTiming::Unsupported
                    };
                }
                _ if directive.starts_with('@') => {
                    directives.unsupported_directive = true;
                }
                _ => {}
            }
        }
        directives
    }

    fn is_supported(self) -> bool {
        !self.peer_target.is_unsupported()
            && self.timing != ScriptTiming::Unsupported
            && !self.unsupported_directive
    }

    fn diagnostics(self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.peer_target.is_unsupported() {
            diagnostics.push(Diagnostic::error(
                "unsupported scenario @peer target; expected host, client, or both",
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
        if self.unsupported_directive {
            diagnostics.push(Diagnostic::error(
                "unknown scenario metadata directive; supported directives are @peer and @timing",
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
    /// Compiled successfully. Initialization runs after dependency admission.
    Ready,
    /// The artifact was prepared against an obsolete shared runtime revision.
    Stale,
}

/// Whether source preparation has an immutable cache result or needs shared
/// background admission. A cache hit skips worker dispatch, while the driver
/// keeps the same progress hold through activation for both paths.
pub enum CompilePreparation<P: Send + 'static> {
    /// Immutable artifact already available from the backend's owner cache.
    Ready(P),
    /// Pure worker operation required to produce the immutable artifact.
    Worker(Box<dyn FnOnce() -> Result<P, Diagnostic> + Send + 'static>),
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
    /// (`task` / `mission` / `on_visualization` / `on_start` / `on_tick` / `on_event` / `on_stop`).
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
    /// Immutable output of compiling one source revision. It crosses from a
    /// background worker to the owning scenario boundary and therefore cannot
    /// contain live ECS references or per-entity interpreter state.
    type PreparedCompile: Send + 'static;

    /// Shared admission category for this backend's immutable preparation.
    fn async_work_kind(&self) -> lunco_core_runtime::AsyncWorkKind;

    /// Capture every runtime setting needed by pure compilation and return a
    /// worker job. The job may parse and build immutable artifacts; it must not
    /// run top-level script statements or access the live World.
    fn prepare_compile(
        &self,
        source: String,
        asset_id: Option<String>,
    ) -> CompilePreparation<Self::PreparedCompile>;

    /// Commit a prepared artifact into this backend's owner-thread cache and
    /// seed fresh per-entity state. This must not execute the script's top-level
    /// body; initialization remains an explicit lifecycle phase.
    fn commit_compile(
        &mut self,
        entity: Entity,
        prepared: Self::PreparedCompile,
        params: &ScenarioParameters,
    ) -> CompileOutcome;

    /// Revision of the engine, prelude, or module snapshot used for preparation.
    /// A changed value makes all older compile artifacts stale.
    fn preparation_revision(&self) -> u64 {
        0
    }

    /// Revision of source inputs imported by this entity's last preparation.
    /// A change schedules a new immutable compile even when the root document
    /// is unchanged. Backends without source dependencies keep the default.
    fn source_dependency_revision(&self, _entity: Entity) -> u64 {
        0
    }

    /// Run mutable top-level initialization after the program's dependency plan
    /// has been resolved and committed. The stable script identity is supplied
    /// so the backend can seed deterministic initialization state. This is the
    /// first executable world phase for a newly compiled program. Runtime errors
    /// are non-fatal diagnostics; lifecycle hooks still run, matching the
    /// scenario's authored contract.
    fn initialize(&mut self, _entity: Entity, _self_gid: i64) -> Option<Diagnostic> {
        None
    }

    /// Run presentation-only preparation for a compiled program once its scene,
    /// document, and terrain inputs are ready. This hook runs in the Twin's
    /// visualization cycle and may run while Modelica participants are still
    /// preparing. It must not issue authoritative simulation actions.
    fn call_visualization(&mut self, _entity: Entity, _self_gid: i64) -> Option<Diagnostic> {
        None
    }

    /// Call a lifecycle hook for `entity` — a no-op if the scenario doesn't
    /// define it or has no compiled program. Returns a runtime-error diagnostic
    /// if the hook ran and failed.
    fn call_hook(
        &mut self,
        entity: Entity,
        hook: ScenarioHook,
        self_gid: i64,
    ) -> Option<Diagnostic>;

    /// Resolve the program's cross-entity simulation dependencies after
    /// compilation and before its first lifecycle hook. Returned ids identify
    /// Modelica participants whose state or ports the scenario consumes or
    /// writes. Backends without a dependency hook declare an empty set.
    fn simulation_dependencies(
        &mut self,
        _entity: Entity,
        _self_gid: i64,
    ) -> Result<ScenarioDependencyPlan, Diagnostic> {
        Ok(ScenarioDependencyPlan::default())
    }

    /// Deliver one event to `entity`'s event hook (no-op if undefined).
    fn deliver_event(
        &mut self,
        entity: Entity,
        self_gid: i64,
        event: &TelemetryEvent,
    ) -> Option<Diagnostic>;

    /// Drop all per-entity state for `entity` (after its `on_stop`).
    fn forget(&mut self, entity: Entity);

    /// Drop executable instance state while retaining source dependency inputs
    /// until a replacement compile commits. Backends without split state can
    /// use the full teardown default.
    fn forget_program(&mut self, entity: Entity) {
        self.forget(entity);
    }

    /// Read-only snapshot of `entity`'s running program — its live state object
    /// and the lifecycle hooks it defines — for the `ScriptInspect` query. The
    /// backend builds `state` into the caller's native value type via `builder`
    /// (so JSON only appears at an API seam, never as an internal hop). Default
    /// `None`: the backend exposes nothing inspectable.
    fn snapshot<B: ValueBuilder>(
        &self,
        _entity: Entity,
        _builder: &B,
    ) -> Result<Option<ScenarioSnapshot<B::Value>>, Diagnostic> {
        Ok(None)
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
    /// Dependency revision most recently committed or failed. `None` forces a
    /// new preparation after a stale result.
    attempted_dependency_revision: Option<u64>,
    /// Parameter revision most recently sent to the compiler, including a
    /// failed compile. Parameters belong to the attached instance rather than
    /// the reusable source document.
    parameters_revision: u64,
    /// Whether `on_start` has run for the current program.
    started: bool,
    /// Whether the current program's one-shot visualization hook has run.
    visualization_complete: bool,
    /// Whether the backend currently holds a compiled program for this entity.
    compiled: bool,
    /// Runtime engine/prelude revision used for the current compiled program.
    preparation_revision: Option<u64>,
    /// Whether the dependency plan and executable top-level initialization
    /// completed for the current prepared program.
    initialized: bool,
    /// Resolved scenario requirements retained while owner inputs are pending.
    dependency_plan: Option<PendingScenarioDependencies>,
    /// Last-known host gid — so `on_stop` has a meaningful `self` after despawn.
    /// The derived default `0` is the telemetry bus' explicit global/no-entity
    /// source. A local script host has no GlobalEntityId, so `-1` would leak
    /// `u64::MAX` into emitted events.
    gid: i64,
    /// Scene generation of the latest compiled or activated dependency plan.
    scene_generation: u64,
    /// Stable Twin identity captured from the host's ownership marker. This is
    /// independent of scene generation and survives entity removal until its
    /// final stop hook has run.
    owner_twin: Option<lunco_workspace::TwinId>,
    /// Event sequence at which this program started. Older queued events belong
    /// to the prior lifecycle state and must not be replayed to this program.
    start_event_sequence: u64,
    /// Source revision whose scheduling metadata is cached below.
    directives_generation: Option<u64>,
    /// Parsed peer target and execution timing for that source revision.
    directives: ScenarioDirectives,
    /// Source generation whose unsupported directives were diagnosed.
    directives_diagnostic_generation: Option<u64>,
    /// Compilation currently admitted for this scenario, if any.
    pending_compile: Option<PendingCompile>,
    /// Admission hold that remains until dependency planning, initialization,
    /// and the first start hook have committed for the prepared program.
    initialization_progress_key: Option<lunco_core_runtime::SimulationProgressKey>,
    /// Error from the outgoing program's stop hook during a revision change.
    pending_transition_error: Option<Diagnostic>,
    /// A prepared program has not yet completed its first lifecycle pass.
    newly_compiled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScenarioPass {
    FixedTick,
    PausedLifecycle,
    Startup,
}

struct PendingScenarioDependencies {
    scene_generation: u64,
    modelica_entities: Vec<Entity>,
    entity_reads: Vec<Entity>,
    entity_writes: Vec<Entity>,
    query_reads: Vec<String>,
    required_inputs: Vec<lunco_core_runtime::SimulationDependencyKey>,
    observed_revision: Option<u64>,
}

struct PendingCompile {
    key: lunco_core_runtime::AsyncWorkKey,
    progress_key: lunco_core_runtime::SimulationProgressKey,
    generation: u64,
    parameters_revision: u64,
    scene_generation: u64,
    runtime_revision: u64,
    source_dependency_revision: u64,
    queued: bool,
    result_ready: bool,
    capacity_revision: u64,
}

/// Accepted Rhai scenario attachments waiting for the compilation owner to
/// start their exact preparation operation.
///
/// Command admission records the hold before returning to the fixed loop. The
/// driver consumes the reservation when it snapshots the attached source.
#[derive(Resource, Default)]
pub struct ScenarioPreparationAdmissions {
    next_operation_id: u64,
    pending: HashMap<Entity, ScenarioPreparationReservation>,
}

#[derive(Clone, Copy)]
struct ScenarioPreparationReservation {
    document_id: u64,
    generation: u64,
    parameters_revision: u64,
    scene_generation: u64,
    key: lunco_core_runtime::SimulationProgressKey,
}

impl ScenarioPreparationAdmissions {
    /// Reserve simulation admission for an accepted scenario revision.
    pub fn reserve(
        &mut self,
        entity: Entity,
        document_id: u64,
        generation: u64,
        parameters_revision: u64,
        scene_generation: u64,
        progress: &mut lunco_core_runtime::SimulationProgress,
    ) -> lunco_core_runtime::SimulationProgressKey {
        let key = self.allocate_key();
        progress.acquire(
            key,
            format!("Preparing Rhai scenario document {document_id} generation {generation}"),
        );
        if let Some(previous) = self.pending.insert(
            entity,
            ScenarioPreparationReservation {
                document_id,
                generation,
                parameters_revision,
                scene_generation,
                key,
            },
        ) {
            progress.release(previous.key);
        }
        key
    }

    fn admit_compile(
        &mut self,
        entity: Entity,
        document_id: u64,
        generation: u64,
        parameters_revision: u64,
        scene_generation: u64,
        mut progress: Option<&mut lunco_core_runtime::SimulationProgress>,
    ) -> lunco_core_runtime::SimulationProgressKey {
        if let Some(reservation) = self.pending.remove(&entity) {
            if reservation.document_id == document_id
                && reservation.generation == generation
                && reservation.parameters_revision == parameters_revision
                && reservation.scene_generation == scene_generation
            {
                return reservation.key;
            }
            if let Some(progress) = progress.as_deref_mut() {
                progress.release(reservation.key);
            }
        }

        let key = self.allocate_key();
        if let Some(progress) = progress.as_deref_mut() {
            progress.acquire(
                key,
                format!("Preparing Rhai scenario document {document_id} generation {generation}"),
            );
        }
        key
    }

    fn retain_entities(
        &mut self,
        live: &HashSet<Entity>,
        mut progress: Option<&mut lunco_core_runtime::SimulationProgress>,
    ) {
        self.pending.retain(|entity, reservation| {
            if live.contains(entity) {
                return true;
            }
            if let Some(progress) = progress.as_deref_mut() {
                progress.release(reservation.key);
            }
            false
        });
    }

    fn cancel_entity(
        &mut self,
        entity: Entity,
        progress: Option<&mut lunco_core_runtime::SimulationProgress>,
    ) {
        if let Some(reservation) = self.pending.remove(&entity)
            && let Some(progress) = progress
        {
            progress.release(reservation.key);
        }
    }

    fn cancel_all(&mut self, mut progress: Option<&mut lunco_core_runtime::SimulationProgress>) {
        for (_, reservation) in self.pending.drain() {
            if let Some(progress) = progress.as_deref_mut() {
                progress.release(reservation.key);
            }
        }
    }

    fn allocate_key(&mut self) -> lunco_core_runtime::SimulationProgressKey {
        let operation_id = self.next_operation_id;
        self.next_operation_id = operation_id.wrapping_add(1);
        lunco_core_runtime::SimulationProgressKey {
            owner: lunco_core_runtime::SimulationProgressOwner::ScriptPreparation,
            operation_id,
        }
    }
}

fn cancel_scenario_preparation_admission(world: &mut World, entity: Entity) {
    world.resource_scope(
        |world, mut admissions: Mut<ScenarioPreparationAdmissions>| {
            let mut progress = world.get_resource_mut::<lunco_core_runtime::SimulationProgress>();
            admissions.cancel_entity(entity, progress.as_deref_mut());
        },
    );
}

struct CompileCompletion<P> {
    key: lunco_core_runtime::AsyncWorkKey,
    entity: Entity,
    document_id: u64,
    gid: i64,
    generation: u64,
    parameters_revision: u64,
    scene_generation: u64,
    runtime_revision: u64,
    source_dependency_revision: u64,
    result: Result<P, Diagnostic>,
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
    owner_twin: Option<lunco_workspace::TwinId>,
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
        route: Some(match (owner_twin, scene_generation) {
            (Some(twin), Some(generation)) => {
                lunco_core::RuntimeRoute::twin_owned(cycle, generation, twin.raw())
            }
            (Some(twin), None) => lunco_core::RuntimeRoute::twin_owned(cycle, 0, twin.raw()),
            (None, _) => lunco_core::RuntimeRoute::application(cycle),
        }),
        phase: lunco_core::RuntimePhase::Unclassified,
        clock,
        time_seconds,
        delta_seconds,
        sequence,
        producer: None,
    }
}

fn scenario_visualization_context(
    world: &World,
    entity: Entity,
    scene_generation: u64,
) -> lunco_core::RuntimeExecutionContext {
    let owner_twin = scenario_twin_owner(world, entity);
    let generation = scenario_scope_generation(world, entity, scene_generation);
    let mut context = scenario_execution_context(world, owner_twin, false, Some(generation));
    context.route = Some(match owner_twin {
        Some(twin) => lunco_core::RuntimeRoute::twin_owned(
            lunco_core::RuntimeCycle::Visualization,
            generation,
            twin.raw(),
        ),
        None => lunco_core::RuntimeRoute::application(lunco_core::RuntimeCycle::Visualization),
    });
    context.phase = lunco_core::RuntimePhase::Visualization;
    context.clock = lunco_core::RuntimeClock::Presentation;
    context
}

fn scenario_twin_owner(world: &World, entity: Entity) -> Option<lunco_workspace::TwinId> {
    world
        .get::<crate::TwinOwnedScript>(entity)
        .map(|owner| owner.twin)
}

fn scenario_uses_scene_generation(world: &World, entity: Entity) -> bool {
    world.get::<crate::TwinOwnedScript>(entity).is_some()
        || world.get::<crate::SceneOwnedScript>(entity).is_some()
}

fn scenario_scope_generation(world: &World, entity: Entity, scene_generation: u64) -> u64 {
    if scenario_uses_scene_generation(world, entity) {
        scene_generation
    } else {
        0
    }
}

fn scenario_scene_generation_is_current(world: &World, entity: Entity, expected: u64) -> bool {
    if !scenario_uses_scene_generation(world, entity) {
        return expected == 0;
    }
    committed_scene_generation(world) == Some(expected)
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
    compile_sender: mpsc::Sender<CompileCompletion<R::PreparedCompile>>,
    compile_receiver: Mutex<mpsc::Receiver<CompileCompletion<R::PreparedCompile>>>,
    ready_compiles: Mutex<Vec<CompileCompletion<R::PreparedCompile>>>,
}

fn external_progress_is_held(progress: &lunco_core_runtime::SimulationProgress) -> bool {
    progress.blockers().any(|blocker| {
        blocker.key.owner != lunco_core_runtime::SimulationProgressOwner::ScriptPreparation
    })
}

fn scenario_startup_is_blocked(world: &World) -> bool {
    world
        .get_resource::<lunco_time::SceneTimeState>()
        .is_some_and(|state| !state.is_ready())
        || world
            .get_resource::<lunco_readiness::ReadinessState>()
            .is_some_and(|state| state.world_hold || !state.held_entities.is_empty())
        || world
            .get_resource::<lunco_core_runtime::SimulationProgress>()
            .is_some_and(external_progress_is_held)
}

impl<R: ScenarioRuntime + Default> Default for ScenarioDriver<R> {
    fn default() -> Self {
        Self::with_runtime(R::default())
    }
}

impl<R: ScenarioRuntime> ScenarioDriver<R> {
    /// Construct a scenario driver around an explicit runtime backend.
    pub fn with_runtime(runtime: R) -> Self {
        let (compile_sender, compile_receiver) = mpsc::channel();
        Self {
            runtime,
            fsm: HashMap::new(),
            compile_sender,
            compile_receiver: Mutex::new(compile_receiver),
            ready_compiles: Mutex::new(Vec::new()),
        }
    }

    /// Admit immutable scenario compilation before the time spine and commit
    /// completed artifacts in stable actor order. All active compile holds are
    /// released only by their exact current completion or explicit retirement.
    pub fn prepare_compiles(world: &mut World, language: ScriptLanguage) {
        world.init_resource::<ScenarioPreparationAdmissions>();
        let execution_enabled = world
            .get_resource::<ScenarioExecutionGate>()
            .is_none_or(|gate| gate.enabled);
        if !execution_enabled {
            Self::cancel_scene_owned_compiles(world);
        }
        if !has_scenario_models(world, language) {
            Self::cancel_pending_compiles(world);
            return;
        }
        let committed_generation = committed_scene_generation(world);
        if execution_enabled
            && committed_generation.is_none()
            && has_scene_owned_scenario_models(world, language)
        {
            report_missing_scenario_generation(world);
        }
        let scene_generation_available = committed_generation.is_some();
        let scene_generation = committed_generation.unwrap_or(0);

        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let held_roots = world
            .get_resource::<lunco_readiness::ReadinessState>()
            .map(|state| state.held_entities.clone())
            .unwrap_or_default();
        let mut models = {
            let mut query = world.query::<(Entity, &ScriptedModel, Option<&ScriptAuthority>)>();
            query
                .iter(world)
                .filter(|(entity, model, _)| {
                    model.language == Some(language)
                        && (scene_generation_available
                            || !scenario_uses_scene_generation(world, *entity))
                        && (execution_enabled || !scenario_uses_scene_generation(world, *entity))
                })
                .map(|(entity, model, authority)| {
                    (
                        entity,
                        model.paused,
                        model.document_id,
                        authority.and_then(|authority| authority.0),
                        model.reload_policy,
                        model.parameters_revision,
                    )
                })
                .collect::<Vec<_>>()
        };
        models.sort_unstable_by_key(|model| scenario_actor_order_key(world, model.0));
        let live: HashSet<Entity> = models.iter().map(|model| model.0).collect();
        world.resource_scope(
            |world, mut admissions: Mut<ScenarioPreparationAdmissions>| {
                let mut progress =
                    world.get_resource_mut::<lunco_core_runtime::SimulationProgress>();
                admissions.retain_entities(&live, progress.as_deref_mut());
            },
        );

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            driver.runtime.maintain();
            let runtime_revision = driver.runtime.preparation_revision();
            let work_kind = driver.runtime.async_work_kind();
            let mut completions = {
                let mut ready = driver
                    .ready_compiles
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let mut completions = std::mem::take(&mut *ready);
                completions.extend(
                    driver
                        .compile_receiver
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .try_iter(),
                );
                completions
            };
            for completion in completions.drain(..) {
                let pending_matches = driver
                    .fsm
                    .get(&completion.entity)
                    .and_then(|state| state.pending_compile.as_ref())
                    .is_some_and(|pending| pending.key == completion.key);
                if !pending_matches {
                    continue;
                }
                let input_is_current = world
                    .get_resource::<ScriptRegistry>()
                    .and_then(|registry| {
                        registry
                            .documents
                            .get(&DocumentId::new(completion.document_id))
                    })
                    .is_some_and(|host| host.document().generation == completion.generation)
                    && world
                        .get::<ScriptedModel>(completion.entity)
                        .is_some_and(|model| {
                            model.parameters_revision == completion.parameters_revision
                        })
                    && scenario_scene_generation_is_current(
                        world,
                        completion.entity,
                        completion.scene_generation,
                    )
                    && runtime_revision == completion.runtime_revision;
                let input_is_current = input_is_current
                    && driver.runtime.source_dependency_revision(completion.entity)
                        == completion.source_dependency_revision;
                if !input_is_current {
                    retire_pending_compile(world, &mut driver, completion.entity, true);
                    continue;
                }
                if let Some(pending) = driver
                    .fsm
                    .get_mut(&completion.entity)
                    .and_then(|state| state.pending_compile.as_mut())
                {
                    pending.result_ready = true;
                }
                driver
                    .ready_compiles
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(completion);
            }

            let capacity_revision = world
                .get_resource::<lunco_core_runtime::AsyncWorkAdmission>()
                .map(lunco_core_runtime::AsyncWorkAdmission::capacity_revision)
                .unwrap_or(0);
            for (entity, paused, raw, authority, reload_policy, parameters_revision) in &models {
                let script_scene_generation =
                    scenario_scope_generation(world, *entity, scene_generation);
                let Some(raw) = *raw else {
                    cancel_scenario_preparation_admission(world, *entity);
                    retire_pending_compile(world, &mut driver, *entity, true);
                    continue;
                };
                let Some(document) = world
                    .get_resource::<ScriptRegistry>()
                    .and_then(|registry| registry.documents.get(&DocumentId::new(raw)))
                    .map(|host| host.document().clone())
                else {
                    // Closing or detaching a document is terminal for this
                    // preparation. Release its exact hold even if queued work
                    // never reaches a worker or activation boundary.
                    cancel_scenario_preparation_admission(world, *entity);
                    retire_pending_compile(world, &mut driver, *entity, true);
                    continue;
                };
                if document.language != language {
                    cancel_scenario_preparation_admission(world, *entity);
                    retire_pending_compile(world, &mut driver, *entity, true);
                    continue;
                }
                let generation = document.generation;
                let source_dependency_revision = driver.runtime.source_dependency_revision(*entity);
                let prior = driver.fsm.get(entity);
                let directives = prior
                    .filter(|state| state.directives_generation == Some(generation))
                    .map(|state| state.directives)
                    .unwrap_or_else(|| ScenarioDirectives::from_source(&document.source));
                let eligible = !paused
                    && directives.is_supported()
                    && directives.peer_target.runs_on_peer(is_client)
                    && !scenario_owner_is_held(world, *entity, &held_roots);
                if !eligible {
                    cancel_scenario_preparation_admission(world, *entity);
                    let remove_dependencies = prior
                        .filter(|state| {
                            state.pending_compile.is_some()
                                || state.initialization_progress_key.is_some()
                        })
                        .map(|state| !state.initialized);
                    if let Some(remove_dependencies) = remove_dependencies {
                        retire_pending_compile(world, &mut driver, *entity, remove_dependencies);
                    }
                    continue;
                }

                let scene_restart = *reload_policy == crate::doc::ScenarioReloadPolicy::Restart
                    && prior.is_some_and(|state| {
                        state.started && state.scene_generation != script_scene_generation
                    });
                let needs_recompile = prior.is_none_or(|state| {
                    state.attempted_generation != Some(generation)
                        || state.parameters_revision != *parameters_revision
                        || state.preparation_revision != Some(runtime_revision)
                        || state.attempted_dependency_revision != Some(source_dependency_revision)
                        || scene_restart
                });
                let pending_same = prior.is_some_and(|state| {
                    state.pending_compile.as_ref().is_some_and(|pending| {
                        pending.generation == generation
                            && pending.parameters_revision == *parameters_revision
                            && pending.scene_generation == script_scene_generation
                            && pending.runtime_revision == runtime_revision
                            && pending.source_dependency_revision == source_dependency_revision
                    })
                });
                let retry_queued = prior.is_some_and(|state| {
                    state.pending_compile.as_ref().is_some_and(|pending| {
                        !pending.queued
                            && !pending.result_ready
                            && pending.capacity_revision != capacity_revision
                    })
                });
                if pending_same && !retry_queued || !needs_recompile && !retry_queued {
                    continue;
                }

                let gid = scenario_self_id(world, *entity);
                retire_pending_compile(world, &mut driver, *entity, false);

                let context = scenario_execution_context(
                    world,
                    scenario_twin_owner(world, *entity),
                    false,
                    Some(script_scene_generation),
                );
                let mut stop_error = None;
                let (started, compiled) = driver
                    .fsm
                    .get(entity)
                    .map(|state| (state.started, state.compiled))
                    .unwrap_or_default();
                bridge_core::set_script_client_local(is_client);
                bridge_core::set_script_authority(*authority);
                if scene_restart {
                    driver.runtime.forget_program(*entity);
                } else if started && compiled {
                    let _scope = bridge_core::WorldScope::enter(world, context);
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        context.with_phase(lunco_core::RuntimePhase::Stop),
                    );
                    let _script_entity = bridge_core::ScriptEntityScope::enter(gid as u64);
                    stop_error = driver.runtime.call_hook(*entity, ScenarioHook::Stop, gid);
                }
                driver.runtime.forget_program(*entity);

                let state = driver.fsm.entry(*entity).or_default();
                state.started = false;
                state.visualization_complete = false;
                state.compiled = false;
                state.initialized = false;
                state.dependency_plan = None;
                state.gid = gid;
                state.document_id = Some(raw);
                state.directives_generation = Some(generation);
                state.directives = directives;
                state.directives_diagnostic_generation = None;
                state.attempted_generation = Some(generation);
                state.parameters_revision = *parameters_revision;
                state.preparation_revision = Some(runtime_revision);
                state.scene_generation = script_scene_generation;
                state.pending_transition_error = stop_error;
                state.newly_compiled = false;

                if let Some(mut participants) =
                    world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
                {
                    participants.mark_scenario_plan_pending(*entity);
                }
                let progress_key = world.resource_scope(
                    |world, mut admissions: Mut<ScenarioPreparationAdmissions>| {
                        let mut progress =
                            world.get_resource_mut::<lunco_core_runtime::SimulationProgress>();
                        admissions.admit_compile(
                            *entity,
                            raw,
                            generation,
                            *parameters_revision,
                            script_scene_generation,
                            progress.as_deref_mut(),
                        )
                    },
                );
                let operation_id = progress_key.operation_id;
                let key = lunco_core_runtime::AsyncWorkKey::new(
                    work_kind,
                    script_scene_generation,
                    ((raw as u128) << 64) | u128::from(entity.to_bits()),
                    generation,
                    operation_id,
                );
                let preparation = driver
                    .runtime
                    .prepare_compile(document.source, document.asset_id);
                driver.fsm.get_mut(entity).unwrap().pending_compile = Some(PendingCompile {
                    key,
                    progress_key,
                    generation,
                    parameters_revision: *parameters_revision,
                    scene_generation: script_scene_generation,
                    runtime_revision,
                    source_dependency_revision,
                    queued: false,
                    result_ready: false,
                    capacity_revision,
                });
                let Some(progress) =
                    world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
                else {
                    driver
                        .fsm
                        .get_mut(entity)
                        .and_then(|state| state.pending_compile.as_mut())
                        .expect("compile preparation owns a pending result")
                        .result_ready = true;
                    driver
                        .ready_compiles
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(CompileCompletion {
                            key,
                            entity: *entity,
                            document_id: raw,
                            gid,
                            generation,
                            parameters_revision: *parameters_revision,
                            scene_generation: script_scene_generation,
                            runtime_revision,
                            source_dependency_revision,
                            result: Err(Diagnostic::error(
                                "scenario compilation requires SimulationProgress",
                                None,
                                None,
                            )),
                        });
                    continue;
                };
                drop(progress);
                match preparation {
                    CompilePreparation::Ready(prepared) => {
                        driver
                            .fsm
                            .get_mut(entity)
                            .expect("scenario FSM was installed above")
                            .pending_compile
                            .as_mut()
                            .expect("compile preparation owns a pending result")
                            .result_ready = true;
                        driver
                            .ready_compiles
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .push(CompileCompletion {
                                key,
                                entity: *entity,
                                document_id: raw,
                                gid,
                                generation,
                                parameters_revision: *parameters_revision,
                                scene_generation: script_scene_generation,
                                runtime_revision,
                                source_dependency_revision,
                                result: Ok(prepared),
                            });
                    }
                    CompilePreparation::Worker(job) => {
                        submit_scenario_compile(
                            world,
                            &mut driver,
                            *entity,
                            raw,
                            gid,
                            generation,
                            *parameters_revision,
                            script_scene_generation,
                            runtime_revision,
                            source_dependency_revision,
                            job,
                        );
                    }
                }
            }

            let dead: Vec<Entity> = driver
                .fsm
                .keys()
                .copied()
                .filter(|entity| !live.contains(entity))
                .collect();
            for entity in dead {
                retire_pending_compile(world, &mut driver, entity, true);
            }

            // Worker completion order and frame timing are not commit order.
            // Keep every prepared result behind the scene-wide compile barrier,
            // then adopt the complete set by stable owner identity in one
            // lifecycle boundary before the fixed runner admits another tick.
            let has_unready_compile = driver.fsm.values().any(|state| {
                state
                    .pending_compile
                    .as_ref()
                    .is_some_and(|pending| !pending.result_ready)
            });
            if !has_unready_compile {
                let mut ready = std::mem::take(
                    &mut *driver
                        .ready_compiles
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()),
                );
                for completion in &mut ready {
                    completion.gid = scenario_self_id(world, completion.entity);
                }
                ready.sort_unstable_by(|left, right| {
                    scenario_actor_order_key(world, left.entity)
                        .cmp(&scenario_actor_order_key(world, right.entity))
                        .then_with(|| left.key.cmp(&right.key))
                });
                for completion in ready {
                    let pending_matches = driver
                        .fsm
                        .get(&completion.entity)
                        .and_then(|state| state.pending_compile.as_ref())
                        .is_some_and(|pending| pending.key == completion.key);
                    if !pending_matches {
                        continue;
                    }
                    let input_is_current = world
                        .get_resource::<ScriptRegistry>()
                        .and_then(|registry| {
                            registry
                                .documents
                                .get(&DocumentId::new(completion.document_id))
                        })
                        .is_some_and(|host| host.document().generation == completion.generation)
                        && world
                            .get::<ScriptedModel>(completion.entity)
                            .is_some_and(|model| {
                                model.parameters_revision == completion.parameters_revision
                            })
                        && scenario_scene_generation_is_current(
                            world,
                            completion.entity,
                            completion.scene_generation,
                        )
                        && driver.runtime.preparation_revision() == completion.runtime_revision;
                    let input_is_current = input_is_current
                        && driver.runtime.source_dependency_revision(completion.entity)
                            == completion.source_dependency_revision;
                    if !input_is_current {
                        retire_pending_compile(world, &mut driver, completion.entity, true);
                        continue;
                    }
                    let params = world
                        .get::<ScriptedModel>(completion.entity)
                        .map(|model| model.parameters.clone())
                        .unwrap_or_default();
                    let outcome = match completion.result {
                        Ok(prepared) => {
                            let context = scenario_execution_context(
                                world,
                                scenario_twin_owner(world, completion.entity),
                                false,
                                Some(completion.scene_generation),
                            );
                            let _scope = bridge_core::WorldScope::enter(world, context);
                            let _phase = bridge_core::ExecutionContextScope::enter(
                                context.with_phase(lunco_core::RuntimePhase::Preparation),
                            );
                            driver
                                .runtime
                                .commit_compile(completion.entity, prepared, &params)
                        }
                        Err(diagnostic) => CompileOutcome::Failed(diagnostic),
                    };
                    finish_compile_completion(
                        world,
                        &mut driver,
                        completion.key,
                        completion.entity,
                        completion.document_id,
                        completion.gid,
                        completion.generation,
                        completion.parameters_revision,
                        completion.scene_generation,
                        completion.runtime_revision,
                        completion.source_dependency_revision,
                        outcome,
                    );
                }
            }
        });
    }

    /// Retire queued scenario work when the owning runtime or scene is not
    /// available to accept a result. In-flight jobs are allowed to finish; their
    /// completions are ignored because the exact pending key has been removed.
    pub fn cancel_pending_compiles(world: &mut World) {
        world.init_resource::<ScenarioPreparationAdmissions>();
        world.resource_scope(
            |world, mut admissions: Mut<ScenarioPreparationAdmissions>| {
                let mut progress =
                    world.get_resource_mut::<lunco_core_runtime::SimulationProgress>();
                admissions.cancel_all(progress.as_deref_mut());
            },
        );
        if !world.contains_resource::<Self>() {
            return;
        }
        world.resource_scope(|world, mut driver: Mut<Self>| {
            let entities: Vec<Entity> = driver
                .fsm
                .iter()
                .filter_map(|(entity, state)| {
                    (state.pending_compile.is_some() || state.initialization_progress_key.is_some())
                        .then_some(*entity)
                })
                .collect();
            for entity in entities {
                let remove_dependencies = driver
                    .fsm
                    .get(&entity)
                    .is_some_and(|state| !state.initialized);
                retire_pending_compile(world, &mut driver, entity, remove_dependencies);
            }
        });
    }

    /// Retire only scene/Twin preparation when the scene execution gate closes.
    /// Application-owned programs can keep preparing across a Twin transition.
    pub fn cancel_scene_owned_compiles(world: &mut World) {
        let entities: Vec<_> = {
            let mut query = world.query::<(
                Entity,
                Option<&crate::TwinOwnedScript>,
                Option<&crate::SceneOwnedScript>,
            )>();
            query
                .iter(world)
                .filter(|(_, twin_owner, scene_owner)| {
                    twin_owner.is_some() || scene_owner.is_some()
                })
                .map(|(entity, _, _)| entity)
                .collect::<Vec<_>>()
        };
        for entity in &entities {
            cancel_scenario_preparation_admission(world, *entity);
        }
        if !world.contains_resource::<Self>() {
            return;
        }
        world.resource_scope(|world, mut driver: Mut<Self>| {
            for entity in entities {
                let remove_dependencies = !driver
                    .fsm
                    .get(&entity)
                    .is_some_and(|state| state.initialized);
                retire_pending_compile(world, &mut driver, entity, remove_dependencies);
            }
        });
    }

    /// Invalidate every attached program after a shared runtime contract, such
    /// as the authored Rhai prelude, has changed. The scene entities remain
    /// attached; their programs are rebuilt on the next enabled pass.
    #[cfg(feature = "rhai")]
    pub fn invalidate(
        &mut self,
        admission: &mut lunco_core_runtime::AsyncWorkAdmission,
        progress: &mut lunco_core_runtime::SimulationProgress,
    ) -> Vec<Entity> {
        let entities = self.fsm.keys().copied().collect();
        for state in self.fsm.values_mut() {
            if let Some(pending) = state.pending_compile.take() {
                if pending.queued {
                    admission.cancel_queued(pending.key);
                }
                progress.release(pending.progress_key);
            }
            if let Some(key) = state.initialization_progress_key.take() {
                progress.release(key);
            }
        }
        self.ready_compiles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.fsm.clear();
        self.runtime.invalidate();
        entities
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
        Self::stop_entity_with_twin_owner(world, entity, None);
    }

    /// Stop one scenario admitted by `twin` while preserving the outgoing
    /// identity in its `on_stop` execution route. The Twin may already have
    /// left `WorkspaceResource` when `TwinClosed` is delivered.
    pub fn stop_entity_for_twin(world: &mut World, entity: Entity, twin: lunco_workspace::TwinId) {
        Self::stop_entity_with_twin_owner(world, entity, Some(twin));
    }

    fn stop_entity_with_twin_owner(
        world: &mut World,
        entity: Entity,
        twin: Option<lunco_workspace::TwinId>,
    ) {
        let owner_twin = twin.or_else(|| {
            world
                .get::<crate::TwinOwnedScript>(entity)
                .map(|owner| owner.twin)
        });
        let Some(driver) = world.get_resource::<ScenarioDriver<R>>() else {
            return;
        };
        if !driver.fsm.contains_key(&entity) {
            return;
        }

        let mut stop_error = None;
        let mut document_id = None;
        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            retire_pending_compile(world, &mut driver, entity, false);
            let Some(state) = driver.fsm.remove(&entity) else {
                return;
            };
            document_id = state.document_id;
            let generation = committed_scene_generation(world);
            let owner_twin = owner_twin.or(state.owner_twin);
            let mut context = scenario_execution_context(world, owner_twin, false, generation)
                .with_phase(lunco_core::RuntimePhase::Stop);
            if let Some(twin) = owner_twin {
                let scene_generation = context.route.map_or(0, |route| route.generation);
                context.route = Some(lunco_core::RuntimeRoute::twin_owned(
                    lunco_core::RuntimeCycle::Lifecycle,
                    scene_generation,
                    twin.raw(),
                ));
            }
            let _scope = bridge_core::WorldScope::enter(world, context);
            // The entity is still present, but the scene/tutor owns the
            // transition. Its final cleanup is host-authoritative, matching
            // the normal despawn teardown path below.
            bridge_core::set_script_authority(None);
            bridge_core::set_script_client_local(false);
            if state.started && state.compiled {
                let _phase = bridge_core::ExecutionContextScope::enter(context);
                let _script_entity = bridge_core::ScriptEntityScope::enter(state.gid as u64);
                stop_error = driver
                    .runtime
                    .call_hook(entity, ScenarioHook::Stop, state.gid);
            }
            driver.runtime.forget(entity);
        });
        if let Some(mut participants) =
            world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
        {
            participants.remove_scenario_dependencies(entity);
        }
        if let Some(diagnostic) = stop_error {
            publish_scenario_stop_error(world, document_id, diagnostic);
        }
    }

    /// Stop and detach persistent scenarios owned by one Twin. Returns their
    /// entities so backend adapters can release their own source handles after
    /// the generic lifecycle and document ownership have been retired.
    pub fn stop_twin_owned_scripts(
        world: &mut World,
        twin: lunco_workspace::TwinId,
    ) -> Vec<Entity> {
        let targets: Vec<_> = {
            let mut query =
                world.query::<(Entity, &crate::TwinOwnedScript, Option<&ScriptedModel>)>();
            query
                .iter(world)
                .filter(|(_, owner, _)| owner.twin == twin)
                .map(|(entity, owner, model)| {
                    (
                        entity,
                        owner.close_document,
                        model.and_then(|model| model.document_id),
                    )
                })
                .collect()
        };
        let mut stopped = Vec::with_capacity(targets.len());
        for (entity, close_document, document_id) in targets {
            Self::stop_entity_for_twin(world, entity, twin);
            if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
                entity_mut
                    .remove::<ScriptedModel>()
                    .remove::<ScriptAuthority>()
                    .remove::<crate::TwinOwnedScript>()
                    .remove::<crate::SceneOwnedScript>();
            }
            if close_document
                && let Some(document_id) = document_id
                && let Some(mut registry) = world.get_resource_mut::<ScriptRegistry>()
            {
                registry.documents.remove(&DocumentId::new(document_id));
            }
            stopped.push(entity);
        }
        stopped
    }

    /// Exclusive-system body: drive every non-paused `ScriptedModel { language }`
    /// through its lifecycle against the live World, including its fixed-step
    /// behavior.
    pub fn run(world: &mut World, language: ScriptLanguage) {
        Self::run_with_pass(world, language, ScenarioPass::FixedTick);
    }

    /// Exclusive-system body for a paused simulation pass.
    ///
    /// Queued discrete events remain responsive while `Time<Virtual>` stops the
    /// fixed simulation schedule. Startup is admitted by [`Self::run_startup`];
    /// continuous hooks, native tasks, and missions are left to [`Self::run`].
    pub fn run_without_simulation_tick(world: &mut World, language: ScriptLanguage) {
        Self::run_with_pass(world, language, ScenarioPass::PausedLifecycle);
    }

    /// Start ready scenarios before the next fixed tick and admit any scenarios
    /// attached by their `on_start` hooks before returning to the time spine.
    ///
    /// Compilation, dependency planning, initialization, and `on_start` share
    /// the current simulation sequence. The startup pass does not deliver
    /// events or run fixed behavior; its preparation hold remains active until
    /// each ready scenario has completed `on_start`.
    pub fn run_startup(world: &mut World, language: ScriptLanguage) {
        Self::run_with_pass(world, language, ScenarioPass::Startup);
        // Startup hooks can synchronously attach more scenarios through typed
        // commands. Commit those attachments and let the same compiler owner
        // acquire their preparation holds before the fixed runner admits time.
        world.flush();
        Self::prepare_compiles(world, language);
    }

    /// Run each compiled program's one-shot presentation preparation in the
    /// Twin visualization cycle. Scene/reference/document/terrain/Twin-policy
    /// preparation remains a prerequisite; Modelica preparation is independent
    /// and does not hold this visual path.
    pub fn run_visualization(world: &mut World, language: ScriptLanguage) {
        if !world
            .get_resource::<ScenarioExecutionGate>()
            .is_none_or(|gate| gate.enabled)
        {
            return;
        }
        let committed_generation = committed_scene_generation(world);
        if committed_generation.is_none() && has_scene_owned_scenario_models(world, language) {
            report_missing_scenario_generation(world);
        }
        let scene_generation_available = committed_generation.is_some();
        let scene_generation = committed_generation.unwrap_or(0);
        let visualization_inputs_ready = world
            .get_resource::<lunco_core_runtime::SimulationProgress>()
            .is_none_or(|progress| {
                !progress.blockers().any(|blocker| {
                    matches!(
                        blocker.key.owner,
                        lunco_core_runtime::SimulationProgressOwner::SceneLifecycle
                            | lunco_core_runtime::SimulationProgressOwner::SceneReferences
                            | lunco_core_runtime::SimulationProgressOwner::UsdNativeAssetPreparation
                            | lunco_core_runtime::SimulationProgressOwner::UsdSimulationTopology
                            | lunco_core_runtime::SimulationProgressOwner::UsdDomainProjection
                            | lunco_core_runtime::SimulationProgressOwner::UsdDomainDiscovery
                            | lunco_core_runtime::SimulationProgressOwner::TerrainPreparation
                            | lunco_core_runtime::SimulationProgressOwner::DocumentPreparation
                            | lunco_core_runtime::SimulationProgressOwner::TwinPolicyPreparation
                    )
                })
            });
        if !visualization_inputs_ready {
            return;
        }

        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let held_roots = world
            .get_resource::<lunco_readiness::ReadinessState>()
            .map(|state| state.held_entities.clone())
            .unwrap_or_default();
        let mut models = {
            let model_facts = {
                let mut query = world.query::<(Entity, &ScriptedModel, Option<&ScriptAuthority>)>();
                query
                    .iter(world)
                    .filter(|(entity, model, _)| {
                        model.language == Some(language)
                            && (scene_generation_available
                                || !scenario_uses_scene_generation(world, *entity))
                    })
                    .map(|(entity, model, authority)| {
                        (
                            entity,
                            model.document_id,
                            model.parameters_revision,
                            authority.and_then(|authority| authority.0),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let registry = world.get_resource::<ScriptRegistry>();
            model_facts
                .into_iter()
                .map(|(entity, document_id, parameters_revision, authority)| {
                    let document_generation = document_id.and_then(|raw| {
                        registry
                            .and_then(|registry| registry.documents.get(&DocumentId::new(raw)))
                            .map(|host| host.document().generation)
                    });
                    (
                        entity,
                        document_id,
                        document_generation,
                        parameters_revision,
                        authority,
                    )
                })
                .collect::<Vec<_>>()
        };
        models.sort_unstable_by_key(|model| scenario_actor_order_key(world, model.0));

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let runtime_revision = driver.runtime.preparation_revision();
            for (entity, document_id, document_generation, parameters_revision, authority) in
                &models
            {
                let Some(raw) = *document_id else {
                    continue;
                };
                let Some(document_generation) = *document_generation else {
                    continue;
                };
                let source_dependency_revision = driver.runtime.source_dependency_revision(*entity);
                let script_scene_generation =
                    scenario_scope_generation(world, *entity, scene_generation);
                let context = scenario_visualization_context(world, *entity, scene_generation);
                let Some(state) = driver.fsm.get_mut(entity) else {
                    continue;
                };
                let directives = state.directives;
                if !state.compiled
                    || state.visualization_complete
                    || state.document_id != Some(raw)
                    || state.generation != document_generation
                    || state.scene_generation != script_scene_generation
                    || state.preparation_revision != Some(runtime_revision)
                    || state.attempted_dependency_revision != Some(source_dependency_revision)
                    || state.parameters_revision != *parameters_revision
                    || !directives.is_supported()
                    || !directives.peer_target.runs_on_peer(is_client)
                    || scenario_owner_is_held(world, *entity, &held_roots)
                {
                    continue;
                }

                state.visualization_complete = true;
                let gid = state.gid;
                bridge_core::set_script_client_local(is_client);
                bridge_core::set_script_authority(*authority);
                let _scope = bridge_core::WorldScope::enter(world, context);
                let _phase = bridge_core::ExecutionContextScope::enter(context);
                let _visualization_span = bevy::log::info_span!(
                    "scenario_run_visualization_hook",
                    document_id = raw,
                    document_generation,
                    entity = ?entity,
                )
                .entered();
                let _script_entity = bridge_core::ScriptEntityScope::enter(gid as u64);
                if let Some(diagnostic) = driver.runtime.call_visualization(*entity, gid) {
                    if let Some(mut diagnostics) = world.get_resource_mut::<DocumentDiagnostics>() {
                        let document = DocumentId::new(raw);
                        let mut current = diagnostics.diagnostics(document).to_vec();
                        current.push(diagnostic);
                        diagnostics.set_error(document, current);
                    }
                }
            }
        });
    }

    fn run_with_pass(world: &mut World, language: ScriptLanguage, pass: ScenarioPass) {
        if !world
            .get_resource::<ScenarioExecutionGate>()
            .is_none_or(|gate| gate.enabled)
        {
            return;
        }
        let has_driver_state = world
            .get_resource::<Self>()
            .is_some_and(|driver| !driver.fsm.is_empty());
        if !has_driver_state && !has_scenario_models(world, language) {
            return;
        }
        let committed_generation = committed_scene_generation(world);
        if committed_generation.is_none() && has_scene_owned_scenario_models(world, language) {
            report_missing_scenario_generation(world);
        }
        let scene_generation_available = committed_generation.is_some();
        let scene_generation = committed_generation.unwrap_or(0);
        let run_tick = pass == ScenarioPass::FixedTick;
        let startup_pass = pass == ScenarioPass::Startup;
        let pass_start_event_sequence = world
            .get_resource::<ScriptEventInbox>()
            .map(|inbox| inbox.next_sequence)
            .unwrap_or_default();
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
        // A predicting client only ticks scenarios targeted to run there
        // (`Client`/`Both`); the host ticks `Host`/`Both`. Read once — constant
        // for the whole pass.
        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let preparation_revision = world
            .get_resource::<ScenarioDriver<R>>()
            .map(|driver| driver.runtime.preparation_revision())
            .unwrap_or_default();
        let current_sim_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0);
        let held_roots = world
            .get_resource::<lunco_readiness::ReadinessState>()
            .map(|state| state.held_entities.clone())
            .unwrap_or_default();
        // A script's own preparation hold must not deadlock its activation.
        // Other owners' progress holds do gate first initialization/start so a
        // lifecycle hook cannot observe a partially prepared authoritative
        // source set.
        let external_progress_blocked = scenario_startup_is_blocked(world);
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
                .filter(|(entity, _, l, _, _, _, _)| {
                    *l == Some(language)
                        && (scene_generation_available
                            || !scenario_uses_scene_generation(world, *entity))
                })
                .map(|(e, ..)| *e)
                .collect();

            for (entity, paused, lang, doc_id, authority, reload_policy, parameters_revision) in
                models
            {
                if lang != Some(language) {
                    continue;
                }
                if !scene_generation_available && scenario_uses_scene_generation(world, entity) {
                    continue;
                }
                let Some(raw) = doc_id else { continue };
                let script_scene_generation =
                    scenario_scope_generation(world, entity, scene_generation);
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
                    if startup_pass && state.is_some_and(|state| state.started) {
                        continue;
                    }
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
                        && directives.peer_target.runs_on_peer(is_client)
                        && !scenario_owner_is_held(world, entity, &held_roots);
                    let needs_recompile = can_compile
                        && state.is_none_or(|state| {
                            state.attempted_generation != Some(generation)
                                || state.parameters_revision != parameters_revision
                                || state.preparation_revision != Some(preparation_revision)
                                || (reload_policy == crate::doc::ScenarioReloadPolicy::Restart
                                    && state.started
                                    && state.scene_generation != script_scene_generation)
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
                    if !directives.peer_target.runs_on_peer(is_client) {
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
                    if directives.peer_target.runs_on_peer(is_client)
                        && scenario_owner_is_held(world, entity, &held_roots)
                    {
                        continue;
                    }
                }

                let gid = scenario_self_id(world, entity);
                work.push((
                    entity,
                    raw,
                    gid,
                    generation,
                    parameters_revision,
                    maybe_src,
                    authority,
                    reload_policy,
                    script_scene_generation,
                    directives,
                ));
            }
        }

        // Query/archetype order is not a simulation contract. `cmd()` is
        // synchronous inside each hook, so actor order is observable later in
        // the same pass. Use the source-owned identity component directly;
        // local-only hosts are explicitly scoped to this World and use their
        // ECS key only within that scope.
        work.sort_unstable_by_key(|actor| scenario_actor_order_key(world, actor.0));

        // A fixed-step event is eligible after SimTickSet only when its recorded
        // tick precedes the current tick. That boundary determines when the
        // event becomes visible; events stamped at the current tick remain
        // queued until a later tick. The paused Update pass drains all
        // events to keep discrete lifecycle events responsive while simulation
        // is stopped. Drain before the no-work return so traffic cannot accrue.
        let mut events: Vec<QueuedScenarioEvent> = if startup_pass {
            Vec::new()
        } else {
            world
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
                            inbox.pending.sort_by(compare_queued_events);
                        }
                        let ready_count = inbox
                            .pending
                            .iter()
                            .take_while(|event| {
                                lunco_core_runtime::SimTick(current_tick).wrapping_diff(
                                    lunco_core_runtime::SimTick(event.event.sim_tick),
                                ) > 0
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
                .unwrap_or_default()
        };
        if events.len() > 1 {
            events.sort_by(compare_queued_events);
        }

        // Run if there's work OR a tracked entity vanished (needs on_stop).
        let needs_teardown = !startup_pass
            && world
                .get_resource::<ScenarioDriver<R>>()
                .is_some_and(|d| d.fsm.keys().any(|e| !live.contains(e)));
        if work.is_empty() && !needs_teardown && diag_updates.is_empty() {
            // No scenario can consume this batch. Preserve the allocation for
            // the next pass, but intentionally discard the events themselves.
            events.clear();
            if !startup_pass && let Some(mut inbox) = world.get_resource_mut::<ScriptEventInbox>() {
                inbox.pending = events;
            }
            return;
        }

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let maintenance_context = scenario_execution_context(world, None, false, None);
            let _scope = bridge_core::WorldScope::enter(world, maintenance_context);
            let _phase = bridge_core::ExecutionContextScope::enter(
                maintenance_context.with_phase(lunco_core::RuntimePhase::Preparation),
            );
            driver.runtime.maintain();
            drop(_phase);
            drop(_scope);
            let ScenarioDriver { runtime, fsm, .. } = &mut *driver;

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
                let owner_twin = scenario_twin_owner(world, entity);
                let pass_context = scenario_execution_context(
                    world,
                    owner_twin,
                    run_tick,
                    Some(scene_generation),
                );
                let activation_context = scenario_execution_context(
                    world,
                    owner_twin,
                    true,
                    Some(scene_generation),
                );
                let _scope = bridge_core::WorldScope::enter(world, pass_context);
                // Gate this entity's hook `cmd()`s against the launching session
                // (§3.4). `None` for a host-trusted launch → ungated. Covers the
                // hot-reload `on_stop` below too (still inside this iteration).
                bridge_core::set_script_authority(authority);
                let st = fsm.entry(entity).or_default();
                st.document_id = Some(raw);
                st.directives_generation = Some(generation);
                st.directives = directives;
                let directive_invalid = !directives.is_supported();
                let runs_on_peer = directives.peer_target.runs_on_peer(is_client);
                if directive_invalid || !runs_on_peer {
                    let scene_restart = reload_policy
                        == crate::doc::ScenarioReloadPolicy::Restart
                        && st.started
                        && st.scene_generation != scene_generation;
                    let stop_error = if st.started && st.compiled && !scene_restart {
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context.with_phase(lunco_core::RuntimePhase::Stop),
                        );
                        let _script_entity =
                            bridge_core::ScriptEntityScope::enter(gid as u64);
                        runtime.call_hook(entity, ScenarioHook::Stop, gid)
                    } else {
                        None
                    };
                    runtime.forget_program(entity);
                    if let Some(mut participants) = world
                        .get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
                    {
                        participants.remove_scenario_dependencies(entity);
                    }
                    st.gid = gid;
                    st.started = false;
                    st.visualization_complete = false;
                    st.compiled = false;
                    st.initialized = false;
                    st.dependency_plan = None;
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
                st.owner_twin = owner_twin;
                let mut initialization_diag: Option<Diagnostic> = None;
                let recompiled = st.newly_compiled;
                if maybe_src.is_some() {
                    // The PreUpdate preparation owner admits the source before
                    // the next admitted fixed cycle. A late or unadmitted revision cannot execute
                    // against this tick.
                    continue;
                }

                // Retained programs rebind to replacement participants just
                // like a fresh program. Keep the outgoing VM state, but wait
                // for the incoming scene owners to finish their admission.
                if external_progress_blocked
                    && (!st.started || st.scene_generation != scene_generation)
                {
                    continue;
                }

                if st.compiled && (!st.initialized || st.scene_generation != scene_generation) {
                    // Retain keeps VM state, not references into the outgoing
                    // scene. Rebind the access plan at the replacement boundary.
                    if st.dependency_plan.as_ref().is_some_and(|plan|
                        plan.scene_generation != scene_generation) {
                        st.dependency_plan = None;
                    }
                    if st.dependency_plan.is_none() {
                        let dependency_result = {
                            let _phase = bridge_core::ExecutionContextScope::enter(
                                activation_context
                                    .with_phase(lunco_core::RuntimePhase::DependencyPlan),
                            );
                            let _script_entity =
                                bridge_core::ScriptEntityScope::enter(gid as u64);
                            runtime.simulation_dependencies(entity, gid).and_then(|plan| {
                                let modelica_entities =
                                    resolve_simulation_dependencies(world, plan.modelica_entities)?;
                                let entity_reads = resolve_simulation_entity_access(
                                    world,
                                    plan.entity_reads,
                                    "entity_reads",
                                )?;
                                let entity_writes = resolve_simulation_entity_access(
                                    world,
                                    plan.entity_writes,
                                    "entity_writes",
                                )?;
                                let mut query_reads = plan.query_reads;
                                if query_reads.iter().any(|name| name.trim().is_empty()) {
                                    return Err(Diagnostic::error(
                                        "simulation_dependencies `query_reads` entries must not be empty",
                                        None,
                                        None,
                                    ));
                                }
                                query_reads = query_reads
                                    .into_iter()
                                    .map(|name| name.trim().to_owned())
                                    .collect();
                                query_reads.sort_unstable();
                                query_reads.dedup();
                                Ok(PendingScenarioDependencies {
                                    scene_generation,
                                    modelica_entities,
                                    entity_reads,
                                    entity_writes,
                                    query_reads,
                                    required_inputs: plan.required_inputs,
                                    observed_revision: None,
                                })
                            })
                        };
                        match dependency_result {
                            Ok(plan) => st.dependency_plan = Some(plan),
                            Err(diagnostic) => {
                                fail_scenario_dependency_plan(
                                    world,
                                    runtime,
                                    st,
                                    entity,
                                    raw,
                                    diagnostic,
                                    &mut diag_updates,
                                );
                                continue;
                            }
                        }
                    }

                    let plan_revision = world
                        .get_resource::<lunco_core_runtime::SimulationDependencyStates>()
                        .map(lunco_core_runtime::SimulationDependencyStates::revision);
                    let plan = st
                        .dependency_plan
                        .as_ref()
                        .expect("dependency plan is created before readiness is checked");
                    let unchanged_pending_inputs = !plan.required_inputs.is_empty()
                        && plan.observed_revision == plan_revision;
                    if unchanged_pending_inputs {
                        continue;
                    }

                    match required_input_readiness(world, &plan.required_inputs) {
                        Ok(RequiredInputReadiness::Ready) => {
                            let plan = st
                                .dependency_plan
                                .take()
                                .expect("ready scenario dependency plan is present");
                            if let Some(mut participants) = world.get_resource_mut::<
                                lunco_core_runtime::SimulationBarrierParticipants,
                            >() {
                                participants.replace_scenario_plan(
                                    entity,
                                    plan.modelica_entities,
                                    plan.entity_reads,
                                    plan.entity_writes,
                                    plan.query_reads,
                                );
                            }
                            let _phase = bridge_core::ExecutionContextScope::enter(
                                activation_context
                                    .with_phase(lunco_core::RuntimePhase::Initialization),
                            );
                            let _script_entity =
                                bridge_core::ScriptEntityScope::enter(gid as u64);
                            if !st.initialized {
                                initialization_diag = runtime.initialize(entity, gid);
                                st.initialized = true;
                            }
                            st.scene_generation = scene_generation;
                        }
                        Ok(RequiredInputReadiness::Waiting { revision, reason }) => {
                            if let Some(plan) = st.dependency_plan.as_mut() {
                                plan.observed_revision = Some(revision);
                            }
                            if let Some(key) = st.initialization_progress_key {
                                if let Some(mut progress) = world.get_resource_mut::<
                                    lunco_core_runtime::SimulationProgress,
                                >() {
                                    progress.update_reason(key, reason);
                                }
                            }
                            continue;
                        }
                        Err(diagnostic) => {
                            fail_scenario_dependency_plan(
                                world,
                                runtime,
                                st,
                                entity,
                                raw,
                                diagnostic,
                                &mut diag_updates,
                            );
                            continue;
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
                runtime_errors.extend(st.pending_transition_error.take());
                if !st.started {
                    st.start_event_sequence = pass_start_event_sequence;
                    st.started = true;
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        activation_context.with_phase(lunco_core::RuntimePhase::Start),
                    );
                    let _script_entity =
                        bridge_core::ScriptEntityScope::enter(gid as u64);
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Start, gid) {
                        runtime_errors.push(d);
                    }
                }
                if let Some(key) = st.initialization_progress_key.take() {
                    if let Some(mut progress) =
                        world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
                    {
                        progress.release(key);
                    }
                }
                if receive_events {
                    for ev in &events {
                        if !event_after_start(ev.sequence, st.start_event_sequence) {
                            continue;
                        }
                        if let Some(source) =
                            bridge_core::resolve_entity(world, ev.event.source)
                        {
                            let undeclared_modelica_event = world
                                .get_resource::<
                                    lunco_core_runtime::SimulationBarrierParticipants,
                                >()
                                .is_some_and(|participants| {
                                    participants.is_modelica_participant(source)
                                        && !participants
                                            .scenario_declares_dependency(entity, source)
                                });
                            if undeclared_modelica_event {
                                runtime_errors.push(Diagnostic::error(
                                    format!(
                                        "scenario received event {:?} from Modelica entity {} without this scenario's declared dependency in simulation_dependencies(me, ctx)",
                                        ev.event.name, ev.event.source
                                    ),
                                    None,
                                    None,
                                ));
                                continue;
                            }
                        }
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            pass_context
                                .with_phase(lunco_core::RuntimePhase::Event)
                                .with_producer(lunco_core::RuntimeProducerStamp::simulation(
                                    scene_generation,
                                    ev.event.sim_tick,
                                )),
                        );
                        let _script_entity =
                            bridge_core::ScriptEntityScope::enter(gid as u64);
                        if let Some(d) = runtime.deliver_event(entity, gid, &ev.event) {
                            runtime_errors.push(d);
                        }
                    }
                }
                if run_tick {
                    let _phase = bridge_core::ExecutionContextScope::enter(
                        pass_context.with_phase(lunco_core::RuntimePhase::Behavior),
                    );
                    let _script_entity =
                        bridge_core::ScriptEntityScope::enter(gid as u64);
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Tick, gid) {
                        runtime_errors.push(d);
                    }
                }

                // Authoritative commands a client-peer scenario tried (and was
                // denied) this pass — collected in `bridge_core::cmd_value`. Surface
                // them as ONE per-scenario warning diagnostic, not a per-tick log:
                // the author sees, once, that a client-targeted script is
                // reaching for host-owned state. Warning severity → the scenario
                // still reports Ready (it compiled and ran fine).
                let dropped = bridge_core::take_script_rejects();

                // Publish status: any Error diagnostic → Error state; a warning-only
                // set stays Ready. Cleared to OK only when a (re)compile ran clean.
                let mut diags = Vec::new();
                diags.extend(initialization_diag);
                diags.extend(runtime_errors);
                if !dropped.is_empty() {
                    diags.push(Diagnostic::warning(
                        format!(
                            "client-targeted scenario dropped authoritative command(s): {} — \
                             the host owns shared sim state. Move these to a host scenario, \
                             or remove the `// @peer client` directive.",
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
                st.newly_compiled = false;
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
                        let dead_context = scenario_execution_context(
                            world,
                            st.owner_twin,
                            false,
                            Some(st.scene_generation),
                        );
                        let _scope = bridge_core::WorldScope::enter(world, dead_context);
                        let _phase = bridge_core::ExecutionContextScope::enter(
                            dead_context.with_phase(lunco_core::RuntimePhase::Stop),
                        );
                        let _script_entity =
                            bridge_core::ScriptEntityScope::enter(st.gid as u64);
                        if let Some(diagnostic) = runtime.call_hook(entity, ScenarioHook::Stop, st.gid) {
                            if let Some(raw) = st.document_id {
                                diag_updates.push((raw, Some(vec![diagnostic])));
                            } else {
                                bevy::log::error!("[scenario] on_stop failed without an owning script document: {}", diagnostic.message);
                            }
                        }
                    }
                    runtime.forget(entity);
                    if let Some(mut participants) = world
                        .get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
                    {
                        participants.remove_scenario_dependencies(entity);
                    }
                }
            }
        });

        if !diag_updates.is_empty() {
            let mut store = world.resource_mut::<DocumentDiagnostics>();
            for (raw, status) in diag_updates {
                match status {
                    // Severity-derived: an error-carrying set marks Error, a
                    // warning-only set (e.g. a dropped client-peer command)
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
    ) -> Result<Option<ScenarioIntrospection<B::Value>>, Diagnostic> {
        let Some(fsm) = self.fsm.get(&entity) else {
            return Ok(None);
        };
        let (state, hooks) = match self.runtime.snapshot(entity, builder) {
            Ok(Some(snapshot)) => (snapshot.state, snapshot.hooks),
            Ok(None) => (builder.unit(), Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(Some(ScenarioIntrospection {
            generation: fsm.generation,
            started: fsm.started,
            compiled: fsm.compiled,
            gid: fsm.gid,
            state,
            hooks,
        }))
    }
}

fn retire_pending_compile<R: ScenarioRuntime>(
    world: &mut World,
    driver: &mut ScenarioDriver<R>,
    entity: Entity,
    remove_dependencies: bool,
) {
    let pending = driver
        .fsm
        .get_mut(&entity)
        .and_then(|state| state.pending_compile.take());
    if let Some(pending) = pending {
        driver
            .ready_compiles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|completion| completion.key != pending.key);
        if pending.queued {
            if let Some(mut admission) =
                world.get_resource_mut::<lunco_core_runtime::AsyncWorkAdmission>()
            {
                admission.cancel_queued(pending.key);
            }
        }
        if let Some(mut progress) =
            world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
        {
            progress.release(pending.progress_key);
        }
        if let Some(state) = driver.fsm.get_mut(&entity) {
            if !state.compiled {
                state.attempted_generation = None;
                state.attempted_dependency_revision = None;
                state.preparation_revision = None;
            }
        }
    }
    let initialization_key = driver
        .fsm
        .get_mut(&entity)
        .and_then(|state| state.initialization_progress_key.take());
    if let Some(key) = initialization_key {
        if let Some(mut progress) =
            world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
        {
            progress.release(key);
        }
    }
    if remove_dependencies {
        if let Some(state) = driver.fsm.get_mut(&entity) {
            state.dependency_plan = None;
        }
        if let Some(mut participants) =
            world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
        {
            participants.remove_scenario_dependencies(entity);
        }
    }
}

fn submit_scenario_compile<R: ScenarioRuntime>(
    world: &mut World,
    driver: &mut ScenarioDriver<R>,
    entity: Entity,
    document_id: u64,
    gid: i64,
    generation: u64,
    parameters_revision: u64,
    scene_generation: u64,
    runtime_revision: u64,
    source_dependency_revision: u64,
    job: Box<dyn FnOnce() -> Result<R::PreparedCompile, Diagnostic> + Send + 'static>,
) {
    let Some(pending) = driver
        .fsm
        .get(&entity)
        .and_then(|state| state.pending_compile.as_ref())
    else {
        return;
    };
    let key = pending.key;
    let sender = driver.compile_sender.clone();
    let worker = move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|_| {
                Err(Diagnostic::error(
                    "scenario compilation worker panicked",
                    None,
                    None,
                ))
            });
        let _ = sender.send(CompileCompletion {
            key,
            entity,
            document_id,
            gid,
            generation,
            parameters_revision,
            scene_generation,
            runtime_revision,
            source_dependency_revision,
            result,
        });
    };

    let Some(mut admission) = world.get_resource_mut::<lunco_core_runtime::AsyncWorkAdmission>()
    else {
        if let Some(pending) = driver
            .fsm
            .get_mut(&entity)
            .and_then(|state| state.pending_compile.as_mut())
        {
            pending.result_ready = true;
        }
        driver
            .ready_compiles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(CompileCompletion {
                key,
                entity,
                document_id,
                gid,
                generation,
                parameters_revision,
                scene_generation,
                runtime_revision,
                source_dependency_revision,
                result: Err(Diagnostic::error(
                    "scenario compilation requires AsyncWorkAdmission",
                    None,
                    None,
                )),
            });
        return;
    };
    let capacity_revision = admission.capacity_revision();
    let result = admission.submit(
        lunco_core_runtime::AsyncWorkPriority::SimulationRequired,
        key,
        worker,
    );
    drop(admission);

    match result {
        Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
            if let Some(pending) = driver
                .fsm
                .get_mut(&entity)
                .and_then(|state| state.pending_compile.as_mut())
            {
                pending.queued = true;
                pending.capacity_revision = capacity_revision;
            }
        }
        Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
            if let Some(pending) = driver
                .fsm
                .get_mut(&entity)
                .and_then(|state| state.pending_compile.as_mut())
            {
                pending.queued = false;
                pending.capacity_revision = capacity_revision;
            }
        }
        Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
            if let Some(pending) = driver
                .fsm
                .get_mut(&entity)
                .and_then(|state| state.pending_compile.as_mut())
            {
                pending.queued = false;
                pending.result_ready = true;
                pending.capacity_revision = capacity_revision;
            }
            driver
                .ready_compiles
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(CompileCompletion {
                    key,
                    entity,
                    document_id,
                    gid,
                    generation,
                    parameters_revision,
                    scene_generation,
                    runtime_revision,
                    source_dependency_revision,
                    result: Err(Diagnostic::error(
                        "scenario compilation requires a host worker transport",
                        None,
                        None,
                    )),
                });
        }
    }
}

fn finish_compile_completion<R: ScenarioRuntime>(
    world: &mut World,
    driver: &mut ScenarioDriver<R>,
    key: lunco_core_runtime::AsyncWorkKey,
    entity: Entity,
    document_id: u64,
    gid: i64,
    generation: u64,
    parameters_revision: u64,
    scene_generation: u64,
    runtime_revision: u64,
    _source_dependency_revision: u64,
    outcome: CompileOutcome,
) {
    let Some(pending) = driver
        .fsm
        .get_mut(&entity)
        .and_then(|state| state.pending_compile.take())
    else {
        return;
    };
    if pending.key != key {
        if let Some(state) = driver.fsm.get_mut(&entity) {
            state.pending_compile = Some(pending);
        }
        return;
    }
    let awaiting_activation = matches!(&outcome, CompileOutcome::Ready);
    if !awaiting_activation {
        if let Some(mut progress) =
            world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
        {
            progress.release(pending.progress_key);
        }
    }

    let state = driver.fsm.entry(entity).or_default();
    state.gid = gid;
    state.document_id = Some(document_id);
    state.scene_generation = scene_generation;
    match outcome {
        CompileOutcome::Ready => {
            state.generation = generation;
            state.attempted_generation = Some(generation);
            state.attempted_dependency_revision =
                Some(driver.runtime.source_dependency_revision(entity));
            state.parameters_revision = parameters_revision;
            state.preparation_revision = Some(runtime_revision);
            state.compiled = true;
            state.visualization_complete = false;
            state.initialized = false;
            state.dependency_plan = None;
            state.newly_compiled = true;
            state.initialization_progress_key = Some(pending.progress_key);
        }
        CompileOutcome::Failed(diagnostic) => {
            bevy::log::error!(
                "[scenario] compilation failed for document {document_id}: {}",
                diagnostic.message
            );
            state.attempted_generation = Some(generation);
            state.attempted_dependency_revision =
                Some(driver.runtime.source_dependency_revision(entity));
            state.parameters_revision = parameters_revision;
            state.preparation_revision = Some(runtime_revision);
            state.compiled = false;
            state.initialized = false;
            state.dependency_plan = None;
            state.newly_compiled = false;
            state.initialization_progress_key = None;
            let mut diagnostics = Vec::new();
            diagnostics.extend(state.pending_transition_error.take());
            diagnostics.push(diagnostic);
            if let Some(mut store) = world.get_resource_mut::<DocumentDiagnostics>() {
                store.set_diagnostics(DocumentId::new(document_id), diagnostics);
            }
            if let Some(mut participants) =
                world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
            {
                participants.remove_scenario_dependencies(entity);
            }
        }
        CompileOutcome::Stale => {
            state.attempted_generation = None;
            state.attempted_dependency_revision = None;
            state.preparation_revision = None;
            state.compiled = false;
            state.initialized = false;
            state.dependency_plan = None;
            state.newly_compiled = false;
            state.initialization_progress_key = None;
            if let Some(mut participants) =
                world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
            {
                participants.remove_scenario_dependencies(entity);
            }
        }
    }
}

fn resolve_simulation_dependencies(
    world: &World,
    ids: Vec<i64>,
) -> Result<Vec<Entity>, Diagnostic> {
    let entities = resolve_live_simulation_entities(world, ids, "modelica_entities")?;
    if entities.is_empty() {
        return Ok(Vec::new());
    }
    let participants = world
        .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
        .ok_or_else(|| {
            Diagnostic::error(
                "simulation_dependencies requires SimulationBarrierParticipants",
                None,
                None,
            )
        })?;
    let mut modelica_entities = Vec::with_capacity(entities.len());
    for (id, entity) in entities {
        if !participants.is_modelica_participant(entity) {
            return Err(Diagnostic::error(
                format!(
                    "simulation_dependencies modelica_entities contains entity {id}, but it is not a live Modelica participant"
                ),
                None,
                None,
            ));
        }
        modelica_entities.push(entity);
    }
    Ok(modelica_entities)
}

fn resolve_simulation_entity_access(
    world: &World,
    ids: Vec<i64>,
    field: &str,
) -> Result<Vec<Entity>, Diagnostic> {
    let entities = resolve_live_simulation_entities(world, ids, field)?;
    if entities.is_empty() {
        return Ok(Vec::new());
    }
    if world
        .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
        .is_none()
    {
        return Err(Diagnostic::error(
            "simulation entity access plan requires SimulationBarrierParticipants",
            None,
            None,
        ));
    }
    Ok(entities.into_iter().map(|(_, entity)| entity).collect())
}

fn resolve_live_simulation_entities(
    world: &World,
    ids: Vec<i64>,
    field: &str,
) -> Result<Vec<(i64, Entity)>, Diagnostic> {
    let mut ids = ids;
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut entities = Vec::with_capacity(ids.len());
    for id in ids {
        let raw = u64::try_from(id).map_err(|_| {
            Diagnostic::error(
                format!("simulation_dependencies {field} contains invalid entity id {id}"),
                None,
                None,
            )
        })?;
        let entity = bridge_core::resolve_entity(world, raw).ok_or_else(|| {
            Diagnostic::error(
                format!("simulation_dependencies {field} contains unresolved entity id {id}"),
                None,
                None,
            )
        })?;
        entities.push((id, entity));
    }
    entities.sort_unstable_by_key(|(_, entity)| entity.to_bits());
    entities.dedup_by_key(|(_, entity)| entity.to_bits());
    Ok(entities)
}

enum RequiredInputReadiness {
    Ready,
    Waiting { revision: u64, reason: String },
}

fn fail_scenario_dependency_plan<R: ScenarioRuntime>(
    world: &mut World,
    runtime: &mut R,
    state: &mut Fsm,
    entity: Entity,
    document_id: u64,
    diagnostic: Diagnostic,
    diag_updates: &mut Vec<(u64, Option<Vec<Diagnostic>>)>,
) {
    runtime.forget_program(entity);
    state.compiled = false;
    state.initialized = false;
    state.dependency_plan = None;
    if let Some(mut participants) =
        world.get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
    {
        participants.remove_scenario_dependencies(entity);
    }
    let mut diagnostics = Vec::new();
    diagnostics.extend(state.pending_transition_error.take());
    diagnostics.push(diagnostic);
    diag_updates.push((document_id, Some(diagnostics)));
    if let Some(key) = state.initialization_progress_key.take() {
        if let Some(mut progress) =
            world.get_resource_mut::<lunco_core_runtime::SimulationProgress>()
        {
            progress.release(key);
        }
    }
}

fn required_input_readiness(
    world: &World,
    required_inputs: &[lunco_core_runtime::SimulationDependencyKey],
) -> Result<RequiredInputReadiness, Diagnostic> {
    if required_inputs.is_empty() {
        return Ok(RequiredInputReadiness::Ready);
    }
    let states = world
        .get_resource::<lunco_core_runtime::SimulationDependencyStates>()
        .ok_or_else(|| {
            Diagnostic::error(
                "scenario dependency plan requires SimulationDependencyStates",
                None,
                None,
            )
        })?;
    for key in required_inputs {
        if !states.owner_is_registered(&key.owner) {
            return Err(Diagnostic::error(
                format!(
                    "simulation dependency owner `{}` is not registered for `{}`",
                    key.owner, key.identity
                ),
                None,
                None,
            ));
        }
    }
    for key in required_inputs {
        match states.status(key) {
            Some(lunco_core_runtime::SimulationDependencyStatus::Ready { .. }) => {}
            Some(lunco_core_runtime::SimulationDependencyStatus::Failed { errors, .. }) => {
                return Err(Diagnostic::error(
                    format!(
                        "required simulation input `{}` from `{}` failed: {}",
                        key.identity,
                        key.owner,
                        errors.join("; ")
                    ),
                    None,
                    None,
                ));
            }
            Some(lunco_core_runtime::SimulationDependencyStatus::Pending { .. }) | None => {
                return Ok(RequiredInputReadiness::Waiting {
                    revision: states.revision(),
                    reason: format!(
                        "Waiting for simulation input `{}` from `{}`",
                        key.identity, key.owner
                    ),
                });
            }
        }
    }
    Ok(RequiredInputReadiness::Ready)
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
    pending: Vec<QueuedScenarioEvent>,
    /// Sequence assigned to the next accepted event.
    next_sequence: u64,
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
            next_sequence: 0,
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
    /// `false` is an explicit overflow signal. Rejected events are not cloned;
    /// the caller owns the policy for surfacing overflow, and no event is
    /// silently evicted from the front of the queue.
    pub fn enqueue(&mut self, event: TelemetryEvent) -> bool {
        self.enqueue_with(|| event)
    }

    fn enqueue_borrowed(&mut self, event: &TelemetryEvent) -> bool {
        self.enqueue_with(|| event.clone())
    }

    fn enqueue_with(&mut self, event: impl FnOnce() -> TelemetryEvent) -> bool {
        if self.faulted || self.pending.len() >= SCRIPT_EVENT_INBOX_CAPACITY {
            self.overflowed = true;
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.pending.push(QueuedScenarioEvent {
            sequence: self.next_sequence,
            event: event(),
        });
        self.next_sequence = self.next_sequence.wrapping_add(1);
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

#[derive(Debug)]
struct QueuedScenarioEvent {
    sequence: u64,
    event: TelemetryEvent,
}

fn event_after_start(event_sequence: u64, start_sequence: u64) -> bool {
    event_sequence.wrapping_sub(start_sequence) < (1u64 << 63)
}

fn compare_queued_events(a: &QueuedScenarioEvent, b: &QueuedScenarioEvent) -> Ordering {
    compare_telemetry_events(&a.event, &b.event).then_with(|| a.sequence.cmp(&b.sequence))
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
        (Value::U64(a), Value::U64(b)) => a.cmp(b),
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
        (Value::U64(_), _) => Ordering::Less,
        (_, Value::U64(_)) => Ordering::Greater,
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
/// replaced. Client-targeted scenarios (`// @peer client`) therefore see events
/// fired on the client; host-authoritative events reach them only when explicitly
/// replicated.
pub fn collect_script_events(
    trigger: On<lunco_telemetry_core::StampedTelemetryEvent>,
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
    if inbox.enqueue_borrowed(trigger.event()) {
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
    use super::{SCRIPT_EVENT_INBOX_CAPACITY, ScriptEventInbox, external_progress_is_held};
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
    fn external_preparation_holds_first_script_activation_but_own_compile_hold_does_not() {
        use lunco_core_runtime::{
            SimulationProgress, SimulationProgressKey, SimulationProgressOwner,
        };

        let mut progress = SimulationProgress::default();
        let compile = SimulationProgressKey {
            owner: SimulationProgressOwner::ScriptPreparation,
            operation_id: 1,
        };
        progress.acquire(compile, "compile this scenario");
        assert!(!external_progress_is_held(&progress));

        let modelica = SimulationProgressKey {
            owner: SimulationProgressOwner::ModelicaPreparation,
            operation_id: 2,
        };
        progress.acquire(modelica, "prepare a required Modelica participant");
        assert!(external_progress_is_held(&progress));

        assert!(progress.release(modelica));
        assert!(!external_progress_is_held(&progress));
        assert!(progress.release(compile));
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
        assert_eq!(inbox.pending[0].event.name, "event:0");
    }

    #[test]
    fn collector_latches_a_diagnostic_without_exiting_the_app() {
        let mut app = App::new();
        app.add_plugins(lunco_telemetry_core::LunCoTelemetryCorePlugin)
            .init_resource::<super::ScenarioExecutionGate>()
            .init_resource::<ScriptEventInbox>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_observer(super::collect_script_events);

        for index in 0..=SCRIPT_EVENT_INBOX_CAPACITY {
            app.world_mut().trigger(event(index));
            app.world_mut().flush();
        }

        let inbox = app.world().resource::<ScriptEventInbox>();
        assert!(inbox.faulted);
        assert!(inbox.pending.is_empty());
        assert_eq!(inbox.dropped, 1);
        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert!(
            diagnostics
                .findings
                .iter()
                .any(|finding| finding.code == "telemetry-event-overflow")
        );
    }
}

#[cfg(test)]
mod lifecycle_readiness_tests {
    use super::*;
    use crate::doc::ScriptDocument;
    use std::sync::{Arc, Condvar, Mutex, mpsc};

    fn scene_coordinator_at_generation(generation: u64) -> lunco_core::SceneTransitionCoordinator {
        let mut coordinator = lunco_core::SceneTransitionCoordinator::default();
        for _ in 0..generation {
            let request = lunco_core::SceneTransitionRequest::clear();
            assert_eq!(
                coordinator.admit(request),
                lunco_core::SceneTransitionAdmission::Admitted
            );
            assert!(coordinator.take_admitted().is_some());
            let id = coordinator.start(lunco_core::SceneTransition::clear());
            assert!(coordinator.complete(id));
        }
        coordinator
    }

    #[test]
    fn scenario_directives_bind_only_to_supported_peer_and_timing_values() {
        assert_eq!(
            ScenarioDirectives::from_source("fn on_start(me, ctx) {}"),
            ScenarioDirectives::default()
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @peer host\n// @timing simulation\n"),
            ScenarioDirectives::default()
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @peer client\n").peer_target,
            ScenarioPeerTarget::Client
        );
        assert_eq!(
            ScenarioDirectives::from_source("// @peer both\n").peer_target,
            ScenarioPeerTarget::Both
        );

        let unknown_peer = ScenarioDirectives::from_source("// @peer clinet\n");
        assert_eq!(unknown_peer.peer_target, ScenarioPeerTarget::Unsupported);
        assert_eq!(unknown_peer.timing, ScriptTiming::Simulation);
        assert_eq!(unknown_peer.diagnostics().len(), 1);

        let unknown_timing = ScenarioDirectives::from_source("// @timing presentation\n");
        assert_eq!(unknown_timing.peer_target, ScenarioPeerTarget::Host);
        assert_eq!(unknown_timing.timing, ScriptTiming::Unsupported);
        assert_eq!(unknown_timing.diagnostics().len(), 1);

        let invalid = ScenarioDirectives::from_source("// @peer clinet\n// @timing presentation\n");
        assert_eq!(invalid.diagnostics().len(), 2);

        let unknown_directive = ScenarioDirectives::from_source("// @peerhost\n");
        assert!(!unknown_directive.is_supported());
        assert_eq!(unknown_directive.diagnostics().len(), 1);
    }

    #[test]
    fn application_scenario_runs_without_a_scene_generation() {
        let mut world = World::new();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(71),
            ScriptDocument::new(71, ScriptLanguage::Rhai, "application scenario"),
        );
        world.spawn(ScriptedModel {
            document_id: Some(71),
            language: Some(ScriptLanguage::Rhai),
            ..Default::default()
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts.clone(),
            Arc::new(Mutex::new(false)),
        )));

        run_scenarios(&mut world);

        assert!(
            world
                .resource::<lunco_core::RuntimeFaults>()
                .first
                .is_none()
        );
        assert!(calls.lock().unwrap().contains(&RecordedCall::Compile));
        assert!(contexts.lock().unwrap().iter().any(|context| {
            context.route.is_some_and(|route| {
                route.scope == lunco_core::RuntimeScope::Application
                    && route.generation == 0
                    && route.owner_id.is_none()
            })
        }));
    }

    #[test]
    fn scene_owned_scenario_requires_a_committed_scene_generation() {
        let mut world = World::new();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(72),
            ScriptDocument::new(72, ScriptLanguage::Rhai, "scene scenario"),
        );
        world.spawn((
            ScriptedModel {
                document_id: Some(72),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
            crate::SceneOwnedScript,
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(false)),
        )));

        run_scenarios(&mut world);

        let fault = world
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .as_ref()
            .expect("scene-owned work without a committed generation must fault");
        assert_eq!(fault.kind, "scenario-generation-missing");
        assert!(!calls.lock().unwrap().contains(&RecordedCall::Compile));
    }

    #[test]
    fn closing_a_pending_scenario_document_releases_its_progress_hold() {
        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(2));
        world.insert_resource(ScriptRegistry::default());
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        let entity = world
            .spawn(ScriptedModel {
                document_id: Some(404),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            })
            .id();
        let calls = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(false)),
        )));

        let key = lunco_core_runtime::AsyncWorkKey::new(
            lunco_core_runtime::AsyncWorkKind::RhaiCompilation,
            2,
            (u128::from(404u64) << 64) | u128::from(entity.to_bits()),
            1,
            0,
        );
        let progress_key = lunco_core_runtime::SimulationProgressKey {
            owner: lunco_core_runtime::SimulationProgressOwner::ScriptPreparation,
            operation_id: 42,
        };
        world
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .acquire(progress_key, "preparing scenario");
        world
            .resource_mut::<ScenarioDriver<RecordingRuntime>>()
            .fsm
            .insert(
                entity,
                Fsm {
                    pending_compile: Some(PendingCompile {
                        key,
                        progress_key,
                        generation: 1,
                        parameters_revision: 0,
                        scene_generation: 2,
                        runtime_revision: 0,
                        source_dependency_revision: 0,
                        queued: false,
                        result_ready: false,
                        capacity_revision: 0,
                    }),
                    ..Default::default()
                },
            );

        ScenarioDriver::<RecordingRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);

        assert!(
            world
                .resource::<ScenarioDriver<RecordingRuntime>>()
                .fsm
                .get(&entity)
                .is_some_and(|state| state.pending_compile.is_none())
        );
        assert!(
            !world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );
    }

    #[test]
    fn retained_scenario_rebinds_dependencies_without_restarting() {
        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(1));
        world.init_resource::<ScriptRegistry>();
        world.init_resource::<DocumentDiagnostics>();
        world.init_resource::<ScriptEventInbox>();
        world.init_resource::<lunco_core_runtime::SimTick>();
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(81),
            ScriptDocument::new(81, ScriptLanguage::Rhai, "scenario"),
        );
        let host = world
            .spawn((
                ScriptedModel {
                    document_id: Some(81),
                    language: Some(ScriptLanguage::Rhai),
                    reload_policy: crate::doc::ScenarioReloadPolicy::Retain,
                    ..Default::default()
                },
                crate::TwinOwnedScript {
                    twin: lunco_workspace::TwinId::new(17),
                    close_document: false,
                },
            ))
            .id();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let plan_calls = Arc::new(Mutex::new(0));
        world.insert_resource(ScenarioDriver::with_runtime(DependencyRuntime {
            plan: ScenarioDependencyPlan {
                query_reads: vec!["Before".into()],
                ..Default::default()
            },
            calls: calls.clone(),
            plan_calls: plan_calls.clone(),
        }));
        ScenarioDriver::<DependencyRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);
        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(*plan_calls.lock().unwrap(), 1);
        assert!(
            world
                .resource::<lunco_core_runtime::SimulationBarrierParticipants>()
                .scenario_declares_query_read(host, "Before")
        );

        world.insert_resource(scene_coordinator_at_generation(2));
        world
            .resource_mut::<ScenarioDriver<DependencyRuntime>>()
            .runtime
            .plan
            .query_reads = vec!["After".into()];
        // A retained VM must not validate its replacement dependency plan
        // while the new scene's Modelica participant projection is held.
        world.insert_resource(lunco_readiness::ReadinessState {
            world_hold: true,
            ..Default::default()
        });
        ScenarioDriver::<DependencyRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);
        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(*plan_calls.lock().unwrap(), 1);
        assert!(world.resource::<ScenarioDriver<DependencyRuntime>>().fsm[&host].started);
        world
            .resource_mut::<lunco_readiness::ReadinessState>()
            .world_hold = false;
        ScenarioDriver::<DependencyRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);
        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert_eq!(*plan_calls.lock().unwrap(), 2);
        let participants = world.resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        assert!(participants.scenario_declares_query_read(host, "After"));
        assert!(!participants.scenario_declares_query_read(host, "Before"));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![RecordedCall::Start, RecordedCall::Tick, RecordedCall::Tick]
        );
    }

    #[test]
    fn scenario_waits_for_owner_input_and_resumes_on_its_commit_revision() {
        let key = lunco_core_runtime::SimulationDependencyKey::new("sysml.twin-analysis", "school")
            .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let plan_calls = Arc::new(Mutex::new(0));
        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(lunco_core_runtime::SimTick(0));
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        let mut input_states = lunco_core_runtime::SimulationDependencyStates::default();
        input_states.register_owner(key.owner.clone()).unwrap();
        input_states
            .publish(
                key.clone(),
                lunco_core_runtime::SimulationDependencyStatus::Pending { operation_id: 1 },
            )
            .unwrap();
        world.insert_resource(input_states);
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(81),
            ScriptDocument::new(81, ScriptLanguage::Rhai, "scenario"),
        );
        world.spawn(ScriptedModel {
            document_id: Some(81),
            language: Some(ScriptLanguage::Rhai),
            ..Default::default()
        });
        world.insert_resource(ScenarioDriver::with_runtime(DependencyRuntime {
            plan: ScenarioDependencyPlan {
                modelica_entities: Vec::new(),
                entity_reads: Vec::new(),
                entity_writes: Vec::new(),
                query_reads: Vec::new(),
                required_inputs: vec![key.clone()],
            },
            calls: calls.clone(),
            plan_calls: plan_calls.clone(),
        }));

        ScenarioDriver::<DependencyRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);
        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(*plan_calls.lock().unwrap(), 1);
        let blocker = world
            .resource::<lunco_core_runtime::SimulationProgress>()
            .blockers()
            .next()
            .expect("pending scenario input keeps its activation hold");
        assert!(blocker.reason.contains("school"));

        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(*plan_calls.lock().unwrap(), 1);

        world
            .resource_mut::<lunco_core_runtime::SimulationDependencyStates>()
            .publish(
                key,
                lunco_core_runtime::SimulationDependencyStatus::Ready {
                    source_revision: 44,
                },
            )
            .unwrap();
        ScenarioDriver::<DependencyRuntime>::run(&mut world, ScriptLanguage::Rhai);
        assert!(calls.lock().unwrap().contains(&RecordedCall::Start));
        assert!(
            !world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );
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
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts,
            Arc::new(Mutex::new(false)),
        )));
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(72),
            ScriptDocument::new(
                72,
                ScriptLanguage::Rhai,
                "// @peer clinet\n// @timing presentation\n",
            ),
        );
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(74),
            ScriptDocument::new(
                74,
                ScriptLanguage::Rhai,
                "// @peer host\n// @timing simulation\n",
            ),
        );
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(lunco_core_runtime::SimTick(4));

        run_scenarios(&mut world);
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
            .expect("invalid peer metadata is visible to the document owner");
        assert_eq!(status.diagnostics.len(), 2);
        assert!(
            status.diagnostics[0]
                .message
                .contains("unsupported scenario @peer")
        );
        assert!(
            status.diagnostics[1]
                .message
                .contains("unsupported scenario @timing")
        );

        run_scenarios(&mut world);
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
        Visualization,
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
        type PreparedCompile = ();

        fn async_work_kind(&self) -> lunco_core_runtime::AsyncWorkKind {
            lunco_core_runtime::AsyncWorkKind::RhaiCompilation
        }

        fn prepare_compile(
            &self,
            _source: String,
            _asset_id: Option<String>,
        ) -> CompilePreparation<Self::PreparedCompile> {
            CompilePreparation::Ready(())
        }

        fn commit_compile(
            &mut self,
            _entity: Entity,
            _prepared: Self::PreparedCompile,
            _params: &ScenarioParameters,
        ) -> CompileOutcome {
            self.0.lock().unwrap().push(RecordedCall::Compile);
            self.1
                .lock()
                .unwrap()
                .push(bridge_core::execution_context());
            CompileOutcome::Ready
        }

        fn call_visualization(&mut self, _entity: Entity, _self_gid: i64) -> Option<Diagnostic> {
            self.1
                .lock()
                .unwrap()
                .push(bridge_core::execution_context());
            self.0.lock().unwrap().push(RecordedCall::Visualization);
            None
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

    #[derive(Clone)]
    struct DependencyRuntime {
        plan: ScenarioDependencyPlan,
        calls: Arc<Mutex<Vec<RecordedCall>>>,
        plan_calls: Arc<Mutex<usize>>,
    }

    impl ScenarioRuntime for DependencyRuntime {
        type PreparedCompile = ();

        fn async_work_kind(&self) -> lunco_core_runtime::AsyncWorkKind {
            lunco_core_runtime::AsyncWorkKind::RhaiCompilation
        }

        fn prepare_compile(
            &self,
            _source: String,
            _asset_id: Option<String>,
        ) -> CompilePreparation<Self::PreparedCompile> {
            CompilePreparation::Ready(())
        }

        fn commit_compile(
            &mut self,
            _entity: Entity,
            _prepared: Self::PreparedCompile,
            _params: &ScenarioParameters,
        ) -> CompileOutcome {
            CompileOutcome::Ready
        }

        fn simulation_dependencies(
            &mut self,
            _entity: Entity,
            _self_gid: i64,
        ) -> Result<ScenarioDependencyPlan, Diagnostic> {
            *self.plan_calls.lock().unwrap() += 1;
            Ok(self.plan.clone())
        }

        fn call_hook(
            &mut self,
            _entity: Entity,
            hook: ScenarioHook,
            _self_gid: i64,
        ) -> Option<Diagnostic> {
            self.calls.lock().unwrap().push(match hook {
                ScenarioHook::Start => RecordedCall::Start,
                ScenarioHook::Tick => RecordedCall::Tick,
                ScenarioHook::Stop => RecordedCall::Stop,
            });
            None
        }

        fn deliver_event(
            &mut self,
            _entity: Entity,
            _self_gid: i64,
            event: &TelemetryEvent,
        ) -> Option<Diagnostic> {
            self.calls
                .lock()
                .unwrap()
                .push(RecordedCall::Event(event.name.clone()));
            None
        }

        fn forget(&mut self, _entity: Entity) {}
    }

    fn run_scenarios(world: &mut World) {
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        ScenarioDriver::<RecordingRuntime>::prepare_compiles(world, ScriptLanguage::Rhai);
        ScenarioDriver::<RecordingRuntime>::run(world, ScriptLanguage::Rhai);
    }

    fn run_scenarios_without_simulation_tick(world: &mut World) {
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        ScenarioDriver::<RecordingRuntime>::prepare_compiles(world, ScriptLanguage::Rhai);
        ScenarioDriver::<RecordingRuntime>::run_without_simulation_tick(
            world,
            ScriptLanguage::Rhai,
        );
    }

    #[test]
    fn scenario_startup_runs_before_the_first_fixed_tick() {
        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(lunco_core_runtime::SimTick(0));
        world.insert_resource(lunco_time::SceneTimeState {
            phase: lunco_time::SceneTimePhase::Ready,
            ..Default::default()
        });
        world.insert_resource(lunco_readiness::ReadinessState::default());
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("before_start", 0));
        world.spawn(ScriptedModel {
            document_id: Some(81),
            language: Some(ScriptLanguage::Rhai),
            ..Default::default()
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts.clone(),
            Arc::new(Mutex::new(false)),
        )));
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(81),
            ScriptDocument::new(81, ScriptLanguage::Rhai, "scenario"),
        );

        ScenarioDriver::<RecordingRuntime>::prepare_compiles(&mut world, ScriptLanguage::Rhai);
        ScenarioDriver::<RecordingRuntime>::run_startup(&mut world, ScriptLanguage::Rhai);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![RecordedCall::Compile, RecordedCall::Start]
        );
        let start = contexts.lock().unwrap()[1];
        assert_eq!(start.clock, lunco_core::RuntimeClock::Simulation);
        assert_eq!(start.sequence, Some(0));

        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("after_start", 0));
        world.resource_mut::<lunco_core_runtime::SimTick>().0 = 1;
        ScenarioDriver::<RecordingRuntime>::run(&mut world, ScriptLanguage::Rhai);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Event("after_start".into()),
                RecordedCall::Tick,
            ]
        );
        assert!(
            !calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::Event("before_start".into()))
        );
        let contexts = contexts.lock().unwrap();
        assert!(contexts.iter().any(|context| {
            context.phase == lunco_core::RuntimePhase::Behavior
                && context.clock == lunco_core::RuntimeClock::Simulation
                && context.sequence == Some(1)
        }));
    }

    #[test]
    fn visualization_hook_runs_during_modelica_preparation_before_start() {
        use lunco_core_runtime::{SimulationProgressKey, SimulationProgressOwner};

        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(DocumentDiagnostics::default());
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        world.init_resource::<ScriptEventInbox>();
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(81),
            ScriptDocument::new(81, ScriptLanguage::Rhai, "scenario"),
        );
        let entity = world
            .spawn((
                ScriptedModel {
                    document_id: Some(81),
                    language: Some(ScriptLanguage::Rhai),
                    ..Default::default()
                },
                crate::TwinOwnedScript {
                    twin: lunco_workspace::TwinId::new(17),
                    close_document: false,
                },
                crate::SceneOwnedScript,
            ))
            .id();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts.clone(),
            Arc::new(Mutex::new(false)),
        )));
        world
            .resource_mut::<ScenarioDriver<RecordingRuntime>>()
            .fsm
            .insert(
                entity,
                Fsm {
                    generation: 0,
                    document_id: Some(81),
                    attempted_generation: Some(0),
                    attempted_dependency_revision: Some(0),
                    started: false,
                    visualization_complete: false,
                    compiled: true,
                    preparation_revision: Some(0),
                    initialized: false,
                    gid: 17,
                    scene_generation: 1,
                    directives_generation: Some(0),
                    ..Default::default()
                },
            );
        let modelica_key = SimulationProgressKey {
            owner: SimulationProgressOwner::ModelicaPreparation,
            operation_id: 9,
        };
        world
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .acquire(modelica_key, "Preparing Modelica participant");

        ScenarioDriver::<RecordingRuntime>::run_visualization(&mut world, ScriptLanguage::Rhai);
        assert_eq!(*calls.lock().unwrap(), [RecordedCall::Visualization]);
        let visualization_context = contexts.lock().unwrap()[0];
        assert_eq!(
            visualization_context.route.unwrap().cycle,
            lunco_core::RuntimeCycle::Visualization
        );
        assert_eq!(
            visualization_context.phase,
            lunco_core::RuntimePhase::Visualization
        );
        assert_eq!(
            visualization_context.clock,
            lunco_core::RuntimeClock::Presentation
        );

        ScenarioDriver::<RecordingRuntime>::run_without_simulation_tick(
            &mut world,
            ScriptLanguage::Rhai,
        );
        assert_eq!(*calls.lock().unwrap(), [RecordedCall::Visualization]);

        world
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .release(modelica_key);
        ScenarioDriver::<RecordingRuntime>::run_without_simulation_tick(
            &mut world,
            ScriptLanguage::Rhai,
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [RecordedCall::Visualization, RecordedCall::Start]
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct CompileGate {
        released: Mutex<bool>,
        changed: Condvar,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl CompileGate {
        fn new() -> Self {
            Self {
                released: Mutex::new(false),
                changed: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.changed.notify_all();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Clone)]
    struct GatedRuntime {
        gates: Arc<HashMap<String, Arc<CompileGate>>>,
        started: mpsc::Sender<String>,
        committed: Arc<Mutex<Vec<String>>>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl ScenarioRuntime for GatedRuntime {
        type PreparedCompile = String;

        fn async_work_kind(&self) -> lunco_core_runtime::AsyncWorkKind {
            lunco_core_runtime::AsyncWorkKind::RhaiCompilation
        }

        fn prepare_compile(
            &self,
            source: String,
            _asset_id: Option<String>,
        ) -> CompilePreparation<Self::PreparedCompile> {
            let gate = self.gates[&source].clone();
            let started = self.started.clone();
            CompilePreparation::Worker(Box::new(move || {
                started.send(source.clone()).unwrap();
                gate.wait();
                Ok(source)
            }))
        }

        fn commit_compile(
            &mut self,
            _entity: Entity,
            prepared: Self::PreparedCompile,
            _params: &ScenarioParameters,
        ) -> CompileOutcome {
            self.committed.lock().unwrap().push(prepared);
            CompileOutcome::Ready
        }

        fn call_hook(
            &mut self,
            _entity: Entity,
            _hook: ScenarioHook,
            _self_gid: i64,
        ) -> Option<Diagnostic> {
            None
        }

        fn deliver_event(
            &mut self,
            _entity: Entity,
            _self_gid: i64,
            _event: &TelemetryEvent,
        ) -> Option<Diagnostic> {
            None
        }

        fn forget(&mut self, _entity: Entity) {}
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn prepare_gated_scenarios(world: &mut World) {
        ScenarioDriver::<GatedRuntime>::prepare_compiles(world, ScriptLanguage::Rhai);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn async_compiles_commit_as_one_stable_batch_after_reverse_completion() {
        use std::time::{Duration, Instant};

        let alpha_gate = Arc::new(CompileGate::new());
        let beta_gate = Arc::new(CompileGate::new());
        let gates = Arc::new(HashMap::from([
            ("alpha".to_owned(), alpha_gate.clone()),
            ("beta".to_owned(), beta_gate.clone()),
        ]));
        let (started_tx, started_rx) = mpsc::channel();
        let committed = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, lunco_core_runtime::AsyncWorkAdmissionPlugin));
        app.init_resource::<lunco_core_runtime::SimulationProgress>()
            .init_resource::<ScriptRegistry>()
            .init_resource::<DocumentDiagnostics>()
            .init_resource::<ScriptEventInbox>()
            .insert_resource(scene_coordinator_at_generation(1));
        app.insert_resource(ScenarioDriver::with_runtime(GatedRuntime {
            gates,
            started: started_tx,
            committed: committed.clone(),
        }));

        app.world_mut().spawn((
            ScriptedModel {
                document_id: Some(81),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
            lunco_core::GlobalEntityId::from_raw(10),
        ));
        app.world_mut().spawn((
            ScriptedModel {
                document_id: Some(82),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            },
            lunco_core::GlobalEntityId::from_raw(20),
        ));
        {
            let mut registry = app.world_mut().resource_mut::<ScriptRegistry>();
            registry.insert_document(
                DocumentId::new(81),
                ScriptDocument::new(81, ScriptLanguage::Rhai, "alpha"),
            );
            registry.insert_document(
                DocumentId::new(82),
                ScriptDocument::new(82, ScriptLanguage::Rhai, "beta"),
            );
        }
        app.add_systems(PreUpdate, prepare_gated_scenarios);

        app.update();
        let started = [
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ];
        assert_eq!(started.iter().collect::<HashSet<_>>().len(), 2);
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );

        beta_gate.release();
        let deadline = Instant::now() + Duration::from_secs(2);
        while app
            .world()
            .resource::<lunco_core_runtime::AsyncWorkAdmission>()
            .snapshot()
            .finished
            < 1
        {
            assert!(Instant::now() < deadline, "beta compile did not complete");
            std::thread::yield_now();
        }
        app.update();
        assert!(committed.lock().unwrap().is_empty());
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );

        alpha_gate.release();
        let deadline = Instant::now() + Duration::from_secs(2);
        while app
            .world()
            .resource::<lunco_core_runtime::AsyncWorkAdmission>()
            .snapshot()
            .finished
            < 2
        {
            assert!(Instant::now() < deadline, "alpha compile did not complete");
            std::thread::yield_now();
        }
        app.update();

        assert_eq!(*committed.lock().unwrap(), ["alpha", "beta"]);
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );
        ScenarioDriver::<GatedRuntime>::run_without_simulation_tick(
            app.world_mut(),
            ScriptLanguage::Rhai,
        );
        assert!(
            !app.world()
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );
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
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(true)),
        )));
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(73),
            ScriptDocument::new(
                73,
                ScriptLanguage::Rhai,
                "// @peer host\n// @timing simulation\n",
            ),
        );
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(lunco_core_runtime::SimTick(1));

        run_scenarios(&mut world);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                RecordedCall::Compile,
                RecordedCall::Start,
                RecordedCall::Tick
            ]
        );

        assert!(
            world
                .resource_mut::<ScriptRegistry>()
                .reload_external_source(
                    DocumentId::new(73),
                    "// @peer clinet\n// @timing presentation\n",
                )
        );
        run_scenarios(&mut world);

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
        assert!(
            diagnostics.diagnostics[2]
                .message
                .contains("on_stop failed")
        );

        run_scenarios(&mut world);
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
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts.clone(),
            Arc::new(Mutex::new(false)),
        )));
        world.insert_resource(ScriptRegistry::default());
        world.resource_mut::<ScriptRegistry>().insert_document(
            DocumentId::new(71),
            ScriptDocument::new(71, ScriptLanguage::Rhai, ""),
        );
        world.insert_resource(DocumentDiagnostics::default());
        world.insert_resource(ScriptEventInbox::default());
        world.insert_resource(scene_coordinator_at_generation(1));
        world.insert_resource(lunco_core_runtime::SimTick(5));
        world.insert_resource(lunco_readiness::ReadinessState {
            world_hold: false,
            held_entities: vec![owner],
        });
        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("before_release", 4));

        run_scenarios(&mut world);
        assert!(calls.lock().unwrap().is_empty());

        world
            .resource_mut::<lunco_readiness::ReadinessState>()
            .held_entities
            .clear();
        run_scenarios(&mut world);
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
        run_scenarios(&mut world);
        assert!(
            !calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::Event("current_tick".into()))
        );

        world.resource_mut::<lunco_core_runtime::SimTick>().0 = 6;
        run_scenarios(&mut world);

        let recorded_calls = calls.lock().unwrap();
        assert!(recorded_calls.contains(&RecordedCall::Event("current_tick".into())));
        assert!(!recorded_calls.contains(&RecordedCall::Event("before_release".into())));
        drop(recorded_calls);

        world
            .resource_mut::<ScriptEventInbox>()
            .enqueue(event("paused_update", 6));
        run_scenarios_without_simulation_tick(&mut world);
        assert!(
            calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::Event("paused_update".into()))
        );

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

    #[test]
    fn twin_shutdown_stops_only_owned_scripts_with_the_outgoing_owner_route() {
        let first = lunco_workspace::TwinId::new(17);
        let second = lunco_workspace::TwinId::new(23);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        let mut world = World::new();
        world.insert_resource(ScriptRegistry::default());
        world.insert_resource(scene_coordinator_at_generation(4));
        world.insert_resource(ScenarioDriver::with_runtime(RecordingRuntime(
            calls.clone(),
            contexts.clone(),
            Arc::new(Mutex::new(false)),
        )));

        let first_entity = world
            .spawn((
                ScriptedModel {
                    document_id: Some(901),
                    language: Some(ScriptLanguage::Rhai),
                    ..Default::default()
                },
                super::super::TwinOwnedScript {
                    twin: first,
                    close_document: true,
                },
                super::super::SceneOwnedScript,
            ))
            .id();
        let second_entity = world
            .spawn((
                ScriptedModel {
                    document_id: Some(902),
                    language: Some(ScriptLanguage::Rhai),
                    ..Default::default()
                },
                super::super::TwinOwnedScript {
                    twin: second,
                    close_document: true,
                },
            ))
            .id();
        let app_entity = world
            .spawn(ScriptedModel {
                document_id: Some(903),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            })
            .id();
        for (entity, document_id) in [(first_entity, 901), (second_entity, 902), (app_entity, 903)]
        {
            world.resource_mut::<ScriptRegistry>().insert_document(
                DocumentId::new(document_id),
                ScriptDocument::new(document_id, ScriptLanguage::Rhai, ""),
            );
            world
                .resource_mut::<ScenarioDriver<RecordingRuntime>>()
                .fsm
                .insert(
                    entity,
                    Fsm {
                        document_id: Some(document_id),
                        started: true,
                        compiled: true,
                        gid: i64::try_from(document_id).expect("test document id fits gid"),
                        ..Default::default()
                    },
                );
        }

        let stopped =
            ScenarioDriver::<RecordingRuntime>::stop_twin_owned_scripts(&mut world, first);

        assert_eq!(stopped, vec![first_entity]);
        assert!(world.get::<ScriptedModel>(first_entity).is_none());
        assert!(
            world
                .get::<super::super::SceneOwnedScript>(first_entity)
                .is_none()
        );
        assert!(world.get::<ScriptedModel>(second_entity).is_some());
        assert!(world.get::<ScriptedModel>(app_entity).is_some());
        assert!(
            world
                .get::<super::super::TwinOwnedScript>(second_entity)
                .is_some()
        );
        assert!(
            world
                .resource::<ScriptRegistry>()
                .documents
                .contains_key(&DocumentId::new(902))
        );
        assert!(
            world
                .resource::<ScriptRegistry>()
                .documents
                .contains_key(&DocumentId::new(903))
        );
        assert!(
            !world
                .resource::<ScriptRegistry>()
                .documents
                .contains_key(&DocumentId::new(901))
        );
        assert_eq!(*calls.lock().unwrap(), vec![RecordedCall::Stop]);
        assert!(contexts.lock().unwrap().iter().any(|context| {
            context.route.is_some_and(|route| {
                route.scope == lunco_core::RuntimeScope::Twin
                    && route.owner_id == Some(first.raw())
                    && route.generation == 4
            })
        }));
    }

    #[test]
    fn application_scenarios_keep_scope_across_twin_scene_generations() {
        let twin = lunco_workspace::TwinId::new(17);
        let mut world = World::new();
        world.insert_resource(scene_coordinator_at_generation(6));
        let application = world.spawn(ScriptedModel::default()).id();
        let twin_script = world
            .spawn((
                ScriptedModel::default(),
                crate::TwinOwnedScript {
                    twin,
                    close_document: true,
                },
            ))
            .id();
        let scene_script = world
            .spawn((ScriptedModel::default(), crate::SceneOwnedScript))
            .id();

        assert_eq!(scenario_scope_generation(&world, application, 6), 0);
        assert!(scenario_scene_generation_is_current(&world, application, 0));
        assert!(!scenario_scene_generation_is_current(
            &world,
            application,
            6
        ));
        assert_eq!(scenario_scope_generation(&world, twin_script, 6), 6);
        assert_eq!(scenario_scope_generation(&world, scene_script, 6), 6);

        let app_route = scenario_execution_context(&world, None, false, Some(6))
            .route
            .expect("application route");
        let twin_route = scenario_execution_context(
            &world,
            scenario_twin_owner(&world, twin_script),
            false,
            Some(6),
        )
        .route
        .expect("Twin route");
        assert_eq!(app_route.scope, lunco_core::RuntimeScope::Application);
        assert_eq!(app_route.generation, 0);
        assert_eq!(app_route.owner_id, None);
        assert_eq!(twin_route.scope, lunco_core::RuntimeScope::Twin);
        assert_eq!(twin_route.generation, 6);
        assert_eq!(twin_route.owner_id, Some(twin.raw()));
    }
}
