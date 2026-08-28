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
//! native picker via [`crate::picker::PickHandle`]; non-empty paths skip the
//! dialog (recents, drag-drop, automation).
//!
//! ## What this module ships
//!
//! - Shell-only commands such as [`SaveAll`], [`SaveAsTwin`], and the picker
//!   requests.
//! - The picker-resolution router ([`on_pick_resolved`]) that turns a
//!   [`crate::picker::PickResolved`] event into the matching typed verb with the
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
use lunco_core::{on_command, register_commands, Command};
use lunco_doc_bevy::SaveAsDocument;
use lunco_twin::{DocumentKindId, DocumentKindRegistry};

use crate::picker::{PickFollowUp, PickHandle, PickMode, PickResolved};
use lunco_workspace::open::{
    drain_pending_twin_opens, AddFolderToWorkspace, AddTwin, CreateTwin, OpenFolder, OpenTwin,
    PendingTwinOpens,
};
use lunco_workspace::{FileRenamed, WorkspaceResource};

/// Request a system "Open File" dialog.
///
/// Dispatches [`ShowOpenFilePicker`] which triggers the picker via
/// [`crate::picker::PickHandle`]. On success, the picker resolves to
/// [`OpenFile`] with the chosen path.
#[Command(default)]
pub struct ShowOpenFilePicker {}

/// Request a system "Open Folder" dialog.
///
/// Dispatches [`ShowOpenFolderPicker`] which triggers the picker via
/// [`crate::picker::PickHandle`]. On success, the picker resolves to
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
/// (`lunco-modelica` encodes the active model's source into a URL
/// fragment). The headless HTTP API exposes the read-only `GetShareLink`
/// query separately; it returns the URL in its `data` payload instead of
/// touching a clipboard.
#[Command(default)]
pub struct CopyShareLink {}

/// Rename an open document (a tab in the workspace).
///
/// Differs from [`RenameTwinEntry`]: identifies the target by
/// [`DocumentId`] rather than `(twin_root, relative_path)`, so it works
/// for Untitled drafts that have no on-disk path, as well as for saved
/// files that belong to no open Twin.
///
/// The observer routes by [`DocumentOrigin`]:
///
/// - `File { writable: true }` *under an open Twin*: forwards to
///   [`RenameTwinEntry`] — same on-disk path, same `FileRenamed` chain,
///   same Modelica class-name rewrite.
/// - `Untitled { name }`: domain crates observe this command directly
///   (Modelica chains to [`RenameModelicaClass`]) — workbench has no
///   semantic handle on what an Untitled draft means.
/// - `File { writable: false }` or `Bundled`: read-only, rejected.
#[Command(default)]
pub struct RenameOpenDocument {
    /// The document to rename.
    pub doc: lunco_doc::DocumentId,
    /// New filename / class identifier — no path separators allowed.
    pub new_name: String,
}

/// Rename a file or folder *inside* an open Twin.
///
/// Identifies the entry by `(twin_root, relative_path)` so the
/// command body is self-contained (no Bevy resource handles) — HTTP
/// callers, scripts, and the inline browser editor all dispatch the
/// same shape. The observer:
///
/// 1. Validates inputs (new_name non-empty, no path separators, source
///    exists, target doesn't already exist).
/// 2. Asks [`lunco_storage`] to rename backend handles for the absolute paths.
/// 3. Re-scans the affected Twin via [`Twin::reload`] so the file
///    index reflects disk.
/// 4. Patches every open Document whose `DocumentOrigin::File { path }`
///    lay under the old path — paths are rewritten so live edits don't
///    detach from disk.
/// 5. Fires [`FileRenamed`] for domain plugins to chain follow-ups
///    (Modelica class-declaration rename, USD reference rewrites, …).
#[Command(default)]
pub struct RenameTwinEntry {
    /// Absolute path of the Twin root the entry belongs to. The
    /// observer resolves this back to a `TwinId` via
    /// [`WorkspaceResource::twins`].
    pub twin_root: String,
    /// Path of the entry relative to `twin_root` (e.g. `Rover.mo` or
    /// `subdir/Other.mo`).
    pub relative_path: String,
    /// New filename — no path separators allowed (rename only; move
    /// across directories is a separate concern).
    pub new_name: String,
}

/// Save every open document in the current session.
///
/// Documents with a writable canonical path are written via their
/// owning domain's [`SaveDocument`](lunco_doc_bevy::SaveDocument)
/// observer. Untitled documents are written into the active Twin using
/// their workspace title; with no active Twin their domain's normal Save-As
/// picker is used.
#[Command(default)]
pub struct SaveAll {}

/// Promote the current session into a Twin at `folder`.
///
/// Writes `twin.toml`, saves every open document into the new root, and
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
    // Pick the first registered kind that opts into File→New. UI may
    // surface a "last used" preference later; for now first-found is
    // fine — only Modelica registers today.
    let default_kind: Option<DocumentKindId> = registry
        .iter()
        .find(|(_, m)| m.can_create_new)
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
    use crate::picker::{PickHandle, PickMode};
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
    commands.trigger(PickHandle {
        mode: PickMode::OpenFile(crate::picker::OpenFilter::new("Supported files", &ext_refs)),
        on_resolved: PickFollowUp::OpenFile,
    });
}

#[on_command(ShowOpenFolderPicker)]
fn on_show_open_folder_picker(_trigger: On<ShowOpenFolderPicker>, mut commands: Commands) {
    use crate::picker::{PickHandle, PickMode};
    commands.trigger(PickHandle {
        mode: PickMode::OpenFolder,
        on_resolved: PickFollowUp::OpenFolder,
    });
}

/// Empty path means "ask the windowed workbench for a folder". A non-empty
/// path is handled by the workspace-owned creation observer.
#[on_command(CreateTwin)]
fn on_create_twin_pick(trigger: On<CreateTwin>, mut commands: Commands) {
    let event = trigger.event();
    if event.path.is_empty() {
        let name = event.name.clone();
        let default_scene = event.default_scene.clone();
        commands.trigger(PickHandle {
            mode: PickMode::OpenFolder,
            on_resolved: PickFollowUp::CreateTwin {
                name,
                default_scene,
            },
        });
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
    use crate::picker::{PickHandle, PickMode};
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open::on_open_twin`
    }
    commands.trigger(PickHandle {
        mode: PickMode::OpenFolder,
        on_resolved: PickFollowUp::OpenTwin,
    });
}

/// Picker seam for [`AddFolderToWorkspace`] — see [`on_open_twin_pick`].
#[on_command(AddFolderToWorkspace)]
fn on_add_folder_to_workspace_pick(trigger: On<AddFolderToWorkspace>, mut commands: Commands) {
    use crate::picker::{PickHandle, PickMode};
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open`
    }
    commands.trigger(PickHandle {
        mode: PickMode::OpenFolder,
        on_resolved: PickFollowUp::AddFolderToWorkspace,
    });
}

/// Picker seam for [`AddTwin`] — see [`on_open_twin_pick`].
#[on_command(AddTwin)]
fn on_add_twin_pick(trigger: On<AddTwin>, mut commands: Commands) {
    use crate::picker::{PickHandle, PickMode};
    if !trigger.event().path.is_empty() {
        return; // handled by `lunco_workspace::open`
    }
    commands.trigger(PickHandle {
        mode: PickMode::OpenFolder,
        on_resolved: PickFollowUp::AddTwin,
    });
}

#[on_command(RenameOpenDocument)]
fn on_rename_open_document(
    trigger: On<RenameOpenDocument>,
    workspace: Res<WorkspaceResource>,
    mut commands: Commands,
) {
    use lunco_doc::DocumentOrigin;
    let ev = trigger.event();
    let new_name = ev.new_name.trim().to_string();
    if new_name.is_empty() {
        warn!("[RenameOpenDocument] empty new_name");
        return;
    }
    let Some(entry) = workspace.document(ev.doc) else {
        warn!("[RenameOpenDocument] no Workspace doc with id {}", ev.doc);
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
                    ev.doc,
                    path.display()
                );
                return;
            };
            let rel = match path.strip_prefix(&root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => return,
            };
            commands.trigger(RenameTwinEntry {
                twin_root: root.to_string_lossy().into_owned(),
                relative_path: rel.to_string_lossy().into_owned(),
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
            warn!("[RenameOpenDocument] doc {} is read-only", ev.doc);
        }
    }
}

#[on_command(RenameTwinEntry)]
fn on_rename_twin_entry(
    trigger: On<RenameTwinEntry>,
    #[cfg(not(target_arch = "wasm32"))] mut workspace: ResMut<WorkspaceResource>,
    #[cfg(not(target_arch = "wasm32"))] mut commands: Commands,
) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = trigger;
        warn!("[RenameTwinEntry] rename not supported on wasm for filesystem-path Twin entries");
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use lunco_doc::DocumentOrigin;
        let ev = trigger.event();
        let twin_root = std::path::PathBuf::from(&ev.twin_root);
        let new_name = ev.new_name.trim();
        if new_name.is_empty() {
            warn!("[RenameTwinEntry] new_name is empty");
            return;
        }
        if new_name.contains(std::path::MAIN_SEPARATOR)
            || new_name.contains('/')
            || new_name == "."
            || new_name == ".."
        {
            warn!(
                "[RenameTwinEntry] new_name `{new_name}` contains a path separator or \
             special segment — rename only, no move across directories"
            );
            return;
        }
        // Resolve TwinId by matching root path.
        let twin_id = workspace
            .twins()
            .find(|(_, t)| t.root == twin_root)
            .map(|(id, _)| id);
        let Some(twin_id) = twin_id else {
            warn!(
                "[RenameTwinEntry] no open Twin matches root {}",
                twin_root.display()
            );
            return;
        };
        let old_rel = std::path::PathBuf::from(&ev.relative_path);
        if old_rel.as_os_str().is_empty()
            || !old_rel
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            warn!(
                "[RenameTwinEntry] relative_path must stay within the Twin: {}",
                old_rel.display()
            );
            return;
        }
        let old_abs = twin_root.join(&old_rel);
        let old_kind = match lunco_storage::entry_kind_file_sync(&old_abs) {
            Ok(kind) => kind,
            Err(lunco_storage::StorageError::NotFound) => {
                warn!("[RenameTwinEntry] source missing: {}", old_abs.display());
                return;
            }
            Err(error) => {
                warn!(
                    "[RenameTwinEntry] cannot inspect source {}: {error}",
                    old_abs.display()
                );
                return;
            }
        };
        let new_abs = old_abs
            .parent()
            .map(|p| p.join(new_name))
            .unwrap_or_else(|| twin_root.join(new_name));
        if new_abs == old_abs {
            // No-op (user submitted the existing name) — silent.
            return;
        }
        match lunco_storage::entry_kind_file_sync(&new_abs) {
            Ok(_) => {
                warn!(
                    "[RenameTwinEntry] target already exists: {}",
                    new_abs.display()
                );
                return;
            }
            Err(lunco_storage::StorageError::NotFound) => {}
            Err(error) => {
                warn!(
                    "[RenameTwinEntry] cannot inspect target {}: {error}",
                    new_abs.display()
                );
                return;
            }
        }
        let is_dir = matches!(old_kind, lunco_storage::StorageEntryKind::Directory);
        if let Err(e) = lunco_storage::rename_file_sync(&old_abs, &new_abs) {
            warn!(
                "[RenameTwinEntry] storage rename {} -> {} failed: {e}",
                old_abs.display(),
                new_abs.display()
            );
            return;
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
    } // end #[cfg(not(target_arch = "wasm32"))]
}

#[on_command(SaveAll)]
fn on_save_all(
    _trigger: On<SaveAll>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else {
        warn!("[SaveAll] no workspace is installed");
        return;
    };
    let active_root = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .map(|twin| twin.root.clone());
    let entries = workspace.documents().to_vec();
    let mut used = std::collections::HashSet::new();
    for entry in entries {
        if entry.origin.is_untitled() {
            if let Some(root) = &active_root {
                let path = promoted_document_path(root, &entry, &mut used);
                commands.trigger(SaveAsDocument {
                    doc: entry.id,
                    path: path.display().to_string(),
                });
            } else {
                // The domain SaveDocument observer opens its normal Save-As
                // picker. This keeps Save All useful for a loose draft while
                // leaving path selection with the document owner.
                commands.trigger(lunco_doc_bevy::SaveDocument { doc: entry.id });
            }
        } else {
            commands.trigger(lunco_doc_bevy::SaveDocument { doc: entry.id });
        }
    }
}

#[on_command(SaveAsTwin)]
fn on_save_as_twin(
    trigger: On<SaveAsTwin>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    use crate::picker::{PickHandle, PickMode};
    let folder = trigger.event().folder.clone();
    if folder.is_empty() {
        commands.trigger(PickHandle {
            mode: PickMode::OpenFolder,
            on_resolved: PickFollowUp::SaveAsTwin,
        });
        return;
    }
    let root = std::path::PathBuf::from(&folder);
    let manifest_path = root.join(lunco_twin::MANIFEST_FILENAME);
    if matches!(
        lunco_storage::entry_kind_file_sync(&manifest_path),
        Ok(lunco_storage::StorageEntryKind::File) | Ok(lunco_storage::StorageEntryKind::Directory)
    ) {
        warn!(
            "[SaveAsTwin] `{}` already contains {} — choose a new Twin folder",
            root.display(),
            lunco_twin::MANIFEST_FILENAME
        );
        return;
    }
    match lunco_storage::entry_kind_file_sync(&root) {
        Ok(lunco_storage::StorageEntryKind::File) => {
            warn!("[SaveAsTwin] `{}` is not a folder", root.display());
            return;
        }
        Ok(lunco_storage::StorageEntryKind::Directory)
        | Err(lunco_storage::StorageError::NotFound) => {}
        Err(error) => {
            warn!("[SaveAsTwin] cannot inspect `{}`: {error}", root.display());
            return;
        }
    }
    let Some(workspace) = workspace else {
        warn!("[SaveAsTwin] no workspace is installed");
        return;
    };
    let entries = workspace.documents().to_vec();
    let mut used = std::collections::HashSet::new();
    let mut default_scene = String::new();
    let mut saves = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = promoted_document_path(&root, &entry, &mut used);
        if default_scene.is_empty() && entry.kind.as_str() == "usd" {
            default_scene = path
                .strip_prefix(&root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .into_owned();
        }
        saves.push((entry.id, path));
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
        commands.trigger(SaveAsDocument {
            doc,
            path: path.display().to_string(),
        });
    }
}

/// Choose a safe, stable filename for an open document being promoted into a
/// new Twin. Document domains still own serialization; the workbench only
/// supplies a collision-free destination based on their workspace title.
fn promoted_document_path(
    root: &std::path::Path,
    entry: &lunco_workspace::DocumentEntry,
    used: &mut std::collections::HashSet<String>,
) -> std::path::PathBuf {
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
    let original = base.clone();
    let mut suffix = 2;
    while !used.insert(base.to_ascii_lowercase()) {
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
    root.join(base)
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
            context_twin: None,
            title: "Engine Model".into(),
            dirty: true,
        };
        let second = lunco_workspace::DocumentEntry {
            id: lunco_doc::DocumentId::new(2),
            title: first.title.clone(),
            ..first.clone()
        };
        assert_eq!(
            promoted_document_path(root, &first, &mut used),
            root.join("Engine_Model.mo")
        );
        assert_eq!(
            promoted_document_path(root, &second, &mut used),
            root.join("Engine_Model-2.mo")
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Picker resolution → typed command
// ─────────────────────────────────────────────────────────────────────────────

/// Translate a [`PickResolved`] event into the matching typed
/// file-workflow command, with the chosen path filled in.
///
/// Cancellations ([`picker::PickCancelled`]) are silent by design —
/// no observer here for them. Add one if you want telemetry.
fn on_pick_resolved(trigger: On<PickResolved>, mut commands: Commands) {
    let ev = trigger.event();
    let Some(path) = ev.handle.as_file_path().map(|p| p.display().to_string()) else {
        warn!(
            "[PickResolved] non-file handle — picker backend produced something \
             other than `StorageHandle::File`; ignoring"
        );
        return;
    };
    match &ev.follow_up {
        PickFollowUp::OpenFile => {
            commands.trigger(OpenFile { path });
        }
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
        PickFollowUp::SaveAs(doc) => {
            commands.trigger(SaveAsDocument { doc: *doc, path });
        }
        PickFollowUp::SaveAsTwin => {
            commands.trigger(SaveAsTwin { folder: path });
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
// loads `.mo` content lives in `lunco-modelica` and registers itself
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
/// Auto-installed by `WorkbenchPlugin`. Headless tests that want
/// these commands without the full dock shell can add it directly.
pub struct FileOpsPlugin;

impl Plugin for FileOpsPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
        // OpenFile is defined by lunco-doc-bevy, but the shell registers its
        // reflected type so a GUI-only host advertises the shared picker
        // command before a domain-specific observer is installed. No bytes
        // are read here; domain plugins own the observers.
        app.register_type::<OpenFile>();
        // CopyShareLink: workbench owns the typed struct so HTTP-API
        // introspection sees it; the observer lives in lunco-modelica.
        app.register_type::<CopyShareLink>();
        // USD scene-root resolution is owned by `lunco-usd` so GUI and
        // headless launches use the same doc-first world-mount path.
        app.add_observer(on_pick_resolved);
        // Off-thread folder-scan pipeline: each `Open*` / `Add*` parks
        // a `Task<Result<TwinMode, _>>` in `PendingTwinOpens`; this
        // system polls them every frame and registers Twins as scans
        // complete. Keeps the UI thread responsive on huge trees
        // (`~/.cargo`, `node_modules`, …).
        app.init_resource::<PendingTwinOpens>();
        app.add_systems(Update, drain_pending_twin_opens);
    }
}
