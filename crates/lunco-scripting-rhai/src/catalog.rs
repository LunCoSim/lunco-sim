//! Scripting authoring catalog — the discoverability surface.
//!
//! A single `ScriptingCatalog` query that aggregates *everything* a script can
//! call, so editors (completion / hover / signature help), agents, and docs have
//! one source of truth instead of stitching together `DiscoverSchema` +
//! `ListToolLibraries` + tribal knowledge of the built-in verbs.
//!
//! The catalog is the data layer; wiring it into the editor's autocomplete is a
//! separate (UI) step. Returns:
//!   - `verbs`   — the world-bridge built-ins (`cmd`/`get`/`query`/…) + signatures.
//!   - `hooks`   — the lifecycle and policy entrypoints a scenario *defines*
//!     (`task`, `mission`, `on_event`, …).
//!   - `prelude` — ergonomic helpers authored in `prelude.rhai` (name + params).
//!   - `tools`   — registered `name::fn` tool libraries (incl. file-loaded ones).
//!   - `commands`— every reflected API command (the `cmd("…")` targets) + fields.
//!   - `queries` — every registered read-only provider.
//!   - `reflection` — reflected component/resource types and their fields, with
//!     writable flags derived from the same converter used by `set()`.
//!
//! `ScriptComplete { prefix, limit? }` is the lightweight completion query over
//! this same surface. UI/LSP clients can consume it without reimplementing the
//! matcher.

use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::prelude::*;
use bevy::reflect::TypeInfo;
use lunco_api::queries::{
    ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, ApiVisibility,
};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value, api_value_from_serializable};

fn query_ok(value: ApiValue) -> ApiQueryResult {
    Ok(Some(value))
}

fn query_error(code: ApiErrorCode, message: impl Into<String>) -> ApiQueryResult {
    Err(ApiQueryError::new(code, message))
}

fn required_field<'a>(value: &'a ApiValue, name: &str) -> Result<&'a ApiValue, ApiQueryError> {
    value.get(name).ok_or_else(|| {
        ApiQueryError::new(
            ApiErrorCode::InternalError,
            format!("scripting catalog entry is missing `{name}`"),
        )
    })
}

fn required_str<'a>(value: &'a ApiValue, name: &str) -> Result<&'a str, ApiQueryError> {
    required_field(value, name)?.as_str().ok_or_else(|| {
        ApiQueryError::new(
            ApiErrorCode::InternalError,
            format!("scripting catalog field `{name}` is not a string"),
        )
    })
}

fn push_completion(
    candidates: &mut Vec<(String, ApiValue)>,
    prefix: &str,
    label: String,
    value: ApiValue,
) {
    if label.to_ascii_lowercase().starts_with(prefix) {
        candidates.push((label, value));
    }
}

/// World-bridge built-in verbs: `(name, signature, returns, doc)`. Hand-kept in
/// step with the registrations in `world_bridge::build_world_engine` (and the
/// language-neutral logic in `bridge_core`). Same surface in every backend.
const VERBS: &[(&str, &str, &str, &str)] = &[
    (
        "cmd",
        "cmd(name, #{params})",
        "#{ id, ok, status, data, error }",
        "WRITE. Fire a command by name through ApiCommandEvent — every #[Command] is reachable with no per-command binding. `status` is applied, rejected, failed, or pending; `data` carries command-specific result data (a spawned gid, stdout, etc.). Use command_result(id) when a deferred owner has not finished yet.",
    ),
    (
        "command_result",
        "command_result(id)",
        "#{ id, ok, status, data, error }",
        "READ. Get the shared terminal result of a prior cmd() call. Deferred commands remain status=pending until their owner records applied, rejected, or failed; do not treat acceptance as applied.",
    ),
    (
        "get",
        "get(id, \"Component.field\")",
        "value | ()",
        "READ. Generic reflection read of a live component field. Vectors come back as [x,y,z] arrays; () if absent.",
    ),
    (
        "set",
        "set(id, \"Component.field\", value)",
        "bool",
        "LOCAL WRITE. The mirror of get(): write a supported value straight onto a reflected component field (native → reflect, no JSON). This is host-side tuning, not the authoritative/replicated/undoable command bus; it is authority-gated and false on bad path/type.",
    ),
    (
        "get_setting",
        "get_setting(\"Resource.field\")",
        "value | ()",
        "READ. Reflection read of a global Resource field — settings/config live in resources, not components. () if absent.",
    ),
    (
        "get_twin_setting",
        "get_twin_setting(\"namespace.key\")",
        "value | ()",
        "READ. Read a scalar project-owned setting from the active Twin manifest. () when the Twin or key is absent.",
    ),
    (
        "get_exposure",
        "get_exposure(\"namespace\", \"property\")",
        "value | ()",
        "READ. Read one raw scalar from the generic engine exposure registry. Presentation policy belongs in Rhai; () means the producer or property is unavailable.",
    ),
    (
        "input_binding",
        "input_binding(\"forward\")",
        "string | ()",
        "READ. Resolve a semantic input binding from the active user settings. Tutorials use this for current labels; () means the intent is unbound.",
    ),
    (
        "set_setting",
        "set_setting(\"Resource.field\", value)",
        "bool",
        "LOCAL WRITE. The resource twin of set(): tune a supported reflect-registered Resource field from a host-authoritative scenario. Use cmd() for authoritative, replicated, or undoable changes. false on bad path/type.",
    ),
    (
        "set_twin_setting",
        "set_twin_setting(\"namespace.key\", value)",
        "bool",
        "WRITE. Persist a scalar project-owned setting on the active Twin through SetTwinSetting. false when no Twin is active or the key/value is invalid.",
    ),
    (
        "query",
        "query(name, #{params})",
        "value | ()",
        "READ. Invoke a registered ApiQueryProvider by name (Raycast, Nearest, …). Successful data is returned directly; no-data is (); failures return #{ok:false,error}.",
    ),
    (
        "vadd",
        "vadd(a, b)",
        "Vec3 | [x,y,z] | ()",
        "Pure vector addition. Native Vec3 operands stay in glam; array operands retain the compatibility contract.",
    ),
    (
        "vsub",
        "vsub(a, b)",
        "Vec3 | [x,y,z] | ()",
        "Pure vector subtraction; native Vec3 operands avoid an array round-trip.",
    ),
    (
        "vcross",
        "vcross(a, b)",
        "Vec3 | [x,y,z] | ()",
        "Pure vector cross product; native Vec3 operands stay in glam.",
    ),
    (
        "vscale",
        "vscale(a, scalar)",
        "Vec3 | [x,y,z] | ()",
        "Pure vector scaling; native Vec3 operands are checked in Rust.",
    ),
    (
        "vlen",
        "vlen(a)",
        "f64 | ()",
        "Pure vector length (native Vec3 or array).",
    ),
    (
        "vdot",
        "vdot(a, b)",
        "f64 | ()",
        "Pure vector dot product (native Vec3 or array).",
    ),
    (
        "vnorm",
        "vnorm(a)",
        "Vec3 | [x,y,z] | ()",
        "Pure checked normalization; native Vec3 stays native.",
    ),
    (
        "vec3",
        "vec3(x, y, z)",
        "Vec3",
        "Construct a finite native glam DVec3. Invalid values are a script error.",
    ),
    (
        "vec3_from",
        "vec3_from(value)",
        "Vec3",
        "Admit either a finite Vec3 or a three-element array into the native math path; malformed input is an error.",
    ),
    (
        "vec3_array",
        "vec3_array(value)",
        "[x,y,z]",
        "Explicitly lower a native Vec3 at a report/command boundary.",
    ),
    (
        "quat",
        "quat(x, y, z, w)",
        "Quat",
        "Construct and normalize a finite native glam DQuat. Zero-length input is an error.",
    ),
    (
        "quat_from",
        "quat_from(value)",
        "Quat",
        "Admit either a finite Quat or an xyzw array into the native orientation path.",
    ),
    (
        "quat_array",
        "quat_array(value)",
        "[x,y,z,w]",
        "Explicitly lower a native Quat at a report/command boundary.",
    ),
    (
        "quat_from_euler_xyz_deg",
        "quat_from_euler_xyz_deg(Vec3)",
        "Quat",
        "Build a native Quat from USD rotateXYZ degrees using the shared Rust Euler convention.",
    ),
    (
        "quat_to_euler_xyz_deg",
        "quat_to_euler_xyz_deg(Quat)",
        "Vec3",
        "Decompose a native Quat into the shared USD rotateXYZ degree convention.",
    ),
    (
        "quat_inverse",
        "quat_inverse(Quat)",
        "Quat",
        "Checked native quaternion inverse.",
    ),
    (
        "clamp",
        "clamp(value, lo, hi)",
        "f64",
        "Finite-safe scalar clamp.",
    ),
    (
        "qrot",
        "qrot(quaternion, vector)",
        "Vec3 | [x,y,z] | ()",
        "Rotate a vector by a native Quat or xyzw array; mixed/array calls retain the array form.",
    ),
    (
        "angle_deg",
        "angle_deg(a, b)",
        "f64 | ()",
        "Unsigned angle between directions in degrees.",
    ),
    (
        "yaw_delta_deg",
        "yaw_delta_deg(previous, current)",
        "f64 | ()",
        "Signed per-step heading delta in degrees.",
    ),
    (
        "world_pos",
        "world_pos(id)",
        "[x, y, z] | ()",
        "f64 position in the active simulation frame (site-local on a surface); stable across camera recentering and celestial ancestor motion.",
    ),
    (
        "world_pos3",
        "world_pos3(id)",
        "Vec3 | ()",
        "Native glam position for hot-loop geometry/control code; use vec3_array only when emitting a wire/report value.",
    ),
    (
        "nav_command",
        "nav_command(id, target, speed, radius)",
        "#{ throttle, steer, brake, arrived } | ()",
        "Compute one authored-capability-aware navigation command through the shared host law; () means the authoritative pose or steering geometry is unavailable and the caller must hold brake.",
    ),
    (
        "usd_document_generation",
        "usd_document_generation(doc_id)",
        "u64 | ()",
        "Read the authoritative USD document generation as a cheap structural invalidation clock; perform detailed topology queries only after it changes.",
    ),
    (
        "geolocation",
        "geolocation(id)",
        "#{lat, lon, height} | ()",
        "Where on the BODY an entity is — lat/lon in degrees, height in metres (body datum). Works for any positioned entity, including route points, masts, and markers. () when the scene has no SiteAnchor.",
    ),
    (
        "world_forward",
        "world_forward(id)",
        "[x, y, z] | ()",
        "Unit forward/heading vector in the active simulation frame.",
    ),
    (
        "world_forward3",
        "world_forward3(id)",
        "Vec3 | ()",
        "Native glam heading for hot-loop geometry/control code.",
    ),
    (
        "world_rotation",
        "world_rotation(id)",
        "[x, y, z, w] | ()",
        "Orientation quaternion in the active simulation frame. Derive any axis rhai-side (up/forward/right = quat * unit); feeds tilt/tip-over checks.",
    ),
    (
        "world_rotation_quat",
        "world_rotation_quat(id)",
        "Quat | ()",
        "Native glam orientation for hot-loop geometry/control code; array world_rotation remains the compatibility/report form.",
    ),
    (
        "find",
        "find(name)",
        "id (i64)",
        "Entity id with the given canonical Name, or -1 if none.",
    ),
    (
        "name",
        "name(id)",
        "string | ()",
        "The entity's human-readable presentation label; QueryEntity supplies the canonical USD path.",
    ),
    (
        "parent",
        "parent(id)",
        "id | ()",
        "Parent entity id, or () if no parent / parent unregistered.",
    ),
    (
        "children",
        "children(id)",
        "[id, ...]",
        "Direct, registered child entity ids (empty if none).",
    ),
    (
        "owner_of",
        "owner_of(id)",
        "i64 | ()",
        "READ. Session currently controlling the entity, if any.",
    ),
    (
        "controller",
        "controller(id)",
        "string | ()",
        "READ. Role of the current controller, if any.",
    ),
    (
        "is_controlled",
        "is_controlled(id)",
        "bool",
        "READ. Whether a control session currently owns the entity.",
    ),
    (
        "list_entities",
        "list_entities()",
        "[#{ id, name, type, pos, catalog_id, input_surface, control_bound, celestial_body }]",
        "Every registered entity with display metadata; `catalog_id` is empty when the entity was not catalog-spawned and `input_surface` reports the authoritative InputPorts readiness.",
    ),
    (
        "add",
        "add(id, \"Comp\", #{fields})",
        "bool",
        "STRUCTURAL. Insert/replace a reflected component, built from its default + the field map (native → reflect). The C of CRUD; requires the type to register ReflectDefault. false on bad entity/type/field.",
    ),
    (
        "remove",
        "remove(id, \"Comp\")",
        "bool",
        "STRUCTURAL. Strip a reflected component from an entity. false if absent.",
    ),
    (
        "despawn",
        "despawn(id)",
        "bool",
        "STRUCTURAL. Despawn an entity (+ children); replicates on a networked host. Runtime SPAWN has no generic verb — use cmd(\"SpawnEntity\", #{entry_id, position}) so clients can reconstruct from the catalog.",
    ),
    (
        "emit",
        "emit(name, value?)",
        "bool",
        "Fire a TelemetryEvent on the shared bus; delivered to on_event hooks on the next scenario pass. `value` may be a scalar, array, or map and keeps its typed structure.",
    ),
    (
        "bind_policy",
        "bind_policy(id, entry, source)",
        "#{ id, ok, status, value, error }",
        "PRIVILEGED POLICY. Compile and install an inline Rhai policy into an installable hook seam; requires Operator authority. A rejected replacement removes the old implementation.",
    ),
    (
        "unbind_policy",
        "unbind_policy(id)",
        "#{ id, ok, status, value, error }",
        "PRIVILEGED POLICY. Remove exactly the installed policy implementation; it never restores a hidden fallback. Requires Operator authority.",
    ),
    (
        "invoke_hook",
        "invoke_hook(id, [args])",
        "#{ id, ok, status, value, error }",
        "READ. Invoke a reflected hook with native Rhai values. `status=unavailable` means no implementation; `status=fault` means an installed policy failed.",
    ),
    (
        "list_hooks",
        "list_hooks()",
        "[#{id, owner, description, input, output, policy_file, policy_entry, deterministic, required, installable, declared, installed, backend}]",
        "READ. Reflect every declared hook contract, authored policy binding, and current implementation. `input` and `output` describe the HookValue ABI accepted by invoke_hook.",
    ),
    (
        "policy_status",
        "policy_status()",
        "#{scope, installed, failed, required_failures, error}",
        "READ. Report the last application/Twin policy-set transition, including source/compile failures and whether any failed policy was mandatory.",
    ),
    (
        "subscribe",
        "subscribe(name)",
        "()",
        "OPTIONAL, call in on_start. Deliver ONLY the named event(s) to on_event (default = all). Skips the per-event VM entry for events you don't name. Footgun: an unnamed event won't reach on_event — omit subscribe entirely to get all.",
    ),
    (
        "subscribe_prefix",
        "subscribe_prefix(prefix)",
        "()",
        "OPTIONAL, call in on_start. Deliver every event whose name starts with `prefix` (e.g. \"enter:\" for all zone-enters). Combines with subscribe().",
    ),
    ("sim_tick", "sim_tick()", "i64", "Current FixedUpdate tick."),
    (
        "dt",
        "dt()",
        "f64",
        "Fixed-step integration delta in seconds — multiply rates by this.",
    ),
    (
        "elapsed_seconds",
        "elapsed_seconds()",
        "f64",
        "Admitted simulation seconds derived from SimTick; excludes scheduler overstep while a causal barrier is held.",
    ),
    (
        "clock_snapshot",
        "clock_snapshot()",
        "map",
        "READ. Snapshot of fixed, virtual, physics, mission, wall, clock-tree, transport, and co-simulation clocks. `sim_tick` is the deterministic master; wall time is diagnostic-only.",
    ),
    (
        "param",
        "param(id, key, default?)",
        "f64 | ()",
        "READ. Read the authored USD `lunco:param:<key>` value from ScriptParams.",
    ),
    (
        "twin_root",
        "twin_root()",
        "string",
        "READ. Absolute root of the active Twin, or an empty string.",
    ),
    (
        "twin_name",
        "twin_name()",
        "string",
        "READ. Stable twin:// authority of the active Twin, or an empty string.",
    ),
    (
        "asset_source_relative_uri",
        "asset_source_relative_uri(document, relative)",
        "string | error",
        "READ. Resolve a safe document-relative asset while preserving the document's registered source authority (for example, twin://name). This is URI algebra only; it does not read files.",
    ),
    (
        "is_unattended",
        "is_unattended()",
        "bool",
        "READ. Whether the current run has no interactive controller.",
    ),
    (
        "rand",
        "rand()",
        "f64",
        "Uniform [0,1). DETERMINISTIC — seeded per hook from (entity, tick, hook), so identical on every networked peer and every replay. Use this, never an OS/wall-clock source.",
    ),
    (
        "rand_range",
        "rand_range(lo, hi)",
        "f64",
        "Deterministic uniform float in [lo, hi).",
    ),
    (
        "rand_int",
        "rand_int(lo, hi)",
        "i64",
        "Deterministic uniform integer in [lo, hi) (half-open).",
    ),
];

fn reflected_surface(world: &World) -> Vec<ApiValue> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut entries = registry
        .iter()
        .filter_map(|registration| {
            let is_component = registration.data::<ReflectComponent>().is_some();
            let is_resource = registration.data::<ReflectResource>().is_some();
            if !is_component && !is_resource {
                return None;
            }
            let short_type = registration.type_info().type_path_table().short_path();
            registry.get_with_short_type_path(short_type)?;
            let (fields, writable_field) = match registration.type_info() {
                TypeInfo::Struct(info) => {
                    let mut writable = false;
                    let fields = info
                        .iter()
                        .map(|field| {
                            let field_writable =
                                lunco_scripting_rhai_core::values::dynamic_write_supported(
                                    field.type_path(),
                                );
                            writable |= field_writable;
                            api_value!({
                                "name": field.name(),
                                "type": field.type_path(),
                                "readable": true,
                                "writable": field_writable,
                            })
                        })
                        .collect::<Vec<_>>();
                    (fields, writable)
                }
                _ => (Vec::new(), false),
            };
            let type_writable =
                lunco_scripting_rhai_core::values::dynamic_write_supported(short_type);
            Some(api_value!({
                "type": short_type,
                "kind": if is_resource { "resource" } else { "component" },
                "readable": true,
                "writable": (is_component || is_resource) && (type_writable || writable_field),
                "fields": fields,
            }))
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|a, b| {
        a.get("type")
            .and_then(ApiValue::as_str)
            .cmp(&b.get("type").and_then(ApiValue::as_str))
    });
    entries
}

fn prelude_surface(world: &World) -> Vec<ApiValue> {
    let mut engine = rhai::Engine::new();
    // This engine only introspects prelude text already admitted by the shared
    // asset registry. Keep imports fail-closed: completion must not read
    // arbitrary files from the process working directory.
    engine.set_module_resolver(rhai::module_resolvers::StaticModuleResolver::new());
    lunco_hooks_rhai::rhai_limits::apply(&mut engine);
    let Some(sources) = world.get_resource::<lunco_assets_runtime::script_source::ScriptSources>()
    else {
        return Vec::new();
    };
    lunco_scripting_rhai_world::world_bridge::prelude_files_from_sources(sources)
        .ok()
        .and_then(|files| {
            lunco_scripting_rhai_world::world_bridge::compile_prelude_set_for_runtime(
                &engine, files,
            )
            .ok()
        })
        .map(|ast| {
            let mut functions: Vec<ApiValue> = ast
                .iter_functions()
                .map(|function| {
                    api_value!({
                        "name": function.name,
                        "params": function.params,
                    })
                })
                .collect();
            functions.sort_by(|a, b| {
                a.get("name")
                    .and_then(ApiValue::as_str)
                    .cmp(&b.get("name").and_then(ApiValue::as_str))
            });
            functions
        })
        .unwrap_or_default()
}

fn tool_surface() -> Vec<ApiValue> {
    lunco_tools::index()
        .into_iter()
        .map(|info| {
            api_value!({
                "name": info.name,
                "backend": info.backend,
                "functions": info.functions,
                "scope": info.scope,
            })
        })
        .collect()
}

fn hook_surface() -> Vec<ApiValue> {
    lunco_hooks::catalog()
        .into_iter()
        .map(|hook| {
            api_value!({
                "id": hook.id,
                "owner": hook.owner,
                "description": hook.description,
                "parameters": hook.parameters.into_iter().map(|parameter| {
                    api_value!({
                        "name": parameter.name,
                        "type": parameter.value_type.as_str(),
                    })
                }).collect::<Vec<_>>(),
                "output": hook.output.as_str(),
                "policy_file": hook.policy_file,
                "policy_entry": hook.policy_entry,
                "deterministic": hook.deterministic,
                "required": hook.required,
                "installable": hook.installable,
                "declared": hook.declared,
                "installed": hook.installed,
                "backend": hook.backend,
            })
        })
        .collect()
}

/// Completion query over the same runtime catalog used by authoring tools.
struct ScriptCompleteProvider;

impl ApiQueryProvider for ScriptCompleteProvider {
    fn name(&self) -> &'static str {
        "ScriptComplete"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let prefix = match params.get("prefix") {
            None => String::new(),
            Some(ApiValue::Str(prefix)) => prefix.to_ascii_lowercase(),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ScriptComplete: `prefix` must be a string",
                );
            }
        };
        let limit = match params.get("limit") {
            None => 50,
            Some(ApiValue::Int(limit)) if *limit >= 0 => (*limit as usize).clamp(1, 200),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ScriptComplete: `limit` must be an unsigned integer",
                );
            }
        };
        let mut candidates: Vec<(String, ApiValue)> = Vec::new();
        for (name, signature, _, doc) in VERBS {
            push_completion(
                &mut candidates,
                &prefix,
                (*name).to_owned(),
                api_value!({
                    "label": *name,
                    "kind": "verb",
                    "detail": *signature,
                    "documentation": *doc,
                }),
            );
        }
        for (name, doc) in HOOKS {
            push_completion(
                &mut candidates,
                &prefix,
                (*name).to_owned(),
                api_value!({ "label": *name, "kind": "hook", "detail": *doc }),
            );
        }
        for function in prelude_surface(world) {
            let name = required_str(&function, "name")?.to_owned();
            let params = required_field(&function, "params")?.clone();
            push_completion(
                &mut candidates,
                &prefix,
                name.clone(),
                api_value!({ "label": name, "kind": "prelude", "detail": params }),
            );
        }
        for hook in hook_surface() {
            let name = required_str(&hook, "id")?.to_owned();
            let parameters = match required_field(&hook, "parameters")? {
                ApiValue::Array(parameters) => parameters,
                _ => {
                    return query_error(
                        ApiErrorCode::InternalError,
                        "scripting hook parameter catalog is not an array",
                    );
                }
            };
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    Ok(format!(
                        "{}: {}",
                        required_str(parameter, "name")?,
                        required_str(parameter, "type")?,
                    ))
                })
                .collect::<Result<Vec<_>, ApiQueryError>>()?
                .join(", ");
            let detail = format!("({parameters}) -> {}", required_str(&hook, "output")?);
            let documentation = required_field(&hook, "description")?.clone();
            push_completion(
                &mut candidates,
                &prefix,
                name.clone(),
                api_value!({
                    "label": name,
                    "kind": "policy-hook",
                    "detail": detail,
                    "documentation": documentation,
                }),
            );
        }
        for tool in tool_surface() {
            let namespace = required_str(&tool, "name")?.to_owned();
            let backend = required_str(&tool, "backend")?.to_owned();
            let functions = match required_field(&tool, "functions")? {
                ApiValue::Array(functions) => functions,
                _ => {
                    return query_error(
                        ApiErrorCode::InternalError,
                        "scripting tool function catalog is not an array",
                    );
                }
            };
            for function in functions {
                let Some(signature) = function.as_str() else {
                    return query_error(
                        ApiErrorCode::InternalError,
                        "scripting tool function signature is not a string",
                    );
                };
                let function_name = signature.split('/').next().unwrap_or(signature);
                let label = format!("{namespace}::{function_name}");
                push_completion(
                    &mut candidates,
                    &prefix,
                    label.clone(),
                    api_value!({
                        "label": label,
                        "kind": "tool",
                        "detail": format!("{backend} {signature}"),
                    }),
                );
            }
        }
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let commands = {
            let registry = type_registry.read();
            let visibility = world.get_resource::<ApiVisibility>();
            lunco_api::discover_commands(&registry, visibility)
        };
        for command in commands {
            let name = command.name.clone();
            let detail = command
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            push_completion(
                &mut candidates,
                &prefix,
                name.clone(),
                api_value!({ "label": name, "kind": "command", "detail": detail }),
            );
        }
        for query in lunco_api::discover_queries(world.get_resource::<ApiQueryRegistry>()) {
            push_completion(
                &mut candidates,
                &prefix,
                query.clone(),
                api_value!({
                    "label": query,
                    "kind": "query",
                    "detail": "read-only structured provider",
                }),
            );
        }
        for entry in reflected_surface(world) {
            let name = required_str(&entry, "type")?.to_owned();
            let kind = required_field(&entry, "kind")?.clone();
            push_completion(
                &mut candidates,
                &prefix,
                name.clone(),
                api_value!({ "label": name, "kind": kind, "detail": "reflected type" }),
            );
        }
        candidates.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        candidates.truncate(limit);
        let candidates = candidates
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect::<Vec<_>>();
        query_ok(api_value!({ "prefix": prefix, "candidates": candidates }))
    }
}

/// Lifecycle hooks a persistent scenario *defines* (not verbs it calls).
const HOOKS: &[(&str, &str)] = &[
    (
        "task",
        "fn task(me, ctx) — returns the native task tree; action/predicate leaves are anonymous |me| closures, and the behavior kernel binds this and advances the tree every fixed step.",
    ),
    (
        "mission",
        "fn mission(me, ctx) — returns declarative objectives evaluated alongside the task tree.",
    ),
    (
        "on_start",
        "fn on_start(me, ctx) — called once after (re)compile; `me` is the host entity id, `ctx` is the typed launch context, and `this` is persistent scenario state.",
    ),
    (
        "on_tick",
        "fn on_tick(me, ctx) — test-only fixed-step observer for sampling state and publishing a bounded verdict; production missions use task/events.",
    ),
    (
        "on_stop",
        "fn on_stop(me, ctx) — teardown: called before a hot-reload swaps in a new compile, and when the scenario is detached/despawned (StopScenario). Stop actuators / release here.",
    ),
    (
        "on_event",
        "fn on_event(me, evt, ctx) — a TelemetryEvent arrived; evt is #{ name, source, value, severity, timestamp }, and ctx is the typed launch context. `source` = emitter gid (WHICH sensor/script fired — branch on it), `value` = payload (e.g. a zone enter's entrant gid).",
    ),
];

/// `ScriptingCatalog` → the full authoring surface as one document.
struct ScriptingCatalogProvider;

impl ApiQueryProvider for ScriptingCatalogProvider {
    fn name(&self) -> &'static str {
        "ScriptingCatalog"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        // Built-in verbs + hooks (static).
        let verbs: Vec<ApiValue> = VERBS
            .iter()
            .map(|(name, signature, returns, doc)| {
                api_value!({ "name": *name, "signature": *signature, "returns": *returns, "doc": *doc })
            })
            .collect();
        let hooks: Vec<ApiValue> = HOOKS
            .iter()
            .map(|(name, doc)| api_value!({ "name": *name, "doc": *doc }))
            .collect();
        let policy_hooks = hook_surface();

        // Prelude helpers and tool libraries (incl. file-loaded ones) use the
        // same helpers as `ScriptComplete`, keeping both discovery surfaces in
        // lockstep.
        let prelude = prelude_surface(world);
        let tools = tool_surface();

        // Reflected commands (cmd targets) — reuse the canonical discovery walk,
        // respecting API visibility so internal commands stay hidden.
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let commands = {
            let reg = type_registry.read();
            let visibility = world.get_resource::<ApiVisibility>();
            lunco_api::discover_commands(&reg, visibility)
        };
        let commands = api_value_from_serializable(&commands)?;

        // Registered read-only providers (query targets), from the same
        // registry the runtime executes.
        let queries = api_value!(lunco_api::discover_queries(Some(
            world.resource::<ApiQueryRegistry>(),
        )));
        let reflection = reflected_surface(world);
        let policy_status = lunco_scripting_rhai_world::world_bridge::policy_status_value(world);

        query_ok(api_value!({
            "verbs": verbs,
            "hooks": hooks,
            "policy_hooks": policy_hooks,
            "policy_status": policy_status,
            "prelude": prelude,
            "tools": tools,
            "commands": commands,
            "queries": queries,
            "reflection": reflection,
        }))
    }
}

/// Register the authoring-catalog query. Idempotent re: the registry resource.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ScriptingCatalogProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ScriptCompleteProvider);
}
