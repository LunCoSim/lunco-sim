//! Bevy / API glue for the tool registry.
//!
//! The tool abstraction itself is runtime-agnostic ([`lunco_tools`]); its rhai
//! binding lives in [`lunco_tools_rhai`]. This module is the thin layer that
//! (a) seeds the built-in tools, (b) bridges registration and engine binding into the
//! scripting plugin, and (c) exposes tools on the API (discovery queries; the
//! `RegisterToolLibrary` command lives in `commands.rs`). Keeping the API/Bevy
//! deps here keeps the two tool crates lean and reusable.

#![cfg(feature = "rhai")]

use bevy::asset::{AssetEvent, AssetLoadFailedEvent};
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiQueryError, ApiQueryResult};
use lunco_api_core::ApiErrorCode;
use lunco_api_core::{ApiValue, api_value};
use lunco_hooks::HookValue;
use rhai::Engine;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

/// Policy seam that classifies one loaded engine-library Rhai source.
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

/// Replace the active Twin's tool-library scope before its policy requests
/// selected source assets.
#[lunco_core::Command(default)]
pub struct ActivateTwinToolScope {
    /// Workspace identity of the active Twin.
    pub twin_id: u64,
}

/// Load and install one indexed tool library selected by Twin Rhai policy.
#[lunco_core::Command(default)]
pub struct LoadTwinToolLibrary {
    /// Workspace identity of the active Twin.
    pub twin_id: u64,
    /// Exact `twin://` authority returned by the asset owner.
    pub name: String,
    /// Library name assigned by policy.
    pub library_name: String,
    /// Indexed Rhai source path relative to the Twin root.
    pub relative_path: String,
}

struct PendingTwinTool {
    handle: Handle<crate::source_asset::RhaiSource>,
    twin: lunco_workspace::TwinId,
    library_name: String,
    relative_path: String,
}

/// Async source assets requested by the authored Twin loading policy.
#[derive(Resource, Default)]
pub struct PendingTwinTools {
    items: Vec<PendingTwinTool>,
    ready: HashSet<bevy::asset::AssetId<crate::source_asset::RhaiSource>>,
    failed: HashMap<bevy::asset::AssetId<crate::source_asset::RhaiSource>, String>,
}

impl PendingTwinTools {
    fn mark_ready(&mut self, id: bevy::asset::AssetId<crate::source_asset::RhaiSource>) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.ready.insert(id);
        }
    }

    fn mark_failed(
        &mut self,
        id: bevy::asset::AssetId<crate::source_asset::RhaiSource>,
        error: String,
    ) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.failed.insert(id, error);
        }
    }

    fn release_twin(&mut self, twin: lunco_workspace::TwinId) {
        self.items.retain(|item| item.twin != twin);
        let live = self
            .items
            .iter()
            .map(|item| item.handle.id())
            .collect::<HashSet<_>>();
        self.ready.retain(|id| live.contains(id));
        self.failed.retain(|id, _| live.contains(id));
    }
}

#[lunco_core::on_command(ActivateTwinToolScope)]
fn on_activate_twin_tool_scope(
    trigger: bevy::ecs::observer::On<ActivateTwinToolScope>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut scoped: ResMut<TwinToolLibraries>,
    mut pending: ResMut<PendingTwinTools>,
) -> Result<lunco_command_contracts::Ack, String> {
    let twin_id = lunco_workspace::TwinId::new(trigger.event().twin_id);
    if workspace
        .as_deref()
        .is_none_or(|workspace| workspace.active_twin != Some(twin_id))
    {
        return Err(format!("Twin {} is not active", trigger.event().twin_id));
    }
    pending.release_twin(twin_id);
    scoped.activate(twin_id);
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

#[lunco_core::on_command(LoadTwinToolLibrary)]
fn on_load_twin_tool_library(
    trigger: bevy::ecs::observer::On<LoadTwinToolLibrary>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<crate::source_asset::RhaiSource>>>,
    mut pending: ResMut<PendingTwinTools>,
) -> Result<lunco_command_contracts::Ack, String> {
    let request = trigger.event();
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let workspace = workspace
        .as_deref()
        .ok_or_else(|| "Workspace is not installed".to_owned())?;
    let twin = workspace
        .twin(twin_id)
        .ok_or_else(|| format!("workspace Twin {} is unavailable", request.twin_id))?;
    if workspace.active_twin != Some(twin_id) {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    lunco_scripting_rhai_core::names::validate_file_stem(&request.library_name)
        .map_err(|error| format!("invalid Twin tool library name: {error}"))?;
    let relative = std::path::Path::new(&request.relative_path);
    if !lunco_assets_path::is_safe_relative_path(&request.relative_path)
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        || relative.parent() != Some(std::path::Path::new(TOOLS_DIR))
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rhai")
    {
        return Err(format!(
            "Twin tool path `{}` must be a safe `tools/*.rhai` file",
            request.relative_path
        ));
    }
    if !twin
        .files()
        .iter()
        .any(|entry| entry.relative_path.as_path() == relative)
    {
        return Err(format!(
            "Twin tool path `{}` is not indexed",
            request.relative_path
        ));
    }
    let authority = roots
        .as_deref()
        .and_then(|roots| roots.name_for_root(&twin.root).ok().flatten())
        .ok_or_else(|| format!("Twin asset authority `{}` is unavailable", request.name))?;
    if authority != request.name {
        return Err(format!(
            "Twin asset authority `{}` does not belong to Twin {}",
            request.name, request.twin_id
        ));
    }
    if pending.items.iter().any(|item| {
        item.twin == twin_id
            && item.relative_path == request.relative_path
            && item.library_name == request.library_name
    }) {
        return Ok(lunco_command_contracts::Ack::new(
            lunco_command_contracts::OpId::new(),
        ));
    }
    let asset_server = asset_server.ok_or_else(|| "AssetServer is not installed".to_owned())?;
    let handle = asset_server.load::<crate::source_asset::RhaiSource>(lunco_assets_core::twin_uri(
        &request.name,
        &request.relative_path,
    ));
    let id = handle.id();
    if assets
        .as_deref()
        .is_some_and(|assets| assets.get(id).is_some())
    {
        pending.ready.insert(id);
    }
    let failed = asset_server
        .get_load_state(id)
        .is_some_and(|state| state.is_failed());
    pending.items.push(PendingTwinTool {
        handle,
        twin: twin_id,
        library_name: request.library_name.clone(),
        relative_path: request.relative_path.clone(),
    });
    if failed {
        pending.mark_failed(id, "the source asset had already failed to load".to_owned());
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

fn mark_pending_twin_tools(
    mut pending: ResMut<PendingTwinTools>,
    mut events: MessageReader<AssetEvent<crate::source_asset::RhaiSource>>,
    mut failures: MessageReader<AssetLoadFailedEvent<crate::source_asset::RhaiSource>>,
) {
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => pending.mark_ready(*id),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => pending.mark_failed(
                *id,
                "Twin tool source asset was removed before reading".to_owned(),
            ),
        }
    }
    for failure in failures.read() {
        pending.mark_failed(failure.id, failure.error.to_string());
    }
}

fn drain_pending_twin_tools(
    mut pending: ResMut<PendingTwinTools>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    assets: Res<Assets<crate::source_asset::RhaiSource>>,
    sources: Option<Res<lunco_assets_runtime::script_source::ScriptSources>>,
    mut scoped: ResMut<TwinToolLibraries>,
) {
    let ready = std::mem::take(&mut pending.ready);
    let failed = std::mem::take(&mut pending.failed);
    let items = std::mem::take(&mut pending.items);
    let mut still_pending = Vec::new();
    for item in items {
        let id = item.handle.id();
        if let Some(error) = failed.get(&id) {
            warn!(
                "[tool_libs] failed to load `{}`: {error}",
                item.relative_path
            );
            continue;
        }
        if !ready.contains(&id) {
            still_pending.push(item);
            continue;
        }
        let Some(source) = assets.get(&item.handle) else {
            warn!(
                "[tool_libs] `{}` became ready without source",
                item.relative_path
            );
            continue;
        };
        if !workspace
            .as_deref()
            .is_some_and(|workspace| workspace.active_twin == Some(item.twin))
            || scoped.owner() != Some(item.twin)
        {
            continue;
        }
        let validation = match crate::world_bridge::validate_tool_library(
            &item.library_name,
            &source.text,
            sources.as_deref().cloned().unwrap_or_default(),
        ) {
            Ok(validation) => validation,
            Err(error) => {
                warn!("[tool_libs] invalid `{}`: {error}", item.relative_path);
                continue;
            }
        };
        if let Err(error) = scoped.register(item.twin, &item.library_name, &source.text) {
            warn!(
                "[tool_libs] cannot install `{}`: {error}",
                item.relative_path
            );
            continue;
        }
        info!(
            "[tool_libs] loaded Twin library `{}` with {} function{}",
            item.library_name,
            validation.len(),
            if validation.len() == 1 { "" } else { "s" },
        );
    }
    pending.items = still_pending;
}

fn wind_down_twin_tool_scope(
    trigger: On<lunco_workspace::TwinClosed>,
    mut pending: ResMut<PendingTwinTools>,
    mut scoped: ResMut<TwinToolLibraries>,
) {
    let closed = trigger.event().twin;
    pending.release_twin(closed);
    scoped.wind_down_for(closed);
}

lunco_core::register_commands!(on_activate_twin_tool_scope, on_load_twin_tool_library);

/// Register typed Twin tool-loading commands and their async source lifecycle.
pub fn register_twin_tool_loading(app: &mut App) {
    app.init_resource::<PendingTwinTools>()
        .add_observer(wind_down_twin_tool_scope)
        .add_systems(
            Update,
            (mark_pending_twin_tools, drain_pending_twin_tools)
                .chain()
                .after(crate::source_asset::RhaiSourceAssetSet),
        );
    register_all_commands(app);
}

/// Register the small native tool that is part of the generic scripting
/// substrate. Source-defined tools are installed by the Bevy asset pipeline.
pub fn register_native_builtins() {
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
// tool source root. Twin Rhai loading policy selects indexed source assets and
// issues typed load commands. The `RegisterToolLibrary` command path can
// mirror an in-memory registration back to disk via [`save_tool_library_file`].

/// Sub-directory under a Twin root that holds shared rhai tool libraries.
pub const TOOLS_DIR: &str = "tools";

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

    fn execute(&self, _world: &World, _params: &ApiValue) -> ApiQueryResult {
        let libs: Vec<ApiValue> = lunco_tools::index()
            .into_iter()
            .map(|i| {
                api_value!({
                    "name": i.name,
                    "backend": i.backend,
                    "functions": i.functions,
                })
            })
            .collect();
        let count = libs.len();
        Ok(Some(api_value!({ "count": count, "libraries": libs })))
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(name) = params.get("name").and_then(ApiValue::as_str) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "GetToolLibrary: `name` required",
            ));
        };
        match lunco_tools::get(name) {
            Some(tool) => {
                let sources = world
                    .get_resource::<lunco_assets_runtime::script_source::ScriptSources>()
                    .cloned()
                    .unwrap_or_default();
                let engine = match crate::world_bridge::build_world_engine(sources) {
                    Ok(engine) => engine,
                    Err(error) => {
                        return Err(ApiQueryError::new(ApiErrorCode::InternalError, error));
                    }
                };
                let binding = lunco_tools_rhai::inspect_tool_with_engine(&tool, &engine);
                let scope = world
                    .get_resource::<TwinToolLibraries>()
                    .and_then(TwinToolLibraries::owner)
                    .map(|twin| api_value!({ "kind": "twin", "id": twin.raw() }))
                    .unwrap_or_else(|| api_value!({ "kind": "session" }));
                let functions = lunco_api_core::api_value_from_serializable(&binding.functions)?;
                let diagnostics =
                    lunco_api_core::api_value_from_serializable(&binding.diagnostics)?;
                Ok(Some(api_value!({
                    "name": name,
                    "backend": tool.backend().to_string(),
                    "source": tool.source().map(str::to_string),
                    "active_twin": world
                        .get_resource::<TwinToolLibraries>()
                        .and_then(TwinToolLibraries::owner)
                        .map(|twin| twin.raw()),
                    "scope": scope,
                    "registry_generation": generation(),
                    "functions": functions,
                    "callable": binding.callable,
                    "diagnostics": diagnostics,
                })))
            }
            None => Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("tool library '{name}' not found"),
            )),
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

    /// Tool source persistence retains the exact authored source bytes.
    #[test]
    fn tool_library_file_save_load_roundtrip() {
        let _registry_guard = registry_test_guard();
        let temp = tempfile::tempdir().expect("tool library test directory");
        let root = temp.path();

        let src = "fn double(x) { x * 2 }";
        let path = save_tool_library_file(&root, "persist_probe", src).unwrap();
        assert!(lunco_storage::read_file_sync(&path).is_ok());
        assert_eq!(path, root.join("tools").join("persist_probe.rhai"));

        let stored = lunco_storage::read_file_sync(&path).expect("saved Rhai source");
        let stored = String::from_utf8(stored).expect("UTF-8 Rhai source");
        assert_eq!(stored, src);

        // The scoped owner installs the source into the global binding registry.
        let twin = lunco_workspace::TwinId::new(1);
        let mut scoped = TwinToolLibraries::default();
        scoped.activate(twin);
        scoped.register(twin, "persist_probe", &stored).unwrap();
        let tool = lunco_tools::get("persist_probe").expect("registered");
        assert_eq!(tool.backend(), "rhai");
        assert_eq!(tool.source(), Some(src));
        assert!(scoped.wind_down_for(twin));
        assert!(lunco_tools::get("persist_probe").is_none());
    }
}
