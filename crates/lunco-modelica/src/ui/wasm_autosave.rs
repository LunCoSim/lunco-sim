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

/// Build the storage key for a document's display name.
#[cfg(target_arch = "wasm32")]
fn storage_key(display_name: &str) -> String {
    format!("{KEY_PREFIX}{display_name}")
}

/// Restore previously-autosaved Untitled documents at startup. One
/// allocation per entry; the existing `DocumentOpened` observers
/// (in `ui/mod.rs`) take care of registering the WorkspaceClass on
/// the side. Idempotent: re-running would no-op because we check
/// for an existing in-memory entry by display name.
#[cfg(target_arch = "wasm32")]
fn restore_from_localstorage(world: &mut World) {
    let Some(storage) = local_storage() else { return };
    let len = storage.length().unwrap_or(0);
    if len == 0 {
        return;
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    for i in 0..len {
        let Some(key) = storage.key(i).ok().flatten() else { continue };
        let Some(name) = key.strip_prefix(KEY_PREFIX) else { continue };
        let Some(source) = storage.get_item(&key).ok().flatten() else { continue };
        entries.push((name.to_string(), source));
    }
    // Sort so the restore order is deterministic across reloads —
    // localStorage iteration order is implementation-defined.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (display_name, source) in entries {
        // Skip if a doc with this display name already exists
        // (e.g. the bundled default tab, or a re-fired Startup).
        let already = {
            let cache = world
                .get_resource::<crate::ui::panels::package_browser::PackageTreeCache>();
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
        let mut registry = world.resource_mut::<crate::ui::state::ModelicaDocumentRegistry>();
        let doc_id = registry.allocate_with_origin(
            source,
            lunco_doc::DocumentOrigin::untitled(display_name.clone()),
        );
        // Remember the key so a later close can clear localStorage
        // even after the registry host is gone.
        world
            .resource_mut::<AutosaveKeys>()
            .by_doc
            .insert(doc_id, display_name.clone());
        // Register an in-memory entry so the Package Browser shows
        // the doc under "Workspace / (Untitled)". The browser's
        // existing render path picks it up; no extra UI plumbing.
        if let Some(mut cache) = world
            .get_resource_mut::<crate::ui::panels::package_browser::PackageTreeCache>()
        {
            let id = format!("mem://{display_name}");
            cache
                .in_memory_models
                .push(crate::ui::panels::package_browser::InMemoryEntry {
                    display_name,
                    id,
                    doc: doc_id,
                });
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
    registry: bevy::prelude::Res<crate::ui::state::ModelicaDocumentRegistry>,
    gesture: bevy::prelude::Res<IsGestureActive>,
    mut keys: bevy::prelude::ResMut<AutosaveKeys>,
) {
    let Some(storage) = local_storage() else { return };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else { return };
    let document = host.document();
    let origin = document.origin();
    if !should_autosave(gesture.any(), origin.is_untitled()) {
        return;
    }
    let display_name = origin.display_name();
    let key = storage_key(&display_name);
    let _ = storage.set_item(&key, document.source());
    // Capture the key while the doc still exists — `forget_on_closed`
    // can't reach the origin once the registry host is removed.
    keys.by_doc.insert(doc, display_name.to_string());
}

/// Drop the autosaved entry when the user closes the tab — the
/// reload-and-find-it-back behaviour only makes sense for tabs
/// that are still part of the session.
#[cfg(target_arch = "wasm32")]
fn forget_on_closed(
    trigger: bevy::prelude::On<lunco_doc_bevy::DocumentClosed>,
    mut keys: bevy::prelude::ResMut<AutosaveKeys>,
) {
    let Some(storage) = local_storage() else { return };
    let doc = trigger.event().doc;
    // `CloseDocument`'s `on_close_document` observer removes the doc
    // from the registry *before* `DocumentClosed` fires, so the
    // origin is unreachable here. Use the display name captured in
    // `AutosaveKeys` while the doc still existed. Absent ⇒ the doc was
    // never autosaved (File-backed / bundled) ⇒ nothing to clear.
    let Some(display_name) = keys.by_doc.remove(&doc) else { return };
    let key = storage_key(&display_name);
    let _ = storage.remove_item(&key);
}
