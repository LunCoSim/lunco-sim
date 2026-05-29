//! Per-Twin (per-project) workbench state — VSCode's `workspaceStorage`.
//!
//! VSCode keeps *volatile UI state* (which editors were open, the active
//! one, window layout) in a global store keyed by a hash of the
//! workspace path — **not** inside the project folder, so repos stay
//! clean. We do the same: each Twin gets a
//! `~/.lunco/workspace-state/<hash>.json` keyed off its root path.
//!
//! ## What's stored (and what isn't)
//!
//! - **Active perspective** — restored on Twin activation (workbench
//!   local, side-effect free).
//! - **Open document paths + active document** — persisted so a future
//!   session-restore can reopen them. We do *not* auto-reopen yet:
//!   reopening means replaying domain-specific open commands
//!   (`OpenClass` for Modelica, scene-open for USD, …) with parse /
//!   recompile side effects. That wiring is a deliberate follow-up; the
//!   data is captured now so it's ready when it lands.
//!
//! Global, app-wide preferences (theme, perf HUD, **default window
//! geometry**) stay in `~/.lunco/settings.json` via `lunco-settings` —
//! see [`crate::window_persistence`]. This module owns only the
//! per-project slice.
//!
//! ## Persistence pattern
//!
//! Mirrors recents (`session.rs`): load on Twin activation, save on
//! change via a serialized-snapshot compare (so unrelated
//! `WorkspaceResource` mutations don't write), atomic tmp+rename, and a
//! corrupt / missing file degrades to "open with defaults" — never a
//! panic.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::session::WorkspaceResource;
use crate::WorkbenchLayout;

/// Per-Twin volatile UI state. One of these per project, stored at
/// [`workspace_state_path`].
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct WorkspaceState {
    /// The Twin root this state belongs to. Stored so a hash collision
    /// (two different paths landing on the same file stem) is
    /// detectable — a mismatch is treated as a miss, not silently
    /// applied to the wrong project.
    pub twin_root: PathBuf,
    /// `PerspectiveId` string of the perspective active at save time.
    /// `None` ⇒ leave the app's startup default.
    pub perspective: Option<String>,
    /// Absolute paths of the documents open in this Twin at save time.
    /// Persisted for future session-restore; not auto-reopened yet.
    pub open_documents: Vec<PathBuf>,
    /// Absolute path of the active document, if it was path-backed.
    pub active_document: Option<PathBuf>,
}

impl WorkspaceState {
    /// Load the state for a Twin root. Returns `None` on missing /
    /// unreadable / corrupt file, or when the stored `twin_root` doesn't
    /// match (hash collision guard) — all of which mean "use defaults".
    pub fn load(twin_root: &Path) -> Option<Self> {
        let path = workspace_state_path(twin_root);
        let text = std::fs::read_to_string(&path).ok()?;
        let state: WorkspaceState = serde_json::from_str(&text).ok()?;
        if state.twin_root != twin_root {
            warn!(
                "[WorkspaceState] {} stores a different twin_root ({}); ignoring (hash collision?)",
                path.display(),
                state.twin_root.display(),
            );
            return None;
        }
        Some(state)
    }

    /// Atomically write this state for its `twin_root` (tmp + rename so a
    /// kill mid-write can't corrupt the file).
    pub fn save(&self) -> std::io::Result<()> {
        let path = workspace_state_path(&self.twin_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)
    }
}

/// FNV-1a 64-bit hash. Used to key the per-Twin state file by path.
/// Picked over `DefaultHasher` because the latter's output is *not*
/// guaranteed stable across std versions — a state file written today
/// must still be found after a toolchain bump.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Resolve the on-disk path for a Twin's state file:
/// `<config>/workspace-state/<fnv1a-hex>.json`. Honours the
/// `LUNCOSIM_CONFIG` override via `lunco_assets::user_config_dir`.
///
/// The root is canonicalized first when possible so cwd-relative and
/// absolute spellings of the same folder collapse to one key; falls back
/// to the raw path bytes when canonicalization fails (e.g. the folder
/// was deleted).
pub fn workspace_state_path(twin_root: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(twin_root).unwrap_or_else(|_| twin_root.to_path_buf());
    let key = fnv1a64(canonical.to_string_lossy().as_bytes());
    lunco_assets::user_config_dir()
        .join("workspace-state")
        .join(format!("{key:016x}.json"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Bevy wiring
// ─────────────────────────────────────────────────────────────────────────────

/// Last-saved snapshot, keyed by the active Twin, so the save system
/// only writes the file when this Twin's state actually changed.
#[derive(Resource, Default)]
struct WorkspaceStateLast {
    /// Hex key of the Twin the snapshot belongs to.
    key: Option<String>,
    /// Pretty-printed JSON of the last-saved [`WorkspaceState`].
    json: String,
}

/// Tracks which Twin we last applied state for, so activation-driven
/// restore fires once per switch rather than every frame.
#[derive(Resource, Default)]
struct AppliedTwin(Option<lunco_workspace::TwinId>);

/// Build a [`WorkspaceState`] for the currently-active Twin from live
/// resources. `None` when there's no active Twin (⇒ nothing per-project
/// to persist; global settings cover that case).
fn current_state(
    ws: &WorkspaceResource,
    layout: &WorkbenchLayout,
) -> Option<WorkspaceState> {
    let twin_id = ws.active_twin?;
    let twin_root = ws.twin(twin_id)?.root.clone();

    let open_documents: Vec<PathBuf> = ws
        .documents_in_twin(twin_id)
        .filter_map(|d| d.origin.canonical_path().map(Path::to_path_buf))
        .collect();

    let active_document = ws
        .active_document
        .and_then(|id| ws.document(id))
        .and_then(|d| d.origin.canonical_path().map(Path::to_path_buf));

    Some(WorkspaceState {
        twin_root,
        perspective: layout.active_perspective().map(|p| p.as_str().to_string()),
        open_documents,
        active_document,
    })
}

/// On Twin activation, restore that Twin's saved perspective. Open
/// documents are loaded into the state but not reopened (see module
/// docs). Runs every frame but early-returns unless the active Twin
/// changed — no per-frame work in the steady state (AGENTS.md §7.1).
fn apply_workspace_state_on_twin_change(
    ws: Res<WorkspaceResource>,
    mut layout: ResMut<WorkbenchLayout>,
    mut applied: ResMut<AppliedTwin>,
) {
    let active = ws.active_twin;
    if applied.0 == active {
        return;
    }
    applied.0 = active;
    let Some(twin_id) = active else { return };
    let Some(twin) = ws.twin(twin_id) else { return };
    let Some(state) = WorkspaceState::load(&twin.root) else {
        return;
    };
    if let Some(persp) = &state.perspective {
        // Reconcile: only activates if a perspective with this id is
        // registered in *this* app; unknown ids are dropped.
        layout.activate_perspective_by_str(persp);
    }
}

/// Persist the active Twin's state when it changes. Snapshot-gated like
/// recents so unrelated `WorkspaceResource` / layout mutations don't
/// touch the disk. Native-only (wasm has no filesystem).
fn persist_workspace_state_when_changed(
    ws: Res<WorkspaceResource>,
    layout: Res<WorkbenchLayout>,
    mut last: ResMut<WorkspaceStateLast>,
) {
    let Some(state) = current_state(&ws, &layout) else {
        return;
    };
    let key = format!(
        "{:016x}",
        fnv1a64(
            std::fs::canonicalize(&state.twin_root)
                .unwrap_or_else(|_| state.twin_root.clone())
                .to_string_lossy()
                .as_bytes()
        )
    );
    let current = match serde_json::to_string_pretty(&state) {
        Ok(s) => s,
        Err(e) => {
            warn!("[WorkspaceState] serialise failed: {e}");
            return;
        }
    };
    // Same Twin and identical content ⇒ nothing to do.
    if last.key.as_deref() == Some(key.as_str()) && current == last.json {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Err(e) = state.save() {
            warn!("[WorkspaceState] save failed: {e}");
            return;
        }
    }
    last.key = Some(key);
    last.json = current;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a is stable for a given input — the keying must not drift,
    /// or yesterday's state files become unfindable.
    #[test]
    fn fnv1a64_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// Distinct Twin roots must land on distinct state files.
    #[test]
    fn distinct_roots_distinct_paths() {
        let a = workspace_state_path(Path::new("/tmp/lunco-test-twin-a"));
        let b = workspace_state_path(Path::new("/tmp/lunco-test-twin-b"));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".json"));
    }

    /// End-to-end: save round-trips through disk, and a state file whose
    /// stored `twin_root` doesn't match the lookup root is rejected
    /// (hash-collision guard). One test so the `LUNCOSIM_CONFIG` env
    /// override (read by `user_config_dir`) isn't raced by siblings.
    #[test]
    fn save_load_roundtrip_and_collision_guard() {
        let tmp = std::env::temp_dir().join(format!(
            "lunco-ws-state-test-{}",
            fnv1a64(b"roundtrip-fixture")
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("LUNCOSIM_CONFIG", &tmp);

        let root = tmp.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let state = WorkspaceState {
            twin_root: root.clone(),
            perspective: Some("analyze".into()),
            open_documents: vec![root.join("a.mo"), root.join("b.mo")],
            active_document: Some(root.join("a.mo")),
        };
        state.save().unwrap();

        let loaded = WorkspaceState::load(&root).expect("round-trips");
        assert_eq!(loaded, state);

        // Tamper the stored root → load must reject it.
        let path = workspace_state_path(&root);
        let mut bad = state.clone();
        bad.twin_root = PathBuf::from("/totally/different");
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        assert!(WorkspaceState::load(&root).is_none(), "collision guard");

        std::env::remove_var("LUNCOSIM_CONFIG");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Registers per-Twin workspace-state load/save. Added by
/// [`WorkbenchPlugin`](crate::WorkbenchPlugin) (which owns
/// [`WorkbenchLayout`]). Idempotent.
pub struct WorkspaceStatePlugin;

impl Plugin for WorkspaceStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceStateLast>()
            .init_resource::<AppliedTwin>()
            .add_systems(
                Update,
                (
                    apply_workspace_state_on_twin_change,
                    persist_workspace_state_when_changed,
                )
                    .chain(),
            );
    }
}
