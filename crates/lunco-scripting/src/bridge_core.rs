//! Language-neutral world bridge — the runtime-agnostic core that lets *any*
//! scripting backend read ECS state and drive the simulation.
//!
//! # Why this exists
//!
//! The verbs a script gets (`cmd` / `get` / `query` / `world_pos` / hierarchy /
//! `emit` / clock) are identical regardless of language. This module owns that
//! logic *once*, free of any interpreter type, so rhai and Python are thin
//! bindings over it rather than parallel reimplementations.
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
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};

use lunco_api::discovery::find_api_command;
use lunco_api::executor::{
    authz_target_gid, command_result_json, validate_command_params, ApiCommandEvent,
};
use lunco_api::queries::{ApiQueryRegistry, ApiVisibility};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::ApiResponse;
use lunco_core::session::{authorize, CommandPolicyRegistry, SessionRbac, SessionRegistry};
use lunco_core::{
    CelestialBody, CommandResults, GlobalEntityId, OpId, SessionId, Severity,
    SimTick, TelemetryEvent, TelemetryValue, SECS_PER_TICK,
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

/// Commands a particular host intentionally accepts from scenarios without
/// executing. This is for presentation intents on a windowless host: the same
/// scenario can update a GUI HUD when one exists, while a headless acceptance
/// run acknowledges the intent without inventing UI state or warning that a
/// command is misspelled.
///
/// The host owns the explicit list. Unknown commands remain real failures.
#[derive(Resource, Default, Clone, Debug)]
pub struct IgnoredScenarioCommands(HashSet<String>);

impl IgnoredScenarioCommands {
    /// Build a policy from the presentation command names that this host omits.
    pub fn new(names: impl IntoIterator<Item = &'static str>) -> Self {
        Self(names.into_iter().map(str::to_owned).collect())
    }

    /// Whether this host intentionally ignores this scenario command.
    pub fn accepts(&self, name: &str) -> bool {
        self.0.contains(name)
    }
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
        ReflectRef::List(l) => {
            Some(b.array(l.iter().filter_map(|x| build_from_reflect(b, x)).collect()))
        }
        ReflectRef::Array(a) => {
            Some(b.array(a.iter().filter_map(|x| build_from_reflect(b, x)).collect()))
        }
        ReflectRef::Tuple(t) => Some(
            b.array(
                t.iter_fields()
                    .filter_map(|x| build_from_reflect(b, x))
                    .collect(),
            ),
        ),
        ReflectRef::TupleStruct(ts) if ts.field_len() == 1 => {
            ts.field(0).and_then(|f| build_from_reflect(b, f))
        }
        ReflectRef::TupleStruct(ts) => Some(
            b.array(
                ts.iter_fields()
                    .filter_map(|x| build_from_reflect(b, x))
                    .collect(),
            ),
        ),
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
        J::Object(o) => b.map(
            o.iter()
                .map(|(k, x)| (k.clone(), build_from_json(b, x)))
                .collect(),
        ),
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

    /// True while a **client-scoped** scenario runs on a predicting client. Its
    /// [`cmd`] calls are then restricted to the client-local surface
    /// ([`lunco_core::ClientCommandPolicy`]) so a presentation/HUD script can't
    /// mutate authoritative sim state. Set per-pass by the scenario driver; reset
    /// with the [`WorldScope`] so it never leaks across evals.
    static SCRIPT_CLIENT_LOCAL: Cell<bool> = const { Cell::new(false) };

    /// Names of authoritative commands a client-scoped scenario tried to issue
    /// and were dropped this hook (see [`cmd_raw`]). The scenario driver drains
    /// this per-entity via [`take_script_rejects`] and folds it into that
    /// scenario's *diagnostics* — the drop surfaces once in the editor as an
    /// authoring warning, not as a per-tick server log line. Deduped; reset with
    /// the [`WorldScope`].
    static SCRIPT_REJECTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that publishes a `&mut World` to the thread-local for the lifetime
/// of a script evaluation, and clears it on drop (even on panic).
pub struct WorldScope;

impl WorldScope {
    /// Publish `world` to the scoped thread-local for the guard's lifetime.
    pub fn enter(world: &mut World) -> Self {
        WORLD_PTR.with(|p| p.set(world as *mut World));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
        SCRIPT_CLIENT_LOCAL.with(|c| c.set(false));
        SCRIPT_REJECTS.with(|r| r.borrow_mut().clear());
        WorldScope
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        WORLD_PTR.with(|p| p.set(std::ptr::null_mut()));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
        SCRIPT_CLIENT_LOCAL.with(|c| c.set(false));
        SCRIPT_REJECTS.with(|r| r.borrow_mut().clear());
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

/// Mark the current script pass as a client-scoped scenario on a predicting
/// client, so [`cmd`] restricts it to the client-local command surface. Set by
/// the scenario driver; reset with the [`WorldScope`].
pub fn set_script_client_local(on: bool) {
    SCRIPT_CLIENT_LOCAL.with(|c| c.set(on));
}

/// Whether the current script is a client-scoped scenario (its `cmd()`s are
/// restricted to the client-local surface).
pub fn script_is_client_local() -> bool {
    SCRIPT_CLIENT_LOCAL.with(|c| c.get())
}

/// Take (and clear) the authoritative commands a client-scoped scenario tried to
/// issue and were dropped since the last drain. The scenario driver calls this
/// once per entity, right after its hooks, and turns any names into a single
/// per-scenario diagnostic — so the drop is surfaced once in the editor instead
/// of spamming the server log every tick.
pub fn take_script_rejects() -> Vec<String> {
    SCRIPT_REJECTS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// Well-known capability keys for script operations that are NOT a reflected
/// command but are still authorized through the same [`CommandPolicyRegistry`]
/// gate as commands — structural mutation and reflected field/resource writes.
/// These operations mutate ECS directly rather than dispatching a command, so
/// they name their capability explicitly at the owning reflection seam.
pub mod capability {
    /// Write a live co-simulation input port. Unlike `SetPorts`, the generic
    /// `set()` fallback is a raw write and does not create a persistent hold;
    /// it is therefore still an owned mutation, not a read.
    pub const PORT_MUTATE: &str = "ScriptPortMutate";
    /// Structurally mutate a target entity from a script (`add` / `remove` a
    /// component, `despawn`). Registered `OWNED_CONTROL` (see
    /// `commands::register_command_policies`) so a remote script may only
    /// restructure entities its launching session owns.
    pub const STRUCTURAL_MUTATE: &str = "ScriptStructuralMutate";
    /// Mutate a reflected component field from a remote script. Ownership is
    /// required for the target entity, just like structural mutation.
    pub const FIELD_MUTATE: &str = "ScriptFieldMutate";
    /// Mutate a reflected global resource field from a remote script. There is
    /// no entity to own, so this is an Operator-floor capability.
    pub const SETTING_MUTATE: &str = "ScriptSettingMutate";
    /// Replace or remove an installed policy hook from a script. Hook changes
    /// alter authorization globally, so remote scripts need Operator authority.
    pub const POLICY_MUTATE: &str = "ScriptPolicyMutate";
}

/// Gate direct script mutations that do not pass through a reflected command.
///
/// A client-scoped script is allowed to issue an explicitly client-local
/// command through `cmd()`, or an ownership-gated predictive command such as
/// `SetPorts`. Direct reflection has no forwarding or
/// prediction path, so they are denied for client-local execution. This keeps
/// `set`, structural verbs, and raw port writes from silently changing
/// only one peer.
pub fn enforce_script_mutation(
    world: &World,
    capability: &str,
    target_gid: Option<u64>,
) -> Result<(), String> {
    if script_is_client_local() {
        return Err(format!(
            "'{capability}' denied: direct script mutations are not available from a client-scoped script; use an allowed typed command"
        ));
    }
    enforce_script_authority(world, capability, target_gid)
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
    // Remote script authority is bound to the launching session and uses the
    // same role/ownership/policy lattice as networked command dispatch. Local
    // host launches intentionally have no remote session to authorize.
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
    // `ControlPathRegistry` is a plain default when absent: an app that never
    // declares a blackout has none down, so the gate is unchanged.
    let paths = world
        .get_resource::<lunco_core::session::ControlPathRegistry>()
        .cloned()
        .unwrap_or_default();
    authorize(reg, rbac, pol, &paths, session, op, target_gid).map_err(|r| r.to_string())
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
/// evaluation for the duration of the call, and it is TAKEN out of the slot while
/// `f` runs — a nested `with_world` sees null and returns `None` instead of
/// reconstructing a second live `&mut` — so the reconstructed `&mut` is unique.
pub fn with_world<R>(f: impl FnOnce(&mut World) -> R) -> Option<R> {
    WORLD_PTR.with(|p| {
        let ptr = p.replace(std::ptr::null_mut());
        if ptr.is_null() {
            return None;
        }
        let result = f(unsafe { &mut *ptr });
        p.set(ptr);
        Some(result)
    })
}

pub(crate) fn resolve_entity(world: &World, gid: u64) -> Option<Entity> {
    world
        .get_resource::<ApiEntityRegistry>()?
        .resolve(&GlobalEntityId::from_raw(gid))
}

/// The session id currently controlling `gid`'s vessel (`0` = the local human, the
/// autopilot band for an AI), or `None` if nobody owns it. Reads the same
/// [`SessionRegistry`] ownership the possession arbiter uses, so a scenario can
/// answer "is this rover controlled, and by whom?" **uniformly across human and AI**
/// drivers — the observability the audit flagged as missing.
pub fn owner_of(gid: u64) -> Option<u64> {
    with_world(|world| Some(world.get_resource::<SessionRegistry>()?.owner_of(gid)?.0)).flatten()
}

/// The role of `gid`'s controlling session — `"AiAgent"` (an autopilot),
/// `"Owner"`/`"Operator"` (a human), … — or `None` if unowned. Falls back to
/// `"Owner"` for an owned-but-unregistered (local) session. The human-vs-AI test.
pub fn controller_role(gid: u64) -> Option<String> {
    with_world(|world| {
        let owner = world.get_resource::<SessionRegistry>()?.owner_of(gid)?;
        let role = world
            .get_resource::<SessionRbac>()
            .and_then(|rbac| rbac.sessions.get(&owner.0).map(|s| format!("{:?}", s.role)))
            .unwrap_or_else(|| "Owner".to_string());
        Some(role)
    })
    .flatten()
}

// ── Verbs: write (cmd) ──────────────────────────────────────────────────────

/// Fire a command by name through `ApiCommandEvent` (the same entry point the
/// HTTP API / MCP use) and return its `{ id, ok, data?, error? }` result as
/// JSON. `params` is the JSON the API contract expects. Runs SYNCHRONOUSLY (the
/// bridge flushes) so `data` carries any command result data the handler returned.
pub fn cmd_raw(name: &str, mut params: serde_json::Value) -> serde_json::Value {
    let id = OpId::new().0;
    with_world(|world| {
        if world
            .get_resource::<IgnoredScenarioCommands>()
            .is_some_and(|commands| commands.accepts(name))
        {
            return serde_json::json!({ "id": id, "ok": true });
        }

        // Rhai is an in-process transport, but it still uses the same public
        // typed-command contract as HTTP/MCP. Resolve the marker, visibility,
        // ambiguity, and reflected parameter schema before any client or RBAC
        // policy can run. The dispatcher repeats the marker check because
        // `ApiCommandEvent` is also an internal event that callers can trigger
        // directly.
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let type_reg = type_registry.read();
        let visibility = world.get_resource::<ApiVisibility>();
        let registration = match find_api_command(&type_reg, name, visibility) {
            Ok(registration) => registration,
            Err(error) => {
                return serde_json::json!({
                    "id": id,
                    "ok": false,
                    "error": error.message(name),
                });
            }
        };
        let Some(entity_registry) = world.get_resource::<ApiEntityRegistry>() else {
            return serde_json::json!({
                "id": id,
                "ok": false,
                "error": "API entity registry is unavailable",
            });
        };
        if let Err(error) =
            validate_command_params(name, &params, registration, &type_reg, entity_registry)
        {
            return serde_json::json!({ "id": id, "ok": false, "error": error });
        }
        drop(type_reg);

        // Client-scoped scenario on a predicting client: allow ONLY the
        // client-local surface (HUD / notifications / camera). Anything else is
        // an authoritative mutation the host owns — running it here would
        // double-apply or fight replication, so drop it (the host stays the sole
        // author of shared sim state). Deny-all by default: a command opts in via
        // `App::mark_client_local` in its own crate.
        if script_is_client_local() {
            let allowed = world
                .get_resource::<lunco_core::ClientCommandPolicy>()
                .is_some_and(|p| p.allows(name));
            // Case 2 — a client script may drive what it OWNS. Beyond the static
            // client-local surface (Case 1), a client-scoped script may issue an
            // **ownership-gated** command (e.g. `SetPorts`) against a target this
            // client possesses. That is the legitimate predict-own input path: the
            // command applies LOCALLY this tick (immediate client-side prediction)
            // AND is forwarded to the host, which re-authorizes under the SAME
            // ownership gate — so no authority is smuggled. A non-owned target, or
            // a command that isn't ownership-gated, stays dropped (Case 1). This
            // reuses the authority substrate (`CommandPolicyRegistry` +
            // `SessionRegistry`) rather than a second allowlist.
            let owns_target = !allowed
                && world
                    .get_resource::<CommandPolicyRegistry>()
                    .is_some_and(|reg| reg.policy_for(name).ownership_gated)
                && match (
                    command_target_gid(world, name, &params),
                    world.get_resource::<lunco_core::LocalSession>(),
                    world.get_resource::<SessionRegistry>(),
                ) {
                    (Some(gid), Some(local), Some(reg)) => reg.owns(local.0, gid),
                    _ => false,
                };
            if !allowed && !owns_target {
                // Record the dropped command name for the driver to surface as a
                // per-scenario diagnostic (once, located to the script) rather
                // than logging it every tick. Dedup within the pass.
                SCRIPT_REJECTS.with(|r| {
                    let mut v = r.borrow_mut();
                    if !v.iter().any(|n| n == name) {
                        v.push(name.to_string());
                    }
                });
                return serde_json::json!({
                    "id": id, "ok": false,
                    "error": format!("`{name}` is not permitted from a client-scoped script"),
                });
            }
            // Thread a real `seq`/`tick` for a client-owned control command so it
            // engages the PREDICT-OWN path the same way keyboard input does
            // (`drive_from_bindings`). A scenario/API `drive()` sends `seq:0`/
            // `tick:0`; `record_control_input` only buffers an input frame when
            // `seq != 0`, and `maintain_owned_locally`'s activity signal needs a
            // real tick — so without this the owned rover never predicts locally
            // (it stays a snapshot proxy and only crawls from the host's authority).
            // Stamping the next per-vessel seq here — at the origin, BEFORE
            // `capture_command` serializes the command for the wire — means the
            // client and host agree on the seq the reconcile acks against.
            if owns_target && name == "SetPorts" {
                if let Some(gid) = command_target_gid(world, name, &params) {
                    let tick = world
                        .get_resource::<lunco_core::SimTick>()
                        .map_or(0, |t| t.0);
                    let seq =
                        world
                            .get_resource_mut::<lunco_core::OwnedInputLog>()
                            .map(|mut log| {
                                let entry = log.0.entry(gid).or_default();
                                entry.next_seq = entry.next_seq.wrapping_add(1); // seq 0 reserved
                                entry.next_seq
                            });
                    if let (Some(seq), Some(obj)) = (seq, params.as_object_mut()) {
                        obj.insert("seq".into(), serde_json::json!(seq));
                        obj.insert("tick".into(), serde_json::json!(tick));
                    }
                }
            }
        }
        // §3.4: a script launched by a remote session must not exceed that
        // session's authority. When an authority is set (remote launch),
        // re-authorize through the SAME gate the networked command path uses; an
        // unset authority (local / host-trusted launch) stays ungated after the
        // shared public-command schema gate above.
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
            correlation_id: None,
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
    .unwrap_or_else(|| serde_json::json!({ "id": -1, "ok": false, "error": "no world in scope" }))
}

/// `cmd` as a native value: fire, then convert the JSON result in one pass.
pub fn cmd<B: ValueBuilder>(b: &B, name: &str, params: serde_json::Value) -> B::Value {
    build_from_json(b, &cmd_raw(name, params))
}

// ── Verbs: query ────────────────────────────────────────────────────────────

/// Invoke a registered `ApiQueryProvider` by name.
///
/// `Ok(None)` is a successful provider response with no data. Missing providers
/// and provider errors are `Err`, so callers can distinguish an empty answer
/// from a broken or unavailable query surface.
pub fn query_raw(
    name: &str,
    params: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    with_world(|world| {
        let provider = world
            .get_resource::<ApiQueryRegistry>()
            .and_then(|reg| reg.get(name))
            .ok_or_else(|| format!("query '{name}' is not registered"))?;
        match provider.execute(world, &params) {
            ApiResponse::Ok { data } => Ok(data),
            ApiResponse::Error { code, message } => {
                Err(format!("query '{name}' failed ({code}): {message}"))
            }
            _ => Err(format!(
                "query '{name}' returned an unsupported response kind"
            )),
        }
    })
    .ok_or_else(|| "no world in scope".to_string())?
}

/// `query` as a native value. Successful data remains the provider's native
/// value; a successful no-data response is unit. Errors become an explicit
/// `#{ ok: false, error: "..." }` value so a script can branch without losing
/// the provider's diagnostic.
pub fn query<B: ValueBuilder>(b: &B, name: &str, params: serde_json::Value) -> B::Value {
    match query_raw(name, params) {
        Ok(Some(data)) => build_from_json(b, &data),
        Ok(None) => b.unit(),
        Err(error) => b.map(vec![
            ("ok".to_string(), b.bool(false)),
            ("error".to_string(), b.string(&error)),
        ]),
    }
}

// ── Verbs: ports ──────────────────────────────────────────────────────────────
//
// The co-sim **port registry** ([`lunco_core::ports::PortRegistry`]) is the one
// surface every participant exchanges scalars through — the wire engine, the API
// (`GetPort`/`SetPorts`), the inspector, and (here) scripts. A script reaches
// Modelica variables, avian rigid-body state (`mass`, `inertia_*`, `com_*`,
// `force_*`, `quat_*`, …), joint angles, and hardware ports by the SAME path the
// simulation uses — language-neutral, so rhai and python share it.

/// Read a co-sim port value on entity `gid`. `None` means the canonical
/// co-simulation namespace has no port with that name. The scripting `get` verb
/// consults it after the generic reflected component namespace.
pub fn read_port(gid: u64, name: &str) -> Option<f64> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let registry = world.get_resource::<lunco_core::ports::PortRegistry>()?;
        registry.read_port(world, entity, name)
    })
    .flatten()
}

/// Write a co-sim port input on entity `gid` — the same path `SetPorts` and wires
/// use. `true` if a writable input port of that name existed. Strict: never
/// creates a port (an unknown name returns `false`).
pub fn write_port(gid: u64, name: &str, value: f64) -> bool {
    with_world(|world| {
        if enforce_script_mutation(world, capability::PORT_MUTATE, Some(gid)).is_err() {
            return false;
        }
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

/// `world_pos(id)` — f64 position in the active simulation frame, or `None`.
///
/// For a surface scene this is the authored site frame used by Avian, terrain,
/// routes, and spawn commands. Celestial/root transforms are an implementation
/// detail and cannot leak into ordinary script navigation.
pub fn world_pos(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        state
            .get(world)
            .ok()?
            .position(entity)
            .map(|position| position.0)
    })
    .flatten()
}

/// `geolocation(id)` — where on the body an entity actually is, as
/// `(lat_deg, lon_deg, height_m)`. `None` when the scene is not site-anchored
/// (no `SiteAnchor`) or the anchor's body is not present.
///
/// Works for any positioned entity — rover, waypoint, mast, marker — through
/// the same explicit site/body-fixed frame query used by HUDs and billboards.
/// Root-world position is deliberately not a fallback: celestial ancestors
/// move with ephemeris time and are not site ENU coordinates.
pub fn geolocation(gid: u64) -> Option<lunco_celestial::Geodetic> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
            Query<(Entity, &lunco_celestial::GeodeticAnchor), With<lunco_celestial::SiteAnchor>>,
            Res<lunco_celestial::CelestialBodyRegistry>,
            Res<lunco_celestial::ReferenceFrameIndex>,
        )> = SystemState::new(world);
        let (q_parents, q_grids, q_spatial, q_site, bodies, frame_index) = state.get(world).ok()?;
        lunco_celestial::resolve_surface_pose(
            entity,
            &q_site,
            &bodies,
            &frame_index,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
        .map(|pose| pose.geodetic)
    })
    .flatten()
}

/// `world_forward(id)` — unit heading in the active simulation frame, or `None`.
pub fn world_forward(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        let rotation = state.get(world).ok()?.rotation(entity)?;
        Some(rotation.0 * DVec3::NEG_Z)
    })
    .flatten()
}

/// `world_rotation(id)` — orientation in the active simulation frame as a
/// quaternion `[x, y, z, w]`, or `None`. The GENERAL orientation accessor: every axis
/// (`up`, `forward`, `right`) is `quat * unit_axis`, derived rhai-side, so this
/// one host fn subsumes `world_forward` and unblocks tilt/tip-over logic (a rover
/// is tipped when its up-vector's `y` drops below `cos(θ)`) without a per-axis
/// Rust fn each. It uses the same active-frame hierarchy sample as
/// `world_forward`, so surface-up remains +Y below a rotated celestial branch.
pub fn world_rotation(gid: u64) -> Option<[f64; 4]> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        let q = state.get(world).ok()?.rotation(entity)?.0;
        Some([q.x, q.y, q.z, q.w])
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
/// entity's [`lunco_core::ScriptParams`] (authored in USD as `lunco:param:<key>`). A
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
        // bevy 0.19: resources live on dedicated entities and `ReflectResource`
        // is a functionless marker — its presence certifies "is a Resource",
        // the actual access goes through `ReflectComponent` on that entity.
        registration.data::<ReflectResource>()?;
        let reflect_component = registration.data::<ReflectComponent>()?;
        let component_id = world
            .components()
            .get_valid_id(registration.type_info().type_id())?;
        let entity = world.resource_entities().get(component_id)?;
        let reflected = reflect_component.reflect(world.get_entity(entity).ok()?)?;

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
/// systems normally. Host-side scripts may use it when authorized; client-scoped
/// scripts are denied because this path has no prediction/forwarding contract.
pub fn set_component_field(
    gid: u64,
    path: &str,
    apply: impl FnOnce(&mut dyn bevy::reflect::PartialReflect) -> Result<(), String>,
) -> Result<(), String> {
    let (comp, sub) = split_type_path(path);

    with_world(|world| -> Result<(), String> {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        enforce_script_mutation(world, capability::FIELD_MUTATE, Some(gid))?;
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
        enforce_script_mutation(world, capability::SETTING_MUTATE, None)?;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let registration = reg
            .get_with_short_type_path(res)
            .ok_or_else(|| format!("unknown type '{res}'"))?;
        // bevy 0.19: `ReflectResource` is a marker; mutate via the resource's
        // dedicated entity + `ReflectComponent` (see `get_resource_field`).
        registration
            .data::<ReflectResource>()
            .ok_or_else(|| format!("'{res}' is not a Resource"))?;
        let reflect_component = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| format!("'{res}' has no reflected Component data"))?;
        let component_id = world
            .components()
            .get_valid_id(registration.type_info().type_id())
            .ok_or_else(|| format!("resource '{res}' not present"))?;
        let entity = world
            .resource_entities()
            .get(component_id)
            .ok_or_else(|| format!("resource '{res}' not present"))?;
        let entity_mut = world
            .get_entity_mut(entity)
            .map_err(|_| format!("resource '{res}' not present"))?;
        let mut reflected = reflect_component
            .reflect_mut(entity_mut)
            .ok_or_else(|| format!("resource '{res}' not present"))?;
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
// *structure* — add/remove a component, despawn an entity. Host-side scripts may
// use it when authorized. Replication follows the same rule as `set`: a change
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
        enforce_script_mutation(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
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
        enforce_script_mutation(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
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
        enforce_script_mutation(world, capability::STRUCTURAL_MUTATE, Some(gid))?;
        world.despawn(entity);
        Ok(())
    })
    .unwrap_or_else(|| Err("no world in scope".into()))
}

/// `list_entities()` — `[{ id, name, type, pos, catalog_id, usd_prim_path,
/// control_bound, celestial_body }]` for every registered entity. `type` comes
/// from the projected USD `kind`; it is never inferred from control or physics
/// components. `catalog_id` is present only for catalog-spawned entities.
pub fn list_entities<B: ValueBuilder>(b: &B) -> B::Value {
    with_world(|world| {
        let Some(pairs) = world
            .get_resource::<ApiEntityRegistry>()
            .map(ApiEntityRegistry::entities)
        else {
            return b.array(Vec::new());
        };
        // One SystemState carries every per-entity read so the loop never
        // re-borrows the World.
        let mut state: SystemState<(
            lunco_physics::SimulationPoseQuery,
            Query<(
                Option<&Name>,
                Has<lunco_core::ControlBinding>,
                Option<&CelestialBody>,
                Option<&lunco_core::CatalogEntryId>,
                Option<&lunco_core::UsdPrimKind>,
            )>,
        )> = SystemState::new(world);
        let Some((poses, q_meta)) = state.get(world).ok() else {
            return b.array(Vec::new());
        };
        let items = pairs
            .into_iter()
            .map(|(gid, entity)| {
                let (name, accepts_commands, body, catalog_id, usd_kind) = q_meta
                    .get(entity)
                    .unwrap_or((None, false, None, None, None));
                let kind = usd_kind.map(|kind| kind.0.as_str()).unwrap_or("untyped");
                let pos = poses
                    .position(entity)
                    .map(|v| vec3_value(b, v.0.x, v.0.y, v.0.z))
                    .unwrap_or_else(|| b.unit());
                b.map(vec![
                    ("id".to_string(), b.int(gid.get() as i64)),
                    (
                        "name".to_string(),
                        b.string(name.map(|n| n.as_str()).unwrap_or("")),
                    ),
                    ("type".to_string(), b.string(kind)),
                    ("control_bound".to_string(), b.bool(accepts_commands)),
                    ("celestial_body".to_string(), b.bool(body.is_some())),
                    (
                        "catalog_id".to_string(),
                        b.string(catalog_id.map(|id| id.0.as_str()).unwrap_or("")),
                    ),
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
        let Some(registry) = world.get_resource::<ApiEntityRegistry>() else {
            return -1;
        };
        let pairs = registry.entities();
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

/// `twin_root()` — absolute path of the ACTIVE twin's folder, or `""` if none.
///
/// The twin is how a scene reaches files that ship beside it rather than inside the
/// engine: `load_startup_scene` resolves the scene's root with `lunco_twin::root_for_file`
/// and registers it via `Workspace::add_twin`, which sets `active_twin` when nothing else
/// has claimed it. So for a scene loaded by path, this is the directory containing that
/// scene.
///
/// It exists so a scenario can name a sibling file WITHOUT hardcoding an absolute path.
/// The campaign recording scripts previously spelled their output directory in full,
/// which meant a checkout on any other machine silently wrote to a path that did not
/// exist. `twin_root() + "/shots"` is the same string, derived.
///
/// Returns `""` rather than an error when no twin is active (a bare test world, or a
/// scene loaded from the engine's own `assets/`): a script concatenating onto it then
/// produces a relative path, which fails visibly at the write rather than silently
/// targeting `/`.
#[cfg(feature = "rhai")]
pub fn twin_root() -> String {
    with_world(|w| {
        let ws = w.get_resource::<lunco_workspace::WorkspaceResource>()?;
        let id = ws.0.active_twin?;
        Some(ws.0.twin(id)?.root.to_string_lossy().into_owned())
    })
    .flatten()
    .unwrap_or_default()
}

/// `get_twin_setting("ui.camera_status")` — read a scalar setting from the
/// active Twin manifest. Missing keys, plain folders, and no active Twin are
/// represented as the backend's unit value by the caller.
#[cfg(feature = "rhai")]
pub fn get_twin_setting<B: ValueBuilder>(b: &B, key: &str) -> Option<B::Value> {
    with_world(|world| {
        let workspace = world.get_resource::<lunco_workspace::WorkspaceResource>()?;
        let twin_id = workspace.active_twin?;
        let value = workspace.twin(twin_id)?.manifest.as_ref()?.setting(key)?;
        Some(match value {
            lunco_workspace::TwinSettingValue::Bool(value) => b.bool(*value),
            lunco_workspace::TwinSettingValue::Integer(value) => b.int(*value),
            lunco_workspace::TwinSettingValue::Number(value) => b.float(*value),
            lunco_workspace::TwinSettingValue::Text(value) => b.string(value),
        })
    })
    .flatten()
}

/// Read one scalar from the generic engine exposure registry. This is the
/// language-neutral bridge for Rhai-owned presentation policy: an engine
/// producer publishes facts, and a script can consume them without importing
/// the producer's domain crate.
#[cfg(feature = "rhai")]
pub fn get_exposure<B: ValueBuilder>(b: &B, namespace: &str, property: &str) -> Option<B::Value> {
    with_world(|world| {
        let exposures = world.get_resource::<lunco_core::exposure::EngineExposures>()?;
        let value = exposures
            .surfaces
            .get(namespace)?
            .properties
            .get(property)?;
        Some(match value {
            lunco_core::exposure::ExposureValue::Text(value) => b.string(value),
            lunco_core::exposure::ExposureValue::Bool(value) => b.bool(*value),
            lunco_core::exposure::ExposureValue::Number(value) => b.float(*value),
        })
    })
    .flatten()
}

/// `is_unattended()` — whether NOTHING can take user input this run, so a
/// scenario carrying an autopilot should drive itself. See
/// [`ScenarioAudience`](crate::scenario::ScenarioAudience) for how it's resolved
/// and why it is not the build profile.
///
/// Unresolvable (no such resource — a bare `World`) ⇒ `true`: a world with no
/// scripting plugin has no window either, and an autopilot that runs when it
/// should not is visible, whereas a lesson that silently refuses to run in CI is
/// a green test that tested nothing.
#[cfg(any(feature = "rhai", feature = "python"))]
pub fn is_unattended() -> bool {
    with_world(|w| {
        w.get_resource::<crate::scenario::ScenarioAudience>()
            .copied()
    })
    .flatten()
    .unwrap_or_default()
    .is_unattended()
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
    let seed = gid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ salt.wrapping_mul(0xA076_1D64_78BD_642F);
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
        (
            "severity".to_string(),
            b.string(&format!("{:?}", ev.severity)),
        ),
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
    use bevy::math::DQuat;
    use lunco_api::queries::ApiQueryProvider;
    use lunco_core::session::{AuthorityRole, CommandPolicy, UserSession};

    #[test]
    fn script_pose_reads_are_empty_before_a_simulation_frame_exists() {
        let mut world = World::new();
        world.init_resource::<ApiEntityRegistry>();
        let entity = world.spawn_empty().id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(entity, GlobalEntityId::from_raw(42));

        let _scope = WorldScope::enter(&mut world);
        assert_eq!(world_pos(42), None);
        assert_eq!(world_forward(42), None);
        assert_eq!(world_rotation(42), None);
    }

    #[test]
    fn script_pose_reads_share_the_active_frame_below_rotating_ancestors() {
        let mut world = World::new();
        let world_grid = lunco_core::ensure_world_root(&mut world);
        world.insert_resource(lunco_core::ActivePhysicsFrame(world_grid));
        world.init_resource::<ApiEntityRegistry>();
        let root = world.resource::<lunco_core::ActivePhysicsFrame>().0;
        let body = world
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(100_000, -2_000, 40_000),
                Transform::from_rotation(Quat::from_rotation_x(0.9)),
                ChildOf(root),
            ))
            .id();
        let site = world
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(800, -950, 300),
                Transform::from_rotation(Quat::from_rotation_z(-0.7)),
                ChildOf(body),
            ))
            .id();
        world.insert_resource(lunco_core::ActivePhysicsFrame(site));
        let local_position = DVec3::new(14.0, -1_901.5, -8.0);
        let local_rotation = DQuat::from_rotation_y(0.35);
        let entity = world
            .spawn((
                Transform::from_translation(local_position.as_vec3())
                    .with_rotation(local_rotation.as_quat()),
                ChildOf(site),
            ))
            .id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(entity, GlobalEntityId::from_raw(42));

        let _scope = WorldScope::enter(&mut world);
        let position = world_pos(42).expect("position");
        let forward = world_forward(42).expect("forward");
        let rotation = world_rotation(42).expect("rotation");

        assert!((position - local_position).length() < 1.0e-4);
        assert!((forward - local_rotation * DVec3::NEG_Z).length() < 1.0e-6);
        let rotation = DQuat::from_array(rotation);
        assert!(rotation.angle_between(local_rotation).abs() < 1.0e-6);
    }

    /// §3.4: a `cmd()` from a script launched by a remote session is
    /// re-authorized against that session (same gate as the networked path);
    /// a local/host launch (`None` authority) stays ungated.
    #[test]
    fn scripted_cmd_gated_by_authority() {
        #[lunco_core::Command(default)]
        struct ScriptOpenCommand {}

        #[lunco_core::Command(default)]
        struct ScriptOwnedCommand {
            #[authz_target]
            target: u64,
        }

        let mut world = World::new();
        let type_registry = AppTypeRegistry::default();
        type_registry.write().register::<ScriptOpenCommand>();
        type_registry.write().register::<ScriptOwnedCommand>();
        world.insert_resource(type_registry);
        world.init_resource::<ApiEntityRegistry>();
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
        // valid command, so it dispatches as a fire-and-forget no-op and reports
        // ok; command resolution itself still succeeds.
        set_script_authority(None);
        let r = cmd_raw("ScriptOwnedCommand", serde_json::json!({ "target": 1 }));
        assert_eq!(
            r["ok"],
            serde_json::json!(true),
            "local launch must be ungated"
        );

        // (2) Authenticated Observer + an OPEN command (not in the policy base)
        // → allowed.
        set_script_authority(Some(SessionId(7)));
        let r = cmd_raw("ScriptOpenCommand", serde_json::json!({}));
        assert_eq!(
            r["ok"],
            serde_json::json!(true),
            "OPEN command passes for an authed session"
        );

        // (3) Same Observer + an OWNED_CONTROL command on a target it does NOT
        // own → demands Operator → rejected BEFORE dispatch.
        set_script_authority(Some(SessionId(7)));
        world
            .resource_mut::<CommandPolicyRegistry>()
            .register("ScriptOwnedCommand", CommandPolicy::OWNED_CONTROL);
        let r = cmd_raw("ScriptOwnedCommand", serde_json::json!({ "target": 1 }));
        assert_eq!(
            r["ok"],
            serde_json::json!(false),
            "unowned OWNED_CONTROL must be rejected"
        );
        assert!(r["error"].is_string());

        // (4) Unknown / unauthenticated session → denied even for an OPEN command.
        set_script_authority(Some(SessionId(999)));
        let r = cmd_raw("ScriptOpenCommand", serde_json::json!({}));
        assert_eq!(
            r["ok"],
            serde_json::json!(false),
            "unknown session denied even for OPEN"
        );
    }

    #[test]
    fn scripted_cmd_rejects_unknown_internal_and_hidden_events() {
        #[lunco_core::Command(default)]
        struct HiddenCommand {}

        #[derive(Event, Reflect, Clone, Debug)]
        #[reflect(Event)]
        struct InternalEvent;

        let mut world = World::new();
        let type_registry = AppTypeRegistry::default();
        type_registry.write().register::<HiddenCommand>();
        type_registry.write().register::<InternalEvent>();
        world.insert_resource(type_registry);
        world.init_resource::<ApiEntityRegistry>();
        world.init_resource::<ApiVisibility>();
        world.init_resource::<CommandResults>();
        world.resource_mut::<ApiVisibility>().hide("HiddenCommand");

        let _scope = WorldScope::enter(&mut world);
        set_script_authority(None);

        for name in ["MissingCommand", "InternalEvent", "HiddenCommand"] {
            let result = cmd_raw(name, serde_json::json!({}));
            assert_eq!(result["ok"], serde_json::json!(false), "{name}: {result}");
            assert!(result["error"].is_string(), "{name}: {result}");
        }
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
        let _ = world
            .resource_mut::<SessionRegistry>()
            .claim(SessionId(7), 1);

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

    #[test]
    fn client_scoped_scripts_cannot_use_direct_mutation_paths() {
        let mut world = World::new();
        let _scope = WorldScope::enter(&mut world);
        set_script_client_local(true);

        for capability in [
            capability::PORT_MUTATE,
            capability::FIELD_MUTATE,
            capability::STRUCTURAL_MUTATE,
            capability::SETTING_MUTATE,
        ] {
            let error = enforce_script_mutation(&world, capability, Some(1))
                .expect_err("client-scoped direct mutation must be rejected");
            assert!(error.contains("client-scoped"), "{error}");
        }

        assert!(script_is_client_local());
    }

    #[test]
    fn structured_queries_use_the_read_channel() {
        struct ReadProvider;
        impl ApiQueryProvider for ReadProvider {
            fn name(&self) -> &'static str {
                "ReadProbe"
            }

            fn execute(&self, _world: &World, _params: &serde_json::Value) -> ApiResponse {
                ApiResponse::ok(serde_json::json!({ "value": 7 }))
            }
        }

        let mut world = World::new();
        let mut registry = ApiQueryRegistry::default();
        registry.register(ReadProvider);
        world.insert_resource(registry);
        let _scope = WorldScope::enter(&mut world);

        assert_eq!(
            query_raw("ReadProbe", serde_json::json!({}))
                .expect("read provider execution")
                .expect("read provider result")["value"],
            7
        );
        let error = query_raw("MissingProbe", serde_json::json!({})).expect_err("missing query");
        assert!(error.contains("not registered"), "{error}");
    }
}
