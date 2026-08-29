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

#![cfg(feature = "rhai")]

use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::prelude::*;
use bevy::reflect::TypeInfo;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry, ApiVisibility};
use lunco_api::schema::ApiResponse;

/// World-bridge built-in verbs: `(name, signature, returns, doc)`. Hand-kept in
/// step with the registrations in `world_bridge::build_world_engine` (and the
/// language-neutral logic in `bridge_core`). Same surface in every backend.
const VERBS: &[(&str, &str, &str, &str)] = &[
    (
        "cmd",
        "cmd(name, #{params})",
        "#{ id, ok, data, error }",
        "WRITE. Fire a command by name through ApiCommandEvent — every #[Command] is reachable with no per-command binding. Runs synchronously; `data` carries command-specific result data (a spawned gid, stdout, etc.).",
    ),
    (
        "to_json",
        "to_json(#{value})",
        "string",
        "Serialize a Rhai map/array into JSON for commands whose contract carries a JSON string.",
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
        "[x,y,z] | ()",
        "Pure vector addition; returns () for invalid/non-finite vectors.",
    ),
    (
        "vsub",
        "vsub(a, b)",
        "[x,y,z] | ()",
        "Pure vector subtraction.",
    ),
    (
        "vcross",
        "vcross(a, b)",
        "[x,y,z] | ()",
        "Pure vector cross product.",
    ),
    (
        "vscale",
        "vscale(a, scalar)",
        "[x,y,z] | ()",
        "Pure vector scaling.",
    ),
    (
        "vlen",
        "vlen(a)",
        "f64 | ()",
        "Pure vector length.",
    ),
    (
        "vdot",
        "vdot(a, b)",
        "f64 | ()",
        "Pure vector dot product.",
    ),
    (
        "vnorm",
        "vnorm(a)",
        "[x,y,z] | ()",
        "Pure safe normalization.",
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
        "[x,y,z] | ()",
        "Rotate a vector by an xyzw quaternion.",
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
        "geolocation",
        "geolocation(id)",
        "#{lat, lon, height} | ()",
        "Where on the BODY an entity is — lat/lon in degrees, height in metres (body datum). Works for anything positioned: rover, waypoint, mast, marker. () when the scene has no SiteAnchor.",
    ),
    (
        "world_forward",
        "world_forward(id)",
        "[x, y, z] | ()",
        "Unit forward/heading vector in the active simulation frame.",
    ),
    (
        "world_rotation",
        "world_rotation(id)",
        "[x, y, z, w] | ()",
        "Orientation quaternion in the active simulation frame. Derive any axis rhai-side (up/forward/right = quat * unit); feeds tilt/tip-over checks.",
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
        "READ. Whether a human or autopilot currently controls the entity.",
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
        "Fire a TelemetryEvent on the shared bus; delivered to on_event hooks next tick. `value` may be float/int/bool/string.",
    ),
    (
        "register_hook",
        "register_hook(id, entry, source)",
        "bool",
        "PRIVILEGED POLICY. Install a deterministic Rhai policy hook; requires Operator authority.",
    ),
    (
        "unregister_hook",
        "unregister_hook(id)",
        "bool",
        "PRIVILEGED POLICY. Remove an installed policy hook; requires Operator authority.",
    ),
    (
        "list_hooks",
        "list_hooks()",
        "[#{id, backend, deterministic}]",
        "READ. List installed policy hooks.",
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
        "Monotonic simulation seconds since startup.",
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

fn reflected_surface(world: &World) -> Vec<serde_json::Value> {
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
                                crate::world_bridge::dynamic_write_supported(field.type_path());
                            writable |= field_writable;
                            serde_json::json!({
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
            let type_writable = crate::world_bridge::dynamic_write_supported(short_type);
            Some(serde_json::json!({
                "type": short_type,
                "kind": if is_resource { "resource" } else { "component" },
                "readable": true,
                "writable": (is_component || is_resource) && (type_writable || writable_field),
                "fields": fields,
            }))
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|a, b| a["type"].as_str().cmp(&b["type"].as_str()));
    entries
}

fn prelude_surface() -> Vec<serde_json::Value> {
    let mut engine = rhai::Engine::new();
    // This engine only introspects the embedded prelude. Keep imports
    // fail-closed: completion must not read arbitrary files from the process
    // working directory.
    engine.set_module_resolver(rhai::module_resolvers::StaticModuleResolver::new());
    crate::rhai_limits::apply(&mut engine);
    crate::world_bridge::compile_prelude(&engine)
        .map(|ast| {
            let mut functions: Vec<serde_json::Value> = ast
                .iter_functions()
                .map(|function| {
                    serde_json::json!({
                        "name": function.name,
                        "params": function.params,
                    })
                })
                .collect();
            functions.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            functions
        })
        .unwrap_or_default()
}

fn tool_surface() -> Vec<serde_json::Value> {
    lunco_tools::index()
        .into_iter()
        .map(|info| {
            serde_json::json!({
                "name": info.name,
                "backend": info.backend,
                "functions": info.functions,
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

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let prefix = params
            .get("prefix")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let limit = params
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let mut candidates = VERBS
            .iter()
            .map(|(name, signature, _, doc)| {
                serde_json::json!({
                    "label": name,
                    "kind": "verb",
                    "detail": signature,
                    "documentation": doc,
                })
            })
            .chain(HOOKS.iter().map(
                |(name, doc)| serde_json::json!({ "label": name, "kind": "hook", "detail": doc }),
            ))
            .collect::<Vec<_>>();
        candidates.extend(prelude_surface().into_iter().map(|function| {
            serde_json::json!({
                "label": function["name"],
                "kind": "prelude",
                "detail": function["params"],
            })
        }));
        candidates.extend(tool_surface().into_iter().flat_map(|tool| {
            let namespace = tool["name"].as_str().unwrap_or_default().to_owned();
            let backend = tool["backend"].as_str().unwrap_or_default().to_owned();
            let functions = tool["functions"].as_array().cloned().unwrap_or_default();
            functions.into_iter().filter_map(move |function| {
                let signature = function.as_str()?;
                let function_name = signature.split('/').next().unwrap_or(signature);
                Some(serde_json::json!({
                    "label": format!("{namespace}::{function_name}"),
                    "kind": "tool",
                    "detail": format!("{backend} {signature}"),
                }))
            })
        }));
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let commands = {
            let registry = type_registry.read();
            let visibility = world.get_resource::<ApiVisibility>();
            lunco_api::discover_commands(&registry, visibility)
        };
        candidates.extend(commands.into_iter().map(|command| {
            serde_json::json!({
                "label": command.name,
                "kind": "command",
                "detail": command.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>().join(", "),
            })
        }));
        candidates.extend(
            lunco_api::discover_queries(world.get_resource::<ApiQueryRegistry>())
                .into_iter()
                .map(|query| {
                    serde_json::json!({
                        "label": query,
                        "kind": "query",
                        "detail": "read-only structured provider",
                    })
                }),
        );
        candidates.extend(reflected_surface(world).into_iter().map(|entry| {
            serde_json::json!({
                "label": entry["type"],
                "kind": entry["kind"],
                "detail": "reflected type",
            })
        }));
        candidates.retain(|candidate| {
            candidate["label"]
                .as_str()
                .is_some_and(|label| label.to_ascii_lowercase().starts_with(&prefix))
        });
        candidates.sort_unstable_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
        candidates.truncate(limit);
        ApiResponse::ok(serde_json::json!({ "prefix": prefix, "candidates": candidates }))
    }
}

/// Lifecycle hooks a persistent scenario *defines* (not verbs it calls).
const HOOKS: &[(&str, &str)] = &[
    (
        "task",
        "fn task(me) — returns the native task tree; action/predicate leaves are anonymous |me| closures, and the behavior kernel binds this and advances the tree every fixed step.",
    ),
    (
        "mission",
        "fn mission(me) — returns declarative objectives evaluated alongside the task tree.",
    ),
    (
        "on_start",
        "fn on_start(me) — called once after (re)compile; `me` is the host entity id and `this` is persistent scenario state.",
    ),
    (
        "on_tick",
        "fn on_tick(me) — test-only fixed-step observer for sampling state and publishing a bounded verdict; production missions use task/events.",
    ),
    (
        "on_stop",
        "fn on_stop(me) — teardown: called before a hot-reload swaps in a new compile, and when the scenario is detached/despawned (StopScenario). Stop actuators / release here.",
    ),
    (
        "on_event",
        "fn on_event(me, evt) — a TelemetryEvent arrived; evt is #{ name, source, value, severity, timestamp }. `source` = emitter gid (WHICH sensor/script fired — branch on it), `value` = payload (e.g. a zone enter's entrant gid).",
    ),
];

/// `ScriptingCatalog` → the full authoring surface as one document.
struct ScriptingCatalogProvider;

impl ApiQueryProvider for ScriptingCatalogProvider {
    fn name(&self) -> &'static str {
        "ScriptingCatalog"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        // Built-in verbs + hooks (static).
        let verbs: Vec<serde_json::Value> = VERBS
            .iter()
            .map(|(name, signature, returns, doc)| {
                serde_json::json!({ "name": name, "signature": signature, "returns": returns, "doc": doc })
            })
            .collect();
        let hooks: Vec<serde_json::Value> = HOOKS
            .iter()
            .map(|(name, doc)| serde_json::json!({ "name": name, "doc": doc }))
            .collect();

        // Prelude helpers and tool libraries (incl. file-loaded ones) use the
        // same helpers as `ScriptComplete`, keeping both discovery surfaces in
        // lockstep.
        let prelude = prelude_surface();
        let tools = tool_surface();

        // Reflected commands (cmd targets) — reuse the canonical discovery walk,
        // respecting API visibility so internal commands stay hidden.
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let commands = {
            let reg = type_registry.read();
            let visibility = world.get_resource::<ApiVisibility>();
            lunco_api::discover_commands(&reg, visibility)
        };
        let commands = serde_json::to_value(&commands).unwrap_or_default();

        // Registered read-only providers (query targets), from the same
        // registry the runtime executes.
        let queries = serde_json::to_value(lunco_api::discover_queries(Some(
            world.resource::<ApiQueryRegistry>(),
        )))
        .unwrap_or_default();
        let reflection = reflected_surface(world);

        ApiResponse::ok(serde_json::json!({
            "verbs": verbs,
            "hooks": hooks,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_verbs_hooks_prelude_and_tools() {
        // Bare world with the registries the provider reads.
        let mut app = App::new();
        app.init_resource::<AppTypeRegistry>();
        app.init_resource::<ApiQueryRegistry>();
        // A known tool library so `tools` is non-empty.
        crate::tool_libs::register_tool_library("probe_lib", "fn ping() { 1 }");

        let provider = ScriptingCatalogProvider;
        let resp = provider.execute(app.world_mut(), &serde_json::Value::Null);
        let data = match resp {
            ApiResponse::Ok { data: Some(d), .. } => d,
            other => panic!("expected Ok, got {other:?}"),
        };

        // Verbs include the command and query channels.
        let verbs = data["verbs"].as_array().unwrap();
        for v in ["cmd", "get", "query", "world_pos", "emit"] {
            assert!(
                verbs
                    .iter()
                    .filter_map(|verb| verb["name"].as_str())
                    .any(|name| name == v),
                "missing verb {v}"
            );
        }

        // Hooks present.
        assert!(data["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["name"].as_str())
            .any(|name| name == "on_tick"));
        for entry in ["task", "mission"] {
            assert!(
                data["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|hook| hook["name"].as_str())
                    .any(|name| name == entry),
                "missing policy entrypoint {entry}"
            );
        }

        // Prelude introspected (the embedded prelude defines helpers).
        assert!(
            !data["prelude"].as_array().unwrap().is_empty(),
            "prelude empty"
        );

        // Our registered tool library shows up.
        let tool_names: Vec<&str> = data["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(tool_names.contains(&"probe_lib"), "tools: {tool_names:?}");

        // Commands/queries keys exist (arrays; empty in this bare world is fine).
        assert!(data["commands"].is_array());
        assert!(data["queries"].is_array());
        assert!(data["reflection"].is_array());
    }

    #[test]
    fn reflection_catalog_matches_the_dynamic_write_converter() {
        #[derive(Component, Reflect)]
        #[reflect(Component)]
        struct CatalogProbe {
            supported: Vec3,
            unsupported: u8,
        }

        let mut app = App::new();
        app.init_resource::<AppTypeRegistry>();
        app.register_type::<CatalogProbe>();

        let entries = reflected_surface(app.world());
        let probe = entries
            .iter()
            .find(|entry| entry["type"] == "CatalogProbe")
            .expect("registered component is discoverable");
        assert_eq!(probe["writable"], serde_json::json!(true));
        let fields = probe["fields"].as_array().expect("field catalog");
        assert_eq!(
            fields
                .iter()
                .find(|field| field["name"] == "supported")
                .expect("supported field")["writable"],
            serde_json::json!(true)
        );
        assert_eq!(
            fields
                .iter()
                .find(|field| field["name"] == "unsupported")
                .expect("unsupported field")["writable"],
            serde_json::json!(false)
        );
    }
}
