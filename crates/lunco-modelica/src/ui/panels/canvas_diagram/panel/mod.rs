//! `CanvasDiagramPanel` — top-level panel implementation.

pub(crate) mod util;
pub(crate) mod snapshots;
pub(crate) mod interaction;
pub(crate) mod projection_sync;
pub(crate) mod render;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::Scene;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::model_tabs_types::TabRenderContext;
use super::{CANVAS_DIAGRAM_PANEL_ID, CanvasDiagramState, active_doc_from_world};
use projection_sync::{trigger_projection_if_needed, poll_and_swap_projection};
use render::render_diagram_canvas;

pub(crate) use util::{invalidate_port_icon_cache};

pub struct CanvasDiagramPanel;

impl Panel for CanvasDiagramPanel {
    fn id(&self) -> PanelId { CANVAS_DIAGRAM_PANEL_ID }
    fn title(&self) -> String { "🧩 Canvas Diagram".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Center }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<CanvasDiagramState>().is_none() {
            world.insert_resource(CanvasDiagramState::default());
        }

        let render_tab_id = world.resource::<TabRenderContext>().tab_id;
        let active_doc = active_doc_from_world(world);

        if active_doc.is_none() {
            world.resource_mut::<CanvasDiagramState>().get_mut(None).canvas.scene = Scene::new();
            self.render_canvas(ui, world);
            return;
        }

        trigger_projection_if_needed(ui, world, render_tab_id);
        poll_and_swap_projection(ui, world, render_tab_id);

        self.render_canvas(ui, world);
    }
}

impl CanvasDiagramPanel {
    pub(crate) fn render_canvas(&self, ui: &mut egui::Ui, world: &mut World) {
        render_diagram_canvas(self, ui, world);
    }
}
