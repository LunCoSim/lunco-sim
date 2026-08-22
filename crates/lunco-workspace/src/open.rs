//! Opening a Twin — the command and the scan pipeline behind it.
//!
//! WHY HERE AND NOT IN THE WORKBENCH. Opening a Twin is a WORKSPACE operation:
//! it walks a folder, adds the result to [`WorkspaceResource`] and fires
//! [`TwinAdded`]. None of that needs a window, and a headless host — which never
//! adds the workbench — must be able to mount the twin whose scenarios it runs.
//!
//! WHAT THE WORKBENCH KEEPS. Exactly one thing: choosing a folder when the
//! caller names none. An empty `path` means "ask the human", which is the
//! workbench's job and nobody else's — it keeps picker-only observers for that
//! case, and the picker fires these same commands back with a resolved path. The
//! open pipeline is not duplicated; there is one implementation and one seam.
//!
//! The same split covers [`OpenFolder`], [`AddFolderToWorkspace`] and
//! [`AddTwin`]. `OpenFile` stays in the workbench: it resolves scene paths and
//! opens documents, and reaches this module through [`spawn_twin_scan`].

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_core::{
    on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue,
};
use lunco_twin::{TwinError, TwinMode};

use crate::session::{TwinAdded, TwinClosed, WorkspaceResource};

/// Telemetry event name published when an Open (Twin / Folder / Add) that the
/// user actually asked for produces no workspace root.
///
/// Published at [`Severity::Error`] on the shared telemetry bus, so it reaches
/// every consumer: the black-box
/// log, the API telemetry stream (any subscriber, incl. an automated client
/// watching an Open it requested), and any host observer that cares. The scan
/// runs off-thread, so by the time it fails the click is long gone — without
/// this the user is left staring at an unchanged workspace with the reason only
/// in a terminal they are not watching.
///
/// NOTE (workspace-wide gap, not this module's to close): several crates
/// document `Severity::Error` telemetry as "the workbench status bar surfaces
/// it", but no observer in the workspace actually bridges `TelemetryEvent` to
/// `lunco_workbench::status_bus::StatusBus`. Publishing here is still the right
/// and only correct move — it is the one bus every host can see — but the
/// status-bar fan-out has to be built once, centrally, for all of them.
///
/// The payload is a human-readable string naming WHICH command, WHICH path, and
/// WHY — that string is what a status bar / diagnostics row shows verbatim.
pub const TWIN_OPEN_FAILED: &str = "TWIN_OPEN_FAILED";

/// The one shape every [`TWIN_OPEN_FAILED`] publication takes, so the payload
/// convention cannot drift between the failure arms.
fn twin_open_failed(detail: impl Into<String>) -> TelemetryEvent {
    TelemetryEvent {
        name: TWIN_OPEN_FAILED.into(),
        source: 0,
        severity: Severity::Error,
        data: TelemetryValue::String(detail.into()),
        timestamp: 0.0,
    }
}

/// Open a Twin folder — strict: the folder must contain a `twin.toml`.
///
/// VS Code semantics: this **replaces** the currently open folders. Use
/// [`AddTwin`] to keep them.
///
/// Empty `path` means "ask the user", which only a windowed host can honour —
/// see the module docs.
#[Command(default)]
pub struct OpenTwin {
    /// Filesystem path of the Twin root (must contain `twin.toml`).
    /// Empty asks a windowed host to show a folder picker.
    pub path: String,
}

#[on_command(OpenTwin)]
fn on_open_twin(
    trigger: On<OpenTwin>,
    mut workspace: ResMut<WorkspaceResource>,
    mut pending: ResMut<PendingTwinOpens>,
    mut commands: Commands,
) {
    let path = trigger.event().path.clone();
    if path.is_empty() {
        // "Ask the human" — a windowed host answers this with a picker; a
        // headless one has nobody to ask.
        return;
    }
    let folder = std::path::Path::new(&path);
    let manifest = folder.join(lunco_twin::MANIFEST_FILENAME);
    if !manifest.is_file() {
        warn!(
            "[OpenTwin] {} has no {} — refusing (use OpenFolder for plain folders)",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        return;
    }
    close_all_open_folders(&mut workspace, &mut pending, &mut commands, "OpenTwin");
    spawn_twin_from_path(folder, &mut pending, "OpenTwin");
}

/// Open a folder as the workspace root — a Twin if it has a `twin.toml`,
/// otherwise a plain folder Twin (a first-class mode, no manifest required).
///
/// VS Code semantics: this **replaces** the current workspace folders. Use
/// [`AddFolderToWorkspace`] to keep them.
///
/// Unlike [`OpenTwin`], an empty `path` is an ERROR rather than a picker
/// request — a windowed host dispatches `ShowOpenFolderPicker` for that.
#[Command(default)]
pub struct OpenFolder {
    /// Filesystem path of the folder to open.
    pub path: String,
}

#[on_command(OpenFolder)]
fn on_open_folder(
    trigger: On<OpenFolder>,
    mut workspace: ResMut<WorkspaceResource>,
    mut pending: ResMut<PendingTwinOpens>,
    mut commands: Commands,
) {
    let path = trigger.event().path.clone();
    if path.is_empty() {
        warn!(
            "[OpenFolder] fired with empty path — ignoring (use ShowOpenFolderPicker for dialog)"
        );
        return;
    }
    let folder = std::path::Path::new(&path);
    if folder.join(lunco_twin::MANIFEST_FILENAME).is_file() {
        info!(
            "[OpenFolder] {} contains {} — routing to OpenTwin",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        commands.trigger(OpenTwin { path });
        return;
    }
    // VS Code semantics: "Open Folder" *replaces* the current workspace
    // folders. Callers that want to keep existing roots and add another fire
    // `AddFolderToWorkspace` instead.
    close_all_open_folders(&mut workspace, &mut pending, &mut commands, "OpenFolder");
    spawn_twin_from_path(folder, &mut pending, "OpenFolder");
}

/// Add a folder to the workspace **without** closing the open ones —
/// VS Code's "Add Folder to Workspace…". A folder with a `twin.toml` routes to
/// [`AddTwin`].
///
/// Empty `path` asks a windowed host for a picker (see the module docs).
#[Command(default)]
pub struct AddFolderToWorkspace {
    /// Filesystem path of the folder to add. Empty asks for a picker.
    pub path: String,
}

#[on_command(AddFolderToWorkspace)]
fn on_add_folder_to_workspace(
    trigger: On<AddFolderToWorkspace>,
    mut pending: ResMut<PendingTwinOpens>,
    mut commands: Commands,
) {
    let path = trigger.event().path.clone();
    if path.is_empty() {
        return; // windowed hosts answer this with a picker
    }
    let folder = std::path::Path::new(&path);
    if folder.join(lunco_twin::MANIFEST_FILENAME).is_file() {
        info!(
            "[AddFolderToWorkspace] {} contains {} — routing to AddTwin",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        commands.trigger(AddTwin { path });
        return;
    }
    spawn_twin_from_path(folder, &mut pending, "AddFolderToWorkspace");
}

/// Strict variant of [`AddFolderToWorkspace`] — requires a `twin.toml`.
///
/// Empty `path` asks a windowed host for a picker (see the module docs).
#[Command(default)]
pub struct AddTwin {
    /// Filesystem path of the Twin root (must contain `twin.toml`).
    /// Empty asks for a picker.
    pub path: String,
}

#[on_command(AddTwin)]
fn on_add_twin(trigger: On<AddTwin>, mut pending: ResMut<PendingTwinOpens>) {
    let path = trigger.event().path.clone();
    if path.is_empty() {
        return; // windowed hosts answer this with a picker
    }
    let folder = std::path::Path::new(&path);
    if !folder.join(lunco_twin::MANIFEST_FILENAME).is_file() {
        warn!(
            "[AddTwin] {} has no {} — refusing (use AddFolderToWorkspace for plain folders)",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        return;
    }
    spawn_twin_from_path(folder, &mut pending, "AddTwin");
}

/// Close every open folder/Twin, firing [`TwinClosed`] for each.
///
/// Shared by the replace-semantics openers ([`OpenTwin`], [`OpenFolder`], and
/// the workbench's `OpenFile`-on-a-scene), so "replacing the workspace" means
/// the same thing everywhere.
pub fn close_all_open_folders(
    workspace: &mut WorkspaceResource,
    pending: &mut PendingTwinOpens,
    commands: &mut Commands,
    log_tag: &str,
) {
    // Replace semantics invalidate an older folder scan before the new one is
    // admitted. A stale scan must never add the Twin that the user had already
    // replaced while its filesystem walk was still running.
    pending.cancel_all();
    let ids: Vec<crate::TwinId> = workspace.twins().map(|(id, _)| id).collect();
    for id in ids {
        let Some(root) = workspace.twin(id).map(|twin| twin.root.clone()) else {
            continue;
        };
        workspace.close_twin(id);
        commands.trigger(TwinClosed { twin: id, root });
        info!("[{log_tag}] closed pre-existing Twin {:?}", id);
    }
}

/// In-flight folder scans. [`TwinMode::open`] walks the filesystem
/// synchronously — large trees (~/.cargo, node_modules, …) easily take seconds
/// to enumerate, and running that on the UI thread freezes the window long
/// enough for the Wayland/X11 compositor to drop the client. Each open
/// dispatches its scan to [`AsyncComputeTaskPool`] and parks the handle here;
/// [`drain_pending_twin_opens`] polls one frame at a time and registers the Twin
/// once the walker finishes.
#[derive(Resource, Default)]
pub struct PendingTwinOpens {
    tasks: Vec<TwinOpenTask>,
}

impl PendingTwinOpens {
    /// Cancel scans that belong to a replaced workspace-open request. Dropping
    /// the task handles prevents their results from being observed; the
    /// replacement command owns the next scan exclusively.
    fn cancel_all(&mut self) {
        self.tasks.clear();
    }
}

struct TwinOpenTask {
    task: Task<Result<TwinMode, TwinError>>,
    path: std::path::PathBuf,
    log_tag: String,
    /// Scene to select once the scan lands, relative to the scanned folder.
    /// `Some` when the caller opened a *scene file* rather than a folder.
    scene: Option<String>,
}

/// Shared helper for Open Folder / Open Twin / Add Folder / Add Twin.
///
/// Spawns the scan asynchronously and parks the handle in [`PendingTwinOpens`].
/// The actual `add_twin` + [`TwinAdded`] firing happens in
/// [`drain_pending_twin_opens`] once the walker returns.
pub fn spawn_twin_from_path(
    folder: &std::path::Path,
    pending: &mut PendingTwinOpens,
    log_tag: &str,
) {
    spawn_twin_scan(folder, pending, log_tag, None);
}

/// As [`spawn_twin_from_path`], but selects `scene` (root-relative) once the
/// scan lands — the "opening a scene file IS opening its root" path.
pub fn spawn_twin_scan(
    folder: &std::path::Path,
    pending: &mut PendingTwinOpens,
    log_tag: &str,
    scene: Option<String>,
) {
    let path = folder.to_path_buf();
    let scan_path = path.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move { TwinMode::open(&scan_path) });
    match &scene {
        Some(rel) => info!(
            "[{log_tag}] scanning {} for `{rel}` (off-thread)…",
            path.display()
        ),
        None => info!("[{log_tag}] scanning {} (off-thread)…", path.display()),
    }
    pending.tasks.push(TwinOpenTask {
        task,
        path,
        log_tag: log_tag.to_string(),
        scene,
    });
}

/// Poll each in-flight folder scan. Ready scans add their Twin to the Workspace
/// and fire [`TwinAdded`]; in-flight ones are kept for the next frame.
pub fn drain_pending_twin_opens(
    mut pending: ResMut<PendingTwinOpens>,
    mut workspace: ResMut<WorkspaceResource>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;
    if pending.tasks.is_empty() {
        return;
    }
    let mut still_running = Vec::with_capacity(pending.tasks.len());
    for mut entry in pending.tasks.drain(..) {
        match future::block_on(future::poll_once(&mut entry.task)) {
            None => still_running.push(entry),
            Some(Ok(TwinMode::Twin(mut twin))) | Some(Ok(TwinMode::Folder(mut twin))) => {
                // Opened by scene file → select it, so the doc-first mount that
                // `TwinAdded` kicks off loads that scene rather than whatever
                // `twin.toml` happened to name as default.
                if let Some(rel) = &entry.scene {
                    twin.set_default_scene(rel.clone());
                }
                let twin_id = workspace.add_twin(twin);
                commands.trigger(TwinAdded { twin: twin_id });
                match &entry.scene {
                    Some(rel) => info!(
                        "[{}] opened {} @ `{rel}`",
                        entry.log_tag,
                        entry.path.display()
                    ),
                    None => info!("[{}] opened {}", entry.log_tag, entry.path.display()),
                }
            }
            Some(Ok(TwinMode::Orphan(_))) => {
                warn!(
                    "[{}] {} resolved to Orphan unexpectedly — ignoring",
                    entry.log_tag,
                    entry.path.display()
                );
                commands.trigger(twin_open_failed(format!(
                    "{} failed: cannot open {} — the folder scan produced no workspace root",
                    entry.log_tag,
                    entry.path.display()
                )));
            }
            Some(Err(e)) => {
                warn!(
                    "[{}] failed to index {}: {e}",
                    entry.log_tag,
                    entry.path.display()
                );
                commands.trigger(twin_open_failed(format!(
                    "{} failed: cannot open {} — {e}",
                    entry.log_tag,
                    entry.path.display()
                )));
            }
        }
    }
    pending.tasks = still_running;
}

/// Wire `OpenTwin` + the scan pipeline. Added by
/// [`WorkspacePlugin`](crate::session::WorkspacePlugin), so every host that has
/// a workspace can open a Twin — window or no window.
pub(crate) fn build(app: &mut App) {
    app.init_resource::<PendingTwinOpens>()
        .add_systems(Update, drain_pending_twin_opens);
    register_all_commands(app);
}

register_commands!(
    on_open_twin,
    on_open_folder,
    on_add_folder_to_workspace,
    on_add_twin,
);
