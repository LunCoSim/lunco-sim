//! Language-neutral scenario lifecycle — the runtime-agnostic driver.
//!
//! A *scenario* is a persistent per-entity program with lifecycle hooks
//! (`on_start` / `on_tick` / `on_event` / `on_stop`). EVERYTHING about *when*
//! those fire — scheduling, hot-reload on a generation bump, `on_event`
//! frame-delayed delivery, pause, despawn/detach teardown, diagnostics
//! reporting — is identical across languages. That orchestration lives here, in
//! [`ScenarioDriver`], free of any interpreter type.
//!
//! The only language-specific part is the *mechanics*: turning source into a
//! compiled program and calling a hook. That's the [`ScenarioRuntime`] trait —
//! one impl per language (rhai today; see the Python TODO below). This mirrors
//! the [`crate::bridge_core`] split: neutral core + thin per-language binding.
//!
//! TODO(python scenarios): give Python lifecycle parity by implementing
//! `ScenarioRuntime` for a `PythonScenarioRuntime` (compile a module per entity;
//! map hooks to module-level `on_start`/`on_tick`/`on_stop`/`on_event(evt)`
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
use lunco_core::{SessionId, TelemetryEvent};
use lunco_doc::{Diagnostic, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;

use crate::bridge_core::{self, ValueBuilder};
use crate::doc::{ScriptLanguage, ScriptedModel};
use crate::ScriptRegistry;

/// The session a scenario acts on behalf of — captured at attach from the wire
/// origin ([`lunco_core::session::SyncApplyGuard`]). `Some` only for a scenario
/// launched by a *remote* networked session; the driver sets it as the `cmd()`
/// authority for that entity's hooks, so a remote script can't exceed its
/// submitter's authority (design §3.4). Absent / `None` ⇒ host-trusted launch
/// (local, standalone, USD-embedded) → ungated, matching the open-by-default
/// substrate (and side-stepping the default-deny `SessionRbac` would apply where
/// no sessions are registered).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ScriptAuthority(pub Option<SessionId>);

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
            let Some(rest) = t.strip_prefix("//") else { continue };
            let rest = rest.trim_start().trim_start_matches('!').trim_start();
            let Some(val) = rest.strip_prefix("@scope") else { continue };
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
/// [`ValueBuilder`] — JSON appears only when the caller passes a
/// [`bridge_core::JsonBuilder`] at an API seam, never as an internal transform.
pub struct ScenarioSnapshot<V> {
    /// The scenario's per-entity state object (rhai `this`, future Python state),
    /// built into `V` by the caller's builder. The builder's `unit` if none.
    pub state: V,
    /// Which lifecycle hooks the compiled program actually defines
    /// (`on_start` / `on_tick` / `on_event` / `on_stop`).
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
    /// Lifecycle hooks the program defines.
    pub hooks: Vec<String>,
}

/// A language backend that runs persistent per-entity scenarios. Supplies ONLY
/// the language mechanics — [`ScenarioDriver`] owns all lifecycle policy.
///
/// Each method is keyed by the host `entity`; the impl owns the per-entity
/// compiled state internally. `self_gid` is the host's `GlobalEntityId` (the
/// `self` a hook receives), passed in so the impl never resolves it.
///
// TODO(hooks): evaluate folding this onto the `lunco-hooks` registry (the
// language-neutral internal-hook substrate that backs `MergePolicy`,
// `rbac.authorize`, and `ScriptedDriveKernel`). NOT done yet — deliberately —
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
    /// `on_stop` has already been called before this. `params` is the scenario's
    /// parameter JSON-object string (empty for none) — the backend exposes it to
    /// the script (rhai: a `params` constant) so one scenario is reusable.
    fn compile(&mut self, entity: Entity, source: &str, params: &str) -> CompileOutcome;

    /// Call a lifecycle hook for `entity` — a no-op if the scenario doesn't
    /// define it or has no compiled program. Returns a runtime-error diagnostic
    /// if the hook ran and failed.
    fn call_hook(&mut self, entity: Entity, hook: ScenarioHook, self_gid: i64)
        -> Option<Diagnostic>;

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
}

/// Neutral per-entity lifecycle bookkeeping — the FSM the driver owns. Kept
/// separate from the backend's compiled state so the policy stays language-free.
struct Fsm {
    /// `ScriptDocument.generation` the current program was compiled from.
    generation: u64,
    /// Whether `on_start` has run for the current program.
    started: bool,
    /// Whether the backend currently holds a compiled program for this entity.
    compiled: bool,
    /// Last-known host gid — so `on_stop` has a meaningful `self` after despawn.
    gid: i64,
}

impl Default for Fsm {
    fn default() -> Self {
        Self { generation: 0, started: false, compiled: false, gid: -1 }
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
        Self { runtime: R::default(), fsm: HashMap::new() }
    }
}

impl<R: ScenarioRuntime> ScenarioDriver<R> {
    /// Exclusive-system body: drive every non-paused `ScriptedModel { language }`
    /// through its lifecycle against the live World. Fully language-neutral — only
    /// the `R` trait calls touch the interpreter.
    pub fn run(world: &mut World, language: ScriptLanguage) {
        // 1. Snapshot (entity, doc_id, gid, generation, source), releasing every
        //    World borrow before we execute scripts. `live` = all THIS-LANGUAGE
        //    entities (incl. paused) — drives despawn/detach teardown.
        // (entity, doc_id, gid, generation, maybe (source, params-json), authority).
        // Source+params are cloned only when a (re)compile is due (see below) — not
        // every tick — since they're consumed solely by `runtime.compile`.
        let mut work: Vec<(Entity, u64, i64, u64, Option<(String, String)>, Option<SessionId>)> =
            Vec::new();
        // A predicting client only ticks scenarios scoped to run there
        // (`Client`/`Both`); the host ticks `Host`/`Both`. Read once — constant
        // for the whole pass.
        let is_client = matches!(
            world.get_resource::<lunco_core::NetworkRole>(),
            Some(lunco_core::NetworkRole::Client)
        );
        let live: HashSet<Entity>;
        {
            let mut q =
                world.query::<(Entity, &ScriptedModel, Option<&ScriptAuthority>, Option<&ScriptScope>)>();
            let models: Vec<(
                Entity,
                bool,
                Option<ScriptLanguage>,
                Option<u64>,
                Option<SessionId>,
                ScriptScope,
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
                    )
                })
                .collect();
            live = models
                .iter()
                .filter(|(_, _, l, _, _, _)| *l == Some(language))
                .map(|(e, ..)| *e)
                .collect();

            for (entity, paused, lang, doc_id, authority, scope) in models {
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
                    // predicate mirrors the loop's `!compiled || generation-changed` gate.
                    let needs_recompile = world
                        .get_resource::<ScenarioDriver<R>>()
                        .and_then(|d| d.fsm.get(&entity))
                        .map_or(true, |st| !st.compiled || st.generation != generation);
                    let maybe_src =
                        needs_recompile.then(|| (doc.source.clone(), doc.params.clone()));
                    (generation, maybe_src)
                };
                let gid = world
                    .resource::<ApiEntityRegistry>()
                    .api_id_for(entity)
                    .map(|g| g.get() as i64)
                    .unwrap_or(-1);
                work.push((entity, raw, gid, generation, maybe_src, authority));
            }
        }

        // Drain events fired since last tick (frame-delayed actor-model delivery)
        // UNCONDITIONALLY, before the early-return below. `collect_script_events`
        // pushes a clone of every telemetry event into the inbox each frame; if we
        // returned without draining whenever no scenario is active (the common
        // case) `pending` would grow without bound (review H1). Dropping events
        // with no scenario to consume them is correct — there's nothing to deliver.
        let events: Vec<TelemetryEvent> = world
            .get_resource_mut::<ScriptEventInbox>()
            .map(|mut inbox| std::mem::take(&mut inbox.pending))
            .unwrap_or_default();

        // Run if there's work OR a tracked entity vanished (needs on_stop).
        let needs_teardown = world
            .get_resource::<ScenarioDriver<R>>()
            .is_some_and(|d| d.fsm.keys().any(|e| !live.contains(e)));
        if work.is_empty() && !needs_teardown {
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
            // client-local surface (see `bridge_core::cmd_raw`). Host/standalone
            // leaves the filter off.
            bridge_core::set_script_client_local(is_client);

            for (entity, raw, gid, generation, maybe_src, authority) in work {
                // Gate this entity's hook `cmd()`s against the launching session
                // (§3.4). `None` for a host-trusted launch → ungated. Covers the
                // hot-reload `on_stop` below too (still inside this iteration).
                bridge_core::set_script_authority(authority);
                let st = fsm.entry(entity).or_default();
                st.gid = gid;
                let mut recompiled = false;
                let mut compile_diag: Option<Diagnostic> = None;

                // (Re)compile on first sight or generation bump. Phase 1 provides
                // `maybe_src` exactly when this is due (Some ⟺ recompile), so the
                // presence of the source IS the gate — no per-tick source clone.
                if let Some((source, params)) = &maybe_src {
                    recompiled = true;
                    // Hot-reload teardown: the OUTGOING program cleans up first.
                    if st.started && st.compiled {
                        let _ = runtime.call_hook(entity, ScenarioHook::Stop, gid);
                    }
                    st.started = false;
                    match runtime.compile(entity, source, params) {
                        CompileOutcome::Failed(diag) => {
                            st.compiled = false;
                            st.generation = generation;
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

                // First runtime error from any hook this tick.
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
                if let Some(d) = runtime.call_hook(entity, ScenarioHook::Tick, gid) {
                    runtime_err.get_or_insert(d);
                }

                // Authoritative commands a client-scoped scenario tried (and was
                // denied) this pass — collected in `bridge_core::cmd_raw`. Surface
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
    }

    /// Live introspection of `entity`'s scenario: the neutral FSM state joined
    /// with the backend's [`ScenarioSnapshot`]. `None` if the driver isn't
    /// tracking this entity (no scenario, or it hasn't been driven yet). Powers
    /// the `ScriptInspect` query — the same data for any language `R`. `builder`
    /// chooses the value format (a [`bridge_core::JsonBuilder`] for the API).
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
// (every scenario sees every TelemetryEvent next tick). Two follow-ups, only one
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

/// Frame-delayed inbox of `TelemetryEvent`s destined for scenario `on_event`
/// hooks. An observer ([`collect_script_events`]) clones every fired event here;
/// the driver drains it at the start of the next tick, so an event emitted on
/// tick N is delivered on tick N+1 (deterministic actor model — order never
/// depends on system scheduling). Language-neutral: shared by every backend.
#[derive(Resource, Default)]
pub struct ScriptEventInbox {
    /// Events awaiting delivery on the next driver pass.
    pub pending: Vec<TelemetryEvent>,
}

/// Observer: mirror every fired `TelemetryEvent` into the scenario inbox. Reuses
/// the existing telemetry bus — scenarios are just another subscriber.
///
/// Runs on EVERY peer. It collected only on the host originally, back when the
/// driver was gated off on a predicting client (`scripts_run_here`) so nothing
/// drained the inbox and `pending` would grow without bound (review H1). That
/// gate is gone: client-scoped scenarios (`// @scope client`) now tick on the
/// client, and [`ScenarioDriver::run`] drains the inbox UNCONDITIONALLY every
/// pass (even with no active scenario), so there is no unbounded growth. A client
/// scenario's `on_event` therefore sees the events that fire *on the client*
/// (local input, client-side emits); host-authoritative game events reach it only
/// once they are explicitly replicated — a scoped follow-up, not this collector's
/// concern.
pub fn collect_script_events(
    trigger: On<TelemetryEvent>,
    mut inbox: ResMut<ScriptEventInbox>,
) {
    inbox.pending.push(trigger.event().clone());
}
