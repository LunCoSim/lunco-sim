//! Twin edit-journal persistence (B1).
//!
//! Saves the canonical [`Journal`](lunco_twin_journal::Journal) to
//! `<twin-root>/.lunco/journal/journal.json` and reloads it when a Twin
//! opens, so edit history (and, later, versions / branches) survives across
//! sessions. UI-free and headless: the disk I/O goes through [`lunco_storage`],
//! the same byte-level layer the rest of the app uses — no business logic in
//! the storage crate.
//!
//! - **Load** on [`TwinAdded`](crate::session::TwinAdded): read the file and
//!   swap it into the live [`JournalResource`] *in place*, preserving the
//!   shared `Arc` so the op-recorders installed on document hosts (A3) keep
//!   writing to the loaded journal.
//! - **Save** on [`DocumentSaved`](lunco_doc_bevy::DocumentSaved): serialize
//!   the journal and write it to the active Twin's folder.
//!
//! `.lunco/` is excluded from the Twin file index (it's session state), so the
//! journal file never appears as a document.
//!
//! Both observers no-op when no [`JournalResource`] is present, so they're safe
//! to register unconditionally (headless `--no-ui` servers without journaling
//! just skip them).

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_doc_bevy::{DocumentSaved, JournalResource};
use lunco_storage::{Storage, StorageHandle};
use lunco_twin_journal::Journal as CanonicalJournal;

use crate::session::TwinAdded;
use crate::{TwinId, WorkspaceResource};

/// Location of the journal file within a Twin folder.
const JOURNAL_REL_PATH: &str = ".lunco/journal/journal.json";

/// Absolute path to a Twin's journal file.
fn journal_path(twin_root: &Path) -> PathBuf {
    twin_root.join(JOURNAL_REL_PATH)
}

/// On-disk folder of `twin`, if it's a known Twin in the workspace.
fn twin_root(workspace: &WorkspaceResource, twin: TwinId) -> Option<PathBuf> {
    workspace.twin(twin).map(|t| t.root.clone())
}

/// Read the persisted journal bytes for `twin_root`, or `None` when there is
/// no journal yet (or it couldn't be read). Tolerant by design: a missing /
/// unreadable file means "start fresh", never an error surfaced to the user.
fn read_journal_bytes(twin_root: &Path) -> Option<Vec<u8>> {
    let handle = StorageHandle::File(journal_path(twin_root));
    #[cfg(not(target_arch = "wasm32"))]
    let result = lunco_storage::FileStorage::new().read_sync(&handle);
    #[cfg(target_arch = "wasm32")]
    let result = lunco_storage::WebStorage::new().read_sync(&handle);
    result.ok()
}

/// Write `bytes` to `twin_root`'s journal file. Native: write a `.tmp` sibling
/// then atomically `rename` over the target (the established lunco pattern, see
/// `recents.rs`). Wasm: a `localStorage` set is already atomic, so write
/// directly.
fn write_journal_bytes(twin_root: &Path, bytes: &[u8]) -> lunco_storage::StorageResult<()> {
    let path = journal_path(twin_root);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        lunco_storage::FileStorage::new().write_sync(&StorageHandle::File(tmp.clone()), bytes)?;
        std::fs::rename(&tmp, &path).map_err(lunco_storage::StorageError::Io)?;
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        lunco_storage::WebStorage::new().write_sync(&StorageHandle::File(path), bytes)
    }
}

/// Load the persisted journal for a newly-opened Twin into the live
/// [`JournalResource`], replacing the in-memory journal in place.
pub(crate) fn on_twin_added_load_journal(
    trigger: On<TwinAdded>,
    workspace: Res<WorkspaceResource>,
    journal: Option<Res<JournalResource>>,
) {
    let Some(journal) = journal else { return };
    let Some(root) = twin_root(&workspace, trigger.event().twin) else { return };
    let Some(bytes) = read_journal_bytes(&root) else { return };
    match CanonicalJournal::from_bytes(&bytes) {
        Ok(loaded) => {
            let n = loaded.len();
            // Swap in place to keep the shared `Arc`: recorders on document
            // hosts hold clones of this resource and must see the loaded journal.
            journal.with_write(|j| *j = loaded);
            info!(
                "[journal] loaded {n} entr{} from {}",
                if n == 1 { "y" } else { "ies" },
                journal_path(&root).display(),
            );
        }
        Err(err) => warn!(
            "[journal] could not parse {} — starting fresh: {err}",
            journal_path(&root).display(),
        ),
    }
}

/// Persist the journal to the active Twin's folder whenever a document is saved.
pub(crate) fn on_document_saved_persist_journal(
    _trigger: On<DocumentSaved>,
    workspace: Res<WorkspaceResource>,
    journal: Option<Res<JournalResource>>,
) {
    let Some(journal) = journal else { return };
    // No active Twin (a loose / untitled doc) → nowhere twin-scoped to save.
    let Some(twin) = workspace.active_twin else { return };
    let Some(root) = twin_root(&workspace, twin) else { return };
    let bytes = match journal.with_read(|j| j.to_bytes()) {
        Ok(b) => b,
        Err(err) => {
            warn!("[journal] serialize failed: {err}");
            return;
        }
    };
    if let Err(err) = write_journal_bytes(&root, &bytes) {
        warn!(
            "[journal] save to {} failed: {err}",
            journal_path(&root).display(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DocumentId;
    use lunco_twin_journal::{AuthorTag, LifecycleKind};

    #[test]
    fn journal_file_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Build a journal with one entry, persist it.
        let mut j = CanonicalJournal::new(
            lunco_twin_journal::TwinId::new("t"),
            lunco_twin_journal::AuthorId::local(),
        );
        let doc = DocumentId::new(1);
        j.record_lifecycle(AuthorTag::local_user(), doc, LifecycleKind::Saved);
        write_journal_bytes(root, &j.to_bytes().unwrap()).unwrap();

        // The file landed at the documented twin-relative path.
        assert!(journal_path(root).exists());

        // Read it back.
        let bytes = read_journal_bytes(root).expect("journal file present");
        let loaded = CanonicalJournal::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries_for_doc(doc).count(), 1);
    }

    #[test]
    fn missing_journal_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_journal_bytes(dir.path()).is_none());
    }
}
