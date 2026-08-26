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
//! [`AddTwin`]. The generic [`lunco_doc_bevy::OpenFile`] command is defined by
//! the document layer; the USD adapter resolves scene paths and reaches this
//! module through [`spawn_twin_scan`].

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_core::{
    on_command, register_commands, Command, Severity, TelemetryEvent, TelemetryValue,
};
use lunco_storage::{StorageEntryKind, StorageError};
use lunco_twin::{TwinError, TwinManifest, TwinMode, UsdManifest};

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
/// The workbench's `StatusBusPlugin` forwards error/critical events to the
/// status bar when that UI is present; headless hosts still receive the same
/// event on the shared telemetry bus.
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

fn inspect_path(path: &std::path::Path) -> Result<StorageEntryKind, StorageError> {
    lunco_storage::entry_kind_file_sync(path)
}

fn storage_io(error: StorageError) -> std::io::Error {
    match error {
        StorageError::Io(error) => error,
        other => std::io::Error::other(other.to_string()),
    }
}

/// Write a new Twin manifest at `path` without indexing the folder.
///
/// The write is deliberately separate from [`create_twin`]: command callers
/// can persist the tiny manifest synchronously and hand the potentially large
/// folder scan to [`spawn_twin_from_path`], keeping the UI responsive. The
/// manifest is the commit point; an existing manifest is never overwritten.
fn write_new_twin_manifest(
    path: &std::path::Path,
    name: &str,
    default_scene: &str,
) -> Result<(), TwinError> {
    if path.as_os_str().is_empty() {
        return Err(TwinError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Twin path must not be empty",
            ),
        });
    }
    let manifest_path = path.join(lunco_twin::MANIFEST_FILENAME);
    match inspect_path(&manifest_path) {
        Ok(StorageEntryKind::File) | Ok(StorageEntryKind::Directory) => {
            return Err(TwinError::AlreadyExists(path.to_path_buf()));
        }
        Err(StorageError::NotFound) => {}
        Err(error) => {
            return Err(TwinError::Io {
                path: manifest_path,
                source: storage_io(error),
            });
        }
    }
    match inspect_path(path) {
        Ok(StorageEntryKind::Directory) | Err(StorageError::NotFound) => {}
        Ok(StorageEntryKind::File) => {
            return Err(TwinError::NotAFileOrFolder(path.to_path_buf()));
        }
        Err(error) => {
            return Err(TwinError::Io {
                path: path.to_path_buf(),
                source: storage_io(error),
            });
        }
    }
    let fallback = path
        .file_name()
        .and_then(|part| part.to_str())
        .filter(|part| !part.trim().is_empty())
        .unwrap_or("Untitled Twin");
    let display_name = if name.trim().is_empty() {
        fallback
    } else {
        name.trim()
    };
    let mut manifest = TwinManifest::new(display_name);
    if !default_scene.trim().is_empty() {
        manifest.usd = Some(UsdManifest {
            default_scene: Some(default_scene.trim().to_string()),
            scenes: None,
        });
    }
    manifest.write(&manifest_path)
}

/// Create a new on-disk Twin and return its freshly indexed representation.
///
/// This pure helper is used by tests and headless callers. The command-side
/// observer uses the manifest-only half above before starting the asynchronous
/// workspace scan.
pub fn create_twin(path: &std::path::Path, name: &str) -> Result<TwinMode, TwinError> {
    write_new_twin_manifest(path, name, "")?;
    TwinMode::open(path)
}

/// Create a new Twin folder and asynchronously add it to the workspace.
/// Empty `path` means "ask the windowed workbench for a folder".
#[Command(default)]
pub struct CreateTwin {
    /// Target Twin folder. The manifest is created here; missing ancestors are
    /// created by the storage-backed manifest writer.
    pub path: String,
    /// Human-readable name. Empty uses the target folder name.
    pub name: String,
    /// Optional Twin-relative USD stage opened when the Twin is admitted.
    pub default_scene: String,
}

#[on_command(CreateTwin)]
fn on_create_twin(
    trigger: On<CreateTwin>,
    mut pending: ResMut<PendingTwinOpens>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if event.path.is_empty() {
        return;
    }
    let path = std::path::PathBuf::from(&event.path);
    match write_new_twin_manifest(&path, &event.name, &event.default_scene) {
        Ok(()) => spawn_twin_from_path(&path, &mut pending, "CreateTwin", TwinOpenMode::Replace),
        Err(error) => {
            let detail = format!("CreateTwin failed at `{}`: {error}", path.display());
            warn!("{detail}");
            commands.trigger(twin_open_failed(detail));
        }
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
    if !matches!(inspect_path(&manifest), Ok(StorageEntryKind::File)) {
        let detail = format!(
            "OpenTwin failed: `{path}` has no {} — use OpenFolder for plain folders",
            lunco_twin::MANIFEST_FILENAME
        );
        warn!("{detail}");
        commands.trigger(twin_open_failed(detail));
        return;
    }
    spawn_twin_from_path(folder, &mut pending, "OpenTwin", TwinOpenMode::Replace);
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
    if matches!(
        inspect_path(&folder.join(lunco_twin::MANIFEST_FILENAME)),
        Ok(StorageEntryKind::File)
    ) {
        info!(
            "[OpenFolder] {} contains {} — routing to OpenTwin",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        commands.trigger(OpenTwin { path });
        return;
    }
    // VS Code semantics: "Open Folder" *replaces* the current workspace
    // folders. The replacement is committed only after the off-thread scan
    // succeeds; callers that want to keep existing roots and add another fire
    // `AddFolderToWorkspace` instead.
    spawn_twin_from_path(folder, &mut pending, "OpenFolder", TwinOpenMode::Replace);
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
    if matches!(
        inspect_path(&folder.join(lunco_twin::MANIFEST_FILENAME)),
        Ok(StorageEntryKind::File)
    ) {
        info!(
            "[AddFolderToWorkspace] {} contains {} — routing to AddTwin",
            path,
            lunco_twin::MANIFEST_FILENAME
        );
        commands.trigger(AddTwin { path });
        return;
    }
    spawn_twin_from_path(
        folder,
        &mut pending,
        "AddFolderToWorkspace",
        TwinOpenMode::Add,
    );
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
fn on_add_twin(
    trigger: On<AddTwin>,
    mut pending: ResMut<PendingTwinOpens>,
    mut commands: Commands,
) {
    let path = trigger.event().path.clone();
    if path.is_empty() {
        return; // windowed hosts answer this with a picker
    }
    let folder = std::path::Path::new(&path);
    if !matches!(
        inspect_path(&folder.join(lunco_twin::MANIFEST_FILENAME)),
        Ok(StorageEntryKind::File)
    ) {
        let detail = format!(
            "AddTwin failed: `{path}` has no {} — use AddFolderToWorkspace for plain folders",
            lunco_twin::MANIFEST_FILENAME
        );
        warn!("{detail}");
        commands.trigger(twin_open_failed(detail));
        return;
    }
    spawn_twin_from_path(folder, &mut pending, "AddTwin", TwinOpenMode::Add);
}

/// How a completed asynchronous folder scan is admitted to the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwinOpenMode {
    /// Replace every currently open Twin after the candidate has been scanned.
    Replace,
    /// Add the candidate without closing existing Twins.
    Add,
}

/// Close every open folder/Twin, firing [`TwinClosed`] for each.
///
/// Shared by the replace-semantics openers ([`OpenTwin`], [`OpenFolder`], and
/// USD's `OpenFile`-on-a-scene adapter), so "replacing the workspace" means
/// the same thing everywhere.
fn close_all_open_folders(
    workspace: &mut WorkspaceResource,
    commands: &mut Commands,
    log_tag: &str,
) {
    let ids: Vec<crate::TwinId> = workspace.twins().map(|(id, _)| id).collect();
    for id in ids {
        let Some(root) = workspace.twin(id).map(|twin| twin.root.clone()) else {
            continue;
        };
        let was_active = workspace.active_twin == Some(id);
        workspace.close_twin(id);
        commands.trigger(TwinClosed {
            twin: id,
            root,
            was_active,
        });
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
    /// Cancel scans superseded by a newer replacement request. Dropping the
    /// task handles prevents their results from being observed; the candidate
    /// that completes owns the next workspace commit.
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
    /// Replacement is committed only after this task returns a valid Twin.
    mode: TwinOpenMode,
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
    mode: TwinOpenMode,
) {
    spawn_twin_scan(folder, pending, log_tag, None, mode);
}

/// As [`spawn_twin_from_path`], but selects `scene` (root-relative) once the
/// scan lands — the "opening a scene file IS opening its root" path.
pub fn spawn_twin_scan(
    folder: &std::path::Path,
    pending: &mut PendingTwinOpens,
    log_tag: &str,
    scene: Option<String>,
    mode: TwinOpenMode,
) {
    if mode == TwinOpenMode::Replace {
        pending.cancel_all();
    }
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
        mode,
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
                if entry.mode == TwinOpenMode::Replace {
                    close_all_open_folders(&mut workspace, &mut commands, &entry.log_tag);
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
    on_create_twin,
    on_open_twin,
    on_open_folder,
    on_add_folder_to_workspace,
    on_add_twin,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_twin_writes_manifest_and_can_be_reopened() {
        let tmp = tempfile::tempdir().expect("parent directory");
        let root = tmp.path().join("created-twin");

        let mode = create_twin(&root, "Created Twin").expect("create Twin");
        let TwinMode::Twin(twin) = mode else {
            panic!("created Twin must open in Twin mode");
        };
        assert_eq!(twin.manifest.as_ref().unwrap().name, "Created Twin");
        assert!(root.join(lunco_twin::MANIFEST_FILENAME).is_file());

        let reopened = TwinMode::open(&root).expect("reopen created Twin");
        assert!(matches!(reopened, TwinMode::Twin(_)));
    }

    #[test]
    fn create_twin_does_not_overwrite_existing_manifest() {
        let tmp = tempfile::tempdir().expect("parent directory");
        let root = tmp.path().join("existing-twin");
        std::fs::create_dir_all(&root).expect("Twin directory");
        std::fs::write(
            root.join(lunco_twin::MANIFEST_FILENAME),
            "name = \"original\"\nversion = \"0.1.0\"\n",
        )
        .expect("existing manifest");

        let error = create_twin(&root, "replacement").expect_err("existing Twin must reject");
        assert!(error.to_string().contains("already contains"));
        assert!(
            std::fs::read_to_string(root.join(lunco_twin::MANIFEST_FILENAME))
                .unwrap()
                .contains("original")
        );
    }

    #[test]
    fn create_twin_command_adds_the_new_root_to_workspace() {
        let tmp = tempfile::tempdir().expect("parent directory");
        let root = tmp.path().join("command-twin");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::session::WorkspacePlugin);
        app.update();

        app.world_mut().trigger(CreateTwin {
            path: root.display().to_string(),
            name: "Command Twin".into(),
            default_scene: String::new(),
        });
        for _ in 0..60 {
            app.update();
            if app.world().resource::<WorkspaceResource>().twins().count() == 1 {
                break;
            }
        }

        let workspace = app.world().resource::<WorkspaceResource>();
        let twin = workspace
            .twins()
            .next()
            .expect("created Twin is registered");
        assert_eq!(twin.1.root, root);
        assert_eq!(twin.1.manifest.as_ref().unwrap().name, "Command Twin");
    }

    #[test]
    fn failed_replacement_scan_keeps_the_active_twin() {
        let good = tempfile::tempdir().expect("good Twin directory");
        std::fs::write(
            good.path().join(lunco_twin::MANIFEST_FILENAME),
            "name = \"good\"\nversion = \"0.1.0\"\n",
        )
        .expect("good manifest");
        let bad = tempfile::tempdir().expect("bad Twin directory");
        std::fs::write(
            bad.path().join(lunco_twin::MANIFEST_FILENAME),
            "name = [not valid toml",
        )
        .expect("bad manifest");

        let existing = match TwinMode::open(good.path()).expect("good Twin opens") {
            TwinMode::Twin(twin) => twin,
            other => panic!("expected Twin, got {other:?}"),
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<WorkspaceResource>()
            .init_resource::<PendingTwinOpens>()
            .add_systems(Update, drain_pending_twin_opens);
        app.world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(existing);

        let good_root = good.path().to_path_buf();
        {
            let mut pending = app.world_mut().resource_mut::<PendingTwinOpens>();
            spawn_twin_from_path(bad.path(), &mut pending, "test", TwinOpenMode::Replace);
        }
        for _ in 0..60 {
            app.update();
            if app.world().resource::<PendingTwinOpens>().tasks.is_empty() {
                break;
            }
        }

        let workspace = app.world().resource::<WorkspaceResource>();
        assert_eq!(workspace.twins().count(), 1);
        assert_eq!(
            workspace.twins().next().map(|(_, twin)| &twin.root),
            Some(&good_root)
        );
    }
}
