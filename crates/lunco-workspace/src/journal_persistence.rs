//! Twin edit-journal persistence (B1).
//!
//! Saves the canonical [`Journal`](lunco_twin_journal::Journal) to a visible,
//! project-local `<twin-root>/history/journal.json` and reloads it when a Twin
//! opens, so a twin's **current state** (its scene `.usda`, written on Save)
//! and its **history** (this replayable op log) are persisted side by side.
//! Edit history (and, later, versions / branches) survives across sessions.
//! UI-free and headless: the disk I/O goes through [`lunco_storage`], the same
//! byte-level layer the rest of the app uses — no business logic in the storage
//! crate.
//!
//! - **Load** on [`TwinAdded`](crate::session::TwinAdded): read the file and
//!   swap it into the live [`JournalResource`] *in place*, preserving the
//!   shared `Arc` so the op-recorders installed on document hosts (A3) keep
//!   writing to the loaded journal.
//! - **Save** on [`DocumentSaved`](lunco_doc_bevy::DocumentSaved): serialize
//!   the journal and write it to the active Twin's folder.
//!
//! `history/journal.json` is a JSON file, so the extension-keyed document
//! classifier (`.usda` → USD, etc.) never mounts it as a scene/document — it's
//! visible in the project folder but is not an openable twin document.
//!
//! Both observers no-op when no [`JournalResource`] is present, so they're safe
//! to register unconditionally (headless `--no-ui` servers without journaling
//! just skip them).

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_doc_bevy::{DocumentSaved, JournalResource};
use lunco_storage::{Storage, StorageHandle};
use lunco_twin_journal::{AuthorId, Journal as CanonicalJournal, TwinId as JournalTwinId};

use crate::session::TwinAdded;
use crate::{TwinId, WorkspaceResource};

/// Location of the journal file within a Twin folder — a **visible**
/// project-local `history/` folder (the durable, replayable edit log) so a
/// twin's history lives alongside its scene, not hidden under `.lunco/`.
const JOURNAL_REL_PATH: &str = "history/journal.json";

/// Absolute path to a Twin's journal file.
fn journal_path(twin_root: &Path) -> PathBuf {
    twin_root.join(JOURNAL_REL_PATH)
}

/// On-disk folder of `twin`, if it's a known Twin in the workspace.
fn twin_root(workspace: &WorkspaceResource, twin: TwinId) -> Option<PathBuf> {
    workspace.twin(twin).map(|t| t.root.clone())
}

/// The journal's **stable, cross-session** Twin identity, derived from the
/// Twin's on-disk root.
///
/// The journal file lives *under* this root, so the root is the natural
/// durable key — it moves with the folder, unlike the workspace's ephemeral
/// `TwinId(u64)` handle, which is re-minted every session and would point a
/// reloaded journal at the wrong Twin. This is what a journal is stamped with
/// and what save/load routing keys off, so a journal is always persisted to
/// the folder it actually belongs to (never to whichever Twin happens to be
/// active — the bug this fixes).
fn journal_twin_id(root: &Path) -> JournalTwinId {
    JournalTwinId::new(root.to_string_lossy())
}

/// Resolve a journal's stable id back to the on-disk root of the open Twin it
/// belongs to, or `None` when no currently-open Twin matches (e.g. its Twin
/// was closed). Routing through the *open* set means a stale journal can never
/// write into an unrelated Twin's folder.
fn root_for_journal_id(workspace: &WorkspaceResource, id: &JournalTwinId) -> Option<PathBuf> {
    workspace
        .twins()
        .find_map(|(_, t)| (journal_twin_id(&t.root) == *id).then(|| t.root.clone()))
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

/// Write `bytes` to `twin_root`'s journal file through the Storage API.
/// [`lunco_storage::write_file_sync`] is the single I/O chokepoint: on native it
/// is an atomic tmp+`fsync`+`rename` replace that also creates parent dirs
/// (CQ-107); on wasm it maps onto a `localStorage` set — both from one `#[cfg]`-free
/// call. The previous native path hand-rolled a `.tmp` sibling + `std::fs::rename`,
/// a redundant second rename on top of the backend's atomic write and a raw-`std::fs`
/// bypass of the abstraction.
fn write_journal_bytes(twin_root: &Path, bytes: &[u8]) -> lunco_storage::StorageResult<()> {
    lunco_storage::write_file_sync(&journal_path(twin_root), bytes)
}

/// Bind the in-memory [`JournalResource`] to a newly-opened Twin: flush the
/// outgoing Twin's journal to disk first (so opening a second Twin never drops
/// the first's history), then load this Twin's journal — stamping it with the
/// Twin's stable id — and swap it in.
///
/// The swap is in place to keep the shared `Arc`: recorders on document hosts
/// hold clones of this resource and must observe the bound journal.
///
/// > **Single-active-Twin scope.** There is one global journal, so this binds
/// > whichever Twin most recently opened. Two Twins with concurrently-recording
/// > documents would still share one in-memory journal; per-Twin in-memory
/// > isolation is the `HashMap<TwinId, JournalResource>` work tracked for the
/// > multi-Twin phase. This observer makes *persistence* correct regardless:
/// > each journal is always flushed/saved to the folder it belongs to.
pub(crate) fn on_twin_added_load_journal(
    trigger: On<TwinAdded>,
    workspace: Res<WorkspaceResource>,
    journal: Option<Res<JournalResource>>,
) {
    let Some(journal) = journal else { return };
    let Some(root) = twin_root(&workspace, trigger.event().twin) else { return };
    let target_id = journal_twin_id(&root);

    // 1. Flush the outgoing journal to *its own* Twin's folder before clobber,
    //    so its entries are never lost when a different Twin opens.
    let outgoing = journal.with_read(|j| {
        (j.twin() != &target_id && !j.is_empty()).then(|| (j.twin().clone(), j.to_bytes()))
    });
    if let Some((old_id, bytes)) = outgoing {
        match (root_for_journal_id(&workspace, &old_id), bytes) {
            (Some(old_root), Ok(bytes)) => {
                if let Err(err) = write_journal_bytes(&old_root, &bytes) {
                    warn!("[journal] flush of outgoing twin {} failed: {err}", old_id.0);
                }
            }
            (None, _) => {} // its Twin is no longer open — nothing to flush to.
            (_, Err(err)) => warn!("[journal] serialize of outgoing twin failed: {err}"),
        }
    }

    // 2. Load this Twin's journal (or start fresh), bound to its stable id.
    //    Re-stamping normalises journals written before the Twin had a stable
    //    id, so subsequent saves route back to this same folder.
    let loaded = match read_journal_bytes(&root) {
        Some(bytes) => match CanonicalJournal::from_bytes(&bytes) {
            Ok(mut j) => {
                j.set_twin(target_id.clone());
                j
            }
            Err(err) => {
                // A corrupt journal must never be overwritten by the fresh
                // empty one on the next save — that would destroy the twin's
                // entire history. Preserve it as `journal.json.bad` (the
                // `lunco-settings` pattern) before starting fresh.
                let path = journal_path(&root);
                let bad = path.with_extension("json.bad");
                warn!(
                    "[journal] could not parse {} ({err}); preserving as {} and starting fresh",
                    path.display(),
                    bad.display(),
                );
                if let Err(err) = lunco_storage::write_file_sync(&bad, &bytes) {
                    warn!(
                        "[journal] could not preserve corrupt journal to {}: {err}",
                        bad.display(),
                    );
                }
                CanonicalJournal::new(target_id.clone(), AuthorId::local())
            }
        },
        None => CanonicalJournal::new(target_id.clone(), AuthorId::local()),
    };
    let n = loaded.len();
    journal.with_write(|j| *j = loaded);
    info!(
        "[journal] bound twin {} ({n} entr{}) from {}",
        target_id.0,
        if n == 1 { "y" } else { "ies" },
        journal_path(&root).display(),
    );
}

/// Persist the journal to the folder of **the Twin it belongs to** whenever a
/// document is saved — resolved from the journal's own stable id, never from
/// the active Twin. Routing by the active Twin is the A1 corruption bug: saving
/// a doc while a *different* Twin is active would overwrite that Twin's
/// `journal.json` with the wrong history.
pub(crate) fn on_document_saved_persist_journal(
    _trigger: On<DocumentSaved>,
    workspace: Res<WorkspaceResource>,
    journal: Option<Res<JournalResource>>,
) {
    let Some(journal) = journal else { return };
    let (id, bytes) = journal.with_read(|j| (j.twin().clone(), j.to_bytes()));
    let bytes = match bytes {
        Ok(b) => b,
        Err(err) => {
            warn!("[journal] serialize failed: {err}");
            return;
        }
    };
    // Route by the journal's own identity. No matching open Twin (a loose /
    // untitled doc whose journal is still the default) → nothing twin-scoped
    // to persist.
    let Some(root) = root_for_journal_id(&workspace, &id) else {
        return;
    };
    if let Err(err) = write_journal_bytes(&root, &bytes) {
        warn!(
            "[journal] save to {} failed: {err}",
            journal_path(&root).display(),
        );
    }
}

/// Debounced periodic-save cadence — the journal is rewritten at most this often.
const SAVE_INTERVAL_SECS: f32 = 5.0;

/// Debounced periodic save: rewrite the journal to **its own twin's**
/// `<root>/history/journal.json` at most every [`SAVE_INTERVAL_SECS`], and only
/// when it grew. Gives a continuously-running host (the headless server)
/// crash-durable history without an explicit `DocumentSaved`. Routes by the
/// journal's own stable id (never the active twin), exactly like
/// [`on_document_saved_persist_journal`]; a no-op until a twin has bound the
/// journal (before that there's no twin folder to route to).
pub(crate) fn persist_journal_periodic(
    time: Res<Time>,
    workspace: Res<WorkspaceResource>,
    journal: Option<Res<JournalResource>>,
    mut acc: Local<f32>,
    mut last_saved_len: Local<usize>,
) {
    let Some(journal) = journal else { return };
    *acc += time.delta_secs();
    if *acc < SAVE_INTERVAL_SECS {
        return;
    }
    *acc = 0.0;
    let (id, len) = journal.with_read(|j| (j.twin().clone(), j.len()));
    if len == *last_saved_len {
        return; // nothing new since the last save
    }
    let Some(root) = root_for_journal_id(&workspace, &id) else {
        return; // journal not yet bound to an open twin — nothing to route to
    };
    let bytes = match journal.with_read(|j| j.to_bytes()) {
        Ok(b) => b,
        Err(err) => {
            warn!("[journal] serialize failed: {err}");
            return;
        }
    };
    if write_journal_bytes(&root, &bytes).is_ok() {
        *last_saved_len = len;
    }
}

/// The **single** journal-persistence mechanism, twin-folder-scoped: load
/// `<twin>/history/journal.json` when a twin opens, and save it back on every
/// explicit [`DocumentSaved`] **and** on a debounced periodic tick. Registered
/// by both the GUI workspace and the headless server — the former per-crate
/// global `lunco-doc-bevy` copy (which wrote to `~/.lunco/journal/`) is retired,
/// so there is now one history location and one code path (DRY).
pub struct WorkspaceJournalPlugin;

impl Plugin for WorkspaceJournalPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_twin_added_load_journal)
            .add_observer(on_document_saved_persist_journal)
            .add_systems(Update, persist_journal_periodic);
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

    #[test]
    fn journal_twin_id_is_stable_and_distinct_per_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_eq!(journal_twin_id(a.path()), journal_twin_id(a.path()));
        assert_ne!(journal_twin_id(a.path()), journal_twin_id(b.path()));
    }

    #[test]
    fn save_routes_by_journal_identity_not_active_twin() {
        // Two open Twins, A and B; B is the active one.
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let open = |p: &Path| match lunco_twin::TwinMode::open(p).unwrap() {
            lunco_twin::TwinMode::Twin(t) | lunco_twin::TwinMode::Folder(t) => t,
            lunco_twin::TwinMode::Orphan(_) => panic!("expected a folder"),
        };
        let mut ws = WorkspaceResource::new();
        let _a = ws.add_twin(open(dir_a.path()));
        let b = ws.add_twin(open(dir_b.path()));
        ws.active_twin = Some(b); // B active, but the journal belongs to A

        // A journal bound to A's identity (as `on_twin_added_load_journal` would
        // stamp it) with one entry.
        let mut j = CanonicalJournal::new(journal_twin_id(dir_a.path()), AuthorId::local());
        j.record_lifecycle(AuthorTag::local_user(), DocumentId::new(1), LifecycleKind::Saved);

        // Routing resolves A's folder from the journal's own id, *not* `active_twin`.
        let root =
            root_for_journal_id(&ws, j.twin()).expect("journal id resolves to its open twin");
        assert_eq!(root, dir_a.path());
        write_journal_bytes(&root, &j.to_bytes().unwrap()).unwrap();

        // A got the journal; B's folder is untouched (the corruption A1 fixed).
        assert!(journal_path(dir_a.path()).exists());
        assert!(!journal_path(dir_b.path()).exists());
    }

    #[test]
    fn set_twin_rebinds_legacy_journal_to_stable_id() {
        // A journal written before stable ids existed carries a placeholder id.
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = CanonicalJournal::new(JournalTwinId::new("local-twin"), AuthorId::local());
        legacy.record_lifecycle(AuthorTag::local_user(), DocumentId::new(1), LifecycleKind::Saved);

        // Loading it for `dir` re-stamps it so future saves route back here.
        legacy.set_twin(journal_twin_id(dir.path()));
        assert_eq!(legacy.twin(), &journal_twin_id(dir.path()));
    }
}
