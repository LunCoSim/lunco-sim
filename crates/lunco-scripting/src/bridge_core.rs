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

use lunco_api::executor::{authz_target_gid, ApiCommandEvent};
use lunco_api::queries::ApiQueryRegistry;
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::ApiResponse;
use lunco_core::session::{authorize, CommandPolicyRegistry, SessionRbac, SessionRegistry};
use lunco_core::{
    coords, CelestialBody, CommandOutcome, CommandResults, GlobalEntityId, OpId,
    Severity, SessionId, SimTick, TelemetryEvent, TelemetryValue, SECS_PER_TICK,
};
use lunco_fsw::FlightSoftware;

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

    /// The session a running script acts on behalf of — the authority its
    /// [`cmd`] calls are gated against (design §3.4). `Some` only for a script
    /// launched by a *remote* networked session (captured at launch from the
    /// wire origin); `None` for a local / host-trusted launch (single-player,
    /// standalone, USD-embedded), where `cmd` stays ungated. Set per-entity by
    /// the scenario driver and per-eval by the `RunRhai` drain; reset to `None`
    /// whenever a [`WorldScope`] enters or drops, so it never leaks across evals.
    static SCRIPT_AUTHORITY: Cell<Option<SessionId>> = const { Cell::new(None) };
}

/// RAII guard that publishes a `&mut World` to the thread-local for the lifetime
/// of a script evaluation, and clears it on drop (even on panic).
pub struct WorldScope;

impl WorldScope {
    /// Publish `world` to the scoped thread-local for the guard's lifetime.
    pub fn enter(world: &mut World) -> Self {
        WORLD_PTR.with(|p| p.set(world as *mut World));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
        WorldScope
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        WORLD_PTR.with(|p| p.set(std::ptr::null_mut()));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
    }
}

/// Set the session the current script acts on behalf of, for [`cmd`]
/// authorization. `None` = host-trusted (no gate). The scenario driver sets this
/// per-entity from its `ScriptAuthority`; the `RunRhai` drain sets it per-eval.
pub fn set_script_authority(session: Option<SessionId>) {
    SCRIPT_AUTHORITY.with(|a| a.set(session));
}

/// The session the current [`cmd`] is authorized against, if any.
pub fn script_authority() -> Option<SessionId> {
    SCRIPT_AUTHORITY.with(|a| a.get())
}

/// Well-known capability keys for script operations that are NOT a reflected
/// command but are still authorized through the same [`CommandPolicyRegistry`]
/// gate as commands — currently the structural mutation verbs (add/remove a
/// component, despawn), which restructure a target entity directly via
/// reflection rather than by dispatching a command. Mirrors the avatar-relay
/// capability keys in `lunco_core::session::capability`.
pub mod capability {
    /// Structurally mutate a target entity from a script (`add` / `remove` a
    /// component, `despawn`). Registered `OWNED_CONTROL` (see
    /// `commands::register_command_policies`) so a remote script may only
    /// restructure entities its launching session owns.
    pub const STRUCTURAL_MUTATE: &str = "ScriptStructuralMutate";
}

/// The §3.4 authority gate, shared by [`cmd`] and the structural verbs so every
/// authoritative script mutation flows through ONE path: authorize operation
/// `op` on `target_gid` against the current [`script_authority`], exactly as the
/// networked command gate does ([`authorize`]: role lattice + ownership, policy
/// from [`CommandPolicyRegistry`]).
///
/// Returns `Ok` immediately when no authority is set (a local / host-trusted
/// launch → ungated). Fails CLOSED if the session resources are absent: an
/// authority is only ever set under active networking (a remote launch), so
/// their absence is a misconfiguration we must not silently wave through.
pub fn enforce_script_authority(
    world: &World,
    op: &str,
    target_gid: Option<u64>,
) -> Result<(), String> {
    let Some(session) = script_authority() else {
        return Ok(());
    };
    let (Some(reg), Some(rbac), Some(pol)) = (
        world.get_resource::<SessionRegistry>(),
        world.get_resource::<SessionRbac>(),
        world.get_resource::<CommandPolicyRegistry>(),
    ) else {
        return Err(format!(
            "'{op}' denied: script authority set but session registries are unavailable"
        ));
    };
    authorize(reg, rbac, pol, session, op, target_gid).map_err(|r| r.to_string())
}

/// The `#[authz_target]` gid a command authorizes against, read from the
/// (global-gid) script `params` via its reflect schema. `None` for a target-less
/// command (or an unknown name).
fn command_target_gid(world: &World, name: &str, params: &serde_json::Value) -> Option<u64> {
    let app_reg = world.resource::<AppTypeRegistry>();
    let type_reg = app_reg.read();
    type_reg
        .get_with_short_type_path(name)
        .and_then(|r| authz_target_gid(params, r.type_id(), &type_reg))
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
        // §3.4: a script launched by a remote session must not exceed that
        // session's authority. When an authority is set (remote launch),
        // re-authorize through the SAME gate the networked command path uses; an
        // unset authority (local / host-trusted launch) stays ungated (and skips
        // the schema lookup on this per-tick hot path).
        if script_authority().is_some() {
            let target_gid = command_target_gid(world, name, &params);
            if let Err(error) = enforce_script_authority(world, name, target_gid) {
                return serde_json::json!({ "id": id, "ok": false, "error": error });
            }
        }
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

// ── Verbs: ports ──────────────────────────────────────────────────────────────
//
// The co-sim **port registry** ([`lunco_core::ports::PortRegistry`]) is the one
// surface every participant exchanges scalars through — the wire engine, the API
// (`GetPort`/`SetPort`), the inspector, and (here) scripts. A script reaches
// Modelica variables, avian rigid-body state (`mass`, `inertia_*`, `com_*`,
// `force_*`, `quat_*`, …), joint angles, and hardware ports by the SAME path the
// simulation uses — language-neutral, so rhai and python share it.

/// Read a co-sim port value on entity `gid`. `None` if no such port — which is
/// what lets the scripting `get` verb fall back here only after generic
/// reflection misses.
pub fn read_port(gid: u64, name: &str) -> Option<f64> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let registry = world.get_resource::<lunco_core::ports::PortRegistry>()?;
        registry.read_port(world, entity, name)
    })
    .flatten()
}

/// Write a co-sim port input on entity `gid` — the same path `SetPort` and wires
/// use. `true` if a writable input port of that name existed. Strict: never
/// creates a port (an unknown name returns `false`).
pub fn write_port(gid: u64, name: &str, value: f64) -> bool {
    with_world(|world| {
        let Some(entity) = resolve_entity(world, gid) else {
            return false;
        };
        let Some(registry) = world
            .get_resource::<lunco_core::ports::PortRegistry>()
            .cloned()
        else {
            return false;
        };
        registry.write_port(world, entity, name, value)
    })
    .unwrap_or(false)
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

/// `param(gid, key)` — read a per-prim numeric script parameter from the
/// entity's [`lunco_core::ScriptParams`] (authored in USD as `lunco:params`). A
/// HashMap lookup — the typed, fast way for a reusable script to get per-instance
/// config, vs scanning `name(me)`. `None` if the entity/component/key is absent.
pub fn script_param(gid: u64, key: &str) -> Option<f64> {
    with_world(|world| {
        let e = resolve_entity(world, gid)?;
        let p = world.get::<lunco_core::ScriptParams>(e)?;
        p.0.get(key).copied()
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

// ── Verbs: structural mutation ──────────────────────────────────────────────
//
// The C/D of CRUD: `set`/`get` are the R/U of *fields*; these change an entity's
// *structure* — add/remove a component, despawn an entity. Host-authoritative
// (scripts run host-only). Replication follows the same rule as `set`: a change
// reaches clients only if the affected component is in the replicated set, so
// `ApiVisibility` curates what is safe to expose. NOTE: there is deliberately no
// generic `spawn(components)` — runtime spawns replicate by catalog `entry_id`
// (`NetSpawn`), so clients reconstruct from the catalog, not an arbitrary
// component bag; use `cmd("SpawnEntity", …)` for a replicable spawn.

/// `add(id, "Comp", #{fields})` — insert (or replace) a reflected component,
/// constructed from its `ReflectDefault` then patched field-by-field by `build`
/// (`native → reflect`, no JSON), the structural twin of [`set_component_field`].
/// Requires the type to register `ReflectDefault` (`#[reflect(Component, Default)]`).
pub fn add_component(
    gid: u64,
    comp: &str,
    build: impl FnOnce(&mut dyn bevy::reflect::Reflect) -> Result<(), String>,
) -> Result<(), String> {
    use bevy::reflect::std_traits::ReflectDefault;

    with_world(|world| -> Result<(), String> {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        // §3.4: same authority gate as `cmd()` — a remote script may restructure
        // only entities its launching session owns (ungated for local launches).
        enforce_script_authority(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg
            .get_with_short_type_path(comp)
            .ok_or_else(|| format!("unknown type '{comp}'"))?;
        let reflect_component = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| format!("'{comp}' is not a Component"))?;
        let reflect_default = registration
            .data::<ReflectDefault>()
            .ok_or_else(|| format!("'{comp}' has no ReflectDefault (add #[reflect(Default)])"))?;
        let mut value = reflect_default.default();
        build(&mut *value)?;
        let mut entity_mut = world
            .get_entity_mut(entity)
            .map_err(|_| format!("entity {gid} despawned"))?;
        reflect_component.insert(&mut entity_mut, value.as_partial_reflect(), &reg);
        Ok(())
    })
    .unwrap_or_else(|| Err("no world in scope".into()))
}

/// `remove(id, "Comp")` — strip a reflected component from an entity.
pub fn remove_component(gid: u64, comp: &str) -> Result<(), String> {
    with_world(|world| -> Result<(), String> {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        enforce_script_authority(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg
            .get_with_short_type_path(comp)
            .ok_or_else(|| format!("unknown type '{comp}'"))?;
        let reflect_component = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| format!("'{comp}' is not a Component"))?;
        let mut entity_mut = world
            .get_entity_mut(entity)
            .map_err(|_| format!("entity {gid} despawned"))?;
        reflect_component.remove(&mut entity_mut);
        Ok(())
    })
    .unwrap_or_else(|| Err("no world in scope".into()))
}

/// `despawn(id)` — despawn an entity (and its children). On a networked host the
/// removal replicates via `broadcast_despawns` (off `RemovedComponents<
/// GlobalEntityId>`), so clients drop their proxy instead of leaving a ghost.
pub fn despawn_entity(gid: u64) -> Result<(), String> {
    with_world(|world| -> Result<(), String> {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        enforce_script_authority(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
        world.despawn(entity);
        Ok(())
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
            Query<(Option<&Name>, Has<FlightSoftware>, Option<&CelestialBody>)>,
        )> = SystemState::new(world);
        let (q_parents, q_grids, q_spatial, q_meta) = state.get(world);
        let items = pairs
            .into_iter()
            .map(|(gid, entity)| {
                let (name, is_vehicle, body) = q_meta.get(entity).unwrap_or((None, false, None));
                let kind = if is_vehicle {
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

// ── Deterministic RNG ───────────────────────────────────────────────────────
//
// Scripts WILL want randomness (scatter, jitter, exploration, retry backoff). A
// wall-clock / OS source would diverge across host and clients and break replay,
// so the bridge gives them a stream that is a pure function of stable inputs:
// the entity's networked `GlobalEntityId`, the sim tick, and the call order
// within the hook. Same entity + same tick + same call index → same number on
// every peer and every re-run. The runtime calls `rng_begin` before each hook;
// each `rng_next_*` advances the per-thread stream. Execution is single-threaded
// (FixedUpdate / wasm), so the thread-local is sound and order is deterministic.

thread_local! {
    static RNG_STATE: Cell<u64> = const { Cell::new(0) };
    /// The gid of the entity whose hook is currently running — set by
    /// `rng_begin` (called before every hook) so `emit` can stamp the EMITTER
    /// onto its `TelemetryEvent.source` without the script passing `me`.
    static CURRENT_SELF: Cell<u64> = const { Cell::new(0) };
}

/// The gid of the script entity whose hook is currently executing (`0` if none).
pub fn current_self() -> u64 {
    CURRENT_SELF.with(|c| c.get())
}

/// SplitMix64 — advance `state`, return a well-diffused 64-bit value. Tiny,
/// stateless-modulo-`state`, and identical on every platform.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Seed the per-hook RNG stream from `(gid, tick, salt)`. `salt` decorrelates
/// distinct hooks/events firing in the same tick on the same entity (so on_tick
/// and on_event don't draw the identical sequence). Called by the scenario
/// runtime before each hook invocation.
pub fn rng_begin(gid: u64, tick: u64, salt: u64) {
    let seed = gid
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ salt.wrapping_mul(0xA0761_D6478_BD642F);
    RNG_STATE.with(|c| c.set(seed));
    CURRENT_SELF.with(|c| c.set(gid));
}

/// Next uniform `f64` in `[0, 1)` from the seeded stream (53-bit mantissa).
pub fn rng_next_f64() -> f64 {
    RNG_STATE.with(|c| {
        let mut s = c.get();
        let r = splitmix64(&mut s);
        c.set(s);
        (r >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// A stable 64-bit hash of a string, for salting the RNG by event name. FNV-1a.
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ── Verbs: events ───────────────────────────────────────────────────────────

/// `emit(name, value)` — fire a `TelemetryEvent` on the shared bus. The scalar
/// payload is taken from a JSON value (the native→JSON projection at the call
/// site is trivial for scalars). Returns whether a World was in scope.
pub fn emit(name: &str, value: TelemetryValue) -> bool {
    with_world(|world| {
        let timestamp = world
            .get_resource::<lunco_time::WorldTime>()
            .map(|w| w.epoch_jd)
            .unwrap_or(0.0);
        world.trigger(TelemetryEvent {
            name: name.to_string(),
            // The emitter = the script whose hook is running (set by rng_begin).
            source: current_self(),
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
        // The emitter's gid — branch on `evt.source` to tell WHICH sensor/script
        // fired (independent of the name). `0` = global/no entity.
        ("source".to_string(), b.int(ev.source as i64)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::session::{AuthorityRole, CommandPolicy, UserSession};

    /// §3.4: a `cmd()` from a script launched by a remote session is
    /// re-authorized against that session (same gate as the networked path);
    /// a local/host launch (`None` authority) stays ungated.
    #[test]
    fn scripted_cmd_gated_by_authority() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.init_resource::<SessionRegistry>();
        world.init_resource::<SessionRbac>();
        world.init_resource::<CommandPolicyRegistry>();
        world.init_resource::<CommandResults>();

        // An authenticated Observer (server-issued token) that owns nothing.
        world.resource_mut::<SessionRbac>().sessions.insert(
            7,
            UserSession {
                session_id: SessionId(7),
                username: "tester".into(),
                role: AuthorityRole::Observer,
                authenticated: true,
                token: Some("server-token".into()),
            },
        );

        let _scope = WorldScope::enter(&mut world);

        // (1) Local/host launch → ungated. No observer is registered for the
        // command, so it dispatches as a fire-and-forget no-op and reports ok.
        set_script_authority(None);
        let r = cmd_raw("SetPorts", serde_json::json!({ "target": 1, "writes": [] }));
        assert_eq!(r["ok"], serde_json::json!(true), "local launch must be ungated");

        // (2) Authenticated Observer + an OPEN command (not in the policy base)
        // → allowed.
        set_script_authority(Some(SessionId(7)));
        let r = cmd_raw("SomeOpenCommand", serde_json::json!({}));
        assert_eq!(r["ok"], serde_json::json!(true), "OPEN command passes for an authed session");

        // (3) Same Observer + an OWNED_CONTROL command on a target it does NOT
        // own → demands Operator → rejected BEFORE dispatch.
        set_script_authority(Some(SessionId(7)));
        let r = cmd_raw("SetPorts", serde_json::json!({ "target": 1, "writes": [] }));
        assert_eq!(r["ok"], serde_json::json!(false), "unowned OWNED_CONTROL must be rejected");
        assert!(r["error"].is_string());

        // (4) Unknown / unauthenticated session → denied even for an OPEN command.
        set_script_authority(Some(SessionId(999)));
        let r = cmd_raw("SomeOpenCommand", serde_json::json!({}));
        assert_eq!(r["ok"], serde_json::json!(false), "unknown session denied even for OPEN");
    }

    /// The structural verbs (`add`/`remove`/`despawn`) route through the SAME
    /// gate via [`enforce_script_authority`] under the `STRUCTURAL_MUTATE`
    /// capability: ungated locally, ownership-gated for a remote session.
    #[test]
    fn structural_verbs_share_the_authority_gate() {
        let mut world = World::new();
        world.init_resource::<SessionRegistry>();
        world.init_resource::<SessionRbac>();
        // OWNED_CONTROL for the structural capability, as the plugin registers it.
        let mut policies = CommandPolicyRegistry::default();
        policies.register(capability::STRUCTURAL_MUTATE, CommandPolicy::OWNED_CONTROL);
        world.insert_resource(policies);

        // An authenticated Observer that owns entity gid 1 (but not gid 2).
        world.resource_mut::<SessionRbac>().sessions.insert(
            7,
            UserSession {
                session_id: SessionId(7),
                username: "tester".into(),
                role: AuthorityRole::Observer,
                authenticated: true,
                token: Some("server-token".into()),
            },
        );
        let _ = world.resource_mut::<SessionRegistry>().claim(SessionId(7), 1);

        let _scope = WorldScope::enter(&mut world);

        // Local launch → ungated for any target.
        set_script_authority(None);
        assert!(enforce_script_authority(&world, capability::STRUCTURAL_MUTATE, Some(2)).is_ok());

        // Remote owner may restructure the entity it owns (gid 1)…
        set_script_authority(Some(SessionId(7)));
        assert!(enforce_script_authority(&world, capability::STRUCTURAL_MUTATE, Some(1)).is_ok());
        // …but NOT an entity it does not own (gid 2).
        assert!(enforce_script_authority(&world, capability::STRUCTURAL_MUTATE, Some(2)).is_err());
    }
}
