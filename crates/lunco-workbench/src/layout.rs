//! Frame-level layout rendering.
//!
//! Composes egui's top/bottom/left/right/central panels into the standard
//! workbench layout documented in `docs/architecture/11-workbench.md`.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{PanelId, StatusContent, WorkbenchLayout};

pub(crate) fn render(ctx: &egui::Context, layout: &mut WorkbenchLayout, world: &mut World) {
    // ── Menu bar (top) ──────────────────────────────────────────────────
    egui::TopBottomPanel::top("lunco_workbench_menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                ui.label("(File menu — todo)");
            });
            ui.menu_button("Edit", |ui| {
                ui.label("(Edit menu — todo)");
            });
            ui.menu_button("View", |ui| {
                if ui.button("Toggle Activity Bar").clicked() {
                    layout.toggle_activity_bar();
                    ui.close();
                }
                if ui.button("Toggle Bottom Dock").clicked() {
                    layout.toggle_bottom();
                    ui.close();
                }
            });
            ui.menu_button("Window", |ui| {
                ui.label("(Window menu — todo)");
            });
            ui.menu_button("Help", |ui| {
                ui.label("LunCoSim workbench v0.1");
            });

            // Right-aligned command-palette search stub.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled(
                    false,
                    egui::Button::new(egui::RichText::new("Ctrl+P  search anything").weak()),
                );
            });
        });
    });

    // ── Transport bar: workspace tabs on the left, transport (todo) right ─
    egui::TopBottomPanel::top("lunco_workbench_transport_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if layout.workspaces.is_empty() {
                ui.label(egui::RichText::new("(no workspaces registered)").weak());
            } else {
                // Build a list of (id, title, active?) up-front so we can
                // call `activate_workspace` without clashing with the
                // iterator's immutable borrow of `layout.workspaces`.
                let active = layout.active_workspace;
                let tabs: Vec<(crate::WorkspaceId, String, bool)> = layout
                    .workspaces
                    .iter()
                    .map(|w| {
                        let id = w.id();
                        (id, w.title(), active == Some(id))
                    })
                    .collect();
                for (id, title, is_active) in tabs {
                    let button = egui::Button::new(title).selected(is_active);
                    if ui.add(button).clicked() && !is_active {
                        layout.activate_workspace(id);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("(transport — todo)").weak());
            });
        });
    });

    // ── Status bar (bottom) ─────────────────────────────────────────────
    egui::TopBottomPanel::bottom("lunco_workbench_status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            match layout.status.as_ref() {
                Some(StatusContent::Text(s)) => {
                    ui.label(egui::RichText::new(s).small());
                }
                None => {
                    ui.label(egui::RichText::new("ready").small().weak());
                }
            }
        });
    });

    // ── Activity bar (far left) ─────────────────────────────────────────
    if layout.activity_bar {
        egui::SidePanel::left("lunco_workbench_activity_bar")
            .resizable(false)
            .exact_width(40.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    // Placeholder activity icons. Real icons + wired behaviour
                    // land with the first real panel migration.
                    for icon in ["📁", "🧩", "📦", "🔎", "⚙"] {
                        ui.label(icon);
                        ui.add_space(8.0);
                    }
                });
            });
    }

    // ── Side browser ────────────────────────────────────────────────────
    if let Some(id) = layout.side_browser {
        egui::SidePanel::left("lunco_workbench_side_browser")
            .default_width(220.0)
            .min_width(120.0)
            .show(ctx, |ui| {
                render_panel_header(ui, &id, layout);
                ui.separator();
                render_panel_body(ui, id, layout, world);
            });
    }

    // ── Right inspector ─────────────────────────────────────────────────
    if let Some(id) = layout.right_inspector {
        egui::SidePanel::right("lunco_workbench_right_inspector")
            .default_width(280.0)
            .min_width(160.0)
            .show(ctx, |ui| {
                render_panel_header(ui, &id, layout);
                ui.separator();
                render_panel_body(ui, id, layout, world);
            });
    }

    // ── Bottom dock ─────────────────────────────────────────────────────
    if layout.bottom_visible {
        if let Some(id) = layout.bottom {
            egui::TopBottomPanel::bottom("lunco_workbench_bottom_dock")
                .resizable(true)
                .default_height(180.0)
                .show(ctx, |ui| {
                    render_panel_header(ui, &id, layout);
                    ui.separator();
                    render_panel_body(ui, id, layout, world);
                });
        }
    }

    // ── Central viewport slot ───────────────────────────────────────────
    // Never dockable, never subdivided — the 3D world owns this region
    // (or, in headless contexts, whoever is currently rendering the
    // main canvas). v0.1 shows a placeholder so the layout reads right
    // in the example; the host app replaces this by drawing into the
    // primary egui context's CentralPanel themselves.
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.4);
            ui.label(
                egui::RichText::new("VIEWPORT")
                    .weak()
                    .size(20.0),
            );
            ui.label(
                egui::RichText::new(
                    "(host app draws here — 3D world, in rover_sandbox_usd; \n\
                     reserved central tab in modelica_workbench)",
                )
                .weak()
                .small(),
            );
        });
    });
}

fn render_panel_header(ui: &mut egui::Ui, id: &PanelId, layout: &mut WorkbenchLayout) {
    if let Some(panel) = layout.panels.get(id) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(panel.title()).strong());
        });
    } else {
        ui.label(
            egui::RichText::new(format!("<missing panel: {}>", id.as_str()))
                .color(egui::Color32::LIGHT_RED),
        );
    }
}

fn render_panel_body(
    ui: &mut egui::Ui,
    id: PanelId,
    layout: &mut WorkbenchLayout,
    world: &mut World,
) {
    // Temporarily move the panel out so its `render` method can take a
    // `&mut World` that (in the future) might read the layout itself.
    // The panel goes back into the layout when rendering finishes.
    if let Some(mut panel) = layout.panels.remove(&id) {
        panel.render(ui, world);
        layout.panels.insert(id, panel);
    }
}
