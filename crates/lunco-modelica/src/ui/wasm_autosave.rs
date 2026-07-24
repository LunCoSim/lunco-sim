//! Wasm-only autosave for Untitled / duplicated Modelica documents.
//!
//! On native the workbench has a real filesystem behind
//! `Save / Save As`, so the user persists explicitly. The browser
//! sandbox doesn't, and reloading the page silently loses everything
//! the user typed or duplicated. This plugin closes that gap with
//! `localStorage`-backed autosave:
//!
//! 1. **Save**: every `DocumentChanged` for an Untitled document
//!    writes its current source to `localStorage` under
//!    `<KEY_PREFIX><display_name>`. The save is keyed on display
//!    name (the same name `bundled_models()` uses) so the restore
//!    side can reconstruct the in-memory entry from `localStorage`
//!    alone — no extra index file.
//! 2. **Restore**: on the first frame, scan `localStorage` for
//!    entries with the prefix, allocate one Modelica document per
//!    entry, register matching `InMemoryEntry` + `WorkspaceClass`
//!    rows so they show up in the Package Browser exactly like a
//!    fresh duplicate.
//! 3. **Forget**: `DocumentClosed` removes the entry — closing a
//!    tab really discards it.
//!
//! All three paths are wasm-only via `cfg(target_arch = "wasm32")`;
//! native compiles to an empty plugin. The browser's storage quota
//! is a few MB per origin — Modelica sources are well under 100 KB
//! typically, so a session with a dozen scratch models still fits.

use bevy::prelude::*;

/// Namespace prefix for autosave entries in `localStorage`. Keeps our
/// records out of the way of any other code (extensions, future
/// `lunco-storage` backends) that touches the same `localStorage`.
#[cfg(target_arch = "wasm32")]
const KEY_PREFIX: &str = "lunco_modelica/untitled/";

/// Namespace prefix for autosaved **uploaded / editable file** documents.
/// On web there is no real filesystem behind an opened `.mo`, so a
/// `DocumentOrigin::File { writable: true }` (the origin an upload lands as)
/// would be lost on reload exactly like an Untitled scratch doc. We persist
/// it here keyed by its path so `restore_from_localstorage` can rebuild the
/// File-backed doc. Distinct prefix from [`KEY_PREFIX`] so restore knows which
/// origin to reconstruct.
#[cfg(target_arch = "wasm32")]
const KEY_PREFIX_FILE: &str = "lunco_modelica/file/";

/// Single key holding the open-tab session: a JSON map of
/// `storage_key → drilled_class` for every open model-view tab whose document
/// is persistable (Untitled / writable File). Lets a reload bring back the
/// *set of tabs* the user had open (and which class each was drilled into),
/// not just the documents. Deliberately NOT under [`KEY_PREFIX`] /
/// [`KEY_PREFIX_FILE`] so the document-restore scan ignores it.
#[cfg(target_arch = "wasm32")]
const SESSION_TABS_KEY: &str = "lunco_modelica/session/tabs";

/// Bevy plugin that wires the three lifecycle observers + the
/// startup restore system. Add this **after** `ModelicaPlugin` so
/// the document registry it observes is already initialised.
pub struct WasmAutosavePlugin;

impl Plugin for WasmAutosavePlugin {
    fn build(&self, app: &mut App) {
        // R1 gesture flag — registered cross-platform so non-wasm
        // builds can also gate future native autosave on it without
        // a separate resource. Default `false` so behaviour is
        // unchanged until setters wire in.
        app.init_resource::<IsGestureActive>()
            // Mirror the editor's debounced-commit window into the
            // `text` source. Single-driver system: caller-side
            // setters in panel renders only handle their own field
            // (canvas in canvas_diagram::panel; modal — TODO), and
            // the text field is decoupled from the editor's render
            // path because `pending_commit_at` is just a timestamp
            // we can observe headlessly.
            .add_systems(
                bevy::prelude::Update,
                (drive_text_gesture_flag, drive_modal_gesture_flag),
            );
        #[cfg(target_arch = "wasm32")]
        {
            app.init_resource::<AutosaveKeys>()
                .add_systems(bevy::prelude::Startup, restore_from_localstorage)
                .add_systems(bevy::prelude::Update, persist_open_tabs)
                .add_observer(autosave_on_changed)
                .add_observer(forget_on_closed);
        }
        let _ = app;
    }
}

/// Mirror `EditorBufferState.pending_commit_at.is_some()` into
/// [`IsGestureActive::text`] every frame. Active while the editor
/// has uncommitted bytes; clears on the debounce flush (which sets
/// `pending_commit_at = None`). Cheap — two resource reads + one
/// write.
fn drive_text_gesture_flag(
    buf: bevy::prelude::Res<crate::ui::panels::code_editor::EditorBufferState>,
    mut gesture: bevy::prelude::ResMut<IsGestureActive>,
) {
    let active = buf.pending_commit_at.is_some();
    if gesture.text != active {
        gesture.text = active;
    }
}

/// Mirror "any modal dialog open" into [`IsGestureActive::modal`].
/// Currently observes the unsaved-close prompt (the only resource-
/// keyed dialog state on the bus today). When new modals land
/// (e.g. an in-app file picker, conflict-resolution prompt) extend
/// this driver to OR their pending state in.
///
/// `Option<Res<...>>` because the dialog state resource is owned by
/// `ModelicaCommandsPlugin`; if that plugin isn't loaded (minimal
/// test apps), the driver is a no-op.
fn drive_modal_gesture_flag(
    dialogs: Option<bevy::prelude::Res<crate::ui::commands::CloseDialogState>>,
    mut gesture: bevy::prelude::ResMut<IsGestureActive>,
) {
    let active = dialogs.map(|d| !d.pending.is_empty()).unwrap_or(false);
    if gesture.modal != active {
        gesture.modal = active;
    }
}

/// Side map of `DocumentId → display name` for every document we've
/// autosaved this session (plus those restored at startup).
///
/// `forget_on_closed` needs the display name to rebuild a document's
/// `localStorage` key, but `CloseDocument`'s `on_close_document`
/// observer removes the doc from the registry *before* the
/// `DocumentClosed` event fires — so the origin is unreachable by
/// then. This map captures the name while the document still exists,
/// so the autosave key survives long enough to be cleared. Without it
/// the `localStorage` entry leaks and `restore_from_localstorage`
/// resurrects the doc on the next reload.
#[cfg(target_arch = "wasm32")]
#[derive(bevy::prelude::Resource, Default)]
struct AutosaveKeys {
    by_doc: std::collections::HashMap<lunco_doc::DocumentId, String>,
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Build the storage key for an Untitled document's display name.
#[cfg(target_arch = "wasm32")]
fn storage_key(display_name: &str) -> String {
    format!("{KEY_PREFIX}{display_name}")
}

/// Build the storage key for an uploaded/editable File document's path.
#[cfg(target_arch = "wasm32")]
fn file_storage_key(path: &str) -> String {
    format!("{KEY_PREFIX_FILE}{path}")
}

/// Restore previously-autosaved Untitled documents at startup. One
/// allocation per entry; the existing `DocumentOpened` observers
/// (in `ui/mod.rs`) take care of registering the WorkspaceClass on
/// the side. Idempotent: re-running would no-op because we check
/// for an existing in-memory entry by display name.
#[cfg(target_arch = "wasm32")]
fn restore_from_localstorage(world: &mut World) {
    let Some(storage) = local_storage() else {
        return;
    };
    let len = storage.length().unwrap_or(0);
    if len == 0 {
        return;
    }
    // (full_key, is_file, ident, source). `ident` is the display name for an
    // Untitled doc or the path for an uploaded File doc. Scan BOTH namespaces
    // so uploads come back too, not just scratch docs.
    let mut entries: Vec<(String, bool, String, String)> = Vec::new();
    for i in 0..len {
        let Some(key) = storage.key(i).ok().flatten() else {
            continue;
        };
        let Some(source) = storage.get_item(&key).ok().flatten() else {
            continue;
        };
        if let Some(path) = key.strip_prefix(KEY_PREFIX_FILE) {
            entries.push((key.clone(), true, path.to_string(), source));
        } else if let Some(name) = key.strip_prefix(KEY_PREFIX) {
            entries.push((key.clone(), false, name.to_string(), source));
        }
    }
    // Sort so the restore order is deterministic across reloads —
    // localStorage iteration order is implementation-defined.
    entries.sort_by(|a, b| a.2.cmp(&b.2));
    // Open-tab session: storage_key → drilled class. Drives which restored docs
    // are re-opened as tabs (and to which drilled class) so the reload brings
    // back the user's tab layout, not just the documents.
    let session_tabs: std::collections::BTreeMap<String, Option<String>> = storage
        .get_item(SESSION_TABS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    for (full_key, is_file, ident, source) in entries {
        // Display name: the filename stem for an uploaded file, the stored
        // name for an Untitled doc.
        let display_name = if is_file {
            std::path::Path::new(&ident)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&ident)
                .to_string()
        } else {
            ident.clone()
        };
        // Skip if a doc with this display name already exists
        // (e.g. the bundled default tab, or a re-fired Startup).
        let already = {
            let cache = world.get_resource::<crate::package_tree::PackageTreeCache>();
            cache
                .map(|c| {
                    c.in_memory_models
                        .iter()
                        .any(|e| e.display_name == display_name)
                })
                .unwrap_or(false)
        };
        if already {
            continue;
        }
        // Rebuild the original origin: an uploaded file comes back as a
        // writable File (so its name/path persist and it keeps autosaving);
        // a scratch doc comes back Untitled.
        let origin = if is_file {
            lunco_doc::DocumentOrigin::File {
                path: std::path::PathBuf::from(&ident),
                writable: true,
            }
        } else {
            lunco_doc::DocumentOrigin::untitled(display_name.clone())
        };
        // Was this doc an open tab last session, and into which class was it
        // drilled? Looked up before `full_key` is moved into `AutosaveKeys`.
        let tab_drilled = session_tabs.get(&full_key).cloned();
        let mut registry = world.resource_mut::<crate::state::ModelicaDocumentRegistry>();
        let doc_id = registry.allocate_with_origin(source, origin);
        // Remember the full key so a later close can clear localStorage
        // even after the registry host is gone.
        world
            .resource_mut::<AutosaveKeys>()
            .by_doc
            .insert(doc_id, full_key);
        // Register an in-memory entry so the Package Browser shows the doc
        // under "Your Models". The browser's existing render path picks it up;
        // no extra UI plumbing.
        if let Some(mut cache) = world.get_resource_mut::<crate::package_tree::PackageTreeCache>() {
            let id = format!("mem://{display_name}");
            cache
                .in_memory_models
                .push(crate::package_tree::InMemoryEntry {
                    display_name,
                    id,
                    doc: doc_id,
                });
        }
        // Re-open the tab so a reload brings the user's model back *open* (and
        // drilled into the same class), not merely listed — a restored doc with
        // no tab looks "gone" (canvas shows Welcome). Reopen when the doc was an
        // open tab last session (`tab_drilled` present); also always reopen
        // uploaded files as a sensible fallback when there's no session record.
        if let Some(drilled) = tab_drilled.or(if is_file { Some(None) } else { None }) {
            let tab_id = world
                .resource_mut::<crate::model_tabs::ModelTabs>()
                .ensure_for(doc_id, drilled);
            if let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() {
                layout.open_instance(crate::ui::MODEL_VIEW_KIND, tab_id);
            }
        }
    }
}

/// Cross-truth rule R1 (see `docs/architecture/B0_CROSS_TRUTH_POLICY.md`):
/// "active gesture wins until idle". When any field is `true`,
/// `autosave_on_changed` bails — autosave fires again on the next
/// `DocumentChanged` after every source clears.
///
/// Per-source fields (rather than one global bool) because three
/// sources can be active simultaneously and a single bool would
/// race: e.g. canvas drag setting `false` on release while a modal
/// is still open. Each field has exactly one writer; readers OR
/// them via [`IsGestureActive::any`].
///
/// Default is "all clear" — autosave runs as before until setters
/// wire in. Setters land incrementally:
/// - `canvas`: written from `canvas_diagram/panel.rs` per frame
///   from `response.is_pointer_button_down_on()`. **Wired.R1.**
/// - `text`: written by the code editor while
///   `EditorBufferState.pending_commit_at` is `Some(_)`. **TODO.**
/// - `modal`: written by Open/Save As/prompt dialogs while open.
///   **TODO.**
#[derive(bevy::prelude::Resource, Default, Debug, Clone, Copy)]
pub struct IsGestureActive {
    pub canvas: bool,
    pub text: bool,
    pub modal: bool,
}

impl IsGestureActive {
    /// True when any source is in flight.
    pub fn any(&self) -> bool {
        self.canvas || self.text || self.modal
    }
}

/// Pure gate logic — extracted so it's testable on native without
/// the wasm-only `local_storage()` path. Returns `true` when the
/// observer should write to localStorage for `(active, untitled)`.
pub fn should_autosave(active: bool, is_untitled: bool) -> bool {
    // Two filters, both required:
    //   1. Untitled docs only — File-backed docs have a real save
    //      path, library/MSL/bundled docs are read-only.
    //   2. No active gesture — autosave snapshotting a half-drag
    //      writes "one component in two places" to disk.
    is_untitled && !active
}

/// Persist the document's current source to `localStorage` after
/// every change. Filters to Untitled docs only — File-backed docs
/// have a real save path; library/MSL/bundled docs are read-only.
/// Bails when an `IsGestureActive` resource indicates the user is
/// mid-gesture (R1).
#[cfg(target_arch = "wasm32")]
fn autosave_on_changed(
    trigger: bevy::prelude::On<lunco_doc_bevy::DocumentChanged>,
    registry: bevy::prelude::Res<crate::state::ModelicaDocumentRegistry>,
    gesture: bevy::prelude::Res<IsGestureActive>,
    mut keys: bevy::prelude::ResMut<AutosaveKeys>,
) {
    let Some(storage) = local_storage() else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    let document = host.document();
    let origin = document.origin();
    // Persist any *writable* doc the browser would otherwise lose on reload:
    //   - Untitled scratch docs        → keyed by display name (KEY_PREFIX)
    //   - uploaded / editable `.mo`     → keyed by path        (KEY_PREFIX_FILE)
    // Read-only origins (library/MSL/bundled) are never persisted. Both share
    // the R1 gesture gate (`should_autosave` for the untitled side; the same
    // `!gesture.any()` for files) so a half-drag is never snapshotted.
    let key = if origin.is_untitled() {
        if !should_autosave(gesture.any(), true) {
            return;
        }
        storage_key(&origin.display_name())
    } else if origin.is_writable() {
        if gesture.any() {
            return;
        }
        // Match the variant directly rather than `origin.path()` — bevy's
        // `GetPath` prelude trait otherwise shadows the inherent method.
        match origin {
            lunco_doc::DocumentOrigin::File { path, .. } => {
                file_storage_key(&path.to_string_lossy())
            }
            _ => return,
        }
    } else {
        return;
    };
    let source = document.source();
    // Never overwrite a good autosave with an empty buffer: a transient
    // `DocumentChanged` can fire before the source is installed (observed as
    // `len=0` localStorage entries that then "restore" as blank models). An
    // empty doc isn't worth persisting anyway.
    if source.is_empty() {
        return;
    }
    let _ = storage.set_item(&key, source);
    // Capture the full key while the doc still exists — `forget_on_closed`
    // can't reach the origin once the registry host is removed.
    keys.by_doc.insert(doc, key);
}

/// Persist the set of open model-view tabs (and the class each is drilled
/// into) so a reload restores the user's tab layout, not just the documents.
/// Keyed by each doc's `localStorage` storage key → drilled class; only tabs
/// on persistable docs (Untitled / writable File) are recorded — read-only
/// library/bundled tabs are re-openable from the browser and aren't persisted
/// as docs anyway. Writes only when the serialized set changes (cheap idle).
#[cfg(target_arch = "wasm32")]
fn persist_open_tabs(
    tabs: bevy::prelude::Res<crate::model_tabs::ModelTabs>,
    registry: bevy::prelude::Res<crate::state::ModelicaDocumentRegistry>,
    mut last: bevy::prelude::Local<String>,
) {
    let Some(storage) = local_storage() else {
        return;
    };
    // BTreeMap → deterministic JSON so the change-detection compare is stable.
    let mut map: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    for (_id, st) in tabs.iter() {
        let Some(host) = registry.host(st.doc) else {
            continue;
        };
        let origin = host.document().origin();
        let key = if origin.is_untitled() {
            storage_key(&origin.display_name())
        } else if origin.is_writable() {
            match origin {
                lunco_doc::DocumentOrigin::File { path, .. } => {
                    file_storage_key(&path.to_string_lossy())
                }
                _ => continue,
            }
        } else {
            continue;
        };
        // A doc can have several tabs (top-level + drilled); prefer recording a
        // drilled class so the reopened tab lands where the user was working.
        let entry = map.entry(key).or_insert(None);
        if st.drilled_class.is_some() {
            *entry = st.drilled_class.clone();
        }
    }
    let json = serde_json::to_string(&map).unwrap_or_default();
    if *last != json {
        let _ = storage.set_item(SESSION_TABS_KEY, &json);
        *last = json;
    }
}

/// Drop the autosaved entry when the user closes the tab — the
/// reload-and-find-it-back behaviour only makes sense for tabs
/// that are still part of the session.
#[cfg(target_arch = "wasm32")]
fn forget_on_closed(
    trigger: bevy::prelude::On<lunco_doc_bevy::DocumentClosed>,
    mut keys: bevy::prelude::ResMut<AutosaveKeys>,
) {
    let Some(storage) = local_storage() else {
        return;
    };
    let doc = trigger.event().doc;
    // `CloseDocument`'s `on_close_document` observer removes the doc
    // from the registry *before* `DocumentClosed` fires, so the
    // origin is unreachable here. Use the full storage key captured in
    // `AutosaveKeys` while the doc still existed. Absent ⇒ the doc was
    // never autosaved (read-only library/bundled) ⇒ nothing to clear.
    let Some(key) = keys.by_doc.remove(&doc) else {
        return;
    };
    let _ = storage.remove_item(&key);
}
