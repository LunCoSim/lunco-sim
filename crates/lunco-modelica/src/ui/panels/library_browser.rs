//! Library Browser panel — file system navigation for Modelica models.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::ui::WorkbenchState;

/// Library Browser panel — file system navigation for Modelica models.
pub struct LibraryBrowserPanel;

impl WorkbenchPanel for LibraryBrowserPanel {
    fn id(&self) -> &str { "library_browser" }
    fn title(&self) -> String { "📁 Library".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(35, 35, 40))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mut state = match world.get_resource_mut::<WorkbenchState>() {
            Some(s) => s,
            None => return,
        };

        // Navigation bar
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

                use crate::models::BUNDLED_MODELS;
                for (name, source) in BUNDLED_MODELS {
                    if ui.button(format!("📄 {name}")).clicked() {
                        state.editor_buffer = source.to_string();
                    }
                }
            }

            #[cfg(not(target_arch = "wasm32"))]
            {
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
                if state.current_path != PathBuf::from("assets/models")
                    && state.current_path != PathBuf::from(".cache/msl")
                {
                    ui.separator();
                    if ui.button("⬅ Back").clicked() {
                        state.current_path.pop();
                    }
                }
            }
        });
    }
}
