//! Twin persistence + discovery for declarative mission **timelines**.
//!
//! A timeline is the typed parameter map `RunTimeline` executes
//! (`{ name?, steps: [...] }`). This module gives timelines the same durable,
//! discoverable treatment shared tool libraries get (the sibling
//! `lunco-scripting-rhai-world::tool_libs` registry): named timelines persist
//! as `<twin>/timelines/*.json` files (the file is the source of truth, selected
//! by the active Twin's Rhai loading policy), and the API can enumerate / fetch
//! / run them by name.
//!
//! Unlike tool libraries — which must be reachable from the rhai engine OUTSIDE
//! the ECS (hence a process-global static) — timelines are plain data only ever
//! read through queries / commands, so a Bevy [`Resource`] is the right home: no
//! global state, and it composes with the World like everything else.

#![cfg(feature = "rhai")]

use bevy::asset::{AssetEvent, AssetLoadFailedEvent};
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiQueryError, ApiQueryResult};
use lunco_api_core::ApiErrorCode;
use lunco_api_core::{ApiValue, IntoApiValue, api_value_from_serializable};
use lunco_scripting::ScenarioParameters;
use std::collections::{HashMap, HashSet};

/// The owner of the currently addressable timeline set.
///
/// A session-owned set is only for hosts without a Workspace/Twin (for
/// example, a focused scripting test). As soon as a Twin becomes active, its
/// timeline set replaces the session set. Keeping this scope explicit prevents
/// a missing workspace resource from silently reusing a previous Twin's data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineOwner {
    /// Timelines loaded from or registered against this Twin.
    Twin(lunco_workspace::TwinId),
    /// Explicit headless session scope with no Workspace/Twin.
    Session,
}

/// The error returned when a caller tries to mutate a store owned by another
/// lifecycle scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineOwnerMismatch {
    /// Scope that currently owns the store.
    pub actual: Option<TimelineOwner>,
    /// Scope the caller attempted to mutate.
    pub requested: TimelineOwner,
}

/// In-memory store of named typed mission timelines, mirrored to
/// `<twin>/timelines/*.json` on disk. Populated by the active Twin's loading
/// policy and by
/// `RegisterTimeline`; read by `ListTimelines` / `GetTimeline` / `RunStoredTimeline`.
#[derive(Resource, Default)]
pub struct TimelineStore {
    /// The lifecycle scope whose names are currently addressable.
    owner: Option<TimelineOwner>,
    /// Name to structured timeline parameters.
    timelines: HashMap<String, ScenarioParameters>,
}

impl TimelineStore {
    /// The scope currently owning the store, if one has been activated.
    pub fn owner(&self) -> Option<TimelineOwner> {
        self.owner
    }

    /// Switch the store to `owner`, discarding every timeline from the prior
    /// scope. Calling this for the same owner also clears the set: it is the
    /// reload boundary used after a Twin is opened or promoted to active.
    pub fn replace_scope(
        &mut self,
        owner: TimelineOwner,
        timelines: impl IntoIterator<Item = (String, ScenarioParameters)>,
    ) {
        self.owner = Some(owner);
        self.timelines.clear();
        self.timelines.extend(timelines);
    }

    /// Ensure that subsequent writes target exactly `owner`. A changed owner
    /// starts with an empty set; the current owner's entries remain intact.
    pub fn ensure_scope(&mut self, owner: TimelineOwner) {
        if self.owner != Some(owner) {
            self.owner = Some(owner);
            self.timelines.clear();
        }
    }

    /// Register / hot-replace a named timeline in its explicit lifecycle scope.
    pub fn insert_for(
        &mut self,
        owner: TimelineOwner,
        name: impl Into<String>,
        timeline: ScenarioParameters,
    ) -> Result<(), TimelineOwnerMismatch> {
        if self.owner != Some(owner) {
            return Err(TimelineOwnerMismatch {
                actual: self.owner,
                requested: owner,
            });
        }
        self.timelines.insert(name.into(), timeline);
        Ok(())
    }

    /// Remove all entries only when this scope still owns the store. This is
    /// idempotent and deliberately cannot clear a replacement Twin's data.
    pub fn clear_for(&mut self, owner: TimelineOwner) -> bool {
        if self.owner != Some(owner) {
            return false;
        }
        self.owner = None;
        self.timelines.clear();
        true
    }

    /// The stored structured timeline for `name`, if any.
    pub fn get(&self, name: &str) -> Option<&ScenarioParameters> {
        self.timelines.get(name)
    }

    /// Sorted names of every stored timeline.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.timelines.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Sub-directory under a Twin root that holds saved mission timelines.
pub const TIMELINES_DIR: &str = "timelines";

/// Replace the active Twin's addressable timeline scope before loading files.
#[lunco_core::Command(default)]
pub struct ActivateTwinTimelineScope {
    /// Workspace identity of the active Twin.
    pub twin_id: u64,
}

/// Load one indexed timeline file selected by the Twin loading policy.
#[lunco_core::Command(default)]
pub struct LoadTwinTimelineFile {
    /// Workspace identity of the active Twin.
    pub twin_id: u64,
    /// Exact `twin://` authority returned by the asset owner.
    pub name: String,
    /// Timeline name exposed through `ListTimelines`.
    pub timeline_name: String,
    /// Indexed JSON file relative to the Twin root.
    pub relative_path: String,
}

struct PendingTwinTimeline {
    handle: Handle<lunco_assets_runtime::TextAsset>,
    twin: lunco_workspace::TwinId,
    timeline_name: String,
    relative_path: String,
}

/// Async timeline text assets requested by the authored Twin policy.
#[derive(Resource, Default)]
pub struct PendingTwinTimelines {
    items: Vec<PendingTwinTimeline>,
    ready: HashSet<bevy::asset::AssetId<lunco_assets_runtime::TextAsset>>,
    failed: HashMap<bevy::asset::AssetId<lunco_assets_runtime::TextAsset>, String>,
}

impl PendingTwinTimelines {
    fn mark_ready(&mut self, id: bevy::asset::AssetId<lunco_assets_runtime::TextAsset>) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.ready.insert(id);
        }
    }

    fn mark_failed(
        &mut self,
        id: bevy::asset::AssetId<lunco_assets_runtime::TextAsset>,
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

#[lunco_core::on_command(ActivateTwinTimelineScope)]
fn on_activate_twin_timeline_scope(
    trigger: bevy::ecs::observer::On<ActivateTwinTimelineScope>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut store: ResMut<TimelineStore>,
    mut pending: ResMut<PendingTwinTimelines>,
) -> Result<lunco_command_contracts::Ack, String> {
    let twin_id = lunco_workspace::TwinId::new(trigger.event().twin_id);
    let is_active = workspace
        .as_deref()
        .is_some_and(|workspace| workspace.active_twin == Some(twin_id));
    if !is_active {
        return Err(format!("Twin {} is not active", trigger.event().twin_id));
    }
    pending.release_twin(twin_id);
    store.replace_scope(TimelineOwner::Twin(twin_id), std::iter::empty());
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

#[lunco_core::on_command(LoadTwinTimelineFile)]
fn on_load_twin_timeline_file(
    trigger: bevy::ecs::observer::On<LoadTwinTimelineFile>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<lunco_assets_runtime::TextAsset>>>,
    mut pending: ResMut<PendingTwinTimelines>,
) -> Result<lunco_command_contracts::Ack, String> {
    let request = trigger.event();
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let twin = workspace
        .as_deref()
        .and_then(|workspace| workspace.twin(twin_id))
        .ok_or_else(|| format!("workspace Twin {} is unavailable", request.twin_id))?;
    if workspace
        .as_deref()
        .is_none_or(|workspace| workspace.active_twin != Some(twin_id))
    {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    let relative = std::path::Path::new(&request.relative_path);
    if !lunco_assets_path::is_safe_relative_path(&request.relative_path)
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        || relative.parent() != Some(std::path::Path::new(TIMELINES_DIR))
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
    {
        return Err(format!(
            "Twin timeline path `{}` must be a safe `timelines/<name>.json` file",
            request.relative_path
        ));
    }
    if !twin
        .files()
        .iter()
        .any(|entry| entry.relative_path.as_path() == relative)
    {
        return Err(format!(
            "Twin timeline path `{}` is not indexed",
            request.relative_path
        ));
    }
    lunco_scripting_rhai_core::names::validate_file_stem(&request.timeline_name)
        .map_err(|error| format!("invalid Twin timeline name: {error}"))?;
    if relative.file_stem().and_then(|stem| stem.to_str()) != Some(request.timeline_name.as_str()) {
        return Err(format!(
            "Twin timeline name `{}` must match `{}`",
            request.timeline_name, request.relative_path
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
    let asset_server = asset_server.ok_or_else(|| "AssetServer is not installed".to_owned())?;
    if pending
        .items
        .iter()
        .any(|item| item.twin == twin_id && item.relative_path == request.relative_path)
    {
        return Ok(lunco_command_contracts::Ack::new(
            lunco_command_contracts::OpId::new(),
        ));
    }
    let handle = asset_server.load::<lunco_assets_runtime::TextAsset>(lunco_assets_core::twin_uri(
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
    pending.items.push(PendingTwinTimeline {
        handle,
        twin: twin_id,
        timeline_name: request.timeline_name.clone(),
        relative_path: request.relative_path.clone(),
    });
    if failed {
        pending.mark_failed(id, "the source asset had already failed to load".into());
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

fn mark_pending_twin_timelines(
    mut pending: ResMut<PendingTwinTimelines>,
    mut events: MessageReader<AssetEvent<lunco_assets_runtime::TextAsset>>,
    mut failures: MessageReader<AssetLoadFailedEvent<lunco_assets_runtime::TextAsset>>,
) {
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => pending.mark_ready(*id),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => pending.mark_failed(
                *id,
                "Twin timeline text asset was removed before reading".to_owned(),
            ),
        }
    }
    for failure in failures.read() {
        pending.mark_failed(failure.id, failure.error.to_string());
    }
}

fn drain_pending_twin_timelines(
    mut pending: ResMut<PendingTwinTimelines>,
    mut store: ResMut<TimelineStore>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    assets: Option<Res<Assets<lunco_assets_runtime::TextAsset>>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let ready = std::mem::take(&mut pending.ready);
    let failed = std::mem::take(&mut pending.failed);
    let items = std::mem::take(&mut pending.items);
    let mut still_pending = Vec::new();
    for item in items {
        let id = item.handle.id();
        if let Some(error) = failed.get(&id) {
            warn!(
                "[timelines] failed to load `{}`: {error}",
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
                "[timelines] `{}` became ready without text",
                item.relative_path
            );
            continue;
        };
        if !workspace
            .as_deref()
            .is_some_and(|workspace| workspace.active_twin == Some(item.twin))
        {
            continue;
        }
        let timeline: ScenarioParameters = match serde_json::from_str(&source.text) {
            Ok(timeline) => timeline,
            Err(error) => {
                warn!("[timelines] invalid `{}`: {error}", item.relative_path);
                continue;
            }
        };
        if let Err(error) = crate::commands::timeline_step_count(&timeline) {
            warn!("[timelines] invalid `{}`: {error}", item.relative_path);
            continue;
        }
        if let Err(error) =
            store.insert_for(TimelineOwner::Twin(item.twin), item.timeline_name, timeline)
        {
            warn!(
                "[timelines] cannot install `{}`: {error:?}",
                item.relative_path
            );
        }
    }
    pending.items = still_pending;
}

fn release_twin_timelines(
    trigger: On<lunco_workspace::TwinClosed>,
    mut pending: ResMut<PendingTwinTimelines>,
    mut store: ResMut<TimelineStore>,
) {
    let closed = trigger.event().twin;
    pending.release_twin(closed);
    store.clear_for(TimelineOwner::Twin(closed));
}

lunco_core::register_commands!(on_activate_twin_timeline_scope, on_load_twin_timeline_file);

/// Register the typed Twin timeline loader and its async text-asset lifecycle.
pub fn register_twin_timeline_loading(app: &mut App) {
    app.init_resource::<PendingTwinTimelines>()
        .add_observer(release_twin_timelines)
        .add_systems(
            Update,
            (mark_pending_twin_timelines, drain_pending_twin_timelines).chain(),
        );
    register_all_commands(app);
}

/// Persist a typed timeline to `<root>/timelines/<name>.json` (creating the dir
/// if needed). JSON is only the durable file representation; runtime consumers
/// use `ScenarioParameters`. Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_timeline_file(
    root: &std::path::Path,
    name: &str,
    timeline: &ScenarioParameters,
) -> lunco_storage::StorageResult<std::path::PathBuf> {
    lunco_scripting_rhai_core::names::validate_file_stem(name)
        .map_err(lunco_storage::StorageError::Unsupported)?;
    let dir = root.join(TIMELINES_DIR);
    let path = dir.join(format!("{name}.json"));
    let source = serde_json::to_vec_pretty(timeline).map_err(|error| {
        lunco_storage::StorageError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    lunco_storage::write_file_sync(&path, &source)?;
    Ok(path)
}

/// Return the active lifecycle scope. A present Workspace with no active Twin
/// is not a headless session; it is an empty workspace and must reject
/// Twin-owned writes rather than silently keeping old data alive.
pub fn active_owner(
    ws: Option<&lunco_workspace::WorkspaceResource>,
) -> Result<TimelineOwner, String> {
    match ws {
        Some(ws) => ws
            .active_twin
            .map(TimelineOwner::Twin)
            .ok_or_else(|| "no active Twin".to_string()),
        None => Ok(TimelineOwner::Session),
    }
}

// ── API discovery surface ────────────────────────────────────────────────────

/// `ListTimelines` → `{ count, timelines: [name, ...] }`.
struct ListTimelinesProvider;
impl ApiQueryProvider for ListTimelinesProvider {
    fn name(&self) -> &'static str {
        "ListTimelines"
    }

    fn execute(
        &self,
        world: &World,
        _params: &lunco_api_core::ApiValue,
    ) -> lunco_api::ApiQueryResult {
        let Some(store) = world.get_resource::<TimelineStore>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ListTimelines requires TimelineStore",
            ));
        };
        let names = store.names();
        Ok(Some(lunco_api_core::ApiValue::map([
            ("count", (names.len() as i64).into_api_value()),
            ("timelines", names.into_api_value()),
        ])))
    }
}

/// `GetTimeline { name }` → `{ name, timeline }` (the structured data), or not-found.
struct GetTimelineProvider;
impl ApiQueryProvider for GetTimelineProvider {
    fn name(&self) -> &'static str {
        "GetTimeline"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(name) = params
            .get("name")
            .and_then(lunco_api_core::ApiValue::as_str)
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "GetTimeline: `name` required",
            ));
        };
        let Some(store) = world.get_resource::<TimelineStore>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "GetTimeline requires TimelineStore",
            ));
        };
        match store.get(name) {
            Some(timeline) => {
                let timeline = api_value_from_serializable(timeline).map_err(|error| {
                    ApiQueryError::new(
                        ApiErrorCode::InternalError,
                        format!("GetTimeline: could not expose typed timeline: {error}"),
                    )
                })?;
                Ok(Some(ApiValue::map([
                    ("name", name.into_api_value()),
                    ("timeline", timeline),
                ])))
            }
            None => Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("timeline '{name}' not found"),
            )),
        }
    }
}

/// Register the timeline discovery providers into the API query registry.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(ListTimelinesProvider);
    reg.register(GetTimelineProvider);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn timeline() -> ScenarioParameters {
        serde_json::from_str(r#"{"steps":[{"wait":1.0}]}"#).expect("typed timeline fixture")
    }

    /// Timeline persistence keeps the authored typed JSON representation.
    #[test]
    fn timeline_file_save_load_roundtrip() {
        let temp = tempfile::tempdir().expect("timeline test directory");
        let root = temp.path();

        let source = timeline();
        let path = save_timeline_file(root, "approach", &source).unwrap();
        assert!(lunco_storage::read_file_sync(&path).is_ok());
        assert_eq!(path, root.join("timelines").join("approach.json"));

        let stored = lunco_storage::read_file_sync(&path).expect("saved timeline bytes");
        let loaded: ScenarioParameters = serde_json::from_slice(&stored).unwrap();
        assert_eq!(loaded, source);
    }

    #[test]
    fn replacing_scope_discards_previous_twin_timelines() {
        let mut store = TimelineStore::default();
        let first = TimelineOwner::Twin(lunco_workspace::TwinId::new(1));
        let second = TimelineOwner::Twin(lunco_workspace::TwinId::new(2));

        let old = timeline();
        store.replace_scope(first, [("old".to_string(), old.clone())]);
        assert_eq!(store.get("old"), Some(&old));

        let new = timeline();
        store.replace_scope(second, [("new".to_string(), new.clone())]);
        assert_eq!(store.owner(), Some(second));
        assert_eq!(store.get("old"), None);
        assert_eq!(store.get("new"), Some(&new));
    }

    #[test]
    fn stale_twin_cannot_clear_or_write_the_replacement_scope() {
        let mut store = TimelineStore::default();
        let first = TimelineOwner::Twin(lunco_workspace::TwinId::new(1));
        let second = TimelineOwner::Twin(lunco_workspace::TwinId::new(2));
        let current = timeline();
        store.replace_scope(second, [("current".to_string(), current.clone())]);

        assert!(!store.clear_for(first));
        assert!(store.insert_for(first, "stale", timeline()).is_err());
        assert_eq!(store.get("current"), Some(&current));
    }
}
