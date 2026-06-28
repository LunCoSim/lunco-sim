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

use lunco_core::{coords, Ack, CommandResults, GlobalEntityId, OpId, SimTick};
use lunco_api::executor::ApiCommandEvent;
use lunco_api::registry::ApiEntityRegistry;

use rhai::{Dynamic, Engine, ImmutableString, Map};

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

/// Build a rhai [`Engine`] with the World-bridge verbs registered and the same
/// sandbox caps as the one-shot backend.
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

    // sim_tick() -> i64 — current FixedUpdate tick.
    engine.register_fn("sim_tick", || -> i64 {
        with_world(|w| w.get_resource::<SimTick>().map(|t| t.0 as i64))
            .flatten()
            .unwrap_or(0)
    });

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
