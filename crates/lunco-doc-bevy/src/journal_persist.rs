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
fn load_journal_once(journal: Option<Res<JournalResource>>) {
    if let Some(journal) = journal {
        load_from_disk(&journal);
    }
}

/// Load the persisted journal into `journal` in place. Swaps to preserve the
/// shared `Arc` (recorders on document hosts keep writing to the loaded journal),
/// and preserves the current local author (the networking layer stamps a stable
/// install id) and the live merge strategy (configuration, not persisted data) so
/// new edits keep *this* peer's identity and a scripted merge policy survives a
/// reload. Returns the number of entries loaded, or `None` when nothing is
/// persisted yet / the file is unreadable. Shared by the startup system and tests.
pub(crate) fn load_from_disk(journal: &JournalResource) -> Option<usize> {
    let twin_id = journal.with_read(|j| j.twin().0.clone());
    let path = journal_path(&twin_id);
    let bytes = read_bytes(&path)?; // nothing persisted → start fresh
    match Journal::from_bytes(&bytes) {
        Ok(loaded) => {
            let n = loaded.len();
            journal.with_write(|j| {
                let me = j.local_author().clone();
                let strategy = j.merge_strategy().clone();
                *j = loaded;
                j.set_local_author(me);
                j.set_merge_strategy(strategy);
            });
            info!("[journal-persist] loaded {n} entries from {}", path.display());
            Some(n)
        }
        Err(e) => {
            warn!("[journal-persist] parse of {} failed — starting fresh: {e}", path.display());
            None
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
    let len = journal.with_read(|j| j.len());
    if len == *last_saved_len {
        return; // nothing new since the last save
    }
    if save_to_disk(&journal).is_some() {
        *last_saved_len = len;
    }
}

/// Serialize `journal` and write it atomically to its on-disk path (tmp + fsync +
/// rename, creating parent dirs) via [`lunco_storage::write_file_sync`]. Returns
/// the number of entries written, or `None` on a serialize / I/O failure. Shared
/// by the debounced save system and tests.
pub(crate) fn save_to_disk(journal: &JournalResource) -> Option<usize> {
    let (twin_id, len, bytes) = journal.with_read(|j| (j.twin().0.clone(), j.len(), j.to_bytes()));
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            warn!("[journal-persist] serialize failed: {e}");
            return None;
        }
    };
    let path = journal_path(&twin_id);
    match lunco_storage::write_file_sync(&path, &bytes) {
        Ok(()) => {
            debug!("[journal-persist] saved {len} entries to {}", path.display());
            Some(len)
        }
        Err(e) => {
            warn!("[journal-persist] save to {} failed: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end persist→reload through the REAL save/load helpers + real file
    /// I/O: author edits, save, then reload into a fresh journal (as a restarted
    /// process would) and confirm the history — and that a reload preserves THIS
    /// peer's live identity + merge strategy rather than clobbering them with the
    /// saved file's. Isolated by a unique twin id; cleans up its file.
    #[test]
    fn save_then_load_roundtrips_on_disk() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorId, AuthorTag, EntryKind, LifecycleKind, MergeStrategy, TwinId};

        let twin = "lunco-test-journal-persist-roundtrip";
        let path = journal_path(twin);
        let _ = std::fs::remove_file(&path); // clear any leftover from a prior run

        // Session 1: author 3 edits, then save.
        let j1 = JournalResource::new(TwinId::new(twin), AuthorId::new("peer-A"));
        j1.with_write(|j| {
            for _ in 0..3 {
                j.append_local(
                    AuthorTag::for_tool("test"),
                    DocumentId::new(1),
                    EntryKind::Lifecycle(LifecycleKind::Saved),
                    None,
                );
            }
        });
        assert_eq!(save_to_disk(&j1), Some(3), "save writes all 3 entries");
        assert!(path.exists(), "journal file exists after save");

        // Session 2 (a restarted process): a fresh, empty journal for the SAME
        // twin, but with a DIFFERENT local author and an activated scripted merge
        // strategy — neither of which the reload may clobber.
        let j2 = JournalResource::new(TwinId::new(twin), AuthorId::new("peer-B"));
        j2.with_write(|j| j.set_merge_strategy(MergeStrategy::Scripted("policy-x".into())));
        assert_eq!(load_from_disk(&j2), Some(3), "reload restores all 3 entries");
        j2.with_read(|j| {
            assert_eq!(j.len(), 3, "history survived the restart");
            assert_eq!(*j.local_author(), AuthorId::new("peer-B"), "this peer's identity preserved");
            assert_eq!(
                *j.merge_strategy(),
                MergeStrategy::Scripted("policy-x".into()),
                "live merge policy preserved across reload",
            );
        });

        let _ = std::fs::remove_file(&path);
    }

    /// Fixture generator (NOT an assertion): writes a real 3-entry journal for the
    /// default `"local-twin"` so a headless-`sandbox-server` boot can be observed
    /// loading it. Run with `--ignored` under an isolated `LUNCOSIM_CONFIG`; does
    /// NOT clean up (the server boot reads it afterward).
    #[test]
    #[ignore = "fixture generator for the headless-server persist boot check"]
    fn seed_local_twin_journal() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorId, AuthorTag, EntryKind, LifecycleKind, TwinId};
        let j = JournalResource::new(TwinId::new("local-twin"), AuthorId::new("seed"));
        j.with_write(|jj| {
            for _ in 0..3 {
                jj.append_local(
                    AuthorTag::for_tool("seed"),
                    DocumentId::new(1),
                    EntryKind::Lifecycle(LifecycleKind::Saved),
                    None,
                );
            }
        });
        let n = save_to_disk(&j).expect("seed save");
        eprintln!("SEEDED {n} entries at {}", journal_path("local-twin").display());
    }

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
