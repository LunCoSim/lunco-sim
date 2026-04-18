//! Render pipeline — ordered passes over the scene.
//!
//! A [`Layer`] is a render pass. The canvas composes them in order,
//! each one getting a [`crate::visual::DrawCtx`] plus read access to
//! the scene and selection. The default ordering (and what each
//! built-in layer does) is:
//!
//! 1. [`GridLayer`] — faint grid in world coordinates, snaps visually
//!    at every zoom level. Skipped when `show_grid = false`.
//! 2. [`EdgesLayer`] — dispatches to each edge's
//!    [`crate::visual::EdgeVisual::draw`].
//! 3. [`NodesLayer`] — dispatches to each node's
//!    [`crate::visual::NodeVisual::draw`].
//! 4. [`SelectionLayer`] — outlines for selected items, on top of
//!    the normal paint so node bodies don't cover their own highlight.
//! 5. [`ToolPreviewLayer`] — ghost edge while dragging from a port,
//!    rubber-band rect, drop-target highlight. Drawn last so tool
//!    feedback is never obscured.
//!
//! Apps can reorder, skip, or add — the canvas just walks a
//! `Vec<Box<dyn Layer>>`. A "minimap dim-others" mode, for example,
//! would insert a dim overlay between `NodesLayer` and
//! `SelectionLayer`.
//!
//! # Why layers instead of a monolithic `render()`
//!
//! - New decorations land as a new layer — zero churn on core code.
//! - Layer order is data, so apps pick a different pipeline for
//!   different modes (e.g. "focus" mode dims everything except the
//!   selection).
//! - Unit-testable in isolation — the grid layer doesn't need a
//!   scene, just a viewport.

use bevy_egui::egui;

use crate::scene::Scene;
use crate::selection::Selection;
use crate::visual::{DrawCtx, VisualRegistry};

/// One render pass. `draw` is called once per frame, in list order.
pub trait Layer: Send + Sync {
    fn draw(&mut self, ctx: &mut DrawCtx, scene: &Scene, selection: &Selection);

    /// Debug / toolbar name.
    fn name(&self) -> &'static str;
}

/// Faint dotted grid in world space. Spacing and colour configurable.
pub struct GridLayer {
    pub spacing_world: f32,
    pub color: egui::Color32,
    pub enabled: bool,
}

impl Default for GridLayer {
    fn default() -> Self {
        Self {
            spacing_world: 20.0,
            color: egui::Color32::from_rgba_premultiplied(60, 60, 72, 50),
            enabled: true,
        }
    }
}

impl Layer for GridLayer {
    fn draw(&mut self, ctx: &mut DrawCtx, _scene: &Scene, _selection: &Selection) {
        if !self.enabled {
            return;
        }
        let painter = ctx.ui.painter();
        let sr = ctx.screen_rect;
        let zoom = ctx.viewport.zoom.max(f32::EPSILON);

        // Adaptive grid: double or halve the base spacing until each
        // dot lands 8-64 screen pixels from its neighbour. Keeps the
        // grid visually consistent from deep zoom-in to zoom-out
        // without ever disappearing or turning into moire.
        let base = self.spacing_world.max(1.0);
        let mut step = base;
        // Too dense (<8 px) — double the world spacing so we draw
        // fewer, further-apart dots.
        while step * zoom < 8.0 {
            step *= 2.0;
            if step > base * 1024.0 {
                break; // safety bail — astronomical zoom-out
            }
        }
        // Too sparse (>64 px) — halve it.
        while step * zoom > 64.0 {
            step *= 0.5;
            if step < base / 1024.0 {
                break;
            }
        }

        let min_w = ctx
            .viewport
            .screen_to_world(crate::scene::Pos::new(sr.min.x, sr.min.y), sr);
        let max_w = ctx
            .viewport
            .screen_to_world(crate::scene::Pos::new(sr.max.x, sr.max.y), sr);
        let start_x = (min_w.x / step).floor() * step;
        let start_y = (min_w.y / step).floor() * step;
        let r = 1.2_f32;
        let col = self.color;
        let mut y = start_y;
        while y <= max_w.y {
            let mut x = start_x;
            while x <= max_w.x {
                let p = ctx
                    .viewport
                    .world_to_screen(crate::scene::Pos::new(x, y), sr);
                painter.circle_filled(egui::pos2(p.x, p.y), r, col);
                x += step;
            }
            y += step;
        }
    }
    fn name(&self) -> &'static str {
        "grid"
    }
}

/// Dispatches each edge through the visual registry. Edge endpoints
/// are computed from port local-offsets so wires meet the port
/// graphic, not the node corner.
pub struct EdgesLayer {
    registry_handle: std::sync::Arc<VisualRegistry>,
}

impl EdgesLayer {
    pub fn new(registry: std::sync::Arc<VisualRegistry>) -> Self {
        Self {
            registry_handle: registry,
        }
    }
}

impl Layer for EdgesLayer {
    fn draw(&mut self, ctx: &mut DrawCtx, scene: &Scene, selection: &Selection) {
        let sr = ctx.screen_rect;
        for (eid, edge) in scene.edges() {
            let Some(from_node) = scene.node(edge.from.node) else { continue };
            let Some(to_node) = scene.node(edge.to.node) else { continue };
            let Some(from_port) = from_node
                .ports
                .iter()
                .find(|p| p.id == edge.from.port)
            else {
                continue;
            };
            let Some(to_port) = to_node.ports.iter().find(|p| p.id == edge.to.port) else {
                continue;
            };
            let from_w = crate::scene::Pos::new(
                from_node.rect.min.x + from_port.local_offset.x,
                from_node.rect.min.y + from_port.local_offset.y,
            );
            let to_w = crate::scene::Pos::new(
                to_node.rect.min.x + to_port.local_offset.x,
                to_node.rect.min.y + to_port.local_offset.y,
            );
            let from_s = ctx.viewport.world_to_screen(from_w, sr);
            let to_s = ctx.viewport.world_to_screen(to_w, sr);
            let visual = self
                .registry_handle
                .build_edge(edge.kind.as_str(), &edge.data)
                .unwrap_or_else(|| Box::new(crate::visual::PlaceholderEdgeVisual));
            let selected =
                selection.contains(crate::selection::SelectItem::Edge(*eid));
            visual.draw(ctx, from_s, to_s, selected);
        }
    }
    fn name(&self) -> &'static str {
        "edges"
    }
}

/// Dispatches each node through the visual registry.
pub struct NodesLayer {
    registry_handle: std::sync::Arc<VisualRegistry>,
}

impl NodesLayer {
    pub fn new(registry: std::sync::Arc<VisualRegistry>) -> Self {
        Self {
            registry_handle: registry,
        }
    }
}

impl Layer for NodesLayer {
    fn draw(&mut self, ctx: &mut DrawCtx, scene: &Scene, selection: &Selection) {
        for (nid, node) in scene.nodes() {
            let visual = self
                .registry_handle
                .build_node(node.kind.as_str(), &node.data)
                .unwrap_or_else(|| Box::new(crate::visual::PlaceholderNodeVisual));
            let selected =
                selection.contains(crate::selection::SelectItem::Node(*nid));
            visual.draw(ctx, node, selected);
        }
    }
    fn name(&self) -> &'static str {
        "nodes"
    }
}

/// Selection halo — drawn after [`NodesLayer`] so it's never covered
/// by the node body. Blue outline plus a slight outer glow.
pub struct SelectionLayer;

impl Layer for SelectionLayer {
    fn draw(&mut self, ctx: &mut DrawCtx, scene: &Scene, selection: &Selection) {
        if selection.is_empty() {
            return;
        }
        let sr = ctx.screen_rect;
        let painter = ctx.ui.painter();
        let outline = egui::Color32::from_rgb(120, 170, 255);
        for item in selection.iter() {
            if let crate::selection::SelectItem::Node(nid) = *item {
                if let Some(node) = scene.node(nid) {
                    let r = ctx.viewport.world_rect_to_screen(node.rect, sr);
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(r.min.x - 2.0, r.min.y - 2.0),
                        egui::pos2(r.max.x + 2.0, r.max.y + 2.0),
                    );
                    painter.rect_stroke(
                        rect,
                        5.0,
                        egui::Stroke::new(1.5, outline),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
    }
    fn name(&self) -> &'static str {
        "selection"
    }
}

/// Ghost edge during port-drag, rubber-band rect, drop-target glow.
/// B1 ships with no state (draws nothing); the real preview state
/// lives on `DefaultTool` when B2 lands and this layer will read it
/// from `DrawCtx.extras`.
#[derive(Default)]
pub struct ToolPreviewLayer;

impl Layer for ToolPreviewLayer {
    fn draw(&mut self, _ctx: &mut DrawCtx, _scene: &Scene, _selection: &Selection) {}
    fn name(&self) -> &'static str {
        "tool_preview"
    }
}
