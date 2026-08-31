//! Bevy / API glue for the tool registry.
//!
//! The tool abstraction itself is runtime-agnostic ([`lunco_tools`]); its rhai
//! binding lives in [`lunco_tools_rhai`]. This module is the thin layer that
//! (a) seeds the built-in tools, (b) bridges registration and engine binding into the
//! scripting plugin, and (c) exposes tools on the API (discovery queries; the
//! `RegisterToolLibrary` command lives in `commands.rs`). Keeping the API/Bevy
//! deps here keeps the two tool crates lean and reusable.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use rhai::Engine;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Process-global tool registries still need an ECS lifecycle owner. This
/// resource records exactly which names the active Twin installed and what
/// each name replaced, so closing that Twin restores the previous authoritative
/// registry instead of leaving a stale source library callable.
#[derive(Resource, Default)]
pub struct TwinToolLibraries {
    owner: Option<lunco_workspace::TwinId>,
    loaded: HashSet<String>,
    replaced: HashMap<String, Option<Arc<dyn lunco_tools::Tool>>>,
}

impl TwinToolLibraries {
    /// The Twin whose libraries are currently installed.
    pub fn owner(&self) -> Option<lunco_workspace::TwinId> {
        self.owner
    }

    /// Replace the active Twin scope and restore all names from the prior
    /// scope before admitting the new one.
    pub fn activate(&mut self, twin: lunco_workspace::TwinId) {
        self.wind_down();
        self.owner = Some(twin);
    }

    /// Keep the current scope when it already belongs to `twin`; otherwise
    /// perform the full replacement boundary.
    pub fn ensure_active(&mut self, twin: lunco_workspace::TwinId) {
        if self.owner != Some(twin) {
            self.activate(twin);
        }
    }

    /// Install one library into the active Twin scope, snapshotting the
    /// previous definition exactly once for restoration on close.
    pub fn register(
        &mut self,
        twin: lunco_workspace::TwinId,
        name: &str,
        source: &str,
    ) -> Result<(), String> {
        if self.owner != Some(twin) {
            return Err(format!(
                "tool library scope belongs to {:?}, not Twin {:?}",
                self.owner, twin
            ));
        }
        if !self.replaced.contains_key(name) {
            self.replaced
                .insert(name.to_string(), lunco_tools::get(name));
        }
        register_tool_library(name, source);
        self.loaded.insert(name.to_string());
        Ok(())
    }

    /// Restore the names owned by `twin`. A stale close event cannot remove a
    /// replacement Twin's tools.
    pub fn wind_down_for(&mut self, twin: lunco_workspace::TwinId) -> bool {
        if self.owner != Some(twin) {
            return false;
        }
        self.wind_down();
        true
    }

    fn wind_down(&mut self) {
        for name in self.loaded.drain() {
            match self.replaced.remove(&name).flatten() {
                Some(previous) => lunco_tools::register(previous),
                None => {
                    lunco_tools::unregister(&name);
                }
            }
        }
        self.replaced.clear();
        self.owner = None;
    }
}

/// Seed the built-in tools (idempotent). Call once at plugin build, BEFORE the
/// runtime engine is created, so they bind immediately:
///   - every `assets/scripting/tools/*.rhai` — a rhai-source library, name = stem
///     (`formation`, `survey`, `debug_viz`, …). Add one by dropping a file. The
///     files are embedded + enumerated by [`lunco_assets::scripting::tool_libraries`]
///     (the asset-owning crate), so wasm — which has no filesystem — is covered;
///     the runtime Twin scan ([`load_tool_libraries_from_dir`]) is the native-only,
///     user-authored counterpart. The scan only reads source; the active-Twin
///     observer installs it through [`TwinToolLibraries`] so ownership and
///     restoration stay in one place.
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
pub fn load_tool_libraries_from_dir(root: &std::path::Path) -> Vec<(String, String)> {
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
        match lunco_storage::read_file_sync(&path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            Some(source) => {
                loaded.push((name.to_string(), source));
            }
            None => warn!("[tool_libs] failed to read {}", path.display()),
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
) -> lunco_storage::StorageResult<std::path::PathBuf> {
    crate::names::validate_file_stem(name).map_err(lunco_storage::StorageError::Unsupported)?;
    let dir = root.join(TOOLS_DIR);
    let path = dir.join(format!("{name}.rhai"));
    lunco_storage::write_file_sync(&path, source.as_bytes())?;
    Ok(path)
}

/// Observer: on Twin open, replace the active scoped tool libraries with every
/// `tools/*.rhai` file authored by that Twin. A non-active Twin does not alter
/// the process-global registry.
pub fn sync_tools_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut scoped: ResMut<TwinToolLibraries>,
) {
    let twin_id = trigger.event().twin;
    let Some(ws) = ws.as_deref() else {
        return;
    };
    if ws.active_twin != Some(twin_id) {
        return;
    }
    let Some(twin) = ws.twin(twin_id) else {
        return;
    };
    scoped.activate(twin_id);
    #[cfg(not(target_arch = "wasm32"))]
    let loaded = load_tool_libraries_from_dir(&twin.root);
    #[cfg(target_arch = "wasm32")]
    let loaded: Vec<(String, String)> = Vec::new();
    for (name, source) in &loaded {
        if let Err(error) = scoped.register(twin_id, name, source) {
            error!("[tool_libs] failed to install '{name}' for Twin: {error}");
        }
    }
    if !loaded.is_empty() {
        info!(
            "[tool_libs] loaded {} tool librar{} from Twin: {loaded:?}",
            loaded.len(),
            if loaded.len() == 1 { "y" } else { "ies" },
        );
    }
}

/// Observer: restore every tool definition shadowed by the closed active Twin,
/// then install the replacement active Twin's libraries if one remains.
pub fn wind_down_tools_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut scoped: ResMut<TwinToolLibraries>,
) {
    let closed = trigger.event().twin;
    if !scoped.wind_down_for(closed) {
        return;
    }
    let Some(ws) = ws.as_deref() else {
        return;
    };
    let Some(active) = ws.active_twin else {
        return;
    };
    let Some(twin) = ws.twin(active) else {
        return;
    };
    scoped.activate(active);
    #[cfg(not(target_arch = "wasm32"))]
    let loaded = load_tool_libraries_from_dir(&twin.root);
    #[cfg(target_arch = "wasm32")]
    let loaded: Vec<(String, String)> = Vec::new();
    for (name, source) in &loaded {
        if let Err(error) = scoped.register(active, name, source) {
            error!("[tool_libs] failed to install '{name}' for Twin: {error}");
        }
    }
}

/// Registry generation (changes when a tool is registered, replaced, or
/// unregistered) — drives hot-reload.
pub fn generation() -> u64 {
    lunco_tools::generation()
}

/// Bind every registered tool into `engine` as a static module (`name::fn`),
/// logging any that fail (one bad tool never blocks the rest).
pub fn bind_registered_tools(engine: &mut Engine) {
    for (name, err) in lunco_tools_rhai::bind_registered_tools(engine) {
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

    fn execute(&self, _world: &World, _params: &serde_json::Value) -> ApiResponse {
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

    fn execute(&self, _world: &World, params: &serde_json::Value) -> ApiResponse {
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
        for expected in [
            "formation",
            "survey",
            "debug_viz",
            "gizmo",
            "nurbs",
            "mathx",
        ] {
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

    /// `save_tool_library_file` → `load_tool_libraries_from_dir` round-trips;
    /// installation is a separate scoped operation.
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
        assert_eq!(loaded, vec![("persist_probe".to_string(), src.to_string())]);

        // The scoped owner installs the source into the global binding registry.
        let twin = lunco_workspace::TwinId::new(1);
        let mut scoped = TwinToolLibraries::default();
        scoped.activate(twin);
        scoped.register(twin, &loaded[0].0, &loaded[0].1).unwrap();
        let tool = lunco_tools::get("persist_probe").expect("registered");
        assert_eq!(tool.backend(), "rhai");
        assert_eq!(tool.source(), Some(src));
        assert!(scoped.wind_down_for(twin));
        assert!(lunco_tools::get("persist_probe").is_none());

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
