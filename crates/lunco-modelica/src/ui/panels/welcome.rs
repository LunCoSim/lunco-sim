//! Welcome tab — the app's landing page.
//!
//! Shown in the center dock at startup and any time the user has no
//! model tabs open. Two roles:
//!
//! 1. **Getting started** — the discoverable paths into the app
//!    (New Model, Open Folder).
//! 2. **Learn by example** — the bundled examples used to live in
//!    the sidebar, which confused "my work" with "sample material".
//!    They live here instead, with one-line taglines explaining
//!    what each teaches.
//!
//! The panel is non-closable so the dock layout always has a center
//! anchor — even with no tabs open, the user has somewhere to land.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::models::BUNDLED_MODELS;
use crate::ui::state::ModelLibrary;

/// Panel id.
pub const WELCOME_PANEL_ID: PanelId = PanelId("modelica_welcome");

/// Curated MSL example classes offered on the welcome page.
///
/// Chosen across domains (control, electrical, mechanical, fluid,
/// thermal, multi-body) so a new user can pick the one that matches
/// their discipline. Each entry's `qualified` is the fully-scoped
/// MSL name the duplicate pipeline needs; `tagline` is a single
/// sentence explaining what the model demonstrates.
///
/// Clicking an entry dispatches
/// [`crate::ui::commands::OpenExampleInWorkspace`], which does the
/// full async build (resolve MSL path → read → extract the target
/// class → rename + strip `within` → parse on bg thread → open in
/// Canvas view). The MSL original is never modified.
struct MslExample {
    qualified: &'static str,
    short: &'static str,
    tagline: &'static str,
}

const MSL_EXAMPLES: &[MslExample] = &[
    MslExample {
        qualified: "Modelica.Blocks.Examples.PID_Controller",
        short: "PID_Controller",
        tagline: "Control: PID feedback loop tracking a setpoint — classic blocks wiring.",
    },
    MslExample {
        qualified: "Modelica.Blocks.Examples.FilterWithRiseTime",
        short: "FilterWithRiseTime",
        tagline: "Control: continuous filter with rise-time spec — step response.",
    },
    MslExample {
        qualified: "Modelica.Electrical.Analog.Examples.CauerLowPassAnalog",
        short: "CauerLowPassAnalog",
        tagline: "Electrical: 5th-order Cauer low-pass with R/L/C network.",
    },
    MslExample {
        qualified: "Modelica.Electrical.Analog.Examples.Rectifier",
        short: "Rectifier",
        tagline: "Electrical: full-wave rectifier driving an RC load.",
    },
    MslExample {
        qualified: "Modelica.Mechanics.Rotational.Examples.First",
        short: "Rotational.First",
        tagline: "Mechanics: torque, inertia, spring-damper — the rotational starter.",
    },
    MslExample {
        qualified: "Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum",
        short: "DoublePendulum",
        tagline: "MultiBody: chaotic 2-link pendulum — 3D visualisation + simulation.",
    },
    MslExample {
        qualified: "Modelica.Thermal.HeatTransfer.Examples.TwoMasses",
        short: "TwoMasses",
        tagline: "Thermal: two masses exchanging heat through a conductor.",
    },
    MslExample {
        qualified: "Modelica.Fluid.Examples.BranchingDynamicPipes",
        short: "BranchingDynamicPipes",
        tagline: "Fluid: branching pipe network with dynamic momentum balance.",
    },
];

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
        // Scroll area so narrow/short windows still let users reach
        // the examples list.
        let mut create_new = false;
        let mut open_folder = false;
        let mut open_example: Option<&'static str> = None;
        let mut open_msl_example: Option<&'static str> = None;

        // Snapshot the theme once per frame so every example button
        // pulls its colours from the same source. Cloning `Theme` is
        // cheap (few hundred bytes).
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        // Semantic tokens used repeatedly below. Surface pair for
        // "card on panel" reads cleanly in both dark and light modes;
        // `accent` for the interactive-card title tint; `text_subdued`
        // replaces the generic `Color32::GRAY` that was almost
        // invisible on Latte.
        //
        // Bundled and MSL example rows share the same accent — the
        // distinction is the *heading* above each group, not a
        // per-row colour. If a future design needs two accents the
        // right move is to add `accent_secondary` to
        // `DesignTokens`, not to pick a palette entry here.
        let button_fill = theme.colors.surface0;
        let button_stroke = theme.colors.surface2;
        let msl_button_fill = theme.colors.surface1;
        let msl_button_stroke = theme.colors.overlay0;
        let title_tint = theme.tokens.accent;
        let muted = theme.tokens.text_subdued;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(40.0);

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

            ui.add_space(32.0);

            // ── Getting Started ────────────────────────────
            // Two big buttons: New Model, Open Folder.
            ui.vertical_centered(|ui| {
                ui.set_max_width(520.0);
                ui.heading(egui::RichText::new("Get started").size(16.0));
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let new_btn = ui.add_sized(
                        [240.0, 48.0],
                        egui::Button::new(
                            egui::RichText::new("➕  New Model")
                                .size(14.0)
                                .strong(),
                        ),
                    );
                    if new_btn
                        .on_hover_text("Create a new untitled model (Ctrl+N)")
                        .clicked()
                    {
                        create_new = true;
                    }

                    let open_btn = ui.add_sized(
                        [240.0, 48.0],
                        egui::Button::new(
                            egui::RichText::new("📁  Open Folder")
                                .size(14.0)
                                .strong(),
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

            ui.add_space(40.0);

            // ── Learn by example ───────────────────────────
            // Each bundled model renders as a selectable row with
            // name + tagline. Click opens it as a read-only tab.
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.heading(egui::RichText::new("Learn by example").size(16.0));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Open any example in a read-only tab — simulate, \
                         read the source, copy what you need.",
                    )
                    .size(10.5)
                    .color(muted),
                );
                ui.add_space(10.0);

                for model in BUNDLED_MODELS {
                    let display = model
                        .filename
                        .strip_suffix(".mo")
                        .unwrap_or(model.filename);

                    // One row per example: left-aligned title + grey
                    // tagline underneath, full width, selectable.
                    let resp = ui
                        .add_sized(
                            [560.0, 48.0],
                            egui::Button::new("")
                                .fill(button_fill)
                                .stroke(egui::Stroke::new(1.0, button_stroke)),
                        )
                        .on_hover_text(format!("Open {} as a read-only tab", display));
                    let rect = resp.rect;

                    // Paint the label + tagline manually inside the
                    // button rect so alignment/sizing is consistent
                    // regardless of tagline length.
                    let painter = ui.painter_at(rect);
                    let title_pos = rect.min + egui::vec2(16.0, 8.0);
                    let tagline_pos = rect.min + egui::vec2(16.0, 28.0);
                    painter.text(
                        title_pos,
                        egui::Align2::LEFT_TOP,
                        format!("📄  {}", display),
                        egui::FontId::proportional(13.5),
                        title_tint,
                    );
                    painter.text(
                        tagline_pos,
                        egui::Align2::LEFT_TOP,
                        model.tagline,
                        egui::FontId::proportional(10.5),
                        muted,
                    );

                    if resp.clicked() {
                        open_example = Some(model.filename);
                    }
                    ui.add_space(4.0);
                }
            });

            ui.add_space(40.0);

            // ── MSL Examples ────────────────────────────────
            // Curated examples from the Modelica Standard Library.
            // Clicking any of them creates a fresh editable copy
            // (the MSL originals stay read-only) and drops the
            // user on the Canvas view so they can see the diagram
            // first. Contrast with the bundled examples above,
            // which are small standalone files shipped in-repo.
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.heading(egui::RichText::new("MSL Examples").size(16.0));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Pick an example from the Modelica Standard Library. \
                         One click → editable copy opens in the diagram view, \
                         the MSL original stays untouched.",
                    )
                    .size(10.5)
                    .color(muted),
                );
                ui.add_space(10.0);

                for ex in MSL_EXAMPLES {
                    let resp = ui
                        .add_sized(
                            [560.0, 48.0],
                            egui::Button::new("")
                                .fill(msl_button_fill)
                                .stroke(egui::Stroke::new(1.0, msl_button_stroke)),
                        )
                        .on_hover_text(format!(
                            "Open editable copy of {}",
                            ex.qualified
                        ));
                    let rect = resp.rect;
                    let painter = ui.painter_at(rect);
                    let title_pos = rect.min + egui::vec2(16.0, 8.0);
                    let tagline_pos = rect.min + egui::vec2(16.0, 28.0);
                    painter.text(
                        title_pos,
                        egui::Align2::LEFT_TOP,
                        format!("🧩  {}", ex.short),
                        egui::FontId::proportional(13.5),
                        title_tint,
                    );
                    painter.text(
                        tagline_pos,
                        egui::Align2::LEFT_TOP,
                        ex.tagline,
                        egui::FontId::proportional(10.5),
                        muted,
                    );
                    if resp.clicked() {
                        open_msl_example = Some(ex.qualified);
                    }
                    ui.add_space(4.0);
                }
            });

            ui.add_space(40.0);

            // ── Keyboard shortcuts footer ──────────────────
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Ctrl+N  new    ·    Ctrl+S  save    ·    \
                         Ctrl+Z / Ctrl+Shift+Z  undo/redo    ·    F5  compile",
                    )
                    .size(10.0)
                    .color(egui::Color32::DARK_GRAY),
                );
            });

            ui.add_space(40.0);
        });

        // Side effects after the render closure.
        if create_new {
            world
                .commands()
                .trigger(crate::ui::commands::CreateNewScratchModel);
        }
        if open_folder {
            // Same synchronous picker the sidebar uses. Scan is async
            // so a huge folder doesn't freeze us.
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
                // Also feed the new Twin Browser. `TwinMode::open` is
                // synchronous but only walks the file tree (no parse),
                // so it's cheap even on large folders. Anything that
                // produces a valid Folder/Twin gets stashed; failures
                // (deleted between picker and now, etc.) leave the
                // browser empty rather than crashing.
                match lunco_twin::TwinMode::open(&folder) {
                    Ok(lunco_twin::TwinMode::Folder(twin))
                    | Ok(lunco_twin::TwinMode::Twin(twin)) => {
                        world
                            .resource_mut::<lunco_workbench::OpenTwin>()
                            .0 = Some(twin);
                    }
                    Ok(lunco_twin::TwinMode::Orphan(_)) => {
                        // User picked a file via the folder dialog
                        // (shouldn't happen with `pick_folder`, but be
                        // defensive). Don't replace OpenTwin.
                    }
                    Err(e) => {
                        log::warn!("OpenTwin: failed to index {:?}: {}", folder, e);
                    }
                }
            }
        }
        if let Some(filename) = open_example {
            let id = format!("bundled://{}", filename);
            let name = filename.strip_suffix(".mo").unwrap_or(filename).to_string();
            crate::ui::panels::package_browser::open_model(
                world,
                id,
                name,
                ModelLibrary::Bundled,
            );
        }
        if let Some(qualified) = open_msl_example {
            world
                .commands()
                .trigger(crate::ui::commands::OpenExampleInWorkspace {
                    qualified: qualified.to_string(),
                });
        }
    }
}
