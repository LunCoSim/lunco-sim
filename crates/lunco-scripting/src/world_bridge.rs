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

use std::sync::Arc;

use lunco_core::{Ack, CommandResults, OpId, TelemetryEvent, TelemetryValue};
use lunco_hash::fnv1a64;

use rhai::{Dynamic, Engine, FnPtr, ImmutableString, Map, AST};

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

/// Whether scenarios run in DEBUG mode (autopilots on, verbose narration, …).
/// Defaults to the build profile; `LUNCO_SCENARIO_DEBUG=1|0` (or true/false)
/// overrides at runtime with no rebuild. Backs the rhai `is_debug()`/`env()` verbs.
///
/// Resolved ONCE and cached in a `OnceLock`: the cfg check + env-var read happen
/// on the first call only, so a scenario polling `is_debug()` every tick pays just
/// an atomic load, not an allocation/env lookup per frame. (The environment is
/// fixed at launch, so caching is correct — "dynamic" means no rebuild, not
/// per-frame mutation.)
fn scenario_debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| match std::env::var("LUNCO_SCENARIO_DEBUG").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => cfg!(debug_assertions),
    })
}

/// Compile the split prelude into one AST (per-file compile + `AST::merge`).
///
/// The prelude is the ergonomic policy layer (drive/distance/arrived/nav/HUD/…)
/// authored in rhai. Its topic files live under `assets/scripting/prelude/` and are
/// embedded + enumerated by [`lunco_assets::scripting::prelude_files`] (the
/// asset-owning crate) — sorted by stem for a deterministic merge, the files
/// being pure `fn` definitions so order is semantically irrelevant. Flat
/// namespace + embedded, identical to compiling one concatenated string, but a
/// syntax error is logged with the offending file's name and a position relative
/// to that file (error locality). Both prelude uses — the global module and the
/// per-scenario `prelude_ast` merge — go through here.
pub(crate) fn compile_prelude(engine: &Engine) -> Result<AST, rhai::ParseError> {
    // Disk-first (edit -> restart, no rebuild); a disk prelude that fails to
    // parse falls back to the EMBEDDED copy so a broken helper edit degrades
    // to stock behaviour instead of bricking startup. The embedded copy is
    // parse-tested in CI, so its failure is a real bug worth propagating.
    match compile_prelude_set(engine, lunco_assets::scripting::prelude_files()) {
        Ok(ast) => Ok(ast),
        Err(e) => {
            error!(
                "[rhai] prelude failed to parse — falling back to the embedded prelude \
                 (fix assets/scripting/prelude/ and restart): {e}"
            );
            compile_prelude_set(engine, lunco_assets::scripting::embedded_prelude_files())
        }
    }
}

fn compile_prelude_set(
    engine: &Engine,
    files: Vec<(String, String)>,
) -> Result<AST, rhai::ParseError> {
    let mut acc: Option<AST> = None;
    for (name, src) in files {
        match engine.compile(&src) {
            Ok(part) => acc = Some(match acc {
                Some(a) => a.merge(&part),
                None => part,
            }),
            Err(e) => {
                error!("[rhai] prelude/{name}.rhai failed to parse: {e}");
                return Err(e);
            }
        }
    }
    Ok(acc.expect("prelude set is non-empty"))
}

/// Build a rhai [`Engine`] with the World-bridge verbs registered, the embedded
/// prelude loaded as a global module, and the same sandbox caps as the one-shot
/// backend.
pub fn build_world_engine() -> Engine {
    let mut engine = Engine::new();

    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);
    // Above rhai's defaults (64 global / 32 in functions): task-tree literals
    // nest composites (e.g. par_all → seq → leaf) as one map expression. The
    // tick recursion itself is native now (task_tree.rs), so this only widens
    // the structural nesting allowance for authored trees.
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

    // world_rotation(id) -> [x, y, z, w] world orientation quaternion, or ().
    // The general orientation accessor: up/forward/right are `quat * axis`,
    // derived in the rhai prelude (see `world_up`/`world_right` helpers), so this
    // one host fn covers every axis and feeds tilt/tip-over checks — no per-axis
    // Rust. Same GlobalTransform source as world_forward.
    engine.register_fn("world_rotation", |id: i64| -> Dynamic {
        match bridge_core::world_rotation(id as u64) {
            Some(q) => RhaiBuilder.array(vec![
                Dynamic::from_float(q[0]),
                Dynamic::from_float(q[1]),
                Dynamic::from_float(q[2]),
                Dynamic::from_float(q[3]),
            ]),
            None => Dynamic::UNIT,
        }
    });

    // register_hook(id, entry, src) -> bool — plug a rhai rule into ANY Rust
    // policy seam (lunco-hooks) from a scenario: merge policies, RBAC,
    // control-authority takeover, comms link availability
    // ("comms.link.connected"), … Replaces the previously-registered hook for
    // that id (the built-in `assets/scripting/policy/*` rules are just earlier
    // registrations), so a scenario re-shapes policy live, no rebuild — the
    // doc-37 §8 "policy = rhai" surface. `src` must define `fn <entry>(...)`;
    // returns false (and logs why) on a compile error.
    engine.register_fn(
        "register_hook",
        |id: ImmutableString, entry: ImmutableString, src: ImmutableString| -> bool {
            match lunco_hooks_rhai::register_rhai_hook(
                id.as_str(),
                entry.as_str(),
                src.as_str(),
                false,
            ) {
                Ok(_) => {
                    bevy::log::info!("[rhai] register_hook: '{id}' → {entry}()");
                    true
                }
                Err(e) => {
                    bevy::log::warn!("[rhai] register_hook '{id}' failed to compile: {e}");
                    false
                }
            }
        },
    );

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

    // ── Control ownership (who drives this vessel — human OR autopilot) ───────
    // owner_of(id) -> session id (i64) currently controlling the vessel (0 = local
    // human, the autopilot band for an AI), or () if nobody owns it. Reads the
    // possession arbiter's ownership, so a scenario can branch on whether a rover is
    // driven — uniformly across a human and an autopilot (which is just a user with
    // a specialty).
    engine.register_fn("owner_of", |id: i64| -> Dynamic {
        bridge_core::owner_of(id as u64).map(|s| Dynamic::from_int(s as i64)).unwrap_or(Dynamic::UNIT)
    });
    // controller(id) -> role string of the driver ("AiAgent" = autopilot, "Owner"/
    // "Operator" = human), or () if unowned. The human-vs-AI test.
    engine.register_fn("controller", |id: i64| -> Dynamic {
        bridge_core::controller_role(id as u64).map(Dynamic::from).unwrap_or(Dynamic::UNIT)
    });
    // is_controlled(id) -> bool — true if any session (human or autopilot) drives it.
    engine.register_fn("is_controlled", |id: i64| -> bool {
        bridge_core::owner_of(id as u64).is_some()
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

    // subscribe(name) — call in on_start to receive ONLY the named events in
    // on_event (default with no subscribe = all events). An optimisation: skips
    // the per-event VM entry for events you don't name. Footgun — a name you
    // forget won't reach on_event; when unsure, don't subscribe (you get all).
    engine.register_fn("subscribe", |name: ImmutableString| {
        SUBS_ACCUM.with(|s| {
            if let Some(a) = s.borrow_mut().as_mut() {
                a.exact.push(name.into());
            }
        });
    });
    // subscribe_prefix(pfx) — receive every event whose name starts with `pfx`
    // (e.g. "enter:" for all zone-enters). Same on_start-only semantics.
    engine.register_fn("subscribe_prefix", |pfx: ImmutableString| {
        SUBS_ACCUM.with(|s| {
            if let Some(a) = s.borrow_mut().as_mut() {
                a.prefixes.push(pfx.into());
            }
        });
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

    // is_debug() -> bool / env(key) -> bool — the SCENARIO ENVIRONMENT, so a
    // script can branch on it: `if is_debug() { autopilot() }` runs an autopilot in
    // debug and lets a human play in release. Defaults to the BUILD PROFILE
    // (`cfg!(debug_assertions)` — true under `cargo run`, false under `--release`),
    // overridable AT RUNTIME with no rebuild via `LUNCO_SCENARIO_DEBUG=1|0`, so a
    // release build can force-enable a debug scenario and vice-versa. Callable from
    // any function (verbs are global, unlike the `params`/`env` scope constants).
    engine.register_fn("is_debug", || -> bool { scenario_debug() });
    engine.register_fn("env", |key: &str| -> bool {
        match key {
            "debug" => scenario_debug(),
            "release" => !scenario_debug(),
            _ => false,
        }
    });

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
    match compile_prelude(&engine) {
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

/// Which lifecycle hooks a program defines — **structure**, derived from the AST
/// once at compile and cached with it. Replaces the per-call
/// `ast.iter_functions().any(...)` scan: the driver consults these bits before
/// entering the VM, so an absent hook (esp. `on_event`, checked per *event*) is a
/// cheap bool test, not an AST walk + a wasted `call_fn`.
#[derive(Clone, Copy, Debug, Default)]
struct ProgramMask {
    start: bool,
    tick: bool,
    stop: bool,
    event: bool,
}

impl ProgramMask {
    fn from_ast(ast: &AST) -> Self {
        let mut m = ProgramMask::default();
        for f in ast.iter_functions() {
            match (f.name, f.params.len()) {
                ("on_start", 1) => m.start = true,
                ("on_tick", 1) => m.tick = true,
                ("on_stop", 1) => m.stop = true,
                ("on_event", 2) => m.event = true,
                _ => {}
            }
        }
        m
    }

    /// Whether the one-arg lifecycle hook for `hook` is defined.
    fn has(&self, hook: crate::scenario::ScenarioHook) -> bool {
        match hook {
            crate::scenario::ScenarioHook::Start => self.start,
            crate::scenario::ScenarioHook::Tick => self.tick,
            crate::scenario::ScenarioHook::Stop => self.stop,
        }
    }

    /// The hook names present, in the driver's dispatch order — for `ScriptInspect`.
    fn hook_names(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.start {
            v.push("on_start".into());
        }
        if self.tick {
            v.push("on_tick".into());
        }
        if self.stop {
            v.push("on_stop".into());
        }
        if self.event {
            v.push("on_event".into());
        }
        v
    }
}

/// A compiled scenario program: the prelude-merged `AST` + its derived hook mask.
/// **Pure structure** — a function of `source` alone, so identical sources share
/// one `Arc` across every entity and every relaunch (content-addressed by
/// `fnv1a64(source)` in [`RhaiScenarioRuntime::compiled`]). Carries **no**
/// per-instance state: the `scope` (const globals, seeded by a world-touching
/// top-level run) and `this` map live in [`RhaiScenarioState`] and are never
/// shared or cached — that firewall is what keeps the memo determinism-safe.
struct CompiledProgram {
    ast: AST,
    mask: ProgramMask,
}

/// A memoized compile outcome: a shared program, or the diagnostic a bad source
/// produced (cached so a fleet sharing one broken source parses + logs once).
enum CacheEntry {
    Ok(Arc<CompiledProgram>),
    Err(Diagnostic),
}

/// Soft cap on the compile memo (see [`RhaiScenarioRuntime::compiled`]). At the
/// limit the map is cleared; the working set of distinct scenario sources is far
/// below this, so a clear is rare and only costs a cold re-parse next compile.
const COMPILED_CACHE_CAP: usize = 512;

/// Which events a scenario wants delivered to its `on_event` — **per-entity
/// state** (set at runtime in `on_start` via `subscribe`/`subscribe_prefix`), so
/// it is NOT part of the shared [`CompiledProgram`]. Default [`All`] = every
/// event (behaviour-identical to no filter). A scenario that subscribes trades a
/// tiny footgun (forget a name ⇒ that event skips its `on_event`) for skipping
/// the VM entry on every event it doesn't care about — the P2 optimisation, opt-in.
#[derive(Clone, Debug)]
enum EventFilter {
    /// No filter — deliver every event (the default; forgetting to subscribe is
    /// safe, it just means "all", never a silent drop).
    All,
    /// Only events whose name is in `exact` or starts with a `prefixes` entry
    /// (e.g. `subscribe_prefix("enter:")` for every zone-enter).
    Named {
        exact: std::collections::HashSet<String>,
        prefixes: Vec<String>,
    },
}

impl Default for EventFilter {
    fn default() -> Self {
        EventFilter::All
    }
}

impl EventFilter {
    fn matches(&self, name: &str) -> bool {
        match self {
            EventFilter::All => true,
            EventFilter::Named { exact, prefixes } => {
                exact.contains(name) || prefixes.iter().any(|p| name.starts_with(p.as_str()))
            }
        }
    }
}

/// Accumulates `subscribe()` calls made during the CURRENT `on_start` (see
/// [`SUBS_ACCUM`]); harvested into the entity's [`EventFilter`] right after.
#[derive(Default)]
struct SubsAccum {
    exact: Vec<String>,
    prefixes: Vec<String>,
}

impl SubsAccum {
    fn into_filter(self) -> EventFilter {
        if self.exact.is_empty() && self.prefixes.is_empty() {
            EventFilter::All
        } else {
            EventFilter::Named { exact: self.exact.into_iter().collect(), prefixes: self.prefixes }
        }
    }
}

thread_local! {
    /// Subscription accumulator for the `on_start` currently executing. The
    /// driver arms it (`Some(default)`) before `on_start` and harvests it after;
    /// `subscribe`/`subscribe_prefix` verbs push into it. `None` outside an
    /// `on_start` window, so a stray `subscribe()` in `on_tick` is a harmless
    /// no-op (documented: subscribe in `on_start`). Single-threaded — the
    /// scenario driver is an exclusive system.
    static SUBS_ACCUM: std::cell::RefCell<Option<SubsAccum>> =
        const { std::cell::RefCell::new(None) };
}

/// Per-entity live state + a handle to the shared compiled program.
struct RhaiScenarioState {
    /// Shared, content-addressed compiled program (AST + hook mask).
    program: Arc<CompiledProgram>,
    /// Top-level `const` globals, populated by running the body once at compile.
    scope: rhai::Scope<'static>,
    /// Per-entity mutable state bound as `this` in every hook.
    this: Dynamic,
    /// Which events reach this entity's `on_event` (default: all). Set from
    /// `subscribe()` calls harvested after `on_start`.
    filter: EventFilter,
    /// The `this.task` map compiled onto the [`lunco_behavior`] kernel (the
    /// native replacement for the prelude's retired `__tick*` engine). Runtime
    /// tick state (cursors, dwell stamps) lives HERE, not in the map — the map
    /// stays the pristine spec.
    task: Option<crate::task_tree::CompiledTask>,
    /// Events buffered for the task's `wait_for` leaves since the last tick
    /// (`(name, emitter-gid)`), drained by [`tick_native_task`] — the native
    /// replacement for the retired `this.__events` buffer.
    task_events: Vec<(ImmutableString, i64)>,
}

/// The rhai [`ScenarioRuntime`](crate::scenario::ScenarioRuntime): one
/// bridge-enabled engine + a per-entity program cache. Wrapped by
/// `ScenarioDriver<RhaiScenarioRuntime>` (which owns the neutral lifecycle FSM).
pub struct RhaiScenarioRuntime {
    /// `Arc` so the native task-tree ctx (which must be `'static` — see
    /// `task_tree::TaskCtx`) can own a handle for closure calls; the runtime
    /// is single-threaded per access, so `Arc::get_mut` for tool hot-reload
    /// always succeeds (no ctx outlives its tick).
    engine: std::sync::Arc<Engine>,
    states: std::collections::HashMap<Entity, RhaiScenarioState>,
    /// Content-addressed memo of compile *outcomes*, keyed by `fnv1a64(source)`.
    /// The compile step (parse + prelude-merge + mask derive) is pure structure,
    /// so identical sources — every rover with the same controller, every replay
    /// of a tutorial — reuse one `Arc` instead of re-parsing. Failures are cached
    /// too, so a fleet sharing a broken source parses + logs once, not per entity.
    /// A source edit bumps the doc generation → new source → new key → a fresh
    /// entry. Not keyed on tool-lib/prelude generation: those affect the engine's
    /// *runtime* module resolution, not the AST parse, so a cached AST stays valid.
    ///
    /// Bounded, not GC'd: entries are retained for cross-entity/replay reuse (even
    /// after an entity despawns), so an authoring session that edits a script many
    /// times would grow this unboundedly. [`COMPILED_CACHE_CAP`] caps it — at the
    /// limit the whole map is cleared (a cold rebuild on the next compile, cheap
    /// since the working set is small). A finer LRU is deferred until measured.
    compiled: std::collections::HashMap<u64, CacheEntry>,
    /// The prelude compiled to an `AST`, merged into every scenario's AST so its
    /// helpers — including the engine-driven `__init_task` / `__note_task_event`
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
        let prelude_ast =
            compile_prelude(&engine).unwrap_or_else(|e| panic!("prelude must compile: {e}"));
        Self {
            engine: std::sync::Arc::new(engine),
            states: std::collections::HashMap::new(),
            compiled: std::collections::HashMap::new(),
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
        // ── Structure: the compiled program (parse + prelude-merge + hook mask)
        // is a pure function of `source`. Content-address it by `fnv1a64(source)`
        // and reuse the `Arc` across every entity/replay; parse only on a miss.
        let key = fnv1a64(source.as_bytes());
        let program = match self.compiled.get(&key) {
            Some(CacheEntry::Ok(p)) => p.clone(),
            // Cached failure: return the same diagnostic without re-parsing or
            // re-logging (each entity still surfaces its own DocumentDiagnostics).
            Some(CacheEntry::Err(d)) => return CompileOutcome::Failed(d.clone()),
            None => {
                // Bound the memo before inserting (see COMPILED_CACHE_CAP).
                if self.compiled.len() >= COMPILED_CACHE_CAP {
                    self.compiled.clear();
                }
                match self.engine.compile(source) {
                    Ok(ast) => {
                        // Merge the prelude's functions into the scenario AST so
                        // the engine-driven `__init_task`/`__note_task_event` (and
                        // every other prelude helper) are resolvable by `call_fn`.
                        // Merging prelude←user lets a user function win on any
                        // name/arity clash; the prelude has no top-level body, so
                        // the seed-run below still executes only the user's top level.
                        let ast = self.prelude_ast.merge(&ast);
                        let mask = ProgramMask::from_ast(&ast);
                        let p = Arc::new(CompiledProgram { ast, mask });
                        self.compiled.insert(key, CacheEntry::Ok(p.clone()));
                        p
                    }
                    Err(e) => {
                        error!("[rhai] entity {entity:?} compile error: {e}");
                        let d = rhai_diagnostic(e.to_string(), e.position());
                        self.compiled.insert(key, CacheEntry::Err(d.clone()));
                        return CompileOutcome::Failed(d);
                    }
                }
            },
        };

        // ── State: seed a FRESH scope + `this` for THIS entity — never shared.
        // The top-level seed-run touches the world and depends on `params`, so it
        // is per-instance; this is the firewall that lets the AST above be shared.
        let mut scope = rhai::Scope::new();
        // Expose scenario parameters as a read-only `params` constant (native
        // JSON→Dynamic, one hop). Empty / bad JSON → empty map, so `params` is
        // always a readable object.
        let params_value = match (params.is_empty(), serde_json::from_str::<serde_json::Value>(params)) {
            (true, _) => RhaiBuilder.map(Vec::new()),
            (false, Ok(v)) => bridge_core::build_from_json(&RhaiBuilder, &v),
            (false, Err(e)) => {
                warn!("[rhai] entity {entity:?} ignoring bad params JSON: {e}");
                RhaiBuilder.map(Vec::new())
            }
        };
        scope.push_constant_dynamic("params", params_value);
        // Run the top-level body once to seed `const` globals; a runtime error
        // there is non-fatal (hooks still run) — surface it.
        let top_level = match self.engine.run_ast_with_scope(&mut scope, &program.ast) {
            Ok(()) => None,
            Err(e) => {
                error!("[rhai] entity {entity:?} top-level failed: {e}");
                Some(rhai_diagnostic(e.to_string(), e.position()))
            }
        };
        self.states.insert(
            entity,
            RhaiScenarioState {
                program,
                scope,
                this: Dynamic::from_map(Map::new()),
                filter: EventFilter::default(),
                task: None,
                task_events: Vec::new(),
            },
        );
        CompileOutcome::Ready { top_level }
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
        // Arm the subscription accumulator for `on_start`, so `subscribe()` calls
        // made during it are captured (harvested into `st.filter` below). Left
        // `None` for other hooks → a stray `subscribe()` outside `on_start` is a
        // harmless no-op.
        if matches!(hook, ScenarioHook::Start) {
            SUBS_ACCUM.with(|s| *s.borrow_mut() = Some(SubsAccum::default()));
        }
        // Seed the deterministic RNG for this hook: (entity, tick, hook).
        bridge_core::rng_begin(self_gid as u64, bridge_core::sim_tick() as u64, salt);
        // Only enter the VM if the program defines this lifecycle hook (cached
        // mask bit — no AST scan). The built-in drivers below run regardless.
        let user = if st.program.mask.has(hook) {
            call_hook(&self.engine, &mut st.scope, &st.program.ast, name, self_gid, &mut st.this)
        } else {
            None
        };
        // Built-in drivers (prelude fns, called regardless of what the user AST
        // defines): after on_start, seed `this.task`/`this.mission` from `task(me)`
        // / `mission(me)` fns if present; after on_tick, advance the declared task
        // and evaluate the mission. Each no-ops when the script declared neither,
        // so a plain scenario pays only a couple of cheap calls.
        let drivers: &[&str] = match hook {
            ScenarioHook::Start => &["__init_task", "__init_mission"],
            // `__run_task` is retired: `this.task` ticks natively on the
            // lunco-behavior kernel (see `tick_native_task`), same slot in the
            // order (task advances before the mission evaluates).
            ScenarioHook::Tick => &["__run_mission"],
            ScenarioHook::Stop => &[],
        };
        let mut driver_err = None;
        if matches!(hook, ScenarioHook::Tick) {
            driver_err = tick_native_task(&self.engine, st, self_gid);
        }
        for name in drivers {
            let e = call_prelude_driver(
                &self.engine,
                &mut st.scope,
                &st.program.ast,
                name,
                &mut st.this,
                vec![Dynamic::from_int(self_gid)],
            );
            driver_err = driver_err.or(e);
        }
        // Harvest subscriptions declared during `on_start` into the entity's
        // event filter (no calls → stays `EventFilter::All`). Always take() to
        // clear the accumulator, but only APPLY it when `on_start` ran clean:
        // a hook that errored partway may have registered only some of its
        // `subscribe()` calls, and a partial `Named` filter would silently drop
        // every unnamed event for the entity's life. On error, leave filter = All.
        if matches!(hook, ScenarioHook::Start) {
            let accum = SUBS_ACCUM.with(|s| s.borrow_mut().take());
            if user.is_none() {
                if let Some(accum) = accum {
                    st.filter = accum.into_filter();
                }
            }
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
        // Buffer for the native task tree's `wait_for` leaves (drained every
        // tick by `tick_native_task`). Capped defensively: a scenario that stops
        // ticking while events keep arriving must not grow this unboundedly.
        st.task_events.push((event.name.as_str().into(), event.source as i64));
        if st.task_events.len() > 256 {
            let excess = st.task_events.len() - 256;
            st.task_events.drain(..excess);
        }
        // Per-EVENT hot path: only enter the VM for the user hook if the program
        // defines `on_event` (cached mask bit — no AST scan) AND this event passes
        // the entity's subscription filter (default: all). Either fails → skip the
        // call entirely; the built-in task driver below still sees every event.
        let user = if st.program.mask.event && st.filter.matches(&event.name) {
            call_event_hook(&self.engine, &mut st.scope, &st.program.ast, self_gid, &mut st.this, evt.clone())
        } else {
            None
        };
        // Feed the event into the BUILT-IN task too, so `wait_for(name)` steps in a
        // `this.task` complete without a hand-written on_event. No-op if no task.
        let task = call_prelude_driver(
            &self.engine,
            &mut st.scope,
            &st.program.ast,
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
        // Report only the lifecycle hooks the program defines — straight from the
        // cached mask (derived at compile), no AST re-scan.
        let hooks = st.program.mask.hook_names();
        Some(crate::scenario::ScenarioSnapshot { state, hooks })
    }

    fn maintain(&mut self) {
        // Hot-reload tool libraries if any were (re)registered since last pass.
        let cur = crate::tool_libs::generation();
        if self.tool_gen != cur {
            // No task ctx is alive between ticks, so the Arc is unique here.
            let engine = std::sync::Arc::get_mut(&mut self.engine)
                .expect("engine Arc must be unique outside a task tick");
            crate::tool_libs::refresh(engine);
            self.tool_gen = cur;
        }
    }
}

/// Exclusive system (FixedUpdate): drive every `ScriptedModel { Rhai }` through
/// its lifecycle via the neutral [`crate::scenario::ScenarioDriver`].
pub fn tick_rhai_scenarios(world: &mut World) {
    crate::scenario::ScenarioDriver::<RhaiScenarioRuntime>::run(world, ScriptLanguage::Rhai);
}

/// Call a one-arg hook (`fn name(self)`), binding `this` to the entity's
/// persistent state map. The caller guarantees the hook exists (via the cached
/// `ProgramMask`), so this does no presence check. `eval_ast=false` so only the
/// function runs (top-level already ran at compile); `rewind_scope=false` keeps
/// the `const` globals available across calls. Logs any error.
fn call_hook(
    engine: &Engine,
    scope: &mut rhai::Scope,
    ast: &AST,
    name: &str,
    self_id: i64,
    this: &mut Dynamic,
) -> Option<(String, rhai::Position)> {
    // Presence is the caller's responsibility — the driver gates on the cached
    // `ProgramMask`, so there's no per-call AST scan here.
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
    // Presence is the caller's responsibility now — `deliver_event` gates this on
    // the cached `ProgramMask::event` bit, so no per-event AST scan here.
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
/// One tick's world access for the native task tree: leaves call script
/// closures through the entity's own program AST (closures carry their
/// captures by currying — same visibility the retired rhai engine gave them;
/// named-fn pointers resolve against the merged prelude+user AST).
struct RhaiTaskCtx {
    engine: std::sync::Arc<Engine>,
    program: Arc<CompiledProgram>,
    /// Host gid, passed to every leaf closure (the `|m| …` argument).
    me: i64,
    now: f64,
    events: Vec<(ImmutableString, i64)>,
    /// First closure error this tick (leaves keep `Running`; the error surfaces
    /// once as the hook's diagnostic — the retired engine aborted-and-retried
    /// the same way).
    error: Option<(String, rhai::Position)>,
}

impl RhaiTaskCtx {
    fn note(&mut self, e: Box<rhai::EvalAltResult>) {
        error!("[rhai] task leaf failed: {e}");
        if self.error.is_none() {
            self.error = Some((e.to_string(), e.position()));
        }
    }
}

impl crate::task_tree::TaskCtx for RhaiTaskCtx {
    fn now(&self) -> f64 {
        self.now
    }
    fn events(&self) -> &[(ImmutableString, i64)] {
        &self.events
    }
    fn resolve(&mut self, path: &str) -> i64 {
        bridge_core::find(path)
    }
    fn call_action(&mut self, f: &FnPtr) -> Result<(), ()> {
        match f.call::<Dynamic>(&self.engine, &self.program.ast, (self.me,)) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.note(e);
                Err(())
            }
        }
    }
    fn call_pred(&mut self, f: &FnPtr) -> Result<bool, ()> {
        match f.call::<Dynamic>(&self.engine, &self.program.ast, (self.me,)) {
            Ok(d) => d.as_bool().map_err(|t| {
                error!("[rhai] task predicate returned `{t}`, expected bool");
                if self.error.is_none() {
                    self.error = Some((
                        format!("task predicate must return a bool, got `{t}`"),
                        rhai::Position::NONE,
                    ));
                }
            }),
            Err(e) => {
                self.note(e);
                Err(())
            }
        }
    }
}

/// What `tick_native_task`'s spec inspection decided under the `this` borrow.
enum TaskPlan {
    /// No `this.task` declared (or it was cleared) — drop any compiled tree.
    None,
    /// The compiled tree matches the current spec — just tick it.
    Have,
    /// A new/re-assigned spec (no `__bt` marker matching): compile this clone
    /// under the given identity.
    Compile(Dynamic, i64),
}

/// Advance `this.task` one tick on the [`lunco_behavior`] kernel — the native
/// replacement for the prelude's retired `__run_task` rhai driver. The map in
/// `this.task` is the pristine SPEC (constructors build pure data); runtime
/// state lives in the compiled tree on [`RhaiScenarioState`]. A fresh
/// assignment is detected by the `__bt` identity marker this function stamps
/// into the map after compiling (a rhai re-assignment always produces an
/// unmarked map). Emits `TASK_COMPLETE` once on root success (parity with the
/// retired engine) and `TASK_FAILED` on root failure (new — only reachable via
/// the new `check`/`sel`/`retry` vocabulary).
fn tick_native_task(
    engine: &std::sync::Arc<Engine>,
    st: &mut RhaiScenarioState,
    self_gid: i64,
) -> Option<(String, rhai::Position)> {
    // Drain unconditionally so the buffer can't accumulate while no task runs.
    let events = std::mem::take(&mut st.task_events);

    static NEXT_TASK_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

    // Inspect the spec (and stamp a new marker) inside one `this` borrow.
    let plan = {
        let Some(mut this_map) = st.this.write_lock::<Map>() else {
            return None;
        };
        match this_map.get_mut("task") {
            None => TaskPlan::None,
            Some(v) if v.is_unit() => TaskPlan::None,
            Some(v) => {
                let marked = v
                    .read_lock::<Map>()
                    .and_then(|m| m.get("__bt").and_then(|d| d.as_int().ok()));
                match (marked, st.task.as_ref()) {
                    (Some(id), Some(ct)) if ct.id == id => TaskPlan::Have,
                    _ => {
                        let id = NEXT_TASK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let spec = v.clone();
                        if let Some(mut m) = v.write_lock::<Map>() {
                            m.insert("__bt".into(), Dynamic::from_int(id));
                        }
                        TaskPlan::Compile(spec, id)
                    }
                }
            }
        }
    };

    match plan {
        TaskPlan::None => {
            st.task = None;
            return None;
        }
        TaskPlan::Have => {}
        TaskPlan::Compile(spec, id) => match crate::task_tree::compile_node(&spec) {
            Ok(tree) => st.task = Some(crate::task_tree::CompiledTask::new(id, tree)),
            Err(msg) => {
                error!("[rhai] this.task does not compile: {msg}");
                // Poisoned (latched done) so the error reports once, not every tick.
                st.task = Some(crate::task_tree::CompiledTask::poisoned(id));
                return Some((format!("task tree invalid: {msg}"), rhai::Position::NONE));
            }
        },
    }

    let program = st.program.clone();
    let Some(ct) = st.task.as_mut() else { return None };
    if ct.done {
        return None;
    }
    let mut ctx = RhaiTaskCtx {
        engine: engine.clone(),
        program,
        me: self_gid,
        now: bridge_core::elapsed_seconds(),
        events,
        error: None,
    };
    let status = ct.tree.tick(&mut ctx);
    match status {
        lunco_behavior::Status::Running => {}
        lunco_behavior::Status::Success => {
            ct.done = true;
            bridge_core::emit("TASK_COMPLETE", TelemetryValue::I64(0));
        }
        lunco_behavior::Status::Failure => {
            ct.done = true;
            warn!("[rhai] task tree ended in Failure for gid {self_gid}");
            bridge_core::emit("TASK_FAILED", TelemetryValue::I64(0));
        }
    }
    ctx.error
}

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
        // Use the SAME raised expr-depth the scenario engine uses at runtime — the
        // prelude's sequencer legitimately needs it; a stock engine's lower default
        // rejects it (this test used to fail on that pre-existing mismatch).
        let mut engine = rhai::Engine::new();
        engine.set_max_expr_depths(128, 128);
        super::compile_prelude(&engine).expect("prelude must parse");

        // Every embedded example scenario, built-in tool library, AND bundled
        // runtime scenario must parse — all enumerated from the asset-owning
        // crate, so new files are covered automatically (no hand-kept list here).
        // The bundled scenarios include the lander auto-land GUIDANCE, so a
        // syntax slip can't silently disable auto-land at scene load.
        let examples = lunco_assets::scripting::examples();
        let tools = lunco_assets::scripting::tool_libraries();
        let scenarios = lunco_assets::scripting::scenarios();
        assert!(
            !examples.is_empty() && !tools.is_empty() && !scenarios.is_empty(),
            "embedded scripting assets empty"
        );
        for (name, src) in examples.into_iter().chain(tools).chain(scenarios) {
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
