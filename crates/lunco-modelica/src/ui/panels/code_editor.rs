//! Code Editor panel — central viewport for Modelica source code.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use std::collections::HashMap;
use std::sync::Arc;

use crate::state::{ModelicaDocumentRegistry, WorkbenchState};

/// Per-tab editor buffer snapshot. Stashed in `EditorBufferState.per_doc`
/// when the user switches tabs and restored when they switch back, so
/// in-progress edits survive tab navigation. Without this each tab
/// switch re-pulled the doc source and clobbered any
/// uncommitted typing.
///
/// Lives keyed by `model_path` (the same string `EditorBufferState`
/// carries to identify "which doc this buffer belongs to") rather
/// than `DocumentId` so the rest of the editor's plumbing — which
/// already passes `model_path` around — doesn't need to change.
#[derive(Default, Clone)]
pub struct TabBuffer {
    pub generation: u64,
    pub text: String,
    pub detected_name: Option<String>,
    pub cached_galley: Option<Arc<egui::Galley>>,
    pub pending_commit_at: Option<f64>,
}

/// One-shot edit verbs forwarded from the global Edit menu. The
/// menu callback (registered in `ui/mod.rs::register_edit_menu`)
/// flips the matching field; the code-editor render reads and
/// clears it on the next frame, OR-merging into the same
/// `tb_copy/tb_cut/tb_paste/tb_select_all` flags the in-panel
/// toolbar uses. Keeps all clipboard/selection logic in one place
/// while letting the menu drive it.
#[derive(Resource, Default)]
pub struct CodeEditorMenuRequest {
    pub copy: bool,
    pub cut: bool,
    pub paste: bool,
    pub select_all: bool,
}

/// Tracks which model the editor buffer belongs to, to detect model switches.
#[derive(Resource)]
pub struct EditorBufferState {
    /// Document generation that was last loaded into editor_buffer.
    /// B.3 phase 6 replacement for `source_hash` — comparing `u64`
    /// from the registry is faster and more authoritative than
    /// hashing the whole buffer.
    pub generation: u64,
    /// Typed doc identity for the current buffer contents. Single
    /// source of truth for "which doc owns the live editor fields".
    /// Tab-switch detection compares this against the tab's target
    /// doc; per-doc snapshots are keyed by it. PR14 (2026-05-12)
    /// removed the parallel `model_path: String` field that derived
    /// the same identity from `canonical_path() / mem://{name}` —
    /// it diverged across Save-As (untitled → file) and dropped
    /// uncommitted edits.
    pub bound_doc: Option<lunco_doc::DocumentId>,
    /// The actual text content (persistent across frames for selection).
    pub text: String,
    /// Memoized detected name to avoid per-frame regex on large strings.
    pub detected_name: Option<String>,
    /// Pre-computed text layout for high-performance rendering.
    pub cached_galley: Option<Arc<egui::Galley>>,
    /// When `true` long lines wrap at the editor width; when `false`
    /// (default) long lines stay on one line and the view scrolls
    /// horizontally — mirroring VS Code's default "Word Wrap: Off".
    pub word_wrap: bool,
    /// When `true` (default), pressing Enter copies the previous
    /// line's leading whitespace onto the new line — i.e. continues
    /// at the same indent level. Mirrors the default behaviour of
    /// every modern code editor (VS Code, Sublime, IntelliJ, Emacs
    /// electric-indent). Turn off for plain-text-buffer behaviour.
    pub auto_indent: bool,
    /// egui timestamp (in seconds, from `ctx.input(|i| i.time)`) of
    /// the last keystroke that diverged `text` from the bound
    /// document's source. `None` when the buffer is already in sync.
    ///
    /// Used to debounce the text→document commit so the diagram / AST
    /// sync fires on an idle gap rather than on every keystroke —
    /// VS Code, rust-analyzer, and every LSP client do the same
    /// thing. Each keystroke refreshes this timestamp; once the idle
    /// threshold (`EDIT_DEBOUNCE_SEC`) has elapsed without further
    /// typing, the buffer is checkpointed into the document in one
    /// shot.
    pub pending_commit_at: Option<f64>,
    /// Per-tab buffer snapshots keyed by [`lunco_doc::DocumentId`].
    /// Save on tab switch (current live fields → `per_doc[old]`),
    /// restore on tab switch back (`per_doc[new]` → live fields).
    /// Without this, switching to a new tab clobbered any
    /// uncommitted edits in the previous tab — the buffer is a
    /// singleton bound to egui's `TextEdit`, so two tabs can't share
    /// it without an off-screen save slot.
    pub per_doc: HashMap<lunco_doc::DocumentId, TabBuffer>,
}

/// One-shot request to move the editor caret to a source position,
/// raised by the Diagnostics panel when a located finding is clicked
/// and consumed by [`CodeEditorPanel::render`] on the next frame.
///
/// `doc` is the document the position belongs to (the active doc at
/// click time). The editor applies the jump only when that doc is the
/// tab it's currently showing, so a stale request can't scroll the
/// wrong file. Unconditionally drained after one read either way.
#[derive(Resource, Default)]
pub struct EditorJumpRequest {
    pending: Option<(Option<lunco_doc::DocumentId>, crate::ui::panels::log::SourceLoc)>,
}

impl EditorJumpRequest {
    /// Queue a jump to `loc` within `doc`.
    pub fn request(
        &mut self,
        doc: Option<lunco_doc::DocumentId>,
        loc: crate::ui::panels::log::SourceLoc,
    ) {
        self.pending = Some((doc, loc));
    }

    /// Take the queued jump, leaving the request empty.
    fn take(
        &mut self,
    ) -> Option<(Option<lunco_doc::DocumentId>, crate::ui::panels::log::SourceLoc)> {
        self.pending.take()
    }
}

/// Convert a 1-based (line, column) into a char offset within `text`,
/// clamped to the buffer. Columns past a line's end clamp to its end;
/// lines past EOF clamp to the last position. egui cursors index by
/// `char`, so we count chars (not bytes) — correct for non-ASCII
/// source.
fn line_col_to_char_offset(text: &str, line: u32, column: u32) -> usize {
    let target_line = line.saturating_sub(1) as usize;
    let target_col = column.saturating_sub(1) as usize;
    let mut offset = 0usize;
    for (idx, l) in text.split_inclusive('\n').enumerate() {
        // chars in this line excluding the trailing '\n'
        let line_chars = l.chars().filter(|&c| c != '\n').count();
        if idx == target_line {
            return offset + target_col.min(line_chars);
        }
        offset += l.chars().count();
    }
    // Line beyond EOF — clamp to end of buffer.
    text.chars().count()
}

/// Return the byte index of the newline character when `new` differs
/// from `old` by exactly one inserted `'\n'`. Otherwise returns `None`.
///
/// Used by auto-indent to detect "user pressed Enter" edits without
/// hooking egui's input pipeline. We capture the pre-edit buffer,
/// let `TextEdit` run, and compare. Pastes, multi-char edits, and
/// anything that isn't a single `\n` insertion skip the auto-indent
/// pass.
fn detect_single_newline_insertion(old: &str, new: &str) -> Option<usize> {
    if new.len() != old.len() + 1 {
        return None;
    }
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let mut i = 0;
    while i < ob.len() && ob[i] == nb[i] {
        i += 1;
    }
    if i < nb.len() && nb[i] == b'\n' && ob.get(i..) == Some(&nb[i + 1..]) {
        Some(i)
    } else {
        None
    }
}

/// Idle window (seconds) after the last keystroke before the editor
/// commits its buffer into the document. Small enough to feel
/// responsive in the diagram view, long enough to coalesce a burst of
/// typing into a single `ReplaceSource` op (and therefore one undo
/// entry, not one per character).
pub const EDIT_DEBOUNCE_SEC: f64 = 0.35;

impl Default for EditorBufferState {
    fn default() -> Self {
        Self {
            generation: 0,
            bound_doc: None,
            text: String::new(),
            detected_name: None,
            cached_galley: None,
            word_wrap: false,
            auto_indent: true,
            pending_commit_at: None,
            per_doc: HashMap::new(),
        }
    }
}

impl EditorBufferState {
    /// Save the live fields into `per_doc[bound_doc]`. Call before
    /// overwriting them with another tab's content so uncommitted
    /// edits aren't lost. No-op when no doc is bound.
    pub fn snapshot_current(&mut self) {
        let Some(doc) = self.bound_doc else { return };
        self.per_doc.insert(
            doc,
            TabBuffer {
                generation: self.generation,
                text: self.text.clone(),
                detected_name: self.detected_name.clone(),
                cached_galley: self.cached_galley.clone(),
                pending_commit_at: self.pending_commit_at,
            },
        );
    }

    /// Pull a previously-saved snapshot for `doc` into the live
    /// fields. Returns `true` if a snapshot was found (caller can
    /// skip the registry-source re-sync), `false` otherwise.
    pub fn restore_snapshot(&mut self, doc: lunco_doc::DocumentId) -> bool {
        if let Some(snap) = self.per_doc.remove(&doc) {
            self.generation = snap.generation;
            self.text = snap.text;
            self.detected_name = snap.detected_name;
            self.cached_galley = snap.cached_galley;
            self.pending_commit_at = snap.pending_commit_at;
            self.bound_doc = Some(doc);
            true
        } else {
            false
        }
    }
}

/// Reacts to `DocumentChanged` to keep the live editor buffer in
/// step with the document's source without a per-frame poll.
/// Replaces the same-tab divergence check that used to live in
/// `CodeEditorPanel::render`.
///
/// `buf_state.generation` is the per-bound-doc watermark — no
/// separate resource needed. Snapshots for other tabs
/// (`EditorBufferState::per_doc`) stay frozen and reconcile on
/// tab-switch via the post-restore stale check in `render()`.
///
/// Pending typing: when `pending_commit_at.is_some()` we advance
/// the generation watermark but don't overwrite buffer text.
/// `commit_pending_buffer` will re-diff the still-typing buffer
/// against the new source on its next flush. Concurrent
/// structural edits during active typing are accepted as lost
/// (the user is staring at the text); merging both intents
/// requires per-keystroke ops, out of scope here.
pub fn editor_on_doc_changed(
    trigger: On<lunco_doc_bevy::DocumentChanged>,
    registry: Res<ModelicaDocumentRegistry>,
    mut buf_state: ResMut<EditorBufferState>,
) {
    let doc = trigger.event().doc;
    let bound = buf_state.bound_doc;
    if bound != Some(doc) {
        bevy::log::info!(
            "[editor-obs] skip: doc={} bound={:?}",
            doc.raw(),
            bound.map(|d| d.raw())
        );
        return;
    }
    let Some(host) = registry.host(doc) else { return };
    let current_gen = host.generation();
    let buf_gen = buf_state.generation;
    let pending = buf_state.pending_commit_at;
    bevy::log::info!(
        "[editor-obs] fire: doc={} current_gen={} buf_gen={} pending={:?}",
        doc.raw(),
        current_gen,
        buf_gen,
        pending,
    );
    if buf_gen == current_gen {
        return;
    }

    if pending.is_some() {
        buf_state.generation = current_gen;
        return;
    }

    bevy::log::warn!(
        "[editor-obs] RESYNC: doc={} buf_gen={} → current_gen={} — buf.text will be overwritten with doc.source",
        doc.raw(),
        buf_gen,
        current_gen,
    );

    let document = host.document();
    let src = document.source().to_string();
    let detected = document
        .index()
        .classes
        .values()
        .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
        .map(|c| c.name.clone());

    buf_state.text = src;
    buf_state.generation = current_gen;
    buf_state.detected_name = detected;
    buf_state.cached_galley = None;
}

/// Pull the live source for `path` from the document registry by
/// `doc` id and stuff it into `EditorBufferState`'s live fields.
/// Per-tab routing (split views) requires the doc-id keyed lookup —
/// the legacy active-doc-only variant returned
/// whichever tab rendered last.
fn sync_buffer_from_registry(world: &mut World, doc: lunco_doc::DocumentId) {
    let (source, detected_name, generation) = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc) else { return };
        let document = host.document();
        let src = document.source().to_string();
        // Cheap per-doc detected name from the AST index (no parse).
        let detected = document
            .index()
            .classes
            .values()
            .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
            .map(|c| c.name.clone());
        (src, detected, host.generation())
    };
    // Galley cache is layout-tied; can't pull it from the registry.
    // Letting it be None here forces a one-frame relayout — same
    // cost path tab-switch already takes. Cheap on text bodies.
    let mut buf_state = world.resource_mut::<EditorBufferState>();
    buf_state.text = source;
    buf_state.bound_doc = Some(doc);
    buf_state.generation = generation;
    buf_state.detected_name = detected_name;
    buf_state.cached_galley = None;
    // Fresh load from doc → no pending commit yet.
    buf_state.pending_commit_at = None;
}

/// Code Editor panel — central viewport for Modelica source code.
pub struct CodeEditorPanel;

impl Panel for CodeEditorPanel {
    fn id(&self) -> PanelId { PanelId("modelica_code_preview") }
    fn title(&self) -> String { "📝 Code Editor".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Center }
    fn closable(&self) -> bool { false }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // ── Ensure persistent buffer exists ──
        if world.get_resource::<EditorBufferState>().is_none() {
            world.insert_resource(EditorBufferState::default());
        }

        // ── Determine what model to show ──
        // Resolve *this tab's* doc via the per-render
        // [`TabRenderContext`] — splits each render in the same
        // frame, so reading the singleton the registry-by-doc lookup
        // would mirror whichever tab rendered last. Fall back to the
        // workspace-wide active doc for non-tab render paths
        // (welcome screen, no tab open).
        let tab_target: Option<lunco_doc::DocumentId> = world
            .get_resource::<crate::model_tabs_types::TabRenderContext>()
            .and_then(|c| c.doc)
            .or_else(|| {
                world
                    .get_resource::<lunco_workspace::WorkspaceResource>()
                    .and_then(|ws| ws.active_document)
            });
        // Pull display fields from the registry directly — this
        // bypasses the the registry-by-doc lookup snapshot which is
        // a singleton stamped by whichever tab rendered last.
        let (display_name, is_read_only, source_len) = match tab_target
            .and_then(|d| world.resource::<ModelicaDocumentRegistry>().host(d))
        {
            Some(host) => {
                let document = host.document();
                let display = document.origin().display_name();
                (
                    Some(display),
                    document.is_read_only(),
                    document.source().len(),
                )
            }
            None => (None, false, 0),
        };
        let (compilation_error, selected_entity, is_loading) = {
            let state = world.resource::<WorkbenchState>();
            let entity = state.selected_entity;
            // Any in-flight stage on the bus for this doc — covers
            // file-load, drill-in, duplicate, reparse. Same predicate
            // the canvas overlay uses; no per-panel loading-state
            // bookkeeping.
            let loading = source_len == 0
                && tab_target
                    .map(|d| {
                        world
                            .get_resource::<lunco_workbench::status_bus::StatusBus>()
                            .map(|bus| bus.is_busy(lunco_workbench::status_bus::BusyScope::Document(d.0)))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
            let err = tab_target.and_then(|d| {
                world
                    .get_resource::<lunco_doc_bevy::DocumentDiagnostics>()
                    .and_then(|cs| cs.error_message(d).map(str::to_string))
            });
            (err, entity, loading)
        };

        if is_loading {
            let muted = world
                .get_resource::<lunco_theme::Theme>()
                .map(|t| t.tokens.text_subdued)
                .unwrap_or(egui::Color32::GRAY);
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.spinner();
                ui.add_space(10.0);
                ui.heading("Opening model...");
                ui.label(egui::RichText::new("Reading from disk and indexing...").size(10.0).color(muted));
            });
            return;
        }

        // True when the user switched to a different model this frame.
        // Used below to snap the horizontal scroll back to column 0 so
        // the left edge of the new file is always visible — otherwise
        // egui's ScrollArea retains the previous file's scroll offset
        // (driven by cursor position) and the new file opens with its
        // first few columns hidden behind the left panel boundary.
        // Tab-switch handling. Same-tab content divergence is
        // pushed into `editor_on_doc_changed` (see this file) so
        // there's no per-frame generation poll here — render only
        // runs sync logic when the user actually moves between
        // tabs.
        let mut model_switched = false;
        if let Some(doc) = tab_target {
            let doc_changed = world.resource::<EditorBufferState>().bound_doc != Some(doc);
            model_switched = doc_changed;

            if doc_changed {
                let mut buf_state = world.resource_mut::<EditorBufferState>();
                buf_state.snapshot_current();
                let restored = buf_state.restore_snapshot(doc);
                drop(buf_state);
                // Restored snapshots may be stale if the doc was
                // mutated while this tab was hidden — the observer
                // only updates the *bound* tab. Compare against the
                // registry and fall through to a fresh pull when
                // they diverge. New-tab path falls straight through
                // to the same sync.
                let stale = {
                    let buf_gen = world.resource::<EditorBufferState>().generation;
                    let external_gen = world
                        .resource::<ModelicaDocumentRegistry>()
                        .host(doc)
                        .map(|h| h.generation())
                        .unwrap_or(0);
                    buf_gen != external_gen
                };
                if !restored || stale {
                    sync_buffer_from_registry(world, doc);
                }
            }
        }

        if tab_target.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.heading("📝 Code Editor");
                ui.add_space(20.0);
                ui.label("Click a model in the Package Browser to open it here.");
                ui.label("");
                ui.label("Or click \"➕ New Model\" to create one from scratch.");
            });
            return;
        }

        // Panel body only — the toolbar (view switch, compile, undo/redo,
        // status chip) is rendered by [`ModelViewPanel`], which also owns
        // the action handlers (see `dispatch_compile_from_buffer` /
        // `apply_document_undo` / `apply_document_redo` below).
        let _ = (display_name, compilation_error);

        // Resolve the DocumentId for the currently-shown model so the
        // focus-loss commit below writes into it. Active document
        // wins (this *is* a per-tab editor — it's looking at the
        // focused doc by definition); fall back to the legacy
        // `selected_entity → document_of(entity)` lookup only when
        // there's no active document, which covers the brief window
        // before workspace state is initialised.
        // Use this tab's resolved doc (TabRenderContext-aware) so a
        // split-Text edit commits into the right document.
        let doc_id = tab_target.or_else(|| {
            selected_entity
                .and_then(|e| world.resource::<ModelicaDocumentRegistry>().document_of(e))
        });

        // ── Settings menu (gear button) ──
        //
        // All editor preferences live inside one dropdown menu
        // rather than being scattered as inline toolbar toggles — the
        // panel toolbar stays tidy and this scales as we add more
        // options (font size, indent width, tab-vs-spaces, …)
        // without turning into a button pile.
        //
        // Current options, all persisted on `EditorBufferState`:
        //   • **Word wrap** (default off): long lines wrap at editor
        //     width when on, horizontal-scroll when off. Matches VS
        //     Code's default.
        //   • **Auto indent** (default on): pressing Enter copies
        //     the previous line's leading whitespace onto the new
        //     line. Every modern code editor does this.
        // Editor prefs (word wrap, auto indent) live in the app-wide
        // Settings menu — see `register_settings_menu` in `ui/mod.rs`.
        // Here we just read the current values.
        let (word_wrap, auto_indent) = {
            let buf = world.resource::<EditorBufferState>();
            (buf.word_wrap, buf.auto_indent)
        };

        // ── Editor area ──
        //
        // Single full-bleed TextEdit. I originally shipped a
        // line-number gutter next to this, but the gutter's nested
        // ScrollArea took unconstrained horizontal width and shoved
        // the editor to the right. Dropped the gutter until it can be
        // done properly (layouter-prefix or synchronized-scroll).
        // Full-width editor beats a half-broken gutter.
        let mut buffer_changed = false;
        let mut buffer_commit = false;
        let mut new_text = String::new();

        let avail = ui.available_size();

        // `text` must be a `&mut String` — egui's `TextBuffer` impl
        // for `&str` is read-only, so passing `&mut &str` to
        // `TextEdit::multiline` silently produces a non-editable
        // widget.
        //
        // Snapshot `old_text` before the TextEdit runs so the
        // auto-indent pass below can diff the pre/post buffer and
        // detect single-char newline insertions.
        let (mut text, galley_cache, old_text) = {
            let buf_state = world.resource::<EditorBufferState>();
            (
                buf_state.text.clone(),
                buf_state.cached_galley.clone(),
                buf_state.text.clone(),
            )
        };
        let is_ro = is_read_only;
        let editor_width = avail.x.max(100.0);
        let editor_height = avail.y.max(200.0);

        // When word-wrap is off, long lines must live inside a
        // horizontal `ScrollArea` so they can scroll rather than
        // clip. When word-wrap is on, the layouter does the right
        // thing at editor-width and no horizontal scroll is needed.
        // Stable id so the auto-indent cursor-reposition logic below
        // can look up and overwrite the TextEdit's state via
        // `TextEditState::load`. Without a pinned id, egui generates
        // a hash from the widget's location and we can't target it
        // from outside the closure.
        let text_edit_id = egui::Id::new("modelica_code_editor");
        // Snapshot the cursor range BEFORE TextEdit runs. We use it
        // to derive `selection_chars` so the toolbar / keyboard /
        // paste handlers know what range was selected at the start
        // of this frame, even after TextEdit's own pointer logic
        // mutates the cursor.
        let pre_cursor: Option<egui::text::CCursorRange> =
            egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                .and_then(|s| s.cursor.char_range());

        // ── No right-click context menu ──
        //
        // We don't ship a right-click menu inside the editor. Two
        // upstream egui issues make a usable one impossible without
        // forking the crate:
        //
        //   1. `TextEdit::pointer_interaction` collapses the active
        //      selection on every pointer press, including
        //      secondary, and the fields needed to suppress that
        //      (`PointerState.pointer_events` etc.) are
        //      `pub(crate)`. See egui issue #5382 ("Context menu on
        //      selected text") — open since Nov 2024 with no fix.
        //
        //   2. egui only paints the selection highlight while
        //      TextEdit owns focus (`paint_text_selection` gate at
        //      `text_edit/builder.rs:719`). Any popup steals focus
        //      to handle keyboard nav, so even after restoring the
        //      cursor range the highlight disappears.
        //
        // We tried snapshot/restore + force-focus + manual popup
        // rendering across several iterations; every patch exposed
        // the next layer of upstream behaviour. The toolbar below is
        // the pragmatic alternative — visible affordance for Copy /
        // Cut / Paste / Select all that doesn't fight egui. Keyboard
        // shortcuts (Ctrl/Cmd+C/X/V) keep working independently via
        // the wasm clipboard bridge + the keyboard handler further
        // down.
        let selection_chars: Option<(usize, usize)> = pre_cursor.and_then(|r| {
            let a = r.primary.index;
            let b = r.secondary.index;
            let (s, e) = (a.min(b), a.max(b));
            if e > s { Some((s, e)) } else { None }
        });
        let has_selection = selection_chars.is_some();

        let mut tb_copy = false;
        let mut tb_cut = false;
        let mut tb_paste = false;
        let mut tb_select_all = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .add_enabled(has_selection, egui::Button::new("Copy"))
                .on_hover_text("Ctrl/Cmd+C")
                .clicked()
            {
                tb_copy = true;
            }
            if ui
                .add_enabled(has_selection && !is_ro, egui::Button::new("Cut"))
                .on_hover_text("Ctrl/Cmd+X")
                .clicked()
            {
                tb_cut = true;
            }
            if ui
                .add_enabled(!is_ro, egui::Button::new("Paste"))
                .on_hover_text("Ctrl/Cmd+V (no permission prompt) or this button (prompts once)")
                .clicked()
            {
                tb_paste = true;
            }
            ui.separator();
            if ui.button("Select all").on_hover_text("Ctrl/Cmd+A").clicked() {
                tb_select_all = true;
            }
        });
        // Merge any one-shot request from the global Edit menu so the
        // menu and the toolbar share the exact same downstream code
        // path. Resource is initialised lazily — first menu click
        // creates it, which is fine because we only consume here.
        if let Some(mut req) = world.get_resource_mut::<CodeEditorMenuRequest>() {
            tb_copy |= req.copy;
            tb_cut |= req.cut;
            tb_paste |= req.paste;
            tb_select_all |= req.select_all;
            *req = CodeEditorMenuRequest::default();
        }
        ui.separator();
        // When word-wrap is off we need the TextEdit widget's rect to
        // be AT LEAST as wide as the longest line — otherwise egui's
        // TextEdit auto-scrolls horizontally *inside its own rect* to
        // follow the cursor, hiding the leftmost columns. Estimate
        // width from the longest line's char count × monospace glyph
        // width (cheap and good enough; overshooting is harmless, the
        // outer ScrollArea just gains a bit of empty runway).
        let content_width = if word_wrap {
            editor_width
        } else {
            // Approx monospace glyph width at the default 14px editor
            // font; overshooting is harmless, we just get a bit of
            // empty runway at the right edge of the scroll area.
            let glyph_w = 8.0_f32;
            let max_chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
            ((max_chars as f32) * glyph_w + 32.0).max(editor_width)
        };
        let show_editor = |ui: &mut egui::Ui, text: &mut String| -> egui::Response {
            let inner_width = content_width;
            ui.add_sized(
                [inner_width, editor_height],
                egui::TextEdit::multiline(text)
                    .id(text_edit_id)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(inner_width)
                    .desired_rows(((editor_height / 16.0) as usize).max(10))
                    .lock_focus(true)
                    .interactive(true)
                    .layouter(&mut |ui, string, _wrap_width| {
                        if is_ro {
                            if let Some(galley) = &galley_cache {
                                return galley.clone();
                            }
                        }
                        let mut layout_job =
                            modelica_layouter(ui.style(), string.as_str());
                        layout_job.wrap.max_width = if word_wrap {
                            editor_width
                        } else {
                            f32::INFINITY
                        };
                        ui.painter().layout_job(layout_job)
                    }),
            )
        };

        // Inner margin so the first column of code isn't flush
        // against the panel's left edge. Without it the glyphs of
        // the leftmost column sit under the adjacent panel's
        // boundary (package browser on the left), hiding the first
        // few characters of every line.
        // Vertical scroll area owned by the panel (egui_dock's
        // per-tab ScrollArea is disabled so the toolbar stays
        // pinned — see lunco-workbench::PanelTabViewer::scroll_bars).
        // Without this the TextEdit claims all available vertical
        // space and long files can't be scrolled.
        let output = egui::Frame::default()
            .inner_margin(egui::Margin {
                left: 8,
                right: 0,
                top: 0,
                bottom: 0,
            })
            .show(ui, |ui| {
                let v_area = egui::ScrollArea::vertical()
                    .id_salt("modelica_code_editor_vscroll")
                    .auto_shrink([false, false]);
                v_area
                    .show(ui, |ui| {
                        if word_wrap {
                            show_editor(ui, &mut text)
                        } else {
                            let mut area = egui::ScrollArea::horizontal()
                                .id_salt("modelica_code_editor_hscroll")
                                .auto_shrink([false, false]);
                            if model_switched {
                                area = area.horizontal_scroll_offset(0.0);
                            }
                            area.show(ui, |ui| show_editor(ui, &mut text)).inner
                        }
                    })
                    .inner
            })
            .inner;

        // Pre-extract the selected text now that `text` is in scope
        // — keyboard / toolbar handlers below need it for the
        // clipboard write.
        let selected_text: Option<String> = selection_chars.map(|(s, e)| {
            text.chars().skip(s).take(e - s).collect::<String>()
        });

        // Translate toolbar button clicks into the same
        // `copy_payload` / `cut_range` / `select_all_request` flags
        // the keyboard handler uses, so all three input paths share
        // one set of action processors below.
        let mut copy_payload: Option<String> = None;
        let mut cut_range: Option<(usize, usize)> = None;
        let mut select_all_request = false;
        if tb_copy && has_selection {
            copy_payload = selected_text.clone();
        }
        if tb_cut && has_selection && !is_ro {
            copy_payload = selected_text.clone();
            cut_range = selection_chars;
        }
        if tb_paste && !is_ro {
            crate::ui::wasm_clipboard::request_paste_from_clipboard();
        }
        if tb_select_all {
            select_all_request = true;
        }

        // Apply menu actions outside the menu closure — `text`,
        // `new_text`, and `buffer_changed` are exclusively borrowed
        // here so we can mutate freely.
        if let Some(payload) = copy_payload {
            if !payload.is_empty() {
                ui.ctx().copy_text(payload);
            }
        }
        if let Some((cut_start, cut_end)) = cut_range {
            let mut out = String::with_capacity(text.len());
            for (i, c) in text.chars().enumerate() {
                if i < cut_start || i >= cut_end {
                    out.push(c);
                }
            }
            text = out;
            new_text = text.clone();
            buffer_changed = true;
            if let Some(mut state) =
                egui::TextEdit::load_state(ui.ctx(), text_edit_id)
            {
                state.cursor.set_char_range(Some(
                    egui::text::CCursorRange::one(egui::text::CCursor::new(cut_start)),
                ));
                state.store(ui.ctx(), text_edit_id);
            }
        }
        if select_all_request {
            if let Some(mut state) =
                egui::TextEdit::load_state(ui.ctx(), text_edit_id)
            {
                let n = text.chars().count();
                state.cursor.set_char_range(Some(
                    egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(n),
                    ),
                ));
                state.store(ui.ctx(), text_edit_id);
            }
        }

        // ── Jump-to-source (Diagnostics click) ──
        //
        // A located finding was clicked in the Diagnostics panel; move
        // the caret to it and focus the editor so its ScrollArea brings
        // the line into view. Applied only when the request targets the
        // document this tab is showing, so a stale request can't scroll
        // the wrong file. The request is one-shot — `take` drains it.
        if let Some((jump_doc, loc)) = world
            .get_resource_mut::<EditorJumpRequest>()
            .and_then(|mut r| r.take())
        {
            if jump_doc.is_none() || jump_doc == tab_target {
                let offset = line_col_to_char_offset(&text, loc.line, loc.column);
                if let Some(mut state) =
                    egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                {
                    state.cursor.set_char_range(Some(
                        egui::text::CCursorRange::one(egui::text::CCursor::new(offset)),
                    ));
                    state.store(ui.ctx(), text_edit_id);
                }
                // Focus the TextEdit so egui scrolls the caret into view
                // within the panel's vertical ScrollArea.
                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
            }
        }

        // ── Keyboard clipboard shortcuts ──
        //
        // On wasm, the browser's native `copy`/`cut`/`paste` JS
        // events are suppressed by Bevy's
        // `prevent_default_event_handling: true` window setting (see
        // `bin/lunica.rs`). That means neither bevy_egui's
        // built-in clipboard listeners nor any custom JS listener
        // can ever fire for Ctrl/Cmd+C/X/V. Egui's keyboard input,
        // however, *is* delivered correctly — so we detect the
        // shortcut here and run the same code path the right-click
        // menu's Copy/Cut already use (which we know works because
        // the user confirmed menu Copy works).
        //
        // For paste we kick off an async
        // `navigator.clipboard.readText()` and pick the resolved
        // text up on the next visit via `take_pending_paste()`.
        let (kbd_copy, kbd_cut) = ui.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && i.key_pressed(egui::Key::C),
                cmd && i.key_pressed(egui::Key::X),
            )
        });
        if kbd_copy && has_selection {
            if let Some(s) = selected_text.as_ref() {
                if !s.is_empty() {
                    ui.ctx().copy_text(s.clone());
                }
            }
        }
        if kbd_cut && has_selection && !is_ro {
            if let Some(s) = selected_text.as_ref() {
                if !s.is_empty() {
                    ui.ctx().copy_text(s.clone());
                }
            }
            // Reuse the menu's `cut_range` so the splice happens in
            // the cut-handler block below — no duplicate splice loop.
            cut_range = selection_chars;
        }
        // Ctrl/Cmd+V is intentionally not handled here: the document
        // capture-phase keydown listener in `wasm_clipboard.rs` lets
        // the browser fire its native `paste` event, and the
        // capture-phase `paste` listener queues `pending_paste`
        // synchronously without prompting the user. Drained below.

        // Re-run the cut splice from the menu block — `cut_range` may
        // have just been populated by the keyboard handler above.
        // (The earlier menu-action block ran before this point with
        // `cut_range=None` for the keyboard path.) Idempotent if
        // already applied.
        if let Some((cut_start, cut_end)) = cut_range {
            // Guard against double-apply: only splice if `text` still
            // contains the original range. Cheap heuristic — compare
            // the slice we'd remove against `selected_text`.
            let still_present = selected_text.as_deref().map(|sel| {
                let extracted: String =
                    text.chars().skip(cut_start).take(cut_end - cut_start).collect();
                extracted == sel
            }).unwrap_or(false);
            if still_present {
                let mut out = String::with_capacity(text.len());
                for (i, c) in text.chars().enumerate() {
                    if i < cut_start || i >= cut_end {
                        out.push(c);
                    }
                }
                text = out;
                new_text = text.clone();
                buffer_changed = true;
                if let Some(mut state) =
                    egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                {
                    state.cursor.set_char_range(Some(
                        egui::text::CCursorRange::one(egui::text::CCursor::new(cut_start)),
                    ));
                    state.store(ui.ctx(), text_edit_id);
                }
            }
        }

        // Drain any paste text the async clipboard read resolved
        // since the previous frame, and apply it at the cursor (or
        // replace the selection).
        if !is_ro {
            if let Some(raw_paste) = crate::ui::wasm_clipboard::take_pending_paste() {
                // Normalise line endings + strip a leading BOM. The
                // OS clipboard frequently delivers CRLF (Windows) or
                // CR-only (rare, but some terminals) sequences;
                // rumoca's lexer expects LF and panics on a non-
                // char-boundary slice when it tries to advance over
                // a CR. The BOM strip handles UTF-8-with-BOM text
                // pasted from Windows tools.
                let paste_text = raw_paste
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                let paste_text = paste_text
                    .strip_prefix('\u{FEFF}')
                    .map(|s| s.to_string())
                    .unwrap_or(paste_text);
                let (start, end) = selection_chars.unwrap_or_else(|| {
                    let cur = egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                        .and_then(|s| s.cursor.char_range())
                        .map(|r| r.primary.index)
                        .unwrap_or_else(|| text.chars().count());
                    (cur, cur)
                });
                let total = text.chars().count();
                let mut out =
                    String::with_capacity(text.len() + paste_text.len());
                let mut inserted = false;
                for (i, c) in text.chars().enumerate() {
                    if i == start {
                        out.push_str(&paste_text);
                        inserted = true;
                    }
                    if i < start || i >= end {
                        out.push(c);
                    }
                }
                if !inserted && start >= total {
                    out.push_str(&paste_text);
                }
                text = out;
                new_text = text.clone();
                buffer_changed = true;
                let new_caret = start + paste_text.chars().count();
                if let Some(mut state) =
                    egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                {
                    state.cursor.set_char_range(Some(
                        egui::text::CCursorRange::one(
                            egui::text::CCursor::new(new_caret),
                        ),
                    ));
                    state.store(ui.ctx(), text_edit_id);
                }
            }
        }

        if output.changed() && !is_ro {
            new_text = text.clone();
            buffer_changed = true;

            // ── Auto-indent ──
            //
            // If this change added exactly one '\n' to the buffer
            // (user pressed Enter) and the previous line had leading
            // whitespace, inject that same leading whitespace after
            // the '\n' and move the caret past it — so the user
            // picks up editing at the same indent level as the
            // previous line.
            //
            // Behaviour chosen to match every modern code editor:
            // VS Code, Sublime, IntelliJ, Emacs electric-indent,
            // Neovim's `autoindent` / `smartindent`.
            if auto_indent {
                if let Some(nl_byte) = detect_single_newline_insertion(&old_text, &new_text) {
                    let line_start = new_text[..nl_byte]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let indent: String = new_text[line_start..nl_byte]
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .collect();
                    if !indent.is_empty() {
                        let after_nl = nl_byte + 1;
                        new_text.insert_str(after_nl, &indent);
                        text = new_text.clone();
                        // Move the TextEdit caret past the inserted
                        // indent. egui caret positions are in *char*
                        // offsets (not bytes), so convert.
                        let new_caret_chars =
                            new_text[..after_nl + indent.len()].chars().count();
                        if let Some(mut state) =
                            egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                        {
                            state.cursor.set_char_range(Some(
                                egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(new_caret_chars),
                                ),
                            ));
                            state.store(ui.ctx(), text_edit_id);
                        }
                    }
                }
            }
        }
        // Focus-loss commit: edits flow into the Document so other
        // observers (diagram re-parse, dirty tracker) see them
        // without requiring Compile.
        if output.lost_focus() && !is_ro {
            if new_text.is_empty() {
                new_text = text.clone();
            }
            buffer_commit = true;
        }

        // Capture the current egui time BEFORE we mutate the buffer
        // state — used to stamp the pending-commit timestamp below.
        let now = ui.ctx().input(|i| i.time);

        if buffer_changed {
            let mut buf_state = world.resource_mut::<EditorBufferState>();
            buf_state.text = new_text.clone();
            // NOTE: do NOT call `extract_model_name(&buf_state.text)`
            // here. That function runs a full rumoca parse which
            // takes seconds on a non-trivial source and visibly
            // stalls the UI on every keystroke (see warning in
            // `ast_extract.rs::extract_model_name` doc — and we
            // hit a real freeze + occasional rumoca panic when this
            // ran per-edit). `detected_name` is updated through the
            // worker-parsed AST path on commit/flush; the live UI
            // consumers (inspector, model_view, package_browser,
            // canvas overlays) read it from the per-doc Index on
            // the registry, which is refreshed off-thread.
            // Mark the buffer as dirty vs. the document. The flush
            // block below only commits once `now - pending_commit_at
            // >= EDIT_DEBOUNCE_SEC`, so a burst of typing resets this
            // timestamp on every keystroke and debounces cleanly
            // into a single checkpoint at the end.
            buf_state.pending_commit_at = Some(now);

            if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                if state.editor_buffer != new_text {
                    state.editor_buffer = new_text;
                }
            }

            // Egui doesn't repaint a stationary UI on its own. When
            // the user stops typing we still want the debounce timer
            // to fire — schedule a wake-up just past the window.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(
                EDIT_DEBOUNCE_SEC + 0.05,
            ));
        }

        // Decide whether to flush the pending buffer into the
        // document this frame. Three trigger points:
        //
        //   1. `buffer_commit` (focus-loss): flush immediately — the
        //      user has clearly finished editing this session.
        //   2. Debounce elapsed: `now - pending_commit_at >=
        //      EDIT_DEBOUNCE_SEC`. The user has been idle long
        //      enough; coalesce their typing into one checkpoint.
        //   3. Nothing pending: no-op.
        //
        // `checkpoint_source` is idempotent when the content hasn't
        // changed, so duplicate triggers are safe.
        let should_flush = {
            let buf = world.resource::<EditorBufferState>();
            match (buffer_commit, buf.pending_commit_at) {
                (true, _) => buf.pending_commit_at.is_some(),
                (false, Some(t)) => now - t >= EDIT_DEBOUNCE_SEC,
                (false, None) => false,
            }
        };

        if should_flush && !is_read_only {
            if let Some(doc) = doc_id {
                commit_pending_buffer(world, doc);
            }
        }
    }
}

/// Diff `EditorBufferState.text` against the registry's current
/// source for `doc` and emit a minimal `EditText` splice if they
/// differ. Clears `pending_commit_at` either way.
///
/// Public so cross-truth rule R3 (`B0_CROSS_TRUTH_POLICY.md`) can
/// force-flush before a tab-mode switch transitions away from the
/// text view: any uncommitted typing turns into a real op before
/// the canvas tab activates, so its first render observes the new
/// generation.
///
/// Returns `true` when something was actually committed (diff
/// non-empty AND the apply succeeded), `false` for silent no-ops
/// (buffer matched source, or doc absent).
pub fn commit_pending_buffer(world: &mut World, doc: lunco_doc::DocumentId) -> bool {
    let committed = world.resource::<EditorBufferState>().text.clone();
    let prior = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
        .map(|h| h.document().source().to_string());
    let mut wrote = false;
    if let Some(prior) = prior {
        if let Some((range, replacement)) =
            crate::text_diff::diff_to_edit(&prior, &committed)
        {
            bevy::log::info!(
                "[editor-flush] doc={} EditText {:?} replace_len={} prior_len={} committed_len={}",
                doc.raw(),
                range,
                replacement.len(),
                prior.len(),
                committed.len(),
            );
            let result = crate::ui::panels::canvas_diagram::apply_one_op_as(
                world,
                doc,
                crate::document::ModelicaOp::EditText { range, replacement },
                lunco_twin_journal::AuthorTag::for_tool("code-editor"),
            );
            bevy::log::info!(
                "[editor-flush] doc={} apply result: {}",
                doc.raw(),
                if result.is_ok() { "Ok" } else { "Err" },
            );
            if result.is_ok() {
                // Self-edit landed: pull the doc's new generation
                // forward into our buffer state so the next frame's
                // same-tab divergence check doesn't fire on our own
                // op and resync over still-uncommitted typing the
                // user added between this flush and the next render.
                if let Some(host) =
                    world.resource::<ModelicaDocumentRegistry>().host(doc)
                {
                    let new_gen = host.generation();
                    world.resource_mut::<EditorBufferState>().generation = new_gen;
                }
                wrote = true;
            }
        }
    }
    world.resource_mut::<EditorBufferState>().pending_commit_at = None;
    wrote
}

// ─────────────────────────────────────────────────────────────────────────────
// Previously this module exported `dispatch_compile_from_buffer`,
// `apply_document_undo`, and `apply_document_redo` — ad-hoc helpers the
// ModelViewPanel toolbar called directly. All of that logic now lives in
// command observers in `crate::ui::commands` (`on_compile_model`,
// `on_undo_document`, `on_redo_document`). Buttons fire the corresponding
// `#[Command]` events instead of calling helpers, so keyboard shortcuts,
// scripting, and the remote API share one write path. This comment is
// the only thing left of them — the observers *are* the documentation.

// Modelica keyword categories — each gets its own colour so declaration
// intent (`parameter`, `input`, …) reads at a glance against structural
// keywords (`model`, `equation`, …) and control flow (`if`, `when`, …).
// Lists are kept small on purpose: MLS §A.1 defines the full reserved-word
// set, but the editor only needs to highlight the ones users actually
// type and read.
const MODIFIER_KEYWORDS: &[&str] = &[
    "parameter", "input", "output", "constant", "discrete",
    "flow", "stream", "final", "inner", "outer",
    "replaceable", "redeclare", "each", "partial",
];
const STRUCTURAL_KEYWORDS: &[&str] = &[
    "model", "block", "connector", "function", "package", "record", "type",
    "class", "operator", "equation", "algorithm", "initial", "annotation",
    "end", "extends", "within", "import", "public", "protected",
];
const CONTROL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elseif", "for", "in", "loop",
    "while", "when", "elsewhen", "break", "return",
];
const OPERATOR_KEYWORDS: &[&str] = &[
    "and", "or", "not", "der", "connect", "time", "true", "false",
];
const BUILTIN_TYPES: &[&str] = &[
    "Real", "Integer", "Boolean", "String", "enum",
];

/// Mode-aware syntax-highlight palette. Both rows hand-tuned for
/// readable contrast on their respective surfaces (`mantle`/`base`
/// in dark Mocha vs light Latte). `style.visuals.dark_mode` is the
/// switch — set by `Visuals::dark()` / `Visuals::light()`, which
/// `Theme::to_visuals` derives from.
struct SyntaxPalette {
    modifier: egui::Color32,    // parameter, input, output, constant…
    structural: egui::Color32,  // model, equation, end, package…
    control: egui::Color32,     // if, then, else, for, when…
    operator: egui::Color32,    // and, or, not, der, time…
    builtin_type: egui::Color32,// Real, Integer, Boolean, String…
    comment: egui::Color32,
    string: egui::Color32,
    number: egui::Color32,
    op: egui::Color32,          // punctuation: =, +, *, …
    upper_ident: egui::Color32, // SomeType-style identifiers
    ident: egui::Color32,
    default: egui::Color32,
}

impl SyntaxPalette {
    fn for_style(style: &egui::Style) -> Self {
        if style.visuals.dark_mode {
            Self {
                modifier:     egui::Color32::from_rgb(240, 180, 80),
                structural:   egui::Color32::from_rgb(255, 120, 120),
                control:      egui::Color32::from_rgb(200, 150, 230),
                operator:     egui::Color32::from_rgb(120, 200, 200),
                builtin_type: egui::Color32::from_rgb(120, 200, 255),
                comment:      egui::Color32::from_rgb(110, 150, 110),
                string:       egui::Color32::from_rgb(200, 220, 140),
                number:       egui::Color32::from_rgb(150, 200, 255),
                op:           egui::Color32::from_rgb(230, 200, 120),
                upper_ident:  egui::Color32::from_rgb(150, 200, 255),
                ident:        egui::Color32::from_rgb(230, 230, 230),
                default:      egui::Color32::from_rgb(180, 180, 180),
            }
        } else {
            // Light: darker / more saturated variants of the same
            // hues so contrast against `base`/`mantle` (≈ #eff1f5)
            // stays readable. Roughly Latte-aligned.
            Self {
                modifier:     egui::Color32::from_rgb(150, 100, 0),
                structural:   egui::Color32::from_rgb(170, 35, 35),
                control:      egui::Color32::from_rgb(120, 60, 180),
                operator:     egui::Color32::from_rgb(30, 120, 120),
                builtin_type: egui::Color32::from_rgb(20, 90, 180),
                comment:      egui::Color32::from_rgb(80, 130, 80),
                string:       egui::Color32::from_rgb(110, 130, 30),
                number:       egui::Color32::from_rgb(20, 90, 180),
                op:           egui::Color32::from_rgb(150, 100, 20),
                upper_ident:  egui::Color32::from_rgb(20, 90, 180),
                ident:        egui::Color32::from_rgb(35, 35, 40),
                default:      egui::Color32::from_rgb(80, 80, 90),
            }
        }
    }
}

fn keyword_color(word: &str, p: &SyntaxPalette) -> Option<egui::Color32> {
    if MODIFIER_KEYWORDS.contains(&word) {
        Some(p.modifier)
    } else if STRUCTURAL_KEYWORDS.contains(&word) {
        Some(p.structural)
    } else if CONTROL_KEYWORDS.contains(&word) {
        Some(p.control)
    } else if OPERATOR_KEYWORDS.contains(&word) {
        Some(p.operator)
    } else if BUILTIN_TYPES.contains(&word) {
        Some(p.builtin_type)
    } else {
        None
    }
}

pub fn modelica_layouter(style: &egui::Style, src: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font_id = egui::TextStyle::Monospace.resolve(style);

    let palette = SyntaxPalette::for_style(style);
    let comment_color = palette.comment;
    let string_color = palette.string;
    let number_color = palette.number;
    let op_color = palette.op;
    let upper_ident_color = palette.upper_ident;
    let ident_color = palette.ident;
    let default_color = palette.default;

    let push = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
        job.append(text, 0.0, egui::TextFormat {
            font_id: font_id.clone(),
            color,
            ..Default::default()
        });
    };

    let mut current_idx = 0;
    while current_idx < src.len() {
        let remaining = &src[current_idx..];

        // Single-line comment.
        if remaining.starts_with("//") {
            let line_end = remaining.find('\n').unwrap_or(remaining.len());
            push(&mut job, &remaining[..line_end], comment_color);
            current_idx += line_end;
            continue;
        }

        // Multi-line comment (spans may extend beyond the current
        // chunk; fall back to end-of-buffer if no closing `*/`).
        if remaining.starts_with("/*") {
            let end_idx = remaining.find("*/").map(|i| i + 2).unwrap_or(remaining.len());
            push(&mut job, &remaining[..end_idx], comment_color);
            current_idx += end_idx;
            continue;
        }

        // Modelica description strings + general string literals. We
        // accept a simple `"…"` (no escape tracking yet); this is good
        // enough for the `"description"` idiom that follows most
        // declarations. Strings that reach end-of-buffer are coloured
        // anyway so an unterminated literal in mid-edit looks sane.
        if remaining.starts_with('"') {
            let after_quote = &remaining[1..];
            let close_rel = after_quote.find('"').map(|i| i + 2).unwrap_or(remaining.len());
            push(&mut job, &remaining[..close_rel], string_color);
            current_idx += close_rel;
            continue;
        }

        let first_char = match remaining.chars().next() {
            Some(c) => c,
            None => break,
        };

        if first_char.is_alphabetic() || first_char == '_' {
            let word_end = remaining
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(remaining.len());
            let word = &remaining[..word_end];

            let color = keyword_color(word, &palette).unwrap_or_else(|| {
                if word.chars().next().map_or(false, |c| c.is_uppercase()) {
                    upper_ident_color
                } else {
                    ident_color
                }
            });
            push(&mut job, word, color);
            current_idx += word_end;
        } else if first_char.is_numeric() {
            let num_end = remaining
                .find(|c: char| !c.is_numeric() && c != '.' && c != 'e' && c != 'E')
                .unwrap_or(remaining.len());
            push(&mut job, &remaining[..num_end], number_color);
            current_idx += num_end;
        } else {
            let color = if "+-*/=^<>(){}[],;:".contains(first_char) {
                op_color
            } else {
                default_color
            };
            push(&mut job, &remaining[..first_char.len_utf8()], color);
            current_idx += first_char.len_utf8();
        }
    }

    job
}
