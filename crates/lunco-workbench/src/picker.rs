//! Event-driven file picker.
//!
//! Storage is the wrong layer for dialog plumbing — it's the I/O
//! abstraction that loads and saves doc bytes. Picking a path is a UI
//! concern: a modal native dialog on desktop, a JS prompt on the web.
//! So the picker lives here in the workbench and just *uses*
//! `OpenFilter` / `SaveHint` / `StorageHandle` from `lunco-storage` to
//! describe the request and the result.
//!
//! ## Pattern
//!
//! Panels and commands fire [`PickHandle`]; a backend observer (native
//! `rfd` today, web File-System-Access tomorrow) resolves the dialog
//! asynchronously and emits [`PickResolved`] (success) or
//! [`PickCancelled`]. A workbench-side dispatcher reads the resolved
//! event and triggers the matching typed file-workflow command
//! (`OpenFile { path }`, `SaveAsDocument { doc, path }`, ...).
//!
//! This split keeps UI code synchronous (no `async`, no polling), keeps
//! the backend swap a `cfg`-gated observer rather than a call-site
//! rewrite, and gives HTTP / scripting callers a uniform shape: trigger
//! the resolved follow-up command directly to skip the dialog, or
//! trigger [`PickHandle`] to force one.
//!
//! Workbench-level file-workflow commands (`OpenFile`, `OpenFolder`,
//! `OpenTwin`, `SaveAsDocument`, `SaveAsTwin`) and the routing observer
//! that consumes [`PickResolved`] arrive in follow-up commits. This
//! module ships only the picker itself.

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_storage::StorageHandle;

/// One entry in a picker's file-type filter list. A picker may show
/// several — e.g. "Modelica models", "All files".
///
/// Lives here, with the dialog, rather than on the `lunco-storage` I/O trait —
/// the file picker is a UI concern.
#[derive(Debug, Clone)]
pub struct OpenFilter {
    /// Human-readable group label ("Modelica models").
    pub name: String,
    /// Extensions without the leading dot ("mo", "mos").
    pub extensions: Vec<String>,
}

impl OpenFilter {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Hints for a save dialog: starting directory, default filename, and filter
/// list. All optional; the picker falls back to its own defaults when missing.
#[derive(Debug, Clone, Default)]
pub struct SaveHint {
    /// Default filename shown in the picker.
    pub suggested_name: Option<String>,
    /// Starting directory. For a previously-saved document this is usually the
    /// document's own origin folder so "Save As" opens next to the existing file.
    pub start_dir: Option<StorageHandle>,
    /// File type filters offered in the picker.
    pub filters: Vec<OpenFilter>,
}

/// Blocking "save as" dialog. Returns the chosen path as a
/// [`StorageHandle::File`], or `None` on cancel.
///
/// For UI panels that want a synchronous picker (the CSV/plot export flows in
/// the Modelica IDE) rather than the event-driven [`PickHandle`] command. Native
/// `rfd`; a no-op returning `None` on wasm (browsers have no blocking picker).
/// This is the home of the file dialog — `lunco-storage` (the I/O trait)
/// deliberately carries no `rfd`.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick_save_blocking(hint: &SaveHint) -> Option<StorageHandle> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(name) = &hint.suggested_name {
        dialog = dialog.set_file_name(name);
    }
    if let Some(StorageHandle::File(dir)) = &hint.start_dir {
        let start: std::path::PathBuf = if dir.is_dir() {
            dir.clone()
        } else {
            dir.parent().map(std::path::PathBuf::from).unwrap_or_default()
        };
        if !start.as_os_str().is_empty() {
            dialog = dialog.set_directory(&start);
        }
    }
    for f in &hint.filters {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        if !exts.is_empty() {
            dialog = dialog.add_filter(&f.name, &exts);
        }
    }
    dialog.save_file().map(StorageHandle::File)
}

/// wasm stub — browsers have no synchronous file picker.
#[cfg(target_arch = "wasm32")]
pub fn pick_save_blocking(_hint: &SaveHint) -> Option<StorageHandle> {
    None
}

/// What kind of system dialog to show.
#[derive(Clone, Debug)]
pub enum PickMode {
    /// "Open File" picker with a file-type filter.
    OpenFile(OpenFilter),
    /// "Save As" picker with a starting directory + suggested name.
    SaveFile(SaveHint),
    /// "Open Folder" picker (no filter).
    OpenFolder,
}

/// Which command to trigger once the picker resolves with a chosen
/// handle. A user cancellation produces no command — the in-flight
/// entity is despawned and nothing happens.
///
/// Closed enum (rather than a boxed `dyn Command`) because:
/// - every file-workflow path is enumerable; new follow-ups land here
///   intentionally rather than implicitly,
/// - no `Send`/lifetime gymnastics for type-erased commands,
/// - the variant set is reviewable on every diff.
#[derive(Clone, Debug)]
pub enum PickFollowUp {
    /// Resolve → trigger `OpenFile { path }`.
    OpenFile,
    /// Resolve → trigger `OpenFolder { path }` (folder may or may not
    /// contain a `twin.toml`; the routing observer classifies and
    /// dispatches Folder vs Twin accordingly).
    OpenFolder,
    /// Resolve → trigger `OpenTwin { path }` (strict: errors if the
    /// chosen folder lacks a `twin.toml`).
    OpenTwin,
    /// Resolve → trigger `AddFolderToWorkspace { path }` (VS Code-style
    /// multi-root: keeps existing folder Twins, adds this one).
    AddFolderToWorkspace,
    /// Resolve → trigger `AddTwin { path }` (strict variant of
    /// [`Self::AddFolderToWorkspace`]; requires a `twin.toml`).
    AddTwin,
    /// Resolve → trigger `SaveAsDocument { doc, path }` for the doc
    /// whose typed id is carried here.
    SaveAs(DocumentId),
    /// Resolve → trigger `SaveAsTwin { folder }` to promote the
    /// current session into a Twin at the chosen folder.
    SaveAsTwin,
}

/// Request to show a system file dialog.
///
/// Fired by panels, menu items, keybind resolvers, or HTTP callers.
/// Resolved asynchronously by a backend observer; on success the
/// observer fires [`PickResolved`] with the chosen handle.
#[derive(Event, Clone, Debug)]
pub struct PickHandle {
    /// Which dialog to show.
    pub mode: PickMode,
    /// What to do with the result.
    pub on_resolved: PickFollowUp,
}

/// Marker component on the transient entity that owns an in-flight
/// picker task.
///
/// Multiple pickers can coexist (rare, but cheap to allow — e.g. a
/// Save-As dialog opens while an Open-File is already showing). The
/// backend-specific task handle lives as a sibling component on the
/// same entity.
#[derive(Component)]
pub struct PickInFlight {
    /// What to dispatch on success.
    pub follow_up: PickFollowUp,
}

/// Fired when a picker resolves with a chosen handle.
///
/// A workbench-side dispatcher (added in a follow-up commit) observes
/// this and translates the [`PickFollowUp`] variant into the matching
/// typed command (`OpenFile { path }`, `SaveAsDocument { doc, path }`,
/// ...).
#[derive(Event, Clone, Debug)]
pub struct PickResolved {
    /// What the original requester wanted done with the result.
    pub follow_up: PickFollowUp,
    /// The chosen handle (always [`StorageHandle::File`] on native).
    pub handle: StorageHandle,
}

/// Fired when the user dismisses a picker without choosing anything.
///
/// Mostly observed for telemetry / status-bar messaging; the default
/// behaviour on cancellation is to do nothing, which is the silent
/// no-op users expect from "X out of a Save dialog".
#[derive(Event, Clone, Debug)]
pub struct PickCancelled {
    /// What would have been dispatched on success.
    pub follow_up: PickFollowUp,
}

// ─────────────────────────────────────────────────────────────────────────────
// Native backend — desktop OS dialogs via `rfd`
// ─────────────────────────────────────────────────────────────────────────────
//
// Lives behind `cfg(not(wasm32))` so the wasm target can ship its own
// dialog-via-File-System-Access observer in the same module without a
// trait or feature wrapper. Same `PickHandle` event in, same
// `PickResolved` / `PickCancelled` events out — call sites don't change.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use bevy::prelude::*;
    use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};

    use super::{
        PickCancelled, PickHandle, PickInFlight, PickMode, PickResolved, StorageHandle,
    };

    /// Component holding the in-flight `rfd` dialog future. Spawned on
    /// the same entity as [`PickInFlight`] by [`spawn_picker`]; consumed
    /// by [`drive_picker`].
    #[derive(Component)]
    pub(super) struct NativePickTask(pub Task<Option<StorageHandle>>);

    /// Observer: react to [`PickHandle`] by spawning a background task
    /// that opens the OS dialog. The dialog itself blocks the task's
    /// thread — fine, the task pool tolerates blocking work — but the
    /// UI thread never touches it.
    pub(super) fn spawn_picker(trigger: On<PickHandle>, mut commands: Commands) {
        let event = trigger.event().clone();
        let mode = event.mode;
        let task = AsyncComputeTaskPool::get().spawn(async move { run_dialog_blocking(&mode) });
        commands.spawn((
            PickInFlight {
                follow_up: event.on_resolved,
            },
            NativePickTask(task),
        ));
    }

    /// Per-frame system: poll every in-flight native picker. When one
    /// resolves, fire [`PickResolved`] / [`PickCancelled`] and despawn
    /// the carrier entity. Non-blocking — `poll_once` returns `None`
    /// immediately when the dialog is still up, matching the workspace's
    /// existing task-poll convention (see Modelica's Package Browser).
    pub(super) fn drive_picker(
        mut commands: Commands,
        mut q: Query<(Entity, &mut NativePickTask, &PickInFlight)>,
    ) {
        for (entity, mut task, in_flight) in q.iter_mut() {
            let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
                continue;
            };
            let follow_up = in_flight.follow_up.clone();
            commands.entity(entity).despawn();
            match result {
                Some(handle) => commands.trigger(PickResolved { follow_up, handle }),
                None => commands.trigger(PickCancelled { follow_up }),
            }
        }
    }

    /// Blocking dialog driver. Runs inside the spawned task, returns
    /// the chosen handle or `None` on cancellation. Always produces a
    /// [`StorageHandle::File`] today — the only backend `rfd` speaks.
    fn run_dialog_blocking(mode: &PickMode) -> Option<StorageHandle> {
        match mode {
            PickMode::OpenFile(filter) => {
                let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
                rfd::FileDialog::new()
                    .add_filter(&filter.name, &extensions)
                    .pick_file()
                    .map(StorageHandle::File)
            }
            PickMode::SaveFile(hint) => {
                let mut dialog = rfd::FileDialog::new();
                if let Some(name) = &hint.suggested_name {
                    dialog = dialog.set_file_name(name);
                }
                if let Some(StorageHandle::File(p)) = &hint.start_dir {
                    dialog = dialog.set_directory(p);
                }
                for f in &hint.filters {
                    let extensions: Vec<&str> = f.extensions.iter().map(String::as_str).collect();
                    dialog = dialog.add_filter(&f.name, &extensions);
                }
                dialog.save_file().map(StorageHandle::File)
            }
            PickMode::OpenFolder => rfd::FileDialog::new()
                .pick_folder()
                .map(StorageHandle::File),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Web backend — browser dialog via a hidden `<input type="file">`
// ─────────────────────────────────────────────────────────────────────────────
//
// `cfg(wasm32)` sibling of `native`: same `PickHandle` in, same
// `PickResolved` / `PickCancelled` out. A hidden `<input type="file">`
// is the most portable open path — it works on every browser, unlike
// the Chromium-only File-System-Access API. The chosen file's text is
// read browser-side and stashed in a per-name cache; domain `OpenFile`
// observers pull it back via [`take_picked_content`] instead of
// `std::fs`, which has no real filesystem to read on wasm. Save and
// folder pickers are not wired yet and resolve as a cancellation.

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use bevy::prelude::*;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{
        OpenFilter, PickCancelled, PickFollowUp, PickHandle, PickMode, PickResolved,
        StorageHandle,
    };

    thread_local! {
        /// Picks resolved on the JS event loop, awaiting conversion
        /// into Bevy events by [`drain_web_picks`] on the next tick.
        static PENDING: RefCell<Vec<Outcome>> = const { RefCell::new(Vec::new()) };
        /// Text of files chosen through the picker, keyed by file
        /// name. Drained by [`take_picked_content`].
        static PICKED_CONTENT: RefCell<HashMap<String, String>> =
            RefCell::new(HashMap::new());
    }

    enum Outcome {
        Resolved {
            follow_up: PickFollowUp,
            handle: StorageHandle,
        },
        Cancelled {
            follow_up: PickFollowUp,
        },
    }

    fn push(outcome: Outcome) {
        PENDING.with(|p| p.borrow_mut().push(outcome));
    }

    /// Pull a previously-picked file's text out of the stash, by file
    /// name. Returns `None` when the name was never picked (or was
    /// already taken — each entry is consumed once).
    pub(super) fn take_picked_content(name: &str) -> Option<String> {
        PICKED_CONTENT.with(|m| m.borrow_mut().remove(name))
    }

    /// Trigger a browser download of `content` under `file_name`.
    ///
    /// The browser owns where the bytes land (the Downloads folder, or
    /// a Save-As prompt depending on the user's settings) — there is no
    /// writable filesystem path to hand back, so this is a fire-and-
    /// forget alternative to `Storage::write` on wasm. Builds a `Blob`,
    /// wires it to a hidden `<a download>` anchor, and clicks it.
    pub(super) fn download_file(file_name: &str, content: &str) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("no document"))?;
        let body = document
            .body()
            .ok_or_else(|| JsValue::from_str("no document body"))?;

        let parts = js_sys::Array::new();
        parts.push(&JsValue::from_str(content));
        let blob = web_sys::Blob::new_with_str_sequence(&parts)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob)?;

        let anchor: web_sys::HtmlAnchorElement =
            document.create_element("a")?.dyn_into()?;
        anchor.set_href(&url);
        anchor.set_download(file_name);
        anchor.style().set_property("display", "none")?;
        body.append_child(&anchor)?;
        anchor.click();
        if let Some(parent) = anchor.parent_node() {
            let _ = parent.remove_child(&anchor);
        }
        // The object URL is intentionally not revoked: revoking it
        // immediately can race the browser's download kick-off. A few
        // bytes per save leak until the page unloads — negligible.
        Ok(())
    }

    /// Observer: react to [`PickHandle`] by raising a browser dialog.
    pub(super) fn spawn_picker(trigger: On<PickHandle>) {
        let event = trigger.event().clone();
        match event.mode {
            PickMode::OpenFile(filter) => {
                if let Err(e) = open_file_dialog(&filter, event.on_resolved.clone()) {
                    warn!("[picker] could not open web file dialog: {e:?}");
                    push(Outcome::Cancelled {
                        follow_up: event.on_resolved,
                    });
                }
            }
            PickMode::SaveFile(_) | PickMode::OpenFolder => {
                warn!(
                    "[picker] save / folder dialogs are not yet supported on \
                     wasm — request resolves as cancelled"
                );
                push(Outcome::Cancelled {
                    follow_up: event.on_resolved,
                });
            }
        }
    }

    /// Per-frame system: turn JS-resolved picks into Bevy events.
    pub(super) fn drain_web_picks(mut commands: Commands) {
        let drained: Vec<Outcome> = PENDING.with(|p| p.borrow_mut().drain(..).collect());
        for outcome in drained {
            match outcome {
                Outcome::Resolved { follow_up, handle } => {
                    commands.trigger(PickResolved { follow_up, handle });
                }
                Outcome::Cancelled { follow_up } => {
                    commands.trigger(PickCancelled { follow_up });
                }
            }
        }
    }

    /// Build a hidden `<input type="file">`, wire its `change` event,
    /// and click it to raise the browser's native open dialog.
    fn open_file_dialog(
        filter: &OpenFilter,
        follow_up: PickFollowUp,
    ) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("no document"))?;
        let body = document
            .body()
            .ok_or_else(|| JsValue::from_str("no document body"))?;

        let input: web_sys::HtmlInputElement =
            document.create_element("input")?.dyn_into()?;
        input.set_type("file");
        if !filter.extensions.is_empty() {
            let accept = filter
                .extensions
                .iter()
                .map(|e| format!(".{e}"))
                .collect::<Vec<_>>()
                .join(",");
            input.set_accept(&accept);
        }
        input.style().set_property("display", "none")?;
        body.append_child(&input)?;

        let input_for_change = input.clone();
        let change = Closure::<dyn FnMut()>::new(move || {
            on_input_change(&input_for_change, follow_up.clone());
        });
        input.set_onchange(Some(change.as_ref().unchecked_ref()));
        // The browser invokes the closure long after this frame — it
        // must outlive the stack. One small leak per dialog opened;
        // negligible for a user-driven action.
        change.forget();

        input.click();
        Ok(())
    }

    /// `change` handler: read the chosen file's text and resolve the
    /// pending pick. An empty `FileList` means the user cancelled.
    fn on_input_change(input: &web_sys::HtmlInputElement, follow_up: PickFollowUp) {
        let detach = |el: &web_sys::HtmlInputElement| {
            if let Some(parent) = el.parent_node() {
                let _ = parent.remove_child(el);
            }
        };
        let Some(file) = input.files().and_then(|list| list.get(0)) else {
            detach(input);
            push(Outcome::Cancelled { follow_up });
            return;
        };
        let name = file.name();
        detach(input);

        // `File::text()` (inherited from `Blob`) yields a promise of
        // the decoded UTF-8 contents — correct for `.mo` source.
        let text_promise = file.text();
        spawn_local(async move {
            match JsFuture::from(text_promise).await {
                Ok(value) => {
                    let content = value.as_string().unwrap_or_default();
                    PICKED_CONTENT
                        .with(|m| m.borrow_mut().insert(name.clone(), content));
                    push(Outcome::Resolved {
                        follow_up,
                        handle: StorageHandle::File(name.into()),
                    });
                }
                Err(e) => {
                    warn!("[picker] reading picked file failed: {e:?}");
                    push(Outcome::Cancelled { follow_up });
                }
            }
        });
    }
}

/// Retrieve the text of a file chosen through the wasm file picker,
/// keyed by file name. Domain `OpenFile` observers call this on wasm
/// in place of `std::fs` — the browser has no filesystem to read, but
/// the picker already pulled the bytes in. Each entry is consumed
/// once; a second call for the same name returns `None`.
#[cfg(target_arch = "wasm32")]
pub fn take_picked_content(name: &str) -> Option<String> {
    web::take_picked_content(name)
}

/// Save `content` by triggering a browser download named `file_name`.
///
/// The wasm counterpart to writing a file: there is no real filesystem
/// in the browser, so domain "Save" / "Save As" handlers call this on
/// wasm instead of `Storage::write`. The browser decides where the
/// bytes land. Logs and swallows DOM errors — a failed save shouldn't
/// panic the app.
#[cfg(target_arch = "wasm32")]
pub fn download_file(file_name: &str, content: &str) {
    if let Err(e) = web::download_file(file_name, content) {
        bevy::log::warn!("[picker] browser download of `{file_name}` failed: {e:?}");
    }
}

/// Plugin that wires up the picker backend appropriate for the target.
///
/// On native: registers the `rfd`-driven observer + poll system. On
/// wasm: registers the `<input type="file">` observer + drain system.
///
/// `WorkbenchPlugin` adds this automatically; standalone tests that
/// want picker behaviour without the full workbench shell can install
/// it directly.
pub struct PickerPlugin;

impl Plugin for PickerPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.add_observer(native::spawn_picker)
                .add_systems(Update, native::drive_picker);
        }
        #[cfg(target_arch = "wasm32")]
        {
            app.add_observer(web::spawn_picker)
                .add_systems(Update, web::drain_web_picks);
        }
    }
}
