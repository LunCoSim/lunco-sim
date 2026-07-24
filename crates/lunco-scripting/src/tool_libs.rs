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

/// Seed the built-in tools (idempotent). Call once at plugin build, BEFORE the
/// runtime engine is created, so they bind immediately:
///   - every `assets/scripting/tools/*.rhai` — a rhai-source library, name = stem
///     (`formation`, `survey`, `debug_viz`, …). Add one by dropping a file. The
///     files are embedded + enumerated by [`lunco_assets::scripting::tool_libraries`]
///     (the asset-owning crate), so wasm — which has no filesystem — is covered;
///     the runtime Twin scan ([`load_tool_libraries_from_dir`]) is the native-only,
///     user-authored counterpart.
///   - `mathx` — a NATIVE (Rust) tool, proving the backend-agnostic abstraction:
///     the same `name::fn(...)` call site works whether the tool is rhai or Rust.
pub fn register_builtins() {
    for (name, src) in lunco_assets::scripting::tool_libraries() {
        lunco_tools_rhai::register_rhai_tool(name, src);
    }
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

// ── Twin persistence (shared tool libraries → files) ─────────────────────────
//
// Per-entity scenarios live embedded in USD prims (a separate path); shared,
// reusable `name::fn` tool libraries persist as plain `<twin>/tools/*.rhai`
// files — the file IS the source of truth, durable across restarts by
// construction. On twin open we scan that dir and register each; the
// `RegisterToolLibrary` command path can mirror an in-memory registration back
// to disk via [`save_tool_library_file`]. Native-only (no filesystem on wasm).

/// Sub-directory under a Twin root that holds shared rhai tool libraries.
pub const TOOLS_DIR: &str = "tools";

/// Scan `<root>/tools/*.rhai` and register each as a tool library (the file
/// stem is the library name). Returns the names loaded. A single unreadable
/// file is logged and skipped — never blocks the rest. Native-only.
// `disallowed_methods` bans `std::fs` because it silently fails on wasm. This fn
// is `cfg(not(wasm32))`, so that failure mode is unreachable — it does not exist
// on the web target at all. Scoped to this fn, not the module, so the lint stays
// live for anything wasm-reachable added here later.
#[allow(clippy::disallowed_methods)]
#[cfg(not(target_arch = "wasm32"))]
pub fn load_tool_libraries_from_dir(root: &std::path::Path) -> Vec<String> {
    let dir = root.join(TOOLS_DIR);
    let mut loaded = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        // No tools/ dir is the common case (twin has none) — not an error.
        Err(_) => return loaded,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                register_tool_library(name, &source);
                loaded.push(name.to_string());
            }
            Err(e) => warn!("[tool_libs] failed to read {}: {e}", path.display()),
        }
    }
    loaded.sort();
    loaded
}

/// Persist a tool library's source to `<root>/tools/<name>.rhai` (creating the
/// dir if needed). The on-disk counterpart of [`register_tool_library`], so an
/// interactively-registered library survives a restart. Native-only.
// See `load_tool_libraries_from_dir` — native-only, so the wasm foot-gun the
// `disallowed_methods` ban guards against cannot occur here.
#[allow(clippy::disallowed_methods)]
#[cfg(not(target_arch = "wasm32"))]
pub fn save_tool_library_file(
    root: &std::path::Path,
    name: &str,
    source: &str,
) -> std::io::Result<std::path::PathBuf> {
    let dir = root.join(TOOLS_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.rhai"));
    // Atomic replace: a kill mid-write must never leave a truncated library.
    // TODO(backlog): migrate this hand-rolled temp+rename to the Storage handle once
    // the crate gains a lunco-storage dependency (writes go through Storage) — see
    // the engineering-backlog doc in docs/architecture (scripting writes via Storage).
    let tmp = dir.join(format!("{name}.rhai.tmp"));
    std::fs::write(&tmp, source)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Observer: on Twin open, load every `tools/*.rhai` library so file-authored
/// tools are available without re-registering. Native-only (the file scan).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_tools_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    ws: Res<lunco_workspace::WorkspaceResource>,
) {
    let twin_id = trigger.event().twin;
    let Some(twin) = ws.twin(twin_id) else {
        return;
    };
    let loaded = load_tool_libraries_from_dir(&twin.root);
    if !loaded.is_empty() {
        info!(
            "[tool_libs] loaded {} tool librar{} from Twin: {loaded:?}",
            loaded.len(),
            if loaded.len() == 1 { "y" } else { "ies" },
        );
    }
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// `register_builtins` discovers EVERY `assets/scripting/tools/*.rhai` (the
    /// drop-a-file contract) plus the native `mathx` — no per-tool code edit.
    #[test]
    fn builtins_scanned_from_embedded_dir() {
        register_builtins();
        let names = lunco_tools::names();
        for expected in ["formation", "survey", "debug_viz", "mathx"] {
            assert!(
                names.contains(&expected.to_string()),
                "missing built-in {expected}"
            );
        }
        // Every embedded tool `.rhai` registered under its stem — future files
        // are picked up automatically, this guards the scan against silent drops.
        for (stem, _) in lunco_assets::scripting::tool_libraries() {
            assert!(
                names.contains(&stem.to_string()),
                "embedded {stem}.rhai not registered"
            );
        }
    }

    /// `save_tool_library_file` → `load_tool_libraries_from_dir` round-trips,
    /// and a loaded library is registered + readable via the registry.
    #[test]
    fn tool_library_file_save_load_roundtrip() {
        // Unique temp root (no tempfile dep; pid keeps parallel runs disjoint).
        let root = std::env::temp_dir().join(format!("lunco_tl_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let src = "fn double(x) { x * 2 }";
        let path = save_tool_library_file(&root, "persist_probe", src).unwrap();
        assert!(path.exists());
        assert_eq!(path, root.join("tools").join("persist_probe.rhai"));

        let loaded = load_tool_libraries_from_dir(&root);
        assert!(loaded.contains(&"persist_probe".to_string()));

        // Registered into the global tool registry, source readable back.
        let tool = lunco_tools::get("persist_probe").expect("registered");
        assert_eq!(tool.backend(), "rhai");
        assert_eq!(tool.source(), Some(src));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A missing `tools/` dir is the common case — yields no libraries, no error.
    #[test]
    fn missing_tools_dir_is_empty_not_error() {
        let root = std::env::temp_dir().join(format!("lunco_tl_none_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(load_tool_libraries_from_dir(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
