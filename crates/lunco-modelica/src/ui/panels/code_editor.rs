//! Code Editor panel — central viewport for Modelica source code.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::ui::WorkbenchState;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};
use crate::ast_extract::{
    extract_model_name, extract_parameters, extract_inputs_with_defaults,
    extract_input_names, hash_content,
};

/// Code Editor panel — central viewport for Modelica source code.
pub struct CodeEditorPanel;

impl WorkbenchPanel for CodeEditorPanel {
    fn id(&self) -> &str { "modelica_preview" }
    fn title(&self) -> String { "📝 Code Editor".into() }
    fn closable(&self) -> bool { false }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    /// No tab bar — acts as the main viewport.
    fn hide_tab(&self) -> bool { true }
    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(40, 40, 45))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Auto-select first ModelicaModel entity if none selected (matches old behavior)
        {
            let needs_select = world.get_resource::<WorkbenchState>()
                .map_or(true, |s| s.selected_entity.is_none());
            if needs_select {
                type Q = bevy::ecs::query::QueryState<Entity, bevy::ecs::query::With<crate::ModelicaModel>>;
                let mut query_state = Q::new(world);
                if let Some(entity) = query_state.iter(world).next() {
                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.selected_entity = Some(entity);
                    }
                }
            }
        }

        // Step 1: Read state
        let (entity, editor_buffer, compilation_error, display_name) = {
            let state = match world.get_resource::<WorkbenchState>() {
                Some(s) => s,
                None => { ui.label("No state"); return; },
            };
            let e = state.selected_entity;
            let buf = state.editor_buffer.clone();
            let err = state.compilation_error.clone();
            let name = e.and_then(|e| world.query::<Option<&Name>>().get(world, e).ok().flatten())
                .map(|n| n.as_str().to_string());
            (e, buf, err, name)
        };

        let Some(entity) = entity else {
            ui.label("No model loaded. Load a .mo file from the Library Browser.");
            return;
        };

        // Step 2: Top bar
        ui.horizontal(|ui| {
            let detected = extract_model_name(&editor_buffer);
            ui.label(format!("{} ({})",
                display_name.as_deref().unwrap_or("Unnamed Model"),
                detected.as_deref().unwrap_or("Unknown")));

            ui.separator();

            if compilation_error.is_some() {
                ui.colored_label(egui::Color32::LIGHT_RED, "⚠️ Error");
                if ui.button("Clear").clicked() {
                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.compilation_error = None;
                    }
                }
            } else {
                ui.colored_label(egui::Color32::GREEN, "Ready");
            }

            if ui.button("🚀 COMPILE & RUN").clicked() {
                if let Some(model_name) = detected {
                    let mut should_compile = false;
                    let mut session_id = 0;

                    // Read model params
                    let params;
                    let inputs_with_defaults;
                    let runtime_inputs;
                    {
                        params = extract_parameters(&editor_buffer);
                        inputs_with_defaults = extract_inputs_with_defaults(&editor_buffer);
                        runtime_inputs = extract_input_names(&editor_buffer);
                    }

                    // Update model component
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

                    // Send command
                    if should_compile {
                        if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                            let _ = channels.tx.send(ModelicaCommand::Compile {
                                entity,
                                session_id,
                                model_name,
                                source: editor_buffer.clone(),
                            });
                        }
                    }
                } else {
                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.compilation_error = Some("Could not find a valid model declaration.".to_string());
                    }
                }
            }
        });
        ui.separator();

        // Step 3: Editor area — fills ALL remaining space in this tile
        let editor_id = egui::Id::new("editor_content_hash");
        let content_changed = {
            let prev_hash = ui.memory(|mem| mem.data.get_temp::<u64>(editor_id));
            let curr_hash = hash_content(&editor_buffer);
            ui.memory_mut(|mem| mem.data.insert_temp(editor_id, curr_hash));
            prev_hash != Some(curr_hash)
        };

        let _scroll_id = if content_changed { "editor_reset" } else { "editor" };

        // Use max_rect (the tile's bounded rect) to stay within tile boundaries
        let tile_rect = ui.max_rect();
        let line_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let rows = ((tile_rect.height() / line_h) + 0.5).ceil().max(10.0) as usize;
        let mut buf = editor_buffer.clone();
        ui.add(
            egui::TextEdit::multiline(&mut buf)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_width(tile_rect.width())
                .desired_rows(rows)
                .lock_focus(true),
        );
        // Sync buffer back if changed
        if buf != editor_buffer {
            if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                s.editor_buffer = buf;
            }
        }
    }
}
