//! Language-neutral world bridge — the runtime-agnostic core that lets *any*
//! scripting backend read ECS state and drive the simulation.
//!
//! # Why this exists
//!
//! The generic verbs a script gets (`cmd` / `get` / `query` / hierarchy /
//! `emit`) are identical regardless of language. This module owns that
//! mechanism *once*, free of any interpreter type or domain projection, so
//! Rhai and Python are thin bindings over it rather than parallel
//! reimplementations.
//!
//! # Typed values, not JSON in the bridge
//!
//! Reads and command/query calls use typed values throughout this crate:
//!
//! - **Reads** ([`get_field`], resource fields, and hierarchy) read
//!   live reflect data. They build the *native* value in ONE hop via the
//!   [`ValueBuilder`] trait — `reflect → Dynamic` for rhai, `reflect → PyObject`
//!   for Python — never through an intermediate wire value. The
//!   reflect-walker ([`build_from_reflect`]) is written once and monomorphized
//!   per language.
//! - **`cmd` / `query`** route through typed [`lunco_hooks::HookValue`] values.
//!   The API crate owns the one explicit adapter to external JSON/reflection
//!   contracts; this bridge never depends on or constructs JSON.
//!
//! # Execution context
//!
//! Reads are synchronous, so the bridge runs inside a `&mut World`. Registered
//! verbs reach it through a scoped thread-local pointer ([`WorldScope`]), valid
//! only for the duration of one evaluation. Single-threaded (FixedUpdate / wasm)
//! and never re-entrant while a borrow is outstanding, so no aliasing occurs.

use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::math::DVec3;
use bevy::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};

use lunco_api::discovery::find_api_command;
use lunco_api::executor::{
    ApiCommandEvent, authz_target_gid_value, command_result_value, validate_command_params_value,
};
use lunco_api::queries::{
    ApiQueryRegistry, ApiVisibility, SimulationQueryReadScope, execute_query_value,
};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api_core::{ApiValue, api_value_from_u64};
use lunco_command_contracts::{OpId, SessionId};
use lunco_core::{CommandResults, DTransform, GlobalEntityId};
use lunco_core_session::{CommandPolicyRegistry, SessionRbac, SessionRegistry, authorize};
use lunco_hooks::HookValue;
use lunco_telemetry_core::{Severity, TelemetryEvent, TelemetryValue};

// ── Native value construction ──────────────────────────────────────────────

/// How a scripting backend constructs its native values. Implemented once per
/// language (`RhaiBuilder` → `Dynamic`, `PyBuilder` → `PyObject`); the shared
/// reflect/value walkers below are generic over it, so each backend builds
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
    /// An unsigned 64-bit integer.
    fn uint(&self, value: u64) -> Self::Value;
    /// A boolean.
    fn bool(&self, b: bool) -> Self::Value;
    /// A string.
    fn string(&self, s: &str) -> Self::Value;
    /// An ordered array.
    fn array(&self, items: Vec<Self::Value>) -> Self::Value;
    /// A string-keyed map (object).
    fn map(&self, entries: Vec<(String, Self::Value)>) -> Self::Value;
    /// A native semantic two-vector when the backend supports one. The default
    /// keeps wire/serialization builders compatible without forcing them to
    /// know the scripting backend's concrete vector type.
    fn vec2(&self, x: f64, y: f64) -> Self::Value {
        self.array(vec![self.float(x), self.float(y)])
    }

    /// A native semantic three-vector when the backend supports one. The
    /// default keeps wire/serialization builders compatible without forcing
    /// them to know the scripting backend's concrete vector type.
    fn vec3(&self, x: f64, y: f64, z: f64) -> Self::Value {
        self.array(vec![self.float(x), self.float(y), self.float(z)])
    }

    /// A native semantic quaternion when the backend supports one.
    fn quat(&self, x: f64, y: f64, z: f64, w: f64) -> Self::Value {
        self.array(vec![
            self.float(x),
            self.float(y),
            self.float(z),
            self.float(w),
        ])
    }

    /// A native semantic transform when the backend supports one.
    fn transform(&self, transform: DTransform) -> Self::Value {
        self.map(vec![
            (
                "translation".into(),
                self.vec3(
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                ),
            ),
            (
                "rotation".into(),
                self.quat(
                    transform.rotation.x,
                    transform.rotation.y,
                    transform.rotation.z,
                    transform.rotation.w,
                ),
            ),
            (
                "scale".into(),
                self.vec3(transform.scale.x, transform.scale.y, transform.scale.z),
            ),
        ])
    }
    /// An owned byte buffer.
    fn bytes(&self, bytes: &[u8]) -> Self::Value;
}

/// Whether a person can interact with the current scripted run.
///
/// The scenario host resolves this resource from its window/input boundary;
/// the language-neutral bridge only exposes the resolved fact to backends.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScenarioAudience {
    /// No window is available, so an authored program may drive itself.
    #[default]
    Unattended,
    /// A window is available and a person may control the run.
    Attended,
}

impl ScenarioAudience {
    /// Whether the current run has no interactive audience.
    pub fn is_unattended(self) -> bool {
        self == Self::Unattended
    }
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
/// glam vectors/quaternions and transforms use the backend's semantic builder
/// methods. Rhai maps them to the shared f64 math types; transport builders
/// retain arrays/maps at their serialization boundary. Newtype components
/// (e.g. `LinearVelocity(Vec3)`) unwrap to their inner value; structs become
/// maps; lists/arrays/tuples become arrays. Anything still unconvertible
/// (enums, opaque) falls back to its `Debug` string.
pub fn build_from_reflect<B: ValueBuilder>(
    b: &B,
    value: &dyn bevy::reflect::PartialReflect,
) -> Option<B::Value> {
    use bevy::math::{DQuat, DVec2, Quat, Vec2, Vec3};
    use bevy::reflect::ReflectRef;

    if let Some(reflected) = value.try_as_reflect() {
        let any = reflected.as_any();
        // Bevy f32 values widen into the same f64 semantic types used by core
        // calculations; the backend chooses whether that representation is
        // native (Rhai) or lowered (transport).
        if let Some(v) = any.downcast_ref::<Vec3>() {
            return Some(vec3_value(b, v.x as f64, v.y as f64, v.z as f64));
        }
        if let Some(v) = any.downcast_ref::<DVec3>() {
            return Some(b.vec3(v.x, v.y, v.z));
        }
        if let Some(v) = any.downcast_ref::<Vec2>() {
            return Some(vec2_value(b, v.x as f64, v.y as f64));
        }
        if let Some(v) = any.downcast_ref::<DVec2>() {
            return Some(vec2_value(b, v.x, v.y));
        }
        if let Some(v) = any.downcast_ref::<Quat>() {
            return Some(b.quat(v.x as f64, v.y as f64, v.z as f64, v.w as f64));
        }
        if let Some(v) = any.downcast_ref::<DQuat>() {
            return Some(b.quat(v.x, v.y, v.z, v.w));
        }
        if let Some(v) = any.downcast_ref::<DTransform>() {
            return Some(b.transform(*v));
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
            return Some(b.uint(*v));
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
        ReflectRef::Map(m) => {
            // The bridge's native map contract is string-keyed. Preserve that
            // contract for reflected maps and reject non-string keys visibly
            // at the value boundary instead of stringifying an arbitrary key.
            Some(
                b.map(
                    m.iter()
                        .filter_map(|(key, value)| {
                            let key = key
                                .try_as_reflect()
                                .and_then(|key| key.as_any().downcast_ref::<String>())?;
                            Some((key.clone(), build_from_reflect(b, value)?))
                        })
                        .collect(),
                ),
            )
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

/// Convert a typed in-process value into a backend-native value in one pass.
pub fn build_from_value<B: ValueBuilder>(b: &B, value: &ApiValue) -> B::Value {
    match value {
        HookValue::Unit => b.unit(),
        HookValue::Int(value) => b.int(*value),
        HookValue::UInt(value) => b.uint(*value),
        HookValue::Float(value) => b.float(*value),
        HookValue::Bool(value) => b.bool(*value),
        HookValue::Str(value) => b.string(value),
        HookValue::Array(values) => b.array(
            values
                .iter()
                .map(|value| build_from_value(b, value))
                .collect(),
        ),
        HookValue::Map(values) => b.map(
            values
                .iter()
                .map(|(key, value)| (key.clone(), build_from_value(b, value)))
                .collect(),
        ),
        HookValue::Bytes(value) => b.bytes(value),
    }
}

/// Build the backend's semantic two-vector value.
pub fn vec2_value<B: ValueBuilder>(b: &B, x: f64, y: f64) -> B::Value {
    b.vec2(x, y)
}

/// Build the backend's semantic three-vector value.
pub fn vec3_value<B: ValueBuilder>(b: &B, x: f64, y: f64, z: f64) -> B::Value {
    b.vec3(x, y, z)
}

/// Builder for the API-owned typed value used by generic introspection.
pub struct ApiValueBuilder;

impl ValueBuilder for ApiValueBuilder {
    type Value = ApiValue;

    fn unit(&self) -> Self::Value {
        HookValue::Unit
    }
    fn float(&self, value: f64) -> Self::Value {
        HookValue::Float(value)
    }
    fn int(&self, value: i64) -> Self::Value {
        HookValue::Int(value)
    }
    fn uint(&self, value: u64) -> Self::Value {
        lunco_api_core::api_value_from_u64(value)
    }
    fn bool(&self, value: bool) -> Self::Value {
        HookValue::Bool(value)
    }
    fn string(&self, value: &str) -> Self::Value {
        HookValue::Str(value.to_owned())
    }
    fn array(&self, values: Vec<Self::Value>) -> Self::Value {
        HookValue::Array(values)
    }
    fn map(&self, values: Vec<(String, Self::Value)>) -> Self::Value {
        HookValue::Map(values)
    }
    fn bytes(&self, bytes: &[u8]) -> Self::Value {
        HookValue::Bytes(bytes.to_vec())
    }
}

// ── Scoped World access ─────────────────────────────────────────────────────

thread_local! {
    /// Raw pointer to the World currently being scripted. Non-null only while a
    /// [`WorldScope`] guard is alive.
    static WORLD_PTR: Cell<*mut World> = const { Cell::new(std::ptr::null_mut()) };

    /// Typed owner context for synchronous script calls. The context is copied
    /// into each nested call and is never inferred from whichever clock happens
    /// to be installed in the World.
    static EXECUTION_CONTEXT: Cell<lunco_core::RuntimeExecutionContext> =
        const { Cell::new(lunco_core::RuntimeExecutionContext::unclassified()) };

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
    /// and were dropped this hook (see [`cmd_value`]). The scenario driver drains
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
    /// Publish `world` and its owning cycle context for the guard's lifetime.
    pub fn enter(world: &mut World, context: lunco_core::RuntimeExecutionContext) -> Self {
        WORLD_PTR.with(|p| p.set(world as *mut World));
        EXECUTION_CONTEXT.with(|current| current.set(context));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
        SCRIPT_CLIENT_LOCAL.with(|c| c.set(false));
        SCRIPT_REJECTS.with(|r| r.borrow_mut().clear());
        CURRENT_SELF.with(|c| c.set(0));
        WorldScope
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        WORLD_PTR.with(|p| p.set(std::ptr::null_mut()));
        EXECUTION_CONTEXT
            .with(|current| current.set(lunco_core::RuntimeExecutionContext::unclassified()));
        SCRIPT_AUTHORITY.with(|a| a.set(None));
        SCRIPT_CLIENT_LOCAL.with(|c| c.set(false));
        SCRIPT_REJECTS.with(|r| r.borrow_mut().clear());
        CURRENT_SELF.with(|c| c.set(0));
    }
}

/// Temporarily set the phase or event origin for one synchronous script call.
/// Dropping the guard restores its caller's context, including on an error.
pub struct ExecutionContextScope {
    previous: lunco_core::RuntimeExecutionContext,
}

impl ExecutionContextScope {
    /// Set a more specific context for a nested call in the current cycle.
    pub fn enter(context: lunco_core::RuntimeExecutionContext) -> Self {
        let previous = EXECUTION_CONTEXT.with(|current| {
            let previous = current.get();
            current.set(context);
            previous
        });
        Self { previous }
    }
}

impl Drop for ExecutionContextScope {
    fn drop(&mut self) {
        EXECUTION_CONTEXT.with(|current| current.set(self.previous));
    }
}

/// Read the immutable context supplied by the active invocation owner.
pub fn execution_context() -> lunco_core::RuntimeExecutionContext {
    EXECUTION_CONTEXT.with(Cell::get)
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
    ensure_script_mutation_allowed()?;
    validate_simulation_entity_access(world, target_gid, capability, ScriptEntityAccess::Write)?;
    if script_is_client_local() {
        return Err(format!(
            "'{capability}' denied: direct script mutations are not available from a client-scoped script; use an allowed typed command"
        ));
    }
    enforce_script_authority(world, capability, target_gid)
}

/// Direction of direct scenario access to a live entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptEntityAccess {
    Read,
    Write,
}

/// Enforce the scenario's committed entity access plan for simulation-clock
/// world reads and direct writes. Modelica access stays represented by the
/// Modelica participant set because those entities also join the fixed-step
/// causal barrier.
pub fn validate_simulation_entity_access(
    world: &World,
    target_gid: Option<u64>,
    operation: &str,
    access: ScriptEntityAccess,
) -> Result<(), String> {
    if execution_context().clock != lunco_core::RuntimeClock::Simulation {
        return Ok(());
    }
    if execution_context().phase == lunco_core::RuntimePhase::DependencyPlan {
        return Err(format!(
            "simulation_dependencies may resolve entity ids but cannot access live entity state through {operation}"
        ));
    }
    let Some(gid) = target_gid else {
        return Ok(());
    };
    let Some(entity) = resolve_entity(world, gid) else {
        return Ok(());
    };
    let participants = world
        .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
        .ok_or_else(|| {
            format!("{operation} denied: SimulationBarrierParticipants is unavailable")
        })?;
    let scenario = resolve_entity(world, current_self()).ok_or_else(|| {
        format!("{operation} denied: no live scenario owns this simulation access")
    })?;
    let declared_access = match access {
        ScriptEntityAccess::Read => participants.scenario_declares_read(scenario, entity),
        ScriptEntityAccess::Write => participants.scenario_declares_write(scenario, entity),
    };
    let declared = declared_access
        || (participants.is_modelica_participant(entity)
            && participants.scenario_declares_modelica_dependency(scenario, entity));
    if declared {
        return Ok(());
    };
    let access_name = match access {
        ScriptEntityAccess::Read => "read",
        ScriptEntityAccess::Write => "write",
    };
    Err(format!(
        "{operation} targets entity {gid} without this scenario's declared dependency for {access_name}; include it in simulation_dependencies(me, ctx)"
    ))
}

/// Add an entity discovered during simulation execution to the calling
/// scenario's directional access plan.
///
/// The entity must already be live. Modelica entities added here join the
/// scenario barrier when they enter the current Modelica projection.
fn track_simulation_entity_access(gid: i64, access: ScriptEntityAccess) -> Result<(), String> {
    if execution_context().clock != lunco_core::RuntimeClock::Simulation {
        return Err("dynamic entity access declarations require the Simulation clock".into());
    }
    if execution_context().phase == lunco_core::RuntimePhase::DependencyPlan {
        return Err("simulation_dependencies cannot add runtime entity access".into());
    }
    let gid = u64::try_from(gid).map_err(|_| format!("invalid entity id {gid}"))?;
    with_world(|world| {
        let entity = resolve_entity(world, gid).ok_or_else(|| format!("unknown entity {gid}"))?;
        let scenario = resolve_entity(world, current_self())
            .ok_or_else(|| "no live scenario owns this simulation access".to_string())?;
        let mut participants = world
            .get_resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>()
            .ok_or_else(|| "SimulationBarrierParticipants is unavailable".to_string())?;
        let added = match access {
            ScriptEntityAccess::Read => participants.add_scenario_read(scenario, entity),
            ScriptEntityAccess::Write => participants.add_scenario_write(scenario, entity),
        };
        added
            .map(|_| ())
            .ok_or_else(|| "the scenario access plan is not committed".to_string())
    })
    .ok_or_else(|| "no world in scope".to_string())?
}

/// Declare a live entity read discovered during simulation execution.
pub fn track_simulation_entity_read(gid: i64) -> Result<(), String> {
    track_simulation_entity_access(gid, ScriptEntityAccess::Read)
}

/// Declare a live entity write discovered during simulation execution.
pub fn track_simulation_entity_write(gid: i64) -> Result<(), String> {
    track_simulation_entity_access(gid, ScriptEntityAccess::Write)
}

fn validate_simulation_target_dependency(
    world: &World,
    target_gid: Option<u64>,
    operation: &str,
) -> Result<(), String> {
    if execution_context().clock != lunco_core::RuntimeClock::Simulation {
        return Ok(());
    }
    let Some(gid) = target_gid else {
        return Ok(());
    };
    let Some(entity) = resolve_entity(world, gid) else {
        return Ok(());
    };
    let Some(participants) =
        world.get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
    else {
        return Ok(());
    };
    if participants.is_modelica_participant(entity) {
        let scenario = resolve_entity(world, current_self());
        if !scenario.is_some_and(|scenario| {
            participants.scenario_declares_modelica_dependency(scenario, entity)
                || participants.scenario_declares_write(scenario, entity)
        }) {
            return Err(format!(
                "{operation} targets Modelica entity {gid} without this scenario's declared dependency; include it in simulation_dependencies(me, ctx)"
            ));
        }
    }
    Ok(())
}

/// Keep dependency planning free of commands and direct world mutations.
pub fn ensure_script_mutation_allowed() -> Result<(), String> {
    if execution_context().phase == lunco_core::RuntimePhase::DependencyPlan {
        Err("simulation_dependencies is read-only; it may resolve dependency identities but cannot mutate the world".into())
    } else {
        Ok(())
    }
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
        .get_resource::<lunco_core_session::ControlPathRegistry>()
        .cloned()
        .unwrap_or_default();
    authorize(reg, rbac, pol, &paths, session, op, target_gid).map_err(|r| r.to_string())
}

/// The `#[authz_target]` gid a command authorizes against, read from the
/// typed script params via its reflect schema. `None` for a target-less command
/// (or an unknown name).
fn command_target_gid(world: &World, name: &str, params: &ApiValue) -> Result<Option<u64>, String> {
    let app_reg = world.resource::<AppTypeRegistry>();
    let type_reg = app_reg.read();
    Ok(type_reg
        .get_with_short_type_path(name)
        .map(|r| authz_target_gid_value(params, r.type_id(), &type_reg))
        .transpose()?
        .flatten())
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

/// Resolve a reflected global entity id through the authoritative API registry.
pub fn resolve_entity(world: &World, gid: u64) -> Option<Entity> {
    world
        .get_resource::<ApiEntityRegistry>()?
        .resolve(&GlobalEntityId::from_raw(gid))
}

/// The session id currently controlling `gid`, or `None` if nobody owns it. Reads the same
/// [`SessionRegistry`] ownership the possession arbiter uses, so a scenario can
/// answer "is this rover controlled, and by whom?" **uniformly across human and AI**
/// drivers — the observability the audit flagged as missing.
pub fn owner_of(gid: u64) -> Option<u64> {
    with_world(|world| Some(world.get_resource::<SessionRegistry>()?.owner_of(gid)?.0)).flatten()
}

/// The session id used by commands emitted on this peer. Authored policy uses
/// this only to compare generic authority ownership; it does not encode a
/// vehicle, controller, or product-specific role in the engine.
pub fn local_session_id() -> Option<u64> {
    with_world(|world| {
        world
            .get_resource::<lunco_core_session::LocalSession>()
            .map(|session| session.0.0)
    })
    .flatten()
}

/// The role of `gid`'s controlling session — or `None` if unowned. Falls back to
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

fn command_result_error(id: u64, status: &str, error: impl Into<String>) -> ApiValue {
    HookValue::map([
        ("id", api_value_from_u64(id)),
        ("ok", HookValue::Bool(false)),
        ("status", HookValue::Str(status.to_owned())),
        ("error", HookValue::Str(error.into())),
    ])
}

fn insert_map_value(value: &mut ApiValue, key: &str, replacement: ApiValue) -> bool {
    let HookValue::Map(entries) = value else {
        return false;
    };
    if let Some((_, value)) = entries.iter_mut().find(|(name, _)| name == key) {
        *value = replacement;
    } else {
        entries.push((key.to_owned(), replacement));
    }
    true
}

/// Fire a typed command through `ApiCommandEvent` (the same entry point the
/// HTTP API / MCP use) and return its typed `{ id, ok, status, data?, error? }`
/// result. The bridge drains a bounded number of command queues; genuinely
/// asynchronous work returns `status = "pending"` and can be checked with
/// [`command_result`].
pub fn cmd_value(name: &str, mut params: ApiValue) -> ApiValue {
    let id = OpId::new().0;
    with_world(|world| {
        if let Err(error) = ensure_script_mutation_allowed() {
            return command_result_error(id, "rejected", error);
        }
        if world
            .get_resource::<IgnoredScenarioCommands>()
            .is_some_and(|commands| commands.accepts(name))
        {
            return HookValue::map([
                ("id", api_value_from_u64(id)),
                ("ok", HookValue::Bool(true)),
                ("status", HookValue::Str("applied".into())),
            ]);
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
                return command_result_error(id, "rejected", error.message(name));
            }
        };
        let Some(entity_registry) = world.get_resource::<ApiEntityRegistry>() else {
            return command_result_error(id, "failed", "API entity registry is unavailable");
        };
        if let Err(error) =
            validate_command_params_value(name, &params, registration, &type_reg, entity_registry)
        {
            return command_result_error(id, "rejected", error);
        }
        drop(type_reg);

        let target_gid = match command_target_gid(world, name, &params) {
            Ok(target_gid) => target_gid,
            Err(error) => return command_result_error(id, "rejected", error),
        };
        if let Err(error) = validate_simulation_target_dependency(world, target_gid, name) {
            return command_result_error(id, "rejected", error);
        }

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
                    target_gid,
                    world.get_resource::<lunco_core_session::LocalSession>(),
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
                return command_result_error(
                    id,
                    "rejected",
                    format!("`{name}` is not permitted from a client-scoped script"),
                );
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
                if let Some(gid) = target_gid {
                    let tick = world
                        .get_resource::<lunco_core_runtime::SimTick>()
                        .map_or(0, |t| t.0);
                    let seq = world
                        .get_resource_mut::<lunco_core_session::OwnedInputLog>()
                        .map(|mut log| {
                            let entry = log.0.entry(gid).or_default();
                            entry.next_seq = entry.next_seq.wrapping_add(1); // seq 0 reserved
                            entry.next_seq
                        });
                    if let Some(seq) = seq {
                        insert_map_value(&mut params, "seq", api_value_from_u64(seq.into()));
                        insert_map_value(&mut params, "tick", api_value_from_u64(tick));
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
            if let Err(error) = enforce_script_authority(world, name, target_gid) {
                return command_result_error(id, "rejected", error);
            }
        }
        world.trigger(ApiCommandEvent {
            command: name.to_string(),
            params,
            id,
            correlation_id: None,
        });
        // The dispatcher and the typed handler both use `commands.queue`: the
        // first flush runs the reflected command, while the handler's queued
        // work is a second queue generation. Drain a small bounded number of
        // generations so Rhai receives the terminal validation error instead
        // of mistaking a queued-but-rejected edit for success. Truly async
        // commands still return their id without blocking the simulation.
        for _ in 0..4 {
            world.flush();
            if world
                .get_resource::<CommandResults>()
                .is_some_and(|results| results.get(id).is_some())
            {
                break;
            }
        }
        let outcome = world
            .get_resource::<CommandResults>()
            .and_then(|r| r.get(id).cloned());
        command_result_value(id, outcome.as_ref())
    })
    .unwrap_or_else(|| command_result_error(id, "failed", "no world in scope"))
}

/// `cmd` as a native value: fire a typed command and lower the typed result.
pub fn cmd<B: ValueBuilder>(b: &B, name: &str, params: ApiValue) -> B::Value {
    build_from_value(b, &cmd_value(name, params))
}

/// Read the terminal state of a command issued by `cmd()`. Deferred handlers
/// may finish on a later world flush; exposing the shared command-result store
/// lets Rhai tests and tools wait for that result without treating acceptance
/// as success.
pub fn command_result_value_for_id(id: u64) -> ApiValue {
    with_world(|world| {
        let outcome = world
            .get_resource::<CommandResults>()
            .and_then(|results| results.get(id));
        command_result_value(id, outcome)
    })
    .unwrap_or_else(|| command_result_error(id, "failed", "no world in scope"))
}

/// Native-value wrapper for [`command_result_value_for_id`].
pub fn command_result<B: ValueBuilder>(b: &B, id: u64) -> B::Value {
    build_from_value(b, &command_result_value_for_id(id))
}

// ── Verbs: query ────────────────────────────────────────────────────────────

/// Invoke a registered query by name using typed in-process values.
///
/// `Ok(None)` is a successful provider response with no data. Missing providers
/// and provider errors are `Err`, so callers can distinguish an empty answer
/// from a broken or unavailable query surface.
pub fn query_value(name: &str, params: ApiValue) -> Result<Option<ApiValue>, String> {
    let access = with_world(|world| -> Result<(), String> {
        if execution_context().clock != lunco_core::RuntimeClock::Simulation {
            return Ok(());
        }
        let Some(provider) = world
            .get_resource::<ApiQueryRegistry>()
            .and_then(|registry| registry.get(name))
        else {
            return Ok(());
        };
        match provider.simulation_read_scope(&params) {
            SimulationQueryReadScope::EntityTargets => {}
            SimulationQueryReadScope::SceneGeneration => {
                validate_simulation_scene_query_access(world, name)?;
            }
            SimulationQueryReadScope::ScenarioDeclared => {
                validate_simulation_query_read(world, name)?;
            }
        }
        for target in provider.simulation_entity_reads(&params) {
            validate_simulation_entity_access(
                world,
                Some(target.get()),
                name,
                ScriptEntityAccess::Read,
            )?;
        }
        Ok(())
    });
    if let Some(Err(error)) = access {
        return Err(format!("query '{name}' denied: {error}"));
    }
    with_world(|world| execute_query_value(world, name, &params))
        .ok_or_else(|| "no world in scope".to_string())?
        .map_err(|error| {
            format!(
                "query '{name}' failed ({}): {}",
                error.code as u16, error.message
            )
        })
}

/// `query` as a native value. Successful data remains the provider's native
/// value; a successful no-data response is unit. Errors become an explicit
/// `#{ ok: false, error: "..." }` value so a script can branch without losing
/// the provider's diagnostic.
pub fn query<B: ValueBuilder>(b: &B, name: &str, params: ApiValue) -> B::Value {
    match query_value(name, params) {
        Ok(Some(data)) => build_from_value(b, &data),
        Ok(None) => b.unit(),
        Err(error) => b.map(vec![
            ("ok".to_string(), b.bool(false)),
            ("error".to_string(), b.string(&error)),
        ]),
    }
}

// ── Verbs: ports ──────────────────────────────────────────────────────────────
//
// The co-sim **port registry** ([`lunco_port_core::ports::PortRegistry`]) is the one
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
        let registry = world.get_resource::<lunco_port_core::ports::PortRegistry>()?;
        registry.read_port(world, entity, name)
    })
    .flatten()
}

fn validate_simulation_scene_query_access(world: &World, operation: &str) -> Result<(), String> {
    let context = execution_context();
    let Some(route) = context.route else {
        return Err(format!(
            "{operation} denied: Simulation query requires a committed Twin scene generation"
        ));
    };
    if route.scope != lunco_core::RuntimeScope::Twin
        || route.cycle != lunco_core::RuntimeCycle::Simulation
    {
        return Err(format!(
            "{operation} denied: Simulation query requires a Twin Simulation route"
        ));
    }
    let coordinator = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .ok_or_else(|| {
            format!("{operation} denied: scene transition coordinator is unavailable")
        })?;
    if coordinator.active_id().is_some()
        || coordinator.completed_generation() != Some(route.generation)
    {
        return Err(format!(
            "{operation} denied: query route generation {} is not the committed scene generation",
            route.generation
        ));
    }
    Ok(())
}

fn validate_simulation_query_read(world: &World, operation: &str) -> Result<(), String> {
    if execution_context().phase == lunco_core::RuntimePhase::DependencyPlan {
        return Err(format!(
            "{operation} denied: broad API queries cannot run while simulation_dependencies is being resolved"
        ));
    }
    let participants = world
        .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
        .ok_or_else(|| {
            format!("{operation} denied: SimulationBarrierParticipants is unavailable")
        })?;
    let scenario = resolve_entity(world, current_self()).ok_or_else(|| {
        format!("{operation} denied: no live scenario owns this simulation query")
    })?;
    if participants.scenario_declares_query_read(scenario, operation) {
        return Ok(());
    }
    Err(format!(
        "{operation} denied: broad simulation query is not declared; add its public name to simulation_dependencies `query_reads`"
    ))
}

/// Direction of a scenario's direct port access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptPortAccess {
    /// Read a published output or the current input value.
    Read,
    /// Write an input port.
    Write,
}

/// Reject live-port access during dependency planning and require the calling
/// scenario's Modelica or generic entity access declaration at simulation time.
///
/// Modelica declarations also join the shared barrier. Generic entity
/// declarations authorize access only; USD causal edges contribute to the
/// shared barrier but do not authorize a scenario's access. Port access from
/// application/presentation cycles observes committed state without joining
/// the authoritative simulation barrier.
pub fn validate_simulation_port_access(
    gid: u64,
    name: &str,
    access: ScriptPortAccess,
) -> Result<(), String> {
    if execution_context().clock != lunco_core::RuntimeClock::Simulation {
        return Ok(());
    }
    let phase = execution_context().phase;
    with_world(|world| {
        let Some(entity) = resolve_entity(world, gid) else {
            return Ok(());
        };
        let Some(registry) = world.get_resource::<lunco_port_core::ports::PortRegistry>() else {
            return Ok(());
        };
        let is_port = match access {
            ScriptPortAccess::Read => {
                registry.has_output_port(world, entity, name)
                    || registry.has_input_port(world, entity, name)
            }
            ScriptPortAccess::Write => registry.has_input_port(world, entity, name),
        };
        if !is_port {
            return Ok(());
        }
        if phase == lunco_core::RuntimePhase::DependencyPlan {
            return Err(format!(
                "simulation_dependencies may resolve entity ids but cannot read or write live port {name:?} on entity {gid}"
            ));
        }
        validate_simulation_entity_access(
            world,
            Some(gid),
            &format!("Modelica port {name:?}"),
            match access {
                ScriptPortAccess::Read => ScriptEntityAccess::Read,
                ScriptPortAccess::Write => ScriptEntityAccess::Write,
            },
        )
    })
    .unwrap_or(Ok(()))
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
            .get_resource::<lunco_port_core::ports::PortRegistry>()
            .cloned()
        else {
            return false;
        };
        registry.write_port(world, entity, name, value)
    })
    .unwrap_or(false)
}

// ── Verbs: reads ────────────────────────────────────────────────────────────

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

/// `find(name)` — first entity gid with that canonical `Name`, or `-1`.
pub fn find(name: &str) -> i64 {
    with_world(|world| {
        let Some(registry) = world.get_resource::<ApiEntityRegistry>() else {
            return -1;
        };
        registry
            .entities_unordered()
            .into_iter()
            .filter(|(_, entity)| world.get::<Name>(*entity).map(|n| n.as_str()) == Some(name))
            .map(|(gid, _)| gid.get())
            .min()
            .map(|id| id as i64)
            .unwrap_or(-1)
    })
    .unwrap_or(-1)
}

/// `name(id)` — the entity's shared human-readable label, or `None`.
pub fn name_of(gid: u64) -> Option<String> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        Some(lunco_core::entity_display_name(
            world.get::<Name>(entity),
            world.get::<lunco_core::markers::Callsign>(entity),
            world.get::<lunco_core::CatalogEntryId>(entity),
        ))
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
#[cfg(feature = "workspace")]
pub fn twin_root() -> String {
    with_world(|w| {
        let ws = w.get_resource::<lunco_workspace::WorkspaceResource>()?;
        let id = ws.0.active_twin?;
        Some(ws.0.twin(id)?.root.to_string_lossy().into_owned())
    })
    .flatten()
    .unwrap_or_default()
}

/// `twin_name()` — stable `twin://` authority of the ACTIVE Twin, or `""`.
///
/// This is the URI counterpart to [`twin_root`].  Rhai policy should use this
/// name when asking a provider to resolve Twin-owned source sets so the same
/// script works after the project moves to another machine.
#[cfg(feature = "workspace")]
pub fn twin_name() -> String {
    with_world(|world| {
        let workspace = world.get_resource::<lunco_workspace::WorkspaceResource>()?;
        let id = workspace.0.active_twin?;
        let twin = workspace.0.twin(id)?;
        Some(
            twin.manifest
                .as_ref()
                .map(|manifest| manifest.name.clone())
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    twin.root
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
        )
    })
    .flatten()
    .unwrap_or_default()
}

/// `get_twin_setting("ui.camera_status")` — read a scalar setting from the
/// active Twin manifest. Missing keys, plain folders, and no active Twin are
/// represented as the backend's unit value by the caller.
#[cfg(feature = "workspace")]
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
/// language-neutral bridge for authored presentation policy: an engine
/// producer publishes facts, and a script can consume them without importing
/// the producer's domain crate.
pub fn get_exposure<B: ValueBuilder>(b: &B, namespace: &str, property: &str) -> Option<B::Value> {
    with_world(|world| {
        let exposures = world.get_resource::<lunco_exposure_core::EngineExposures>()?;
        let value = exposures
            .surfaces
            .get(namespace)?
            .properties
            .get(property)?;
        Some(exposure_value_to_native(b, value))
    })
    .flatten()
}

fn exposure_value_to_native<B: ValueBuilder>(
    b: &B,
    value: &lunco_exposure_core::ExposureValue,
) -> B::Value {
    match value {
        lunco_exposure_core::ExposureValue::Text(value) => b.string(value),
        lunco_exposure_core::ExposureValue::Bool(value) => b.bool(*value),
        lunco_exposure_core::ExposureValue::Number(value) => b.float(*value),
        lunco_exposure_core::ExposureValue::Array(values) => b.array(
            values
                .iter()
                .map(|value| exposure_value_to_native(b, value))
                .collect(),
        ),
        lunco_exposure_core::ExposureValue::Map(values) => b.map(
            values
                .iter()
                .map(|(key, value)| (key.clone(), exposure_value_to_native(b, value)))
                .collect(),
        ),
    }
}

/// `is_unattended()` — whether NOTHING can take user input this run, so an
/// authored task program may drive itself. See
/// [`ScenarioAudience`] for how it's resolved
/// and why it is not the build profile.
///
/// Unresolvable (no such resource — a bare `World`) ⇒ `true`: a world with no
/// scripting plugin has no window either, and an authored program that runs when it
/// should not is visible, whereas a lesson that silently refuses to run in CI is
/// a green test that tested nothing.
pub fn is_unattended() -> bool {
    with_world(|w| w.get_resource::<ScenarioAudience>().copied())
        .flatten()
        .unwrap_or_default()
        .is_unattended()
}

// ── Deterministic RNG ───────────────────────────────────────────────────────
//
// Scripts WILL want randomness (scatter, jitter, exploration, retry backoff). A
// wall-clock / OS source would diverge across host and clients and break replay,
// so the bridge gives them a stream that is a pure function of stable inputs:
// the entity's networked `GlobalEntityId`, the producer/owner sequence (or a
// discrete lifecycle seed), and the call order within the hook. Same inputs
// produce the same number on every peer and every re-run. The runtime calls
// `rng_begin` before each hook;
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

/// Seed the per-hook RNG stream from `(gid, optional sequence, salt)`. Event
/// hooks use their producer sequence; discrete lifecycle hooks have no sequence
/// and receive a separately tagged deterministic stream. `salt` decorrelates
/// distinct hooks/events on the same entity.
pub fn rng_begin(gid: u64, sequence: Option<u64>, salt: u64) {
    let sequence_seed = sequence
        .map(|sequence| sequence.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ 1)
        .unwrap_or(0xE703_7ED1_A0B4_28DB);
    let seed = gid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ sequence_seed
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

/// `emit(name, value)` — fire a `TelemetryEvent` on the shared bus. The
/// payload preserves scalar, array, and map structure. Returns whether a World
/// was in scope.
pub fn emit(name: &str, value: TelemetryValue) -> bool {
    with_world(|world| {
        if ensure_script_mutation_allowed().is_err() {
            return;
        }
        world.trigger(TelemetryEvent {
            name: name.to_string(),
            // The emitter = the script whose hook is running (set by rng_begin).
            source: current_self(),
            severity: Severity::Info,
            data: value,
            // The telemetry core stamps this event from MissionClock at the
            // current SimTick before any subscriber observes it.
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick: 0,
        });
    })
    .is_some()
}

/// Build the `{ name, value, severity, timestamp, sim_secs, sim_tick }` event
/// value passed to an `on_event` hook, native to the backend.
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
        ("sim_secs".to_string(), b.float(ev.sim_secs)),
        ("sim_tick".to_string(), b.int(ev.sim_tick as i64)),
    ])
}

/// A `TelemetryValue` as a backend-native scalar value.
pub fn telemetry_value<B: ValueBuilder>(b: &B, v: &TelemetryValue) -> B::Value {
    match v {
        TelemetryValue::F64(x) => b.float(*x),
        TelemetryValue::I64(x) => b.int(*x),
        TelemetryValue::U64(x) => b.uint(*x),
        TelemetryValue::Bool(x) => b.bool(*x),
        TelemetryValue::String(x) => b.string(x),
        TelemetryValue::Array(items) => {
            b.array(items.iter().map(|item| telemetry_value(b, item)).collect())
        }
        TelemetryValue::Map(entries) => b.map(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), telemetry_value(b, value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core_session::{AuthorityRole, CommandPolicy, UserSession};

    #[test]
    fn telemetry_unsigned_values_keep_their_unsigned_type() {
        let value = telemetry_value(&ApiValueBuilder, &TelemetryValue::U64(u64::MAX));
        assert_eq!(value, HookValue::UInt(u64::MAX));
    }

    #[test]
    fn api_value_boundary_lowers_reflected_geometry_to_ordered_arrays() {
        use bevy::math::{Quat, Vec2, Vec3};

        let vector2 = build_from_reflect(&ApiValueBuilder, &Vec2::new(1.0, -2.0)).unwrap();
        assert_eq!(
            vector2,
            HookValue::Array(vec![HookValue::Float(1.0), HookValue::Float(-2.0)])
        );

        let vector3 = build_from_reflect(&ApiValueBuilder, &Vec3::new(1.0, -2.0, 3.5)).unwrap();
        assert_eq!(
            vector3,
            HookValue::Array(vec![
                HookValue::Float(1.0),
                HookValue::Float(-2.0),
                HookValue::Float(3.5),
            ])
        );

        let quaternion =
            build_from_reflect(&ApiValueBuilder, &Quat::from_xyzw(0.0, 0.0, 0.0, 1.0)).unwrap();
        assert_eq!(
            quaternion,
            HookValue::Array(vec![
                HookValue::Float(0.0),
                HookValue::Float(0.0),
                HookValue::Float(0.0),
                HookValue::Float(1.0),
            ])
        );

        assert_eq!(
            build_from_reflect(&ApiValueBuilder, &u64::MAX),
            Some(HookValue::UInt(u64::MAX))
        );
    }

    fn map_value<'a>(value: &'a ApiValue, key: &str) -> Option<&'a HookValue> {
        match value {
            HookValue::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
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

        let _scope = WorldScope::enter(
            &mut world,
            lunco_core::RuntimeExecutionContext::unclassified(),
        );

        // (1) Local/host launch → ungated. No observer is registered for the
        // valid command, so it dispatches as a fire-and-forget no-op and reports
        // ok; command resolution itself still succeeds.
        set_script_authority(None);
        let r = cmd_value(
            "ScriptOwnedCommand",
            HookValue::map([("target", HookValue::Int(1))]),
        );
        assert_eq!(map_value(&r, "ok"), Some(&HookValue::Bool(true)));

        // (2) Authenticated Observer + an OPEN command (not in the policy base)
        // → allowed.
        set_script_authority(Some(SessionId(7)));
        let r = cmd_value("ScriptOpenCommand", HookValue::Map(Vec::new()));
        assert_eq!(map_value(&r, "ok"), Some(&HookValue::Bool(true)));

        // (3) Same Observer + an OWNED_CONTROL command on a target it does NOT
        // own → demands Operator → rejected BEFORE dispatch.
        set_script_authority(Some(SessionId(7)));
        world
            .resource_mut::<CommandPolicyRegistry>()
            .register("ScriptOwnedCommand", CommandPolicy::OWNED_CONTROL);
        let r = cmd_value(
            "ScriptOwnedCommand",
            HookValue::map([("target", HookValue::Int(1))]),
        );
        assert_eq!(map_value(&r, "ok"), Some(&HookValue::Bool(false)));
        assert!(matches!(map_value(&r, "error"), Some(HookValue::Str(_))));

        // (4) Unknown / unauthenticated session → denied even for an OPEN command.
        set_script_authority(Some(SessionId(999)));
        let r = cmd_value("ScriptOpenCommand", HookValue::Map(Vec::new()));
        assert_eq!(map_value(&r, "ok"), Some(&HookValue::Bool(false)));
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

        let _scope = WorldScope::enter(
            &mut world,
            lunco_core::RuntimeExecutionContext::unclassified(),
        );
        set_script_authority(None);

        for name in ["MissingCommand", "InternalEvent", "HiddenCommand"] {
            let result = cmd_value(name, HookValue::Map(Vec::new()));
            assert_eq!(
                map_value(&result, "ok"),
                Some(&HookValue::Bool(false)),
                "{name}"
            );
            assert!(
                matches!(map_value(&result, "error"), Some(HookValue::Str(_))),
                "{name}"
            );
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

        let _scope = WorldScope::enter(
            &mut world,
            lunco_core::RuntimeExecutionContext::unclassified(),
        );

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
        let _scope = WorldScope::enter(
            &mut world,
            lunco_core::RuntimeExecutionContext::unclassified(),
        );
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
    fn discrete_rng_seed_is_repeatable_and_distinct_from_sequence_zero() {
        rng_begin(42, None, 1);
        let first = rng_next_f64();
        rng_begin(42, None, 1);
        assert_eq!(rng_next_f64(), first);

        rng_begin(42, Some(0), 1);
        assert_ne!(rng_next_f64(), first);
    }

    #[test]
    fn nested_script_phase_restores_its_callers_context() {
        let simulation = lunco_core::RuntimeExecutionContext {
            route: Some(lunco_core::RuntimeRoute::twin(
                lunco_core::RuntimeCycle::Simulation,
                7,
            )),
            phase: lunco_core::RuntimePhase::Behavior,
            clock: lunco_core::RuntimeClock::Simulation,
            time_seconds: Some(1.5),
            delta_seconds: Some(1.0 / 60.0),
            sequence: Some(90),
            producer: None,
        };
        let event = simulation
            .with_phase(lunco_core::RuntimePhase::Event)
            .with_producer(lunco_core::RuntimeProducerStamp::simulation(7, 89));
        {
            let _scope = ExecutionContextScope::enter(simulation);
            assert_eq!(execution_context(), simulation);
            {
                let _nested = ExecutionContextScope::enter(event);
                assert_eq!(execution_context(), event);
            }
            assert_eq!(execution_context(), simulation);
        }
        assert_eq!(
            execution_context(),
            lunco_core::RuntimeExecutionContext::unclassified()
        );
    }
}
