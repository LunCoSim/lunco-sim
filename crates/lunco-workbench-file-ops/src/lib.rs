//! Shell-level file-workflow commands.
//!
//! Shell-level file workflow lives here so all windowed apps get the same
//! picker, menu, keybind, and HTTP command shape. The generic `OpenFile`
//! command is defined by `lunco-doc-bevy`; folder/Twin scanning is owned by
//! `lunco-workspace`; and document bytes are read by the owning domain through
//! `lunco-storage`. Domain-specific commands (`SaveDocument`,
//! `SaveAsDocument`, `CloseDocument`) stay in `lunco-doc-bevy`; their
//! observers continue to live in domain crates because writing a Modelica
//! `.mo` and writing a USD `.usda` differ in details.
//!
//! ## Pattern
//!
//! Every externally callable verb is a reflected typed command per `AGENTS.md` § 4.2 — UI
//! clicks, menu items, keybinds, HTTP API calls, MCP tools, and AI
//! agents dispatch the same shape. Empty-string path fields fire the
//! native file dialog via [`lunco_workbench_file_dialog::request_pick`]; non-empty paths skip the
//! dialog (recents, drag-drop, automation).
//!
//! ## What this module ships
//!
//! - Shell-only commands such as [`SaveAll`], [`SaveAsTwin`], and the picker
//!   requests.
//! - The picker-resolution router ([`on_pick_resolved`]) that turns a
//!   [`lunco_workbench_file_dialog::PickResolved`] event into the matching typed verb with the
//!   chosen path filled in.
//! - **Picker seams** for `OpenTwin`, `OpenFolder`, `AddTwin` and
//!   `AddFolderToWorkspace`. Those verbs and the folder-scan pipeline behind
//!   them live in [`lunco_workspace::open`] — opening a folder needs no window,
//!   so it must not sit behind one. What remains here is the part that does: an
//!   empty `path` means "ask the human", so these observers show the picker and
//!   ignore everything else. One implementation, one seam.
//! - [`FileOpsPlugin`] which registers the above.
//!
//! ## What's deferred
//!
//! - **[`OpenFile`]** is defined by `lunco-doc-bevy`; its empty-path picker
//!   entry is wired here, while extension-specific observers own loading.
//!   USD also owns scene-root resolution and the doc-first scene mount, which
//!   prevents the shell from mounting the same file through a second path.
//! - [`SaveAll`] dispatches domain-owned save commands; [`SaveAsTwin`]
//!   delegates manifest creation to the workspace-owned [`CreateTwin`]
//!   command and serialization to document owners.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc_bevy::{SaveAsDocument, rename::RenameOpenDocument};
use lunco_twin::{DocumentKindId, DocumentKindRegistry};

use lunco_workbench_file_dialog::{
    CancelPick, PickFollowUp, PickInFlight, PickMode, PickResolved, PickStarted, PickedPath,
    request_pick,
};
#[cfg(not(target_arch = "wasm32"))]
use lunco_workspace::FileRenamed;
use lunco_workspace::open::{
    AddFolderToWorkspace, AddTwin, CreateTwin, OpenFolder, OpenTwin, PendingTwinOpens,
    drain_pending_twin_opens,
};
use lunco_workspace::{WorkspaceResource, rename::RenameTwinEntry};

/// Request a system "Open File" dialog.
///
/// Dispatches [`ShowOpenFilePicker`] which triggers the picker via
/// [`lunco_workbench_file_dialog::request_pick`]. On success, the file dialog resolves to
/// [`OpenFile`] with the chosen path.
#[Command(default)]
pub struct ShowOpenFilePicker {}

/// Request a system "Open Folder" dialog.
///
/// Dispatches [`ShowOpenFolderPicker`] which triggers the picker via
/// [`lunco_workbench_file_dialog::request_pick`]. On success, the file dialog resolves to
/// [`OpenFolder`] with the chosen path.
#[Command(default)]
pub struct ShowOpenFolderPicker {}

// `NewDocument` and `OpenFile` are document-lifecycle verbs, not UI: their
// types live in `lunco-doc-bevy` so headless / luncosim / server binaries can
// dispatch them by `kind` / `path` without pulling the workbench shell. This
// module only installs the workbench-specific default-kind and picker
// adapters.
use lunco_doc_bevy::{NewDocument, OpenFile};

/// Produce a shareable link for the active document and copy it to the
/// clipboard.
///
/// Like [`OpenFile`], this is a typed shell command whose behaviour is
/// domain-specific and lives in the domain crate
/// (`lunco-modelica-core` encodes the active model's source into a URL
/// fragment). The headless HTTP API exposes the read-only `GetShareLink`
/// query separately; it returns the URL in its `data` payload instead of
/// touching a clipboard.
#[Command(default)]
pub struct CopyShareLink {}

/// Save documents in the exact admitted active source scope.
///
/// Documents with a writable canonical path are written via their
/// owning domain's [`SaveDocument`](lunco_doc_bevy::SaveDocument)
/// observer. Untitled documents in a local active Twin are written under that
/// root using their workspace title. Other admitted drafts use their domain's
/// Save-As action. Retired and different-scope documents are excluded.
#[Command(default)]
pub struct SaveAll {}

/// Promote the current session into a Twin at `folder`.
///
/// Writes `twin.toml`, saves documents in the admitted active source scope, and
/// declares the first open USD document as the default scene. Empty
/// `folder` triggers a folder picker.
#[Command(default)]
pub struct SaveAsTwin {
    /// Target folder for the new Twin's `twin.toml`. Empty triggers
    /// the picker.
    pub folder: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Save coordination
// ─────────────────────────────────────────────────────────────────────────────

#[on_command(NewDocument)]
fn on_new_document(
    trigger: On<NewDocument>,
    registry: Res<DocumentKindRegistry>,
    mut commands: Commands,
) {
    // Domain-specific creation is handled by domain crates' own
    // observers, gated on `cmd.kind == "<their_id>"`. This observer
    // exists only to resolve the "default" sentinel (empty `kind`)
    // into a real registered id and re-fire — which is what Ctrl+N
    // dispatches when no specific kind was chosen.
    let kind = trigger.event().kind.clone();
    if !kind.is_empty() {
        return;
    }
    // Use the same deterministic order as File → New so the default
    // command and the visible menu always target the same kind.
    let default_kind: Option<DocumentKindId> = registry
        .creatable()
        .into_iter()
        .next()
        .map(|(id, _)| id.clone());
    let Some(id) = default_kind else {
        warn!("[NewDocument] no document kinds registered with can_create_new=true");
        return;
    };
    commands.trigger(NewDocument {
        kind: id.as_str().to_string(),
    });
}

#[on_command(ShowOpenFilePicker)]
fn on_show_open_file_picker(
    _trigger: On<ShowOpenFilePicker>,
    registry: Res<DocumentKindRegistry>,
    mut commands: Commands,
) {
    use lunco_workbench_file_dialog::PickMode;
    // Collect all unique extensions from every registered kind to
    // build a unified "Supported files" filter.
    let mut extensions: Vec<String> = Vec::new();
    for (_, meta) in registry.iter() {
        for ext in &meta.extensions {
            let ext_str = ext.to_string();
            if !extensions.contains(&ext_str) {
                extensions.push(ext_str);
            }
        }
    }

    if extensions.is_empty() {
        warn!("[OpenFilePicker] no document kinds are registered");
        return;
    }

    let ext_refs: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
    request_pick(
        &mut commands,
        PickMode::OpenFile(lunco_workbench_file_dialog::OpenFilter::new(
            "Supported files",
            &ext_refs,
        )),
        PickFollowUp::OpenFile,
        (),
    );
}

#[on_command(ShowOpenFolderPicker)]
fn on_show_open_folder_picker(_trigger: On<ShowOpenFolderPicker>, mut commands: Commands) {
    use lunco_workbench_file_dialog::PickMode;
    request_pick(
        &mut commands,
        PickMode::OpenFolder,
        PickFollowUp::OpenFolder,
        (),
    );
}

/// Empty path means "ask the windowed workbench for a folder". A non-empty
/// path is handled by the workspace-owned creation observer.
#[on_command(CreateTwin)]
fn on_create_twin_pick(trigger: On<CreateTwin>, mut commands: Commands) {
    let event = trigger.event();
    if event.path.is_empty() {
        let name = event.name.clone();
        let default_scene = event.default_scene.clone();
        request_pick(
            &mut commands,
            PickMode::OpenFolder,
            PickFollowUp::CreateTwin {
                name,
                default_scene,
            },
            (),
        );
    }
}

/// The ONE thing about opening a Twin that needs a window: choosing which one.
///
/// The open pipeline itself lives in `lunco_workspace::open` — it walks a
/// folder and adds the result to the workspace, which needs no UI, and putting
/// it here made `OpenTwin` unreachable on any headless host. What is left is the
/// empty-`path` case, meaning "ask the user": show the folder picker, which
/// fires this same command back with a resolved path and lands in the workspace
/// observer. A non-empty path is not this crate's business and is ignored here.
#[on_command(OpenTwin)]
fn on_open_twin_pick(trigger: On<OpenTwin>, mut commands: Commands) {
    use lunco_workbench_file_dialog::PickMode;
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open::on_open_twin`
    }
    request_pick(
        &mut commands,
        PickMode::OpenFolder,
        PickFollowUp::OpenTwin,
        (),
    );
}

/// Picker seam for [`AddFolderToWorkspace`] — see [`on_open_twin_pick`].
#[on_command(AddFolderToWorkspace)]
fn on_add_folder_to_workspace_pick(trigger: On<AddFolderToWorkspace>, mut commands: Commands) {
    use lunco_workbench_file_dialog::PickMode;
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open`
    }
    request_pick(
        &mut commands,
        PickMode::OpenFolder,
        PickFollowUp::AddFolderToWorkspace,
        (),
    );
}

/// Picker seam for [`AddTwin`] — see [`on_open_twin_pick`].
#[on_command(AddTwin)]
fn on_add_twin_pick(trigger: On<AddTwin>, mut commands: Commands) {
    use lunco_workbench_file_dialog::PickMode;
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open`
    }
    request_pick(
        &mut commands,
        PickMode::OpenFolder,
        PickFollowUp::AddTwin,
        (),
    );
}

#[on_command(RenameOpenDocument)]
fn on_rename_open_document(
    trigger: On<RenameOpenDocument>,
    workspace: Res<WorkspaceResource>,
    mut commands: Commands,
) {
    use lunco_doc::DocumentOrigin;
    let ev = trigger.event();
    let new_name = ev.new_name.clone();
    if new_name.is_empty() {
        warn!("[RenameOpenDocument] empty new_name");
        return;
    }
    let Some(entry) = workspace.document(ev.doc_id) else {
        warn!(
            "[RenameOpenDocument] no Workspace doc with id {}",
            ev.doc_id
        );
        return;
    };
    match &entry.origin {
        DocumentOrigin::File {
            path,
            writable: true,
        } => {
            // Saved file: route through RenameTwinEntry if the path
            // lies under an open Twin. Standalone-file renames (no
            // owning Twin) aren't supported yet — would need a
            // path-only rename path that bypasses Twin::reload.
            let twin_root = workspace.twins().find_map(|(_, t)| {
                if path.starts_with(&t.root) {
                    Some(t.root.clone())
                } else {
                    None
                }
            });
            let Some(root) = twin_root else {
                warn!(
                    "[RenameOpenDocument] doc {} path {} not under any open \
                     Twin — standalone file rename not yet supported",
                    ev.doc_id,
                    path.display()
                );
                return;
            };
            let rel = match path.strip_prefix(&root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => return,
            };
            let (Some(root), Some(relative_path)) = (root.to_str(), rel.to_str()) else {
                reject_pick(
                    &mut commands,
                    "rename source path cannot be represented by the document command transport"
                        .into(),
                );
                return;
            };
            commands.trigger(RenameTwinEntry {
                twin_root: root.to_owned(),
                relative_path: relative_path.to_owned(),
                new_name,
            });
        }
        DocumentOrigin::Untitled { .. } => {
            // Domain plugins observe RenameOpenDocument directly for
            // Untitled docs (Modelica → RenameModelicaClass). The
            // workbench observer doesn't touch them.
        }
        DocumentOrigin::File {
            writable: false, ..
        }
        | DocumentOrigin::Bundled { .. } => {
            warn!("[RenameOpenDocument] doc {} is read-only", ev.doc_id);
        }
    }
}

fn rejected_file_command(commands: &mut Commands, message: String) -> Result<Ack, Reject> {
    reject_pick(commands, message.clone());
    Err(Reject::InvalidOp(message))
}

/// Resolve the directory actually renamed through storage, while leaving the
/// final entry undereferenced: renaming a symlink moves the link itself.
#[cfg(not(target_arch = "wasm32"))]
fn confined_rename_source(
    root: &std::path::Path,
    relative: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    if relative.as_os_str().is_empty() || !lunco_assets_path::is_safe_relative_components(relative)
    {
        return Err(format!(
            "rename source must be an ordinary relative path: {}",
            relative.display()
        ));
    }
    let candidate = root.join(relative);
    let parent = candidate
        .parent()
        .ok_or_else(|| "rename source has no parent directory".to_owned())?;
    let parent = lunco_storage::canonicalize_file_path(parent)
        .map_err(|error| format!("cannot resolve rename source parent: {error}"))?;
    if !parent.starts_with(root) {
        return Err("rename source parent resolves outside its admitted Twin root".into());
    }
    let name = relative
        .file_name()
        .ok_or_else(|| "rename source has no filename".to_owned())?;
    Ok(parent.join(name))
}

#[cfg(not(target_arch = "wasm32"))]
fn unoccupied_rename_destination(
    source: &std::path::Path,
    name: &str,
) -> Result<std::path::PathBuf, String> {
    lunco_assets_path::validate_portable_file_name(name)
        .map_err(|error| format!("rename target {name:?} is invalid: {error}"))?;
    let parent = source
        .parent()
        .ok_or_else(|| "resolved rename source has no parent".to_owned())?;
    let destination = parent.join(name);
    if destination != source {
        match lunco_storage::entry_kind_no_follow_file_sync(&destination) {
            Ok(_) => {
                return Err(format!(
                    "rename target already exists: {}",
                    destination.display()
                ));
            }
            Err(lunco_storage::StorageError::NotFound) => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect rename target {}: {error}",
                    destination.display()
                ));
            }
        }
    }
    Ok(destination)
}

#[on_command(RenameTwinEntry)]
fn on_rename_twin_entry(
    trigger: On<RenameTwinEntry>,
    #[cfg(not(target_arch = "wasm32"))] mut workspace: ResMut<WorkspaceResource>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = trigger;
        rejected_file_command(
            &mut commands,
            "filesystem Twin entry rename is unavailable in this browser host".into(),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use lunco_doc::DocumentOrigin;
        let ev = trigger.event();
        let twin_root = std::path::PathBuf::from(&ev.twin_root);
        let new_name = &ev.new_name;
        let twin_id = workspace
            .twins()
            .find(|(_, twin)| twin.root == twin_root)
            .map(|(id, _)| id);
        let Some(twin_id) = twin_id else {
            return rejected_file_command(
                &mut commands,
                format!("no open Twin matches rename root {}", twin_root.display()),
            );
        };
        let old_abs =
            match confined_rename_source(&twin_root, std::path::Path::new(&ev.relative_path)) {
                Ok(source) => source,
                Err(message) => return rejected_file_command(&mut commands, message),
            };
        let old_kind = match lunco_storage::entry_kind_no_follow_file_sync(&old_abs) {
            Ok(kind) => kind,
            Err(error) => {
                return rejected_file_command(
                    &mut commands,
                    format!(
                        "cannot inspect rename source {}: {error}",
                        old_abs.display()
                    ),
                );
            }
        };
        let new_abs = match unoccupied_rename_destination(&old_abs, new_name) {
            Ok(destination) => destination,
            Err(message) => return rejected_file_command(&mut commands, message),
        };
        if new_abs == old_abs {
            return Ok(Ack::new(OpId::new()));
        }
        let is_dir = matches!(old_kind, lunco_storage::StorageEntryKind::Directory);
        if let Err(error) = lunco_storage::rename_file_sync(&old_abs, &new_abs) {
            return rejected_file_command(
                &mut commands,
                format!(
                    "rename {} to {} failed: {error}",
                    old_abs.display(),
                    new_abs.display()
                ),
            );
        }

        // Re-scan the Twin so its `files()` reflects disk.
        if let Some(twin) = workspace.twin_mut(twin_id) {
            if let Err(e) = twin.reload() {
                warn!(
                    "[RenameTwinEntry] Twin::reload after rename failed: {e} \
                 (twin index may be stale until next OpenFolder)"
                );
            }
        }

        // Patch open documents whose canonical path lay under the old path
        // so live edits stay attached to disk.
        for doc in workspace.documents_mut() {
            if let DocumentOrigin::File { path, writable } = &doc.origin {
                if path.starts_with(&old_abs) {
                    let suffix = path
                        .strip_prefix(&old_abs)
                        .expect("starts_with implies strip_prefix succeeds");
                    let new_path = if suffix.as_os_str().is_empty() {
                        new_abs.clone()
                    } else {
                        new_abs.join(suffix)
                    };
                    let writable = *writable;
                    doc.origin = DocumentOrigin::File {
                        path: new_path,
                        writable,
                    };
                }
            }
        }

        info!(
            "[RenameTwinEntry] {} -> {}",
            old_abs.display(),
            new_abs.display()
        );
        commands.trigger(FileRenamed {
            twin: twin_id,
            old_abs,
            new_abs,
            is_dir,
        });
        Ok(Ack::new(OpId::new()))
    }
}

#[on_command(SaveAll)]
fn on_save_all(
    _trigger: On<SaveAll>,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    let Some(workspace) = workspace else {
        return rejected_file_command(&mut commands, "Save All has no workspace".into());
    };
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    if replica.is_some() && replication.is_none() {
        return rejected_file_command(
            &mut commands,
            "Save All has no live owner for its replicated scene".into(),
        );
    }
    let entries = match PromotionIntent::capture(&workspace, replication.as_ref())
        .and_then(|intent| intent.documents(&workspace, replication.as_ref()))
    {
        Ok(entries) => entries,
        Err(reason) => {
            return rejected_file_command(&mut commands, reason);
        }
    };
    let active_root = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .map(|twin| twin.root.clone());
    let mut used = std::collections::HashSet::new();
    let mut saves = Vec::with_capacity(entries.len());
    for entry in entries {
        let destination = if entry.origin.is_untitled() {
            if let Some(root) = &active_root {
                let path = match promoted_document_path(root, &entry, &mut used) {
                    Ok(path) => path,
                    Err(message) => return rejected_file_command(&mut commands, message),
                };
                let Some(path) = path.to_str() else {
                    return rejected_file_command(&mut commands, "Save All destination cannot be represented by the document command transport".into());
                };
                Some(path.to_owned())
            } else {
                None
            }
        } else {
            None
        };
        saves.push((entry.id, destination));
    }
    for (doc_id, path) in saves {
        if let Some(path) = path {
            commands.trigger(SaveAsDocument { doc_id, path });
        } else {
            commands.trigger(lunco_doc_bevy::SaveDocument { doc_id });
        }
    }
    Ok(Ack::new(OpId::new()))
}

#[on_command(SaveAsTwin)]
fn on_save_as_twin(
    trigger: On<SaveAsTwin>,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    use lunco_workbench_file_dialog::PickMode;
    let folder = trigger.event().folder.clone();
    let Some(workspace) = workspace else {
        return rejected_file_command(&mut commands, "no workspace is installed".into());
    };
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    if replica.is_some() && replication.is_none() {
        return rejected_file_command(
            &mut commands,
            "Save As Twin has no live owner for its replicated scene".into(),
        );
    }
    let intent = match PromotionIntent::capture(&workspace, replication.as_ref()) {
        Ok(intent) => intent,
        Err(reason) => {
            return rejected_file_command(&mut commands, reason);
        }
    };
    if folder.is_empty() {
        request_pick(
            &mut commands,
            PickMode::OpenFolder,
            PickFollowUp::SaveAsTwin,
            intent,
        );
        return Ok(Ack::new(OpId::new()));
    }
    let entries = match intent.documents(&workspace, replication.as_ref()) {
        Ok(entries) => entries,
        Err(reason) => {
            return rejected_file_command(&mut commands, reason);
        }
    };
    promote_documents(folder, entries, &mut commands)
}

fn promote_documents(
    folder: String,
    entries: Vec<lunco_workspace::DocumentEntry>,
    commands: &mut Commands,
) -> Result<Ack, Reject> {
    let root = std::path::PathBuf::from(&folder);
    let manifest_path = root.join(lunco_twin::MANIFEST_FILENAME);
    match lunco_storage::entry_kind_no_follow_file_sync(&manifest_path) {
        Ok(_) => {
            return rejected_file_command(
                commands,
                format!(
                    "{} already contains {}",
                    root.display(),
                    lunco_twin::MANIFEST_FILENAME
                ),
            );
        }
        Err(lunco_storage::StorageError::NotFound) => {}
        Err(error) => {
            return rejected_file_command(
                commands,
                format!(
                    "cannot inspect manifest {}: {error}",
                    manifest_path.display()
                ),
            );
        }
    }
    match lunco_storage::entry_kind_file_sync(&root) {
        Ok(lunco_storage::StorageEntryKind::File)
        | Ok(lunco_storage::StorageEntryKind::Symlink) => {
            return rejected_file_command(commands, format!("{} is not a folder", root.display()));
        }
        Ok(lunco_storage::StorageEntryKind::Directory)
        | Err(lunco_storage::StorageError::NotFound) => {}
        Err(error) => {
            return rejected_file_command(
                commands,
                format!("cannot inspect {}: {error}", root.display()),
            );
        }
    }
    let mut used = std::collections::HashSet::new();
    let mut default_scene = String::new();
    let mut saves = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = match promoted_document_path(&root, &entry, &mut used) {
            Ok(path) => path,
            Err(message) => return rejected_file_command(commands, message),
        };
        if default_scene.is_empty() && entry.kind.as_str() == "usd" {
            let relative = match path.strip_prefix(&root) {
                Ok(relative) => relative,
                Err(reason) => {
                    return rejected_file_command(
                        commands,
                        format!("promoted scene is outside its Twin root: {reason}"),
                    );
                }
            };
            let Some(relative) = relative.to_str() else {
                return rejected_file_command(
                    commands,
                    "promoted scene name cannot be represented in the Twin manifest".into(),
                );
            };
            default_scene = relative.to_owned();
        }
        let Some(path) = path.to_str() else {
            return rejected_file_command(
                commands,
                "promoted file destination cannot be represented by the document command transport"
                    .into(),
            );
        };
        saves.push((entry.id, path.to_owned()));
    }

    // CreateTwin is the sole manifest-writing owner. Save-As commands can run
    // in the same command flush: the storage writer creates the target folder
    // before the asynchronous workspace scan admits it.
    commands.trigger(CreateTwin {
        path: folder,
        name: String::new(),
        default_scene,
    });
    for (doc, path) in saves {
        commands.trigger(SaveAsDocument { doc_id: doc, path });
    }
    Ok(Ack::new(OpId::new()))
}

/// Choose a safe, stable filename for an open document being promoted into a
/// new Twin. Document domains still own serialization; the workbench only
/// supplies a collision-free destination based on their workspace title.
fn promoted_document_path(
    root: &std::path::Path,
    entry: &lunco_workspace::DocumentEntry,
    used: &mut std::collections::HashSet<String>,
) -> Result<std::path::PathBuf, String> {
    let raw = std::path::Path::new(&entry.title)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .trim();
    let mut base: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() || base == "." || base == ".." {
        base = format!("document-{}", entry.id.raw());
    }
    if !base.contains('.') {
        base.push_str(match entry.kind.as_str() {
            "usd" => ".usda",
            "modelica" => ".mo",
            _ => ".txt",
        });
    }
    lunco_assets_path::validate_portable_file_name(&base)
        .map_err(|error| format!("promoted filename {base:?} is invalid: {error}"))?;
    let original = base.clone();
    let mut suffix = 2;
    while used.contains(&base.to_ascii_lowercase()) {
        let path = std::path::Path::new(&original);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&original);
        let extension = path.extension().and_then(|s| s.to_str());
        base = match extension {
            Some(ext) => format!("{stem}-{suffix}.{ext}"),
            None => format!("{stem}-{suffix}"),
        };
        suffix += 1;
    }
    lunco_assets_path::validate_portable_file_name(&base)
        .map_err(|error| format!("promoted filename {base:?} is invalid: {error}"))?;
    used.insert(base.to_ascii_lowercase());
    Ok(root.join(base))
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn promoted_document_names_are_safe_and_unique() {
        let root = std::path::Path::new("/tmp/new-twin");
        let mut used = std::collections::HashSet::new();
        let first = lunco_workspace::DocumentEntry {
            id: lunco_doc::DocumentId::new(1),
            kind: lunco_workspace::DocumentKindId::new("modelica"),
            origin: lunco_doc::DocumentOrigin::untitled("scratch"),
            runtime_context: lunco_workspace::DocumentRuntimeOwner::Application,
            title: "Engine Model".into(),
            dirty: true,
        };
        let second = lunco_workspace::DocumentEntry {
            id: lunco_doc::DocumentId::new(2),
            title: first.title.clone(),
            ..first.clone()
        };
        assert_eq!(
            promoted_document_path(root, &first, &mut used).unwrap(),
            root.join("Engine_Model.mo")
        );
        assert_eq!(
            promoted_document_path(root, &second, &mut used).unwrap(),
            root.join("Engine_Model-2.mo")
        );
    }

    #[test]
    fn promoted_document_names_reject_invalid_final_candidates_without_rewriting() {
        let root = std::path::Path::new("output");
        let mut used = std::collections::HashSet::new();
        for title in ["CON", "NUL.mo", "COM1.mo", "LPT9.txt", "name."] {
            let entry = lunco_workspace::DocumentEntry {
                id: lunco_doc::DocumentId::new(1),
                kind: lunco_workspace::DocumentKindId::new("modelica"),
                origin: lunco_doc::DocumentOrigin::untitled(title),
                runtime_context: lunco_workspace::DocumentRuntimeOwner::Application,
                title: title.into(),
                dirty: true,
            };
            assert!(
                promoted_document_path(root, &entry, &mut used).is_err(),
                "{title}"
            );
            assert!(
                used.is_empty(),
                "an invalid name must not reserve a replacement identity"
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod rename_path_tests {
    use super::*;

    #[test]
    fn rename_parent_confinement_uses_storage_identity_and_rejects_invalid_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = lunco_storage::canonicalize_file_path(directory.path()).unwrap();
        let nested = root.join("nested");
        lunco_storage::ensure_directory_sync(&nested).unwrap();
        assert_eq!(
            confined_rename_source(&root, std::path::Path::new("nested/source.mo")).unwrap(),
            nested.join("source.mo")
        );
        for relative in ["", "..", "../outside.mo", "missing/source.mo"] {
            assert!(
                confined_rename_source(&root, std::path::Path::new(relative)).is_err(),
                "{relative:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rename_parent_confinement_rejects_escaping_directory_links_but_preserves_final_links() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = lunco_storage::canonicalize_file_path(directory.path()).unwrap();
        let secret = outside.path().join("secret.mo");
        lunco_storage::write_file_sync(&secret, b"outside source").unwrap();
        lunco_storage::create_directory_symlink_sync(outside.path(), &root.join("escape")).unwrap();
        assert!(confined_rename_source(&root, std::path::Path::new("escape/secret.mo")).is_err());
        lunco_storage::create_file_symlink_sync(&secret, &root.join("link.mo")).unwrap();
        let source = confined_rename_source(&root, std::path::Path::new("link.mo")).unwrap();
        assert_eq!(
            source,
            root.join("link.mo"),
            "the final symlink must not become its external target"
        );
        lunco_storage::rename_file_sync(&source, &root.join("renamed.mo")).unwrap();
        assert_eq!(
            lunco_storage::read_file_sync(&secret).unwrap(),
            b"outside source"
        );
        assert!(matches!(
            lunco_storage::entry_kind_file_sync(&source),
            Err(lunco_storage::StorageError::NotFound)
        ));
        assert_eq!(
            lunco_storage::read_file_sync(&root.join("renamed.mo")).unwrap(),
            b"outside source"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rename_entry_inspection_preserves_broken_sources_and_rejects_dangling_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let root = lunco_storage::canonicalize_file_path(directory.path()).unwrap();
        let absent = root.join("absent.mo");
        let link = root.join("link.mo");
        lunco_storage::create_file_symlink_sync(&absent, &link).unwrap();
        let source = confined_rename_source(&root, std::path::Path::new("link.mo")).unwrap();
        assert_eq!(
            lunco_storage::entry_kind_no_follow_file_sync(&source).unwrap(),
            lunco_storage::StorageEntryKind::Symlink
        );
        assert!(
            matches!(
                lunco_storage::entry_kind_file_sync(&source),
                Err(lunco_storage::StorageError::NotFound)
            ),
            "followed inspection keeps its target-reading contract"
        );
        let destination = unoccupied_rename_destination(&source, "renamed.mo").unwrap();
        lunco_storage::rename_file_sync(&source, &destination).unwrap();
        assert_eq!(
            lunco_storage::entry_kind_no_follow_file_sync(&destination).unwrap(),
            lunco_storage::StorageEntryKind::Symlink
        );
        let other = root.join("other.mo");
        lunco_storage::write_file_sync(&other, b"original").unwrap();
        assert!(
            unoccupied_rename_destination(&other, "renamed.mo")
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(lunco_storage::read_file_sync(&other).unwrap(), b"original");
        assert!(matches!(
            lunco_storage::entry_kind_file_sync(&absent),
            Err(lunco_storage::StorageError::NotFound)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn promotion_manifest_preflight_rejects_dangling_link_before_queueing_effects() {
        #[derive(Resource, Default)]
        struct Effects {
            manifests: usize,
            saves: usize,
        }
        let directory = tempfile::tempdir().unwrap();
        let root = lunco_storage::canonicalize_file_path(directory.path()).unwrap();
        let manifest = root.join(lunco_twin::MANIFEST_FILENAME);
        let absent = root.join("absent-manifest.toml");
        lunco_storage::create_file_symlink_sync(&absent, &manifest).unwrap();
        let entry = lunco_workspace::DocumentEntry {
            id: lunco_doc::DocumentId::new(1),
            kind: lunco_workspace::DocumentKindId::new("modelica"),
            origin: lunco_doc::DocumentOrigin::untitled("source"),
            runtime_context: lunco_workspace::DocumentRuntimeOwner::Application,
            title: "source.mo".into(),
            dirty: true,
        };
        let mut world = World::new();
        world.init_resource::<Effects>();
        world.add_observer(|_: On<CreateTwin>, mut effects: ResMut<Effects>| {
            effects.manifests += 1;
        });
        world.add_observer(|_: On<SaveAsDocument>, mut effects: ResMut<Effects>| {
            effects.saves += 1;
        });
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let result = {
            let mut commands = Commands::new(&mut queue, &world);
            promote_documents(
                root.to_str().unwrap().to_owned(),
                vec![entry],
                &mut commands,
            )
        };
        assert!(
            matches!(result, Err(Reject::InvalidOp(message)) if message.contains("already contains"))
        );
        queue.apply(&mut world);
        let effects = world.resource::<Effects>();
        assert_eq!(
            (effects.manifests, effects.saves),
            (0, 0),
            "rejected manifest preflight must not enqueue a manifest or save command"
        );
        assert_eq!(
            lunco_storage::entry_kind_no_follow_file_sync(&manifest).unwrap(),
            lunco_storage::StorageEntryKind::Symlink
        );
        assert!(matches!(
            lunco_storage::entry_kind_file_sync(&absent),
            Err(lunco_storage::StorageError::NotFound)
        ));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Picker resolution → typed command
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Component, Clone, Debug)]
struct PromotionIntent {
    active_twin: Option<lunco_workspace::TwinId>,
    replication: Option<lunco_workspace::ReplicationOwner>,
    sources: Vec<lunco_workspace::PinnedDocumentRuntimeOwner>,
}

impl PromotionIntent {
    fn capture(
        workspace: &lunco_workspace::Workspace,
        replication: Option<&lunco_workspace::ReplicationOwner>,
    ) -> Result<Self, String> {
        let sources = workspace
            .documents()
            .iter()
            .map(|entry| lunco_workspace::PinnedDocumentRuntimeOwner {
                document: entry.id,
                runtime: workspace.runtime_owner_for(entry),
            })
            .filter(|source| source.is_in_active_scope(Some(workspace), replication))
            .collect();
        let intent = Self {
            active_twin: workspace.active_twin,
            replication: replication.cloned(),
            sources,
        };
        intent.documents(workspace, replication)?;
        Ok(intent)
    }

    fn documents(
        &self,
        workspace: &lunco_workspace::Workspace,
        replication: Option<&lunco_workspace::ReplicationOwner>,
    ) -> Result<Vec<lunco_workspace::DocumentEntry>, String> {
        if workspace.active_twin != self.active_twin || replication != self.replication.as_ref() {
            return Err("source save session changed after admission".into());
        }
        if self
            .active_twin
            .is_some_and(|twin| workspace.twin(twin).is_none())
        {
            return Err("Save As Twin source Twin has closed".into());
        }
        self.sources
            .iter()
            .map(|source| {
                if !source.is_in_active_scope(Some(workspace), replication) {
                    return Err(format!(
                        "Save As Twin source {} has retired",
                        source.document
                    ));
                }
                workspace
                    .document(source.document)
                    .cloned()
                    .ok_or_else(|| format!("Save As Twin source {} has closed", source.document))
            })
            .collect()
    }
}

/// Source selection remains with Workspace; the picker never interprets Twin
/// policy. Save requests carry the canonical document pin directly.
fn source_pins<'a>(
    promotion: Option<&'a PromotionIntent>,
    source: Option<&'a lunco_workspace::PinnedDocumentRuntimeOwner>,
) -> impl Iterator<Item = &'a lunco_workspace::PinnedDocumentRuntimeOwner> {
    promotion
        .into_iter()
        .flat_map(|intent| intent.sources.iter())
        .chain(source)
}

fn reject_pick(commands: &mut Commands, message: String) {
    warn!("[FilePicker] {message}");
    commands.trigger(lunco_core::RuntimeError {
        name: "file-picker-rejected".into(),
        message,
    });
}

fn admit_source_pick(
    trigger: On<PickStarted>,
    requests: Query<
        (
            Option<&PromotionIntent>,
            Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
        ),
        With<PickInFlight>,
    >,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if !matches!(
        event.follow_up,
        PickFollowUp::SaveAs(_) | PickFollowUp::SaveAsTwin
    ) {
        return;
    }
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let admitted = requests
        .get(event.request)
        .map_err(|_| "picker source carrier retired".to_owned())
        .and_then(|(promotion, source)| {
            let workspace = workspace
                .as_deref()
                .ok_or_else(|| "file save picker has no source workspace".to_owned())?;
            match &event.follow_up {
                PickFollowUp::SaveAs(document) => {
                    let source = source
                        .ok_or_else(|| "save picker has no admitted source pin".to_owned())?;
                    if source.document == *document
                        && source.is_current(Some(&workspace.0), replication.as_ref())
                    {
                        Ok(())
                    } else {
                        Err(format!("save source {document} has retired"))
                    }
                }
                PickFollowUp::SaveAsTwin => promotion
                    .ok_or_else(|| "Save As Twin picker has no admitted session".to_owned())?
                    .documents(&workspace.0, replication.as_ref())
                    .map(|_| ()),
                _ => Err("picker follow-up does not admit a source".into()),
            }
        });
    if let Err(reason) = admitted {
        reject_pick(&mut commands, reason);
        commands.trigger(CancelPick {
            request: event.request,
        });
    }
}

fn retire_document_picks(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    requests: Query<
        (
            Entity,
            Option<&PromotionIntent>,
            Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
        ),
        With<PickInFlight>,
    >,
    mut commands: Commands,
) {
    for (request, promotion, source) in &requests {
        if source_pins(promotion, source).any(|source| source.document == trigger.event().doc) {
            commands.trigger(CancelPick { request });
        }
    }
}

fn retire_twin_picks(
    trigger: On<lunco_workspace::TwinClosed>,
    requests: Query<
        (
            Entity,
            Option<&PromotionIntent>,
            Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
        ),
        With<PickInFlight>,
    >,
    mut commands: Commands,
) {
    let twin = trigger.event().twin;
    for (request, promotion, source) in &requests {
        if source_pins(promotion, source).any(|source| source.runtime.local_twin() == Some(twin))
            || promotion.is_some_and(|promotion| promotion.active_twin == Some(twin))
        {
            commands.trigger(CancelPick { request });
        }
    }
}

fn retire_replication_picks(
    trigger: On<lunco_core_session::ReplicationOwnerRetired>,
    requests: Query<
        (
            Entity,
            Option<&PromotionIntent>,
            Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
        ),
        With<PickInFlight>,
    >,
    mut commands: Commands,
) {
    for (request, promotion, source) in &requests {
        if source_pins(promotion, source).any(|source| matches!(&source.runtime, lunco_workspace::DocumentRuntimeOwner::Replicated(owner) if owner == &trigger.event().owner)) {
            commands.trigger(CancelPick { request });
        }
    }
}

/// Translate a [`PickResolved`] event into the matching typed
/// file-workflow command, with the chosen path filled in.
///
/// Cancellations ([`lunco_workbench_file_dialog::PickCancelled`]) are silent by design —
/// no observer here for them. Add one if you want telemetry.
fn on_pick_resolved(
    trigger: On<PickResolved>,
    requests: Query<(
        &PickInFlight,
        Option<&PromotionIntent>,
        Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
    )>,
    workspace: Option<Res<WorkspaceResource>>,
    kinds: Option<Res<DocumentKindRegistry>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let Ok((in_flight, promotion, source)) = requests.get(event.request) else {
        reject_pick(
            &mut commands,
            "resolved file picker request has retired".into(),
        );
        return;
    };
    if in_flight.follow_up != event.follow_up {
        reject_pick(
            &mut commands,
            "resolved picker intent differs from its admitted request".into(),
        );
        return;
    }
    let handle = match &event.result {
        PickedPath::Path(handle) => handle,
        PickedPath::BrowserFile { display_name, .. } => {
            if !matches!(event.follow_up, PickFollowUp::OpenFile) {
                reject_pick(
                    &mut commands,
                    "browser bytes cannot resolve a folder or save intent".into(),
                );
                return;
            }
            let extension = std::path::Path::new(display_name)
                .extension()
                .and_then(|extension| extension.to_str());
            let supported = extension.is_some_and(|extension| {
                kinds.as_deref().is_some_and(|kinds| {
                    kinds.iter().any(|(_, meta)| {
                        meta.extensions
                            .iter()
                            .any(|supported| supported.eq_ignore_ascii_case(extension))
                    })
                })
            });
            if !supported {
                reject_pick(
                    &mut commands,
                    format!("no document domain is registered for picked file `{display_name}`"),
                );
            }
            // Document domains consume the exact bytes from this same event.
            return;
        }
    };
    let Some(native_path) = handle.as_file_path() else {
        reject_pick(&mut commands, "picker result is not a native path".into());
        return;
    };
    let Some(path) = native_path.to_str().map(str::to_owned) else {
        reject_pick(
            &mut commands,
            "picked native path cannot be represented by the document command transport".into(),
        );
        return;
    };
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    match &event.follow_up {
        PickFollowUp::OpenFile => match lunco_storage::file_path_to_uri(native_path) {
            Ok(path) => commands.trigger(OpenFile { path }),
            Err(reason) => reject_pick(
                &mut commands,
                format!("cannot admit picked file path: {reason}"),
            ),
        },
        PickFollowUp::OpenFolder => {
            commands.trigger(OpenFolder { path });
        }
        PickFollowUp::OpenTwin => {
            commands.trigger(OpenTwin { path });
        }
        PickFollowUp::AddFolderToWorkspace => {
            commands.trigger(AddFolderToWorkspace { path });
        }
        PickFollowUp::AddTwin => {
            commands.trigger(AddTwin { path });
        }
        PickFollowUp::SaveAs(document) => {
            let Some(source) = source else {
                reject_pick(
                    &mut commands,
                    "save picker has no admitted document source".into(),
                );
                return;
            };
            if source.document != *document
                || !source.is_current(
                    workspace.as_deref().map(|workspace| &workspace.0),
                    replication.as_ref(),
                )
            {
                reject_pick(
                    &mut commands,
                    format!("save picker source {document} has retired"),
                );
                return;
            }
            commands.trigger(SaveAsDocument {
                doc_id: *document,
                path,
            });
        }
        PickFollowUp::SaveAsTwin => {
            let Some(intent) = promotion else {
                reject_pick(
                    &mut commands,
                    "Save As Twin picker has no admitted source session".into(),
                );
                return;
            };
            let entries = workspace
                .as_deref()
                .ok_or_else(|| "Save As Twin source workspace has retired".to_owned())
                .and_then(|workspace| intent.documents(&workspace.0, replication.as_ref()));
            match entries {
                Ok(entries) => {
                    let _ = promote_documents(path, entries, &mut commands);
                }
                Err(reason) => reject_pick(&mut commands, reason),
            }
        }
        PickFollowUp::CreateTwin {
            name,
            default_scene,
        } => {
            commands.trigger(CreateTwin {
                path,
                name: name.clone(),
                default_scene: default_scene.clone(),
            });
        }
    }
}

// `register_commands!()` registers each command's type + observer in
// one call — which is also what makes a verb reachable by NAME from the
// HTTP API and rhai (dispatch resolves against the type registry). A
// hand-rolled `add_observer` alone would leave the command working
// in-process but invisible to both, so every `#[Command]` here goes
// through this list. `on_pick_resolved` is *not* in it — it observes a
// non-Command event (`PickResolved`) and is added directly in the
// plugin's `build`. `OpenFile` is also absent: the observer that
// loads `.mo` content lives in `lunco-modelica-core` and registers itself
// there; the workbench owns only the picker entry point.
register_commands!(
    on_add_folder_to_workspace_pick,
    on_add_twin_pick,
    on_create_twin_pick,
    on_new_document,
    on_open_twin_pick,
    on_rename_open_document,
    on_rename_twin_entry,
    on_save_all,
    on_save_as_twin,
    on_show_open_file_picker,
    on_show_open_folder_picker
);

/// Plugin that registers shell-level file-workflow commands.
///
/// Adds the picker backend and registers shell-level file-workflow commands.
///
/// Headless hosts can use the workspace and document commands directly; this
/// plugin is the windowed adapter that turns empty path fields into picker
/// requests and routes the selected handles back into those commands.
pub struct FileOpsPlugin;

impl Plugin for FileOpsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
        if !app.is_plugin_added::<lunco_workbench_file_dialog::PickerPlugin>() {
            app.add_plugins(lunco_workbench_file_dialog::PickerPlugin);
        }
        register_all_commands(app);
        // OpenFile is defined by lunco-doc-bevy, but the shell registers its
        // reflected type so a GUI-only host advertises the shared picker
        // command before a domain-specific observer is installed. No bytes
        // are read here; domain plugins own the observers.
        app.register_type::<OpenFile>();
        // CopyShareLink: workbench owns the typed struct so HTTP-API
        // introspection sees it; the observer lives in lunco-modelica-core.
        app.register_type::<CopyShareLink>();
        // USD scene-root resolution is owned by `lunco-usd-commands` so GUI and
        // headless launches use the same doc-first world-mount path.
        app.add_observer(on_pick_resolved)
            .add_observer(admit_source_pick)
            .add_observer(retire_document_picks)
            .add_observer(retire_twin_picks)
            .add_observer(retire_replication_picks);
        // Off-thread folder-scan pipeline: each `Open*` / `Add*` parks
        // a `Task<Result<TwinMode, _>>` in `PendingTwinOpens`; this
        // system polls them every frame and registers Twins as scans
        // complete. Keeps the UI thread responsive on huge trees
        // (`~/.cargo`, `node_modules`, …).
        app.init_resource::<PendingTwinOpens>();
        app.add_systems(Update, drain_pending_twin_opens);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod picker_admission_tests {
    use super::*;
    use lunco_workspace::{
        DocumentEntry, DocumentRuntimeOwner, ReplicatedSceneOwner, ReplicationOwner, TwinId,
        Workspace,
    };

    fn add_twin(workspace: &mut Workspace) -> (tempfile::TempDir, TwinId) {
        let directory = tempfile::tempdir().unwrap();
        let twin = match lunco_twin::TwinMode::open(directory.path()).unwrap() {
            lunco_twin::TwinMode::Folder(twin) => twin,
            _ => panic!("empty directory must be a folder"),
        };
        let id = workspace.add_twin(twin);
        (directory, id)
    }

    fn document(
        workspace: &mut Workspace,
        id: u64,
        runtime: DocumentRuntimeOwner,
    ) -> lunco_doc::DocumentId {
        let id = lunco_doc::DocumentId::new(id);
        workspace.add_document(DocumentEntry {
            id,
            kind: lunco_workspace::DocumentKindId::new("text"),
            origin: lunco_doc::DocumentOrigin::untitled("source"),
            runtime_context: runtime,
            title: "source.txt".into(),
            dirty: true,
        });
        id
    }

    #[test]
    fn picker_admission_uses_exact_local_and_remote_active_source_scope() {
        let mut workspace = Workspace::new();
        let (_first_root, first) = add_twin(&mut workspace);
        let (second_root, second) = add_twin(&mut workspace);
        let local = document(&mut workspace, 1, DocumentRuntimeOwner::LocalTwin(first));
        document(&mut workspace, 2, DocumentRuntimeOwner::Application);
        document(&mut workspace, 3, DocumentRuntimeOwner::LocalTwin(second));
        document(
            &mut workspace,
            4,
            DocumentRuntimeOwner::LocalTwin(TwinId::new(999)),
        );
        workspace.active_twin = Some(first);
        let intent = PromotionIntent::capture(&workspace, None).unwrap();
        assert_eq!(
            intent
                .sources
                .iter()
                .map(|source| source.document)
                .collect::<Vec<_>>(),
            vec![local]
        );
        workspace.document_mut(local).unwrap().title = "edited.txt".into();
        assert_eq!(
            intent.documents(&workspace, None).unwrap()[0].title,
            "edited.txt",
            "same-owner edits remain saveable"
        );

        let connection = World::new().spawn_empty().id();
        let remote = ReplicationOwner::Twin {
            scene: ReplicatedSceneOwner {
                connection,
                host_twin: TwinId::new(7),
                authority: "mount-a".into(),
                root: second_root.path().to_owned(),
                owns_mount: false,
            },
        };
        workspace.active_twin = None;
        let imported = document(
            &mut workspace,
            5,
            DocumentRuntimeOwner::Replicated(remote.clone()),
        );
        let intent = PromotionIntent::capture(&workspace, Some(&remote)).unwrap();
        assert_eq!(
            intent
                .sources
                .iter()
                .map(|source| source.document)
                .collect::<Vec<_>>(),
            vec![imported]
        );
        workspace.close_document(imported);
        let empty = PromotionIntent::capture(&workspace, Some(&remote)).unwrap();
        assert!(
            empty.sources.is_empty(),
            "Application drafts must not enter a remote Twin export"
        );
        assert!(
            empty.documents(&workspace, None).is_err(),
            "empty exports still pin exact connection/mount scope"
        );
    }

    #[derive(Resource, Default)]
    struct Rejections {
        errors: Vec<String>,
        cancelled: Vec<Entity>,
        original: Option<TwinId>,
    }

    fn observe_error(trigger: On<lunco_core::RuntimeError>, mut observed: ResMut<Rejections>) {
        observed.errors.push(trigger.event().message.clone());
    }

    fn observe_cancel(trigger: On<CancelPick>, mut observed: ResMut<Rejections>) {
        observed.cancelled.push(trigger.event().request);
    }

    #[test]
    fn picker_admission_origin_pin_survives_queued_twin_switch_before_backend() {
        let mut workspace = Workspace::new();
        let (_first_root, first) = add_twin(&mut workspace);
        let (_second_root, second) = add_twin(&mut workspace);
        document(&mut workspace, 1, DocumentRuntimeOwner::LocalTwin(first));
        document(&mut workspace, 2, DocumentRuntimeOwner::LocalTwin(second));
        workspace.active_twin = Some(first);
        let mut app = App::new();
        app.insert_resource(WorkspaceResource(workspace))
            .init_resource::<Rejections>()
            .init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>()
            .add_observer(on_save_as_twin)
            .add_observer(admit_source_pick)
            .add_observer(observe_error)
            .add_observer(observe_cancel);
        // Simulate the transport queue between source-command admission and
        // backend start without opening an OS dialog in a resource-seam test.
        app.add_observer(
            move |trigger: On<lunco_workbench_file_dialog::PickHandle>,
                  intents: Query<&PromotionIntent>,
                  mut observed: ResMut<Rejections>,
                  mut commands: Commands| {
                let request = trigger.event().request;
                observed.original = intents.get(request).unwrap().active_twin;
                commands.queue(move |world: &mut World| {
                    world.resource_mut::<WorkspaceResource>().active_twin = Some(second);
                    let follow_up = world
                        .get::<PickInFlight>(request)
                        .unwrap()
                        .follow_up
                        .clone();
                    world.trigger(PickStarted { request, follow_up });
                });
            },
        );
        app.world_mut().trigger(SaveAsTwin {
            folder: String::new(),
        });
        app.world_mut().flush();
        let observed = app.world().resource::<Rejections>();
        assert_eq!(observed.original, Some(first));
        assert_eq!(observed.cancelled.len(), 1);
        assert!(
            observed
                .errors
                .iter()
                .any(|error| error.contains("source save session changed"))
        );
    }

    #[test]
    fn picker_admission_rejects_missing_and_retired_document_pins() {
        let mut workspace = Workspace::new();
        let (_root, twin) = add_twin(&mut workspace);
        let doc = document(&mut workspace, 1, DocumentRuntimeOwner::LocalTwin(twin));
        let pin = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(doc, Some(&workspace))
            .unwrap();
        workspace.close_twin(twin);
        let mut app = App::new();
        app.insert_resource(WorkspaceResource(workspace))
            .init_resource::<Rejections>()
            .add_observer(admit_source_pick)
            .add_observer(observe_error)
            .add_observer(observe_cancel);
        let missing = app
            .world_mut()
            .spawn(PickInFlight {
                follow_up: PickFollowUp::SaveAs(doc),
            })
            .id();
        let retired = app
            .world_mut()
            .spawn((
                PickInFlight {
                    follow_up: PickFollowUp::SaveAs(doc),
                },
                pin,
            ))
            .id();
        for request in [missing, retired] {
            app.world_mut().trigger(PickStarted {
                request,
                follow_up: PickFollowUp::SaveAs(doc),
            });
        }
        app.world_mut().flush();
        let observed = app.world().resource::<Rejections>();
        assert_eq!(observed.cancelled, vec![missing, retired]);
        assert!(
            observed
                .errors
                .iter()
                .any(|error| error.contains("no admitted source pin"))
        );
        assert!(
            observed
                .errors
                .iter()
                .any(|error| error.contains("retired"))
        );
    }
}
