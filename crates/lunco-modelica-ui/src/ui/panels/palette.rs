//! Component Palette — search-first flat list of instantiable source library components.
//!
//! Solves the research-flagged pain with the Libraries browser: users
//! don't want to navigate `Controls > SignalBlocks > Gain` every time
//! they need a block — they want to type
//! "resis", see results, and drop one onto the canvas. Figma / UE
//! asset-browser style.
//!
//! **What's in the palette**: every *leaf* component from
//! [`lunco_modelica_index::visual_diagram::library_class_library`] — interfaces and
//! package nodes are excluded. One row per component. Click a row to
//! instantiate on the active Diagram tab (placement cycles through a
//! 3-column grid to avoid overlap). Results stay browseable when no writable
//! Modelica document is active, but insertion is disabled with guidance.
//!
//! **Search**: case-insensitive substring match against the component's
//! display name, full source library path, category, and description. Library
//! and package filters come from the installed index, so any admitted Modelica
//! roots can be browsed. Top-100 matches rendered; typing narrows quickly.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

use lunco_modelica_index::visual_diagram::library_class_library;

/// Panel id — registered as a singleton panel, slotted RightInspector.
pub const PALETTE_PANEL_ID: PanelId = PanelId("modelica_component_palette");

/// Drag-and-drop payload for palette → canvas. Set by the palette
/// when a row's drag begins; consumed by the canvas drop handler on
/// pointer release. Cleared on miss (release outside any canvas) so
/// stale payloads don't leak between gestures.
#[derive(Resource, Default)]
pub struct ComponentDragPayload {
    pub def: Option<lunco_modelica_index::index::ClassEntry>,
}

/// Clear the palette drag payload after a canvas drop/miss.
#[derive(Event, Clone, Copy, Default)]
pub(crate) struct ClearComponentDragPayload;

pub(crate) fn on_clear_component_drag_payload(
    _trigger: On<ClearComponentDragPayload>,
    mut payload: ResMut<ComponentDragPayload>,
) {
    payload.def = None;
}

/// Request a component insertion from the palette. The observer owns the
/// document/op pipeline; the panel only supplies the selected class and the
/// optional placement coordinates.
#[derive(Event)]
pub(crate) struct PlaceComponentRequested {
    pub(crate) def: lunco_modelica_index::index::ClassEntry,
    pub(crate) target_doc: Option<lunco_doc::DocumentId>,
    pub(crate) placement: Option<(f32, f32)>,
}

pub(crate) fn on_place_component_requested(
    trigger: On<PlaceComponentRequested>,
    mut commands: Commands,
) {
    let def = trigger.def.clone();
    let target_doc = trigger.target_doc;
    let placement = trigger.placement;
    commands.queue(move |world: &mut World| {
        place_component(world, &def, target_doc, placement);
    });
}

/// Per-frame UI state for the palette. Holds the search query and
/// library/package filter selection.
///
/// Everything else (the component catalog) is static, owned by
/// `library_class_library()`; we just filter over its slice.
#[derive(Resource, Default)]
pub struct PaletteState {
    /// Current search query — normalized on compare (lowercase).
    pub query: String,
    /// Selected installed source root (`None` = all roots).
    pub library: Option<&'static str>,
    /// Selected immediate package within the selected root.
    pub category: Option<&'static str>,
}

/// Should this entry be shown in the instantiable palette?
///
/// Two formal exclusions, both grounded in the Modelica language
/// (not in source library folder-naming conventions):
///
/// - **Connectors** are ports, not standalone components — dragging
///   one onto a canvas is a category error.
/// - **`partial` classes** can't be instantiated directly per
///   MLS §4.4; they only exist to be inherited via `extends`. This
///   replaces the old `.Interfaces.` path heuristic.
///
/// `.Internal.` (library-private convention) is not filtered — it's
/// source library author lore with no formal language marker, and users with a
/// search box don't typically hit them by accident.
pub(crate) fn is_instantiable(c: &lunco_modelica_index::index::ClassEntry) -> bool {
    if c.partial {
        return false;
    }
    !matches!(
        c.kind,
        lunco_modelica_index::index::ClassKind::Connector
            | lunco_modelica_index::index::ClassKind::ExpandableConnector
    )
}

/// Memoized instantiable classes and their library/package counts. The
/// installed class index is immutable during a session, so these counts are
/// built once. Query-filtered counts are calculated only while searching.
struct PaletteCatalog {
    /// Instantiable classes (connectors/partials dropped), in library order.
    lib: Vec<&'static lunco_modelica_index::index::ClassEntry>,
    /// Installed source roots, sorted for stable filters.
    libraries: Vec<&'static str>,
    /// Per-library counts with no search query active.
    library_counts: std::collections::HashMap<&'static str, usize>,
    /// Immediate packages under each installed root.
    packages: std::collections::HashMap<&'static str, Vec<&'static str>>,
    /// Per-package counts, keyed by root and package.
    package_counts: std::collections::HashMap<(&'static str, &'static str), usize>,
    /// `lib.len()` — total instantiable component count.
    total: usize,
}

fn palette_catalog() -> &'static PaletteCatalog {
    static CACHE: std::sync::OnceLock<PaletteCatalog> = std::sync::OnceLock::new();
    static EMPTY: std::sync::OnceLock<PaletteCatalog> = std::sync::OnceLock::new();
    let empty = || {
        EMPTY.get_or_init(|| PaletteCatalog {
            lib: Vec::new(),
            libraries: Vec::new(),
            library_counts: std::collections::HashMap::new(),
            packages: std::collections::HashMap::new(),
            package_counts: std::collections::HashMap::new(),
            total: 0,
        })
    };
    if let Some(c) = CACHE.get() {
        return c;
    }
    let lib_all = library_class_library();
    if lib_all.is_empty() {
        // source library not loaded yet — return the shared empty catalog and retry
        // next call (mirrors `library_class_library`'s own load retry).
        return empty();
    }
    let lib: Vec<&'static lunco_modelica_index::index::ClassEntry> =
        lib_all.iter().filter(|c| is_instantiable(c)).collect();
    let mut library_counts: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    let mut packages: std::collections::HashMap<&'static str, Vec<&'static str>> =
        std::collections::HashMap::new();
    let mut package_counts: std::collections::HashMap<(&'static str, &'static str), usize> =
        std::collections::HashMap::new();
    for c in &lib {
        let library = library_of(c.name.as_str());
        let package = category_of(c.name.as_str());
        *library_counts.entry(library).or_insert(0) += 1;
        *package_counts.entry((library, package)).or_insert(0) += 1;
        packages.entry(library).or_default().push(package);
    }
    let mut libraries = library_counts.keys().copied().collect::<Vec<_>>();
    libraries.sort_unstable();
    for package_names in packages.values_mut() {
        package_names.sort_unstable();
        package_names.dedup();
    }
    let total = lib.len();
    let _ = CACHE.set(PaletteCatalog {
        lib,
        libraries,
        library_counts,
        packages,
        package_counts,
        total,
    });
    CACHE.get().unwrap_or_else(|| empty())
}

/// Top-level installed source root for a fully-qualified class.
fn library_of(qualified_path: &str) -> &str {
    rumoca_core::qualified_name_segments(qualified_path)
        .next()
        .unwrap_or(qualified_path)
}

/// Immediate package below the installed root for a fully-qualified class.
fn category_of(qualified_path: &str) -> &str {
    let Some((parent_path, _)) = qualified_path.rsplit_once('.') else {
        return "General";
    };
    rumoca_core::qualified_name_segments(parent_path)
        .nth(1)
        .unwrap_or("General")
}

/// The panel. Zero-sized; state lives in [`PaletteState`].
pub struct ComponentPalettePanel;

impl Panel for ComponentPalettePanel {
    fn id(&self) -> PanelId {
        PALETTE_PANEL_ID
    }

    fn title(&self) -> String {
        "Components".into()
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Lunica
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn closable(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Snapshot the theme at the top so every chip / row pulls
        // its colour from the same source. Category tints map to
        // schematic-wire tokens; secondary text uses `text_subdued`.
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let muted_text = theme.tokens.text_subdued;
        let active_target = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.active_document)
            .and_then(|document| {
                let documents = ctx.resource::<crate::ui::document_context::ModelicaDocuments>()?;
                let host = documents.host(document)?;
                let origin = host.document().origin();
                Some(origin.is_writable() || origin.is_untitled())
            });
        let can_insert = active_target == Some(true);

        // Snapshot the query and selected library/package filters.
        let state = ctx.resource::<PaletteState>();
        let query = state.map(|s| s.query.clone()).unwrap_or_default();
        let query_lc = query.to_lowercase();
        let selected_library = state.and_then(|s| s.library);
        let selected_category = state.and_then(|s| s.category);

        // Render the search box; capture any edit.
        let mut new_query = query.clone();
        let mut new_library = selected_library;
        let mut new_category = selected_category;
        let mut clear_all = false;

        ui.horizontal(|ui| {
            ui.label("Search");
            let response = ui.add(
                lunco_workbench_widgets::text_editor::singleline(&mut new_query)
                    .hint_text("Search components…")
                    .desired_width(f32::INFINITY),
            );
            // Escape clears the query.
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                new_query.clear();
            }
        });

        // Pre-filter partial classes and connectors before counting or display.
        // The static counts come from the memoized index; active searches only
        // recount matching classes for the visible chips.
        let catalog = palette_catalog();
        let lib = &catalog.lib;
        let pre_filter_total = catalog.total;
        let mut library_counts = catalog.library_counts.clone();
        let mut package_counts = catalog.package_counts.clone();
        let mut query_matches = pre_filter_total;
        if !query_lc.is_empty() {
            library_counts.clear();
            package_counts.clear();
            query_matches = 0;
            for c in lib.iter() {
                if score_component(c, &query_lc) <= 0.0 {
                    continue;
                }
                query_matches += 1;
                let library = library_of(c.name.as_str());
                let category = category_of(c.name.as_str());
                *library_counts.entry(library).or_insert(0) += 1;
                *package_counts.entry((library, category)).or_insert(0) += 1;
            }
        }

        // Installed roots are the first filter level. A dropdown keeps every
        // root reachable in the narrow sidebar without relying on hidden
        // horizontal scrolling.
        let previous_library = new_library;
        let library_label = new_library.map_or_else(
            || format!("All libraries ({query_matches})"),
            |library| {
                format!(
                    "{library} ({})",
                    library_counts.get(library).copied().unwrap_or(0)
                )
            },
        );
        egui::ComboBox::from_id_salt("palette_library_filter")
            .selected_text(library_label)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut new_library,
                    None,
                    format!("All libraries ({query_matches})"),
                );
                for &library in &catalog.libraries {
                    let count = library_counts.get(library).copied().unwrap_or(0);
                    ui.selectable_value(
                        &mut new_library,
                        Some(library),
                        format!("{library} ({count})"),
                    );
                }
            });
        if new_library != previous_library {
            new_category = None;
        }

        // Package filters are scoped to the selected root. This preserves
        // useful domain navigation for large libraries without hiding other
        // installed roots behind an "Other" bucket.
        if let Some(library) = new_library {
            let all_count = library_counts.get(library).copied().unwrap_or(0);
            let package_label = new_category.map_or_else(
                || format!("All packages ({all_count})"),
                |category| {
                    format!(
                        "{category} ({})",
                        package_counts
                            .get(&(library, category))
                            .copied()
                            .unwrap_or(0)
                    )
                },
            );
            egui::ComboBox::from_id_salt(("palette_package_filter", library))
                .selected_text(package_label)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut new_category,
                        None,
                        format!("All packages ({all_count})"),
                    );
                    if let Some(packages) = catalog.packages.get(library) {
                        for &category in packages {
                            let count = package_counts
                                .get(&(library, category))
                                .copied()
                                .unwrap_or(0);
                            ui.selectable_value(
                                &mut new_category,
                                Some(category),
                                format!("{category} ({count})"),
                            );
                        }
                    }
                });
        }

        // Header row: summary + clear-filter.
        ui.horizontal(|ui| {
            let visible = if let (Some(library), Some(category)) = (new_library, new_category) {
                package_counts
                    .get(&(library, category))
                    .copied()
                    .unwrap_or(0)
            } else if let Some(library) = new_library {
                library_counts.get(library).copied().unwrap_or(0)
            } else {
                query_matches
            };
            ui.label(
                egui::RichText::new(
                    if query_lc.is_empty() && new_library.is_none() && new_category.is_none() {
                        format!("{} components", pre_filter_total)
                    } else {
                        format!("{} of {} matching", visible, pre_filter_total)
                    },
                )
                .text_style(lunco_theme::TypographyRole::Caption.text_style())
                .color(muted_text),
            );
            if (!query_lc.is_empty() || new_library.is_some() || new_category.is_some())
                && lunco_workbench_widgets::icon_text_button(
                    ui,
                    lunco_workbench_widgets::UiIcon::Close,
                    "Clear",
                    "Clear the palette filter",
                )
                .clicked()
            {
                clear_all = true;
            }
        });
        if !can_insert {
            let message = if active_target.is_some() {
                "The active Modelica source is read-only. Duplicate it to add components."
            } else {
                "Open or create a writable Modelica model to add components."
            };
            let color = if active_target.is_some() {
                theme.tokens.warning
            } else {
                theme.tokens.text_subdued
            };
            ui.add(egui::Label::new(egui::RichText::new(message).color(color)).wrap());
        }
        ui.separator();

        // Write back state changes.
        if clear_all {
            ctx.resource_scope::<PaletteState, _>(|_, state| {
                state.query.clear();
                state.library = None;
                state.category = None;
            });
            // Skip subsequent rendering with stale query.
            return;
        }
        if new_query != query
            || new_library != selected_library
            || new_category != selected_category
        {
            ctx.resource_scope::<PaletteState, _>(|_, state| {
                state.query = new_query.clone();
                state.library = new_library;
                state.category = new_category;
            });
        }
        let query_lc = new_query.to_lowercase();
        let selected_library = new_library;
        let selected_category = new_category;

        // ── Filter + rank ──
        // Score higher for:
        //   +10 exact name match
        //   +5 name starts with query
        //   +3 name contains query
        //   +1.5 path contains query
        //   +1 category contains query
        //   +0.5 description contains query
        // Library and package selections act as hard gates.
        let mut scored: Vec<(&lunco_modelica_index::index::ClassEntry, f32)> = lib
            .iter()
            .filter_map(|c| {
                if let Some(library) = selected_library {
                    if library_of(c.name.as_str()) != library {
                        return None;
                    }
                }
                if let Some(cat) = selected_category {
                    if category_of(c.name.as_str()) != cat {
                        return None;
                    }
                }
                let score = if query_lc.is_empty() {
                    0.0
                } else {
                    score_component(c, &query_lc)
                };
                if query_lc.is_empty() || score > 0.0 {
                    Some((*c, score))
                } else {
                    None
                }
            })
            .collect();
        // Sort: higher score first, then by name for stable ordering.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.short_name().cmp(b.0.short_name()))
        });

        let shown_cap = 100;
        let shown = scored.len().min(shown_cap);

        // ── Result list ──
        // Searches and package-specific filters use a flat ranked list.
        // Otherwise, group by installed root and immediate package. This keeps
        // large roots from opening as one unscannable flat list.
        let mut clicked: Option<lunco_modelica_index::index::ClassEntry> = None;
        let mut drag_started_def: Option<lunco_modelica_index::index::ClassEntry> = None;
        // Snapshot the currently-dragged def name so render_component_row
        // can dim the source row.
        let dragging_path: Option<String> = ctx
            .resource::<ComponentDragPayload>()
            .and_then(|p| p.def.as_ref().map(|d| d.name.clone()));
        let is_searching = !query_lc.is_empty() || selected_category.is_some();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if lib.is_empty() {
                    if lunco_modelica_index::visual_diagram::library_index_available() {
                        ui.label(
                            egui::RichText::new(
                                "The indexed libraries contain no instantiable components.",
                            )
                            .color(muted_text),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(
                                "No Modelica library index is available. Install or configure libraries in Settings → Data & libraries or Settings → Modelica.",
                            )
                            .color(muted_text),
                        );
                    }
                } else if scored.is_empty() {
                    ui.label(
                        egui::RichText::new("No components match the current search and filters.")
                            .color(muted_text),
                    );
                } else if is_searching {
                    // Flat list (top-100 matches, score-ordered).
                    for (comp, _score) in scored.iter().take(shown) {
                        let being_dragged = dragging_path
                            .as_deref()
                            .map(|p| p == comp.name.as_str())
                            .unwrap_or(false);
                        let action = ui
                            .add_enabled_ui(can_insert, |ui| {
                                render_component_row(ui, comp, &theme, being_dragged)
                            })
                            .inner;
                        if action.clicked {
                            clicked = Some((*comp).clone());
                        }
                        if action.drag_started {
                            drag_started_def = Some((*comp).clone());
                        }
                    }
                    if scored.len() > shown_cap {
                        ui.label(
                            egui::RichText::new(format!(
                                "+ {} more — refine the search",
                                scored.len() - shown_cap
                            ))
                            .italics()
                            .color(muted_text),
                        );
                    }
                } else {
                    // Group by root and immediate package. Include the root
                    // in group labels until the user has selected one.
                    let mut groups: std::collections::BTreeMap<
                        (&'static str, &'static str),
                        Vec<&lunco_modelica_index::index::ClassEntry>,
                    > = std::collections::BTreeMap::new();
                    for (comp, _score) in scored.iter() {
                        let key = (
                            library_of(comp.name.as_str()),
                            category_of(comp.name.as_str()),
                        );
                        groups.entry(key).or_default().push(*comp);
                    }
                    for ((library, category), list) in groups {
                        let title = if selected_library.is_some() {
                            category.to_owned()
                        } else {
                            format!("{library} · {category}")
                        };
                        let header = egui::CollapsingHeader::new(
                            egui::RichText::new(format!("{} ({})", title, list.len()))
                                .color(theme.tokens.text_subdued)
                                .strong(),
                        )
                        .default_open(false)
                        .id_salt(("palette_group", library, category));
                        header.show(ui, |ui| {
                            for comp in list {
                                let being_dragged = dragging_path
                                    .as_deref()
                                    .map(|p| p == comp.name.as_str())
                                    .unwrap_or(false);
                                let action = ui
                                    .add_enabled_ui(can_insert, |ui| {
                                        render_component_row(ui, comp, &theme, being_dragged)
                                    })
                                    .inner;
                                if action.clicked {
                                    clicked = Some((*comp).clone());
                                }
                                if action.drag_started {
                                    drag_started_def = Some((*comp).clone());
                                }
                            }
                        });
                    }
                }
            });

        // ── Side-effect: instantiate clicked component ──
        //
        // Fires `AddModelicaComponent` against the active doc + its
        // drilled-in / detected class. The Reflect observer in
        // `lunco_modelica_api::edit` does the actual AST-level insertion via
        // `ModelicaOp::AddComponent`, so the path is identical to what
        // an external API caller would trigger (per AGENTS.md §4.1).
        //
        // Placement: simple modulo grid in Modelica diagram coords
        // (-100..100). Cycles `(placement_counter % 3, /3)` so
        // successive clicks don't all land on top of each other. The
        // canvas's auto-arrange button lets users tidy after.
        if let Some(def) = clicked {
            ctx.trigger(PlaceComponentRequested {
                def,
                target_doc: None,
                placement: None,
            });
        }

        // Stash drag payload for the canvas drop handler. Overwrites
        // any previous payload — only one drag can be active at a
        // time. The canvas clears it on drop (hit or miss).
        if let Some(def) = drag_started_def {
            ctx.resource_scope::<ComponentDragPayload, _>(|_, payload| {
                payload.def = Some(def);
            });
        }
    }
}

/// Persistent counter for palette-driven placement so successive
/// clicks step across a 3×N grid instead of stacking.
#[derive(Resource, Default)]
struct PalettePlacementCounter(u32);

/// Shared insertion path. `target_doc = None` resolves to the active
/// document (palette click). `placement = None` uses the persistent
/// 3×N grid counter; `Some((mx, my))` drops at explicit Modelica
/// coords (drag-and-drop drop point). Both call sites converge here
/// so naming + class resolution + the actual `AddModelicaComponent`
/// trigger live in one place.
pub(crate) fn place_component(
    world: &mut World,
    def: &lunco_modelica_index::index::ClassEntry,
    target_doc: Option<lunco_doc::DocumentId>,
    placement: Option<(f32, f32)>,
) {
    // Resolve target doc — explicit override (drop site) wins,
    // otherwise fall back to the active editor tab. No active doc →
    // no class to add into; we silently no-op.
    let active_doc = target_doc.or_else(|| {
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
    });
    let Some(doc_id) = active_doc else {
        bevy::log::info!(
            "[Palette] insert of `{}` ignored — no active document",
            def.name
        );
        return;
    };
    // Note: no pre-check for read-only here. The document layer
    // (`ModelicaDocument::apply`) rejects ops on read-only origins
    // and `apply_ops` surfaces a one-shot banner. Pre-checking in
    // panels would duplicate the policy and inevitably drift.

    // Resolve target class — MUST match the class the canvas is actually
    // projecting, or the component lands in a different class than the one
    // on screen (added comps "don't appear", and the diagram's placement /
    // move ops then target the wrong class → "component `x` not found in
    // class `y`"). The projection resolves its target via
    // `default_simulation_class` (drilled pin → run-target override →
    // tier-ranked sim candidate); route through the SAME helper so add and
    // view never disagree. First-non-package is only a last resort for docs
    // with no simulatable candidate (where the canvas shows nothing anyway).
    let class = crate::sim_default::default_simulation_class(world, doc_id)
        .or_else(|| {
            // Fallback to the document's first non-package class, read
            // via the per-doc Index (sees optimistic structural patches
            // and avoids walking the AST every palette click).
            let registry = world.resource::<crate::ui::document_context::ModelicaDocuments>();
            let host = registry.host(doc_id)?;
            host.document()
                .index()
                .classes
                .values()
                .find(|c| !matches!(c.kind, lunco_modelica_index::index::ClassKind::Package))
                .map(|c| c.name.clone())
        })
        .unwrap_or_default();
    if class.is_empty() {
        bevy::log::info!(
            "[Palette] click on `{}` ignored — could not resolve target class on doc {}",
            def.name,
            doc_id.raw()
        );
        return;
    }

    // Increment grid counter (always — also used as the name suffix
    // so dropped components get distinct names too) and resolve
    // placement: explicit drop-site wins, else step the grid.
    let counter_val = {
        let mut counter =
            world.get_resource_or_insert_with::<PalettePlacementCounter>(Default::default);
        counter.0 = counter.0.saturating_add(1);
        counter.0
    };
    let (x, y) = match placement {
        Some(xy) => xy,
        None => {
            let n = counter_val;
            (
                -50.0 + ((n % 3) as f32) * 30.0,
                50.0 - ((n / 3) as f32) * 30.0,
            )
        }
    };

    // Synthesise a unique-ish instance name. Modelica allows letters,
    // digits, underscore — start lower-case. The user can rename via
    // the inspector after placement.
    let short = def.name.split('.').next_back().unwrap_or(&def.name);
    let mut base = String::with_capacity(short.len());
    for (i, ch) in short.chars().enumerate() {
        if i == 0 {
            base.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            base.push(ch);
        }
    }
    if base.is_empty() {
        base.push_str("inst");
    }
    let name = format!("{base}{counter_val}");

    world
        .commands()
        .trigger(lunco_modelica_api::edit::component::AddModelicaComponent {
            doc_id,
            class,
            type_name: def.name.clone(),
            name,
            x,
            y,
            width: 20.0,
            height: 20.0,
            // Local mouse-drag from the palette: zero means
            // "use default" inside the observer, which animates.
            // Drag-add is a *Local* origin in our model; default
            // animation gives the user feedback on what landed
            // even though they initiated the action.
            animation_ms: 0,
        });
}

/// Per-row interaction outcome from [`render_component_row`].
#[derive(Default, Clone, Copy)]
struct RowAction {
    /// User completed a click (press + release without drag motion).
    /// Triggers grid-placement insertion via [`place_component`].
    clicked: bool,
    /// User just started dragging this row this frame. The caller
    /// stashes a [`ComponentDragPayload`] so the canvas can preview
    /// + drop.
    drag_started: bool,
}

/// Draw one component row (category dot + name + subtitle). Reacts
/// to both click (immediate grid-placement) and drag (stash payload
/// for canvas drop). Used by both the flat-search list and the
/// grouped-by-category list.
fn render_component_row(
    ui: &mut egui::Ui,
    comp: &lunco_modelica_index::index::ClassEntry,
    theme: &lunco_theme::Theme,
    is_being_dragged: bool,
) -> RowAction {
    let cat_color = theme.schematic.wire_unknown;
    let muted = theme.tokens.text_subdued;

    // Dim the row while it's the active drag source so the user has
    // a clear "this is the thing I'm dragging" signal alongside the
    // canvas ghost preview.
    if is_being_dragged {
        ui.set_opacity(0.4);
    }

    let resp = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, cat_color);

            ui.vertical(|ui| {
                // `selectable(false)` is critical: by default egui
                // Labels react to click+drag with text-selection,
                // which steals the row's drag gesture and the user
                // ends up highlighting characters instead of starting
                // a palette → canvas drag.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(comp.short_name())
                            .text_style(lunco_theme::TypographyRole::Label.text_style()),
                    )
                    .selectable(false),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&comp.category)
                            .text_style(lunco_theme::TypographyRole::Caption.text_style())
                            .color(muted),
                    )
                    .selectable(false),
                );
            });
        })
        .response
        .interact(egui::Sense::click_and_drag());

    if is_being_dragged {
        ui.set_opacity(1.0);
    }

    let tooltip = if comp.description.is_empty() {
        comp.name.as_str()
    } else {
        comp.description.as_str()
    };
    let resp = resp.on_hover_text(format!(
        "{}\n\n{}\n\nClick to add at grid · drag onto canvas to place at cursor.",
        comp.name, tooltip
    ));

    RowAction {
        clicked: resp.clicked(),
        drag_started: resp.drag_started(),
    }
}

/// Score a component against a lowercased query. Higher = better.
/// Returns 0 for no match.
fn score_component(c: &lunco_modelica_index::index::ClassEntry, query_lc: &str) -> f32 {
    let name_lc = c.short_name().to_lowercase();
    let path_lc = c.name.to_lowercase();
    let cat_lc = c.category.to_lowercase();
    let desc_lc = c.description.to_lowercase();

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
