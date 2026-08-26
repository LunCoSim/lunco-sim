//! Twin persistence + discovery for declarative mission **timelines**.
//!
//! A timeline is the pure-DATA mission format `RunTimeline` executes (a JSON
//! steps array, or `{ name?, steps: [...] }`). This module gives timelines the
//! same durable, discoverable treatment shared tool libraries get
//! ([`crate::tool_libs`]): named timelines persist as `<twin>/timelines/*.json`
//! files (the file IS the source of truth, loaded on active Twin open), and the API can
//! enumerate / fetch / run them by name.
//!
//! Unlike tool libraries — which must be reachable from the rhai engine OUTSIDE
//! the ECS (hence a process-global static) — timelines are plain data only ever
//! read through queries / commands, so a Bevy [`Resource`] is the right home: no
//! global state, and it composes with the World like everything else.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use std::collections::HashMap;

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

/// In-memory store of named mission timelines (the `RunTimeline` JSON format),
/// mirrored to `<twin>/timelines/*.json` on disk. Populated on Twin open and by
/// `RegisterTimeline`; read by `ListTimelines` / `GetTimeline` / `RunStoredTimeline`.
#[derive(Resource, Default)]
pub struct TimelineStore {
    /// The lifecycle scope whose names are currently addressable.
    owner: Option<TimelineOwner>,
    /// name → timeline JSON (a steps array, or a `{ name?, steps: [...] }` object).
    timelines: HashMap<String, String>,
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
        timelines: impl IntoIterator<Item = (String, String)>,
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
        json: impl Into<String>,
    ) -> Result<(), TimelineOwnerMismatch> {
        if self.owner != Some(owner) {
            return Err(TimelineOwnerMismatch {
                actual: self.owner,
                requested: owner,
            });
        }
        self.timelines.insert(name.into(), json.into());
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

    /// The stored JSON for `name`, if any.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.timelines.get(name).map(String::as_str)
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

/// Scan `<root>/timelines/*.json` → `(name, json)` for each (file stem = name).
/// A single unreadable file is logged and skipped, never blocking the rest. A
/// missing dir is the common case (twin has none) → empty, not an error.
/// Native-only.
// `disallowed_methods` bans `std::fs` because it silently fails on wasm. This fn
// is `cfg(not(wasm32))`, so that failure mode is unreachable. Scoped to this fn,
// not the module, so the lint stays live for anything wasm-reachable added later.
#[allow(clippy::disallowed_methods)]
#[cfg(not(target_arch = "wasm32"))]
pub fn load_timelines_from_dir(root: &std::path::Path) -> Vec<(String, String)> {
    let dir = root.join(TIMELINES_DIR);
    let mut loaded = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return loaded,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match lunco_storage::read_file_sync(&path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            Some(json) => loaded.push((name.to_string(), json)),
            None => warn!("[timelines] failed to read {}", path.display()),
        }
    }
    loaded.sort_by(|a, b| a.0.cmp(&b.0));
    loaded
}

/// Persist a timeline's JSON to `<root>/timelines/<name>.json` (creating the dir
/// if needed). The on-disk counterpart of scoped timeline registration, so an
/// interactively-registered timeline survives a restart. Native-only.
// See `load_timelines_from_dir` — native-only, so the wasm foot-gun the
// `disallowed_methods` ban guards against cannot occur here.
#[allow(clippy::disallowed_methods)]
#[cfg(not(target_arch = "wasm32"))]
pub fn save_timeline_file(
    root: &std::path::Path,
    name: &str,
    json: &str,
) -> lunco_storage::StorageResult<std::path::PathBuf> {
    crate::names::validate_file_stem(name).map_err(lunco_storage::StorageError::Unsupported)?;
    let dir = root.join(TIMELINES_DIR);
    let path = dir.join(format!("{name}.json"));
    lunco_storage::write_file_sync(&path, json.as_bytes())?;
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

/// Observer: on Twin open, replace the store with that active Twin's
/// timelines. A non-active Twin is only a workspace entry and must not mutate
/// the active runtime's addressable names.
pub fn sync_timelines_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut store: ResMut<TimelineStore>,
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
    #[cfg(not(target_arch = "wasm32"))]
    let loaded = load_timelines_from_dir(&twin.root);
    #[cfg(target_arch = "wasm32")]
    let loaded = Vec::new();
    let count = loaded.len();
    store.replace_scope(TimelineOwner::Twin(twin_id), loaded);
    if count > 0 {
        info!(
            "[timelines] loaded {count} timeline{} from Twin",
            if count == 1 { "" } else { "s" }
        );
    }
}

/// Observer: close the old active scope and, when Workspace promotion selects
/// another Twin, load that Twin as the new addressable scope. Closing a
/// non-active Twin cannot clear the active store.
pub fn wind_down_timelines_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut store: ResMut<TimelineStore>,
) {
    let closed = TimelineOwner::Twin(trigger.event().twin);
    if !store.clear_for(closed) {
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
    #[cfg(not(target_arch = "wasm32"))]
    let loaded = load_timelines_from_dir(&twin.root);
    #[cfg(target_arch = "wasm32")]
    let loaded = Vec::new();
    store.replace_scope(TimelineOwner::Twin(active), loaded);
}

// ── API discovery surface ────────────────────────────────────────────────────

/// `ListTimelines` → `{ count, timelines: [name, ...] }`.
struct ListTimelinesProvider;
impl ApiQueryProvider for ListTimelinesProvider {
    fn name(&self) -> &'static str {
        "ListTimelines"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let names = world
            .get_resource::<TimelineStore>()
            .map(TimelineStore::names)
            .unwrap_or_default();
        ApiResponse::ok(serde_json::json!({ "count": names.len(), "timelines": names }))
    }
}

/// `GetTimeline { name }` → `{ name, timeline }` (the stored JSON), or not-found.
struct GetTimelineProvider;
impl ApiQueryProvider for GetTimelineProvider {
    fn name(&self) -> &'static str {
        "GetTimeline"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(name) = params.get("name").and_then(serde_json::Value::as_str) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "GetTimeline: `name` required".to_string(),
            );
        };
        match world
            .get_resource::<TimelineStore>()
            .and_then(|s| s.get(name).map(str::to_string))
        {
            Some(json) => ApiResponse::ok(serde_json::json!({ "name": name, "timeline": json })),
            None => ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("timeline '{name}' not found"),
            ),
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

    /// `save_timeline_file` → `load_timelines_from_dir` round-trips by name.
    #[test]
    fn timeline_file_save_load_roundtrip() {
        let root = std::env::temp_dir().join(format!("lunco_tl_tline_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let json = r#"[{"wait":1.0},{"emit":"GO"}]"#;
        let path = save_timeline_file(&root, "approach", json).unwrap();
        assert!(path.exists());
        assert_eq!(path, root.join("timelines").join("approach.json"));

        let loaded = load_timelines_from_dir(&root);
        assert_eq!(loaded, vec![("approach".to_string(), json.to_string())]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A missing `timelines/` dir yields nothing, not an error.
    #[test]
    fn missing_timelines_dir_is_empty_not_error() {
        let root = std::env::temp_dir().join(format!("lunco_tl_none2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(load_timelines_from_dir(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replacing_scope_discards_previous_twin_timelines() {
        let mut store = TimelineStore::default();
        let first = TimelineOwner::Twin(lunco_workspace::TwinId::new(1));
        let second = TimelineOwner::Twin(lunco_workspace::TwinId::new(2));

        store.replace_scope(first, [("old".to_string(), "[]".to_string())]);
        assert_eq!(store.get("old"), Some("[]"));

        store.replace_scope(second, [("new".to_string(), "[]".to_string())]);
        assert_eq!(store.owner(), Some(second));
        assert_eq!(store.get("old"), None);
        assert_eq!(store.get("new"), Some("[]"));
    }

    #[test]
    fn stale_twin_cannot_clear_or_write_the_replacement_scope() {
        let mut store = TimelineStore::default();
        let first = TimelineOwner::Twin(lunco_workspace::TwinId::new(1));
        let second = TimelineOwner::Twin(lunco_workspace::TwinId::new(2));
        store.replace_scope(second, [("current".to_string(), "[]".to_string())]);

        assert!(!store.clear_for(first));
        assert!(store.insert_for(first, "stale", "[]").is_err());
        assert_eq!(store.get("current"), Some("[]"));
    }

    #[test]
    fn twin_events_replace_the_runtime_timeline_scope() {
        let root =
            std::env::temp_dir().join(format!("lunco_timeline_scope_{}_{}", std::process::id(), 1));
        let replacement = root.with_extension("replacement");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&replacement);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        save_timeline_file(&root, "old", "[]").unwrap();
        save_timeline_file(&replacement, "new", "[]").unwrap();

        let first_twin = match lunco_workspace::TwinMode::open(&root).unwrap() {
            lunco_workspace::TwinMode::Folder(twin) | lunco_workspace::TwinMode::Twin(twin) => twin,
            lunco_workspace::TwinMode::Orphan(_) => panic!("test root must be a folder"),
        };
        let second_twin = match lunco_workspace::TwinMode::open(&replacement).unwrap() {
            lunco_workspace::TwinMode::Folder(twin) | lunco_workspace::TwinMode::Twin(twin) => twin,
            lunco_workspace::TwinMode::Orphan(_) => panic!("test root must be a folder"),
        };

        let mut app = App::new();
        app.insert_resource(lunco_workspace::WorkspaceResource::new());
        app.init_resource::<TimelineStore>();
        app.add_observer(sync_timelines_on_twin_added);
        app.add_observer(wind_down_timelines_on_twin_closed);
        let first_id = app
            .world_mut()
            .resource_mut::<lunco_workspace::WorkspaceResource>()
            .add_twin(first_twin);
        app.world_mut()
            .trigger(lunco_workspace::TwinAdded { twin: first_id });
        assert_eq!(
            app.world().resource::<TimelineStore>().get("old"),
            Some("[]")
        );

        let mut workspace = app
            .world_mut()
            .resource_mut::<lunco_workspace::WorkspaceResource>();
        workspace.close_twin(first_id);
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: first_id,
            root: root.clone(),
            was_active: true,
        });
        let second_id = app
            .world_mut()
            .resource_mut::<lunco_workspace::WorkspaceResource>()
            .add_twin(second_twin);
        app.world_mut()
            .trigger(lunco_workspace::TwinAdded { twin: second_id });

        let store = app.world().resource::<TimelineStore>();
        assert_eq!(store.owner(), Some(TimelineOwner::Twin(second_id)));
        assert_eq!(store.get("old"), None);
        assert_eq!(store.get("new"), Some("[]"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&replacement);
    }
}
