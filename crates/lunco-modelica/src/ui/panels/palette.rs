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

/// Per-frame UI state for the palette. Holds the search query + the
/// active category filter chip.
///
/// Everything else (the component catalog) is static, owned by
/// `msl_component_library()`; we just filter over its slice.
#[derive(Resource, Default)]
pub struct PaletteState {
    /// Current search query — normalized on compare (lowercase).
    pub query: String,
    /// Selected top-level category chip (`None` = "All"). Derived
    /// from the MSL path's first segment after `Modelica.` — e.g.
    /// `Modelica.Electrical.…` → `Some("Electrical")`.
    pub category: Option<&'static str>,
}

/// The categories we surface as filter chips, in display order.
/// Derived from Modelica's top-level packages; anything that doesn't
/// match one of these falls under `"Other"`.
///
/// The MSL top-level packages we surface as category chips, in
/// display order. Chip colours come from the schematic-token set in
/// `lunco-theme` (see [`category_color`]) — this file no longer
/// hardcodes palette picks. If you need a new category, add its
/// name here and add the Modelica-side → schematic-token mapping
/// in [`category_color`].
const CATEGORIES: &[&str] = &[
    "Electrical",
    "Mechanical",
    "Thermal",
    "Fluid",
    "Media",
    "Magnetic",
    "Blocks",
    "Math",
    "StateGraph",
    "Other",
];

/// Map a category name to its chip colour via the current theme's
/// schematic tokens. Mapping choices track domain intent —
/// `Electrical → wire_electrical`, `Mechanical → wire_mechanical`,
/// etc. — so a theme override of `wire_electrical` propagates to
/// the palette automatically.
///
/// For categories with no obvious wire-domain cognate (`Blocks`,
/// `Math`, `StateGraph`) we land on signal/integer/boolean tokens —
/// they're generic "processing" colours that don't carry a physical
/// domain meaning, which matches how those categories feel.
fn category_color(name: &str, theme: &lunco_theme::Theme) -> egui::Color32 {
    let s = &theme.schematic;
    match name {
        "Electrical" => s.wire_electrical,
        "Mechanical" => s.wire_mechanical,
        "Thermal" => s.wire_thermal,
        "Fluid" => s.wire_fluid,
        "Media" => s.wire_fluid,
        "Magnetic" => s.wire_multibody,
        "Blocks" => s.wire_signal,
        "Math" => s.wire_integer,
        "StateGraph" => s.wire_boolean,
        _ => s.wire_unknown,
    }
}

/// Match a component's MSL path to one of our display categories.
fn category_of(msl_path: &str) -> &'static str {
    // Strip "Modelica." prefix if present, then take the first
    // segment before the next dot. Non-Modelica libraries land in
    // "Other" for now.
    let after_modelica = msl_path.strip_prefix("Modelica.").unwrap_or(msl_path);
    let first = after_modelica.split('.').next().unwrap_or("Other");
    for &c in CATEGORIES {
        if c == first {
            return c;
        }
    }
    "Other"
}

fn category_info(name: &str) -> Option<&'static str> {
    CATEGORIES.iter().copied().find(|c| *c == name)
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
        // Snapshot the theme at the top so every chip / row pulls
        // its colour from the same source. Category tints map to
        // schematic-wire tokens; secondary text uses `text_subdued`.
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let all_chip_color = theme.tokens.text_subdued;
        let muted_text = theme.tokens.text_subdued;

        // Ensure the per-frame state resource exists (panels are
        // instantiated once; the resource may or may not be present).
        if world.get_resource::<PaletteState>().is_none() {
            world.insert_resource(PaletteState::default());
        }

        // Snapshot the query + selected category up front.
        let state = world.resource::<PaletteState>();
        let query = state.query.clone();
        let query_lc = query.to_lowercase();
        let selected_category: Option<&'static str> = state.category;

        // Render the search box; capture any edit.
        let mut new_query = query.clone();
        let mut new_category = selected_category;
        let mut clear_all = false;

        ui.horizontal(|ui| {
            ui.label("🔍");
            let response = ui.add(
                egui::TextEdit::singleline(&mut new_query)
                    .hint_text("Search components…")
                    .desired_width(f32::INFINITY),
            );
            // Escape clears the query.
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                new_query.clear();
            }
        });

        // Precompute per-category counts for the chip labels (filtered
        // by the current search, so a chip says "Electrical (7)" = 7
        // matches in this category given the current query).
        let lib = msl_component_library();
        let mut cat_counts: std::collections::HashMap<&'static str, usize> =
            std::collections::HashMap::new();
        let pre_filter_total = lib.len();
        let mut pre_filter_matches = 0usize;
        for c in lib {
            let matches = if query_lc.is_empty() {
                true
            } else {
                score_component(c, &query_lc) > 0.0
            };
            if matches {
                pre_filter_matches += 1;
                *cat_counts.entry(category_of(&c.msl_path)).or_insert(0) += 1;
            }
        }

        // ── Category chips ──
        // `All` + one chip per known category. Chips with zero matches
        // are dimmed but still clickable (they'll just show an empty
        // list). Scroll horizontally on narrow docks.
        egui::ScrollArea::horizontal()
            .id_salt("palette_categories")
            .max_height(26.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let all_count = pre_filter_matches;
                    if chip(
                        ui,
                        "All",
                        all_chip_color,
                        all_count,
                        selected_category.is_none(),
                    ) {
                        new_category = None;
                    }
                    for &cat in CATEGORIES {
                        let count = cat_counts.get(cat).copied().unwrap_or(0);
                        if chip(
                            ui,
                            cat,
                            category_color(cat, &theme),
                            count,
                            selected_category == Some(cat),
                        ) {
                            new_category = Some(cat);
                        }
                    }
                });
            });

        // Header row: summary + clear-filter.
        ui.horizontal(|ui| {
            let visible = if let Some(cat) = selected_category {
                cat_counts.get(cat).copied().unwrap_or(0)
            } else {
                pre_filter_matches
            };
            ui.label(
                egui::RichText::new(if query_lc.is_empty() && selected_category.is_none() {
                    format!("{} components", pre_filter_total)
                } else {
                    format!("{} of {} matching", visible, pre_filter_total)
                })
                .size(10.0)
                .color(muted_text),
            );
            if (!query_lc.is_empty() || selected_category.is_some())
                && ui.small_button("✕ clear").clicked()
            {
                clear_all = true;
            }
        });
        ui.separator();

        // Write back state changes.
        if clear_all {
            let mut s = world.resource_mut::<PaletteState>();
            s.query.clear();
            s.category = None;
            // Skip subsequent rendering with stale query.
            return;
        }
        if new_query != query || new_category != selected_category {
            let mut s = world.resource_mut::<PaletteState>();
            s.query = new_query.clone();
            s.category = new_category;
        }
        let query_lc = new_query.to_lowercase();
        let selected_category = new_category;

        // ── Filter + rank ──
        // Score higher for:
        //   +10 exact name match
        //   +5 name starts with query
        //   +3 name contains query
        //   +1.5 path contains query
        //   +1 category contains query
        //   +0.5 description contains query
        // Plus: category filter acts as a hard gate.
        let mut scored: Vec<(&MSLComponentDef, f32)> = lib
            .iter()
            .filter_map(|c| {
                if let Some(cat) = selected_category {
                    if category_of(&c.msl_path) != cat {
                        return None;
                    }
                }
                let score = if query_lc.is_empty() {
                    0.0
                } else {
                    score_component(c, &query_lc)
                };
                if query_lc.is_empty() || score > 0.0 {
                    Some((c, score))
                } else {
                    None
                }
            })
            .collect();
        // Sort: higher score first, then by name for stable ordering.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.display_name.cmp(&b.0.display_name))
        });

        let shown_cap = 100;
        let shown = scored.len().min(shown_cap);

        // ── Result list ──
        // When the user is searching or filtered to a single category,
        // render a flat (score-sorted) list — matches VS Code's
        // search-results behaviour. With no filter, group by category
        // into collapsible sections (Figma Assets style, OMEdit's
        // component-tree style). Each section is closed by default
        // except the first, keeping long category chains scannable.
        let mut clicked: Option<MSLComponentDef> = None;
        let is_searching = !query_lc.is_empty() || selected_category.is_some();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if is_searching {
                    // Flat list (top-100 matches, score-ordered).
                    for (comp, _score) in scored.iter().take(shown) {
                        if render_component_row(ui, comp, &theme) {
                            clicked = Some((*comp).clone());
                        }
                    }
                    if scored.len() > shown_cap {
                        ui.label(
                            egui::RichText::new(format!(
                                "+ {} more — refine the search",
                                scored.len() - shown_cap
                            ))
                            .size(10.0)
                            .italics()
                            .color(muted_text),
                        );
                    }
                } else {
                    // Grouped by top-level category — collapsible. First
                    // group (highest-count Electrical usually) is
                    // expanded by default so users see something
                    // immediately.
                    let mut groups: std::collections::BTreeMap<
                        &'static str,
                        Vec<&MSLComponentDef>,
                    > = std::collections::BTreeMap::new();
                    for (comp, _score) in scored.iter() {
                        let cat = category_of(&comp.msl_path);
                        groups.entry(cat).or_default().push(*comp);
                    }
                    // Render in CATEGORIES order so the color story is
                    // consistent across sessions.
                    let mut first = true;
                    for &cat in CATEGORIES {
                        let Some(list) = groups.get(cat) else {
                            continue;
                        };
                        let header = egui::CollapsingHeader::new(
                            egui::RichText::new(format!("{} ({})", cat, list.len()))
                                .color(category_color(cat, &theme))
                                .strong(),
                        )
                        .default_open(first)
                        .id_salt(("palette_cat", cat));
                        header.show(ui, |ui| {
                            for comp in list {
                                if render_component_row(ui, comp, &theme) {
                                    clicked = Some((*comp).clone());
                                }
                            }
                        });
                        first = false;
                    }
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

/// Draw one component row (category dot + name + subtitle). Returns
/// `true` if the user just clicked it (instantiate target).
/// Used by both the flat-search list and the grouped-by-category list.
fn render_component_row(
    ui: &mut egui::Ui,
    comp: &MSLComponentDef,
    theme: &lunco_theme::Theme,
) -> bool {
    let cat_name = category_of(&comp.msl_path);
    let cat_color = category_color(cat_name, theme);
    let muted = theme.tokens.text_subdued;

    let resp = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(8.0, 8.0),
                egui::Sense::hover(),
            );
            ui.painter().circle_filled(rect.center(), 4.0, cat_color);

            ui.vertical(|ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(&comp.display_name).size(12.0),
                ));
                ui.add(egui::Label::new(
                    egui::RichText::new(&comp.category)
                        .size(9.0)
                        .color(muted),
                ));
            });
        })
        .response
        .interact(egui::Sense::click());

    let tooltip = comp
        .description
        .as_deref()
        .unwrap_or(comp.msl_path.as_str());
    resp.on_hover_text(format!(
        "{}\n\n{}\n\nClick to add to the active diagram.",
        comp.msl_path, tooltip
    ))
    .clicked()
}

/// Draw one category chip. Returns `true` if the user just clicked
/// it. Selected chips render with a tinted background; non-selected
/// chips are outlined. Count is suffixed in parentheses.
fn chip(
    ui: &mut egui::Ui,
    name: &str,
    color: egui::Color32,
    count: usize,
    selected: bool,
) -> bool {
    // Tone down the fill for non-selected chips.
    let fill = if selected {
        color.linear_multiply(0.30)
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke_color = if count == 0 {
        egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90)
    } else {
        color
    };
    let label = if count > 0 {
        format!("{} ({})", name, count)
    } else {
        name.to_string()
    };
    // Selected chips paint white-on-dark regardless of theme mode:
    // the fill above is `color.linear_multiply(0.30)` — multiplying
    // any palette entry by 0.3 always lands in the "very dark"
    // quadrant, so a white glyph stays readable in both Mocha and
    // Latte. Non-selected chips inherit the category `color` itself,
    // which is already a schematic-token value.
    let resp = ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(11.0)
                .color(if selected { egui::Color32::WHITE } else { color }),
        )
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke_color)),
    );
    resp.clicked()
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
