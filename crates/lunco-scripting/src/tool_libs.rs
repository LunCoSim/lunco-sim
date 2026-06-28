//! Bevy / API glue for the tool registry.
//!
//! The tool abstraction itself is runtime-agnostic ([`lunco_tools`]); its rhai
//! binding lives in [`lunco_tools_rhai`]. This module is the thin layer that
//! (a) seeds the built-in tools, (b) bridges registration/refresh into the
//! scripting plugin, and (c) exposes tools on the API (discovery queries; the
//! `RegisterToolLibrary` command lives in `commands.rs`). Keeping the API/Bevy
//! deps here keeps the two tool crates lean and reusable.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use rhai::Engine;

/// Built-in rhai tool library (an example, hot-replaceable like any other).
const FORMATION_SRC: &str = include_str!("../rhai/tools/formation.rhai");

/// Seed the built-in tools (idempotent). Call once at plugin build, BEFORE the
/// runtime engine is created, so they bind immediately:
///   - `formation` — a rhai-source library (selection/formation helpers).
///   - `mathx` — a NATIVE (Rust) tool, proving the backend-agnostic abstraction:
///     the same `name::fn(...)` call site works whether the tool is rhai or Rust.
pub fn register_builtins() {
    lunco_tools_rhai::register_rhai_tool("formation", FORMATION_SRC);
    lunco_tools_rhai::register_native_tool(
        "mathx",
        vec!["hypot/2".into(), "lerp/3".into()],
        |_engine| {
            let mut m = rhai::Module::new();
            m.set_native_fn("hypot", |a: f64, b: f64| Ok((a * a + b * b).sqrt()));
            m.set_native_fn("lerp", |a: f64, b: f64, t: f64| Ok(a + (b - a) * t));
            Ok(m)
        },
    );
}

/// Register / hot-replace a rhai-source tool library (the `RegisterToolLibrary`
/// command path). Native/other-backend tools are registered programmatically via
/// [`lunco_tools_rhai`] from host code, not over this string command.
pub fn register_tool_library(name: &str, source: &str) {
    lunco_tools_rhai::register_rhai_tool(name, source);
}

/// Registry generation (changes on every registration) — drives hot-reload.
pub fn generation() -> u64 {
    lunco_tools::generation()
}

/// Bind every registered tool into `engine` as a static module (`name::fn`),
/// logging any that fail (one bad tool never blocks the rest).
pub fn refresh(engine: &mut Engine) {
    for (name, err) in lunco_tools_rhai::refresh(engine) {
        error!("[rhai] tool '{name}' failed to bind: {err}");
    }
}

/// Sorted names of every registered tool.
pub fn library_names() -> Vec<String> {
    lunco_tools::names()
}

// ── API discovery surface (tools as a first-class, inspectable concept) ──────
//
// Registration rides the `RegisterToolLibrary` command; these read-side
// providers let any caller (HTTP API, MCP, a UI, an agent) discover what tools
// exist (with their backend), and read source for source-defined ones — the
// tool analogue of `DiscoverSchema` for commands.

/// `ListToolLibraries` → `{ count, libraries: [{ name, backend, functions }] }`.
struct ListToolLibrariesProvider;
impl ApiQueryProvider for ListToolLibrariesProvider {
    fn name(&self) -> &'static str {
        "ListToolLibraries"
    }
    fn execute(&self, _world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let libs: Vec<serde_json::Value> = lunco_tools::index()
            .into_iter()
            .map(|i| {
                serde_json::json!({
                    "name": i.name,
                    "backend": i.backend,
                    "functions": i.functions,
                })
            })
            .collect();
        ApiResponse::ok(serde_json::json!({ "count": libs.len(), "libraries": libs }))
    }
}

/// `GetToolLibrary` `{ name }` → `{ name, backend, source }` (`source` null for
/// native tools, which have no textual source).
struct GetToolLibraryProvider;
impl ApiQueryProvider for GetToolLibraryProvider {
    fn name(&self) -> &'static str {
        "GetToolLibrary"
    }
    fn execute(&self, _world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(name) = params.get("name").and_then(serde_json::Value::as_str) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "GetToolLibrary: `name` required".to_string(),
            );
        };
        match lunco_tools::get(name) {
            Some(tool) => ApiResponse::ok(serde_json::json!({
                "name": name,
                "backend": tool.backend(),
                "source": tool.source(),
            })),
            None => ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("tool library '{name}' not found"),
            ),
        }
    }
}

/// Register the tool discovery providers into the API query registry.
/// Idempotent re: the registry resource (init-if-absent).
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(ListToolLibrariesProvider);
    reg.register(GetToolLibraryProvider);
}
