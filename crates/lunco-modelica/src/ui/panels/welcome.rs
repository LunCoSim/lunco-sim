//! Welcome tab — the app's landing page.
//!
//! Shown in the center dock at startup and any time the user has no
//! model tabs open. Four stacked sections:
//!
//! 1. **Hero + Get Started** — headline, New/Open buttons.
//! 2. **Start here** — three hand-picked beginner examples (bundled
//!    files). First-click experience; intentionally curated so the
//!    first tutorial is predictable even as MSL evolves.
//! 3. **Browse examples** — search box + domain chips + 2-column
//!    card grid auto-populated from `msl_index.json`. Scales to the
//!    ~700 example classes shipped with MSL without per-card
//!    curation work. Cards show `short_description` (falling back to
//!    the first paragraph of `documentation_info` extracted by the
//!    indexer).
//! 4. **Shortcuts footer**.
//!
//! Every example — bundled or MSL — opens as a **read-only tab**.
//! The canvas read-only guard (`canvas_diagram::apply_ops`) writes
//! an explanatory message to `WorkbenchState.compilation_error` the
//! first time the user attempts an edit, pointing them to
//! Duplicate-to-Workspace. Same mental model across both sections:
//! look first, copy to edit.
//!
//! The panel is non-closable so the dock layout always has a center
//! anchor — even with no tabs open, the user has somewhere to land.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::models::BUNDLED_MODELS;
use crate::ui::state::ModelLibrary;
use crate::visual_diagram::{msl_component_library, MSLComponentDef};

/// Panel id.
pub const WELCOME_PANEL_ID: PanelId = PanelId("modelica_welcome");

/// Per-panel state for Welcome: the search string + the currently
/// selected domain chip. Stashed in egui's per-id state map so the
/// panel's immediate-mode render closure can read/write without a
/// Bevy resource. Keyed by a constant id below — only one Welcome
/// exists at a time (non-closable singleton).
#[derive(Clone, Default)]
struct BrowseState {
    query: String,
    /// Empty string = "All" (show every domain).
    domain: String,
}

const BROWSE_STATE_ID: &str = "modelica_welcome_browse_state";

/// Icon per MSL domain. Kept as a free function (not a const map)
/// so the match is cheap and obvious in diffs when we add a new
/// domain. Unknown domains fall back to 📦.
fn domain_icon(domain: &str) -> &'static str {
    match domain {
        "Electrical" => "⚡",
        "Mechanics" => "🔧",
        "Fluid" => "💧",
        "Thermal" => "🔥",
        "Magnetic" => "🧲",
        "Blocks" => "🎛",
        "ComplexBlocks" => "🎛",
        "Math" => "🧮",
        "StateGraph" => "🔀",
        "Clocked" => "⏱",
        "Media" => "🧪",
        "Utilities" => "🛠",
        _ => "📦",
    }
}

/// True when `c` is a *top-level* MSL example — its parent package
/// segment is exactly `Examples`. Filters out internal utilities
/// like `Modelica.Fluid.Examples.AST_BatchPlant.BaseClasses.InnerTank`
/// which carry the `is_example` flag (path contains `.Examples.`)
/// but aren't runnable tutorials. We only want the cards that open
/// into working simulations.
fn is_top_level_example(c: &MSLComponentDef) -> bool {
    if !c.is_example {
        return false;
    }
    // `msl_path` like `Modelica.Electrical.Analog.Examples.Rectifier`
    // — split off the leaf and check the immediate parent is
    // `Examples`.
    let mut parts = c.msl_path.rsplit('.');
    let _leaf = parts.next();
    matches!(parts.next(), Some("Examples"))
}

/// Pick the best human-readable line for a card. Prefers the short
/// `"…"` description on the class; falls back to the first sentence
/// of the extracted Documentation info; returns an empty string
/// when neither is available (card still renders, just without a
/// subtitle — rare for top-level examples).
fn card_subtitle(c: &MSLComponentDef) -> String {
    if let Some(s) = c.short_description.as_ref() {
        if !s.is_empty() {
            return s.clone();
        }
    }
    if let Some(info) = c.documentation_info.as_ref() {
        // First-sentence cut. `clean_info_text` already kept the
        // first paragraph; further trimming to one sentence keeps
        // cards visually uniform.
        if let Some(end) = info.find(". ") {
            return format!("{}.", &info[..end]);
        }
        return info.clone();
    }
    String::new()
}

/// The welcome placeholder panel. Zero-sized.
pub struct WelcomePanel;

impl Panel for WelcomePanel {
    fn id(&self) -> PanelId {
        WELCOME_PANEL_ID
    }

    fn title(&self) -> String {
        "🏠 Welcome".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mut create_new = false;
        let mut open_folder = false;
        let mut open_bundled: Option<&'static str> = None;
        let mut open_msl: Option<String> = None;

        // Theme snapshot once per frame.
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let card_fill = theme.colors.surface0;
        let card_stroke = theme.colors.surface2;
        let chip_fill_active = theme.tokens.accent;
        let chip_fill_idle = theme.colors.surface1;
        let chip_text_active = theme.colors.base;
        let chip_text_idle = theme.colors.text;
        let title_tint = theme.tokens.accent;
        let muted = theme.tokens.text_subdued;

        // Load & cache the browse state from egui's data bag.
        let state_id = egui::Id::new(BROWSE_STATE_ID);
        let mut browse: BrowseState = ui
            .ctx()
            .data_mut(|d| d.get_temp::<BrowseState>(state_id).unwrap_or_default());

        // Pre-compute per-frame derived data:
        //   * All top-level MSL examples (filtered from the full
        //     component library).
        //   * Counts per domain (for chip labels).
        //   * Filtered list for the card grid under the current
        //     search + domain selection.
        // Cheap enough to redo each frame at ~700 entries; skipping
        // memoisation keeps the code readable and avoids stale-cache
        // bugs when the user types.
        let lib = msl_component_library();
        let examples: Vec<&MSLComponentDef> =
            lib.iter().filter(|c| is_top_level_example(c)).collect();

        // Domain → count. Sorted for a stable chip order.
        let mut domain_counts: Vec<(String, usize)> = {
            let mut map: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for c in &examples {
                *map.entry(c.domain.clone()).or_default() += 1;
            }
            map.into_iter().collect()
        };
        domain_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let query_lc = browse.query.to_lowercase();
        let filtered: Vec<&MSLComponentDef> = examples
            .iter()
            .copied()
            .filter(|c| {
                (browse.domain.is_empty() || c.domain == browse.domain)
                    && (query_lc.is_empty()
                        || c.name.to_lowercase().contains(&query_lc)
                        || c.msl_path.to_lowercase().contains(&query_lc)
                        || c.short_description
                            .as_deref()
                            .is_some_and(|s| s.to_lowercase().contains(&query_lc))
                        || c.documentation_info
                            .as_deref()
                            .is_some_and(|s| s.to_lowercase().contains(&query_lc)))
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(32.0);

            // ── Headline ───────────────────────────────────
            ui.vertical_centered(|ui| {
                ui.heading(
                    egui::RichText::new("LunCoSim Modelica Workbench")
                        .size(24.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Build physics models, simulate them, see the numbers.",
                    )
                    .size(13.0)
                    .color(muted),
                );
            });

            ui.add_space(24.0);

            // ── Get Started ───────────────────────────────
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.horizontal(|ui| {
                    let new_btn = ui.add_sized(
                        [272.0, 44.0],
                        egui::Button::new(
                            egui::RichText::new("➕  New Model").size(14.0).strong(),
                        ),
                    );
                    if new_btn
                        .on_hover_text("Create a new untitled model (Ctrl+N)")
                        .clicked()
                    {
                        create_new = true;
                    }
                    let open_btn = ui.add_sized(
                        [272.0, 44.0],
                        egui::Button::new(
                            egui::RichText::new("📁  Open Folder").size(14.0).strong(),
                        ),
                    );
                    if open_btn
                        .on_hover_text("Pick a folder of .mo files to browse")
                        .clicked()
                    {
                        open_folder = true;
                    }
                });
            });

            ui.add_space(32.0);

            // ── Start Here (bundled beginner examples) ────
            ui.vertical_centered(|ui| {
                ui.set_max_width(720.0);
                ui.heading(egui::RichText::new("Start here").size(16.0));
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Three small models to run, read, and break. \
                         Opens read-only — duplicate to edit.",
                    )
                    .size(11.0)
                    .color(muted),
                );
                ui.add_space(10.0);

                for model in BUNDLED_MODELS {
                    let display = model
                        .filename
                        .strip_suffix(".mo")
                        .unwrap_or(model.filename);

                    let resp = ui
                        .add_sized(
                            [720.0, 54.0],
                            egui::Button::new("")
                                .fill(card_fill)
                                .stroke(egui::Stroke::new(1.0, card_stroke)),
                        )
                        .on_hover_text(format!(
                            "Open {} as a read-only tab",
                            display
                        ));
                    let rect = resp.rect;
                    let painter = ui.painter_at(rect);
                    painter.text(
                        rect.min + egui::vec2(16.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        format!("📄  {}", display),
                        egui::FontId::proportional(14.0),
                        title_tint,
                    );
                    painter.text(
                        rect.min + egui::vec2(16.0, 32.0),
                        egui::Align2::LEFT_TOP,
                        model.tagline,
                        egui::FontId::proportional(11.0),
                        muted,
                    );
                    if resp.clicked() {
                        open_bundled = Some(model.filename);
                    }
                    ui.add_space(6.0);
                }
            });

            ui.add_space(32.0);

            // ── Browse examples (auto-populated from MSL) ─
            ui.vertical_centered(|ui| {
                ui.set_max_width(720.0);
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new("Browse examples").size(16.0));
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{} models", examples.len()))
                            .size(11.0)
                            .color(muted),
                    );
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Pulled straight from the Modelica Standard Library. \
                         Filter by domain, search anywhere in the name or description.",
                    )
                    .size(11.0)
                    .color(muted),
                );
                ui.add_space(10.0);

                // Search box — focus is opt-in (user must click); we
                // don't auto-focus so keyboard shortcuts like Ctrl+N
                // still land on the panel.
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    let resp = ui.add_sized(
                        [640.0, 28.0],
                        egui::TextEdit::singleline(&mut browse.query)
                            .hint_text("search name, path or description…"),
                    );
                    if !browse.query.is_empty()
                        && ui.button("✕").on_hover_text("Clear search").clicked()
                    {
                        browse.query.clear();
                    }
                    let _ = resp;
                });

                ui.add_space(8.0);

                // Domain chip row. `[All]` sits first; individual
                // domain chips show the count in parentheses so the
                // user can see where the mass of examples lives.
                ui.horizontal_wrapped(|ui| {
                    let chip =
                        |ui: &mut egui::Ui,
                         label: String,
                         active: bool|
                         -> egui::Response {
                            let (fill, fg) = if active {
                                (chip_fill_active, chip_text_active)
                            } else {
                                (chip_fill_idle, chip_text_idle)
                            };
                            ui.add(
                                egui::Button::new(
                                    egui::RichText::new(label)
                                        .size(11.5)
                                        .color(fg),
                                )
                                .fill(fill)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    chip_fill_idle,
                                )),
                            )
                        };

                    if chip(
                        ui,
                        format!("All ({})", examples.len()),
                        browse.domain.is_empty(),
                    )
                    .clicked()
                    {
                        browse.domain.clear();
                    }
                    for (domain, count) in &domain_counts {
                        let label = format!(
                            "{} {} ({})",
                            domain_icon(domain),
                            domain,
                            count
                        );
                        if chip(ui, label, browse.domain == *domain).clicked() {
                            browse.domain = domain.clone();
                        }
                    }
                });

                ui.add_space(10.0);

                // Card grid — 2 columns, flex row height. Each card
                // shows icon + short name (title) + one-line
                // subtitle. Full qualified path is in the hover
                // tooltip. Click → read-only open via OpenClass.
                if filtered.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("No examples match this filter.")
                            .color(muted),
                    );
                } else {
                    let col_w = 352.0;
                    let row_h = 66.0;
                    let mut iter = filtered.iter();
                    loop {
                        let left = iter.next();
                        let right = iter.next();
                        if left.is_none() {
                            break;
                        }
                        ui.horizontal(|ui| {
                            for entry in [left, right].into_iter().flatten() {
                                let c = *entry;
                                let resp = ui
                                    .add_sized(
                                        [col_w, row_h],
                                        egui::Button::new("")
                                            .fill(card_fill)
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                card_stroke,
                                            )),
                                    )
                                    .on_hover_text(format!(
                                        "{}\n\nOpens read-only — duplicate to edit.",
                                        c.msl_path
                                    ));
                                let rect = resp.rect;
                                let painter = ui.painter_at(rect);
                                painter.text(
                                    rect.min + egui::vec2(14.0, 8.0),
                                    egui::Align2::LEFT_TOP,
                                    format!(
                                        "{}  {}",
                                        domain_icon(&c.domain),
                                        c.name
                                    ),
                                    egui::FontId::proportional(13.5),
                                    title_tint,
                                );
                                // Subtitle. Truncate hard at ~64
                                // chars so two cards stay visually
                                // aligned even for rambling docs.
                                let sub = card_subtitle(c);
                                let sub = if sub.chars().count() > 72 {
                                    let mut s: String =
                                        sub.chars().take(72).collect();
                                    s.push('…');
                                    s
                                } else {
                                    sub
                                };
                                painter.text(
                                    rect.min + egui::vec2(14.0, 28.0),
                                    egui::Align2::LEFT_TOP,
                                    sub,
                                    egui::FontId::proportional(10.5),
                                    muted,
                                );
                                painter.text(
                                    rect.min
                                        + egui::vec2(14.0, row_h - 18.0),
                                    egui::Align2::LEFT_TOP,
                                    &c.domain,
                                    egui::FontId::proportional(9.5),
                                    muted,
                                );
                                if resp.clicked() {
                                    open_msl = Some(c.msl_path.clone());
                                }
                            }
                        });
                        ui.add_space(8.0);
                    }
                }
            });

            ui.add_space(32.0);

            // ── Shortcuts footer ──────────────────────────
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Ctrl+N  new    ·    Ctrl+S  save    ·    \
                         Ctrl+Z / Ctrl+Shift+Z  undo/redo    ·    \
                         F5  compile",
                    )
                    .size(10.0)
                    .color(egui::Color32::DARK_GRAY),
                );
            });

            ui.add_space(32.0);
        });

        // Persist browse state across frames.
        ui.ctx().data_mut(|d| d.insert_temp(state_id, browse));

        // ── Side effects (after the render closure) ──────────
        if create_new {
            world
                .commands()
                .trigger(crate::ui::commands::CreateNewScratchModel);
        }
        if open_folder {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Open workspace folder")
                .pick_folder()
            {
                use bevy::tasks::AsyncComputeTaskPool;
                let pool = AsyncComputeTaskPool::get();
                let task = pool.spawn({
                    let folder = folder.clone();
                    async move {
                        crate::ui::panels::package_browser::scan_twin_folder(folder)
                    }
                });
                {
                    let mut cache = world.resource_mut::<
                        crate::ui::panels::package_browser::PackageTreeCache,
                    >();
                    cache.twin = None;
                    cache.twin_scan_task = Some(task);
                }
                match lunco_twin::TwinMode::open(&folder) {
                    Ok(lunco_twin::TwinMode::Folder(twin))
                    | Ok(lunco_twin::TwinMode::Twin(twin)) => {
                        let twin_id = world
                            .resource_mut::<lunco_workbench::WorkspaceResource>()
                            .add_twin(twin);
                        world
                            .commands()
                            .trigger(lunco_workbench::TwinAdded { twin: twin_id });
                    }
                    Ok(lunco_twin::TwinMode::Orphan(_)) => {}
                    Err(e) => {
                        log::warn!(
                            "open folder: failed to index {:?}: {}",
                            folder,
                            e
                        );
                    }
                }
            }
        }
        if let Some(filename) = open_bundled {
            let id = format!("bundled://{}", filename);
            let name =
                filename.strip_suffix(".mo").unwrap_or(filename).to_string();
            crate::ui::panels::package_browser::open_model(
                world,
                id,
                name,
                ModelLibrary::Bundled,
            );
        }
        if let Some(qualified) = open_msl {
            // Read-only-first policy: MSL examples open via
            // `OpenClass` (drill-in path) so users explore first and
            // duplicate on demand. The canvas read-only guard surfaces
            // a Diagnostics message on the first edit attempt.
            world
                .commands()
                .trigger(crate::ui::commands::OpenClass { qualified });
        }
    }
}
