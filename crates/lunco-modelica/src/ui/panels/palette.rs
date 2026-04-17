//! Component Palette — search-first flat list of instantiable MSL components.
//!
//! Solves the research-flagged pain with the Libraries browser: users
//! don't want to navigate `Modelica > Electrical > Analog > Basic >
//! Resistor` every time they need a resistor — they want to type
//! "resis", see results, and drop one onto the canvas. Figma / UE
//! asset-browser style.
//!
//! **What's in the palette**: every *leaf* component from
//! [`crate::visual_diagram::msl_component_library`] — interfaces and
//! package nodes are excluded. One row per component. Click a row to
//! instantiate on the active Diagram tab (placement cycles through a
//! 3-column grid to avoid overlap).
//!
//! **Search**: case-insensitive substring match against the component's
//! display name, full MSL path, category, and description. Top-100
//! matches rendered; typing narrows quickly. No fuzzy-matching
//! library is pulled in yet — substring + simple scoring is enough
//! for ~1-5k components.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::ui::panels::diagram::DiagramState;
use crate::visual_diagram::{msl_component_library, MSLComponentDef};

/// Panel id — registered as a singleton panel, slotted RightInspector.
pub const PALETTE_PANEL_ID: PanelId = PanelId("modelica_component_palette");

/// Per-frame UI state for the palette. Holds the search query.
///
/// Everything else (the component catalog) is static, owned by
/// `msl_component_library()`; we just filter over its slice.
#[derive(Resource, Default)]
pub struct PaletteState {
    /// Current search query — normalized on compare (lowercase).
    pub query: String,
}

/// The panel. Zero-sized; state lives in [`PaletteState`].
pub struct ComponentPalettePanel;

impl Panel for ComponentPalettePanel {
    fn id(&self) -> PanelId {
        PALETTE_PANEL_ID
    }

    fn title(&self) -> String {
        "🧩 Components".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn closable(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Ensure the per-frame state resource exists (panels are
        // instantiated once; the resource may or may not be present).
        if world.get_resource::<PaletteState>().is_none() {
            world.insert_resource(PaletteState::default());
        }

        // Snapshot the query up front.
        let query = world.resource::<PaletteState>().query.clone();
        let query_lc = query.to_lowercase();

        // Render the search box; capture any edit.
        let mut new_query = query.clone();
        ui.horizontal(|ui| {
            ui.label("🔍");
            let response = ui.add(
                egui::TextEdit::singleline(&mut new_query)
                    .hint_text("Search components…")
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                // Written back below, outside this closure.
            }
            // Escape clears the query.
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                new_query.clear();
            }
        });
        if new_query != query {
            world.resource_mut::<PaletteState>().query = new_query.clone();
        }

        ui.separator();

        // ── Filter + rank ──
        // Score higher for:
        //   +3 exact name match
        //   +2 name starts with query
        //   +1 name contains query
        //   +0.5 category/description contains query
        // Drop anything with score 0 unless the query is empty.
        let lib = msl_component_library();
        let mut scored: Vec<(&MSLComponentDef, f32)> = if query_lc.is_empty() {
            lib.iter().map(|c| (c, 0.0)).collect()
        } else {
            lib.iter()
                .filter_map(|c| {
                    let score = score_component(c, &query_lc);
                    (score > 0.0).then_some((c, score))
                })
                .collect()
        };
        // Sort: higher score first, then by name for stable ordering.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.display_name.cmp(&b.0.display_name))
        });

        let total = lib.len();
        let shown_cap = 100;
        let shown = scored.len().min(shown_cap);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if query_lc.is_empty() {
                    format!("{} components", total)
                } else {
                    format!("{} of {} matching", scored.len(), total)
                })
                .size(10.0)
                .color(egui::Color32::GRAY),
            );
            ui.add_space(ui.available_width() - 100.0);
            if !query_lc.is_empty() && ui.small_button("✕ clear").clicked() {
                world.resource_mut::<PaletteState>().query.clear();
            }
        });
        ui.separator();

        // ── Result list ──
        // Defer world mutations until after the render closure to
        // avoid holding a ResMut across egui callbacks.
        let mut clicked: Option<MSLComponentDef> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (comp, _score) in scored.iter().take(shown) {
                    let label =
                        egui::RichText::new(&comp.display_name).size(12.0);
                    let sub = egui::RichText::new(&comp.category)
                        .size(9.0)
                        .color(egui::Color32::GRAY);

                    let resp = ui.add(
                        egui::Button::new(label)
                            .min_size(egui::vec2(ui.available_width(), 0.0))
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE),
                    );
                    // Subtitle row just under the button.
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(sub);
                    });

                    let tooltip = comp
                        .description
                        .as_deref()
                        .unwrap_or(comp.msl_path.as_str());
                    let resp = resp.on_hover_text(format!(
                        "{}\n\n{}\n\nClick to add to the active diagram.",
                        comp.msl_path, tooltip
                    ));

                    if resp.clicked() {
                        clicked = Some((*comp).clone());
                    }

                    ui.separator();
                }
                if scored.len() > shown_cap {
                    ui.label(
                        egui::RichText::new(format!(
                            "+ {} more — refine the search",
                            scored.len() - shown_cap
                        ))
                        .size(10.0)
                        .italics()
                        .color(egui::Color32::GRAY),
                    );
                }
            });

        // ── Side-effect: instantiate clicked component ──
        if let Some(def) = clicked {
            if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                state.placement_counter += 1;
                let x = 100.0 + (state.placement_counter % 3) as f32 * 200.0;
                let y = 80.0 + (state.placement_counter / 3) as f32 * 160.0;
                state.add_component(def, egui::Pos2::new(x, y));
            }
        }
    }
}

/// Score a component against a lowercased query. Higher = better.
/// Returns 0 for no match.
fn score_component(c: &MSLComponentDef, query_lc: &str) -> f32 {
    let name_lc = c.display_name.to_lowercase();
    let path_lc = c.msl_path.to_lowercase();
    let cat_lc = c.category.to_lowercase();
    let desc_lc = c
        .description
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();

    if name_lc == query_lc {
        return 10.0;
    }
    let mut score = 0.0;
    if name_lc.starts_with(query_lc) {
        score += 5.0;
    }
    if name_lc.contains(query_lc) {
        score += 3.0;
    }
    if path_lc.contains(query_lc) {
        score += 1.5;
    }
    if cat_lc.contains(query_lc) {
        score += 1.0;
    }
    if desc_lc.contains(query_lc) {
        score += 0.5;
    }
    score
}
