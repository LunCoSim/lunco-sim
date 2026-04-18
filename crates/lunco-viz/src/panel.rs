//! `VizPanel` — the generic workbench host for a visualization.
//!
//! One panel instance per [`VizId`]. The panel looks up its
//! [`VisualizationConfig`] in the
//! [`VisualizationRegistry`](crate::registry::VisualizationRegistry),
//! finds the matching [`Visualization`] impl in the
//! [`VizKindCatalog`](crate::registry::VizKindCatalog), and dispatches
//! to the right render path based on the config's [`ViewTarget`].
//!
//! Because `VizPanel` is an `InstancePanel`, the workbench can open,
//! close, split, and tab multiple plots the same way it handles model
//! tabs. Creating a new plot = inserting a `VisualizationConfig` +
//! firing `OpenTab`.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{InstancePanel, PanelId, PanelSlot};

use crate::registry::{VisualizationRegistry, VizKindCatalog};
use crate::view::{Panel2DCtx, ViewTarget};
use crate::viz::VizId;

/// Multi-instance kind id used by `lunco_workbench` to address
/// viz-panel instances. Same `kind` value across every viz instance;
/// the `instance` field is the [`VizId`].
pub const VIZ_PANEL_KIND: PanelId = PanelId("lunco_viz_panel");

/// Generic viz-hosting panel. One instance per [`VizId`].
#[derive(Default)]
pub struct VizPanel;

impl InstancePanel for VizPanel {
    fn kind(&self) -> PanelId { VIZ_PANEL_KIND }

    fn default_slot(&self) -> PanelSlot {
        // Time-series charts live in the bottom dock by default —
        // same as the old dedicated Graphs panel. Users can drag
        // them anywhere from there.
        PanelSlot::Bottom
    }

    fn title(&self, world: &World, instance: u64) -> String {
        let id = VizId(instance);
        world
            .get_resource::<VisualizationRegistry>()
            .and_then(|r| r.get(id))
            .map(|cfg| format!("📈 {}", cfg.title))
            .unwrap_or_else(|| format!("📈 Plot #{instance}"))
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64) {
        let id = VizId(instance);

        // Pull the needed data off the world eagerly so we don't
        // hold conflicting borrows while rendering.
        let (config, viz) = {
            let registry = match world.get_resource::<VisualizationRegistry>() {
                Some(r) => r,
                None => {
                    ui.label("VisualizationRegistry not installed.");
                    return;
                }
            };
            let Some(cfg) = registry.get(id).cloned() else {
                ui.label(format!("No visualization #{instance}."));
                return;
            };
            let catalog = match world.get_resource::<VizKindCatalog>() {
                Some(c) => c,
                None => {
                    ui.label("VizKindCatalog not installed.");
                    return;
                }
            };
            let Some(viz) = catalog.get(cfg.kind.clone()) else {
                ui.label(format!(
                    "Unknown viz kind '{}' (not registered).",
                    cfg.kind.as_str()
                ));
                return;
            };
            (cfg, viz)
        };

        match config.view {
            ViewTarget::Panel2D => {
                let mut ctx = Panel2DCtx { ui, world };
                viz.render_panel_2d(&mut ctx, &config);
            }
            ViewTarget::Viewport3D => {
                // Hosted by the primary 3D viewport, not by this
                // panel. The panel still gets rendered because the
                // workbench tab for a `Viewport3D` viz is the
                // inspector / control surface; the actual render
                // happens elsewhere. Left as a placeholder until the
                // 3D view lands.
                ui.label(
                    egui::RichText::new("3D viewport viz — controls coming.")
                        .color(egui::Color32::GRAY),
                );
            }
            ViewTarget::Panel3D => {
                ui.label(
                    egui::RichText::new("3D sub-panel — not implemented yet.")
                        .color(egui::Color32::GRAY),
                );
            }
        }
    }
}
