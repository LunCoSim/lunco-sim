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
//!   - `hooks`   — the lifecycle functions a scenario *defines* (`on_tick`, …).
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
    ("cmd", "cmd(name, #{params})", "#{ id, ok, data, error }",
     "WRITE. Fire a command by name through ApiCommandEvent — every #[Command] is reachable with no per-command binding. Runs synchronously; `data` carries handler-assigned values (a spawned gid, etc.)."),
    ("get", "get(id, \"Component.field\")", "value | ()",
     "READ. Generic reflection read of a live component field. Vectors come back as [x,y,z] arrays; () if absent."),
    ("set", "set(id, \"Component.field\", value)", "bool",
     "WRITE. The mirror of get(): write a value straight onto a reflected component field (native → reflect, no JSON). Coerces by field type (int→float, [x,y,z]→Vec3). Host-authoritative; replicates via component sync. false on bad path/type."),
    ("get_setting", "get_setting(\"Resource.field\")", "value | ()",
     "READ. Reflection read of a global Resource field — settings/config live in resources, not components. () if absent."),
    ("set_setting", "set_setting(\"Resource.field\", value)", "bool",
     "WRITE. The resource twin of set(): tune any reflect-registered Resource field from a scenario, no per-setting command. false on bad path/type."),
    ("query", "query(name, #{params})", "value | ()",
     "READ. Invoke a registered ApiQueryProvider by name (Raycast, Nearest, …). The read twin of cmd(). () if missing/errored."),
    ("world_pos", "world_pos(id)", "[x, y, z] | ()",
     "Absolute (big_space float-origin) world position of an entity."),
    ("world_forward", "world_forward(id)", "[x, y, z] | ()",
     "Unit forward/heading vector of an entity in world space."),
    ("find", "find(name)", "id (i64)",
     "Entity id with the given Name, or -1 if none."),
    ("name", "name(id)", "string | ()",
     "The entity's Name (reverse of find)."),
    ("parent", "parent(id)", "id | ()",
     "Parent entity id, or () if no parent / parent unregistered."),
    ("children", "children(id)", "[id, ...]",
     "Direct, registered child entity ids (empty if none)."),
    ("list_entities", "list_entities()", "[#{ id, name, type, pos }]",
     "Every registered entity with name/type/pos — filter/score/select in-script."),
    ("add", "add(id, \"Comp\", #{fields})", "bool",
     "STRUCTURAL. Insert/replace a reflected component, built from its default + the field map (native → reflect). The C of CRUD; requires the type to register ReflectDefault. false on bad entity/type/field."),
    ("remove", "remove(id, \"Comp\")", "bool",
     "STRUCTURAL. Strip a reflected component from an entity. false if absent."),
    ("despawn", "despawn(id)", "bool",
     "STRUCTURAL. Despawn an entity (+ children); replicates on a networked host. Runtime SPAWN has no generic verb — use cmd(\"SpawnEntity\", #{entry_id, position}) so clients can reconstruct from the catalog."),
    ("emit", "emit(name, value?)", "bool",
     "Fire a TelemetryEvent on the shared bus; delivered to on_event hooks next tick. `value` may be float/int/bool/string."),
    ("sim_tick", "sim_tick()", "i64", "Current FixedUpdate tick."),
    ("dt", "dt()", "f64", "Fixed-step integration delta in seconds — multiply rates by this."),
    ("elapsed_seconds", "elapsed_seconds()", "f64", "Monotonic simulation seconds since startup."),
    ("rand", "rand()", "f64",
     "Uniform [0,1). DETERMINISTIC — seeded per hook from (entity, tick, hook), so identical on every networked peer and every replay. Use this, never an OS/wall-clock source."),
    ("rand_range", "rand_range(lo, hi)", "f64", "Deterministic uniform float in [lo, hi)."),
    ("rand_int", "rand_int(lo, hi)", "i64", "Deterministic uniform integer in [lo, hi) (half-open)."),
];

/// Lifecycle hooks a persistent scenario *defines* (not verbs it calls).
const HOOKS: &[(&str, &str)] = &[
    ("on_start", "fn on_start(self) — called once after (re)compile; `self` is the host entity id."),
    ("on_tick", "fn on_tick(self) — called every FixedUpdate."),
    ("on_stop", "fn on_stop(self) — teardown: called before a hot-reload swaps in a new compile, and when the scenario is detached/despawned (StopScenario). Stop actuators / release here."),
    ("on_event", "fn on_event(self, evt) — a TelemetryEvent arrived; evt is #{ name, value, severity, timestamp }."),
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

        // Prelude helpers — compiled & introspected (name + param names). A bare
        // engine parses fine; calls resolve at runtime, not compile.
        let prelude: Vec<serde_json::Value> = rhai::Engine::new()
            .compile(crate::world_bridge::PRELUDE)
            .map(|ast| {
                let mut fns: Vec<serde_json::Value> = ast
                    .iter_functions()
                    .map(|f| serde_json::json!({ "name": f.name, "params": f.params }))
                    .collect();
                fns.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                fns
            })
            .unwrap_or_default();

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
        let verb_names: Vec<&str> = verbs.iter().filter_map(|v| v["name"].as_str()).collect();
        for v in ["cmd", "get", "query", "world_pos", "emit"] {
            assert!(verb_names.contains(&v), "missing verb {v}");
        }

        // Hooks present.
        let hook_names: Vec<&str> = data["hooks"].as_array().unwrap()
            .iter().filter_map(|h| h["name"].as_str()).collect();
        assert!(hook_names.contains(&"on_tick"));

        // Prelude introspected (the embedded prelude defines helpers).
        assert!(!data["prelude"].as_array().unwrap().is_empty(), "prelude empty");

        // Our registered tool library shows up.
        let tool_names: Vec<&str> = data["tools"].as_array().unwrap()
            .iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(tool_names.contains(&"probe_lib"), "tools: {tool_names:?}");

        // Commands/queries keys exist (arrays; empty in this bare world is fine).
        assert!(data["commands"].is_array());
        assert!(data["queries"].is_array());
    }
}
