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
//!   - `commands`— every reflected `#[Command]` (the `cmd("…")` targets) + fields.
//!   - `queries` — every registered `ApiQueryProvider` (the `query("…")` targets).
//!
//! TODO(autocomplete): build the completion *engine* on top of this — a
//! `ScriptComplete { prefix, limit? }` query that filters + ranks this surface
//! into kind-tagged candidates (`{ label, kind, detail }`) so every editor shares
//! one correct, testable matcher instead of re-filtering the raw catalog.
//! Candidate sources are already here (VERBS/HOOKS consts, the prelude AST walk,
//! `lunco_tools::index`, `discover_commands`, the query registry). The egui popup
//! UI is a further, separate consumer — and note the Modelica editor found egui
//! `TextEdit`-overlapping popups fight upstream focus/selection bugs
//! (`lunco-modelica/.../code_editor.rs`), so an external/LSP editor is the better
//! first client. There is currently NO in-app rhai editor at all.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
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
        "WRITE. Fire a command by name through ApiCommandEvent — every #[Command] is reachable with no per-command binding. Runs synchronously; `data` carries handler-assigned values (a spawned gid, etc.).",
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
        "WRITE. The mirror of get(): write a value straight onto a reflected component field (native → reflect, no JSON). Coerces by field type (int→float, [x,y,z]→Vec3). Host-authoritative; replicates via component sync. false on bad path/type.",
    ),
    (
        "get_setting",
        "get_setting(\"Resource.field\")",
        "value | ()",
        "READ. Reflection read of a global Resource field — settings/config live in resources, not components. () if absent.",
    ),
    (
        "set_setting",
        "set_setting(\"Resource.field\", value)",
        "bool",
        "WRITE. The resource twin of set(): tune any reflect-registered Resource field from a scenario, no per-setting command. false on bad path/type.",
    ),
    (
        "query",
        "query(name, #{params})",
        "value | ()",
        "READ. Invoke a registered ApiQueryProvider by name (Raycast, Nearest, …). The read twin of cmd(). () if missing/errored.",
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
        "Entity id with the given Name, or -1 if none.",
    ),
    (
        "name",
        "name(id)",
        "string | ()",
        "The entity's Name (reverse of find).",
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
        "list_entities",
        "list_entities()",
        "[#{ id, name, type, pos, catalog_id }]",
        "Every registered entity with display metadata and exact catalog identity; `catalog_id` is empty when the entity was not catalog-spawned.",
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

/// Lifecycle hooks a persistent scenario *defines* (not verbs it calls).
const HOOKS: &[(&str, &str)] = &[
    (
        "task",
        "fn task(self) — returns the native task tree; the behavior kernel advances it every fixed step.",
    ),
    (
        "mission",
        "fn mission(self) — returns declarative objectives evaluated alongside the task tree.",
    ),
    (
        "on_start",
        "fn on_start(self) — called once after (re)compile; `self` is the host entity id.",
    ),
    (
        "on_tick",
        "fn on_tick(self) — test-only bounded observer; production progression belongs in task(self).",
    ),
    (
        "on_stop",
        "fn on_stop(self) — teardown: called before a hot-reload swaps in a new compile, and when the scenario is detached/despawned (StopScenario). Stop actuators / release here.",
    ),
    (
        "on_event",
        "fn on_event(self, evt) — a TelemetryEvent arrived; evt is #{ name, source, value, severity, timestamp }. `source` = emitter gid (WHICH sensor/script fired — branch on it), `value` = payload (e.g. a zone enter's entrant gid).",
    ),
];

/// `ScriptingCatalog` → the full authoring surface as one document.
struct ScriptingCatalogProvider;

impl ApiQueryProvider for ScriptingCatalogProvider {
    fn name(&self) -> &'static str {
        "ScriptingCatalog"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
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

        // Prelude helpers — compiled and introspected under the runtime policy.
        let prelude: Vec<serde_json::Value> = {
            let mut engine = rhai::Engine::new();
            // Replace rhai's default `FileModuleResolver` before compiling
            // anything: it reads arbitrary files relative to the process CWD,
            // so a prelude `import` would escape the sandbox. The limits do
            // NOT close this hole — the third engine in this crate to need the
            // same pairing (see `backend.rs` and `world_bridge.rs`).
            //
            // Empty resolver rather than `AssetModuleResolver`: this engine only
            // introspects the prelude for the catalog, never runs authored
            // script, so "no import resolves" is the correct fail-closed answer.
            engine.set_module_resolver(rhai::module_resolvers::StaticModuleResolver::new());
            crate::rhai_limits::apply(&mut engine);
            crate::world_bridge::compile_prelude(&engine)
                .map(|ast| {
                    let mut fns: Vec<serde_json::Value> = ast
                        .iter_functions()
                        .map(|f| serde_json::json!({ "name": f.name, "params": f.params }))
                        .collect();
                    fns.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                    fns
                })
                .unwrap_or_default()
        };

        // Tool libraries (incl. file-loaded ones).
        let tools: Vec<serde_json::Value> = lunco_tools::index()
            .into_iter()
            .map(|i| serde_json::json!({ "name": i.name, "backend": i.backend, "functions": i.functions }))
            .collect();

        // Reflected commands (cmd targets) — reuse the canonical discovery walk,
        // respecting API visibility so internal commands stay hidden.
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let commands = {
            let reg = type_registry.read();
            let visibility = world.get_resource::<ApiVisibility>();
            lunco_api::discover_commands(&reg, visibility)
        };
        let commands = serde_json::to_value(&commands).unwrap_or_default();

        // Registered query providers (query targets).
        let mut queries: Vec<String> = world
            .resource::<ApiQueryRegistry>()
            .names()
            .map(|s| s.to_string())
            .collect();
        queries.sort();

        ApiResponse::ok(serde_json::json!({
            "verbs": verbs,
            "hooks": hooks,
            "prelude": prelude,
            "tools": tools,
            "commands": commands,
            "queries": queries,
        }))
    }
}

/// Register the authoring-catalog query. Idempotent re: the registry resource.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ScriptingCatalogProvider);
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

        // Verbs include the three core channels.
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
    }
}
