//! Code Editor panel — central viewport for Modelica source code.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use std::sync::Arc;

use crate::ast_extract::{extract_model_name, hash_content};
use crate::ui::{ModelicaDocumentRegistry, WorkbenchState};

/// Tracks which model the editor buffer belongs to, to detect model switches.
#[derive(Resource)]
pub struct EditorBufferState {
    /// Hash of the source that was loaded into editor_buffer.
    pub source_hash: u64,
    /// The model_path of the current open_model when buffer was last synced.
    pub model_path: String,
    /// The actual text content (persistent across frames for selection).
    pub text: String,
    /// Byte offsets of the start of each line.
    pub line_starts: Arc<[usize]>,
    /// Memoized detected name to avoid per-frame regex on large strings.
    pub detected_name: Option<String>,
    /// Pre-computed text layout for high-performance rendering.
    pub cached_galley: Option<Arc<egui::Galley>>,
    /// When `true` long lines wrap at the editor width; when `false`
    /// (default) long lines stay on one line and the view scrolls
    /// horizontally — mirroring VS Code's default "Word Wrap: Off".
    pub word_wrap: bool,
}

impl Default for EditorBufferState {
    fn default() -> Self {
        Self {
            source_hash: 0,
            model_path: String::new(),
            text: String::new(),
            line_starts: vec![0].into(),
            detected_name: None,
            cached_galley: None,
            word_wrap: false,
        }
    }
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

        // ── Determine what model to show (fetch metadata first) ──
        let (display_name, is_read_only, model_path, compilation_error, selected_entity, is_loading) = {
            let state = world.resource::<WorkbenchState>();
            let meta = state.open_model.as_ref().map(|m| (m.display_name.clone(), m.read_only, m.model_path.clone()));
            let err = state.compilation_error.clone();
            let entity = state.selected_entity;
            let loading = state.is_loading;
            (meta.as_ref().map(|m| m.0.clone()), 
             meta.as_ref().map(|m| m.1).unwrap_or(false),
             meta.as_ref().map(|m| m.2.clone()),
             err, entity, loading)
        };

        if is_loading {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.spinner();
                ui.add_space(10.0);
                ui.heading("Opening model...");
                ui.label(egui::RichText::new("Reading from disk and indexing...").size(10.0).color(egui::Color32::GRAY));
            });
            return;
        }

        if let Some(ref path) = model_path {
            // Resync from `open_model.source` when either
            //   (a) the model itself changed (`model_path` diverged), or
            //   (b) some other panel mutated `open_model.source` —
            //       currently the diagram panel after applying AST
            //       ops — and the content hash now differs from what
            //       this buffer last synced.
            //
            // Case (b) is what propagates diagram edits (add / delete
            // component) into the code view. Gap: if the user has
            // un-committed text edits in this buffer AND triggers a
            // diagram edit, the resync clobbers them. That's a known
            // transitional gap until the code editor writes every
            // keystroke through the Document (its own `EditText` op).
            let needs_sync = {
                let buf_state = world.resource::<EditorBufferState>();
                let external_hash = world.resource::<WorkbenchState>()
                    .open_model
                    .as_ref()
                    .map(|m| hash_content(&m.source))
                    .unwrap_or(0);
                buf_state.model_path != *path || buf_state.source_hash != external_hash
            };

            if needs_sync {
                let (source, line_starts, detected_name, galley) = {
                    let state = world.resource::<WorkbenchState>();
                    let m = state.open_model.as_ref().unwrap();
                    (m.source.to_string(), m.line_starts.clone(), m.detected_name.clone(), m.cached_galley.clone())
                };

                let mut buf_state = world.resource_mut::<EditorBufferState>();
                buf_state.text = source;
                buf_state.line_starts = line_starts;
                buf_state.model_path = path.clone();
                buf_state.source_hash = hash_content(&buf_state.text);
                buf_state.detected_name = detected_name;
                buf_state.cached_galley = galley;
            }
        }

        if model_path.is_none() {
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
        // focus-loss commit below writes into it. Prefer the registry's
        // entity→doc link (what a compile set up), falling back to
        // `open_model.doc` so edits on an uncompiled in-memory model
        // still land in its pre-allocated Document.
        let doc_id = selected_entity
            .and_then(|e| world.resource::<ModelicaDocumentRegistry>().document_of(e))
            .or_else(|| {
                world
                    .resource::<WorkbenchState>()
                    .open_model
                    .as_ref()
                    .and_then(|m| m.doc)
            });

        // ── Toolbar row: word-wrap toggle ──
        //
        // Default: off. Long lines extend right and the editor body
        // scrolls horizontally to reach them — matching VS Code's
        // default "Word Wrap: Off". Turning it on re-layouts each
        // line at the editor width so everything fits without
        // scrolling.
        //
        // Persisted on `EditorBufferState` so switching panels or
        // rebinding a document doesn't reset the preference.
        let word_wrap = {
            let mut wrap = world.resource::<EditorBufferState>().word_wrap;
            ui.horizontal(|ui| {
                let label = if wrap { "↵ Word Wrap: On" } else { "→ Word Wrap: Off" };
                if ui.selectable_label(wrap, label).clicked() {
                    wrap = !wrap;
                    world.resource_mut::<EditorBufferState>().word_wrap = wrap;
                }
            });
            wrap
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
        let (mut text, galley_cache) = {
            let buf_state = world.resource::<EditorBufferState>();
            (buf_state.text.clone(), buf_state.cached_galley.clone())
        };
        let is_ro = is_read_only;
        let editor_width = avail.x.max(100.0);
        let editor_height = avail.y.max(200.0);

        // When word-wrap is off, long lines must live inside a
        // horizontal `ScrollArea` so they can scroll rather than
        // clip. When word-wrap is on, the layouter does the right
        // thing at editor-width and no horizontal scroll is needed.
        let show_editor = |ui: &mut egui::Ui, text: &mut String| -> egui::Response {
            // `desired_width` defines the widget's allocated width.
            // In scroll mode we want the widget to be AS WIDE AS THE
            // LONGEST LINE so the outer ScrollArea can scroll to it;
            // `f32::INFINITY` would be rejected, so we pass a large
            // finite number and let the layouter determine the
            // galley's actual width.
            let inner_width = if word_wrap { editor_width } else { editor_width.max(2000.0) };
            ui.add_sized(
                [inner_width, editor_height],
                egui::TextEdit::multiline(text)
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

        let output = if word_wrap {
            show_editor(ui, &mut text)
        } else {
            egui::ScrollArea::horizontal()
                .auto_shrink([false, false])
                .show(ui, |ui| show_editor(ui, &mut text))
                .inner
        };

        if output.changed() && !is_ro {
            new_text = text.clone();
            buffer_changed = true;
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

        if buffer_changed {
            let mut buf_state = world.resource_mut::<EditorBufferState>();
            buf_state.text = new_text.clone();
            // Recompute line starts for the editor buffer if changed
            let mut new_starts = vec![0];
            for (i, byte) in buf_state.text.as_bytes().iter().enumerate() {
                if *byte == b'\n' {
                    new_starts.push(i + 1);
                }
            }
            buf_state.line_starts = new_starts.into();
            buf_state.detected_name = extract_model_name(&buf_state.text);

            if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                if state.editor_buffer != new_text {
                    state.editor_buffer = new_text;
                }
            }
        }

        // Commit the buffer into the Document on focus-loss. No-op when
        // there's no backing document yet (the user is typing before any
        // compile has produced an entity) — the existing "spawn fresh
        // entity on Compile" path still handles that case.
        if buffer_commit && !is_read_only {
            if let Some(doc) = doc_id {
                let committed = world.resource::<EditorBufferState>().text.clone();
                world
                    .resource_mut::<ModelicaDocumentRegistry>()
                    .checkpoint_source(doc, committed);
            }
        }
    }
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

fn keyword_color(word: &str) -> Option<egui::Color32> {
    if MODIFIER_KEYWORDS.contains(&word) {
        // Amber — declaration modifiers (parameter, input, output, …).
        Some(egui::Color32::from_rgb(240, 180, 80))
    } else if STRUCTURAL_KEYWORDS.contains(&word) {
        // Coral — class / section structure (model, equation, end, …).
        Some(egui::Color32::from_rgb(255, 120, 120))
    } else if CONTROL_KEYWORDS.contains(&word) {
        // Violet — control flow (if/then/else, for, when, …).
        Some(egui::Color32::from_rgb(200, 150, 230))
    } else if OPERATOR_KEYWORDS.contains(&word) {
        // Teal — builtin operators / named ops (and, or, not, der, …).
        Some(egui::Color32::from_rgb(120, 200, 200))
    } else if BUILTIN_TYPES.contains(&word) {
        // Cyan — primitive types (Real, Integer, Boolean, String).
        Some(egui::Color32::from_rgb(120, 200, 255))
    } else {
        None
    }
}

pub fn modelica_layouter(style: &egui::Style, src: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font_id = egui::TextStyle::Monospace.resolve(style);

    let comment_color = egui::Color32::from_rgb(110, 150, 110);
    let string_color = egui::Color32::from_rgb(200, 220, 140);
    let number_color = egui::Color32::from_rgb(150, 200, 255);
    let op_color = egui::Color32::from_rgb(230, 200, 120);
    let upper_ident_color = egui::Color32::from_rgb(150, 200, 255);
    let ident_color = egui::Color32::from_rgb(230, 230, 230);
    let default_color = egui::Color32::from_rgb(180, 180, 180);

    let mut push = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
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

            let color = keyword_color(word).unwrap_or_else(|| {
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
