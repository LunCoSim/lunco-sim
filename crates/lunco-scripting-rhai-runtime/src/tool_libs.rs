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
use lunco_hooks::HookValue;
use rhai::Engine;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

/// Policy seam that classifies one loaded Rhai source.
///
/// The Rust side supplies only the canonical asset id. The policy returns a
/// role map such as `#{ role: "prelude" }`, `#{ role: "tool", name: "..." }`,
/// or `#{ role: "ignore" }`. Source categorisation and naming stay authored,
/// while loading and registration remain generic native operations.
pub const SOURCE_CLASSIFY_HOOK: &str = "scripting.source.classify";

#[cfg(test)]
pub(crate) fn registry_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

lunco_hooks::declare_hook! {
    id: SOURCE_CLASSIFY_HOOK,
    owner: "lunco-scripting",
    description: "Classify loaded Rhai sources as prelude, tool, or ignored content.",
    signature: [asset_id: String],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}

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

/// Register the small native tool that is part of the generic scripting
/// substrate. Source-defined tools are installed by the Bevy asset pipeline.
pub(crate) fn register_native_builtins() {
    lunco_tools_rhai::register_native_tool("mathx", vec!["lerp/3".into()], |_engine| {
        let mut m = rhai::Module::new();
        m.set_native_fn("lerp", |a: f64, b: f64, t: f64| Ok(a + (b - a) * t));
        Ok(m)
    });
}

/// Register / hot-replace a rhai-source tool library (the `RegisterToolLibrary`
/// command path). Native/other-backend tools are registered programmatically via
/// [`lunco_tools_rhai`] from host code, not over this string command.
pub fn register_tool_library(name: &str, source: &str) {
    lunco_tools_rhai::register_rhai_tool(name, source);
}

/// Apply the authored source-classification policy to one loaded source.
///
/// `Ok(None)` means the source is ignored or no optional policy is active. A
/// malformed policy result is an explicit source diagnostic; it never turns an
/// arbitrary Rhai file into a prelude or callable library by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSourceRole {
    /// Install this source into the global prelude module.
    Prelude,
    /// Register this source as a named callable tool library.
    Tool(String),
}

pub fn classify_source(asset_id: &str) -> Result<Option<ScriptSourceRole>, String> {
    let Some(result) =
        lunco_hooks::invoke(SOURCE_CLASSIFY_HOOK, &[HookValue::str(asset_id.to_owned())])
    else {
        return Ok(None);
    };
    let value = result.map_err(|error| error.to_string())?;
    let Some(role) = value.get("role").and_then(HookValue::as_str) else {
        return Err("source classification policy returned no string `role`".into());
    };
    match role {
        "prelude" => Ok(Some(ScriptSourceRole::Prelude)),
        "ignore" => Ok(None),
        "tool" => {
            let Some(name) = value.get("name").and_then(HookValue::as_str) else {
                return Err(
                    "source classification policy selected a tool without a string `name`".into(),
                );
            };
            lunco_scripting_rhai_core::names::validate_file_stem(name).map_err(|error| {
                format!("source classification returned invalid tool name: {error}")
            })?;
            Ok(Some(ScriptSourceRole::Tool(name.to_owned())))
        }
        other => Err(format!(
            "source classification policy returned unknown role '{other}'"
        )),
    }
}

// ── Twin persistence (shared tool libraries → files) ─────────────────────────
//
// Per-entity scenarios live embedded in USD prims (a separate path); shared,
// reusable tool sources persist as ordinary files beneath the Twin's authored
// tool source root. The source-classification policy chooses which candidates
// become `name::fn` libraries and what namespace they receive. On Twin open we
// scan that root and register each selected source; the RegisterToolLibrary
// command path can mirror an in-memory registration back to disk via
// [`save_tool_library_file`]. Native-only (no filesystem on wasm).

/// Sub-directory under a Twin root that holds shared rhai tool libraries.
pub const TOOLS_DIR: &str = "tools";

/// Scan the Twin's authored tool source root and return `(asset_id, source)`
/// candidates. The asset id is passed to [`classify_source`] so the policy,
/// rather than this filesystem helper, decides whether to activate a source
/// and what name it receives. A single unreadable file is logged and skipped;
/// it never blocks the rest. Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_tool_sources_from_dir(root: &std::path::Path) -> Vec<(String, String)> {
    let dir = root.join(TOOLS_DIR);
    let mut loaded = Vec::new();
    let entries = match lunco_storage::read_directory_sync(&dir) {
        Ok(entries) => entries,
        // No tools/ dir is the common case (twin has none) — not an error.
        Err(_) => return loaded,
    };
    for path in entries {
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        match lunco_storage::read_file_sync(&path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            Some(source) => {
                loaded.push((format!("{TOOLS_DIR}/{filename}"), source));
            }
            None => warn!("[tool_libs] failed to read {}", path.display()),
        }
    }
    loaded.sort_by(|left, right| left.0.cmp(&right.0));
    loaded
}

/// Persist a tool library's source to `<root>/tools/<name>.rhai` (creating the
/// dir if needed). The on-disk counterpart of [`register_tool_library`], so an
/// interactively-registered library survives a restart. Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_tool_library_file(
    root: &std::path::Path,
    name: &str,
    source: &str,
) -> lunco_storage::StorageResult<std::path::PathBuf> {
    lunco_scripting_rhai_core::names::validate_file_stem(name)
        .map_err(lunco_storage::StorageError::Unsupported)?;
    let dir = root.join(TOOLS_DIR);
    let path = dir.join(format!("{name}.rhai"));
    lunco_storage::write_file_sync(&path, source.as_bytes())?;
    Ok(path)
}

#[cfg(not(target_arch = "wasm32"))]
fn install_twin_tool_sources(
    twin: lunco_workspace::TwinId,
    sources: &[(String, String)],
    scoped: &mut TwinToolLibraries,
) {
    for (asset_id, source) in sources {
        let policy_id = format!("twin://{asset_id}");
        match classify_source(&policy_id) {
            Ok(Some(ScriptSourceRole::Tool(name))) => {
                if let Err(error) = scoped.register(twin, &name, &source) {
                    error!("[tool_libs] failed to install '{name}': {error}");
                }
            }
            Ok(Some(ScriptSourceRole::Prelude)) | Ok(None) => {}
            Err(error) => {
                warn!("[tool_libs] source classification rejected {policy_id}: {error}");
            }
        }
    }
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
    let loaded = load_tool_sources_from_dir(&twin.root);
    #[cfg(not(target_arch = "wasm32"))]
    install_twin_tool_sources(twin_id, &loaded, &mut scoped);
    #[cfg(target_arch = "wasm32")]
    let loaded: Vec<(String, String)> = Vec::new();
    if !loaded.is_empty() {
        info!(
            "[tool_libs] classified {} Twin tool source{}: {loaded:?}",
            loaded.len(),
            if loaded.len() == 1 { "" } else { "s" },
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
    let loaded = load_tool_sources_from_dir(&twin.root);
    #[cfg(not(target_arch = "wasm32"))]
    install_twin_tool_sources(active, &loaded, &mut scoped);
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

/// `GetToolLibrary` `{ name }` → source, discovery, and live binding readiness
/// for one tool. (`source` is null for native tools, which have no textual
/// source.)
struct GetToolLibraryProvider;
impl ApiQueryProvider for GetToolLibraryProvider {
    fn name(&self) -> &'static str {
        "GetToolLibrary"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(name) = params.get("name").and_then(serde_json::Value::as_str) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "GetToolLibrary: `name` required".to_string(),
            );
        };
        match lunco_tools::get(name) {
            Some(tool) => {
                let sources = world
                    .get_resource::<lunco_assets_core::script_source::ScriptSources>()
                    .cloned()
                    .unwrap_or_default();
                let engine = match crate::world_bridge::build_world_engine(sources) {
                    Ok(engine) => engine,
                    Err(error) => return ApiResponse::error(ApiErrorCode::InternalError, error),
                };
                let binding = lunco_tools_rhai::inspect_tool_with_engine(&tool, &engine);
                let scope = world
                    .get_resource::<TwinToolLibraries>()
                    .and_then(TwinToolLibraries::owner)
                    .map(|twin| serde_json::json!({ "kind": "twin", "id": twin.raw() }))
                    .unwrap_or_else(|| serde_json::json!({ "kind": "session" }));
                ApiResponse::ok(serde_json::json!({
                    "name": name,
                    "backend": tool.backend(),
                    "source": tool.source(),
                    "active_twin": world
                        .get_resource::<TwinToolLibraries>()
                        .and_then(TwinToolLibraries::owner)
                        .map(|twin| twin.raw()),
                    "scope": scope,
                    "registry_generation": generation(),
                    "functions": binding.functions,
                    "callable": binding.callable,
                    "diagnostics": binding.diagnostics,
                }))
            }
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

    /// `save_tool_library_file` → `load_tool_sources_from_dir` round-trips;
    /// installation is a separate scoped operation.
    #[test]
    fn tool_library_file_save_load_roundtrip() {
        let _registry_guard = registry_test_guard();
        let temp = tempfile::tempdir().expect("tool library test directory");
        let root = temp.path();

        let src = "fn double(x) { x * 2 }";
        let path = save_tool_library_file(&root, "persist_probe", src).unwrap();
        assert!(lunco_storage::read_file_sync(&path).is_ok());
        assert_eq!(path, root.join("tools").join("persist_probe.rhai"));

        let loaded = load_tool_sources_from_dir(&root);
        assert_eq!(
            loaded,
            vec![("tools/persist_probe.rhai".to_string(), src.to_string())]
        );

        // The scoped owner installs the source into the global binding registry.
        let twin = lunco_workspace::TwinId::new(1);
        let mut scoped = TwinToolLibraries::default();
        scoped.activate(twin);
        scoped
            .register(twin, "persist_probe", &loaded[0].1)
            .unwrap();
        let tool = lunco_tools::get("persist_probe").expect("registered");
        assert_eq!(tool.backend(), "rhai");
        assert_eq!(tool.source(), Some(src));
        assert!(scoped.wind_down_for(twin));
        assert!(lunco_tools::get("persist_probe").is_none());
    }

    /// A missing tool source dir is the common case — yields no libraries, no error.
    #[test]
    fn missing_tools_dir_is_empty_not_error() {
        let _registry_guard = registry_test_guard();
        let temp = tempfile::tempdir().expect("tool library test directory");
        let root = temp.path();
        assert!(load_tool_sources_from_dir(&root).is_empty());
    }
}
