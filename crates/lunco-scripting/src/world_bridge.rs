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

use lunco_core::{Ack, CommandResults, OpId, TelemetryEvent, TelemetryValue};

use rhai::{Dynamic, Engine, ImmutableString, Map, AST};

use crate::bridge_core::{self, ValueBuilder};
use crate::doc::ScriptLanguage;
use lunco_doc::Diagnostic;

// ── Native value builder (rhai) ────────────────────────────────────────────
//
// The world-access logic lives in [`crate::bridge_core`], language-neutral. This
// is the rhai binding: `RhaiBuilder` constructs native `Dynamic` values so the
// core's generic readers build `reflect → Dynamic` in one hop (no JSON on the
// read path). `WorldScope` / `with_world` also live in `bridge_core` now.

/// Builds native rhai `Dynamic` values for the bridge core's generic readers —
/// the rhai impl of [`ValueBuilder`]. Unit struct, zero cost.
pub struct RhaiBuilder;

impl ValueBuilder for RhaiBuilder {
    type Value = Dynamic;
    fn unit(&self) -> Dynamic {
        Dynamic::UNIT
    }
    fn float(&self, f: f64) -> Dynamic {
        Dynamic::from_float(f)
    }
    fn int(&self, i: i64) -> Dynamic {
        Dynamic::from_int(i)
    }
    fn bool(&self, b: bool) -> Dynamic {
        Dynamic::from_bool(b)
    }
    fn string(&self, s: &str) -> Dynamic {
        Dynamic::from(s.to_string())
    }
    fn array(&self, items: Vec<Dynamic>) -> Dynamic {
        Dynamic::from_array(items)
    }
    fn map(&self, entries: Vec<(String, Dynamic)>) -> Dynamic {
        let mut m = Map::new();
        for (k, v) in entries {
            m.insert(k.into(), v);
        }
        Dynamic::from_map(m)
    }
}

/// Map a rhai value to the engine-wide [`TelemetryValue`] for `emit`. Scalars
/// map directly; everything else stringifies; unit is a bare pulse.
fn rhai_to_telemetry(value: &Dynamic) -> TelemetryValue {
    if value.is_unit() {
        TelemetryValue::Bool(true)
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

/// Convert a rhai params map to the JSON the API command/query layer expects —
/// the one inherent JSON seam (`cmd`/`query` params are *defined* as JSON).
fn map_to_json(params: Map) -> serde_json::Value {
    rhai::serde::from_dynamic::<serde_json::Value>(&Dynamic::from_map(params))
        .unwrap_or(serde_json::Value::Null)
}

/// Walk a rhai [`Dynamic`] into a backend-native value via a [`ValueBuilder`], in
/// one pass — the inverse of [`RhaiBuilder`], format-agnostic. Used for scenario
/// introspection: the live `this` state is rebuilt into whatever the caller's
/// builder targets (JSON at the API seam, or rhai itself for a script verb),
/// keeping JSON out of the path unless that's the chosen output. Unknown/custom
/// inner types fall back to their display string.
fn dynamic_to_value<B: ValueBuilder>(b: &B, d: &Dynamic) -> B::Value {
    if d.is_unit() {
        b.unit()
    } else if let Ok(x) = d.as_bool() {
        b.bool(x)
    } else if let Ok(i) = d.as_int() {
        b.int(i)
    } else if let Ok(f) = d.as_float() {
        b.float(f)
    } else if d.is_string() {
        b.string(&d.clone().into_string().unwrap_or_default())
    } else if let Some(arr) = d.clone().try_cast::<rhai::Array>() {
        b.array(arr.iter().map(|x| dynamic_to_value(b, x)).collect())
    } else if let Some(m) = d.clone().try_cast::<Map>() {
        b.map(
            m.into_iter()
                .map(|(k, v)| (k.to_string(), dynamic_to_value(b, &v)))
                .collect(),
        )
    } else {
        b.string(&d.to_string())
    }
}

// ── Native write: Dynamic → reflect ────────────────────────────────────────

/// A rhai [`Dynamic`] as `f64` (ints widen); error if it isn't a number.
fn dyn_f64(v: &Dynamic) -> Result<f64, String> {
    v.as_float()
        .or_else(|_| v.as_int().map(|i| i as f64))
        .map_err(|_| "expected a number".to_string())
}

/// A rhai [`Dynamic`] as `i64` (floats truncate); error if it isn't a number.
fn dyn_i64(v: &Dynamic) -> Result<i64, String> {
    v.as_int()
        .or_else(|_| v.as_float().map(|f| f as i64))
        .map_err(|_| "expected an integer".to_string())
}

/// A rhai array of exactly `n` numbers as `f64`s — for glam vec/quat fields.
fn dyn_f64s(v: &Dynamic, n: usize) -> Result<Vec<f64>, String> {
    let arr = v
        .clone()
        .try_cast::<rhai::Array>()
        .ok_or_else(|| format!("expected an array of {n} numbers"))?;
    if arr.len() != n {
        return Err(format!("expected {n} numbers, got {}", arr.len()));
    }
    arr.iter().map(dyn_f64).collect()
}

/// Write a rhai [`Dynamic`] straight onto a reflected field — the inverse of
/// [`build_from_reflect`](bridge_core::build_from_reflect)'s read. The field's
/// concrete type drives the coercion (scalars widen/truncate as needed; arrays
/// become glam vectors/quats), so `native → reflect` happens in one hop with no
/// JSON. Unsupported field types return an error the script verb surfaces.
fn apply_dynamic(field: &mut dyn bevy::reflect::PartialReflect, value: &Dynamic) -> Result<(), String> {
    use bevy::math::{DVec2, DVec3, Quat, Vec2, Vec3};
    let any = field
        .try_as_reflect_mut()
        .ok_or_else(|| "field is not concretely reflectable".to_string())?
        .as_any_mut();

    if let Some(s) = any.downcast_mut::<f64>() {
        *s = dyn_f64(value)?;
    } else if let Some(s) = any.downcast_mut::<f32>() {
        *s = dyn_f64(value)? as f32;
    } else if let Some(s) = any.downcast_mut::<i64>() {
        *s = dyn_i64(value)?;
    } else if let Some(s) = any.downcast_mut::<i32>() {
        *s = dyn_i64(value)? as i32;
    } else if let Some(s) = any.downcast_mut::<u64>() {
        *s = dyn_i64(value)? as u64;
    } else if let Some(s) = any.downcast_mut::<u32>() {
        *s = dyn_i64(value)? as u32;
    } else if let Some(s) = any.downcast_mut::<usize>() {
        *s = dyn_i64(value)? as usize;
    } else if let Some(s) = any.downcast_mut::<bool>() {
        *s = value.as_bool().map_err(|_| "expected a bool".to_string())?;
    } else if let Some(s) = any.downcast_mut::<String>() {
        *s = value.clone().into_string().map_err(|_| "expected a string".to_string())?;
    } else if let Some(v) = any.downcast_mut::<Vec3>() {
        let a = dyn_f64s(value, 3)?;
        *v = Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32);
    } else if let Some(v) = any.downcast_mut::<Vec2>() {
        let a = dyn_f64s(value, 2)?;
        *v = Vec2::new(a[0] as f32, a[1] as f32);
    } else if let Some(v) = any.downcast_mut::<Quat>() {
        let a = dyn_f64s(value, 4)?;
        *v = Quat::from_xyzw(a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32);
    } else if let Some(v) = any.downcast_mut::<DVec3>() {
        let a = dyn_f64s(value, 3)?;
        *v = DVec3::new(a[0], a[1], a[2]);
    } else if let Some(v) = any.downcast_mut::<DVec2>() {
        let a = dyn_f64s(value, 2)?;
        *v = DVec2::new(a[0], a[1]);
    } else {
        return Err(format!("set: unsupported field type '{}'", field.reflect_type_path()));
    }
    Ok(())
}

/// Patch a default-constructed component with a rhai field map — each `key: val`
/// is written onto the matching reflected field via [`apply_dynamic`]. Used by the
/// `add` verb to build a component natively (no JSON).
fn apply_dynamic_fields(component: &mut dyn bevy::reflect::Reflect, fields: &Map) -> Result<(), String> {
    for (k, v) in fields.iter() {
        let path = format!(".{k}");
        let field = component
            .reflect_path_mut(path.as_str())
            .map_err(|e| format!("no field '{k}': {e}"))?;
        apply_dynamic(field, v)?;
    }
    Ok(())
}

// ── Engine construction ────────────────────────────────────────────────────

/// Ergonomic policy wrappers (drive/distance/arrived/...), authored in rhai and
/// embedded at compile time so they're available with zero IO on every target
/// (incl. wasm). Edit `rhai/prelude.rhai` — no Rust change needed for new helpers.
pub(crate) const PRELUDE: &str = include_str!("../rhai/prelude.rhai");

/// Build a rhai [`Engine`] with the World-bridge verbs registered, the embedded
/// prelude loaded as a global module, and the same sandbox caps as the one-shot
/// backend.
pub fn build_world_engine() -> Engine {
    let mut engine = Engine::new();

    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);
    // Above rhai's defaults (64 global / 32 in functions): the built-in task tree
    // nests composites (e.g. par_all → seq → leaf), and the recursive `__tick`
    // helpers walk that depth. Runtime stays bounded by the operations + call-level
    // caps above; this only widens the structural nesting allowance.
    engine.set_max_expr_depths(128, 128);

    // cmd(name, #{params}) -> #{ id, ok, data, error }. Routes through
    // ApiCommandEvent so it inherits macro-reflected dispatch, GlobalEntityId
    // resolution, and result recording. The command runs SYNCHRONOUSLY (the
    // bridge flushes), so `data` carries any values the handler assigned — a
    // spawned entity's gid, an allocated name — enabling create-then-manipulate
    // in one tick. `ok=false` + `error` on a handler error/rejection.
    engine.register_fn("cmd", |name: ImmutableString, params: Map| -> Dynamic {
        bridge_core::cmd(&RhaiBuilder, name.as_str(), map_to_json(params))
    });
    // cmd(name) -> #{...} — convenience for unit/all-defaulted commands.
    engine.register_fn("cmd", |name: ImmutableString| -> Dynamic {
        bridge_core::cmd(&RhaiBuilder, name.as_str(), serde_json::json!({}))
    });

    // world_pos(id) -> [x, y, z] absolute world space, or () on miss.
    engine.register_fn("world_pos", |id: i64| -> Dynamic {
        match bridge_core::world_pos(id as u64) {
            Some(v) => RhaiBuilder.array(vec![
                Dynamic::from_float(v.x),
                Dynamic::from_float(v.y),
                Dynamic::from_float(v.z),
            ]),
            None => Dynamic::UNIT,
        }
    });

    // world_forward(id) -> [x, y, z] unit heading in world space, or ().
    // The ONE read rhai can't derive itself (world orientation needs the ECS
    // float-origin hierarchy). All steering MATH stays in rhai (the prelude);
    // this just exposes the heading vector, like world_pos exposes position.
    engine.register_fn("world_forward", |id: i64| -> Dynamic {
        match bridge_core::world_forward(id as u64) {
            Some(v) => RhaiBuilder.array(vec![
                Dynamic::from_float(v.x),
                Dynamic::from_float(v.y),
                Dynamic::from_float(v.z),
            ]),
            None => Dynamic::UNIT,
        }
    });

    // get(id, "Component.field") -> Dynamic (f64/i64/bool/string/array/map) or ().
    // The generic reflection read — built native (reflect → Dynamic, one hop).
    engine.register_fn("get", |id: i64, path: ImmutableString| -> Dynamic {
        if let Some(v) = bridge_core::get_field(&RhaiBuilder, id as u64, path.as_str()) {
            return v;
        }
        // Reflection missed — fall back to the co-sim port registry (Modelica
        // vars, avian state, joint angles, hardware ports). Same surface the
        // wire engine and the API read, so a script sees what the sim exchanges.
        match bridge_core::read_port(id as u64, path.as_str()) {
            Some(p) => Dynamic::from_float(p),
            None => Dynamic::UNIT,
        }
    });

    // set(id, "Component.field", value) -> bool — the WRITE twin of get(). Applies
    // `value` straight onto the reflected field (native → reflect, no JSON), the
    // mirror of the read path. Coerces by the field's type (scalars widen, arrays
    // → glam vec/quat). Host-only, so it's authoritative; the change replicates
    // via normal component sync. Returns false (and logs why) on a bad
    // entity/path/type — so a scenario can branch on the result.
    engine.register_fn("set", |id: i64, path: ImmutableString, value: Dynamic| -> bool {
        match bridge_core::set_component_field(id as u64, path.as_str(), |f| apply_dynamic(f, &value)) {
            Ok(()) => true,
            Err(e) => {
                // Reflection missed — fall back to the co-sim port registry (the
                // same path wires and `SetPort` use). Ports are scalar, so coerce
                // the value to f64; a non-numeric set genuinely failed.
                let scalar = value.as_float().ok().or_else(|| value.as_int().ok().map(|i| i as f64));
                if let Some(v) = scalar {
                    if bridge_core::write_port(id as u64, path.as_str(), v) {
                        return true;
                    }
                }
                warn!("[rhai] set({id}, \"{path}\") failed: {e}");
                false
            }
        }
    });

    // param(id, "key", default) -> f64 — read a per-prim numeric script param
    // (USD `lunco:params`) via ScriptParams. The typed, fast per-instance-config
    // read; falls back to `default` when absent. Use this, NOT name(me) scanning.
    engine.register_fn("param", |id: i64, key: ImmutableString, default: f64| -> f64 {
        bridge_core::script_param(id as u64, key.as_str()).unwrap_or(default)
    });
    // param(id, "key") -> f64 | () — same, but () when the param is absent.
    engine.register_fn("param", |id: i64, key: ImmutableString| -> Dynamic {
        match bridge_core::script_param(id as u64, key.as_str()) {
            Some(v) => Dynamic::from_float(v),
            None => Dynamic::UNIT,
        }
    });

    // get_setting("Resource.field") -> Dynamic — read a GLOBAL setting (the
    // resource twin of get()). Settings/config live in resources, not components;
    // this reaches any reflect-registered `Resource`. () if missing.
    engine.register_fn("get_setting", |path: ImmutableString| -> Dynamic {
        bridge_core::get_resource_field(&RhaiBuilder, path.as_str()).unwrap_or(Dynamic::UNIT)
    });

    // set_setting("Resource.field", value) -> bool — write a GLOBAL setting (the
    // resource twin of set()). Makes every reflect-registered resource field
    // tunable from a scenario with no per-setting command. Host-authoritative.
    engine.register_fn("set_setting", |path: ImmutableString, value: Dynamic| -> bool {
        match bridge_core::set_resource_field(path.as_str(), |f| apply_dynamic(f, &value)) {
            Ok(()) => true,
            Err(e) => {
                warn!("[rhai] set_setting(\"{path}\") failed: {e}");
                false
            }
        }
    });

    // list_entities() -> [#{ id, name, type, pos }] for every registered entity.
    engine.register_fn("list_entities", || -> Dynamic {
        bridge_core::list_entities(&RhaiBuilder)
    });

    // ── Structural mutation: the C/D twin of get/set's R/U ───────────────────
    // add(id, "Comp", #{fields}) -> bool — insert/replace a reflected component
    // built from its default + the field map (native → reflect). false on bad
    // entity/type/field, or if the type has no ReflectDefault.
    engine.register_fn("add", |id: i64, comp: ImmutableString, fields: Map| -> bool {
        match bridge_core::add_component(id as u64, comp.as_str(), |c| apply_dynamic_fields(c, &fields)) {
            Ok(()) => true,
            Err(e) => {
                warn!("[rhai] add({id}, \"{comp}\") failed: {e}");
                false
            }
        }
    });
    // add(id, "Comp") -> bool — insert the default component (no field overrides).
    engine.register_fn("add", |id: i64, comp: ImmutableString| -> bool {
        match bridge_core::add_component(id as u64, comp.as_str(), |_| Ok(())) {
            Ok(()) => true,
            Err(e) => {
                warn!("[rhai] add({id}, \"{comp}\") failed: {e}");
                false
            }
        }
    });
    // remove(id, "Comp") -> bool — strip a reflected component. false if absent.
    engine.register_fn("remove", |id: i64, comp: ImmutableString| -> bool {
        match bridge_core::remove_component(id as u64, comp.as_str()) {
            Ok(()) => true,
            Err(e) => {
                warn!("[rhai] remove({id}, \"{comp}\") failed: {e}");
                false
            }
        }
    });
    // despawn(id) -> bool — despawn an entity (+ children); replicates on a host.
    // Runtime SPAWN has no generic verb (clients reconstruct from a catalog
    // entry_id, not a component bag) — use cmd("SpawnEntity", #{entry_id, position}).
    engine.register_fn("despawn", |id: i64| -> bool {
        match bridge_core::despawn_entity(id as u64) {
            Ok(()) => true,
            Err(e) => {
                warn!("[rhai] despawn({id}) failed: {e}");
                false
            }
        }
    });

    // query(name, #{params}) -> Dynamic — the READ twin of cmd(): invoke any
    // registered `ApiQueryProvider` by name (Raycast, Nearest, GroundHeight,
    // CosimStatus, …) and get its data back as rhai values. Spatial/physics
    // providers live in their owning crates (e.g. avian-backed Raycast in
    // lunco-mobility); scripting reaches them generically here without taking a
    // physics dependency. Returns () if the provider is missing or errors.
    engine.register_fn("query", |name: ImmutableString, params: Map| -> Dynamic {
        bridge_core::query(&RhaiBuilder, name.as_str(), map_to_json(params))
    });
    engine.register_fn("query", |name: ImmutableString| -> Dynamic {
        bridge_core::query(&RhaiBuilder, name.as_str(), serde_json::Value::Null)
    });

    // find(name) -> id (i64), or -1 if no entity has that Name.
    engine.register_fn("find", |name: ImmutableString| -> i64 {
        bridge_core::find(name.as_str())
    });

    // name(id) -> the entity's Name as a string, or () if unnamed/unknown. The
    // reverse of find(name): turn an id (from list_entities/nearest/children/…)
    // back into a human label for logging/UI.
    engine.register_fn("name", |id: i64| -> Dynamic {
        bridge_core::name_of(id as u64).map(Dynamic::from).unwrap_or(Dynamic::UNIT)
    });

    // parent(id) -> parent entity id (i64), or () if it has no parent or the
    // parent isn't a registered (script-visible) entity. Hierarchy traversal up.
    engine.register_fn("parent", |id: i64| -> Dynamic {
        bridge_core::parent_of(id as u64).map(Dynamic::from_int).unwrap_or(Dynamic::UNIT)
    });

    // children(id) -> [id, ...] of the entity's DIRECT children that are
    // registered entities (skips un-registered internal children). Empty if none.
    // Hierarchy traversal down; compose with parent()/name() for tree walks.
    engine.register_fn("children", |id: i64| -> Dynamic {
        RhaiBuilder.array(
            bridge_core::children_of(id as u64)
                .into_iter()
                .map(Dynamic::from_int)
                .collect(),
        )
    });

    // emit(name, value) -> bool — fire a TelemetryEvent on the shared bus
    // (reused, not reinvented): existing API-subscription + log observers see
    // it immediately, and scripts receive it next tick via on_event. `value`
    // may be float / int / bool / string.
    engine.register_fn("emit", |name: ImmutableString, value: Dynamic| -> bool {
        bridge_core::emit(name.as_str(), rhai_to_telemetry(&value))
    });
    // emit(name) — a bare pulse (no payload).
    engine.register_fn("emit", |name: ImmutableString| -> bool {
        bridge_core::emit(name.as_str(), TelemetryValue::Bool(true))
    });

    // sim_tick() -> i64 — current FixedUpdate tick.
    engine.register_fn("sim_tick", || -> i64 { bridge_core::sim_tick() });

    // dt() -> f64 — the fixed-step integration delta in seconds (1/FIXED_HZ).
    // The per-tick `dt` an on_tick hook should multiply rates by for
    // frame-rate-independent integration. Falls back to the canonical
    // SECS_PER_TICK if no `Time<Fixed>` is in scope (e.g. a bare test world).
    engine.register_fn("dt", || -> f64 { bridge_core::dt() });

    // elapsed_seconds() -> f64 — monotonic simulation seconds since startup, for
    // second-based timeouts / rate limits (`this.t0`-relative dwell, etc.). Uses
    // the fixed clock's elapsed time (advances only while the sim steps), 0.0 if
    // unavailable.
    engine.register_fn("elapsed_seconds", || -> f64 { bridge_core::elapsed_seconds() });

    // rand() -> f64 in [0,1) — DETERMINISTIC: seeded per hook from (entity, tick,
    // hook), so it's identical on every networked peer and every re-run/replay.
    // Use this, never a wall-clock/OS source, or scenarios desync.
    engine.register_fn("rand", || -> f64 { bridge_core::rng_next_f64() });
    // rand_range(lo, hi) -> f64 in [lo, hi).
    engine.register_fn("rand_range", |lo: f64, hi: f64| -> f64 {
        lo + (hi - lo) * bridge_core::rng_next_f64()
    });
    // rand_int(lo, hi) -> i64 in [lo, hi) (half-open). Returns lo if hi <= lo.
    engine.register_fn("rand_int", |lo: i64, hi: i64| -> i64 {
        if hi <= lo {
            lo
        } else {
            lo + (bridge_core::rng_next_f64() * (hi - lo) as f64) as i64
        }
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

    // Register the importable tool libraries as static modules (callable as
    // `libname::fn`). AFTER the prelude global module so their functions can
    // resolve prelude helpers at run time.
    crate::tool_libs::refresh(&mut engine);

    engine
}

// ── Persistent per-entity scenario runtime (rhai backend) ──────────────────
//
// A `ScriptedModel { language: Rhai }` runs its `ScriptDocument` as a persistent
// program with lifecycle hooks (`on_start`/`on_tick`/`on_event`/`on_stop`), NOT
// a one-shot snippet. The lifecycle POLICY (scheduling, hot-reload, pause,
// teardown, diagnostics) is language-neutral and lives in
// [`crate::scenario::ScenarioDriver`]. This is the rhai BACKEND: it implements
// [`crate::scenario::ScenarioRuntime`], supplying only the mechanics — compile
// source → `AST` (running top-level into a persistent `Scope` for `const`s), and
// call a hook via `call_fn_raw`. Per-entity tick-to-tick state lives in a `this`
// object-map (rhai functions are pure — they can't see top-level `let`s). One
// shared `Engine` carries the world-bridge verbs, so a hook can `cmd()`/`get()`.

/// Per-entity compiled rhai program + live state.
struct RhaiScenarioState {
    ast: AST,
    /// Top-level `const` globals, populated by running the body once at compile.
    scope: rhai::Scope<'static>,
    /// Per-entity mutable state bound as `this` in every hook.
    this: Dynamic,
}

/// The rhai [`ScenarioRuntime`](crate::scenario::ScenarioRuntime): one
/// bridge-enabled engine + a per-entity program cache. Wrapped by
/// `ScenarioDriver<RhaiScenarioRuntime>` (which owns the neutral lifecycle FSM).
pub struct RhaiScenarioRuntime {
    engine: Engine,
    states: std::collections::HashMap<Entity, RhaiScenarioState>,
    /// The prelude compiled to an `AST`, merged into every scenario's AST so its
    /// helpers — including the engine-driven `__run_task` / `__note_task_event`
    /// drivers — are resolvable by `call_fn` (which searches the AST, NOT the
    /// engine's registered global modules). The prelude stays registered as a
    /// global module too, for the runtime-resolution path used while a script's
    /// own body executes.
    prelude_ast: AST,
    /// Tool-library generation the engine's static modules were built from; a
    /// mismatch triggers a re-`refresh` so a `RegisterToolLibrary` hot-reloads.
    tool_gen: u64,
}

impl Default for RhaiScenarioRuntime {
    fn default() -> Self {
        let mut engine = build_world_engine();
        engine.on_print(|s| info!("[rhai] {s}"));
        let prelude_ast = engine
            .compile(PRELUDE)
            .unwrap_or_else(|e| panic!("prelude.rhai must compile: {e}"));
        Self {
            engine,
            states: std::collections::HashMap::new(),
            prelude_ast,
            // build_world_engine already refreshed at the current generation.
            tool_gen: crate::tool_libs::generation(),
        }
    }
}

impl crate::scenario::ScenarioRuntime for RhaiScenarioRuntime {
    fn compile(
        &mut self,
        entity: Entity,
        source: &str,
        params: &str,
    ) -> crate::scenario::CompileOutcome {
        use crate::scenario::CompileOutcome;
        match self.engine.compile(source) {
            Ok(ast) => {
                // Merge the prelude's functions into the scenario AST so the
                // engine-driven `__run_task`/`__note_task_event` (and every other
                // prelude helper) are resolvable by `call_fn`. Merging prelude←user
                // lets a user function win on any name/arity clash; the prelude has
                // no top-level body, so the seed-run below still executes only the
                // user's top level.
                let ast = self.prelude_ast.merge(&ast);
                let mut scope = rhai::Scope::new();
                // Expose scenario parameters as a read-only `params` constant
                // (native JSON→Dynamic, one hop). Empty / bad JSON → empty map,
                // so `params` is always a readable object.
                let params_value = match (params.is_empty(), serde_json::from_str::<serde_json::Value>(params)) {
                    (true, _) => RhaiBuilder.map(Vec::new()),
                    (false, Ok(v)) => bridge_core::build_from_json(&RhaiBuilder, &v),
                    (false, Err(e)) => {
                        warn!("[rhai] entity {entity:?} ignoring bad params JSON: {e}");
                        RhaiBuilder.map(Vec::new())
                    }
                };
                scope.push_constant_dynamic("params", params_value);
                // Run the top-level body once to seed `const` globals; a runtime
                // error there is non-fatal (hooks still run) — surface it.
                let top_level = match self.engine.run_ast_with_scope(&mut scope, &ast) {
                    Ok(()) => None,
                    Err(e) => {
                        error!("[rhai] entity {entity:?} top-level failed: {e}");
                        Some(rhai_diagnostic(e.to_string(), e.position()))
                    }
                };
                self.states.insert(
                    entity,
                    RhaiScenarioState { ast, scope, this: Dynamic::from_map(Map::new()) },
                );
                CompileOutcome::Ready { top_level }
            }
            Err(e) => {
                error!("[rhai] entity {entity:?} compile error: {e}");
                CompileOutcome::Failed(rhai_diagnostic(e.to_string(), e.position()))
            }
        }
    }

    fn call_hook(
        &mut self,
        entity: Entity,
        hook: crate::scenario::ScenarioHook,
        self_gid: i64,
    ) -> Option<Diagnostic> {
        use crate::scenario::ScenarioHook;
        let st = self.states.get_mut(&entity)?;
        let (name, salt) = match hook {
            ScenarioHook::Start => ("on_start", 1),
            ScenarioHook::Tick => ("on_tick", 2),
            ScenarioHook::Stop => ("on_stop", 3),
        };
        // Seed the deterministic RNG for this hook: (entity, tick, hook).
        bridge_core::rng_begin(self_gid as u64, bridge_core::sim_tick() as u64, salt);
        let user = call_hook(&self.engine, &mut st.scope, &st.ast, name, self_gid, &mut st.this);
        // Built-in drivers (prelude fns, called regardless of what the user AST
        // defines): after on_start, seed `this.task`/`this.mission` from `task(me)`
        // / `mission(me)` fns if present; after on_tick, advance the declared task
        // and evaluate the mission. Each no-ops when the script declared neither,
        // so a plain scenario pays only a couple of cheap calls.
        let drivers: &[&str] = match hook {
            ScenarioHook::Start => &["__init_task", "__init_mission"],
            ScenarioHook::Tick => &["__run_task", "__run_mission"],
            ScenarioHook::Stop => &[],
        };
        let mut driver_err = None;
        for name in drivers {
            let e = call_prelude_driver(
                &self.engine,
                &mut st.scope,
                &st.ast,
                name,
                &mut st.this,
                vec![Dynamic::from_int(self_gid)],
            );
            driver_err = driver_err.or(e);
        }
        user.or(driver_err).map(|(msg, pos)| rhai_diagnostic(msg, pos))
    }

    fn deliver_event(
        &mut self,
        entity: Entity,
        self_gid: i64,
        event: &TelemetryEvent,
    ) -> Option<Diagnostic> {
        // Build the native event value before borrowing per-entity state.
        let evt = bridge_core::build_event(&RhaiBuilder, event);
        // Seed the deterministic RNG: (entity, tick, event-name) — distinct events
        // in the same tick draw distinct streams.
        bridge_core::rng_begin(
            self_gid as u64,
            bridge_core::sim_tick() as u64,
            bridge_core::hash_str(&event.name),
        );
        let st = self.states.get_mut(&entity)?;
        let user =
            call_event_hook(&self.engine, &mut st.scope, &st.ast, self_gid, &mut st.this, evt.clone());
        // Feed the event into the BUILT-IN task too, so `wait_for(name)` steps in a
        // `this.task` complete without a hand-written on_event. No-op if no task.
        let task = call_prelude_driver(
            &self.engine,
            &mut st.scope,
            &st.ast,
            "__note_task_event",
            &mut st.this,
            vec![Dynamic::from_int(self_gid), evt],
        );
        user.or(task).map(|(msg, pos)| rhai_diagnostic(msg, pos))
    }

    fn forget(&mut self, entity: Entity) {
        self.states.remove(&entity);
    }

    fn snapshot<B: ValueBuilder>(
        &self,
        entity: Entity,
        b: &B,
    ) -> Option<crate::scenario::ScenarioSnapshot<B::Value>> {
        let st = self.states.get(&entity)?;
        // Walk the persistent `this` map straight into the caller's native value
        // type — no serde_json intermediate. JSON only results if the caller
        // passed a JsonBuilder (the API path); a script-facing inspect verb could
        // pass RhaiBuilder and get a Dynamic back with zero conversion.
        let state = dynamic_to_value(b, &st.this);
        // Report only the lifecycle hooks the program actually defines (matched on
        // name + arity, exactly as the driver dispatches them).
        let hooks = st
            .ast
            .iter_functions()
            .filter(|f| {
                matches!(
                    (f.name, f.params.len()),
                    ("on_start", 1) | ("on_tick", 1) | ("on_stop", 1) | ("on_event", 2)
                )
            })
            .map(|f| f.name.to_string())
            .collect();
        Some(crate::scenario::ScenarioSnapshot { state, hooks })
    }

    fn maintain(&mut self) {
        // Hot-reload tool libraries if any were (re)registered since last pass.
        let cur = crate::tool_libs::generation();
        if self.tool_gen != cur {
            crate::tool_libs::refresh(&mut self.engine);
            self.tool_gen = cur;
        }
    }
}

/// Exclusive system (FixedUpdate): drive every `ScriptedModel { Rhai }` through
/// its lifecycle via the neutral [`crate::scenario::ScenarioDriver`].
pub fn tick_rhai_scenarios(world: &mut World) {
    crate::scenario::ScenarioDriver::<RhaiScenarioRuntime>::run(world, ScriptLanguage::Rhai);
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
) -> Option<(String, rhai::Position)> {
    let present = ast
        .iter_functions()
        .any(|f| f.name == name && f.params.len() == 1);
    if !present {
        return None;
    }
    let args = [Dynamic::from_int(self_id)];
    let options = rhai::CallFnOptions::new()
        .eval_ast(false)
        .rewind_scope(false)
        .bind_this_ptr(this);
    match engine.call_fn_with_options::<Dynamic>(options, scope, ast, name, args) {
        Ok(_) => None,
        Err(e) => {
            error!("[rhai] {name}() failed: {e}");
            let pos = e.position();
            Some((e.to_string(), pos))
        }
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
    evt: Dynamic,
) -> Option<(String, rhai::Position)> {
    let present = ast
        .iter_functions()
        .any(|f| f.name == "on_event" && f.params.len() == 2);
    if !present {
        return None;
    }
    let args = [Dynamic::from_int(self_id), evt];
    let options = rhai::CallFnOptions::new()
        .eval_ast(false)
        .rewind_scope(false)
        .bind_this_ptr(this);
    match engine.call_fn_with_options::<Dynamic>(options, scope, ast, "on_event", args) {
        Ok(_) => None,
        Err(e) => {
            error!("[rhai] on_event() failed: {e}");
            let pos = e.position();
            Some((e.to_string(), pos))
        }
    }
}


/// Call a built-in PRELUDE driver function `name` (a global-module fn, NOT in the
/// user's AST) with `this` bound — for engine-driven hooks like task auto-advance
/// that live in the prelude, so the AST-presence gate in [`call_hook`] would skip
/// them. A missing function (a custom prelude without the driver) is benign.
fn call_prelude_driver(
    engine: &Engine,
    scope: &mut rhai::Scope,
    ast: &AST,
    name: &str,
    this: &mut Dynamic,
    args: Vec<Dynamic>,
) -> Option<(String, rhai::Position)> {
    let options = rhai::CallFnOptions::new()
        .eval_ast(false)
        .rewind_scope(false)
        .bind_this_ptr(this);
    match engine.call_fn_with_options::<Dynamic>(options, scope, ast, name, args) {
        Ok(_) => None,
        Err(e) if matches!(*e, rhai::EvalAltResult::ErrorFunctionNotFound(..)) => None,
        Err(e) => {
            error!("[rhai] {name}() failed: {e}");
            Some((e.to_string(), e.position()))
        }
    }
}

/// Build an error [`Diagnostic`] from a rhai error message + [`rhai::Position`]
/// (line/col map straight across — no source needed).
fn rhai_diagnostic(message: String, pos: rhai::Position) -> Diagnostic {
    Diagnostic::error(
        message,
        pos.line().map(|l| l as u32),
        pos.position().map(|c| c as u32),
    )
}

// ── Public entry point ─────────────────────────────────────────────────────

// ── One-shot drain (RunRhai) ───────────────────────────────────────────────

/// Queue of `(command_id, code, authority)` snippets submitted by `RunRhai`,
/// waiting to run inside the exclusive [`drain_world_scripts`] system where
/// `&mut World` is available. The `command_id` is the request id so the outcome
/// can be recorded in [`CommandResults`] for the caller to poll. `authority` is
/// the submitting session (the wire origin captured by the handler) the
/// snippet's `cmd()`s are gated against — `None` for a local/host launch (§3.4).
#[derive(Resource, Default)]
pub struct PendingWorldScripts {
    pub queue: Vec<(u64, String, Option<lunco_core::SessionId>)>,
}

/// Exclusive system: run every queued snippet against the live World and record
/// its real stdout (or error) under the originating command id, overwriting the
/// provisional "queued" outcome the `RunRhai` handler recorded.
pub fn drain_world_scripts(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<PendingWorldScripts>().queue);
    if pending.is_empty() {
        return;
    }
    for (id, code, authority) in pending {
        let outcome = match eval_with_world_as(world, &code, authority) {
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
/// value if non-unit), or the error message. Runs host-trusted (no `cmd()`
/// authority gate) — see [`eval_with_world_as`] to bind a submitting session.
pub fn eval_with_world(world: &mut World, code: &str) -> Result<String, String> {
    eval_with_world_as(world, code, None)
}

/// As [`eval_with_world`], but the snippet's `cmd()` calls are authorized against
/// `authority` (the submitting session) per design §3.4. `None` = host-trusted
/// (ungated), matching the open-by-default substrate.
pub fn eval_with_world_as(
    world: &mut World,
    code: &str,
    authority: Option<lunco_core::SessionId>,
) -> Result<String, String> {
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

    let _scope = bridge_core::WorldScope::enter(world);
    // `enter` reset the authority to None; bind the submitter for this eval.
    bridge_core::set_script_authority(authority);
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

#[cfg(test)]
mod tests {
    //! Syntax-validate the embedded prelude + shipped example scenarios. Rust's
    //! `cargo check` can't see inside the `.rhai` files (they're `include_str!`),
    //! so a parse error would otherwise only surface at runtime as a logged
    //! "prelude compile failed". `compile` checks syntax (unresolved function
    //! calls resolve at runtime, so calling prelude verbs here is fine).

    #[test]
    fn get_returns_vectors_as_arrays() {
        use bevy::math::{Quat, Vec3};
        // Vec3 → [x,y,z]
        let d = crate::bridge_core::build_from_reflect(&super::RhaiBuilder, &Vec3::new(1.0, 2.0, 3.0)).unwrap();
        let a = d.into_array().expect("Vec3 should become a rhai array");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].as_float().unwrap(), 1.0);
        assert_eq!(a[2].as_float().unwrap(), 3.0);

        // Quat → [x,y,z,w]
        let q = crate::bridge_core::build_from_reflect(&super::RhaiBuilder, &Quat::from_xyzw(0.0, 0.0, 0.0, 1.0))
            .unwrap()
            .into_array()
            .expect("Quat should become a rhai array");
        assert_eq!(q.len(), 4);
        assert_eq!(q[3].as_float().unwrap(), 1.0);

        // scalar stays scalar
        let s = crate::bridge_core::build_from_reflect(&super::RhaiBuilder, &7.5_f64).unwrap();
        assert_eq!(s.as_float().unwrap(), 7.5);
    }

    #[test]
    fn prelude_and_examples_parse() {
        let engine = rhai::Engine::new();
        engine
            .compile(super::PRELUDE)
            .expect("prelude.rhai must parse");

        for (name, src) in [
            ("patrol", include_str!("../rhai/examples/patrol.rhai")),
            ("mission", include_str!("../rhai/examples/mission.rhai")),
            (
                "mission_plan",
                include_str!("../rhai/examples/mission_plan.rhai"),
            ),
            ("sequence", include_str!("../rhai/examples/sequence.rhai")),
            ("timeline", include_str!("../rhai/examples/timeline.rhai")),
            ("avoid", include_str!("../rhai/examples/avoid.rhai")),
            // Bundled scenario carrying the lander auto-land GUIDANCE (writes the
            // command ports). Parse-guarded here so a syntax slip can't silently
            // disable auto-land at scene load.
            (
                "lander_subsystems (scenario)",
                include_str!("../../../assets/scenarios/lander_subsystems.rhai"),
            ),
            (
                "formation (tool lib)",
                include_str!("../rhai/tools/formation.rhai"),
            ),
            (
                "survey (tool lib)",
                include_str!("../rhai/tools/survey.rhai"),
            ),
        ] {
            engine
                .compile(src)
                .unwrap_or_else(|e| panic!("{name}.rhai failed to parse: {e}"));
        }
    }

    #[test]
    fn prelude_loads_as_module() {
        // The full build path: verbs + prelude-as-global-module must succeed.
        let _engine = super::build_world_engine();
    }

    #[test]
    fn dt_and_elapsed_read_the_fixed_clock() {
        use bevy::prelude::*;
        use bevy::time::{Fixed, Time};
        let mut world = World::new();
        let mut t: Time<Fixed> = Default::default();
        // Directly advance the fixed clock one step so delta/elapsed are set.
        t.advance_by(std::time::Duration::from_secs_f64(1.0 / 60.0));
        world.insert_resource(t);

        let dt: f64 = super::eval_with_world(&mut world, "dt()")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!((dt - 1.0 / 60.0).abs() < 1e-9, "dt() was {dt}");

        let el: f64 = super::eval_with_world(&mut world, "elapsed_seconds()")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!((el - 1.0 / 60.0).abs() < 1e-9, "elapsed_seconds() was {el}");
    }

    #[test]
    fn dt_falls_back_to_secs_per_tick_without_a_clock() {
        use bevy::prelude::World;
        let mut world = World::new();
        let dt: f64 = super::eval_with_world(&mut world, "dt()")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!((dt - lunco_core::SECS_PER_TICK).abs() < 1e-12, "dt() was {dt}");
    }

    #[test]
    fn selection_toolkit_closures_work_across_the_prelude_module() {
        use bevy::prelude::World;
        let mut world = World::new();
        // min_by/max_by/count_where take rhai closures and `.call` them from
        // inside the prelude global module — validate that boundary works.
        let code = r#"
            let xs = [#{id:1,v:5}, #{id:2,v:2}, #{id:3,v:9}];
            let lo = min_by(xs, |e| e.v);
            let hi = max_by(xs, |e| e.v);
            let n  = count_where(xs, |e| e.v > 3);
            [lo.id, hi.id, n]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(out.trim(), "[2, 3, 2]", "got {out}");
    }

    #[test]
    fn collision_helpers_resolve_the_other_party() {
        use bevy::prelude::World;
        let mut world = World::new();
        // A COLLISION_START between gid 10 and gid 20: from 10's view, the other
        // party is 20; `entered` is true for a START, `exited` false.
        let code = r#"
            let evt = #{ name: "COLLISION_START", value: "10:20" };
            [ collision_other(evt, 10), collision_other(evt, 20),
              entered(evt, 10), exited(evt, 10),
              collision_other(evt, 99) == () ]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        // rhai prints the array as [20, 10, true, false, true]
        assert_eq!(out.trim(), "[20, 10, true, false, true]", "got {out}");
    }

    #[test]
    fn zone_helpers_match_named_trigger_volumes() {
        use bevy::prelude::World;
        let mut world = World::new();
        // An `enter:pad_2` pulse: value is the entrant gid (42). zone_of strips
        // the prefix; entered_zone/exited_zone match by name. (Assert bools only,
        // so the check doesn't depend on how rhai quotes strings in an array.)
        let code = r#"
            let evt = #{ name: "enter:pad_2", value: 42 };
            [ zone_of(evt) == "pad_2", entered_zone(evt, "pad_2"), entered_zone(evt, "bay"),
              exited_zone(#{ name: "exit:bay" }, "bay"), zone_of(#{ name: "COLLISION_START" }) == () ]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(out.trim(), "[true, true, false, true, true]", "got {out}");
    }

    #[test]
    fn sequencer_advances_through_steps() {
        use bevy::prelude::World;
        let mut world = World::new();
        // No fixed clock → elapsed_seconds() is 0.0 every call, so wait(0.0)
        // completes immediately and the cursor walks step 0→1→2→len, then no-ops.
        // me=0 is a dummy id the predicate closures ignore.
        let code = r#"
            let steps = [ wait_until(|m| true), wait(0.0), wait_until(|m| true) ];
            let cur = seq_init();
            cur = run_steps(0, steps, cur); let a = cur.step;   // step0 done -> 1
            cur = run_steps(0, steps, cur); let b = cur.step;   // step1 dwell -> 2
            cur = run_steps(0, steps, cur); let c = cur.step;   // step2 done -> 3 (==len)
            cur = run_steps(0, steps, cur); let d = cur.step;   // past end -> no-op
            [a, b, c, d]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(out.trim(), "[1, 2, 3, 3]", "got {out}");
    }

    #[test]
    fn wait_for_event_completes_via_seq_note_event() {
        use bevy::prelude::World;
        let mut world = World::new();
        // A wait_for("PING") step holds until seq_note_event feeds a matching
        // event; an unrelated event must NOT advance it.
        let code = r#"
            let steps = [ wait_for("PING") ];
            let cur = seq_init();
            cur = run_steps(0, steps, cur); let before = cur.step;        // still 0
            cur = seq_note_event(cur, #{ name: "OTHER" });
            cur = run_steps(0, steps, cur); let after_other = cur.step;   // still 0
            cur = seq_note_event(cur, #{ name: "PING" });
            cur = run_steps(0, steps, cur); let after_ping = cur.step;    // now 1 (==len)
            [before, after_other, after_ping]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(out.trim(), "[0, 0, 1]", "got {out}");
    }

    #[test]
    fn data_timeline_compiles_and_runs_on_layer1() {
        use bevy::prelude::World;
        let mut world = World::new();
        // A pure-DATA timeline (the Layer-2 shape RunTimeline embeds): an emit
        // one-shot, a 0s dwell, then wait-for-event. compile_timeline lowers it
        // to Layer-1 steps run by run_steps; seq_note_event unblocks the wait.
        let code = r#"
            let data = [ #{ emit: "GO_MARK", value: 1 }, #{ wait: 0.0 }, #{ wait_event: "GO" } ];
            let steps = compile_timeline(data);
            let cur = seq_init();
            cur = run_steps(0, steps, cur); let a = cur.step;   // emit once -> 1
            cur = run_steps(0, steps, cur); let b = cur.step;   // dwell 0s   -> 2
            cur = run_steps(0, steps, cur); let c = cur.step;   // wait_event -> still 2
            cur = seq_note_event(cur, #{ name: "GO" });
            cur = run_steps(0, steps, cur); let d = cur.step;   // event seen -> 3 (==len)
            [steps.len(), a, b, c, d]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(out.trim(), "[3, 1, 2, 2, 3]", "got {out}");
    }

    #[test]
    fn hierarchy_verbs_walk_parent_children_and_name() {
        use bevy::prelude::*;
        use lunco_api::registry::ApiEntityRegistry;
        use lunco_core::GlobalEntityId;

        let mut world = World::new();
        world.init_resource::<ApiEntityRegistry>();
        let parent = world.spawn(Name::new("base")).id();
        // Inserting ChildOf fires the relationship hook → parent gains Children.
        let child = world.spawn((Name::new("arm"), ChildOf(parent))).id();
        {
            let mut reg = world.resource_mut::<ApiEntityRegistry>();
            reg.assign(parent, GlobalEntityId::from_raw(100));
            reg.assign(child, GlobalEntityId::from_raw(200));
        }

        // name() reverse-lookup, parent() up, children() down, and the () cases.
        let code = r#"
            [ name(100) == "base", name(200) == "arm",
              parent(200), children(100).len(), children(100)[0],
              name(999) == (), parent(100) == () ]
        "#;
        let out = super::eval_with_world(&mut world, code).unwrap();
        assert_eq!(
            out.trim(),
            "[true, true, 100, 1, 200, true, true]",
            "got {out}"
        );
    }
}
