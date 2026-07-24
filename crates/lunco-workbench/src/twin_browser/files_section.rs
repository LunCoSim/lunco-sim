//! Built-in **Files** section — flat, domain-agnostic listing of every
//! file the [`lunco_twin::Twin`] indexer found.
//!
//! Always present in the Twin Browser. Defaults to *collapsed* because
//! the per-domain sections (Modelica, USD, …) are usually what the
//! user wants; Files is the escape hatch for "show me the raw layout."
//!
//! Click a row → emits [`super::BrowserAction::OpenFile`]. The host
//! app's domain dispatchers decide what "open" means per file kind
//! (Modelica → diagram tab, USD → stage tab, image → external viewer,
//! …). The Files section itself is intentionally dumb about file
//! semantics.

use bevy_egui::egui;

use super::{
    path_tree::{build_path_tree, PathTree},
    BrowserAction, BrowserCtx, BrowserScope, BrowserSection,
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
fn build_tree(files: &[lunco_twin::FileEntry]) -> PathTree<&lunco_twin::FileEntry> {
    build_path_tree(
        files
            .iter()
            .map(|file| (file.relative_path.as_path(), file)),
    )
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
        "lunco.workbench.files"
    }

    fn title(&self) -> &str {
        "Files"
    }

    fn scope(&self) -> BrowserScope {
        // The Files section IS the Files tab — domain-agnostic raw FS
        // view. The Models tab is reserved for typed-content sections
        // contributed by domain crates.
        BrowserScope::Files
    }

    fn default_open(&self) -> bool {
        // Inside the Files tab the section is the only one and should
        // be expanded by default — there's no domain section above to
        // anchor the user's eye.
        true
    }

    fn order(&self) -> u32 {
        // Renders below Modelica (100) in the unified Twin panel; the
        // standalone FilesPanel (when summoned) shows the same section.
        200
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        // Render workspace documents (saved + unsaved) so the list
        // stays stable across Save — a Save shouldn't make a doc
        // disappear from the user's view of "what am I working on."
        // Unsaved drafts get a dirty dot in the theme warning colour
        // plus an italic name; saved docs render plain. Kind badges
        // are intentionally omitted — file extensions in the display
        // name carry that information for the user.
        let docs: Vec<super::UnsavedDocEntry> = ctx
            .resource::<super::UnsavedDocs>()
            .map(|r| r.entries.clone())
            .unwrap_or_default();
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

        for entry in &docs {
            let in_rename = self
                .rename_doc
                .as_ref()
                .map(|r| r.doc == entry.id)
                .unwrap_or(false);
            ui.horizontal(|ui| {
                if entry.is_unsaved {
                    ui.label(egui::RichText::new("•").color(dirty_dot_color).size(8.0));
                } else {
                    ui.label(egui::RichText::new("  "));
                }
                if in_rename {
                    let state = self
                        .rename_doc
                        .as_mut()
                        .expect("in_rename ⇒ rename_doc Some");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut state.buffer).desired_width(f32::INFINITY),
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
                    // Close (✕) control — closes the document, its
                    // tabs, and (on wasm) its localStorage autosave
                    // entry. Without this a restored draft has no
                    // delete path from the UI and resurrects on every
                    // reload. Right-aligned so it doesn't crowd names.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = ui
                            .add(
                                egui::Button::new(egui::RichText::new("✕").size(10.0))
                                    .frame(false)
                                    .small(),
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
                ctx.trigger(super::super::file_ops::RenameOpenDocument { doc, new_name });
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

        // Read every open Twin from the workspace resource. Keep the
        // `(TwinId, &Twin)` pairs so each header can be marked active, and the
        // active id is comparable. `&Twin` borrows from `ctx` for the render
        // loop below; the borrow ends before the post-loop dispatch (which
        // re-borrows `ctx` mutably via `actions`/`trigger`), so NLL keeps both
        // happy. Twin refs are cheap (just `&Twin`).
        let ws_state = ctx.resource::<crate::WorkspaceResource>().map(|ws| {
            (
                ws.active_twin,
                ws.twins()
                    .map(|(id, t)| (id, t))
                    .collect::<Vec<(lunco_workspace::TwinId, &lunco_twin::Twin)>>(),
            )
        });
        let active_twin = ws_state.as_ref().and_then(|(a, _)| *a);
        let twins: Vec<(lunco_workspace::TwinId, &lunco_twin::Twin)> =
            ws_state.map(|(_, ts)| ts).unwrap_or_default();

        // Open-document markers: which on-disk paths have an open editor tab,
        // and which of those are dirty (never-saved this session). A file row
        // whose absolute path matches an open doc gets a marker so the user can
        // see "what am I editing right now" directly in the tree — the
        // workspace-doc list above shows the same docs, but keyed by tab, not
        // by location in the Twin's folders.
        let mut open_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut unsaved_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        if let Some(ws) = ctx.resource::<crate::WorkspaceResource>() {
            // id → is_unsaved from the cross-domain UnsavedDocs projection.
            let unsaved_ids: std::collections::HashSet<lunco_doc::DocumentId> =
                docs.iter().filter(|d| d.is_unsaved).map(|d| d.id).collect();
            for entry in ws.documents() {
                if let Some(path) = entry.origin.canonical_path() {
                    open_paths.insert(path.to_path_buf());
                    if unsaved_ids.contains(&entry.id) {
                        unsaved_paths.insert(path.to_path_buf());
                    }
                }
            }
        }

        if twins.is_empty() {
            if docs.is_empty() {
                ui.label(
                    egui::RichText::new("Open a Twin or folder to browse files.")
                        .weak()
                        .italics(),
                );
            }
            return;
        }

        // Divider only appears between the workspace docs and the
        // folder list — if either is empty, no line to draw.
        if !docs.is_empty() {
            ui.separator();
        }

        // Per-frame queues. Single-click on a row queues an `OpenFile`
        // action; double-click queues a "begin rename" intent; Enter on
        // a rename TextEdit queues a `RenameTwinEntry` command. We
        // accumulate inside the nested egui closures (which can't
        // re-borrow `ctx.world` / `ctx.actions` while the closure
        // borrows `self.rename`), then dispatch in one pass after the
        // closures return. Same pattern the click buffer used.
        let mut clicks: Vec<std::path::PathBuf> = Vec::new();
        let mut begin_rename: Option<RenameInProgress> = None;
        let mut submit_rename: Option<RenameInProgress> = None;
        let mut cancel_rename = false;

        let active_rename_abs: Option<std::path::PathBuf> =
            self.rename.as_ref().map(|r| r.target_abs.clone());

        for (twin_id, twin) in &twins {
            let folder_name = twin
                .root
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| twin.root.to_string_lossy().to_string());
            // Mark the workspace's focused Twin so it's clear which one the
            // editor tabs belong to. `●` matches the loaded-scene marker.
            let active_marker = if Some(*twin_id) == active_twin {
                "● "
            } else {
                ""
            };
            let header_label = format!("📁  {active_marker}{folder_name}");
            let hover_path = twin.root.to_string_lossy().into_owned();
            let salt = twin.root.to_string_lossy().into_owned();
            let twin_root = twin.root.clone();
            let resp = egui::CollapsingHeader::new(header_label)
                .id_salt(("twin_browser_folder", salt.clone()))
                .default_open(true)
                .show(ui, |ui| {
                    let files = twin.files();
                    if files.is_empty() {
                        ui.label(egui::RichText::new("(empty)").weak().italics().small());
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
                    // Closed CollapsingHeaders still skip their contents,
                    // so render cost scales with *expanded* entries.
                    let tree = build_tree(files);
                    render_dir(
                        &tree,
                        std::path::Path::new(""),
                        &twin_root,
                        active_rename_abs.as_deref(),
                        &mut self.rename,
                        &mut clicks,
                        &mut begin_rename,
                        &mut submit_rename,
                        &mut cancel_rename,
                        &open_paths,
                        &unsaved_paths,
                        ui,
                    );
                });
            resp.header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(hover_path);
        }

        // Dispatch queued intents now that the egui closures have
        // released their borrows on `self` and `ctx`.
        for relative_path in clicks {
            ctx.actions.push(BrowserAction::OpenFile { relative_path });
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
                ctx.trigger(super::super::file_ops::RenameTwinEntry {
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

/// Recursively render one directory of the Twin's filesystem tree.
///
/// Directories render as `CollapsingHeader`s with the folder icon;
/// files render as `selectable_label`s. Both support double-click to
/// enter inline rename mode and Enter/Esc to submit/cancel. Files
/// additionally support single-click to dispatch `BrowserAction::OpenFile`.
///
/// All mutation lands on the caller-owned queues (`clicks`,
/// `begin_rename`, …) so the egui closures can stay shallow; the
/// caller drains them after the egui pass completes.
#[allow(clippy::too_many_arguments)]
fn render_dir(
    node: &PathTree<&lunco_twin::FileEntry>,
    rel_prefix: &std::path::Path,
    twin_root: &std::path::Path,
    active_rename_abs: Option<&std::path::Path>,
    rename: &mut Option<RenameInProgress>,
    clicks: &mut Vec<std::path::PathBuf>,
    begin_rename: &mut Option<RenameInProgress>,
    submit_rename: &mut Option<RenameInProgress>,
    cancel_rename: &mut bool,
    open_paths: &std::collections::HashSet<std::path::PathBuf>,
    unsaved_paths: &std::collections::HashSet<std::path::PathBuf>,
    ui: &mut egui::Ui,
) {
    // Directories first, alphabetical.
    for (dir_name, sub) in &node.subdirs {
        let rel = rel_prefix.join(dir_name);
        let abs = twin_root.join(&rel);
        let in_rename = active_rename_abs == Some(abs.as_path());

        if in_rename {
            render_inline_rename(ui, &abs, rename, submit_rename, cancel_rename);
        } else {
            let salt = abs.to_string_lossy().into_owned();
            let header = egui::CollapsingHeader::new(format!("📁 {}", dir_name))
                .id_salt(("twin_browser_dir", salt))
                .default_open(false);
            let resp = header.show(ui, |ui| {
                render_dir(
                    sub,
                    &rel,
                    twin_root,
                    active_rename_abs,
                    rename,
                    clicks,
                    begin_rename,
                    submit_rename,
                    cancel_rename,
                    open_paths,
                    unsaved_paths,
                    ui,
                );
            });
            // Header double-click → enter rename mode for this directory.
            // CollapsingHeader's single click toggles open/closed (egui
            // default), so single-click here is intentionally ignored.
            if resp.header_response.double_clicked() {
                *begin_rename = Some(RenameInProgress {
                    target_abs: abs.clone(),
                    twin_root: twin_root.to_path_buf(),
                    relative_path: rel.clone(),
                    buffer: dir_name.clone(),
                    needs_focus: true,
                });
            }
        }
    }

    // Then files in this directory, alphabetical by leaf name.
    let mut files: Vec<&lunco_twin::FileEntry> = node.files.clone();
    files.sort_by(|a, b| {
        a.relative_path
            .file_name()
            .cmp(&b.relative_path.file_name())
    });
    for entry in files {
        let abs = twin_root.join(&entry.relative_path);
        let in_rename = active_rename_abs == Some(abs.as_path());

        if in_rename {
            render_inline_rename(ui, &abs, rename, submit_rename, cancel_rename);
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
            let marker = if unsaved_paths.contains(&abs) {
                Some("• ")
            } else if open_paths.contains(&abs) {
                Some("○ ")
            } else {
                None
            };
            let label: String = match marker {
                Some(m) => format!("{m}{leaf}"),
                None => leaf.clone(),
            };
            let r = ui.selectable_label(false, &label);
            if r.double_clicked() {
                *begin_rename = Some(RenameInProgress {
                    target_abs: abs.clone(),
                    twin_root: twin_root.to_path_buf(),
                    relative_path: entry.relative_path.clone(),
                    buffer: leaf,
                    needs_focus: true,
                });
            } else if r.clicked() {
                clicks.push(entry.relative_path.clone());
            }
        }
    }
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
    let resp = ui.add(egui::TextEdit::singleline(&mut state.buffer).desired_width(f32::INFINITY));
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
