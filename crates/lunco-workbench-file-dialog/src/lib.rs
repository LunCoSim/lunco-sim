//! Event-driven file picker.
//!
//! Storage is the wrong layer for dialog plumbing — it's the I/O
//! abstraction that loads and saves doc bytes. Picking a path is a UI
//! concern: a modal native dialog on desktop, a JS prompt on the web.
//! So the picker lives in this production capability package and just *uses*
//! `OpenFilter` / `SaveHint` / `StorageHandle` from `lunco-storage` to
//! describe the request and the result.
//!
//! ## Pattern
//!
//! Panels and commands admit requests with [`request_pick`]; a backend observer
//! (`rfd` on native, a browser file input and FileReader on web) resolves the dialog
//! asynchronously and emits [`PickResolved`] (success),
//! [`PickCancelled`] (the user dismissed the dialog) or
//! [`PickUnsupported`] (the backend cannot show this dialog at all —
//! never folded into a cancellation). A workbench-side dispatcher reads the resolved
//! event and triggers the matching typed file-workflow command
//! (`OpenFile { path }`, `SaveAsDocument { doc, path }`, ...).
//!
//! This package keeps UI code synchronous (no `async`, no polling), keeps
//! the backend swap a `cfg`-gated observer rather than a call-site
//! rewrite, and gives HTTP / scripting callers a uniform shape: trigger
//! the resolved follow-up command directly to skip the dialog, or
//! call [`request_pick`] to show one.
//!
//! Each request has an ECS carrier identity. Routing owners can pin their
//! intent in the originating command, validate it on [`PickStarted`], and
//! retire it through [`CancelPick`]. Browser
//! files carry their bytes in the result; their display name is never a
//! filesystem identity.

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
            dir.parent()
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Resolve → trigger `CreateTwin { path, name, default_scene }` for a new
    /// Twin folder.
    CreateTwin {
        /// Optional display name for the new Twin.
        name: String,
        /// Optional Twin-relative default USD scene.
        default_scene: String,
    },
}

/// Request to show a system file dialog.
///
/// Enqueued by [`request_pick`] after the caller's typed admission is installed.
/// Resolved asynchronously by a backend observer; on success the
/// observer fires [`PickResolved`] with the chosen handle.
#[derive(Event, Clone, Debug)]
pub struct PickHandle {
    /// Existing carrier allocated at the originating command's admission.
    pub request: Entity,
    /// Which dialog to show.
    pub mode: PickMode,
}

/// Admit a request with caller-owned typed facts before enqueueing its backend.
/// Application-wide opens pass `()`; source-dependent routing supplies its
/// immutable admission as a component bundle on this same carrier.
pub fn request_pick(
    commands: &mut Commands,
    mode: PickMode,
    follow_up: PickFollowUp,
    intent: impl Bundle,
) -> Entity {
    let request = commands.spawn((PickInFlight { follow_up }, intent)).id();
    commands.trigger(PickHandle { request, mode });
    request
}

/// Marker component on the transient entity that owns an in-flight
/// picker task.
///
/// Multiple pickers can coexist (rare, but cheap to allow — e.g. a
/// Save-As dialog opens while an Open-File is already showing). The
/// native task lives as a sibling component; browser resources are keyed by
/// the same exact entity in the App-owned main-thread resource.
#[derive(Component)]
pub struct PickInFlight {
    /// What to dispatch on success.
    pub follow_up: PickFollowUp,
}

/// The admitted request identity and intent, before its dialog can complete.
#[derive(Event, Clone, Debug)]
pub struct PickStarted {
    pub request: Entity,
    pub follow_up: PickFollowUp,
}

/// Retire a request. A native OS dialog may remain visible, but its result
/// cannot be dispatched after the request has been retired.
#[derive(Event, Clone, Debug)]
pub struct CancelPick {
    pub request: Entity,
}

/// A native storage path or an exact browser file payload. Browser names are
/// presentation only; callers must allocate a pathless document identity.
#[derive(Clone, Debug)]
pub enum PickedPath {
    Path(StorageHandle),
    BrowserFile {
        display_name: String,
        bytes: std::sync::Arc<[u8]>,
    },
}

/// Fired when a picker resolves with a chosen handle.
///
/// The workbench file-ops dispatcher observes
/// this and translates the [`PickFollowUp`] variant into the matching
/// typed command (`OpenFile { path }`, `SaveAsDocument { doc, path }`,
/// ...).
#[derive(Event, Clone, Debug)]
pub struct PickResolved {
    pub request: Entity,
    /// What the original requester wanted done with the result.
    pub follow_up: PickFollowUp,
    /// The selected path or exact bytes, owned by this request.
    pub result: PickedPath,
}

/// Fired when the user dismisses a picker without choosing anything.
///
/// Mostly observed for telemetry / status-bar messaging; the default
/// behaviour on cancellation is to do nothing, which is the silent
/// no-op users expect from "X out of a Save dialog".
#[derive(Event, Clone, Debug)]
pub struct PickCancelled {
    pub request: Entity,
    /// What would have been dispatched on success.
    pub follow_up: PickFollowUp,
}

/// A backend cannot start or finish a request: unsupported mode, retired
/// carrier, DOM failure, or read error. These are diagnosed separately from a
/// user cancellation. [`PickerPlugin`] always logs the exact request and cause.
#[derive(Event, Clone, Debug)]
pub struct PickUnsupported {
    pub request: Entity,
    /// Why the backend refused, in user-facing terms.
    pub reason: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Native backend — desktop OS dialogs via `rfd`
// ─────────────────────────────────────────────────────────────────────────────
//
// Lives behind `cfg(not(wasm32))` so the wasm target can ship its own
// browser input/FileReader observer in the same module without a
// trait or feature wrapper. Same `PickHandle` event in, same
// `PickResolved` / `PickCancelled` events out — call sites don't change.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use bevy::prelude::*;
    use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};

    use super::{
        PickCancelled, PickHandle, PickInFlight, PickMode, PickResolved, PickStarted,
        PickUnsupported, PickedPath, StorageHandle,
    };

    #[derive(Resource, Default)]
    pub(super) struct NativePickerState {
        retired: bool,
    }

    /// Component holding the in-flight `rfd` dialog future. Spawned on
    /// the same entity as [`PickInFlight`] by [`spawn_picker`]; consumed
    /// by [`drive_picker`].
    #[derive(Component)]
    pub(super) struct NativePickTask(pub Task<Option<StorageHandle>>);

    /// Observer: react to [`PickHandle`] by spawning a background task
    /// that opens the OS dialog. The dialog itself blocks the task's
    /// thread — fine, the task pool tolerates blocking work — but the
    /// UI thread never touches it.
    pub(super) fn spawn_picker(
        trigger: On<PickHandle>,
        requests: Query<&PickInFlight>,
        state: Res<NativePickerState>,
        mut commands: Commands,
    ) {
        let event = trigger.event().clone();
        let request = event.request;
        let Some(follow_up) = requests
            .get(request)
            .ok()
            .map(|entry| entry.follow_up.clone())
            .filter(|_| !state.retired)
        else {
            commands.trigger(PickUnsupported {
                request,
                reason: "native picker request or App has retired".into(),
            });
            commands.entity(request).try_despawn();
            return;
        };
        let mode = event.mode;
        let task = AsyncComputeTaskPool::get().spawn(async move { run_dialog_blocking(&mode) });
        commands.entity(request).insert(NativePickTask(task));
        commands.trigger(PickStarted { request, follow_up });
    }

    pub(super) fn retire_on_exit(
        mut exit: MessageReader<bevy::app::AppExit>,
        mut state: ResMut<NativePickerState>,
    ) {
        if exit.read().next().is_some() {
            state.retired = true;
        }
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
            match result {
                Some(handle) => commands.trigger(PickResolved {
                    request: entity,
                    follow_up,
                    result: PickedPath::Path(handle),
                }),
                None => commands.trigger(PickCancelled {
                    request: entity,
                    follow_up,
                }),
            }
            commands.entity(entity).try_despawn();
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
                .set_can_create_directories(true)
                .pick_folder()
                .map(StorageHandle::File),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Browser requests, DOM callbacks, byte results and download URLs share one
// App-owned resource. Async callbacks keep only weak references to it.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};

    use bevy::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    use super::{
        CancelPick, OpenFilter, PickCancelled, PickFollowUp, PickHandle, PickInFlight, PickMode,
        PickResolved, PickStarted, PickUnsupported, PickedPath,
    };

    /// Main-thread browser resources owned by one App, including pending reads
    /// and downloads. Dropping the App releases callbacks, nodes and blob URLs.
    #[derive(Default)]
    pub struct BrowserPicker {
        state: Rc<RefCell<State>>,
    }

    #[derive(Default)]
    struct State {
        retired: bool,
        requests: HashMap<Entity, Request>,
        pending: Vec<Outcome>,
        next_download: u64,
        downloads: HashMap<DownloadId, Download>,
    }

    struct Request {
        follow_up: PickFollowUp,
        dialog: Option<Dialog>,
        read: Option<FileRead>,
    }

    struct Dialog {
        input: web_sys::HtmlInputElement,
        _change: Closure<dyn FnMut()>,
        _cancel: Closure<dyn FnMut()>,
    }

    impl Drop for Dialog {
        fn drop(&mut self) {
            self.input.set_onchange(None);
            self.input.set_oncancel(None);
            self.input.remove();
        }
    }

    struct FileRead {
        reader: web_sys::FileReader,
        _load: Closure<dyn FnMut()>,
        _error: Closure<dyn FnMut()>,
        _abort: Closure<dyn FnMut()>,
    }

    impl Drop for FileRead {
        fn drop(&mut self) {
            self.reader.set_onload(None);
            self.reader.set_onerror(None);
            self.reader.set_onabort(None);
            if self.reader.ready_state() == web_sys::FileReader::LOADING {
                self.reader.abort();
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct DownloadId(u64);

    struct BlobUrl(String);

    impl Drop for BlobUrl {
        fn drop(&mut self) {
            if let Err(error) = web_sys::Url::revoke_object_url(&self.0) {
                warn!("[picker] could not revoke download URL: {error:?}");
            }
        }
    }

    struct Download {
        _url: BlobUrl,
        _callback: Closure<dyn FnMut()>,
        timer: Option<i32>,
    }

    impl Drop for Download {
        fn drop(&mut self) {
            if let (Some(window), Some(timer)) = (web_sys::window(), self.timer) {
                window.clear_timeout_with_handle(timer);
            }
        }
    }

    enum Outcome {
        Resolved(PickResolved),
        Cancelled(PickCancelled),
        Unsupported(PickUnsupported),
    }

    impl Outcome {
        fn request(&self) -> Entity {
            match self {
                Self::Resolved(event) => event.request,
                Self::Cancelled(event) => event.request,
                Self::Unsupported(event) => event.request,
            }
        }
    }

    impl State {
        fn finish(&mut self, request: Entity, result: Result<PickedPath, String>) {
            let Some(entry) = self.requests.remove(&request) else {
                return;
            };
            let follow_up = entry.follow_up;
            self.pending.push(match result {
                Ok(result) => Outcome::Resolved(PickResolved {
                    request,
                    follow_up,
                    result,
                }),
                Err(reason) => Outcome::Unsupported(PickUnsupported { request, reason }),
            });
        }

        fn cancel(&mut self, request: Entity) {
            let Some(entry) = self.requests.remove(&request) else {
                return;
            };
            self.pending.push(Outcome::Cancelled(PickCancelled {
                request,
                follow_up: entry.follow_up,
            }));
        }

        fn retire(&mut self, request: Entity) {
            self.requests.remove(&request);
            self.pending.retain(|outcome| outcome.request() != request);
        }

        fn shutdown(&mut self) {
            self.retired = true;
            self.requests.clear();
            self.pending.clear();
            self.downloads.clear();
        }
    }

    pub(super) fn spawn_picker(
        trigger: On<PickHandle>,
        picker: NonSend<BrowserPicker>,
        requests: Query<&PickInFlight>,
        mut commands: Commands,
    ) {
        let event = trigger.event().clone();
        let request = event.request;
        let Some(follow_up) = requests
            .get(request)
            .ok()
            .map(|entry| entry.follow_up.clone())
            .filter(|_| !picker.state.borrow().retired)
        else {
            commands.trigger(PickUnsupported {
                request,
                reason: "browser picker request or App has retired".into(),
            });
            commands.entity(request).try_despawn();
            return;
        };
        commands.trigger(PickStarted {
            request,
            follow_up: follow_up.clone(),
        });
        picker.state.borrow_mut().requests.insert(
            request,
            Request {
                follow_up,
                dialog: None,
                read: None,
            },
        );
        match event.mode {
            PickMode::OpenFile(filter) => {
                match attach_file_dialog(&picker.state, request, &filter) {
                    Ok(input) => input.click(),
                    Err(error) => picker.state.borrow_mut().finish(
                        request,
                        Err(format!("cannot open browser file dialog: {error:?}")),
                    ),
                }
            }
            PickMode::SaveFile(_) => picker.state.borrow_mut().finish(
                request,
                Err(
                    "the browser build has no Save-As dialog; use the download action instead"
                        .into(),
                ),
            ),
            PickMode::OpenFolder => picker.state.borrow_mut().finish(
                request,
                Err("the browser build cannot open a folder; open individual files instead".into()),
            ),
        }
    }

    pub(super) fn cancel_picker(trigger: On<CancelPick>, picker: NonSend<BrowserPicker>) {
        picker.state.borrow_mut().retire(trigger.event().request);
    }

    pub(super) fn retire_carrier(
        trigger: On<Remove, PickInFlight>,
        picker: NonSend<BrowserPicker>,
    ) {
        picker.state.borrow_mut().retire(trigger.entity);
    }

    pub(super) fn drain_web_picks(
        picker: NonSend<BrowserPicker>,
        requests: Query<(), With<PickInFlight>>,
        mut commands: Commands,
    ) {
        let pending = std::mem::take(&mut picker.state.borrow_mut().pending);
        for outcome in pending {
            let request = outcome.request();
            if requests.get(request).is_err() {
                continue;
            }
            match outcome {
                Outcome::Resolved(event) => commands.trigger(event),
                Outcome::Cancelled(event) => commands.trigger(event),
                Outcome::Unsupported(event) => commands.trigger(event),
            }
            commands.entity(request).try_despawn();
        }
    }

    pub(super) fn shutdown_on_exit(
        mut exit: MessageReader<bevy::app::AppExit>,
        picker: NonSend<BrowserPicker>,
    ) {
        if exit.read().next().is_some() {
            picker.state.borrow_mut().shutdown();
        }
    }

    fn attach_file_dialog(
        state: &Rc<RefCell<State>>,
        request: Entity,
        filter: &OpenFilter,
    ) -> Result<web_sys::HtmlInputElement, JsValue> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("no document"))?;
        let body = document
            .body()
            .ok_or_else(|| JsValue::from_str("no document body"))?;
        let input: web_sys::HtmlInputElement = document.create_element("input")?.dyn_into()?;
        input.set_type("file");
        if !filter.extensions.is_empty() {
            input.set_accept(
                &filter
                    .extensions
                    .iter()
                    .map(|ext| format!(".{ext}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        input.style().set_property("display", "none")?;
        let weak = Rc::downgrade(state);
        let input_for_change = input.clone();
        let change = Closure::<dyn FnMut()>::new(move || {
            on_input_change(&weak, request, &input_for_change);
        });
        let weak = Rc::downgrade(state);
        let cancel = Closure::<dyn FnMut()>::new(move || {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().cancel(request);
            }
        });
        input.set_onchange(Some(change.as_ref().unchecked_ref()));
        input.set_oncancel(Some(cancel.as_ref().unchecked_ref()));
        let dialog = Dialog {
            input: input.clone(),
            _change: change,
            _cancel: cancel,
        };
        body.append_child(&input)?;
        let mut state = state.borrow_mut();
        let Some(entry) = state.requests.get_mut(&request) else {
            return Err(JsValue::from_str("picker request retired"));
        };
        entry.dialog = Some(dialog);
        drop(state);
        Ok(input)
    }

    fn on_input_change(
        state: &Weak<RefCell<State>>,
        request: Entity,
        input: &web_sys::HtmlInputElement,
    ) {
        let Some(owner) = state.upgrade() else {
            return;
        };
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            owner.borrow_mut().cancel(request);
            return;
        };
        let mut owner = owner.borrow_mut();
        let Some(entry) = owner.requests.get_mut(&request) else {
            return;
        };
        if entry.dialog.take().is_none() {
            return;
        }
        drop(owner);
        if let Err(error) = start_file_read(state, request, &file) {
            if let Some(state) = state.upgrade() {
                state
                    .borrow_mut()
                    .finish(request, Err(format!("cannot read picked file: {error:?}")));
            }
        }
    }

    fn start_file_read(
        state: &Weak<RefCell<State>>,
        request: Entity,
        file: &web_sys::File,
    ) -> Result<(), JsValue> {
        let reader = web_sys::FileReader::new()?;
        let weak = state.clone();
        let reader_for_load = reader.clone();
        let display_name = file.name();
        let load = Closure::<dyn FnMut()>::new(move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            if !state.borrow().requests.contains_key(&request) {
                return;
            }
            let result = reader_for_load
                .result()
                .map_err(|error| format!("reading picked file failed: {error:?}"))
                .and_then(|value| {
                    value
                        .dyn_into::<js_sys::ArrayBuffer>()
                        .map_err(|_| "browser file read returned no byte buffer".to_owned())
                })
                .map(|buffer| PickedPath::BrowserFile {
                    display_name: display_name.clone(),
                    bytes: js_sys::Uint8Array::new(&buffer).to_vec().into(),
                });
            state.borrow_mut().finish(request, result);
        });
        let weak = state.clone();
        let reader_for_error = reader.clone();
        let error = Closure::<dyn FnMut()>::new(move || {
            if let Some(state) = weak.upgrade() {
                let reason = reader_for_error
                    .error()
                    .map(|error| format!("{}: {}", error.name(), error.message()))
                    .unwrap_or_else(|| {
                        "browser reported a file read error without details".to_owned()
                    });
                state.borrow_mut().finish(request, Err(reason));
            }
        });
        let weak = state.clone();
        let abort = Closure::<dyn FnMut()>::new(move || {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().cancel(request);
            }
        });
        reader.set_onload(Some(load.as_ref().unchecked_ref()));
        reader.set_onerror(Some(error.as_ref().unchecked_ref()));
        reader.set_onabort(Some(abort.as_ref().unchecked_ref()));
        let read = FileRead {
            reader: reader.clone(),
            _load: load,
            _error: error,
            _abort: abort,
        };
        let owner = state
            .upgrade()
            .ok_or_else(|| JsValue::from_str("picker App retired"))?;
        let mut owner = owner.borrow_mut();
        let entry = owner
            .requests
            .get_mut(&request)
            .ok_or_else(|| JsValue::from_str("picker request retired"))?;
        entry.read = Some(read);
        drop(owner);
        reader.read_as_array_buffer(file)?;
        Ok(())
    }

    fn retain_download(picker: &BrowserPicker, url: BlobUrl) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let mut state = picker.state.borrow_mut();
        let next = state
            .next_download
            .checked_add(1)
            .ok_or_else(|| JsValue::from_str("download request identity exhausted"))?;
        state.next_download = next;
        let id = DownloadId(next);
        drop(state);
        let weak = Rc::downgrade(&picker.state);
        let callback = Closure::<dyn FnMut()>::new(move || {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().downloads.remove(&id);
            }
        });
        let timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            0,
        )?;
        picker.state.borrow_mut().downloads.insert(
            id,
            Download {
                _url: url,
                _callback: callback,
                timer: Some(timer),
            },
        );
        Ok(())
    }

    /// Admit a download and retire its blob URL in the next browser task.
    /// Errors retire the URL immediately; App teardown also retires pending URLs.
    pub(super) fn download_file(
        picker: &BrowserPicker,
        file_name: &str,
        content: &str,
    ) -> Result<(), JsValue> {
        if picker.state.borrow().retired {
            return Err(JsValue::from_str("picker App has retired"));
        }
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
        let url = BlobUrl(web_sys::Url::create_object_url_with_blob(&blob)?);
        let anchor: web_sys::HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
        anchor.set_href(&url.0);
        anchor.set_download(file_name);
        anchor.style().set_property("display", "none")?;
        body.append_child(&anchor)?;
        anchor.click();
        anchor.remove();
        retain_download(picker, url)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use wasm_bindgen_futures::JsFuture;
        use wasm_bindgen_test::*;

        wasm_bindgen_test_configure!(run_in_browser);

        fn admit(picker: &BrowserPicker, request: Entity) {
            picker.state.borrow_mut().requests.insert(
                request,
                Request {
                    follow_up: PickFollowUp::OpenFile,
                    dialog: None,
                    read: None,
                },
            );
        }

        fn file(name: &str, bytes: &[u8]) -> web_sys::File {
            let parts = js_sys::Array::new();
            parts.push(&js_sys::Uint8Array::from(bytes));
            web_sys::File::new_with_u8_array_sequence(&parts, name).unwrap()
        }

        fn read(
            picker: &BrowserPicker,
            request: Entity,
            file: &web_sys::File,
        ) -> web_sys::FileReader {
            admit(picker, request);
            start_file_read(&Rc::downgrade(&picker.state), request, file).unwrap();
            picker.state.borrow().requests[&request]
                .read
                .as_ref()
                .unwrap()
                .reader
                .clone()
        }

        fn load_end(reader: &web_sys::FileReader) -> js_sys::Promise {
            js_sys::Promise::new(&mut |resolve, _| {
                reader.set_onloadend(Some(&resolve));
            })
        }

        fn blob_url() -> BlobUrl {
            let parts = js_sys::Array::new();
            parts.push(&JsValue::from_str("owned download bytes"));
            BlobUrl(
                web_sys::Url::create_object_url_with_blob(
                    &web_sys::Blob::new_with_str_sequence(&parts).unwrap(),
                )
                .unwrap(),
            )
        }

        async fn next_browser_task() {
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                web_sys::window()
                    .unwrap()
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
                    .unwrap();
            });
            JsFuture::from(promise).await.unwrap();
        }

        #[wasm_bindgen_test]
        fn picker_cancel_callback_and_app_drop_release_dom_handlers() {
            let mut world = World::new();
            let request = world.spawn_empty().id();
            let picker = BrowserPicker::default();
            admit(&picker, request);
            let input =
                attach_file_dialog(&picker.state, request, &OpenFilter::new("Source", &["mo"]))
                    .unwrap();
            assert!(input.parent_node().is_some());
            assert!(input.onchange().is_some());
            input
                .dispatch_event(&web_sys::Event::new("cancel").unwrap())
                .unwrap();
            assert!(input.onchange().is_none());
            assert!(input.oncancel().is_none());
            assert!(input.parent_node().is_none());
            assert!(
                matches!(&picker.state.borrow().pending[..], [Outcome::Cancelled(event)] if event.request == request)
            );
            input
                .dispatch_event(&web_sys::Event::new("change").unwrap())
                .unwrap();
            assert_eq!(picker.state.borrow().pending.len(), 1);
            let another = world.spawn_empty().id();
            admit(&picker, another);
            let input = attach_file_dialog(&picker.state, another, &OpenFilter::new("Source", &[]))
                .unwrap();
            let weak = Rc::downgrade(&picker.state);
            drop(picker);
            assert!(weak.upgrade().is_none());
            assert!(input.onchange().is_none());
            assert!(input.oncancel().is_none());
            assert!(input.parent_node().is_none());
        }

        #[wasm_bindgen_test(async)]
        async fn picker_file_reader_same_basename_preserves_each_request_payload() {
            let mut world = World::new();
            let first = world.spawn_empty().id();
            let second = world.spawn_empty().id();
            let picker = BrowserPicker::default();
            let reader_a = read(&picker, first, &file("same.mo", b"first source"));
            let end_a = load_end(&reader_a);
            let reader_b = read(&picker, second, &file("same.mo", b"second source"));
            let end_b = load_end(&reader_b);
            JsFuture::from(end_a).await.unwrap();
            JsFuture::from(end_b).await.unwrap();
            reader_a.set_onloadend(None);
            reader_b.set_onloadend(None);
            let state = picker.state.borrow();
            assert_eq!(state.pending.len(), 2);
            for (request, expected) in [
                (first, b"first source".as_slice()),
                (second, b"second source".as_slice()),
            ] {
                let event = state
                    .pending
                    .iter()
                    .find_map(|outcome| match outcome {
                        Outcome::Resolved(event) if event.request == request => Some(event),
                        _ => None,
                    })
                    .unwrap();
                let PickedPath::BrowserFile {
                    display_name,
                    bytes,
                } = &event.result
                else {
                    panic!("expected byte payload");
                };
                assert_eq!(display_name, "same.mo");
                assert_eq!(bytes.as_ref(), expected);
            }
            assert!(state.requests.is_empty());
            for reader in [reader_a, reader_b] {
                assert!(reader.onload().is_none());
                assert!(reader.onerror().is_none());
                assert!(reader.onabort().is_none());
            }
        }

        #[wasm_bindgen_test]
        fn picker_file_reader_error_and_retirement_release_handlers_and_abort() {
            let mut world = World::new();
            let request = world.spawn_empty().id();
            let picker = BrowserPicker::default();
            let reader = read(&picker, request, &file("source.mo", b"source"));
            reader
                .dispatch_event(&web_sys::Event::new("error").unwrap())
                .unwrap();
            assert!(
                matches!(&picker.state.borrow().pending[..], [Outcome::Unsupported(event)] if event.request == request && !event.reason.is_empty())
            );
            assert_eq!(reader.ready_state(), web_sys::FileReader::DONE);
            assert!(reader.onload().is_none());
            assert!(reader.onerror().is_none());
            assert!(reader.onabort().is_none());
            reader
                .dispatch_event(&web_sys::Event::new("load").unwrap())
                .unwrap();
            assert_eq!(picker.state.borrow().pending.len(), 1);
            let another = world.spawn_empty().id();
            let reader = read(&picker, another, &file("source.mo", b"pending"));
            picker.state.borrow_mut().retire(another);
            assert_eq!(reader.ready_state(), web_sys::FileReader::DONE);
            assert!(reader.onload().is_none());
            assert!(reader.onerror().is_none());
            assert!(reader.onabort().is_none());
            reader
                .dispatch_event(&web_sys::Event::new("load").unwrap())
                .unwrap();
            assert_eq!(picker.state.borrow().pending.len(), 1);
        }

        #[wasm_bindgen_test(async)]
        async fn picker_download_urls_retire_after_task_and_app_exit() {
            let mut app = App::new();
            app.add_plugins(super::super::PickerPlugin);
            let url = blob_url();
            let address = url.0.clone();
            JsFuture::from(web_sys::window().unwrap().fetch_with_str(&address))
                .await
                .unwrap();
            retain_download(app.world().non_send::<BrowserPicker>(), url).unwrap();
            next_browser_task().await;
            assert!(
                app.world()
                    .non_send::<BrowserPicker>()
                    .state
                    .borrow()
                    .downloads
                    .is_empty()
            );
            assert!(
                JsFuture::from(web_sys::window().unwrap().fetch_with_str(&address))
                    .await
                    .is_err()
            );

            let request = app
                .world_mut()
                .spawn(PickInFlight {
                    follow_up: PickFollowUp::OpenFile,
                })
                .id();
            let (reader, url_address) = {
                let picker = app.world().non_send::<BrowserPicker>();
                let reader = read(picker, request, &file("source.mo", b"pending read"));
                picker
                    .state
                    .borrow_mut()
                    .pending
                    .push(Outcome::Cancelled(PickCancelled {
                        request,
                        follow_up: PickFollowUp::OpenFile,
                    }));
                let url = blob_url();
                let address = url.0.clone();
                retain_download(picker, url).unwrap();
                (reader, address)
            };
            app.world_mut()
                .resource_mut::<Messages<bevy::app::AppExit>>()
                .write(bevy::app::AppExit::Success);
            app.world_mut().run_schedule(Last);
            let state = app.world().non_send::<BrowserPicker>().state.borrow();
            assert!(state.requests.is_empty());
            assert!(state.pending.is_empty());
            assert!(state.downloads.is_empty());
            assert!(state.retired);
            drop(state);
            assert_eq!(reader.ready_state(), web_sys::FileReader::DONE);
            assert!(reader.onload().is_none());
            assert!(reader.onerror().is_none());
            assert!(reader.onabort().is_none());
            assert!(
                JsFuture::from(web_sys::window().unwrap().fetch_with_str(&url_address))
                    .await
                    .is_err()
            );
            assert!(
                download_file(
                    app.world().non_send::<BrowserPicker>(),
                    "source.mo",
                    "source"
                )
                .is_err()
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::BrowserPicker;

/// Start a browser download in the supplied App-owned picker. Successful
/// admission means the browser received the download; final disk delivery is
/// controlled by the browser. A failed admission must not mark a document saved.
#[cfg(target_arch = "wasm32")]
pub fn download_file(picker: &BrowserPicker, file_name: &str, content: &str) -> Result<(), String> {
    web::download_file(picker, file_name, content)
        .map_err(|error| format!("browser download of `{file_name}` failed: {error:?}"))
}

/// Plugin that wires up the picker backend appropriate for the target.
///
/// On native: registers the `rfd`-driven observer + poll system. On
/// wasm: registers the `<input type="file">` observer + drain system.
///
/// `WorkbenchPlugin` adds this automatically for hosts that compose the
/// standard shell; hosts that need only the dialog capability can install it
/// directly.
pub struct PickerPlugin;

/// Backstop observer for [`PickUnsupported`], installed on every
/// target. Guarantees a refused dialog leaves a trace even when no
/// domain observer is listening — the failure mode this replaced was a
/// request that produced literally nothing.
fn log_unsupported_pick(trigger: On<PickUnsupported>) {
    let e = trigger.event();
    error!(
        "[picker] {:?} could not complete in this backend: {}",
        e.request, e.reason
    );
}

fn cancel_pick(trigger: On<CancelPick>, requests: Query<&PickInFlight>, mut commands: Commands) {
    let request = trigger.event().request;
    if let Ok(entry) = requests.get(request) {
        commands.trigger(PickCancelled {
            request,
            follow_up: entry.follow_up.clone(),
        });
        commands.entity(request).try_despawn();
    }
}

fn retire_picks_on_exit(
    mut exit: MessageReader<bevy::app::AppExit>,
    requests: Query<Entity, With<PickInFlight>>,
    mut commands: Commands,
) {
    if exit.read().next().is_some() {
        for request in &requests {
            commands.entity(request).try_despawn();
        }
    }
}

impl Plugin for PickerPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<bevy::app::AppExit>()
            .add_observer(log_unsupported_pick)
            .add_observer(cancel_pick)
            .add_systems(Last, retire_picks_on_exit);
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<native::NativePickerState>()
                .add_observer(native::spawn_picker)
                .add_systems(Update, native::drive_picker)
                .add_systems(Last, native::retire_on_exit);
        }
        #[cfg(target_arch = "wasm32")]
        {
            app.init_non_send::<BrowserPicker>()
                .add_observer(web::spawn_picker)
                .add_observer(web::cancel_picker)
                .add_observer(web::retire_carrier)
                .add_systems(Update, web::drain_web_picks)
                .add_systems(Last, web::shutdown_on_exit);
        }
    }
}
