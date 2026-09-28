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
        // `WorkspaceResource` fires for any mutation, so compare the typed
        // recents value to the last saved snapshot before scheduling a write.
        app.add_systems(
            Update,
            (poll_recents_load, persist_recents_when_changed).chain(),
        )
        .add_systems(Last, save_recents_before_exit);
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

/// Tracks loaded and saved recents so unrelated workspace mutations do not
/// schedule disk writes. At most one recents save runs at a time.
#[derive(Resource, Default)]
struct RecentsLastSnapshot {
    /// Last normalized or successfully saved list.
    last_saved: Option<Recents>,
    /// Startup load and canonicalization run off-thread.
    loading: Option<Task<Recents>>,
    /// Config-file writes run off-thread and are serialized in recents order.
    saving: Option<Task<Result<Recents, String>>>,
}

fn load_recents_at_startup(mut snapshot: ResMut<RecentsLastSnapshot>) {
    snapshot.loading =
        Some(AsyncComputeTaskPool::get().spawn(async move { load_and_normalize_recents() }));
}

fn load_and_normalize_recents() -> Recents {
    let path = recents_path();
    let mut loaded = lunco_workspace::Recents::load(&path);
    if loaded.deduplicate() {
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = loaded.save(&path) {
            warn!("[Recents] cleanup save to {} failed: {e}", path.display());
        }
    }
    loaded
}

fn save_recents_async(recents: Recents) -> Task<Result<Recents, String>> {
    let path = recents_path();
    AsyncComputeTaskPool::get().spawn(async move {
        recents
            .save(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(recents)
    })
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
        let last_saved = loaded.clone();
        let mut workspace = world.resource_mut::<WorkspaceResource>();
        let mut current = std::mem::take(&mut workspace.recents);
        merge_loaded_recents(&mut current, loaded);
        workspace.recents = current;
        drop(workspace);
        let mut snapshot = world.resource_mut::<RecentsLastSnapshot>();
        snapshot.last_saved = Some(last_saved);
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

    let save_completed = snapshot.saving.is_some();
    if save_completed {
        let result = {
            let task = snapshot.saving.as_mut().expect("save was present");
            block_on(future::poll_once(task))
        };
        let Some(result) = result else {
            return;
        };
        snapshot.saving = None;
        match result {
            Ok(saved) => snapshot.last_saved = Some(saved),
            Err(error) => {
                warn!("[Recents] save failed: {error}");
                return;
            }
        }
    } else if !workspace.is_changed() {
        return;
    }

    if snapshot.last_saved.as_ref() == Some(&workspace.recents) {
        return;
    }

    // Wasm has no filesystem-backed recents store. Track the snapshot without
    // scheduling a write.
    #[cfg(target_arch = "wasm32")]
    {
        snapshot.last_saved = Some(workspace.recents.clone());
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        snapshot.saving = Some(save_recents_async(workspace.recents.clone()));
    }
}

/// Finish a pending recents load or save before the process exits.
fn save_recents_before_exit(world: &mut World) {
    let has_exit = world
        .get_resource::<bevy::ecs::message::Messages<AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !has_exit {
        return;
    }

    if let Some(task) = world.resource_mut::<RecentsLastSnapshot>().loading.take() {
        let loaded = block_on(task);
        let last_saved = loaded.clone();
        let mut workspace = world.resource_mut::<WorkspaceResource>();
        let mut current = std::mem::take(&mut workspace.recents);
        merge_loaded_recents(&mut current, loaded);
        workspace.recents = current;
        drop(workspace);
        world.resource_mut::<RecentsLastSnapshot>().last_saved = Some(last_saved);
    }

    if let Some(task) = world.resource_mut::<RecentsLastSnapshot>().saving.take() {
        match block_on(task) {
            Ok(saved) => world.resource_mut::<RecentsLastSnapshot>().last_saved = Some(saved),
            Err(error) => warn!("[Recents] save failed during shutdown: {error}"),
        }
    }

    let recents = world.resource::<WorkspaceResource>().recents.clone();
    if world.resource::<RecentsLastSnapshot>().last_saved.as_ref() == Some(&recents) {
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        world.resource_mut::<RecentsLastSnapshot>().last_saved = Some(recents);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let task = save_recents_async(recents);
        match block_on(task) {
            Ok(saved) => world.resource_mut::<RecentsLastSnapshot>().last_saved = Some(saved),
            Err(error) => warn!("[Recents] final save failed: {error}"),
        }
    }
}
