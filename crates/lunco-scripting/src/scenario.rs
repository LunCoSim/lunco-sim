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
use lunco_core::TelemetryEvent;
use lunco_doc::{Diagnostic, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;

use crate::bridge_core::{self, ValueBuilder};
use crate::doc::{ScriptLanguage, ScriptedModel};
use crate::ScriptRegistry;

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
        // (entity, doc_id, gid, generation, source, params-json).
        let mut work: Vec<(Entity, u64, i64, u64, String, String)> = Vec::new();
        let live: HashSet<Entity>;
        {
            let mut q = world.query::<(Entity, &ScriptedModel)>();
            let models: Vec<(Entity, bool, Option<ScriptLanguage>, Option<u64>)> = q
                .iter(world)
                .map(|(e, m)| (e, m.paused, m.language, m.document_id))
                .collect();
            live = models
                .iter()
                .filter(|(_, _, l, _)| *l == Some(language))
                .map(|(e, ..)| *e)
                .collect();

            for (entity, paused, lang, doc_id) in models {
                if paused || lang != Some(language) {
                    continue;
                }
                let Some(raw) = doc_id else { continue };
                let (generation, source, params) = {
                    let registry = world.resource::<ScriptRegistry>();
                    let Some(host) = registry.documents.get(&DocumentId::new(raw)) else {
                        continue;
                    };
                    let doc = host.document();
                    if doc.language != language {
                        continue;
                    }
                    (doc.generation, doc.source.clone(), doc.params.clone())
                };
                let gid = world
                    .resource::<ApiEntityRegistry>()
                    .api_id_for(entity)
                    .map(|g| g.get() as i64)
                    .unwrap_or(-1);
                work.push((entity, raw, gid, generation, source, params));
            }
        }

        // Run if there's work OR a tracked entity vanished (needs on_stop).
        let needs_teardown = world
            .get_resource::<ScenarioDriver<R>>()
            .is_some_and(|d| d.fsm.keys().any(|e| !live.contains(e)));
        if work.is_empty() && !needs_teardown {
            return;
        }

        // Events fired since last tick (frame-delayed actor-model delivery).
        let events: Vec<TelemetryEvent> = world
            .get_resource_mut::<ScriptEventInbox>()
            .map(|mut inbox| std::mem::take(&mut inbox.pending))
            .unwrap_or_default();

        // Per-document diagnostics to publish AFTER the scope: None = OK,
        // Some(diags) = errored. Only (re)compiles + runtime errors record.
        let mut diag_updates: Vec<(u64, Option<Vec<Diagnostic>>)> = Vec::new();

        world.resource_scope(|world, mut driver: Mut<ScenarioDriver<R>>| {
            let _scope = bridge_core::WorldScope::enter(world);
            driver.runtime.maintain();
            let ScenarioDriver { runtime, fsm } = &mut *driver;

            for (entity, raw, gid, generation, source, params) in work {
                let st = fsm.entry(entity).or_default();
                st.gid = gid;
                let mut recompiled = false;
                let mut compile_diag: Option<Diagnostic> = None;

                // (Re)compile on first sight or generation bump.
                if !st.compiled || st.generation != generation {
                    recompiled = true;
                    // Hot-reload teardown: the OUTGOING program cleans up first.
                    if st.started && st.compiled {
                        let _ = runtime.call_hook(entity, ScenarioHook::Stop, gid);
                    }
                    st.started = false;
                    match runtime.compile(entity, &source, &params) {
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

                // Publish status: errors → Error; else OK only when (re)compiled.
                let mut diags = Vec::new();
                diags.extend(compile_diag);
                diags.extend(runtime_err);
                if !diags.is_empty() {
                    diag_updates.push((raw, Some(diags)));
                } else if recompiled {
                    diag_updates.push((raw, None));
                }
            }

            // Teardown: any tracked entity no longer live (despawned / detached)
            // gets a final on_stop, then its state is dropped.
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
                    Some(diags) => store.set_error(DocumentId::new(raw), diags),
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
pub fn collect_script_events(trigger: On<TelemetryEvent>, mut inbox: ResMut<ScriptEventInbox>) {
    inbox.pending.push(trigger.event().clone());
}
