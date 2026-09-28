//! Recents persistence for the editor session.
//!
//! The session **binding** itself — [`WorkspaceResource`](lunco_workspace::WorkspaceResource),
//! the add/close events, and [`WorkspacePlugin`](lunco_workspace::WorkspacePlugin)
//! — now lives in `lunco-workspace` (bevy ECS substrate, no UI), so a `--no-ui`
//! server installs it without the workbench. What stays here is the part that
//! needs on-disk config-dir resolution (via `lunco_settings`): loading the
//! recents list at startup and writing it back when it changes. The workbench
//! owns config-dir I/O, so this is its job, not the headless workspace crate's.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use lunco_workspace::{Recents, WorkspaceResource};

/// Plugin: load the recents list at startup and persist it on change.
/// Added by [`WorkbenchPlugin`](crate::WorkbenchPlugin) alongside
/// `lunco_workspace::WorkspacePlugin` (which installs the resource itself).
pub(crate) struct RecentsPlugin;

impl Plugin for RecentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecentsLastSnapshot>()
            // Load on startup so the first frame's File menu already
            // requests the recents from previous sessions without waiting
            // for config-file I/O or path canonicalization.
            .add_systems(Startup, load_recents_at_startup);
        // Save reactively when recents change. `is_changed()` on
        // `WorkspaceResource` fires for *any* mutation, so we gate by
        // serialising the recents and comparing to a last-saved snapshot —
        // only writes the JSON when the recents themselves actually changed.
        app.add_systems(
            Update,
            (poll_recents_load, persist_recents_when_changed).chain(),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Recents persistence — the shared LunCoSim config directory
// ─────────────────────────────────────────────────────────────────────────────

/// Path resolution for the recents file. Lifted into a helper so the
/// settings crate's resolved config directory, including its `LUNCOSIM_CONFIG`
/// override, flows through to both the load and save paths.
fn recents_path() -> std::path::PathBuf {
    lunco_settings::user_config_dir().join("recents.json")
}

/// Holds the JSON-serialised recents from the last successful save (or
/// initial load). `persist_recents_when_changed` compares the current
/// state to this and only writes the file when they differ — so the
/// disk-write doesn't fire on every unrelated `WorkspaceResource`
/// mutation (open doc, switch active twin, etc.).
#[derive(Resource, Default)]
struct RecentsLastSnapshot {
    /// Pretty-printed JSON of the last-saved [`lunco_workspace::Recents`].
    /// Empty string means "never saved yet" — load-failure also leaves
    /// it empty so the first real change writes a fresh file.
    json: String,
    /// Startup load and canonicalization run off-thread. Saves wait until it
    /// completes so an auto-opened Twin cannot overwrite the previous list.
    loading: Option<Task<LoadedRecents>>,
}

struct LoadedRecents {
    recents: Recents,
    json: String,
}

fn load_recents_at_startup(mut snapshot: ResMut<RecentsLastSnapshot>) {
    snapshot.loading =
        Some(AsyncComputeTaskPool::get().spawn(async move { load_and_normalize_recents() }));
}

fn load_and_normalize_recents() -> LoadedRecents {
    let path = recents_path();
    let mut loaded = lunco_workspace::Recents::load(&path);
    if loaded.deduplicate() {
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = loaded.save(&path) {
            warn!("[Recents] cleanup save to {} failed: {e}", path.display());
        }
    }
    let json = serde_json::to_string_pretty(&loaded).unwrap_or_default();
    LoadedRecents {
        recents: loaded,
        json,
    }
}

fn poll_recents_load(mut commands: Commands, mut snapshot: ResMut<RecentsLastSnapshot>) {
    let Some(task) = snapshot.loading.as_mut() else {
        return;
    };
    let Some(loaded) = block_on(future::poll_once(task)) else {
        return;
    };

    commands.queue(move |world: &mut World| {
        // A Twin or loose file may open while the stored list is being read.
        // Keep those newer entries first, then append the normalized disk list.
        let mut workspace = world.resource_mut::<WorkspaceResource>();
        let mut current = std::mem::take(&mut workspace.recents);
        merge_loaded_recents(&mut current, loaded.recents);
        workspace.recents = current;
        drop(workspace);
        let mut snapshot = world.resource_mut::<RecentsLastSnapshot>();
        snapshot.json = loaded.json;
        snapshot.loading = None;
    });
}

fn merge_loaded_recents(current: &mut Recents, loaded: Recents) {
    merge_recent_paths(
        &mut current.twin_paths,
        loaded.twin_paths,
        lunco_workspace::recents::MAX_RECENT_TWINS,
    );
    merge_recent_paths(
        &mut current.loose_paths,
        loaded.loose_paths,
        lunco_workspace::recents::MAX_RECENT_FILES,
    );
}

fn merge_recent_paths(
    current: &mut Vec<std::path::PathBuf>,
    loaded: Vec<std::path::PathBuf>,
    limit: usize,
) {
    current.truncate(limit);
    for path in loaded {
        if current.len() == limit {
            break;
        }
        if !current.contains(&path) {
            current.push(path);
        }
    }
}

fn persist_recents_when_changed(
    workspace: Res<WorkspaceResource>,
    mut snapshot: ResMut<RecentsLastSnapshot>,
) {
    if snapshot.loading.is_some() {
        return;
    }
    if !workspace.is_changed() {
        return;
    }
    let current = match serde_json::to_string_pretty(&workspace.recents) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Recents] serialise failed: {e}");
            return;
        }
    };
    if current == snapshot.json {
        return;
    }
    // Wasm has no real filesystem — `Recents::save` fails every tick and
    // floods the console. Track the snapshot so we don't keep retrying,
    // but skip the actual write.
    #[cfg(target_arch = "wasm32")]
    {
        snapshot.json = current;
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = recents_path();
        if let Err(e) = workspace.recents.save(&path) {
            warn!("[Recents] save to {} failed: {e}", path.display());
            return;
        }
        snapshot.json = current;
    }
}
