//! Recents persistence for the editor session.
//!
//! The session **binding** itself — [`WorkspaceResource`](lunco_workspace::WorkspaceResource),
//! the add/close events, and [`WorkspacePlugin`](lunco_workspace::WorkspacePlugin)
//! — now lives in `lunco-workspace` (bevy ECS substrate, no UI), so a `--no-ui`
//! server installs it without the workbench. What stays here is the part that
//! needs on-disk config-dir resolution (via `lunco_assets`): loading the
//! recents list at startup and writing it back when it changes. The workbench
//! owns config-dir I/O, so this is its job, not the headless workspace crate's.

use bevy::prelude::*;

use lunco_workspace::WorkspaceResource;

/// Plugin: load the recents list at startup and persist it on change.
/// Added by [`WorkbenchPlugin`](crate::WorkbenchPlugin) alongside
/// `lunco_workspace::WorkspacePlugin` (which installs the resource itself).
pub(crate) struct RecentsPlugin;

impl Plugin for RecentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecentsLastSnapshot>()
            // Load on startup so the first frame's File menu already
            // shows the recents from previous sessions.
            .add_systems(Startup, load_recents_at_startup)
            // Save reactively when recents change. `is_changed()` on
            // `WorkspaceResource` fires for *any* mutation, so we
            // gate by serialising the recents and comparing to a
            // last-saved snapshot — only writes the JSON when the
            // recents themselves actually changed.
            .add_systems(Update, persist_recents_when_changed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Recents persistence — `~/.lunco/recents.json`
// ─────────────────────────────────────────────────────────────────────────────

/// Path resolution for the recents file. Lifted into a helper so the
/// `LUNCOSIM_CONFIG` env override (set by `lunco_assets::user_config_dir`)
/// flows through to both the load and save paths.
fn recents_path() -> std::path::PathBuf {
    lunco_assets::user_config_dir().join("recents.json")
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
}

fn load_recents_at_startup(
    mut workspace: ResMut<WorkspaceResource>,
    mut snapshot: ResMut<RecentsLastSnapshot>,
) {
    let path = recents_path();
    let loaded = lunco_workspace::Recents::load(&path);
    snapshot.json = serde_json::to_string_pretty(&loaded).unwrap_or_default();
    workspace.recents = loaded;
}

fn persist_recents_when_changed(
    workspace: Res<WorkspaceResource>,
    mut snapshot: ResMut<RecentsLastSnapshot>,
) {
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
