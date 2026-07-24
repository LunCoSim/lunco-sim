//! USD's binding of the generic document registry.
//!
//! There is **no `DocumentRegistry<UsdDocument>`**. Every live `.usda` document lives in
//! [`lunco_doc_bevy::DocumentRegistry<UsdDocument>`], the same type Modelica and
//! scripting use — `allocate` / `open_file` / `host` / `apply` / `replay_op` /
//! the pending-event rings are written once, there.
//!
//! WHAT USED TO BE HERE, and why it isn't: a hand-copied registry (same `hosts`
//! map, same `next_doc_id`, same rings, same journal wiring as Modelica's) that
//! also hand-rolled the open-by-path rule — dedup by path, but never refresh the
//! content. So re-opening an edited `.usda` replayed the OLD scene until the app
//! restarted. Modelica's copy omitted the dedup instead and minted duplicate
//! documents. One rule, two hand-rolled copies, two opposite bugs; it now lives
//! in [`lunco_doc_bevy::DocumentRegistry::open_file`] alone.
//!
//! USD's own half of the contract is [`lunco_doc::FileBacked`] on
//! [`UsdDocument`](crate::document::UsdDocument) (`crate::document`) — how to
//! build, whether it's dirty, how to re-read it.

#[cfg(test)]
mod tests {
    use crate::document::{LayerId, UsdDocument, UsdOp};
    use lunco_doc::{Document, OpenOutcome};
    use lunco_doc_bevy::DocumentRegistry;

    const TINY_USDA: &str = "#usda 1.0\ndef Xform \"World\" {}\n";

    fn reg() -> DocumentRegistry<UsdDocument> {
        DocumentRegistry::default()
    }

    /// THE REGRESSION. Re-opening a file whose text changed on disk must project
    /// the NEW text through the SAME document.
    ///
    /// Both open paths used to read `if !already_open { allocate(source) }`, so a
    /// second open dropped the freshly-read source and kept the stale document:
    /// editing a `.usda` and re-opening the Twin replayed the pre-edit scene and
    /// only an app restart picked it up. Identity is reused; content is not.
    #[test]
    fn reopen_refreshes_a_clean_document_in_place() {
        let mut reg = reg();
        let path = std::path::PathBuf::from("/twins/school/traverse.usda");

        let (first, out) = reg.open_file(path.clone(), TINY_USDA.to_string());
        assert_eq!(out, OpenOutcome::Allocated);

        let edited = "#usda 1.0\ndef Xform \"World\" {}\ndef Xform \"POI\" {}\n";
        let (second, out) = reg.open_file(path.clone(), edited.to_string());

        assert_eq!(
            second, first,
            "same file ⇒ same document, never a second id"
        );
        assert_eq!(out, OpenOutcome::Refreshed);
        assert!(
            reg.host(first)
                .unwrap()
                .document()
                .composed_source()
                .contains("POI"),
            "re-open must project the NEW disk text, not replay the resident base"
        );
        // Came from disk ⇒ clean, so nothing prompts the user to save it back.
        assert!(!lunco_doc::FileBacked::is_dirty(
            reg.host(first).unwrap().document()
        ));
    }

    /// Unsaved work outranks disk. Undo cannot recover a clobbered base, so the
    /// registry refuses and hands the conflict back to the caller.
    ///
    /// Note this is deliberately NOT `SdfLayer::Reload()`'s behaviour — USD's
    /// primitive discards a dirty layer's edits. USD leaves the policy to the
    /// app; this is that policy.
    #[test]
    fn reopen_never_clobbers_unsaved_edits() {
        let mut reg = reg();
        let path = std::path::PathBuf::from("/twins/school/traverse.usda");
        let (doc, _) = reg.open_file(path.clone(), TINY_USDA.to_string());

        reg.host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::ReplaceSource {
                edit_target: LayerId::root(),
                text: "#usda 1.0\ndef Xform \"Mine\" {}\n".to_string(),
            })
            .unwrap();
        assert!(lunco_doc::FileBacked::is_dirty(
            reg.host(doc).unwrap().document()
        ));

        let (same, out) = reg.open_file(path, TINY_USDA.to_string());
        assert_eq!(same, doc);
        assert_eq!(out, OpenOutcome::KeptDirty);
        assert!(
            reg.host(doc)
                .unwrap()
                .document()
                .composed_source()
                .contains("Mine"),
            "the user's unsaved edit must survive a re-open"
        );
    }

    /// A referenced `.usda` edited on disk behind the app — a git pull, an
    /// external editor — used to invalidate nothing: the open document stayed
    /// silently stale. `stale_docs` now notices, and a save re-baselines so the
    /// app's own write is never mistaken for an outside change.
    ///
    /// Uses a real temp file and stamps mtime forward with `set_modified`, so it
    /// tests the actual filesystem path without a sleep to cross a clock tick.
    #[test]
    fn stale_docs_flags_an_external_edit_and_save_clears_it() {
        use std::time::{Duration, SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scene.usda");
        std::fs::write(&path, TINY_USDA).unwrap();

        let mut reg = reg();
        let (doc, _) = reg.open_file(path.clone(), TINY_USDA.to_string());
        assert!(reg.stale_docs().is_empty(), "just read — must be in sync");

        // Someone rewrites the file on disk; force mtime unambiguously forward.
        std::fs::write(&path, "#usda 1.0\ndef Xform \"Outside\" {}\n").unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(10))
            .unwrap();
        assert_eq!(reg.stale_docs(), vec![doc], "external edit must be flagged");

        // The app saves (re-baseline): its own write is not an external change.
        reg.note_saved(doc);
        assert!(
            reg.stale_docs().is_empty(),
            "save re-baselines the watermark"
        );
    }

    /// A broken file on disk must not half-apply over a working document.
    #[test]
    fn reopen_with_unparsable_source_keeps_the_resident_base() {
        let mut reg = reg();
        let path = std::path::PathBuf::from("/twins/school/traverse.usda");
        let (doc, _) = reg.open_file(path.clone(), TINY_USDA.to_string());

        let (same, out) = reg.open_file(path, "this is not usda at all {{{".to_string());
        assert_eq!(same, doc);
        assert_eq!(out, OpenOutcome::KeptUnparsable);
        assert!(reg
            .host(doc)
            .unwrap()
            .document()
            .composed_source()
            .contains("World"));
    }

    /// Identity is the FILE, not the string. `allocate` mints a second document
    /// for the same path (two undo stacks, racing saves) — `open_file` is the
    /// reason nobody has to remember that.
    #[test]
    fn open_file_is_one_document_per_path_unlike_allocate() {
        let mut reg = reg();
        let path = std::path::PathBuf::from("/twins/school/traverse.usda");

        let (a, _) = reg.open_file(path.clone(), TINY_USDA.to_string());
        let (b, _) = reg.open_file(path.clone(), TINY_USDA.to_string());
        assert_eq!(a, b);
        assert_eq!(reg.ids().count(), 1);

        assert_eq!(reg.doc_for_file(&path), Some(a));
        assert_eq!(
            reg.doc_for_file(std::path::Path::new("/twins/other.usda")),
            None
        );

        // Untitled docs have no path and must never collide with it.
        reg.allocate(
            TINY_USDA.to_string(),
            lunco_doc::PathlessOrigin::untitled("Untitled.usda"),
        );
        assert_eq!(reg.doc_for_file(&path), Some(a));
    }

    #[test]
    fn allocate_emits_opened_and_changed() {
        let mut reg = reg();
        let id = reg.allocate(
            TINY_USDA.to_string(),
            lunco_doc::PathlessOrigin::untitled("U.usda"),
        );
        let pending = reg.drain_pending();
        assert_eq!(pending.opened, vec![id]);
        assert_eq!(pending.changed, vec![id]);
        assert!(pending.closed.is_empty());
    }

    #[test]
    fn apply_marks_changed() {
        let mut reg = reg();
        let id = reg.allocate(
            TINY_USDA.to_string(),
            lunco_doc::PathlessOrigin::untitled("U.usda"),
        );
        let _ = reg.drain_pending();
        reg.apply(
            id,
            UsdOp::ReplaceSource {
                edit_target: LayerId::root(),
                text: TINY_USDA.to_string(),
            },
        )
        .unwrap();
        assert_eq!(reg.drain_pending().changed, vec![id]);
    }
}
