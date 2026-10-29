//! Active-Twin filesystem navigation from the existing admitted inventory.
//!
//! The tree is prepared on workspace/query changes. Painting visits only
//! expanded branches and emits the normal document/source and rename intents.

use bevy::prelude::*;
use egui;
use lunco_doc_bevy::rename::RenameOpenDocument;
use lunco_workbench_core::source::{OpenTwinSource, is_source_only_text_path};
use lunco_workspace::rename::RenameTwinEntry;
use std::sync::Arc;

use super::{
    BrowserAction, BrowserCtx, BrowserSection,
    path_tree::{PathTree, build_path_tree},
};

/// Map a domain kind id to its canonical file extension. Used to
/// append `.mo`, `.usda`, … to display names for unsaved drafts that
/// carry no on-disk path yet. Saved docs already include their
/// extension in `display_name`; we only synthesize when missing.
fn extension_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "Modelica" | "modelica" => Some("mo"),
        "USD" | "usd" => Some("usda"),
        _ => None,
    }
}

/// `display_name` with the appropriate extension appended when the
/// name doesn't already have one — so an Untitled Modelica draft
/// renders as `Untitled.mo`, not bare `Untitled`. Saved files keep
/// their stored name unchanged.
fn display_name_with_ext(entry: &super::UnsavedDocEntry) -> String {
    if entry.display_name.contains('.') {
        return entry.display_name.clone();
    }
    match extension_for_kind(&entry.kind) {
        Some(ext) => format!("{}.{}", entry.display_name, ext),
        None => entry.display_name.clone(),
    }
}

/// Bucket a flat file list into nested directories by walking each
/// `relative_path`'s components.
fn build_tree(files: &[lunco_twin::FileEntry]) -> PathTree<lunco_twin::FileEntry> {
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    build_path_tree(
        files
            .into_iter()
            .map(|file| (file.relative_path.clone(), file)),
    )
}

#[derive(Resource, Default)]
pub(crate) struct FileTreeView {
    twin: Option<lunco_workspace::TwinId>,
    query: String,
    tree: Arc<PathTree<lunco_twin::FileEntry>>,
}

/// Project the existing inventory on workspace/filter changes, outside painting.
pub(crate) fn prepare_file_tree(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    query: Res<super::BrowserQuery>,
    mut view: ResMut<FileTreeView>,
) {
    if !query.is_changed()
        && !workspace
            .as_ref()
            .is_some_and(|workspace| workspace.is_changed())
    {
        return;
    }
    let twin = workspace.as_ref().and_then(|workspace| {
        workspace
            .active_twin
            .and_then(|id| workspace.twin(id).map(|twin| (id, twin)))
    });
    view.twin = twin.map(|(id, _)| id);
    view.query = query.text.clone();
    view.tree = Arc::new(match twin {
        Some((_, twin)) => build_tree(
            &twin
                .files()
                .iter()
                .filter(|file| query.matches(&file.relative_path.to_string_lossy()))
                .cloned()
                .collect::<Vec<_>>(),
        ),
        None => PathTree::default(),
    });
}

pub(crate) fn retire_file_tree(
    trigger: On<lunco_workspace::TwinClosed>,
    mut view: ResMut<FileTreeView>,
) {
    if trigger.was_active {
        *view = FileTreeView::default();
    }
}

/// In-progress inline rename. At most one row across the section can
/// be in rename mode at a time — `target_abs` identifies which one.
/// `needs_focus` is set on entry and cleared after the first frame so
/// the `TextEdit` receives focus exactly once.
#[derive(Default)]
struct RenameInProgress {
    /// Absolute path of the entry being renamed (`twin.root.join(rel)`).
    /// Used to match against rendered rows and to scope the rename
    /// command to the correct Twin.
    target_abs: std::path::PathBuf,
    /// Absolute path of the Twin root containing the entry — captured
    /// up front so we can dispatch the rename command without
    /// re-resolving from `ctx.twins` at submit time.
    twin_root: std::path::PathBuf,
    /// Path relative to the Twin root, passed verbatim into
    /// [`RenameTwinEntry::relative_path`].
    relative_path: std::path::PathBuf,
    /// Edit buffer, initialised with the current filename (last segment
    /// only, not the full relative path).
    buffer: String,
    /// One-shot flag — focus the `TextEdit` on the first render after
    /// entering rename mode, then clear so subsequent frames don't
    /// steal focus from other widgets.
    needs_focus: bool,
}

/// Inline rename state for a workspace doc row (Untitled or saved
/// file in the top "Workspace" list). Separate from [`RenameInProgress`]
/// because workspace docs are identified by [`DocumentId`], not by an
/// on-disk path — Untitled drafts have no path at all.
#[derive(Default)]
struct DocRenameInProgress {
    /// The document being renamed.
    doc: lunco_doc::DocumentId,
    /// Edit buffer, pre-filled with the current display name.
    buffer: String,
    /// One-shot focus flag.
    needs_focus: bool,
}

pub const FILES_SECTION_ID: &str = "lunco.workbench.files";

/// The built-in Files section impl.
#[derive(Default)]
pub struct FilesSection {
    /// Inline rename state for a Twin-tree row (file or directory).
    rename: Option<RenameInProgress>,
    /// Inline rename state for a workspace-doc row (the top list of
    /// open documents, above the per-Twin trees).
    rename_doc: Option<DocRenameInProgress>,
}

impl BrowserSection for FilesSection {
    fn id(&self) -> &str {
        FILES_SECTION_ID
    }

    fn title(&self) -> &str {
        "Files"
    }

    fn order(&self) -> u32 {
        // Files is the primary view in the selector.
        0
    }

    fn on_twin_closed(&mut self, _event: &lunco_workspace::TwinClosed) {
        self.rename = None;
        self.rename_doc = None;
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let query = ctx
            .resource::<super::BrowserQuery>()
            .cloned()
            .unwrap_or_default();
        // Documents supply dirty markers for indexed file rows. Untitled and
        // loose documents also need standalone rows because they have no Twin path.
        // Unsaved drafts get a dirty dot in the theme warning colour
        // plus an italic name; saved docs render plain. Kind badges
        // are intentionally omitted — file extensions in the display
        // name carry that information for the user.
        let docs: Vec<super::UnsavedDocEntry> = match (
            ctx.resource::<lunco_workspace::WorkspaceResource>(),
            ctx.resource::<super::UnsavedDocs>(),
        ) {
            (Some(workspace), Some(entries)) => entries
                .entries
                .iter()
                .filter(|entry| {
                    workspace
                        .document(entry.id)
                        .is_some_and(|doc| workspace.document_is_in_active_scope(doc))
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let warning = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.warning)
            .unwrap_or(egui::Color32::YELLOW);
        // Dirty marker is intentionally subtle — same hue as warning
        // but small and semi-transparent so it reads as a hint, not a
        // siren. The full-strength warning colour is for actual
        // problems (lints, parse errors), not unsaved drafts.
        let dirty_dot_color =
            egui::Color32::from_rgba_unmultiplied(warning.r(), warning.g(), warning.b(), 110);

        // Workspace-doc rename intents (parallel to the Twin-tree
        // queues below). Drained once the loop finishes so the closures
        // don't fight the borrow checker.
        let mut doc_begin_rename: Option<DocRenameInProgress> = None;
        let mut doc_submit: Option<(lunco_doc::DocumentId, String)> = None;
        let mut doc_cancel = false;
        let mut doc_close: Option<lunco_doc::DocumentId> = None;

        let standalone = |entry: &super::UnsavedDocEntry| {
            ctx.resource::<lunco_workspace::WorkspaceResource>()
                .and_then(|workspace| {
                    let doc = workspace.document(entry.id)?;
                    let path = doc.origin.canonical_path()?;
                    let twin = workspace.active_twin.and_then(|id| workspace.twin(id))?;
                    Some(!path.starts_with(&twin.root))
                })
                .unwrap_or(true)
        };
        let standalone_ids: std::collections::HashSet<_> = docs
            .iter()
            .filter(|entry| standalone(entry))
            .map(|entry| entry.id)
            .collect();
        for entry in docs
            .iter()
            .filter(|entry| standalone_ids.contains(&entry.id))
            .filter(|entry| {
                !query.is_active()
                    || query.matches(&display_name_with_ext(entry))
                    || query.matches(&entry.kind)
            })
        {
            let in_rename = self
                .rename_doc
                .as_ref()
                .map(|r| r.doc == entry.id)
                .unwrap_or(false);
            ui.horizontal(|ui| {
                if entry.is_unsaved {
                    ui.label(
                        egui::RichText::new("•")
                            .color(dirty_dot_color)
                            .text_style(lunco_theme::TypographyRole::Caption.text_style()),
                    );
                } else {
                    ui.label(egui::RichText::new("  "));
                }
                if in_rename {
                    let state = self
                        .rename_doc
                        .as_mut()
                        .expect("in_rename ⇒ rename_doc Some");
                    let resp = ui.add(
                        lunco_workbench_widgets::text_editor::singleline(&mut state.buffer)
                            .desired_width(f32::INFINITY),
                    );
                    if state.needs_focus {
                        resp.request_focus();
                        state.needs_focus = false;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if enter {
                        doc_submit = Some((state.doc, state.buffer.clone()));
                    } else if esc || (resp.lost_focus() && !enter) {
                        doc_cancel = true;
                    }
                } else {
                    let display = display_name_with_ext(entry);
                    let text = if entry.is_unsaved {
                        egui::RichText::new(&display).italics()
                    } else {
                        egui::RichText::new(&display)
                    };
                    let r = ui.add(
                        egui::Label::new(text)
                            .selectable(false)
                            .sense(egui::Sense::click()),
                    );
                    if r.double_clicked() {
                        // Pre-fill with the stem (no extension) so the
                        // user edits just the name. The kind-to-ext
                        // mapping is the same `extension_for_kind`
                        // used in display.
                        let leaf = entry
                            .display_name
                            .split('.')
                            .next()
                            .unwrap_or(&entry.display_name)
                            .to_string();
                        doc_begin_rename = Some(DocRenameInProgress {
                            doc: entry.id,
                            buffer: leaf,
                            needs_focus: true,
                        });
                    }
                    // Close control — closes the document, its
                    // tabs, and (on wasm) its localStorage autosave
                    // entry. Without this a restored draft has no
                    // delete path from the UI and resurrects on every
                    // reload. Right-aligned so it doesn't crowd names.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = lunco_workbench_widgets::icon_button(
                            ui,
                            lunco_workbench_widgets::UiIcon::Close,
                            "Close document",
                        )
                        .on_hover_text(
                            "Close document (discards unsaved \
                                     changes)",
                        );
                        if close.clicked() {
                            doc_close = Some(entry.id);
                        }
                    });
                }
            });
        }

        // Drain the workspace-doc intents (rename + close) here —
        // BEFORE the `twins.is_empty()` early return below. These
        // intents are about open documents, not the Twin file tree,
        // so they must fire even when no Twin/folder is open (e.g. a
        // lone in-memory draft restored from autosave). Draining them
        // after the early return silently dropped every rename and
        // close click in that state.
        if let Some(intent) = doc_begin_rename {
            self.rename_doc = Some(intent);
        }
        if let Some((doc, new_name)) = doc_submit {
            self.rename_doc = None;
            let new_name = new_name.trim().to_string();
            if !new_name.is_empty() {
                ctx.trigger(RenameOpenDocument {
                    doc_id: doc,
                    new_name,
                });
            }
        }
        if doc_cancel {
            self.rename_doc = None;
        }
        if let Some(doc) = doc_close {
            // Cancel any in-flight rename on the doc we're closing.
            if self.rename_doc.as_ref().map(|r| r.doc) == Some(doc) {
                self.rename_doc = None;
            }
            ctx.actions.push(BrowserAction::CloseDoc { doc });
        }

        // The tree belongs to the active Twin. Other open Twins remain workspace
        // state, but their files must not be presented as current or routed
        // through the active Twin's path resolver.
        let active_twin = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_twin);

        // Open-document markers: which on-disk paths have an open editor tab,
        // and which of those are dirty (never-saved this session). A file row
        // whose absolute path matches an open doc gets a marker so the user can
        // see "what am I editing right now" directly in the tree — the
        // standalone list above covers only documents outside this tree.
        let mut open_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut unsaved_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        if let Some(ws) = ctx.resource::<lunco_workspace::WorkspaceResource>() {
            // id → is_unsaved from the cross-domain UnsavedDocs projection.
            let unsaved_ids: std::collections::HashSet<lunco_doc::DocumentId> =
                docs.iter().filter(|d| d.is_unsaved).map(|d| d.id).collect();
            for entry in ws.documents() {
                if !ws.document_is_in_active_scope(entry) {
                    continue;
                }
                if let Some(path) = entry.origin.canonical_path() {
                    open_paths.insert(path.to_path_buf());
                    if unsaved_ids.contains(&entry.id) {
                        unsaved_paths.insert(path.to_path_buf());
                    }
                }
            }
        }

        if active_twin.is_none() {
            let message = if docs.is_empty() {
                "Open a Twin or folder to browse files."
            } else {
                "No active Twin. The files above are loose documents."
            };
            ui.label(egui::RichText::new(message).weak().italics());
            return;
        }

        // Divider only appears between the workspace docs and the
        // folder list — if either is empty, no line to draw.
        if !standalone_ids.is_empty() {
            ui.separator();
        }

        let twins: Vec<(lunco_workspace::TwinId, &lunco_twin::Twin)> = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| {
                ws.active_twin
                    .and_then(|id| ws.twin(id).map(|twin| (id, twin)))
            })
            .into_iter()
            .collect();

        // Per-frame queues. Single-click previews source-only files and USD
        // documents; double-click still enters other registered domain
        // editors, and Rename remains available from the row context menu.
        // Enter on a rename TextEdit queues a `RenameTwinEntry` command. We
        // accumulate inside the nested egui closures (which can't
        // re-borrow `ctx.world` / `ctx.actions` while the closure
        // borrows `self.rename`), then dispatch in one pass after the
        // closures return. Same pattern the click buffer used.
        let mut clicks: Vec<(std::path::PathBuf, std::path::PathBuf, bool)> = Vec::new();
        let mut begin_rename: Option<RenameInProgress> = None;
        let mut submit_rename: Option<RenameInProgress> = None;
        let mut cancel_rename = false;

        let active_rename_abs: Option<std::path::PathBuf> =
            self.rename.as_ref().map(|r| r.target_abs.clone());

        for (twin_id, twin) in &twins {
            let folder_name = twin
                .manifest
                .as_ref()
                .map(|manifest| manifest.name.clone())
                .unwrap_or_else(|| {
                    twin.root
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| twin.root.to_string_lossy().to_string())
                });
            // Mark the workspace's focused Twin so it's clear which one the
            // editor tabs belong to. `●` matches the loaded-scene marker.
            let active_marker = if Some(*twin_id) == active_twin {
                "● "
            } else {
                ""
            };
            let header_label = format!("{active_marker}{folder_name}");
            let hover_path = twin.root.to_string_lossy().into_owned();
            let salt = twin.root.to_string_lossy().into_owned();
            let twin_root = twin.root.clone();
            let id = ui.make_persistent_id(("twin_browser_folder", salt.clone()));
            lunco_workbench_widgets::tree::branch(
                ui,
                id,
                true,
                None,
                |ui| {
                    let width = ui.available_width();
                    lunco_workbench_widgets::tree::label(
                        ui,
                        header_label,
                        width,
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(hover_path.as_str())
                    .clicked()
                },
                |ui| {
                    let Some(view) = ctx
                        .resource::<FileTreeView>()
                        .filter(|view| view.twin == Some(*twin_id) && view.query == query.text)
                    else {
                        ui.label("Preparing Twin files…");
                        return;
                    };
                    let tree = Arc::clone(&view.tree);
                    if tree.files.is_empty() && tree.subdirs.is_empty() {
                        let message = if query.is_active() {
                            "No matching files."
                        } else {
                            "(empty)"
                        };
                        ui.label(egui::RichText::new(message).weak().italics());
                        return;
                    }
                    // Render the directory tree DIRECTLY — no inner
                    // ScrollArea. The panel-level `twin_panel_scroll`
                    // (twin_browser/mod.rs) already scrolls everything.
                    // A vertical ScrollArea nested inside that outer one
                    // gets squished: deep in the outer viewport its
                    // available height is mostly consumed, so a long
                    // file list collapsed to a few rows behind its own
                    // scrollbar — "tons of files but can't see them".
                    // Closed tree branches still skip their contents,
                    // so render cost scales with *expanded* entries.
                    let mut render_state = TreeRenderState {
                        twin_root: &twin_root,
                        active_rename_abs: active_rename_abs.as_deref(),
                        rename: &mut self.rename,
                        clicks: &mut clicks,
                        begin_rename: &mut begin_rename,
                        submit_rename: &mut submit_rename,
                        cancel_rename: &mut cancel_rename,
                        open_paths: &open_paths,
                        unsaved_paths: &unsaved_paths,
                        searching: query.is_active(),
                    };
                    render_dir(&tree, std::path::Path::new(""), &mut render_state, ui);
                },
            );
        }

        // Dispatch queued intents now that the egui closures have
        // released their borrows on `self` and `ctx`. USD has a native
        // preview, so one click enters the domain-owned `BrowserAction`
        // route. Other registered document kinds still require a deliberate
        // double-click. If this app does not provide a domain for an
        // extension, retain the raw-source behaviour so the file never
        // becomes a dead row in the browser.
        for (twin_root, relative_path, pinned) in clicks {
            if should_open_as_document(
                &relative_path,
                ctx.resource::<lunco_twin::DocumentKindRegistry>(),
                pinned,
            ) {
                // Preserve the Twin that emitted the click. BrowserAction's
                // absolute form is the existing cross-Twin contract; using
                // it here prevents the dispatcher from silently anchoring an
                // inactive Twin's file on the active Twin.
                ctx.actions.push(BrowserAction::OpenFile {
                    relative_path: twin_root.join(relative_path),
                });
            } else {
                ctx.trigger(OpenTwinSource {
                    twin_root: twin_root.to_string_lossy().into_owned(),
                    relative_path: relative_path.to_string_lossy().into_owned(),
                    pinned,
                    focus: None,
                });
            }
        }
        if let Some(intent) = begin_rename {
            self.rename = Some(intent);
        }
        if let Some(req) = submit_rename {
            self.rename = None;
            // Skip the round-trip if the user didn't actually change
            // anything — saves a no-op on-disk rename + Twin reload.
            let old_leaf = req
                .relative_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let new_name = req.buffer.trim().to_string();
            if !new_name.is_empty() && new_name != old_leaf {
                ctx.trigger(RenameTwinEntry {
                    twin_root: req.twin_root.to_string_lossy().into_owned(),
                    relative_path: req.relative_path.to_string_lossy().into_owned(),
                    new_name,
                });
            }
        }
        if cancel_rename {
            self.rename = None;
        }
    }
}

/// A USD file has a native preview and therefore opens on the first click.
/// Other domain editors still require a deliberate double-click. Every
/// document route remains gated on the host's registry so standalone Lunica
/// keeps its source-viewer behaviour for unclaimed files.
pub(crate) fn should_open_as_document(
    relative_path: &std::path::Path,
    registry: Option<&lunco_twin::DocumentKindRegistry>,
    pinned: bool,
) -> bool {
    let registered = registry.is_some_and(|registry| registry.classify(relative_path).is_some());
    let is_usd = relative_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "usd" | "usda" | "usdc"
            )
        });
    registered && !is_source_only_text_path(relative_path) && (pinned || is_usd)
}

/// Recursively render one directory of the Twin's filesystem tree.
///
/// Directories render as shared tree branches with the folder label;
/// files render as `selectable_label`s. Both support double-click to
/// enter inline rename mode and Enter/Esc to submit/cancel. Files
/// additionally support single-click to open the editable source panel.
///
/// All mutation lands on the caller-owned queues (`clicks`,
/// `begin_rename`, …) so the egui closures can stay shallow; the
/// caller drains them after the egui pass completes.
fn render_dir(
    node: &PathTree<lunco_twin::FileEntry>,
    rel_prefix: &std::path::Path,
    state: &mut TreeRenderState<'_>,
    ui: &mut egui::Ui,
) {
    let twin_root = state.twin_root;
    // Directories first, alphabetical.
    for (dir_name, sub) in &node.subdirs {
        let rel = rel_prefix.join(dir_name);
        let abs = twin_root.join(&rel);
        let in_rename = state.active_rename_abs == Some(abs.as_path());

        if in_rename {
            render_inline_rename(
                ui,
                &abs,
                state.rename,
                state.submit_rename,
                state.cancel_rename,
            );
        } else {
            let id = ui.make_persistent_id(("twin_browser_dir", abs.to_string_lossy()));
            let mut header_begin_rename = None;
            lunco_workbench_widgets::tree::branch(
                ui,
                id,
                false,
                state.searching.then_some(true),
                |ui| {
                    let width = ui.available_width();
                    let resp = lunco_workbench_widgets::tree::label(
                        ui,
                        dir_name,
                        width,
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                    // Header double-click → enter rename mode for this directory.
                    if resp.double_clicked() {
                        header_begin_rename = Some(RenameInProgress {
                            target_abs: abs.clone(),
                            twin_root: twin_root.to_path_buf(),
                            relative_path: rel.clone(),
                            buffer: dir_name.clone(),
                            needs_focus: true,
                        });
                    }
                    resp.clicked()
                },
                |ui| {
                    render_dir(sub, &rel, state, ui);
                },
            );
            if header_begin_rename.is_some() {
                *state.begin_rename = header_begin_rename;
            }
        }
    }

    // Then files in this directory, alphabetical by leaf name.
    for entry in &node.files {
        let abs = twin_root.join(&entry.relative_path);
        let in_rename = state.active_rename_abs == Some(abs.as_path());

        if in_rename {
            render_inline_rename(
                ui,
                &abs,
                state.rename,
                state.submit_rename,
                state.cancel_rename,
            );
        } else {
            let leaf = entry
                .relative_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            // Status marker so the tree reads the Twin's current editing
            // state at a glance: a dirty dot (•) when the file is open AND
            // has unsaved edits, a hollow marker (○) when open but clean.
            // Matches the workspace-doc list's dirty-dot convention above.
            // Build the label from a borrow of `leaf` so `leaf` stays owned
            // for the rename buffer below.
            let marker = if state.unsaved_paths.contains(&abs) {
                Some("• ")
            } else if state.open_paths.contains(&abs) {
                Some("○ ")
            } else {
                None
            };
            let label: String = match marker {
                Some(m) => format!("{m}{leaf}"),
                None => leaf.clone(),
            };
            let r = lunco_workbench_widgets::tree::leaf(ui, |ui| {
                let width = ui.available_width();
                lunco_workbench_widgets::tree::selectable_label(ui, false, label.as_str(), width)
            })
            .inner
            .interact(egui::Sense::click_and_drag());
            r.dnd_set_drag_payload(lunco_workbench_core::source::SourceDragPayload {
                source: abs.to_string_lossy().replace('\\', "/"),
            });
            if r.double_clicked() {
                state
                    .clicks
                    .push((twin_root.to_path_buf(), entry.relative_path.clone(), true));
            } else if r.clicked() {
                state
                    .clicks
                    .push((twin_root.to_path_buf(), entry.relative_path.clone(), false));
            }
            r.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    ui.close();
                    *state.begin_rename = Some(RenameInProgress {
                        target_abs: abs.clone(),
                        twin_root: twin_root.to_path_buf(),
                        relative_path: entry.relative_path.clone(),
                        buffer: leaf.clone(),
                        needs_focus: true,
                    });
                }
            });
        }
    }
}

/// Mutable queues and immutable projections shared by the recursive Twin-file
/// tree renderer. Keeping them in one owner makes the renderer composable
/// without a long parameter list or hidden global state.
struct TreeRenderState<'a> {
    searching: bool,
    twin_root: &'a std::path::Path,
    active_rename_abs: Option<&'a std::path::Path>,
    rename: &'a mut Option<RenameInProgress>,
    clicks: &'a mut Vec<(std::path::PathBuf, std::path::PathBuf, bool)>,
    begin_rename: &'a mut Option<RenameInProgress>,
    submit_rename: &'a mut Option<RenameInProgress>,
    cancel_rename: &'a mut bool,
    open_paths: &'a std::collections::HashSet<std::path::PathBuf>,
    unsaved_paths: &'a std::collections::HashSet<std::path::PathBuf>,
}

/// Paint the inline rename `TextEdit` for one row (file or directory).
/// Drives `submit_rename` (Enter) / `cancel_rename` (Esc or blur).
fn render_inline_rename(
    ui: &mut egui::Ui,
    target_abs: &std::path::Path,
    rename: &mut Option<RenameInProgress>,
    submit_rename: &mut Option<RenameInProgress>,
    cancel_rename: &mut bool,
) {
    let Some(state) = rename.as_mut() else { return };
    if state.target_abs != target_abs {
        return;
    }
    let resp = ui.add(
        lunco_workbench_widgets::text_editor::singleline(&mut state.buffer)
            .desired_width(f32::INFINITY),
    );
    if state.needs_focus {
        resp.request_focus();
        state.needs_focus = false;
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if enter {
        *submit_rename = Some(RenameInProgress {
            target_abs: state.target_abs.clone(),
            twin_root: state.twin_root.clone(),
            relative_path: state.relative_path.clone(),
            buffer: state.buffer.clone(),
            needs_focus: false,
        });
    } else if esc || (resp.lost_focus() && !enter) {
        *cancel_rename = true;
    }
}

#[cfg(test)]
mod tests {
    use super::should_open_as_document;
    use lunco_twin::{DocumentKindId, DocumentKindMeta, DocumentKindRegistry};
    use std::path::Path;

    fn usd_registry() -> DocumentKindRegistry {
        let mut registry = DocumentKindRegistry::default();
        registry.register(
            DocumentKindId::new("usd"),
            DocumentKindMeta {
                display_name: "USD".into(),
                extensions: vec!["usd", "usda", "usdc"],
                ..Default::default()
            },
        );
        registry
    }

    #[test]
    fn double_click_routes_a_registered_usd_document() {
        assert!(should_open_as_document(
            Path::new("scenes/rover.usda"),
            Some(&usd_registry()),
            true,
        ));
    }

    #[test]
    fn single_click_routes_a_registered_usd_document() {
        for path in ["scenes/rover.usda", "scenes/rover.usd", "scenes/rover.usdc"] {
            assert!(should_open_as_document(
                Path::new(path),
                Some(&usd_registry()),
                false,
            ));
        }
    }

    #[test]
    fn single_click_keeps_modelica_on_the_source_viewer() {
        let mut registry = DocumentKindRegistry::default();
        registry.register(
            DocumentKindId::new("modelica"),
            DocumentKindMeta {
                display_name: "Modelica".into(),
                extensions: vec!["mo"],
                ..Default::default()
            },
        );
        assert!(!should_open_as_document(
            Path::new("models/rover.mo"),
            Some(&registry),
            false,
        ));
    }

    #[test]
    fn an_unclaimed_usd_file_stays_openable_as_source() {
        let registry = DocumentKindRegistry::default();
        assert!(!should_open_as_document(
            Path::new("scenes/rover.usda"),
            Some(&registry),
            true,
        ));
    }

    #[test]
    fn source_only_text_always_uses_the_source_viewer() {
        let mut registry = DocumentKindRegistry::default();
        registry.register(
            DocumentKindId::new("shader"),
            DocumentKindMeta {
                display_name: "Shader".into(),
                extensions: vec!["wesl"],
                ..Default::default()
            },
        );
        registry.register(
            DocumentKindId::new("rhai"),
            DocumentKindMeta {
                display_name: "Rhai".into(),
                extensions: vec!["rhai"],
                ..Default::default()
            },
        );
        assert!(!should_open_as_document(
            Path::new("shaders/terrain.wesl"),
            Some(&registry),
            true,
        ));
        assert!(!should_open_as_document(
            Path::new("scripts/teleop_policy.rhai"),
            Some(&registry),
            true,
        ));
    }
}
