//! Runtime-layer persistence (C5-A).
//!
//! A [`UsdDocument`](crate::document::UsdDocument) has two layers: the authored
//! `base` (serialized to the scene `.usda` on Save) and a generated `runtime`
//! overlay — the C4b spawns + moved transforms — that is deliberately **not**
//! part of the authored file. The edit journal records the runtime ops but
//! never replays them, so without this module a reloaded document's runtime
//! layer is empty and that session state is lost.
//!
//! This persists the runtime overlay to its **own** file,
//! `<twin-root>/.lunco/runtime/<scene-path-relative-to-twin>`, parallel to the
//! journal (`journal_persistence.rs` in `lunco-workspace`), and reloads it when
//! the document opens — so runtime state survives across sessions without ever
//! touching the authored scene file.
//!
//! - **Load** on [`DocumentOpened`]: read the overlay and
//!   [`restore_runtime`](crate::document::UsdDocument::restore_runtime) it into
//!   the freshly-built document.
//! - **Save** on [`DocumentChanged`]: serialize the current runtime layer and
//!   write it, skipping docs with an empty runtime layer or no twin-rooted path.
//!
//! UI-free + headless; I/O goes through [`lunco_storage`]. No-ops for untitled /
//! non-twin docs (nowhere stable to persist) and when no `WorkspaceResource`
//! is present.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentChanged, DocumentOpened};
use lunco_storage::{Storage, StorageHandle};
use lunco_workspace::WorkspaceResource;
use openusd::sdf::SpecType;

use crate::registry::UsdDocumentRegistry;

/// Twin-relative subfolder the runtime overlays live under, alongside the
/// journal's `.lunco/journal/`.
const RUNTIME_SUBDIR: &str = ".lunco/runtime";

/// `<twin-root>/.lunco/runtime/<scene-rel>` for a document whose file lives
/// inside an open twin; `None` for untitled docs or files outside every open
/// twin (nowhere stable to persist).
fn runtime_path(workspace: &WorkspaceResource, doc_path: &Path) -> Option<PathBuf> {
    workspace.twins().find_map(|(_, twin)| {
        doc_path
            .strip_prefix(&twin.root)
            .ok()
            .map(|rel| twin.root.join(RUNTIME_SUBDIR).join(rel))
    })
}

/// Resolve a document's runtime-overlay path from the workspace + the doc's
/// origin. `None` unless the doc is a USD doc with a twin-rooted file path.
fn doc_runtime_path(
    workspace: &WorkspaceResource,
    registry: &UsdDocumentRegistry,
    doc: DocumentId,
) -> Option<PathBuf> {
    let path = registry.host(doc)?.document().origin().path()?;
    runtime_path(workspace, path)
}

/// Tolerant read: a missing / unreadable overlay means "start fresh", never an
/// error surfaced to the user.
fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    let handle = StorageHandle::File(path.to_path_buf());
    #[cfg(not(target_arch = "wasm32"))]
    let result = lunco_storage::FileStorage::new().read_sync(&handle);
    #[cfg(target_arch = "wasm32")]
    let result = lunco_storage::WebStorage::new().read_sync(&handle);
    result.ok()
}

/// Write `bytes` to `path`. Native: write a `.tmp` sibling then atomically
/// `rename` over the target (the established lunco pattern, see `recents.rs` /
/// `journal_persistence.rs`). Wasm: a `localStorage` set is already atomic.
fn write_bytes(path: &Path, bytes: &[u8]) -> lunco_storage::StorageResult<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("usda.tmp");
        lunco_storage::FileStorage::new().write_sync(&StorageHandle::File(tmp.clone()), bytes)?;
        std::fs::rename(&tmp, path).map_err(lunco_storage::StorageError::Io)?;
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        lunco_storage::WebStorage::new().write_sync(&StorageHandle::File(path.to_path_buf()), bytes)
    }
}

/// True when a runtime layer carries real content (any prim opinion), as
/// opposed to a bare/empty stage — used to skip persisting empty overlays.
fn runtime_has_content(runtime: &openusd::sdf::Data) -> bool {
    runtime.iter().any(|(_, spec)| spec.ty == SpecType::Prim)
}

/// Load a freshly-opened USD document's persisted runtime overlay (C4b spawns +
/// moved transforms) from `.lunco/runtime/…` and
/// [`restore_runtime`](crate::document::UsdDocument::restore_runtime) it, so
/// session state survives reload. No-op for untitled / non-twin docs, when no
/// overlay exists yet, or for non-USD documents (which have no host here).
pub(crate) fn on_doc_opened_load_runtime(
    trigger: On<DocumentOpened>,
    workspace: Option<Res<WorkspaceResource>>,
    mut registry: ResMut<UsdDocumentRegistry>,
) {
    let doc = trigger.event().doc;
    let Some(workspace) = workspace else { return };
    let Some(path) = doc_runtime_path(&workspace, &registry, doc) else {
        return;
    };
    let Some(bytes) = read_bytes(&path) else { return };
    let data = match String::from_utf8(bytes)
        .ok()
        .and_then(|text| lunco_usd_bevy::author::usda_to_data(&text).ok())
    {
        Some(data) => data,
        None => {
            warn!("[usd-runtime] could not parse {} — ignoring", path.display());
            return;
        }
    };
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut().restore_runtime(data);
        info!("[usd-runtime] restored runtime overlay from {}", path.display());
    }
}

/// Persist a USD document's runtime overlay to `.lunco/runtime/…` whenever it
/// changes. The runtime layer holds generated state (spawns / moves) excluded
/// from the authored scene Save, so it has its own file. Skips docs with an
/// empty runtime layer (nothing to persist) or no twin-rooted path.
pub(crate) fn on_doc_changed_save_runtime(
    trigger: On<DocumentChanged>,
    workspace: Option<Res<WorkspaceResource>>,
    registry: Res<UsdDocumentRegistry>,
) {
    let doc = trigger.event().doc;
    let Some(workspace) = workspace else { return };
    let Some(host) = registry.host(doc) else { return };
    let Some(path) = doc_runtime_path(&workspace, &registry, doc) else {
        return;
    };
    let runtime = host.document().runtime_data();
    if !runtime_has_content(runtime) {
        return; // no spawns / moves — don't litter `.lunco` with empty overlays
    }
    let text = match lunco_usd_bevy::author::data_to_usda(runtime) {
        Ok(text) => text,
        Err(e) => {
            warn!("[usd-runtime] serialize of runtime layer failed: {e}");
            return;
        }
    };
    if let Err(e) = write_bytes(&path, text.as_bytes()) {
        warn!("[usd-runtime] save to {} failed: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdDocument, UsdOp};
    use lunco_doc::{Document, DocumentOrigin};
    use lunco_usd_bevy::usd_data::UsdDataExt;
    use openusd::sdf::Path as SdfPath;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// Open a folder as a twin (mirrors `journal_persistence` tests).
    fn open_twin(p: &Path) -> lunco_twin::Twin {
        match lunco_twin::TwinMode::open(p).unwrap() {
            lunco_twin::TwinMode::Twin(t) | lunco_twin::TwinMode::Folder(t) => t,
            lunco_twin::TwinMode::Orphan(_) => panic!("expected a folder twin"),
        }
    }

    #[test]
    fn runtime_path_maps_scene_under_twin_dotlunco() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));

        let scene = dir.path().join("scenes/sandbox/scene.usda");
        let rt = runtime_path(&ws, &scene).expect("scene inside twin resolves");
        assert_eq!(
            rt,
            dir.path().join(".lunco/runtime/scenes/sandbox/scene.usda")
        );

        // A path outside every twin has nowhere stable to persist.
        assert!(runtime_path(&ws, Path::new("/elsewhere/x.usda")).is_none());
    }

    #[test]
    fn runtime_overlay_round_trips_and_restores_into_a_fresh_doc() {
        let dir = tempfile::tempdir().unwrap();
        let rt_file = dir.path().join(".lunco/runtime/scene.usda");

        // 1. A document with a C4b spawn authored into its runtime layer.
        let mut src = UsdDocument::with_origin(
            DocumentId::new(1),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        src.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: None,
            reference: Some("vessels/rovers/skid_rover.usda".into()),
        })
        .unwrap();
        assert!(runtime_has_content(src.runtime_data()));

        // 2. Persist the runtime layer to its `.lunco` file.
        let text = lunco_usd_bevy::author::data_to_usda(src.runtime_data()).unwrap();
        write_bytes(&rt_file, text.as_bytes()).unwrap();
        assert!(rt_file.exists());

        // 3. A FRESH document (base only, empty runtime) — then restore.
        let mut reopened = UsdDocument::with_origin(
            DocumentId::new(2),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        assert!(!runtime_has_content(reopened.runtime_data()), "fresh doc has empty runtime");

        let bytes = read_bytes(&rt_file).expect("overlay present");
        let data = lunco_usd_bevy::author::usda_to_data(&String::from_utf8(bytes).unwrap()).unwrap();
        reopened.restore_runtime(data);

        // The spawn is back in the runtime layer + composed view, base still clean.
        let prim = SdfPath::new("/World/rover_1").unwrap();
        assert!(reopened.runtime_data().spec(&prim).is_some(), "runtime spawn restored");
        assert!(reopened.data().spec(&prim).is_none(), "base untouched by restore");
        assert!(
            reopened.composed_source().contains("@vessels/rovers/skid_rover.usda@"),
            "restored spawn rides the composed view"
        );
    }

    #[test]
    fn missing_overlay_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_bytes(&dir.path().join("nope.usda")).is_none());
    }

    #[test]
    fn empty_runtime_layer_is_not_persisted() {
        // A doc with no spawns/moves has nothing to persist.
        let doc = UsdDocument::with_origin(
            DocumentId::new(3),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        assert!(!runtime_has_content(doc.runtime_data()));
    }
}
