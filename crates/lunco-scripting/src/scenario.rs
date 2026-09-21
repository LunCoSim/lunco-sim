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
/// Authored programs attach during scene composition even while this gate is
/// closed, so their event subscriptions exist before the first physics tick.
/// The normal runtime leaves this enabled. Headless scene runners may disable
/// it while an authored scene's asynchronous participants are being compiled
/// and admitted, so `on_start` cannot begin measuring scenario time against a
/// world that is still held for readiness. This is a lifecycle boundary, not a
/// second pause mechanism: once enabled, the ordinary `ScriptedModel::paused`
/// state remains the only per-program pause control.
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
        inbox.clear();
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

/// Open scenario lifecycle only after the completed scene's readiness policy
/// releases every participant. Once opened it stays open: a later dynamically
/// attached participant must not pause an already-running mission script.
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
/// `Time<Virtual>` is the single owner of simulation pause. The paused pass
/// keeps discrete lifecycle events responsive while the fixed simulation clock
/// is stopped; it never advances `on_tick`, task, or mission work.
pub fn simulation_is_paused(time: Option<Res<Time<Virtual>>>) -> bool {
    time.is_some_and(|time| time.is_paused())
}

/// Run condition for continuous fixed-step scenario behavior. Every consumer
/// that mutates simulation state must use the same virtual-clock predicate as
/// the time spine and co-simulation master; otherwise a residual fixed overstep
/// can execute `on_tick` while the shared barrier is paused and advance Rhai
/// state without advancing `SimTick` or physics.
pub fn simulation_is_running(time: Option<Res<Time<Virtual>>>) -> bool {
    lunco_time::simulation_is_running(time)
}

#[cfg(test)]
mod readiness_gate_tests {
    use super::*;
    use lunco_core::{SceneTransition, SceneTransitionCompleted, SceneTransitionStarted};
    use lunco_readiness::ReadinessState;

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

        app.world_mut().trigger(SceneTransitionStarted {
            transition: SceneTransition::clear(),
        });
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(!app.world().resource::<ScenarioReadinessArm>().0);

        app.world_mut().trigger(SceneTransitionCompleted {
            transition: SceneTransition::clear(),
        });
        app.world_mut().resource_mut::<ReadinessState>().world_hold = true;
        app.update();
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(app.world().resource::<ScenarioReadinessArm>().0);

        app.world_mut().resource_mut::<ReadinessState>().world_hold = false;
        app.update();
        assert!(app.world().resource::<ScenarioExecutionGate>().enabled);
        assert!(!app.world().resource::<ScenarioReadinessArm>().0);

        // A participant arriving after scenario start cannot rewind lifecycle.
        app.world_mut().resource_mut::<ReadinessState>().world_hold = true;
        app.update();
        assert!(app.world().resource::<ScenarioExecutionGate>().enabled);

        // The next authoritative transition closes it again.
        app.world_mut().trigger(SceneTransitionStarted {
            transition: SceneTransition::load("next.usda", ""),
        });
        assert!(!app.world().resource::<ScenarioExecutionGate>().enabled);
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
/// Authored via a `// @scope client` (or `both`) directive on one of the first
/// lines of the script source, so it rides the same channel for API-attached
/// (`RunScenario`) and USD-embedded scenarios with no wire or schema change.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptScope {
    #[default]
    Host,
    Client,
    Both,
}

impl ScriptScope {
    /// Parse a `// @scope <host|client|both>` directive from the script source
    /// (scanned in the first lines). Absent / unrecognized ⇒ [`Host`](Self::Host).
    pub fn from_source(src: &str) -> Self {
        for line in src.lines().take(24) {
            let t = line.trim_start();
            let Some(rest) = t.strip_prefix("//") else {
                continue;
            };
            let rest = rest.trim_start().trim_start_matches('!').trim_start();
            let Some(val) = rest.strip_prefix("@scope") else {
                continue;
            };
            return match val.trim().to_ascii_lowercase().as_str() {
                "client" => ScriptScope::Client,
                "both" => ScriptScope::Both,
                _ => ScriptScope::Host,
            };
        }
        ScriptScope::Host
    }

    /// Whether a scenario with this scope should tick on the current peer.
    pub fn runs_on(self, is_client: bool) -> bool {
        match self {
            ScriptScope::Host => !is_client,
            ScriptScope::Client => is_client,
            ScriptScope::Both => true,
        }
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

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let Some(state) = driver.fsm.remove(&entity) else {
                return;
            };
            let _scope = bridge_core::WorldScope::enter(world);
            // The entity is still present, but the scene/tutor owns the
            // transition. Its final cleanup is host-authoritative, matching
            // the normal despawn teardown path below.
            bridge_core::set_script_authority(None);
            bridge_core::set_script_client_local(false);
            if state.started && state.compiled {
                let _ = driver
                    .runtime
                    .call_hook(entity, ScenarioHook::Stop, state.gid);
            }
            driver.runtime.forget(entity);
        });
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
        // 1. Snapshot (entity, doc_id, gid, source revision, parameter revision),
        //    releasing every
        //    World borrow before we execute scripts. `live` = all THIS-LANGUAGE
        //    entities (incl. paused) — drives despawn/detach teardown.
        // (entity, doc_id, gid, generation, parameter revision, maybe (source,
        // parameters, asset-id), authority). Source+parameters+id are cloned only
        // when a (re)compile is due (see below) — not every tick — since they're
        // consumed solely by `runtime.compile`.
        type CompileInput = (String, ScenarioParameters, Option<String>);
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
        )> = Vec::new();
        // A predicting client only ticks scenarios scoped to run there
        // (`Client`/`Both`); the host ticks `Host`/`Both`. Read once — constant
        // for the whole pass.
        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let scene_generation = world
            .get_resource::<ScenarioSceneGeneration>()
            .copied()
            .unwrap_or_default()
            .0;
        let live: HashSet<Entity>;
        {
            let mut q = world.query::<(
                Entity,
                &ScriptedModel,
                Option<&ScriptAuthority>,
                Option<&ScriptScope>,
            )>();
            let models: Vec<(
                Entity,
                bool,
                Option<ScriptLanguage>,
                Option<u64>,
                Option<SessionId>,
                ScriptScope,
                crate::doc::ScenarioReloadPolicy,
                u64,
            )> = q
                .iter(world)
                .map(|(e, m, auth, scope)| {
                    (
                        e,
                        m.paused,
                        m.language,
                        m.document_id,
                        auth.and_then(|a| a.0),
                        scope.copied().unwrap_or_default(),
                        m.reload_policy,
                        m.parameters_revision,
                    )
                })
                .collect();
            live = models
                .iter()
                .filter(|(_, _, l, _, _, _, _, _)| *l == Some(language))
                .map(|(e, ..)| *e)
                .collect();

            for (
                entity,
                paused,
                lang,
                doc_id,
                authority,
                scope,
                reload_policy,
                parameters_revision,
            ) in models
            {
                if paused || lang != Some(language) {
                    continue;
                }
                // Scope gate: skip (don't execute) a scenario not meant for this
                // peer. It stays in `live` above, so it is NOT torn down — just
                // idle here (it ticks on the peer it belongs to).
                if !scope.runs_on(is_client) {
                    continue;
                }
                let Some(raw) = doc_id else { continue };
                let (generation, maybe_src) = {
                    let registry = world.resource::<ScriptRegistry>();
                    let Some(host) = registry.documents.get(&DocumentId::new(raw)) else {
                        continue;
                    };
                    let doc = host.document();
                    if doc.language != language {
                        continue;
                    }
                    let generation = doc.generation;
                    // Only (re)compilation reads source/params, so clone them ONLY when a
                    // recompile is actually due (first sight or generation bump) — otherwise
                    // the multi-KB source was cloned and dropped unused every tick. This
                    // A failed compile is also an attempted revision. Do not compile it
                    // again until the author changes the document: the diagnostic is
                    // already published and the same text cannot become valid by ticking.
                    let needs_recompile = world
                        .get_resource::<ScenarioDriver<R>>()
                        .and_then(|d| d.fsm.get(&entity))
                        .is_none_or(|st| {
                            st.attempted_generation != Some(generation)
                                || st.parameters_revision != parameters_revision
                                || (reload_policy == crate::doc::ScenarioReloadPolicy::Restart
                                    && st.started
                                    && st.scene_generation != scene_generation)
                        });
                    let maybe_src = needs_recompile.then(|| {
                        let parameters = world
                            .get::<ScriptedModel>(entity)
                            .map(|model| model.parameters.clone())
                            .unwrap_or_default();
                        (doc.source.clone(), parameters, doc.asset_id.clone())
                    });
                    (generation, maybe_src)
                };
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
                ));
            }
        }

        // Drain events fired since the previous driver pass.
        // UNCONDITIONALLY, before the early-return below. `collect_script_events`
        // pushes a clone of every telemetry event into the inbox each frame; if we
        // returned without draining whenever no scenario is active (the common
        // case) `pending` would grow without bound (review H1). Dropping events
        // with no scenario to consume them is correct — there's nothing to deliver.
        // Move the batch out so script hooks can run without holding a World
        // resource borrow. The vector is recycled below after the batch has
        // been delivered; dropping it every fixed pass would force a fresh
        // allocation for the next render/control event burst.
        let mut events: Vec<TelemetryEvent> = world
            .get_resource_mut::<ScriptEventInbox>()
            .map(|mut inbox| std::mem::take(&mut inbox.pending))
            .unwrap_or_default();

        // Run if there's work OR a tracked entity vanished (needs on_stop).
        let needs_teardown = world
            .get_resource::<ScenarioDriver<R>>()
            .is_some_and(|d| d.fsm.keys().any(|e| !live.contains(e)));
        if work.is_empty() && !needs_teardown {
            // No scenario can consume this batch. Preserve the allocation for
            // the next pass, but intentionally discard the events themselves.
            events.clear();
            if let Some(mut inbox) = world.get_resource_mut::<ScriptEventInbox>() {
                inbox.pending = events;
            }
            return;
        }

        // Per-document diagnostics to publish AFTER the scope: None = OK,
        // Some(diags) = errored. Only (re)compiles + runtime errors record.
        let mut diag_updates: Vec<(u64, Option<Vec<Diagnostic>>)> = Vec::new();

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let _scope = bridge_core::WorldScope::enter(world);
            driver.runtime.maintain();
            let ScenarioDriver { runtime, fsm } = &mut *driver;

            // Everything that reaches `work` on a client passed the scope gate, so
            // it is a client-scoped scenario: restrict its `cmd()`s to the
            // client-local surface (see `bridge_core::cmd_value`). Host/standalone
            // leaves the filter off.
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
            ) in work
            {
                // Gate this entity's hook `cmd()`s against the launching session
                // (§3.4). `None` for a host-trusted launch → ungated. Covers the
                // hot-reload `on_stop` below too (still inside this iteration).
                bridge_core::set_script_authority(authority);
                let st = fsm.entry(entity).or_default();
                st.gid = gid;
                let mut recompiled = false;
                let mut compile_diag: Option<Diagnostic> = None;
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
                        let _ = runtime.call_hook(entity, ScenarioHook::Stop, gid);
                    }
                    st.started = false;
                    st.scene_generation = scene_generation;
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
                            diag_updates.push((raw, Some(vec![diag])));
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

                // First runtime error from any hook this pass.
                let mut runtime_err: Option<Diagnostic> = None;
                if !st.started {
                    st.started = true;
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Start, gid) {
                        runtime_err.get_or_insert(d);
                    }
                }
                for ev in &events {
                    if let Some(d) = runtime.deliver_event(entity, gid, ev) {
                        runtime_err.get_or_insert(d);
                    }
                }
                if run_tick {
                    if let Some(d) = runtime.call_hook(entity, ScenarioHook::Tick, gid) {
                        runtime_err.get_or_insert(d);
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
                diags.extend(runtime_err);
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
            let dead: Vec<Entity> = fsm.keys().copied().filter(|e| !live.contains(e)).collect();
            for entity in dead {
                if let Some(st) = fsm.remove(&entity) {
                    if st.started && st.compiled {
                        let _ = runtime.call_hook(entity, ScenarioHook::Stop, st.gid);
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
/// normally drains the inbox every pass. Exceeding it is a simulation fault,
/// not a reason to discard mission events silently.
pub const SCRIPT_EVENT_INBOX_CAPACITY: usize = 4096;

/// Pass-delayed inbox of `TelemetryEvent`s destined for scenario `on_event`
/// hooks. An observer ([`collect_script_events`]) clones every fired event here;
/// the driver drains it at the start of the next driver pass (the next fixed
/// simulation pass while running, or the next `Update` pass while paused).
/// Delivery remains deterministic and language-neutral: order never depends on
/// system scheduling. If producers outrun the driver, the inbox refuses new
/// events and requests a loud application exit rather than growing until the
/// simulation becomes progressively slower or drops a control edge.
#[derive(Resource, Debug)]
pub struct ScriptEventInbox {
    /// Events awaiting delivery on the next driver pass.
    pub pending: Vec<TelemetryEvent>,
    /// Whether the fixed-capacity boundary has been crossed.
    pub overflowed: bool,
    /// Number of events refused after the boundary was crossed.
    pub dropped: u64,
}

impl Default for ScriptEventInbox {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(SCRIPT_EVENT_INBOX_CAPACITY),
            overflowed: false,
            dropped: 0,
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

    /// Enqueue one event while preserving FIFO order.
    ///
    /// `false` is an explicit overflow signal. The caller owns the policy for
    /// surfacing it (the runtime observer terminates the app); no event is
    /// silently evicted from the front of the queue.
    pub fn enqueue(&mut self, event: TelemetryEvent) -> bool {
        if self.pending.len() >= SCRIPT_EVENT_INBOX_CAPACITY {
            self.overflowed = true;
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.pending.push(event);
        true
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
    mut commands: Commands,
) {
    if !gate.enabled {
        return;
    }
    if inbox.enqueue(trigger.event().clone()) {
        return;
    }
    // Send the terminal signal only once. The first rejected event is enough
    // to identify the fault; additional events are counted without producing a
    // second message or another log line.
    if inbox.dropped == 1 {
        error!(
            "[scripting] telemetry event inbox overflowed at {} events; refusing further events and stopping the application",
            SCRIPT_EVENT_INBOX_CAPACITY
        );
        commands.write_message(AppExit::error());
    }
}

#[cfg(test)]
mod tests {
    use super::{ScriptEventInbox, SCRIPT_EVENT_INBOX_CAPACITY};
    use lunco_telemetry_core::{Severity, TelemetryEvent, TelemetryValue};

    fn event(index: usize) -> TelemetryEvent {
        TelemetryEvent {
            name: format!("event:{index}"),
            source: 0,
            severity: Severity::Info,
            data: TelemetryValue::F64(index as f64),
            timestamp: index as f64,
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
}
