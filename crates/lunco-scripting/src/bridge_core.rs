//! Language-neutral world bridge — the runtime-agnostic core that lets *any*
//! scripting backend read ECS state and drive the simulation.
//!
//! # Why this exists
//!
//! The verbs a script gets (`cmd` / `get` / `query` / `world_pos` / hierarchy /
//! `emit` / clock) are identical regardless of language. This module owns that
//! logic *once*, free of any interpreter type, so rhai, Python (and a future
//! Lua) are thin bindings over it rather than parallel reimplementations.
//!
//! # Native, not JSON-everywhere
//!
//! Two kinds of boundary, only one inherently JSON:
//!
//! - **Reads** ([`get_field`], [`list_entities`], hierarchy, `world_pos`) read
//!   live reflect data. They build the *native* value in ONE hop via the
//!   [`ValueBuilder`] trait — `reflect → Dynamic` for rhai, `reflect → PyObject`
//!   for Python — never through an intermediate `serde_json::Value`. The
//!   reflect-walker ([`build_from_reflect`]) is written once and monomorphized
//!   per language.
//! - **`cmd` / `query`** route through `ApiCommandEvent` / `ApiQueryRegistry`,
//!   whose params and results are *defined* as `serde_json::Value`. JSON there
//!   is the API's own contract, not a transform we add; results still land in
//!   native values in one pass via [`build_from_json`].
//!
//! # Execution context
//!
//! Reads are synchronous, so the bridge runs inside a `&mut World`. Registered
//! verbs reach it through a scoped thread-local pointer ([`WorldScope`]), valid
//! only for the duration of one evaluation. Single-threaded (FixedUpdate / wasm)
//! and never re-entrant while a borrow is outstanding, so no aliasing occurs.

use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::ecs::system::SystemState;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use std::cell::Cell;

use lunco_api::executor::ApiCommandEvent;
use lunco_api::queries::ApiQueryRegistry;
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::ApiResponse;
use lunco_core::{
    coords, CelestialBody, CelestialClock, CommandOutcome, CommandResults, GlobalEntityId, OpId,
    RoverVessel, Severity, SimTick, TelemetryEvent, TelemetryValue, SECS_PER_TICK,
};

// ── Native value construction ──────────────────────────────────────────────

/// How a scripting backend constructs its native values. Implemented once per
/// language (`RhaiBuilder` → `Dynamic`, `PyBuilder` → `PyObject`); the shared
/// reflect/JSON walkers below are generic over it, so each backend builds
/// natives directly with no intermediate value type.
pub trait ValueBuilder {
    /// The backend's native value type.
    type Value;
    /// The "nothing"/unit value (rhai `()`, Python `None`).
    fn unit(&self) -> Self::Value;
    /// A floating-point number.
    fn float(&self, f: f64) -> Self::Value;
    /// An integer.
    fn int(&self, i: i64) -> Self::Value;
    /// A boolean.
    fn bool(&self, b: bool) -> Self::Value;
    /// A string.
    fn string(&self, s: &str) -> Self::Value;
    /// An ordered array.
    fn array(&self, items: Vec<Self::Value>) -> Self::Value;
    /// A string-keyed map (object).
    fn map(&self, entries: Vec<(String, Self::Value)>) -> Self::Value;
}

/// Convert a reflected value to a backend-native value in one pass.
///
/// glam vectors/quats become arrays (`Vec3` → `[x,y,z]`, `Quat` → `[x,y,z,w]`)
/// so vector math operates on them directly; newtype components (e.g.
/// `LinearVelocity(Vec3)`) unwrap to their inner value; structs become maps;
/// lists/arrays/tuples become arrays. Anything still unconvertible (enums,
/// opaque) falls back to its `Debug` string.
pub fn build_from_reflect<B: ValueBuilder>(
    b: &B,
    value: &dyn bevy::reflect::PartialReflect,
) -> Option<B::Value> {
    use bevy::math::{DQuat, DVec2, Quat, Vec2, Vec3};
    use bevy::reflect::ReflectRef;

    if let Some(reflected) = value.try_as_reflect() {
        let any = reflected.as_any();
        // glam vectors / quats → arrays (the common component-read case).
        if let Some(v) = any.downcast_ref::<Vec3>() {
            return Some(vec3_value(b, v.x as f64, v.y as f64, v.z as f64));
        }
        if let Some(v) = any.downcast_ref::<DVec3>() {
            return Some(vec3_value(b, v.x, v.y, v.z));
        }
        if let Some(v) = any.downcast_ref::<Vec2>() {
            return Some(b.array(vec![b.float(v.x as f64), b.float(v.y as f64)]));
        }
        if let Some(v) = any.downcast_ref::<DVec2>() {
            return Some(b.array(vec![b.float(v.x), b.float(v.y)]));
        }
        if let Some(v) = any.downcast_ref::<Quat>() {
            return Some(b.array(vec![
                b.float(v.x as f64),
                b.float(v.y as f64),
                b.float(v.z as f64),
                b.float(v.w as f64),
            ]));
        }
        if let Some(v) = any.downcast_ref::<DQuat>() {
            return Some(b.array(vec![b.float(v.x), b.float(v.y), b.float(v.z), b.float(v.w)]));
        }
        // scalars
        if let Some(v) = any.downcast_ref::<f64>() {
            return Some(b.float(*v));
        }
        if let Some(v) = any.downcast_ref::<f32>() {
            return Some(b.float(*v as f64));
        }
        if let Some(v) = any.downcast_ref::<i64>() {
            return Some(b.int(*v));
        }
        if let Some(v) = any.downcast_ref::<i32>() {
            return Some(b.int(*v as i64));
        }
        if let Some(v) = any.downcast_ref::<u32>() {
            return Some(b.int(*v as i64));
        }
        if let Some(v) = any.downcast_ref::<u64>() {
            return Some(b.int(*v as i64));
        }
        if let Some(v) = any.downcast_ref::<bool>() {
            return Some(b.bool(*v));
        }
        if let Some(v) = any.downcast_ref::<String>() {
            return Some(b.string(v));
        }
    }

    // Structural fallback: containers → arrays, newtypes unwrap, structs → maps.
    match value.reflect_ref() {
        ReflectRef::List(l) => Some(b.array(l.iter().filter_map(|x| build_from_reflect(b, x)).collect())),
        ReflectRef::Array(a) => Some(b.array(a.iter().filter_map(|x| build_from_reflect(b, x)).collect())),
        ReflectRef::Tuple(t) => Some(b.array(
            t.iter_fields().filter_map(|x| build_from_reflect(b, x)).collect(),
        )),
        ReflectRef::TupleStruct(ts) if ts.field_len() == 1 => {
            ts.field(0).and_then(|f| build_from_reflect(b, f))
        }
        ReflectRef::TupleStruct(ts) => Some(b.array(
            ts.iter_fields().filter_map(|x| build_from_reflect(b, x)).collect(),
        )),
        ReflectRef::Struct(s) => {
            let mut entries = Vec::new();
            for i in 0..s.field_len() {
                if let (Some(name), Some(field)) = (s.name_at(i), s.field_at(i)) {
                    if let Some(v) = build_from_reflect(b, field) {
                        entries.push((name.to_string(), v));
                    }
                }
            }
            Some(b.map(entries))
        }
        _ => Some(b.string(&format!("{value:?}"))),
    }
}

/// Convert a `serde_json::Value` (a `cmd`/`query` result, or telemetry payload)
/// into a backend-native value in one pass. Integers stay integers.
pub fn build_from_json<B: ValueBuilder>(b: &B, v: &serde_json::Value) -> B::Value {
    use serde_json::Value as J;
    match v {
        J::Null => b.unit(),
        J::Bool(x) => b.bool(*x),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                b.int(i)
            } else {
                b.float(n.as_f64().unwrap_or(0.0))
            }
        }
        J::String(s) => b.string(s),
        J::Array(a) => b.array(a.iter().map(|x| build_from_json(b, x)).collect()),
        J::Object(o) => b.map(o.iter().map(|(k, x)| (k.clone(), build_from_json(b, x))).collect()),
    }
}

/// Build a `[x, y, z]` array value.
fn vec3_value<B: ValueBuilder>(b: &B, x: f64, y: f64, z: f64) -> B::Value {
    b.array(vec![b.float(x), b.float(y), b.float(z)])
}

/// The canonical *serialization* [`ValueBuilder`]: constructs `serde_json::Value`.
///
/// Native backends (`RhaiBuilder` → `Dynamic`, future `PyBuilder` → `PyObject`)
/// build their own value types directly; this one is for *output* seams — the
/// HTTP/MCP API, introspection queries — where JSON is the wire format. Building
/// through it keeps the rule "JSON only at the serialization boundary": producers
/// stay generic over `B::Value`, and JSON appears solely because the API layer
/// hands them a `JsonBuilder`. Non-finite floats (NaN/±∞), which JSON can't
/// represent, degrade to `null`.
pub struct JsonBuilder;

impl ValueBuilder for JsonBuilder {
    type Value = serde_json::Value;
    fn unit(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn float(&self, f: f64) -> serde_json::Value {
        serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
    fn int(&self, i: i64) -> serde_json::Value {
        serde_json::Value::Number(i.into())
    }
    fn bool(&self, b: bool) -> serde_json::Value {
        serde_json::Value::Bool(b)
    }
    fn string(&self, s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_string())
    }
    fn array(&self, items: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::Value::Array(items)
    }
    fn map(&self, entries: Vec<(String, serde_json::Value)>) -> serde_json::Value {
        serde_json::Value::Object(entries.into_iter().collect())
    }
}

// ── Scoped World access ─────────────────────────────────────────────────────

thread_local! {
    /// Raw pointer to the World currently being scripted. Non-null only while a
    /// [`WorldScope`] guard is alive.
    static WORLD_PTR: Cell<*mut World> = const { Cell::new(std::ptr::null_mut()) };
}

/// RAII guard that publishes a `&mut World` to the thread-local for the lifetime
/// of a script evaluation, and clears it on drop (even on panic).
pub struct WorldScope;

impl WorldScope {
    /// Publish `world` to the scoped thread-local for the guard's lifetime.
    pub fn enter(world: &mut World) -> Self {
        WORLD_PTR.with(|p| p.set(world as *mut World));
        WorldScope
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        WORLD_PTR.with(|p| p.set(std::ptr::null_mut()));
    }
}

/// Run `f` with the scoped World, or return `None` outside a script evaluation.
///
/// SAFETY: the pointer is only ever set to a live `&mut World` borrow held by an
/// evaluation for the duration of the call; verbs run synchronously and never
/// re-enter while a borrow is outstanding, so the reconstructed `&mut` is unique.
pub fn with_world<R>(f: impl FnOnce(&mut World) -> R) -> Option<R> {
    WORLD_PTR.with(|p| {
        let ptr = p.get();
        if ptr.is_null() {
            None
        } else {
            Some(f(unsafe { &mut *ptr }))
        }
    })
}

fn resolve_entity(world: &World, gid: u64) -> Option<Entity> {
    world
        .get_resource::<ApiEntityRegistry>()?
        .resolve(&GlobalEntityId::from_raw(gid))
}

// ── Verbs: write (cmd) ──────────────────────────────────────────────────────

/// Fire a command by name through `ApiCommandEvent` (the same entry point the
/// HTTP API / MCP use) and return its `{ id, ok, data?, error? }` result as
/// JSON. `params` is the JSON the API contract expects. Runs SYNCHRONOUSLY (the
/// bridge flushes) so `data` carries any values the handler assigned.
pub fn cmd_raw(name: &str, params: serde_json::Value) -> serde_json::Value {
    let id = OpId::new().0;
    with_world(|world| {
        world.trigger(ApiCommandEvent {
            command: name.to_string(),
            params,
            id,
        });
        // The dispatcher defers the real trigger via `commands.queue`; flush so
        // it runs NOW and any result-reporting handler records its Ack under
        // `id` before we read it back.
        world.flush();
        let outcome = world
            .get_resource::<CommandResults>()
            .and_then(|r| r.get(id).cloned());
        command_result_json(id, outcome.as_ref())
    })
    .unwrap_or_else(|| {
        serde_json::json!({ "id": -1, "ok": false, "error": "no world in scope" })
    })
}

/// `cmd` as a native value: fire, then convert the JSON result in one pass.
pub fn cmd<B: ValueBuilder>(b: &B, name: &str, params: serde_json::Value) -> B::Value {
    build_from_json(b, &cmd_raw(name, params))
}

fn command_result_json(id: u64, outcome: Option<&CommandOutcome>) -> serde_json::Value {
    use serde_json::json;
    match outcome {
        Some(CommandOutcome::Succeeded(ack)) => {
            let mut m = json!({ "id": id, "ok": true });
            if !ack.assigned.is_null() {
                m["data"] = ack.assigned.clone();
            }
            m
        }
        Some(CommandOutcome::Failed(msg)) => json!({ "id": id, "ok": false, "error": msg }),
        Some(CommandOutcome::Rejected(reject)) => {
            json!({ "id": id, "ok": false, "error": reject.to_string() })
        }
        // Accepted but no terminal outcome: fire-and-forget / async → success-no-data.
        Some(CommandOutcome::Pending) | None => json!({ "id": id, "ok": true }),
    }
}

// ── Verbs: query ────────────────────────────────────────────────────────────

/// Invoke a registered `ApiQueryProvider` by name; `None` if missing/errored.
pub fn query_raw(name: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    with_world(|world| {
        let provider = world
            .get_resource::<ApiQueryRegistry>()
            .and_then(|reg| reg.get(name))?;
        match provider.execute(world, &params) {
            ApiResponse::Ok { data: Some(data), .. } => Some(data),
            _ => None,
        }
    })
    .flatten()
}

/// `query` as a native value (`unit` if missing/errored).
pub fn query<B: ValueBuilder>(b: &B, name: &str, params: serde_json::Value) -> B::Value {
    match query_raw(name, params) {
        Some(data) => build_from_json(b, &data),
        None => b.unit(),
    }
}

// ── Verbs: reads ────────────────────────────────────────────────────────────

/// `world_pos(id)` — absolute (big_space) world position, or `None`.
pub fn world_pos(gid: u64) -> Option<DVec3> {
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

/// `world_forward(id)` — unit heading in world space, or `None`.
pub fn world_forward(gid: u64) -> Option<DVec3> {
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

/// Split `"Type.field.sub"` into the type's short name and a reflect sub-path
/// (`".field.sub"`). A bare `"Type"` yields an empty sub-path (the whole value).
fn split_type_path(path: &str) -> (&str, String) {
    match path.split_once('.') {
        Some((ty, rest)) => (ty, format!(".{rest}")),
        None => (path, String::new()),
    }
}

/// `get(id, "Component.field")` — generic reflection read as a native value.
pub fn get_field<B: ValueBuilder>(b: &B, gid: u64, path: &str) -> Option<B::Value> {
    let (comp, sub) = split_type_path(path);

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
        build_from_reflect(b, field)
    })
    .flatten()
}

/// `get_setting("Resource.field")` — generic reflection read of a global
/// `Resource` field (the resource twin of [`get_field`]). Settings/config live in
/// resources, not components, so this is how a script reaches them. `None` if the
/// type isn't a registered reflect `Resource`, isn't present, or the path misses.
pub fn get_resource_field<B: ValueBuilder>(b: &B, path: &str) -> Option<B::Value> {
    let (res, sub) = split_type_path(path);

    with_world(|world| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg.get_with_short_type_path(res)?;
        let reflect_resource = registration.data::<ReflectResource>()?;
        let reflected = reflect_resource.reflect(world).ok()?;

        let field: &dyn bevy::reflect::PartialReflect = if sub.is_empty() {
            reflected.as_partial_reflect()
        } else {
            reflected.reflect_path(sub.as_str()).ok()?
        };
        build_from_reflect(b, field)
    })
    .flatten()
}

/// `set(id, "Component.field", value)` — generic reflection WRITE, the mirror of
/// [`get_field`]. Navigates to the live reflected field and hands a `&mut` to
/// `apply`, which writes the backend-native value straight in (`native → reflect`,
/// no JSON) — symmetric with the `reflect → native` read path. The `reflect_mut`
/// borrow trips Bevy change-detection, so the edit replicates / re-runs dependent
/// systems normally. Host-authoritative by construction (scripts run host-only).
pub fn set_component_field(
    gid: u64,
    path: &str,
    apply: impl FnOnce(&mut dyn bevy::reflect::PartialReflect) -> Result<(), String>,
) -> Result<(), String> {
    let (comp, sub) = split_type_path(path);

    with_world(|world| -> Result<(), String> {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg
            .get_with_short_type_path(comp)
            .ok_or_else(|| format!("unknown type '{comp}'"))?;
        let reflect_component = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| format!("'{comp}' is not a Component"))?;
        let entity_mut = world
            .get_entity_mut(entity)
            .map_err(|_| format!("entity {gid} despawned"))?;
        let mut reflected = reflect_component
            .reflect_mut(entity_mut)
            .ok_or_else(|| format!("entity {gid} has no {comp}"))?;
        let field: &mut dyn bevy::reflect::PartialReflect = if sub.is_empty() {
            reflected.as_partial_reflect_mut()
        } else {
            reflected
                .reflect_path_mut(sub.as_str())
                .map_err(|e| format!("no field '{comp}{sub}': {e}"))?
        };
        apply(field)
    })
    .unwrap_or_else(|| Err("no world in scope".into()))
}

/// `set_setting("Resource.field", value)` — generic reflection WRITE to a global
/// `Resource` field (the resource twin of [`set_component_field`]). Same native →
/// reflect application; makes every reflect-registered setting tunable from a
/// script with no per-setting command.
pub fn set_resource_field(
    path: &str,
    apply: impl FnOnce(&mut dyn bevy::reflect::PartialReflect) -> Result<(), String>,
) -> Result<(), String> {
    let (res, sub) = split_type_path(path);

    with_world(|world| -> Result<(), String> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg
            .get_with_short_type_path(res)
            .ok_or_else(|| format!("unknown type '{res}'"))?;
        let reflect_resource = registration
            .data::<ReflectResource>()
            .ok_or_else(|| format!("'{res}' is not a Resource"))?;
        let mut reflected = reflect_resource
            .reflect_mut(world)
            .map_err(|_| format!("resource '{res}' not present"))?;
        let field: &mut dyn bevy::reflect::PartialReflect = if sub.is_empty() {
            reflected.as_partial_reflect_mut()
        } else {
            reflected
                .reflect_path_mut(sub.as_str())
                .map_err(|e| format!("no field '{res}{sub}': {e}"))?
        };
        apply(field)
    })
    .unwrap_or_else(|| Err("no world in scope".into()))
}

/// `list_entities()` — `[{ id, name, type, pos }]` for every registered entity.
pub fn list_entities<B: ValueBuilder>(b: &B) -> B::Value {
    with_world(|world| {
        let pairs = world.resource::<ApiEntityRegistry>().entities();
        // One SystemState carries every per-entity read so the loop never
        // re-borrows the World.
        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
            Query<(Option<&Name>, Option<&RoverVessel>, Option<&CelestialBody>)>,
        )> = SystemState::new(world);
        let (q_parents, q_grids, q_spatial, q_meta) = state.get(world);
        let items = pairs
            .into_iter()
            .map(|(gid, entity)| {
                let (name, rover, body) = q_meta.get(entity).unwrap_or((None, None, None));
                let kind = if rover.is_some() {
                    "rover"
                } else if body.is_some() {
                    "planet"
                } else {
                    "unknown"
                };
                let pos = coords::world_position(entity, &q_parents, &q_grids, &q_spatial)
                    .map(|v| vec3_value(b, v.x, v.y, v.z))
                    .unwrap_or_else(|| b.unit());
                b.map(vec![
                    ("id".to_string(), b.int(gid.get() as i64)),
                    (
                        "name".to_string(),
                        b.string(name.map(|n| n.as_str()).unwrap_or("")),
                    ),
                    ("type".to_string(), b.string(kind)),
                    ("pos".to_string(), pos),
                ])
            })
            .collect();
        b.array(items)
    })
    .unwrap_or_else(|| b.array(Vec::new()))
}

/// `find(name)` — first entity gid with that `Name`, or `-1`.
pub fn find(name: &str) -> i64 {
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

/// `name(id)` — the entity's `Name`, or `None`.
pub fn name_of(gid: u64) -> Option<String> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        world.get::<Name>(entity).map(|n| n.as_str().to_string())
    })
    .flatten()
}

/// `parent(id)` — the parent's gid, or `None` if no parent / parent unregistered.
pub fn parent_of(gid: u64) -> Option<i64> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let parent = world.get::<ChildOf>(entity)?.parent();
        world
            .get_resource::<ApiEntityRegistry>()?
            .api_id_for(parent)
            .map(|g| g.get() as i64)
    })
    .flatten()
}

/// `children(id)` — gids of the entity's direct, registered children.
pub fn children_of(gid: u64) -> Vec<i64> {
    with_world(|world| {
        let Some(entity) = resolve_entity(world, gid) else {
            return Vec::new();
        };
        let Some(children) = world.get::<Children>(entity) else {
            return Vec::new();
        };
        let reg = world.resource::<ApiEntityRegistry>();
        children
            .iter()
            .filter_map(|child| reg.api_id_for(child))
            .map(|g| g.get() as i64)
            .collect()
    })
    .unwrap_or_default()
}

// ── Verbs: clock ────────────────────────────────────────────────────────────

/// `sim_tick()` — current FixedUpdate tick (0 if unavailable).
pub fn sim_tick() -> i64 {
    with_world(|w| w.get_resource::<SimTick>().map(|t| t.0 as i64))
        .flatten()
        .unwrap_or(0)
}

/// `dt()` — fixed-step integration delta in seconds (falls back to SECS_PER_TICK).
pub fn dt() -> f64 {
    with_world(|w| {
        w.get_resource::<Time<bevy::time::Fixed>>()
            .map(|t| t.delta_secs_f64())
    })
    .flatten()
    .filter(|d| *d > 0.0)
    .unwrap_or(SECS_PER_TICK)
}

/// `elapsed_seconds()` — monotonic simulation seconds since startup (0.0 if none).
pub fn elapsed_seconds() -> f64 {
    with_world(|w| {
        w.get_resource::<Time<bevy::time::Fixed>>()
            .map(|t| t.elapsed_secs_f64())
    })
    .flatten()
    .unwrap_or(0.0)
}

// ── Verbs: events ───────────────────────────────────────────────────────────

/// `emit(name, value)` — fire a `TelemetryEvent` on the shared bus. The scalar
/// payload is taken from a JSON value (the native→JSON projection at the call
/// site is trivial for scalars). Returns whether a World was in scope.
pub fn emit(name: &str, value: TelemetryValue) -> bool {
    with_world(|world| {
        let timestamp = world
            .get_resource::<CelestialClock>()
            .map(|c| c.epoch)
            .unwrap_or(0.0);
        world.trigger(TelemetryEvent {
            name: name.to_string(),
            severity: Severity::Info,
            data: value,
            timestamp,
        });
    })
    .is_some()
}

/// Build the `{ name, value, severity, timestamp }` event value passed to an
/// `on_event` hook, native to the backend.
pub fn build_event<B: ValueBuilder>(b: &B, ev: &TelemetryEvent) -> B::Value {
    b.map(vec![
        ("name".to_string(), b.string(&ev.name)),
        ("value".to_string(), telemetry_value(b, &ev.data)),
        ("severity".to_string(), b.string(&format!("{:?}", ev.severity))),
        ("timestamp".to_string(), b.float(ev.timestamp)),
    ])
}

/// A `TelemetryValue` as a backend-native scalar value.
pub fn telemetry_value<B: ValueBuilder>(b: &B, v: &TelemetryValue) -> B::Value {
    match v {
        TelemetryValue::F64(x) => b.float(*x),
        TelemetryValue::I64(x) => b.int(*x),
        TelemetryValue::Bool(x) => b.bool(*x),
        TelemetryValue::String(x) => b.string(x),
    }
}
