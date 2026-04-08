//! Viewport-based docking system for the Modelica workbench.
//!
//! Architecture:
//! - A **Viewport** is the central view (CodeEditor, later 3D Scene)
//! - **Dock panels** attach to Left, Right, or Bottom edges
//! - Each edge has **tabbed panels** — click a tab to switch
//! - All borders are **resizable**
//! - Layout state is persisted in a `DockLayout` resource
//!
//! ## Adding a New Panel
//!
//! 1. Create a `render_xxx` function in `panels/`
//! 2. Add a `PanelId` variant
//! 3. Register it in `DockLayout::default()` for a region
//! 4. Add the render dispatch in `render_panel`

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Line, Plot, PlotPoints};
use std::path::PathBuf;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand, models::BUNDLED_MODELS,
            extract_model_name, extract_parameters, extract_inputs_with_defaults,
            extract_input_names, substitute_params_in_source, hash_content};

mod state;
pub use state::*;

#[cfg(target_arch = "wasm32")]
pub use state::update_file_load_result;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

// ─── Viewport Types ─────────────────────────────────────────────────────

/// Identifies the central viewport type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ViewportId {
    #[default]
    CodeEditor,
}

impl ViewportId {
    pub fn title(&self) -> &'static str {
        match self {
            ViewportId::CodeEditor => "📝 Code Editor",
        }
    }
}

/// Identifies a panel type for tabbing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum PanelId {
    LibraryBrowser,
    Telemetry,
    Graphs,
    Logs,
}

impl PanelId {
    pub fn title(&self) -> &'static str {
        match self {
            PanelId::LibraryBrowser => "📁 Library",
            PanelId::Telemetry => "📊 Telemetry",
            PanelId::Graphs => "📈 Graphs",
            PanelId::Logs => "📋 Logs",
        }
    }
}

// ─── Dock Layout State ─────────────────────────────────────────────────

/// Tracks which panels are visible in each region and their sizes.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PanelRegion {
    /// Available panel tabs in this region.
    pub panels: Vec<PanelId>,
    /// Index of the active tab.
    pub active: usize,
    /// Width (for left/right) or height (for bottom) in pixels.
    pub size: f32,
}

impl Default for PanelRegion {
    fn default() -> Self {
        Self {
            panels: Vec::new(),
            active: 0,
            size: 250.0,
        }
    }
}

/// The main dock layout resource.
#[derive(Resource, serde::Serialize, serde::Deserialize)]
pub struct DockLayout {
    pub left: PanelRegion,
    pub right: PanelRegion,
    pub bottom: PanelRegion,
    pub viewport: ViewportId,
}

impl DockLayout {
    /// Default layout: Library left, Telemetry right, Graphs+Logs bottom tabbed.
    pub fn default_layout() -> Self {
        Self {
            left: PanelRegion {
                panels: vec![PanelId::LibraryBrowser],
                active: 0,
                size: 220.0,
            },
            right: PanelRegion {
                panels: vec![PanelId::Telemetry],
                active: 0,
                size: 320.0,
            },
            bottom: PanelRegion {
                panels: vec![PanelId::Graphs, PanelId::Logs],
                active: 0,
                size: 280.0,
            },
            viewport: ViewportId::CodeEditor,
        }
    }

    /// Check if a panel is visible in any region.
    pub fn is_panel_visible(&self, panel: PanelId) -> bool {
        self.left.panels.contains(&panel)
            || self.right.panels.contains(&panel)
            || self.bottom.panels.contains(&panel)
    }

    /// Toggle a panel's visibility in its default region.
    pub fn toggle_panel(&mut self, panel: PanelId) {
        if self.is_panel_visible(panel) {
            // Remove from all regions
            self.left.panels.retain(|p| *p != panel);
            self.right.panels.retain(|p| *p != panel);
            self.bottom.panels.retain(|p| *p != panel);
            // Fix active indices
            self.fix_active_indices();
        } else {
            // Add to default region
            match panel {
                PanelId::LibraryBrowser => {
                    self.left.panels.push(panel);
                    self.left.active = self.left.panels.len() - 1;
                }
                PanelId::Telemetry => {
                    self.right.panels.push(panel);
                    self.right.active = self.right.panels.len() - 1;
                }
                PanelId::Graphs | PanelId::Logs => {
                    self.bottom.panels.push(panel);
                    self.bottom.active = self.bottom.panels.len() - 1;
                }
            }
        }
    }

    fn fix_active_indices(&mut self) {
        if self.left.active >= self.left.panels.len() {
            self.left.active = self.left.panels.len().saturating_sub(1);
        }
        if self.right.active >= self.right.panels.len() {
            self.right.active = self.right.panels.len().saturating_sub(1);
        }
        if self.bottom.active >= self.bottom.panels.len() {
            self.bottom.active = self.bottom.panels.len().saturating_sub(1);
        }
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::default_layout()
    }
}

// ─── Plugin ─────────────────────────────────────────────────────────────

/// Meta-plugin that assembles the Modelica workbench with a viewport-based dock system.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
            .init_resource::<DockLayout>()
            .add_systems(EguiPrimaryContextPass, render_dock);
    }
}

// ─── Layout Rendering ───────────────────────────────────────────────────

/// Central layout system — renders the dock layout with all regions.
fn render_dock(
    mut contexts: EguiContexts,
    mut state: ResMut<WorkbenchState>,
    mut q_models: Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<Res<ModelicaChannels>>,
    mut layout: ResMut<DockLayout>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Auto-select first model entity
    if state.selected_entity.is_none() {
        if let Some((e, _, _)) = q_models.iter().next() {
            state.selected_entity = Some(e);
        }
    }

    // Clone layout data to avoid borrow conflicts
    let mut left_size = layout.left.size;
    let mut right_size = layout.right.size;
    let mut bottom_size = layout.bottom.size;
    let has_bottom = !layout.bottom.panels.is_empty();
    let viewport = layout.viewport;

    // === LEFT PANEL ===
    if !layout.left.panels.is_empty() {
        egui::SidePanel::left("left_panel")
            .default_width(left_size)
            .min_width(120.0).max_width(500.0)
            .resizable(true)
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(4.0))
            .show(ctx, |ui| {
                render_panel_region(ui, &mut layout.left, &mut state, &mut q_models, channels.as_deref());
            });
    }

    // === RIGHT PANEL ===
    if !layout.right.panels.is_empty() {
        egui::SidePanel::right("right_panel")
            .default_width(right_size)
            .min_width(200.0).max_width(600.0)
            .resizable(true)
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(4.0))
            .show(ctx, |ui| {
                render_panel_region(ui, &mut layout.right, &mut state, &mut q_models, channels.as_deref());
            });
    }

    // === CENTER AREA ===
    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(0.0))
        .show(ctx, |ui| {
            if has_bottom {
                let total_height = ui.available_height();
                let editor_height = (total_height - bottom_size - 1.0).max(30.0);
                let width = ui.available_width();

                // Editor area constrained to prevent overflow
                egui::Frame::none().inner_margin(0.0).outer_margin(0.0)
                    .fill(egui::Color32::from_gray(20))
                    .show(ui, |ui| {
                        ui.set_max_width(width);
                        ui.allocate_ui(egui::vec2(width, editor_height), |ui| {
                            render_viewport(ui, &viewport, &mut state, &mut q_models, channels.as_deref());
                        });
                    });

                ui.separator();
                let response = ui.allocate_response(egui::vec2(ui.available_width(), 4.0), egui::Sense::drag());
                if response.dragged() {
                    bottom_size = (bottom_size - ctx.input(|i| i.pointer.delta().y)).clamp(40.0, 600.0);
                }
                if response.hovered() || response.dragged() {
                    ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeVertical);
                }

                // Bottom area fills remaining width
                render_panel_region(ui, &mut layout.bottom, &mut state, &mut q_models, channels.as_deref());
            } else {
                render_viewport(ui, &viewport, &mut state, &mut q_models, channels.as_deref());
            }
        });

    // Write back sizes
    layout.left.size = left_size;
    layout.right.size = right_size;
    layout.bottom.size = bottom_size;
}

/// Renders a tabbed panel region (left, right, or bottom).
fn render_panel_region(
    ui: &mut egui::Ui,
    region: &mut PanelRegion,
    state: &mut WorkbenchState,
    q_models: &mut Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<&ModelicaChannels>,
) {
    // Tab bar
    if region.panels.len() > 1 {
        ui.horizontal(|ui| {
            for (i, &panel) in region.panels.iter().enumerate() {
                if ui.selectable_label(i == region.active, panel.title()).clicked() {
                    region.active = i;
                }
            }
        });
        ui.separator();
    }

    // Active panel content
    if let Some(&panel) = region.panels.get(region.active) {
        render_panel(panel, ui, state, q_models, channels);
    }
}

/// Dispatches to the correct panel render function.
fn render_panel(
    panel: PanelId,
    ui: &mut egui::Ui,
    state: &mut WorkbenchState,
    q_models: &mut Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<&ModelicaChannels>,
) {
    match panel {
        PanelId::LibraryBrowser => render_browser(ui, state),
        PanelId::Telemetry => render_telemetry(ui, state, q_models, channels),
        PanelId::Graphs => render_graphs(ui, state),
        PanelId::Logs => render_logs(ui, state),
    }
}

/// Renders the central viewport.
fn render_viewport(
    ui: &mut egui::Ui,
    viewport: &ViewportId,
    state: &mut WorkbenchState,
    q_models: &mut Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<&ModelicaChannels>,
) {
    match viewport {
        ViewportId::CodeEditor => render_editor(ui, state, q_models, channels),
    }
}

// ─── Panel Render Functions ─────────────────────────────────────────────

fn render_browser(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    ui.horizontal(|ui| {
        if ui.selectable_label(state.current_path.starts_with("assets/models"), "📦 Models").clicked() {
            state.current_path = PathBuf::from("assets/models");
        }
        if ui.selectable_label(state.current_path.starts_with(".cache/msl"), "📚 MSL").clicked() {
            state.current_path = PathBuf::from(".cache/msl");
        }
    });
    ui.separator();

    egui::ScrollArea::both().id_salt("browser_scroll").show(ui, |ui| {
        #[cfg(target_arch = "wasm32")]
        {
            use web_sys::HtmlInputElement;

            // Load .mo file from browser file picker
            if ui.button("📂 Load .mo File").clicked() {
                if let Some(window) = web_sys::window() {
                    if let Some(existing) = window.document().and_then(|d| d.get_element_by_id("__modelica_load")) {
                        existing.remove();
                    }
                    let document = window.document().unwrap();
                    let input = document.create_element("input").unwrap();
                    let input = input.dyn_into::<HtmlInputElement>().unwrap();
                    input.set_type("file");
                    input.set_attribute("accept", ".mo").unwrap();
                    input.set_attribute("style", "display:none").unwrap();
                    input.set_id("__modelica_load");
                    document.body().unwrap().append_child(&input).unwrap();

                    let onchange = wasm_bindgen::closure::Closure::once(move |_: web_sys::Event| {
                        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                            if let Some(inp) = doc.get_element_by_id("__modelica_load")
                                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                            {
                                if let Some(files) = inp.files() {
                                    if let Some(file) = files.get(0) {
                                        let reader = web_sys::FileReader::new().unwrap();
                                        let reader2 = reader.clone();
                                        let onload = wasm_bindgen::closure::Closure::once(move |_: web_sys::Event| {
                                            let text = reader2.result().unwrap().as_string().unwrap_or_default();
                                        crate::ui::state::set_file_load_result(&text);
                                        });
                                        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
                                        reader.read_as_text(&file).unwrap();
                                        onload.forget();
                                    }
                                }
                                inp.remove();
                            }
                        }
                    });
                    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
                    onchange.forget();
                    input.click();
                }
            }
            ui.separator();

            // Web: show bundled models (no filesystem access in browser)
            if state.current_path.starts_with("assets/models") || state.current_path == PathBuf::new() {
                for (name, _source) in BUNDLED_MODELS {
                    if ui.button(format!("📄 {}", name)).clicked() {
                        // Load bundled model source
                        let source = BUNDLED_MODELS.iter()
                            .find(|(n, _)| *n == *name)
                            .map(|(_, s)| *s)
                            .unwrap_or("");
                        state.editor_buffer = source.to_string();
                    }
                }
            } else if state.current_path.starts_with(".cache/msl") {
                ui.label("MSL not available in web mode.");
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Desktop: use filesystem
            if let Ok(entries) = std::fs::read_dir(&state.current_path) {
                let mut entries: Vec<_> = entries.flatten().collect();
                entries.sort_by_key(|e| e.file_name());
                for entry in entries {
                    let path = entry.path();
                    if path.is_dir() {
                        if ui.button(format!("📁 {}", path.file_name().unwrap().to_string_lossy())).clicked() {
                            state.current_path = path;
                        }
                    } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
                        if ui.button(format!("📄 {}", path.file_name().unwrap().to_string_lossy())).clicked() {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                state.editor_buffer = content;
                            }
                        }
                    }
                }
            }
            if state.current_path != PathBuf::from("assets/models") && state.current_path != PathBuf::from(".cache/msl") {
                ui.separator();
                if ui.button("⬅ Back").clicked() { state.current_path.pop(); }
            }
        }
    });
}

fn render_editor(
    ui: &mut egui::Ui,
    state: &mut WorkbenchState,
    q_models: &mut Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<&ModelicaChannels>,
) {
    let detected_name = extract_model_name(&state.editor_buffer);

    // Detect content change for scroll reset
    let editor_id = egui::Id::new("editor_content_hash");
    let prev_hash = ui.memory(|mem| mem.data.get_temp::<u64>(editor_id));
    let curr_hash = hash_content(&state.editor_buffer);
    let content_changed = prev_hash != Some(curr_hash);
    ui.memory_mut(|mem| mem.data.insert_temp(editor_id, curr_hash));

    // Top bar: title on left, compile button and status on right
    ui.horizontal(|ui| {
        // Title on left
        ui.heading(format!("Editor: {}", detected_name.as_deref().unwrap_or("Unknown")));

        // Fill remaining space
        let spacer = ui.available_width() - 300.0;
        if spacer > 0.0 { ui.add_space(spacer); }

        // Status text
        if state.compilation_error.is_some() {
            ui.colored_label(egui::Color32::LIGHT_RED, "⚠️ Error");
            if ui.button("Clear").clicked() { state.compilation_error = None; }
        } else {
            ui.colored_label(egui::Color32::GREEN, "Ready");
        }

        // Compile button (rightmost)
        if ui.button("🚀 COMPILE & RUN").clicked() {
            if let Some(model_name) = detected_name {
                if let (Some(entity), Some(ch)) = (state.selected_entity, channels) {
                    let params = extract_parameters(&state.editor_buffer);
                    let inputs_with_defaults = extract_inputs_with_defaults(&state.editor_buffer);
                    let runtime_inputs = extract_input_names(&state.editor_buffer);
                    if let Ok((_, mut model, _)) = q_models.get_mut(entity) {
                        let old_inputs: std::collections::HashMap<String, f64> = std::mem::take(&mut model.inputs);
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
                        let _ = ch.tx.send(ModelicaCommand::Compile {
                            entity, session_id: model.session_id,
                            model_name, source: state.editor_buffer.clone(),
                        });
                    }
                }
            } else {
                state.compilation_error = Some("Could not find a valid model declaration.".to_string());
            }
        }
    });
    ui.separator();

    // Editor scroll area - recreate with new ID when content changes to reset scroll
    let scroll_id = if content_changed { "editor_scroll_reset" } else { "editor_scroll" };
    let avail_width = ui.available_width();

    egui::ScrollArea::vertical()
        .id_salt(scroll_id)
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_max_width(avail_width);
            ui.add(egui::TextEdit::multiline(&mut state.editor_buffer)
                .font(egui::TextStyle::Monospace).code_editor()
                .desired_width(avail_width).lock_focus(true).desired_rows(35));
        });

    if let Some(err) = &state.compilation_error {
        ui.separator();
        egui::ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        });
    }
}

fn render_telemetry(
    ui: &mut egui::Ui,
    state: &mut WorkbenchState,
    q_models: &mut Query<(Entity, &mut ModelicaModel, Option<&Name>)>,
    channels: Option<&ModelicaChannels>,
) {
    let Some(entity) = state.selected_entity else {
        ui.label("No model selected.");
        return;
    };
    let Ok((_, mut model, name)) = q_models.get_mut(entity) else {
        ui.label("Model not found.");
        return;
    };

    let label = name.map(|n| n.as_str()).unwrap_or("Unnamed Model");
    ui.heading(format!("{} ({})", label, model.model_name));

    ui.horizontal(|ui| {
        if model.paused { if ui.button("▶ Play").clicked() { model.paused = false; } }
        else { if ui.button("⏸ Pause").clicked() { model.paused = true; } }
        ui.label(format!("Time: {:.4} s", model.current_time));
        ui.add_space(ui.available_width() - 70.0);
        if ui.button("🔄 Reset").clicked() {
            if let Some(ch) = channels {
                model.session_id += 1; model.is_stepping = true;
                let _ = ch.tx.send(ModelicaCommand::Reset { entity, session_id: model.session_id });
            }
            state.history.remove(&entity);
            model.current_time = 0.0; model.last_step_time = 0.0;
        }
    });
    ui.separator();

    if !model.parameters.is_empty() {
        ui.label("Parameters (Dynamic Tuning):");
        egui::ScrollArea::vertical().id_salt("params_scroll").max_height(150.0).show(ui, |ui| {
            let mut param_keys: Vec<_> = model.parameters.keys().cloned().collect();
            param_keys.sort();
            let mut changed = false;
            for key in &param_keys {
                ui.horizontal(|ui| {
                    ui.label(format!("{:16}:", key));
                    let val = model.parameters.get_mut(key).unwrap();
                    if ui.add(egui::DragValue::new(val).speed(0.01).fixed_decimals(2)).changed() { changed = true; }
                });
            }
            if changed {
                let modified = substitute_params_in_source(&state.editor_buffer, &model.parameters);
                if let Some(ch) = channels {
                    model.session_id += 1; model.is_stepping = true;
                    state.editor_buffer = modified.clone();
                    let _ = ch.tx.send(ModelicaCommand::UpdateParameters {
                        entity, session_id: model.session_id,
                        model_name: model.model_name.clone(), source: modified,
                    });
                }
            }
        });
        ui.separator();
    }
    if !model.inputs.is_empty() {
        ui.label("Inputs (Real-time):");
        egui::ScrollArea::vertical().id_salt("inputs_scroll").max_height(120.0).show(ui, |ui| {
            let mut input_keys: Vec<_> = model.inputs.keys().cloned().collect();
            input_keys.sort();
            for key in input_keys {
                ui.horizontal(|ui| {
                    ui.label(format!("{:16}:", key));
                    let val = model.inputs.get_mut(&key).unwrap();
                    ui.add(egui::DragValue::new(val).speed(0.1).fixed_decimals(2));
                });
            }
        });
        ui.separator();
    }
    ui.label("Variables (Toggle to Plot):");
    egui::ScrollArea::vertical().id_salt("telemetry_scroll").show(ui, |ui| {
        let mut all_names: Vec<_> = model.variables.keys().cloned().collect();
        all_names.extend(model.inputs.keys().cloned());
        all_names.sort();
        all_names.dedup();
        for name in all_names {
            ui.horizontal(|ui| {
                let mut is_plotted = state.plotted_variables.contains(&name);
                if ui.checkbox(&mut is_plotted, "").changed() {
                    if is_plotted { state.plotted_variables.insert(name.clone()); }
                    else { state.plotted_variables.remove(&name); }
                }
                ui.label(format!("{}:", name));
                ui.add_space(ui.available_width() - 60.0);
                if let Some(&val) = model.variables.get(&name) { ui.label(format!("{:.4}", val)); }
                else if let Some(&val) = model.inputs.get(&name) { ui.label(format!("{:.4}", val)); }
            });
        }
    });
}

fn render_graphs(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    ui.horizontal(|ui| {
        ui.heading("📈 Graphs");
        ui.add_space(ui.available_width() - 100.0);
        if ui.button("🎯 Auto-Fit").clicked() {
            state.plot_auto_fit = true;
        }
    });
    ui.separator();

    if let Some(entity) = state.selected_entity {
        if let Some(entity_history) = state.history.get(&entity) {
            let do_reset = state.plot_auto_fit;
            state.plot_auto_fit = false;
            let plotted = state.plotted_variables.clone();
            let entity_history = entity_history.clone();

            // Plot fills available width
            let avail = ui.available_size();
            let plot = Plot::new("modelica_plot")
                .view_aspect(2.5)
                .width(avail.x.max(100.0))
                .height((avail.y - 30.0).max(50.0))
                .legend(egui_plot::Legend::default())
                .allow_drag(true).allow_zoom(true).allow_scroll(true)
                .allow_double_click_reset(true);
            let plot = if do_reset { plot.reset() } else { plot };
            plot.show(ui, |plot_ui| {
                for name in &plotted {
                    if let Some(points) = entity_history.get(name) {
                        let pts: Vec<[f64; 2]> = points.iter().cloned().collect();
                        plot_ui.line(Line::new(name, PlotPoints::from(pts)));
                    }
                }
            });
        } else {
            ui.centered_and_justified(|ui| { ui.label("Wait for simulation data..."); });
        }
    } else {
        ui.centered_and_justified(|ui| { ui.label("Select a model to see plots."); });
    }
}

fn render_logs(ui: &mut egui::Ui, state: &mut WorkbenchState) {
    egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        for log in &state.logs {
            ui.label(log);
        }
    });
}
