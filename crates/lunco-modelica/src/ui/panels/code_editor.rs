//! Code Editor panel — central viewport for Modelica source code.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use std::collections::HashMap;
use std::sync::Arc;

use crate::ui::{ModelicaDocumentRegistry, WorkbenchState};
use crate::ui::panels::diagram::DiagramState;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};
use crate::ast_extract::{
    extract_model_name, extract_parameters, extract_inputs_with_defaults,
    extract_input_names, hash_content,
};

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
            let needs_sync = {
                let buf_state = world.resource::<EditorBufferState>();
                buf_state.model_path != *path
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

        // ── Editor area ──
        let mut buffer_changed = false;
        let mut buffer_commit = false;
        let mut new_text = String::new();

        let available_height = ui.available_height();

        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .min_scrolled_height(available_height)
            .show(ui, |ui| {
                // Fetch data needed for the closure first
                let (text_str, line_starts_len, galley_cache) = {
                    let buf_state = world.resource::<EditorBufferState>();
                    (buf_state.text.clone(), buf_state.line_starts.len(), buf_state.cached_galley.clone())
                };
                let mut text = text_str.as_str();
                let is_ro = is_read_only;

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    // Gutter
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let row_height = ui.spacing().interact_size.y.max(font_id.size);

                    ui.vertical(|ui| {
                        ui.set_width(35.0);
                        for i in 1..=line_starts_len {
                            ui.add_sized([30.0, row_height], egui::Label::new(
                                egui::RichText::new(format!("{:>3}", i))
                                    .size(10.0)
                                    .color(egui::Color32::DARK_GRAY)
                            ).selectable(false));
                        }
                    });

                    // TextEdit
                    let output = ui.add(egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .lock_focus(true)
                        .interactive(true) // Always interactive for selection
                        .layouter(&mut |ui, string, _wrap_width| {
                            if is_ro {
                                if let Some(galley) = &galley_cache {
                                    return galley.clone();
                                }
                            }
                            let mut layout_job = modelica_layouter(&ui.style(), string.as_str());
                            layout_job.wrap.max_width = f32::INFINITY;
                            ui.painter().layout_job(layout_job)
                        })
                    );

                    if output.changed() && !is_ro {
                        new_text = text.to_string();
                        buffer_changed = true;
                    }
                    // Commit the buffer into the Document when the user
                    // leaves the editor (clicks elsewhere, tabs away,
                    // presses Esc, etc.). This is the point at which edits
                    // flow into the Document without requiring a Compile —
                    // so splits / diagram / other observers see the change.
                    // Coarse (whole-buffer) op for now; granular ops are a
                    // separate refactor.
                    if output.lost_focus() && !is_ro {
                        if new_text.is_empty() {
                            new_text = text.to_string();
                        }
                        buffer_commit = true;
                    }
                });
            });

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

/// Dispatch a Compile command from the current editor buffer.
///
/// Moved out of the panel's toolbar so [`crate::ui::panels::model_view::ModelViewPanel`]
/// (and any future caller — keyboard shortcut, script, API) can trigger
/// the same flow. Reads [`EditorBufferState`], parses params/inputs,
/// ensures a backing [`crate::document::ModelicaDocument`] exists
/// (spawning an entity if none is selected yet), checkpoints the source,
/// marks the document as `Compiling`, and sends
/// [`ModelicaCommand::Compile`] to the worker.
///
/// On parse failure (no `model <name>` declaration found) sets
/// `WorkbenchState.compilation_error` and returns without dispatching.
pub fn dispatch_compile_from_buffer(world: &mut World) {
    let buf_state = world.resource::<EditorBufferState>();
    let Some(model_name) = buf_state.detected_name.clone() else {
        if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
            s.compilation_error = Some("Could not find a valid model declaration.".to_string());
        }
        return;
    };
    let source = buf_state.text.clone();
    let params = extract_parameters(&source);
    let inputs_with_defaults = extract_inputs_with_defaults(&source);
    let runtime_inputs = extract_input_names(&source);

    let is_read_only = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.read_only)
        .unwrap_or(false);
    if is_read_only {
        return;
    }

    let selected_entity = world.resource::<WorkbenchState>().selected_entity;

    let mut session_id = 0;
    let mut should_compile = false;

    if let Some(entity) = selected_entity {
        if let Ok(mut model) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
            let old_inputs = std::mem::take(&mut model.inputs);
            model.session_id += 1;
            model.is_stepping = true;
            model.model_name = model_name.clone();
            model.parameters = params;
            model.inputs.clear();
            for (name, val) in &inputs_with_defaults {
                let existing = old_inputs.get(name).copied();
                model.inputs.entry(name.clone()).or_insert_with(|| existing.unwrap_or(*val));
            }
            for name in &runtime_inputs {
                let existing = old_inputs.get(name).copied();
                model.inputs.entry(name.clone()).or_insert_with(|| existing.unwrap_or(0.0));
            }
            model.variables.clear();
            model.paused = false;
            model.current_time = 0.0;
            model.last_step_time = 0.0;
            session_id = model.session_id;
            should_compile = true;
        }
    } else {
        let ds = world.resource::<DiagramState>();
        session_id = ds.model_counter as u64 + 1;
        // Reuse the Document already attached to `open_model` when one
        // exists (in-memory models allocate at creation time). This
        // preserves undo history and keeps the in-memory entry in the
        // Package Browser pointing at the same source. Only allocate a
        // fresh Document when the open model has none (on-disk files
        // allocate lazily on first compile).
        let reuse_doc = world
            .resource::<WorkbenchState>()
            .open_model
            .as_ref()
            .and_then(|m| m.doc);
        let doc = match reuse_doc {
            Some(d) => {
                world
                    .resource_mut::<ModelicaDocumentRegistry>()
                    .checkpoint_source(d, source.clone());
                d
            }
            None => world
                .resource_mut::<ModelicaDocumentRegistry>()
                .allocate(source.clone()),
        };
        let entity = world.spawn((
            Name::new(model_name.clone()),
            ModelicaModel {
                model_path: "".into(),
                model_name: model_name.clone(),
                current_time: 0.0,
                last_step_time: 0.0,
                session_id,
                paused: false,
                parameters: params,
                inputs: runtime_inputs.into_iter().map(|n| (n, 0.0)).collect(),
                variables: HashMap::new(),
                document: doc,
                is_stepping: true,
            },
        )).id();
        world.resource_mut::<ModelicaDocumentRegistry>().link(entity, doc);
        if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
            s.selected_entity = Some(entity);
        }
        should_compile = true;
    }

    if !should_compile {
        return;
    }

    let target = world.get_resource::<WorkbenchState>()
        .and_then(|s| s.selected_entity).unwrap_or(Entity::PLACEHOLDER);

    let target_doc = if target != Entity::PLACEHOLDER {
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let doc = registry.document_of(target);
        let doc = if let Some(d) = doc {
            registry.checkpoint_source(d, source.clone());
            d
        } else {
            let d = registry.allocate(source.clone());
            registry.link(target, d);
            d
        };
        if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, target) {
            if m.document != doc {
                m.document = doc;
            }
        }
        Some(doc)
    } else { None };

    if let Some(doc) = target_doc {
        world.resource_mut::<crate::ui::CompileStates>()
            .set(doc, crate::ui::CompileState::Compiling);
    }

    if let Some(channels) = world.get_resource::<ModelicaChannels>() {
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity: target,
            session_id,
            model_name,
            source,
        });
    }
}

/// Undo one op on the currently-selected document and sync the editor
/// buffer to match. No-op if nothing is selected, read-only, or there's
/// nothing to undo.
pub fn apply_document_undo(world: &mut World) {
    apply_document_undo_or_redo(world, /*is_undo=*/ true);
}

/// Redo one op on the currently-selected document and sync the editor
/// buffer to match. Counterpart of [`apply_document_undo`].
pub fn apply_document_redo(world: &mut World) {
    apply_document_undo_or_redo(world, /*is_undo=*/ false);
}

fn apply_document_undo_or_redo(world: &mut World, is_undo: bool) {
    let is_read_only = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.read_only)
        .unwrap_or(false);
    if is_read_only {
        return;
    }
    let Some(entity) = world.resource::<WorkbenchState>().selected_entity else { return };
    let Some(doc) = world.resource::<ModelicaDocumentRegistry>().document_of(entity) else { return };

    let new_source = {
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let result = registry.host_mut(doc).and_then(|host| {
            let changed = if is_undo {
                host.undo().ok().unwrap_or(false)
            } else {
                host.redo().ok().unwrap_or(false)
            };
            changed.then(|| host.document().source().to_string())
        });
        // Bypasses `checkpoint_source`, so emit a change notification
        // so observers (diagram re-parse, etc.) see the revert.
        if result.is_some() {
            registry.mark_changed(doc);
        }
        result
    };

    let Some(source) = new_source else { return };

    // Sync the reverted / advanced source into the editor buffer so the
    // user sees the change immediately.
    let mut new_starts = vec![0];
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            new_starts.push(i + 1);
        }
    }
    let detected = extract_model_name(&source);
    let hash = hash_content(&source);
    {
        let mut buf_state = world.resource_mut::<EditorBufferState>();
        buf_state.text = source.clone();
        buf_state.line_starts = new_starts.into();
        buf_state.detected_name = detected;
        buf_state.source_hash = hash;
    }
    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
        s.editor_buffer = source;
    }
}

pub fn modelica_layouter(style: &egui::Style, src: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font_id = egui::TextStyle::Monospace.resolve(style);

    // Expanded Modelica keywords
    let keywords = [
        "model", "end", "parameter", "Real", "Integer", "Boolean", "String",
        "equation", "algorithm", "if", "then", "else", "elseif", "for", "loop",
        "connect", "connector", "input", "output", "partial", "extends",
        "package", "type", "within", "final", "inner", "outer", "der",
        "function", "class", "record", "block", "constant", "discrete",
        "while", "when", "initial", "protected", "public", "import", "flow", "stream",
    ];

    let mut current_idx = 0;
    while current_idx < src.len() {
        let remaining = &src[current_idx..];
        
        // Single line comments
        if remaining.starts_with("//") {
            let line_end = remaining.find('\n').unwrap_or(remaining.len());
            job.append(&remaining[..line_end], 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color: egui::Color32::from_rgb(100, 150, 100),
                ..Default::default()
            });
            current_idx += line_end;
            continue;
        }

        // Multi-line comment (basic handling for single line segments in virtualized view)
        if remaining.starts_with("/*") {
            let end_idx = remaining.find("*/").map(|i| i + 2).unwrap_or(remaining.len());
            job.append(&remaining[..end_idx], 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color: egui::Color32::from_rgb(100, 150, 100),
                ..Default::default()
            });
            current_idx += end_idx;
            continue;
        }

        let mut chars = remaining.chars();
        let first_char = match chars.next() {
            Some(c) => c,
            None => break,
        };
        
        if first_char.is_alphabetic() || first_char == '_' {
            let word_end = remaining.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(remaining.len());
            let word = &remaining[..word_end];
            
            let color = if keywords.contains(&word) {
                egui::Color32::from_rgb(255, 120, 120) // Keywords
            } else if word.chars().next().unwrap().is_uppercase() {
                egui::Color32::from_rgb(120, 180, 255) // Types/Classes
            } else {
                egui::Color32::from_rgb(230, 230, 230) // Names
            };

            job.append(word, 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            });
            current_idx += word_end;
        } else if first_char.is_numeric() {
            let num_end = remaining.find(|c: char| !c.is_numeric() && c != '.' && c != 'e' && c != 'E').unwrap_or(remaining.len());
            job.append(&remaining[..num_end], 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color: egui::Color32::from_rgb(150, 200, 255), // Numbers
                ..Default::default()
            });
            current_idx += num_end;
        } else {
            let color = if "+-*/=^<>(){}[],;".contains(first_char) {
                egui::Color32::from_rgb(255, 200, 100) // Operators
            } else {
                egui::Color32::from_rgb(180, 180, 180) // Whitespace/Other
            };
            job.append(&remaining[..first_char.len_utf8()], 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            });
            current_idx += first_char.len_utf8();
        }
    }

    job
}
