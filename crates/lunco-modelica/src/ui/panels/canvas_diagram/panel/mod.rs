//! `CanvasDiagramPanel` — top-level panel implementation.

pub(crate) mod util;
pub(crate) mod snapshots;
pub(crate) mod interaction;
pub(crate) mod projection_sync;
pub(crate) mod render;

use bevy_egui::egui;
use lunco_canvas::Scene;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::model_tabs_types::TabRenderContext;
use super::{CANVAS_DIAGRAM_PANEL_ID, CanvasDiagramState, active_doc_from_world_ctx};
use projection_sync::{trigger_projection_if_needed, poll_and_swap_projection};
use render::render_diagram_canvas;

pub struct CanvasDiagramPanel;

impl Panel for CanvasDiagramPanel {
    fn id(&self) -> PanelId { CANVAS_DIAGRAM_PANEL_ID }
    fn title(&self) -> String { "🧩 Canvas Diagram".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Center }
    /// Not listed: the canvas users actually work in is embedded in the
    /// per-document Model view tab. This singleton renders an empty scene when
    /// no document is active, so a menu entry for it opens a blank panel.
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Hidden
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Scope `CanvasDiagramState` out for the whole body so the
        // entire canvas subtree threads `&mut CanvasDiagramState`
        // (no raw `&mut World`). Reinserted after paint.
        let present = ctx.resource_scope::<CanvasDiagramState, _>(|ctx, state| {
            let render_tab_id = ctx.resource::<TabRenderContext>().and_then(|c| c.tab_id);
            let active_doc = active_doc_from_world_ctx(ctx);

            if active_doc.is_none() {
                state.get_mut(None).canvas.scene = Scene::new();
                self.render_canvas(ui, ctx, state);
                return;
            }

            trigger_projection_if_needed(ui, ctx, state, render_tab_id);
            poll_and_swap_projection(ui, ctx, state, render_tab_id);

            self.render_canvas(ui, ctx, state);
        });

        if present.is_none() {
            ctx.defer(|w| {
                w.init_resource::<CanvasDiagramState>();
            });
        }
    }
}

impl CanvasDiagramPanel {
    pub(crate) fn render_canvas(
        &self,
        ui: &mut egui::Ui,
        ctx: &mut PanelCtx,
        state: &mut CanvasDiagramState,
    ) {
        render_diagram_canvas(self, ui, ctx, state);
    }
}
