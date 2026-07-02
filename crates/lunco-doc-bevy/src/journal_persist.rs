//! **Durable journal persistence — headless-safe.**
//!
//! Persists the canonical [`JournalResource`](crate::JournalResource) to disk
//! (debounced) and reloads it at startup, so authored edit history survives
//! process restarts on a **headless server** (`lunco-sandbox-server`) or any
//! headless peer — none of which load `lunco-workspace`, whose Twin-scoped,
//! save-triggered persistence therefore never runs there. In-memory-only before
//! this: a restart lost all collaborative history.
//!
//! Lives in the journal's own substrate crate (not networking, not the GUI
//! workspace) so it's available anywhere a [`JournalResource`] exists, and is
//! role-agnostic — the caller decides where to add it (the GUI keeps its
//! workspace persistence; the headless core adds this). Reuses the journal's own
//! serialization ([`Journal::to_bytes`](lunco_twin_journal::Journal::to_bytes))
//! and the atomic [`lunco_storage::write_file_sync`] — no new I/O logic.
//!
//! Keyed by the journal's stable [`TwinId`](lunco_twin_journal::TwinId), so a
//! server hosting distinct twins/scenarios keeps distinct history files. Full
//! rewrite per debounced save (fine until histories get large; incremental
//! append is a later refinement).

use bevy::prelude::*;
use lunco_storage::{Storage, StorageHandle};
use lunco_twin_journal::Journal;

use crate::JournalResource;

/// Debounced save cadence: the journal is rewritten at most this often, and only
/// when it actually grew since the last save.
const SAVE_INTERVAL_SECS: f32 = 5.0;

/// Adds durable load-on-startup + debounced-save for the [`JournalResource`].
/// Add it where headless durability is wanted (e.g. the sandbox headless core);
/// the GUI keeps its own workspace-scoped persistence.
pub struct JournalPersistencePlugin;

impl Plugin for JournalPersistencePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            load_journal_once.run_if(resource_added::<JournalResource>),
        );
        app.add_systems(Update, persist_journal);
    }
}

/// On-disk path for the journal identified by `twin_id`, under the durable user
/// config dir (not the wipeable cache). The twin id can be path-like, so it's
/// sanitized into a single safe filename component.
fn journal_path(twin_id: &str) -> std::path::PathBuf {
    let safe: String = twin_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let safe = if safe.is_empty() { "default".to_string() } else { safe };
    lunco_assets::user_config_subdir("journal").join(format!("{safe}.json"))
}

fn read_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    let handle = StorageHandle::File(path.to_path_buf());
    #[cfg(not(target_arch = "wasm32"))]
    let result = lunco_storage::FileStorage::new().read_sync(&handle);
    #[cfg(target_arch = "wasm32")]
    let result = lunco_storage::WebStorage::new().read_sync(&handle);
    result.ok()
}

/// Load the persisted journal into the live resource once, the frame it appears.
/// Swaps in place to preserve the shared `Arc` (recorders on document hosts keep
/// writing to the loaded journal), and preserves the current local author (the
/// networking layer stamps a stable install id) so new edits keep *this* peer's
/// identity even if the saved file carried a different one. No-op when nothing is
/// persisted yet.
fn load_journal_once(journal: Option<Res<JournalResource>>) {
    let Some(journal) = journal else {
        return;
    };
    let twin_id = journal.with_read(|j| j.twin().0.clone());
    let path = journal_path(&twin_id);
    let Some(bytes) = read_bytes(&path) else {
        return; // nothing persisted → start fresh
    };
    match Journal::from_bytes(&bytes) {
        Ok(loaded) => {
            let n = loaded.len();
            journal.with_write(|j| {
                let me = j.local_author().clone();
                *j = loaded;
                j.set_local_author(me);
            });
            info!("[journal-persist] loaded {n} entries from {}", path.display());
        }
        Err(e) => {
            warn!("[journal-persist] parse of {} failed — starting fresh: {e}", path.display());
        }
    }
}

/// Debounced periodic save. Rewrites the whole file at most every
/// [`SAVE_INTERVAL_SECS`], and only when the journal grew. Atomic write (tmp +
/// fsync + rename, creates parent dirs) via [`lunco_storage::write_file_sync`].
fn persist_journal(
    journal: Option<Res<JournalResource>>,
    time: Res<Time>,
    mut acc: Local<f32>,
    mut last_saved_len: Local<usize>,
) {
    let Some(journal) = journal else {
        return;
    };
    *acc += time.delta_secs();
    if *acc < SAVE_INTERVAL_SECS {
        return;
    }
    *acc = 0.0;
    let (twin_id, len, bytes) = journal.with_read(|j| (j.twin().0.clone(), j.len(), j.to_bytes()));
    if len == *last_saved_len {
        return; // nothing new since the last save
    }
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            warn!("[journal-persist] serialize failed: {e}");
            return;
        }
    };
    let path = journal_path(&twin_id);
    match lunco_storage::write_file_sync(&path, &bytes) {
        Ok(()) => {
            *last_saved_len = len;
            debug!("[journal-persist] saved {len} entries to {}", path.display());
        }
        Err(e) => warn!("[journal-persist] save to {} failed: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_path_sanitizes_pathlike_twin_ids() {
        // A path-like twin id collapses to one safe filename component.
        let p = journal_path("/home/user/proj");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "_home_user_proj.json");
        // A simple id is preserved; an empty id falls back to "default".
        assert_eq!(journal_path("local-twin").file_name().unwrap().to_string_lossy(), "local-twin.json");
        assert_eq!(journal_path("").file_name().unwrap().to_string_lossy(), "default.json");
    }
}
