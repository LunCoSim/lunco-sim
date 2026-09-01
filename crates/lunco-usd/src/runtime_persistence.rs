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
//! journal (`journal_persistence.rs` in `lunco-workspace`), and can reload it
//! when the document opens — so runtime state survives across sessions without
//! ever touching the authored scene file. Persistence is one Twin-scoped opt-in
//! for both directions and is disabled unless the active Twin says otherwise.
//!
//! - **Load** on [`DocumentOpened`]: only when the active Twin's
//!   [`RUNTIME_PERSISTENCE_SETTING`] is `true`, read the overlay and
//!   [`restore_runtime`](crate::document::UsdDocument::restore_runtime) it into
//!   the freshly-built document.
//! - **Save** on [`DocumentChanged`]: controlled by the same Twin setting. A
//!   stale or corrupt `.lunco` file cannot affect the normal authored-scene
//!   load path.
//!
//! UI-free + headless; I/O goes through [`lunco_storage`]. No-ops for untitled /
//! non-twin docs (nowhere stable to persist) and when no `WorkspaceResource`
//! is present.

use crate::document::UsdDocument;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentChanged, DocumentOpened};
use lunco_storage::{Storage, StorageHandle};
use lunco_workspace::WorkspaceResource;
use openusd::sdf::SpecType;

use lunco_doc_bevy::DocumentRegistry;

/// Twin-relative subfolder the runtime overlays live under. Unlike the journal
/// (the durable, replayable edit log, kept in the visible `history/` folder), the
/// runtime overlay is a derived, disposable cache of live spawns/moves — so it
/// stays hidden under `.lunco/`.
///
/// The constant lives in `lunco-twin` alongside
/// [`is_runtime_state`](lunco_twin::is_runtime_state), the predicate that keeps
/// this directory out of scenario sync and out of release bundles. Writer and
/// excluders must not be able to drift apart.
use lunco_twin::RUNTIME_SUBDIR;

/// Twin-manifest key controlling persistence of generated runtime scene edits.
///
/// The value is deliberately a single boolean: loading without saving would
/// resurrect a state that later silently stops changing, while saving without
/// loading would write a cache that the user cannot see. The generic Twin
/// setting command and the Settings menu are the two callers of this one
/// project-owned policy.
pub const RUNTIME_PERSISTENCE_SETTING: &str = "usd.runtime_persistence";

/// Resolve the Twin that owns a document path.
///
/// The most-specific root wins when a workspace contains nested roots. Path
/// ownership and setting lookup must use the same resolver or a child document
/// could write into one Twin while reading policy from another.
fn twin_for_path<'a>(
    workspace: &'a WorkspaceResource,
    path: &Path,
) -> Option<&'a lunco_twin::Twin> {
    workspace
        .twins()
        .filter(|(_, twin)| path.strip_prefix(&twin.root).is_ok())
        .max_by_key(|(_, twin)| twin.root.components().count())
        .map(|(_, twin)| twin)
}

/// Resolve the Twin's runtime-persistence policy for a document.
///
/// Omitted means disabled. A malformed value is an authoring error and is
/// returned to the caller so the owner can report it rather than silently
/// interpreting a typo as permission to write project state.
pub fn runtime_persistence_enabled(
    workspace: &WorkspaceResource,
    doc_path: &Path,
) -> Result<bool, String> {
    let Some(twin) = twin_for_path(workspace, doc_path) else {
        return Ok(false);
    };
    runtime_persistence_for_twin(twin)
}

/// Read the runtime-persistence policy from one Twin manifest.
///
/// This is shared by the runtime writer/loader and the Settings menu so the
/// UI cannot advertise a policy different from the one that guards I/O.
pub fn runtime_persistence_for_twin(twin: &lunco_twin::Twin) -> Result<bool, String> {
    let Some(manifest) = twin.manifest.as_ref() else {
        return Ok(false);
    };
    match manifest.setting(RUNTIME_PERSISTENCE_SETTING) {
        None => Ok(false),
        Some(lunco_twin::TwinSettingValue::Bool(enabled)) => Ok(*enabled),
        Some(value) => Err(format!(
            "`{RUNTIME_PERSISTENCE_SETTING}` must be a boolean, got {value:?}"
        )),
    }
}

/// `<twin-root>/.lunco/runtime/<scene-rel>` for a document whose file lives
/// inside an open twin; `None` for untitled docs or files outside every open
/// twin (nowhere stable to persist).
fn runtime_path(workspace: &WorkspaceResource, doc_path: &Path) -> Option<PathBuf> {
    let twin = twin_for_path(workspace, doc_path)?;
    let rel = doc_path.strip_prefix(&twin.root).ok()?;
    Some(twin.root.join(RUNTIME_SUBDIR).join(rel))
}

/// Resolve a document's runtime-overlay path from the workspace + the doc's
/// origin. `None` unless the doc is a USD doc with a twin-rooted file path.
fn doc_runtime_path(
    workspace: &WorkspaceResource,
    registry: &DocumentRegistry<UsdDocument>,
    doc: DocumentId,
) -> Option<PathBuf> {
    let path = registry.host(doc)?.document().origin().canonical_path()?;
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

/// Write `bytes` through the shared storage boundary. The backend owns parent
/// creation and atomic replacement on native and persistence on wasm.
fn write_bytes(path: &Path, bytes: &[u8]) -> lunco_storage::StorageResult<()> {
    lunco_storage::write_file_sync(path, bytes)
}

/// True when a runtime layer carries real content (any prim opinion), as
/// opposed to a bare/empty stage — used to skip persisting empty overlays.
fn runtime_has_content(runtime: &openusd::sdf::Data) -> bool {
    runtime.iter().any(|(_, spec)| spec.ty == SpecType::Prim)
}

/// Restore a document's persisted runtime overlay (C4b spawns + moved
/// transforms) from `.lunco/runtime/…`, if one exists and the runtime layer is
/// still empty. No-op for untitled / non-twin docs or when no overlay exists.
///
/// Two callers share this: the twin drain ([`drain_pending_twin_docs`]
/// (crate::twin_projection::drain_pending_twin_docs)), which restores BEFORE the
/// scene's first mount so the single stage build composes `base ⊕ runtime`, and
/// the [`DocumentOpened`] observer (every other doc-open path — the observer
/// fires on a later command flush, too late for the twin mount). The
/// empty-runtime guard makes whichever runs second a no-op instead of a second
/// generation bump — whose synthetic `ReplaceSource` marker would force a
/// whole-scene rebuild (every prim despawned + respawned).
pub(crate) fn restore_doc_runtime(
    workspace: &WorkspaceResource,
    registry: &mut DocumentRegistry<UsdDocument>,
    doc: DocumentId,
) {
    let Some(doc_path) = registry
        .host(doc)
        .and_then(|host| host.document().origin().canonical_path())
    else {
        return;
    };
    match runtime_persistence_enabled(workspace, doc_path) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            warn!("[usd-runtime] {error}");
            return;
        }
    }
    let Some(path) = doc_runtime_path(workspace, registry, doc) else {
        return;
    };
    let restored = registry
        .host(doc)
        .map(|h| runtime_has_content(h.document().runtime_data()))
        .unwrap_or(true);
    if restored {
        return;
    }
    let Some(bytes) = read_bytes(&path) else {
        return;
    };
    let data = match String::from_utf8(bytes)
        .ok()
        .and_then(|text| lunco_usd_bevy::author::usda_to_data(&text).ok())
    {
        Some(data) => data,
        None => {
            warn!(
                "[usd-runtime] could not parse {} — ignoring",
                path.display()
            );
            return;
        }
    };
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut().restore_runtime(data);
        info!(
            "[usd-runtime] restored runtime overlay from {}",
            path.display()
        );
    }
}

/// Load a freshly-opened USD document's persisted runtime overlay on
/// [`DocumentOpened`], so session state survives reload — see
/// [`restore_doc_runtime`] (a no-op when the twin drain already restored it).
pub(crate) fn on_doc_opened_load_runtime(
    trigger: On<DocumentOpened>,
    workspace: Option<Res<WorkspaceResource>>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
) {
    let Some(workspace) = workspace else { return };
    restore_doc_runtime(&workspace, &mut registry, trigger.event().doc);
}

// TODO(#7 journal replay-on-open): today a reopened document's *current state* is
// reconstructed from the saved `.usda` base + this `.lunco/runtime` overlay, and
// the persisted twin journal (`<twin>/history/journal.json`) is a passive log —
// nothing local replays it. To make the journal an active reconstruct/undo
// source: on open, replay `merged_order(journal)` for this document via
// `DocumentRegistry::<UsdDocument>::replay_op` to rebuild runtime state (and the undo stack
// for cross-session undo), then demote `.lunco/runtime/*.usda` from a parallel
// truth to a snapshot cache-of-replay. Blocker: journal entries don't currently
// carry the owning `DocumentId` (EntityRef enrichment is deferred), so there's no
// entry→doc mapping to select which entries replay onto which document; and the
// primary-source switch risks replay-vs-saved divergence. Left as follow-up — the
// author-once op-replay projection (twin_projection) is the write-side prerequisite.

/// Persist a USD document's runtime overlay to `.lunco/runtime/…` whenever it
/// changes. The runtime layer holds generated state (spawns / moves) excluded
/// from the authored scene Save, so it has its own file. Skips docs with an
/// empty runtime layer (nothing to persist) or no twin-rooted path.
pub(crate) fn on_doc_changed_save_runtime(
    trigger: On<DocumentChanged>,
    workspace: Option<Res<WorkspaceResource>>,
    registry: Res<DocumentRegistry<UsdDocument>>,
) {
    let doc = trigger.event().doc;
    let Some(workspace) = workspace else { return };
    let Some(host) = registry.host(doc) else {
        return;
    };
    let Some(path) = doc_runtime_path(&workspace, &registry, doc) else {
        return;
    };
    let Some(doc_path) = host.document().origin().canonical_path() else {
        return;
    };
    match runtime_persistence_enabled(&workspace, doc_path) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            warn!("[usd-runtime] {error}");
            return;
        }
    }
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
    fn runtime_persistence_is_off_without_twin_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));
        let scene = dir.path().join("scene.usda");
        assert_eq!(runtime_persistence_enabled(&ws, &scene), Ok(false));
    }

    fn open_twin_with_setting(p: &Path, value: lunco_twin::TwinSettingValue) -> lunco_twin::Twin {
        let mut manifest = lunco_twin::TwinManifest::new("runtime persistence test");
        manifest
            .set_setting(RUNTIME_PERSISTENCE_SETTING, value)
            .unwrap();
        manifest
            .write(&p.join(lunco_twin::MANIFEST_FILENAME))
            .unwrap();
        match lunco_twin::TwinMode::open(p).unwrap() {
            lunco_twin::TwinMode::Twin(twin) => twin,
            other => panic!("expected manifest-backed Twin, got {other:?}"),
        }
    }

    #[test]
    fn runtime_persistence_requires_a_boolean_twin_setting() {
        let dir = tempfile::tempdir().unwrap();
        let twin =
            open_twin_with_setting(dir.path(), lunco_twin::TwinSettingValue::Text("yes".into()));
        let mut ws = WorkspaceResource::new();
        ws.add_twin(twin);
        let error = runtime_persistence_enabled(&ws, &dir.path().join("scene.usda"))
            .expect_err("malformed setting must be visible");
        assert!(error.contains("must be a boolean"));
    }

    #[test]
    fn runtime_persistence_is_enabled_only_by_the_twin_setting() {
        let dir = tempfile::tempdir().unwrap();
        let twin = open_twin_with_setting(dir.path(), lunco_twin::TwinSettingValue::Bool(true));
        let mut ws = WorkspaceResource::new();
        ws.add_twin(twin);
        assert_eq!(
            runtime_persistence_enabled(&ws, &dir.path().join("scene.usda")),
            Ok(true)
        );
    }

    #[test]
    fn corrupt_runtime_overlay_is_ignored_when_loading_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin_with_setting(
            dir.path(),
            lunco_twin::TwinSettingValue::Bool(true),
        ));
        let scene_abs = dir.path().join("scene.usda");
        std::fs::write(&scene_abs, TINY).unwrap();
        let path = dir.path().join(".lunco/runtime/scene.usda");
        write_bytes(&path, b"not valid USDA").unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());
        restore_doc_runtime(&ws, &mut registry, doc);
        assert!(!runtime_has_content(
            registry.host(doc).unwrap().document().runtime_data()
        ));
    }

    #[test]
    fn runtime_overlay_is_ignored_without_twin_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));
        let scene_abs = dir.path().join("scene.usda");
        std::fs::write(&scene_abs, TINY).unwrap();

        let mut source = UsdDocument::with_origin(
            DocumentId::new(11),
            TINY,
            DocumentOrigin::writable_file(scene_abs.clone()),
        );
        source
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: None,
                reference: Some("vessels/rovers/skid_rover.usda".into()),
            })
            .unwrap();
        let text = lunco_usd_bevy::author::data_to_usda(source.runtime_data()).unwrap();
        write_bytes(
            &dir.path().join(".lunco/runtime/scene.usda"),
            text.as_bytes(),
        )
        .unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());
        restore_doc_runtime(&ws, &mut registry, doc);
        assert!(!runtime_has_content(
            registry.host(doc).unwrap().document().runtime_data()
        ));
    }

    #[test]
    fn runtime_path_maps_scene_under_twin_dotlunco() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));

        let scene = dir.path().join("scenes/luncosim/scene.usda");
        let rt = runtime_path(&ws, &scene).expect("scene inside twin resolves");
        assert_eq!(
            rt,
            dir.path().join(".lunco/runtime/scenes/luncosim/scene.usda")
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
        assert!(
            !runtime_has_content(reopened.runtime_data()),
            "fresh doc has empty runtime"
        );

        let bytes = read_bytes(&rt_file).expect("overlay present");
        let data =
            lunco_usd_bevy::author::usda_to_data(&String::from_utf8(bytes).unwrap()).unwrap();
        reopened.restore_runtime(data);

        // The spawn is back in the runtime layer + composed view, base still clean.
        let prim = SdfPath::new("/World/rover_1").unwrap();
        assert!(
            reopened.runtime_data().spec(&prim).is_some(),
            "runtime spawn restored"
        );
        assert!(
            reopened.data().spec(&prim).is_none(),
            "base untouched by restore"
        );
        assert!(
            reopened
                .composed_source()
                .contains("@vessels/rovers/skid_rover.usda@"),
            "restored spawn rides the composed view"
        );
    }

    #[test]
    fn restore_doc_runtime_is_idempotent_across_drain_and_observer() {
        // The twin drain restores BEFORE the scene mounts; the `DocumentOpened`
        // observer fires a flush later and must NOT restore again — a second
        // restore bumps the generation with a coarse `ReplaceSource` marker,
        // which forces a whole-scene rebuild (the old "everything spawns twice
        // on twin open").
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin_with_setting(
            dir.path(),
            lunco_twin::TwinSettingValue::Bool(true),
        ));

        let scene_abs = dir.path().join("scene.usda");
        std::fs::write(&scene_abs, TINY).unwrap();

        // Persist a runtime overlay with one spawn (same shape the app writes).
        let mut src = UsdDocument::with_origin(
            DocumentId::new(10),
            TINY,
            DocumentOrigin::writable_file(scene_abs.clone()),
        );
        src.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: None,
            reference: Some("vessels/rovers/skid_rover.usda".into()),
        })
        .unwrap();
        let text = lunco_usd_bevy::author::data_to_usda(src.runtime_data()).unwrap();
        write_bytes(
            &dir.path().join(".lunco/runtime/scene.usda"),
            text.as_bytes(),
        )
        .unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());

        restore_doc_runtime(&ws, &mut registry, doc);
        let host = registry.host(doc).unwrap();
        assert!(
            runtime_has_content(host.document().runtime_data()),
            "first call restores the persisted spawn"
        );
        let gen_after_first = host.document().generation();

        restore_doc_runtime(&ws, &mut registry, doc);
        assert_eq!(
            registry.host(doc).unwrap().document().generation(),
            gen_after_first,
            "second call is a no-op — no generation bump, no forced rebuild"
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
