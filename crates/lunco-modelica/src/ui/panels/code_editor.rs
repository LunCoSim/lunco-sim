//! Code Editor panel — central viewport for Modelica source code.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ui::WorkbenchState;
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
}

impl Default for EditorBufferState {
    fn default() -> Self {
        Self {
            source_hash: 0,
            model_path: String::new(),
            text: String::new(),
            line_starts: vec![0].into(),
            detected_name: None,
        }
    }
}

/// Code Editor panel — central viewport for Modelica source code.
pub struct CodeEditorPanel;

impl WorkbenchPanel for CodeEditorPanel {
    fn id(&self) -> &str { "modelica_code_preview" }
    fn title(&self) -> String { "📝 Code Editor".into() }
    fn closable(&self) -> bool { false }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(40, 40, 45))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
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
                let (source, line_starts, detected_name) = {
                    let state = world.resource::<WorkbenchState>();
                    let m = state.open_model.as_ref().unwrap();
                    (m.source.to_string(), m.line_starts.clone(), m.detected_name.clone())
                };
                
                let mut buf_state = world.resource_mut::<EditorBufferState>();
                buf_state.text = source;
                buf_state.line_starts = line_starts;
                buf_state.model_path = path.clone();
                buf_state.source_hash = hash_content(&buf_state.text);
                buf_state.detected_name = detected_name;
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

        let display_name = display_name.unwrap();

        // ── Top bar ──
        let mut compile_clicked = false;
        let mut clear_error = false;

        ui.horizontal(|ui| {
            let buf_state = world.resource::<EditorBufferState>();
            ui.label(format!("{} ({})",
                display_name,
                buf_state.detected_name.as_deref().unwrap_or("...")));

            if is_read_only {
                ui.colored_label(egui::Color32::from_rgb(200, 150, 50), "👁 Read-only");
            }

            ui.separator();

            if let Some(ref err) = compilation_error {
                ui.colored_label(egui::Color32::LIGHT_RED, "⚠️ Error");
                ui.label(err);
                if ui.button("Clear").clicked() {
                    clear_error = true;
                }
            } else {
                ui.colored_label(egui::Color32::GREEN, "Ready");
            }

            if !is_read_only && ui.button("🚀 COMPILE & RUN").clicked() {
                compile_clicked = true;
            }
        });
        ui.separator();

        if clear_error {
            if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                s.compilation_error = None;
            }
        }

        if compile_clicked {
            let buf_state = world.resource::<EditorBufferState>();
            if let Some(model_name) = buf_state.detected_name.clone() {
                let source = buf_state.text.clone();
                let params = extract_parameters(&source);
                let inputs_with_defaults = extract_inputs_with_defaults(&source);
                let runtime_inputs = extract_input_names(&source);

                let mut session_id = 0;
                let mut should_compile = false;

                if let Some(entity) = selected_entity {
                    if let Ok(mut model) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
                        let old_inputs = std::mem::take(&mut model.inputs);
                        model.session_id += 1;
                        model.is_stepping = true;
                        model.model_name = model_name.clone();
                        model.original_source = source.clone().into();
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
                    let entity = world.spawn((
                        Name::new(model_name.clone()),
                        ModelicaModel {
                            model_path: "".into(),
                            model_name: model_name.clone(),
                            original_source: source.clone().into(),
                            current_time: 0.0,
                            last_step_time: 0.0,
                            session_id,
                            paused: false,
                            parameters: params,
                            inputs: runtime_inputs.into_iter().map(|n| (n, 0.0)).collect(),
                            variables: HashMap::new(),
                            is_stepping: true,
                        },
                    )).id();

                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.selected_entity = Some(entity);
                    }
                    should_compile = true;
                }

                if should_compile {
                    if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                        let target = world.get_resource::<WorkbenchState>()
                            .and_then(|s| s.selected_entity).unwrap_or(Entity::PLACEHOLDER);
                        let _ = channels.tx.send(ModelicaCommand::Compile {
                            entity: target,
                            session_id,
                            model_name: model_name.clone(),
                            source,
                        });
                    }
                }
            } else {
                if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                    s.compilation_error = Some("Could not find a valid model declaration.".to_string());
                }
            }
        }

        // ── Editor area ──
        let mut buffer_changed = false;
        let mut new_text = String::new();

        if is_read_only {
            // High-performance virtualized viewer for large read-only STL files
            let buf_state = world.resource::<EditorBufferState>();
            let source = &buf_state.text;
            let line_starts = &buf_state.line_starts;
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let row_height = ui.spacing().interact_size.y.max(font_id.size);

            egui::ScrollArea::both()
                .auto_shrink([false; 2])
                .show_rows(ui, row_height, line_starts.len(), |ui, row_range| {
                    ui.style_mut().spacing.item_spacing.y = 0.0;
                    for row in row_range {
                        let start = line_starts[row];
                        let end = if row + 1 < line_starts.len() {
                            line_starts[row + 1].saturating_sub(1) // exclude \n
                        } else {
                            source.len()
                        };
                        
                        let line_str = source.get(start..end).unwrap_or("");
                        let job = modelica_layouter(ui, line_str);
                        
                        // Line numbers (non-selectable)
                        ui.horizontal(|ui| {
                            ui.add_sized([30.0, row_height], egui::Label::new(
                                egui::RichText::new(format!("{:>3}", row + 1))
                                    .size(10.0)
                                    .color(egui::Color32::DARK_GRAY)
                            ).selectable(false));
                            ui.label(job);
                        });
                    }
                });
        } else {
            // Standard editor for writable files
            egui::ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
                let mut buf_state = world.resource_mut::<EditorBufferState>();
                let mut text = buf_state.text.as_str();
                
                let output = egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(40)
                    .lock_focus(true)
                    .interactive(true)
                    .layouter(&mut |ui, string, wrap_width| {
                        let mut layout_job = modelica_layouter(ui, string.as_str());
                        layout_job.wrap.max_width = wrap_width;
                        ui.painter().layout_job(layout_job)
                    })
                    .show(ui);

                if output.response.changed() {
                    buf_state.text = text.to_string();
                    // Recompute line starts for the editor buffer if changed
                    let mut new_starts = vec![0];
                    for (i, byte) in buf_state.text.as_bytes().iter().enumerate() {
                        if *byte == b'\n' {
                            new_starts.push(i + 1);
                        }
                    }
                    buf_state.line_starts = new_starts.into();
                    buf_state.detected_name = extract_model_name(&buf_state.text);
                    new_text = buf_state.text.clone();
                    buffer_changed = true;
                }
            });
        }

        if buffer_changed {
            if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                if state.editor_buffer != new_text {
                    state.editor_buffer = new_text;
                }
            }
        }
    }
}

fn modelica_layouter(ui: &egui::Ui, src: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());

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
