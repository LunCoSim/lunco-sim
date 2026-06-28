//! World-bound rhai execution — the bridge that lets scripts read ECS state and
//! drive the simulation.
//!
//! # The three verbs
//!
//! A script gets exactly three ways to touch the world, mirroring the
//! engine-wide channel model:
//!
//! - **`cmd(name, #{params})`** — *write*. Triggers an [`ApiCommandEvent`], the
//!   SAME entry point the HTTP API / MCP use. It dispatches by name over the
//!   reflection registry that the `#[Command]` macro auto-populates, so EVERY
//!   command (twin / usd / modelica / cosim / rover / future) is reachable with
//!   **zero per-command binding** — add a `#[Command]`, scripts see it for free.
//!   Writes go through commands so they replicate + pass networking RBAC exactly
//!   like any other command; the script runs host-authoritative (same trust as
//!   a local API client).
//! - **`world_pos(id)` / `get(id, "Comp.field")`** — *read*. Synchronous,
//!   reflection-based reads of live ECS state. `world_pos` is the one
//!   float-origin-correct (big_space) position read; `get` is the generic
//!   component-field reader (the finished `EntityProxy`).
//! - events (`emit`/`on_event`) land in P2 — reusing `TelemetryEvent`.
//!
//! # Execution context
//!
//! Reads must be synchronous, so rhai runs inside a `&mut World` (an exclusive
//! system or `World`-queue closure). The registered functions reach that World
//! through a scoped thread-local pointer ([`WorldScope`]). This is the standard
//! ECS-scripting pattern: the pointer is valid only for the duration of one
//! `eval_with_world` call, our functions never hold the `&mut` across a nested
//! rhai callback, and execution is single-threaded (FixedUpdate / wasm), so no
//! aliasing occurs.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::system::SystemState;
use bevy::math::DVec3;
use big_space::prelude::*;
use std::cell::Cell;

use lunco_core::{
    coords, Ack, CelestialClock, CommandResults, GlobalEntityId, OpId, Severity, SimTick,
    TelemetryEvent, TelemetryValue,
};
use lunco_api::executor::ApiCommandEvent;
use lunco_api::registry::ApiEntityRegistry;

use rhai::{Dynamic, Engine, ImmutableString, Map, AST};

use crate::doc::{ScriptLanguage, ScriptedModel};
use crate::ScriptRegistry;
use lunco_doc::DocumentId;

// ── Scoped World access ────────────────────────────────────────────────────

thread_local! {
    /// Raw pointer to the World currently being scripted. Non-null only while a
    /// [`WorldScope`] guard is alive (i.e. inside `eval_with_world`).
    static WORLD_PTR: Cell<*mut World> = const { Cell::new(std::ptr::null_mut()) };
}

/// RAII guard that publishes a `&mut World` to the thread-local for the
/// lifetime of a script evaluation, and clears it on drop (even on panic).
struct WorldScope;

impl WorldScope {
    fn enter(world: &mut World) -> Self {
        WORLD_PTR.with(|p| p.set(world as *mut World));
        WorldScope
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        WORLD_PTR.with(|p| p.set(std::ptr::null_mut()));
    }
}

/// Run `f` with the scoped World, or return `None` if called outside a script
/// evaluation (no active scope). SAFETY: the pointer is only ever set to a live
/// `&mut World` borrow held by `eval_with_world` for the duration of the call;
/// registered functions run synchronously and never re-enter while a borrow is
/// outstanding, so the reconstructed `&mut` is unique.
fn with_world<R>(f: impl FnOnce(&mut World) -> R) -> Option<R> {
    WORLD_PTR.with(|p| {
        let ptr = p.get();
        if ptr.is_null() {
            None
        } else {
            Some(f(unsafe { &mut *ptr }))
        }
    })
}

// ── Engine construction ────────────────────────────────────────────────────

/// Ergonomic policy wrappers (drive/distance/arrived/...), authored in rhai and
/// embedded at compile time so they're available with zero IO on every target
/// (incl. wasm). Edit `rhai/prelude.rhai` — no Rust change needed for new helpers.
const PRELUDE: &str = include_str!("../rhai/prelude.rhai");

/// Build a rhai [`Engine`] with the World-bridge verbs registered, the embedded
/// prelude loaded as a global module, and the same sandbox caps as the one-shot
/// backend.
pub fn build_world_engine() -> Engine {
    let mut engine = Engine::new();

    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);

    // cmd(name, #{params}) -> i64 (the command id, for polling). Routes through
    // ApiCommandEvent so it inherits macro-reflected dispatch, GlobalEntityId
    // resolution, and result recording. Returns -1 if no World is in scope.
    engine.register_fn("cmd", |name: ImmutableString, params: Map| -> i64 {
        cmd_impl(name.as_str(), Dynamic::from_map(params))
    });
    // cmd(name) -> i64 — convenience for unit/all-defaulted commands.
    engine.register_fn("cmd", |name: ImmutableString| -> i64 {
        cmd_impl(name.as_str(), Dynamic::from_map(Map::new()))
    });

    // world_pos(id) -> [x, y, z] in DVec3 absolute world space, or () on miss.
    engine.register_fn("world_pos", |id: i64| -> Dynamic {
        match world_pos_impl(id as u64) {
            Some(v) => {
                let arr: rhai::Array = vec![
                    Dynamic::from_float(v.x),
                    Dynamic::from_float(v.y),
                    Dynamic::from_float(v.z),
                ];
                Dynamic::from_array(arr)
            }
            None => Dynamic::UNIT,
        }
    });

    // world_forward(id) -> [x, y, z] unit heading in world space, or ().
    // The ONE read rhai can't derive itself (world orientation needs the ECS
    // float-origin hierarchy). All steering MATH stays in rhai (the prelude);
    // this just exposes the heading vector, like world_pos exposes position.
    engine.register_fn("world_forward", |id: i64| -> Dynamic {
        match world_forward_impl(id as u64) {
            Some(v) => {
                let arr: rhai::Array = vec![
                    Dynamic::from_float(v.x),
                    Dynamic::from_float(v.y),
                    Dynamic::from_float(v.z),
                ];
                Dynamic::from_array(arr)
            }
            None => Dynamic::UNIT,
        }
    });

    // get(id, "Component.field") -> Dynamic (f64/i64/bool/string) or ().
    // The generic reflection read — the finished EntityProxy.
    engine.register_fn("get", |id: i64, path: ImmutableString| -> Dynamic {
        get_field_impl(id as u64, path.as_str()).unwrap_or(Dynamic::UNIT)
    });

    // list_entities() -> [#{ id, name }] for every registered GlobalEntityId.
    engine.register_fn("list_entities", list_entities_impl);

    // find(name) -> id (i64), or -1 if no entity has that Name.
    engine.register_fn("find", |name: ImmutableString| -> i64 {
        find_impl(name.as_str())
    });

    // emit(name, value) -> bool — fire a TelemetryEvent on the shared bus
    // (reused, not reinvented): existing API-subscription + log observers see
    // it immediately, and scripts receive it next tick via on_event. `value`
    // may be float / int / bool / string.
    engine.register_fn("emit", |name: ImmutableString, value: Dynamic| -> bool {
        emit_impl(name.as_str(), value)
    });
    // emit(name) — a bare pulse (no payload).
    engine.register_fn("emit", |name: ImmutableString| -> bool {
        emit_impl(name.as_str(), Dynamic::UNIT)
    });

    // sim_tick() -> i64 — current FixedUpdate tick.
    engine.register_fn("sim_tick", || -> i64 {
        with_world(|w| w.get_resource::<SimTick>().map(|t| t.0 as i64))
            .flatten()
            .unwrap_or(0)
    });

    // Load the embedded prelude as a global module so its helpers are callable
    // unqualified (e.g. `drive(r, 1.0, 0.0)`). Compiled against the same engine
    // so the wrappers can reach the native verbs above.
    match engine.compile(PRELUDE) {
        Ok(ast) => match rhai::Module::eval_ast_as_new(rhai::Scope::new(), &ast, &engine) {
            Ok(module) => {
                engine.register_global_module(module.into());
            }
            Err(e) => error!("[rhai] prelude module build failed: {e}"),
        },
        Err(e) => error!("[rhai] prelude compile failed: {e}"),
    }

    engine
}

// ── Verb implementations ───────────────────────────────────────────────────

fn cmd_impl(name: &str, params: Dynamic) -> i64 {
    // rhai Map -> serde_json::Value (rhai `serde` feature drives Serialize).
    let params_json = rhai::serde::from_dynamic::<serde_json::Value>(&params)
        .unwrap_or(serde_json::Value::Null);

    let id = OpId::new().0;
    let triggered = with_world(|world| {
        world.trigger(ApiCommandEvent {
            command: name.to_string(),
            params: params_json,
            id,
        });
    });
    if triggered.is_some() {
        id as i64
    } else {
        -1
    }
}

/// Map a rhai value to the engine-wide [`TelemetryValue`] (the YAMCS-aligned
/// payload type). Floats/ints/bools map directly; everything else stringifies.
fn dynamic_to_telemetry_value(value: &Dynamic) -> TelemetryValue {
    if value.is_unit() {
        TelemetryValue::Bool(true) // a bare pulse
    } else if let Ok(f) = value.as_float() {
        TelemetryValue::F64(f)
    } else if let Ok(i) = value.as_int() {
        TelemetryValue::I64(i)
    } else if let Ok(b) = value.as_bool() {
        TelemetryValue::Bool(b)
    } else {
        TelemetryValue::String(value.to_string())
    }
}

fn emit_impl(name: &str, value: Dynamic) -> bool {
    with_world(|world| {
        let timestamp = world
            .get_resource::<CelestialClock>()
            .map(|c| c.epoch)
            .unwrap_or(0.0);
        world.trigger(TelemetryEvent {
            name: name.to_string(),
            severity: Severity::Info,
            data: dynamic_to_telemetry_value(&value),
            timestamp,
        });
    })
    .is_some()
}

/// Convert a [`TelemetryValue`] back into a rhai [`Dynamic`] for `on_event`.
fn telemetry_value_to_dynamic(v: &TelemetryValue) -> Dynamic {
    match v {
        TelemetryValue::F64(x) => Dynamic::from_float(*x),
        TelemetryValue::I64(x) => Dynamic::from_int(*x),
        TelemetryValue::Bool(x) => Dynamic::from_bool(*x),
        TelemetryValue::String(x) => Dynamic::from(x.clone()),
    }
}

/// Build the `#{ name, value, severity, timestamp }` map passed to `on_event`.
fn event_to_map(ev: &TelemetryEvent) -> Map {
    let mut m = Map::new();
    m.insert("name".into(), Dynamic::from(ev.name.clone()));
    m.insert("value".into(), telemetry_value_to_dynamic(&ev.data));
    m.insert("severity".into(), Dynamic::from(format!("{:?}", ev.severity)));
    m.insert("timestamp".into(), Dynamic::from_float(ev.timestamp));
    m
}

fn world_pos_impl(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(world);
        let (q_parents, q_grids, q_spatial) = state.get(world);
        coords::world_position(entity, &q_parents, &q_grids, &q_spatial)
    })
    .flatten()
}

fn world_forward_impl(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        // GlobalTransform's rotation is true world orientation (the float-origin
        // offset only affects translation), so its forward vector is valid.
        let gt = world.get::<GlobalTransform>(entity)?;
        let f = gt.forward();
        Some(DVec3::new(f.x as f64, f.y as f64, f.z as f64))
    })
    .flatten()
}

fn get_field_impl(gid: u64, path: &str) -> Option<Dynamic> {
    // Split "Component.field.sub" into the component short name and the reflect
    // sub-path (".field.sub"). A bare "Component" reads the whole component as
    // its Debug string (rarely useful, but harmless).
    let (comp, sub) = match path.split_once('.') {
        Some((c, rest)) => (c, format!(".{rest}")),
        None => (path, String::new()),
    };

    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg.get_with_short_type_path(comp)?;
        let reflect_component = registration.data::<ReflectComponent>()?;
        let entity_ref = world.get_entity(entity).ok()?;
        let reflected = reflect_component.reflect(entity_ref)?;

        let field: &dyn bevy::reflect::PartialReflect = if sub.is_empty() {
            reflected.as_partial_reflect()
        } else {
            reflected.reflect_path(sub.as_str()).ok()?
        };
        dynamic_from_reflect(field)
    })
    .flatten()
}

fn list_entities_impl() -> rhai::Array {
    with_world(|world| {
        let pairs = world.resource::<ApiEntityRegistry>().entities();
        pairs
            .into_iter()
            .map(|(gid, entity)| {
                let mut m = Map::new();
                m.insert("id".into(), Dynamic::from_int(gid.get() as i64));
                let name = world
                    .get::<Name>(entity)
                    .map(|n| n.as_str().to_string())
                    .unwrap_or_default();
                m.insert("name".into(), Dynamic::from(name));
                Dynamic::from_map(m)
            })
            .collect()
    })
    .unwrap_or_default()
}

fn find_impl(name: &str) -> i64 {
    with_world(|world| {
        let pairs = world.resource::<ApiEntityRegistry>().entities();
        for (gid, entity) in pairs {
            if world.get::<Name>(entity).map(|n| n.as_str()) == Some(name) {
                return gid.get() as i64;
            }
        }
        -1
    })
    .unwrap_or(-1)
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn resolve_entity(world: &World, gid: u64) -> Option<Entity> {
    world
        .get_resource::<ApiEntityRegistry>()?
        .resolve(&GlobalEntityId::from_raw(gid))
}

/// Best-effort conversion of a reflected scalar into a rhai [`Dynamic`].
/// Covers the common numeric / bool / string leaf types; anything else falls
/// back to its `Debug` string so the script at least sees a value.
fn dynamic_from_reflect(value: &dyn bevy::reflect::PartialReflect) -> Option<Dynamic> {
    let any = value.try_as_reflect()?.as_any();
    if let Some(v) = any.downcast_ref::<f64>() {
        return Some(Dynamic::from_float(*v));
    }
    if let Some(v) = any.downcast_ref::<f32>() {
        return Some(Dynamic::from_float(*v as f64));
    }
    if let Some(v) = any.downcast_ref::<i64>() {
        return Some(Dynamic::from_int(*v));
    }
    if let Some(v) = any.downcast_ref::<i32>() {
        return Some(Dynamic::from_int(*v as i64));
    }
    if let Some(v) = any.downcast_ref::<u32>() {
        return Some(Dynamic::from_int(*v as i64));
    }
    if let Some(v) = any.downcast_ref::<u64>() {
        return Some(Dynamic::from_int(*v as i64));
    }
    if let Some(v) = any.downcast_ref::<bool>() {
        return Some(Dynamic::from_bool(*v));
    }
    if let Some(v) = any.downcast_ref::<String>() {
        return Some(Dynamic::from(v.clone()));
    }
    Some(Dynamic::from(format!("{value:?}")))
}

// ── Persistent per-entity scenario runtime (P2) ────────────────────────────
//
// A `ScriptedModel { language: Rhai }` runs its `ScriptDocument` as a
// persistent program with lifecycle hooks, NOT a one-shot snippet:
//
//   on_start(self)   — called once after (re)compile; `self` is the host gid
//   on_tick(self)    — called every FixedUpdate
//
// State persists between ticks in a per-entity `Scope` (top-level `let`s survive),
// and the same compiled `AST` is reused until the document's `generation` bumps
// (hot-reload). One shared `Engine` carries the World-bridge verbs, so a hook
// can `cmd()` / `world_pos()` / `get()` exactly like a one-shot script.

/// Per-entity compiled program + live state.
struct ModelState {
    /// `None` if the last compile failed (kept so we don't recompile every tick).
    ast: Option<AST>,
    /// Holds top-level `const` globals (visible to hooks); populated by running
    /// the AST body once at compile.
    scope: rhai::Scope<'static>,
    /// Per-entity mutable state, bound as `this` in every hook. rhai functions
    /// are pure — they can't see top-level `let`s — so persistent tick-to-tick
    /// state lives here as an object map (`this.foo = ...`), not in the scope.
    this: Dynamic,
    /// `ScriptDocument.generation` this state was compiled from; a mismatch
    /// triggers recompile (hot-reload).
    generation: u64,
    /// Whether `on_start` has run for the current compile.
    started: bool,
}

/// Shared rhai runtime for all scripted entities: one bridge-enabled engine +
/// a per-entity [`ModelState`] cache.
#[derive(Resource)]
pub struct RhaiModelRuntime {
    engine: Engine,
    states: std::collections::HashMap<Entity, ModelState>,
}

impl Default for RhaiModelRuntime {
    fn default() -> Self {
        let mut engine = build_world_engine();
        engine.on_print(|s| info!("[rhai] {s}"));
        Self {
            engine,
            states: std::collections::HashMap::new(),
        }
    }
}

/// Frame-delayed inbox of TelemetryEvents destined for script `on_event`
/// hooks. An observer ([`collect_script_events`]) clones every fired
/// `TelemetryEvent` here; [`tick_rhai_models`] drains it at the start of the
/// next tick, so an event emitted on tick N is delivered on tick N+1
/// (deterministic actor model — order never depends on system scheduling).
#[derive(Resource, Default)]
pub struct ScriptEventInbox {
    pub pending: Vec<TelemetryEvent>,
}

/// Observer: mirror every fired `TelemetryEvent` into the script inbox. Reuses
/// the existing telemetry bus — scripts are just another subscriber.
pub fn collect_script_events(trigger: On<TelemetryEvent>, mut inbox: ResMut<ScriptEventInbox>) {
    inbox.pending.push(trigger.event().clone());
}

/// Call a one-arg hook (`fn name(self)`) if the AST defines it, binding `this`
/// to the entity's persistent state map. `eval_ast=false` so only the function
/// runs (top-level already ran at compile); `rewind_scope=false` keeps the
/// `const` globals available across calls. Logs any error.
fn call_hook(
    engine: &Engine,
    scope: &mut rhai::Scope,
    ast: &AST,
    name: &str,
    self_id: i64,
    this: &mut Dynamic,
) {
    let present = ast
        .iter_functions()
        .any(|f| f.name == name && f.params.len() == 1);
    if !present {
        return;
    }
    let mut args = [Dynamic::from_int(self_id)];
    if let Err(e) = engine.call_fn_raw(scope, ast, false, false, name, Some(this), &mut args) {
        error!("[rhai] {name}() failed: {e}");
    }
}

/// Call the two-arg event hook (`fn on_event(self, evt)`) if defined, binding
/// `this`. `evt` is the `#{name,value,...}` map.
fn call_event_hook(
    engine: &Engine,
    scope: &mut rhai::Scope,
    ast: &AST,
    self_id: i64,
    this: &mut Dynamic,
    evt: Map,
) {
    let present = ast
        .iter_functions()
        .any(|f| f.name == "on_event" && f.params.len() == 2);
    if !present {
        return;
    }
    let mut args = [Dynamic::from_int(self_id), Dynamic::from_map(evt)];
    if let Err(e) = engine.call_fn_raw(scope, ast, false, false, "on_event", Some(this), &mut args) {
        error!("[rhai] on_event() failed: {e}");
    }
}

/// Exclusive system (FixedUpdate): drive every `ScriptedModel{Rhai}` through its
/// lifecycle hooks against the live World.
pub fn tick_rhai_models(world: &mut World) {
    // 1. Snapshot the rhai models (entity + gid + doc generation + source),
    //    releasing every World borrow before we execute scripts.
    let mut work: Vec<(Entity, i64, u64, String)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &ScriptedModel)>();
        let models: Vec<(Entity, bool, Option<ScriptLanguage>, Option<u64>)> = q
            .iter(world)
            .map(|(e, m)| (e, m.paused, m.language, m.document_id))
            .collect();

        for (entity, paused, lang, doc_id) in models {
            if paused || lang != Some(ScriptLanguage::Rhai) {
                continue;
            }
            let Some(raw) = doc_id else { continue };
            let (generation, source) = {
                let registry = world.resource::<ScriptRegistry>();
                let Some(host) = registry.documents.get(&DocumentId::new(raw)) else {
                    continue;
                };
                let doc = host.document();
                if doc.language != ScriptLanguage::Rhai {
                    continue;
                }
                (doc.generation, doc.source.clone())
            };
            let gid = world
                .resource::<ApiEntityRegistry>()
                .api_id_for(entity)
                .map(|g| g.get() as i64)
                .unwrap_or(-1);
            work.push((entity, gid, generation, source));
        }
    }
    if work.is_empty() {
        return;
    }

    // 2. Run the hooks. `resource_scope` lets us hold the runtime (engine +
    //    states) AND `&mut World` at once; `WorldScope` publishes the World so
    //    the bridge verbs work inside the hooks.
    // Snapshot the events fired since the last tick (frame-delayed delivery).
    let events: Vec<TelemetryEvent> = world
        .get_resource_mut::<ScriptEventInbox>()
        .map(|mut inbox| std::mem::take(&mut inbox.pending))
        .unwrap_or_default();

    world.resource_scope(|world, mut runtime: Mut<RhaiModelRuntime>| {
        let _scope = WorldScope::enter(world);
        let RhaiModelRuntime { engine, states } = &mut *runtime;

        for (entity, gid, generation, source) in work {
            let state = states.entry(entity).or_insert_with(|| ModelState {
                ast: None,
                scope: rhai::Scope::new(),
                this: Dynamic::from_map(Map::new()),
                generation: u64::MAX,
                started: false,
            });

            // (Re)compile on first sight or generation bump, then run top-level
            // once to initialise the persistent scope (top-level `const`s).
            if state.ast.is_none() || state.generation != generation {
                state.generation = generation;
                state.started = false;
                state.scope = rhai::Scope::new();
                state.this = Dynamic::from_map(Map::new());
                match engine.compile(&source) {
                    Ok(ast) => {
                        if let Err(e) = engine.run_ast_with_scope(&mut state.scope, &ast) {
                            error!("[rhai] entity {entity:?} top-level failed: {e}");
                        }
                        state.ast = Some(ast);
                    }
                    Err(e) => {
                        error!("[rhai] entity {entity:?} compile error: {e}");
                        state.ast = None;
                        continue;
                    }
                }
            }

            let run_start = state.ast.is_some() && !state.started;
            if run_start {
                state.started = true;
            }
            if let Some(ast) = &state.ast {
                if run_start {
                    call_hook(engine, &mut state.scope, ast, "on_start", gid, &mut state.this);
                }
                // Deliver this tick's events, then advance the scenario.
                for ev in &events {
                    call_event_hook(
                        engine,
                        &mut state.scope,
                        ast,
                        gid,
                        &mut state.this,
                        event_to_map(ev),
                    );
                }
                call_hook(engine, &mut state.scope, ast, "on_tick", gid, &mut state.this);
            }
        }
    });
}

// ── Public entry point ─────────────────────────────────────────────────────

// ── One-shot drain (RunRhai) ───────────────────────────────────────────────

/// Queue of `(command_id, code)` snippets submitted by `RunRhai`, waiting to
/// run inside the exclusive [`drain_world_scripts`] system where `&mut World`
/// is available. The `command_id` is the request id so the outcome can be
/// recorded in [`CommandResults`] for the caller to poll.
#[derive(Resource, Default)]
pub struct PendingWorldScripts {
    pub queue: Vec<(u64, String)>,
}

/// Exclusive system: run every queued snippet against the live World and record
/// its real stdout (or error) under the originating command id, overwriting the
/// provisional "queued" outcome the `RunRhai` handler recorded.
pub fn drain_world_scripts(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<PendingWorldScripts>().queue);
    if pending.is_empty() {
        return;
    }
    for (id, code) in pending {
        let outcome = match eval_with_world(world, &code) {
            Ok(stdout) => {
                let mut ack = Ack::new(OpId::new());
                ack.assigned = serde_json::json!({ "stdout": stdout });
                Ok(ack)
            }
            Err(e) => Err(e),
        };
        // `id == 0` means an in-process trigger with no pollable request id.
        if id != 0 {
            world.resource_mut::<CommandResults>().record(id, outcome);
        }
    }
}

/// Evaluate `code` against `world`, capturing `print(...)` as stdout. The
/// World is in scope (via [`WorldScope`]) for the whole evaluation, so the
/// bridge verbs work. Returns captured output (plus the final expression's
/// value if non-unit), or the error message.
pub fn eval_with_world(world: &mut World, code: &str) -> Result<String, String> {
    use std::sync::{Arc, Mutex};

    // A fresh engine per call keeps state isolated; cheap relative to the work.
    let mut engine = build_world_engine();

    let out = Arc::new(Mutex::new(String::new()));
    let sink = out.clone();
    engine.on_print(move |s| {
        if let Ok(mut buf) = sink.lock() {
            buf.push_str(s);
            buf.push('\n');
        }
    });

    let _scope = WorldScope::enter(world);
    let result = engine.eval::<Dynamic>(code).map_err(|e| e.to_string())?;

    let mut captured = out
        .lock()
        .map_err(|_| "print buffer poisoned".to_string())?
        .clone();
    if !result.is_unit() {
        captured.push_str(&result.to_string());
    }
    Ok(captured)
}
