//! Twin persistence + discovery for declarative mission **timelines**.
//!
//! A timeline is the pure-DATA mission format `RunTimeline` executes (a JSON
//! steps array, or `{ name?, steps: [...] }`). This module gives timelines the
//! same durable, discoverable treatment shared tool libraries get
//! ([`crate::tool_libs`]): named timelines persist as `<twin>/timelines/*.json`
//! files (the file IS the source of truth, loaded on Twin open), and the API can
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

/// In-memory store of named mission timelines (the `RunTimeline` JSON format),
/// mirrored to `<twin>/timelines/*.json` on disk. Populated on Twin open and by
/// `RegisterTimeline`; read by `ListTimelines` / `GetTimeline` / `RunStoredTimeline`.
#[derive(Resource, Default)]
pub struct TimelineStore {
    /// name → timeline JSON (a steps array, or a `{ name?, steps: [...] }` object).
    timelines: HashMap<String, String>,
}

impl TimelineStore {
    /// Register / hot-replace a named timeline (in-memory only — file persistence
    /// is the command path's job).
    pub fn insert(&mut self, name: impl Into<String>, json: impl Into<String>) {
        self.timelines.insert(name.into(), json.into());
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
        match std::fs::read_to_string(&path) {
            Ok(json) => loaded.push((name.to_string(), json)),
            Err(e) => warn!("[timelines] failed to read {}: {e}", path.display()),
        }
    }
    loaded.sort_by(|a, b| a.0.cmp(&b.0));
    loaded
}

/// Persist a timeline's JSON to `<root>/timelines/<name>.json` (creating the dir
/// if needed). The on-disk counterpart of [`TimelineStore::insert`], so an
/// interactively-registered timeline survives a restart. Native-only.
// See `load_timelines_from_dir` — native-only, so the wasm foot-gun the
// `disallowed_methods` ban guards against cannot occur here.
#[allow(clippy::disallowed_methods)]
#[cfg(not(target_arch = "wasm32"))]
pub fn save_timeline_file(
    root: &std::path::Path,
    name: &str,
    json: &str,
) -> std::io::Result<std::path::PathBuf> {
    let dir = root.join(TIMELINES_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.json"));
    // Atomic replace: a kill mid-write must never leave a truncated timeline.
    // TODO(backlog): migrate this hand-rolled temp+rename to the Storage handle once
    // the crate gains a lunco-storage dependency (writes go through Storage) — see
    // the engineering-backlog doc in docs/architecture (scripting writes via Storage).
    let tmp = dir.join(format!("{name}.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Observer: on Twin open, load every `timelines/*.json` into the [`TimelineStore`]
/// so file-authored missions are runnable by name without re-registering.
/// Native-only (the file scan).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_timelines_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    ws: Res<lunco_workspace::WorkspaceResource>,
    mut store: ResMut<TimelineStore>,
) {
    let twin_id = trigger.event().twin;
    let Some(twin) = ws.twin(twin_id) else {
        return;
    };
    let loaded = load_timelines_from_dir(&twin.root);
    let count = loaded.len();
    for (name, json) in loaded {
        store.insert(name, json);
    }
    if count > 0 {
        info!(
            "[timelines] loaded {count} timeline{} from Twin",
            if count == 1 { "" } else { "s" }
        );
    }
}

// ── API discovery surface ────────────────────────────────────────────────────

/// `ListTimelines` → `{ count, timelines: [name, ...] }`.
struct ListTimelinesProvider;
impl ApiQueryProvider for ListTimelinesProvider {
    fn name(&self) -> &'static str {
        "ListTimelines"
    }
    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
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
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
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
}
