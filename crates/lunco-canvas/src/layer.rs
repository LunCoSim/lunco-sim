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
use lunco_theme::ColorAlpha;

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
        let theme = lunco_theme::active(ctx.ui.ctx());
        let grid_color = theme.colors.overlay0.alpha(60);
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
        let col = grid_color;
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
        // Pre-pass: count edge incidences per (node, port) endpoint
        // so we know which ports host a junction (≥3 wires meet
        // there) — Dymola/OMEdit draw a small filled circle at
        // those points to disambiguate "two crossing wires that
        // are connected" from "two wires that just happen to
        // visually overlap". 2-port connections (the common case)
        // never get a dot.
        // TODO(CQ-202): this endpoint-incidence map is rebuilt from a full
        // edge scan on EVERY frame, though it only changes when the wiring
        // topology does. Cache it on the layer and recompute only when a
        // Scene topology generation bumps (add/remove/reconnect edge) — needs
        // a `topology_gen: u64` counter on `Scene` bumped by its edge
        // mutators. Deferred: requires a Scene API change + in-app verify on
        // a large diagram. See docs/code-quality-remediation.md (CQ-202).
        let mut endpoint_counts: std::collections::HashMap<
            (crate::scene::NodeId, crate::scene::PortId),
            u32,
        > = std::collections::HashMap::new();
        for (_eid, edge) in scene.edges() {
            *endpoint_counts
                .entry((edge.from.node, edge.from.port.clone()))
                .or_insert(0) += 1;
            *endpoint_counts
                .entry((edge.to.node, edge.to.port.clone()))
                .or_insert(0) += 1;
        }
        for (eid, edge) in scene.edges() {
            let Some(from_node) = scene.node(edge.from.node) else { continue };
            let Some(to_node) = scene.node(edge.to.node) else { continue };
            // Port lookup: when the edge endpoint references a port
            // that doesn't exist on the node (typical for top-level
            // connector instances — they ARE the connector and have
            // no sub-ports), fall back to the node's centre. Skipping
            // the edge entirely (the previous behaviour) made every
            // wire from `u_s`/`u_m`/`u_ff`/`y` invisible because
            // those nodes carry no port that matches the connector's
            // own name. Anchoring on the centre lets the orthogonal
            // router still draw a stub-then-waypoint polyline.
            let from_port = from_node.ports.iter().find(|p| p.id == edge.from.port);
            let to_port = to_node.ports.iter().find(|p| p.id == edge.to.port);
            let from_w = if let Some(p) = from_port {
                crate::scene::Pos::new(
                    from_node.rect.min.x + p.local_offset.x,
                    from_node.rect.min.y + p.local_offset.y,
                )
            } else {
                crate::scene::Pos::new(
                    (from_node.rect.min.x + from_node.rect.max.x) * 0.5,
                    (from_node.rect.min.y + from_node.rect.max.y) * 0.5,
                )
            };
            let to_w = if let Some(p) = to_port {
                crate::scene::Pos::new(
                    to_node.rect.min.x + p.local_offset.x,
                    to_node.rect.min.y + p.local_offset.y,
                )
            } else {
                crate::scene::Pos::new(
                    (to_node.rect.min.x + to_node.rect.max.x) * 0.5,
                    (to_node.rect.min.y + to_node.rect.max.y) * 0.5,
                )
            };
            // Snap endpoints to integer pixels. Without this, the
            // floating-point world→screen transform produces sub-
            // pixel endpoint coordinates that egui anti-aliases at
            // different sub-pixel offsets per zoom step, making the
            // wire visibly jitter / shift its connection point as
            // the user pans or zooms. The icon's port marker (also
            // drawn at world-to-screen of the same world coord)
            // shifts in lockstep, so snapping both keeps them
            // aligned to the same pixel grid.
            let from_s_raw = ctx.viewport.world_to_screen(from_w, sr);
            let to_s_raw = ctx.viewport.world_to_screen(to_w, sr);
            let from_s = crate::scene::Pos::new(from_s_raw.x.round(), from_s_raw.y.round());
            let to_s = crate::scene::Pos::new(to_s_raw.x.round(), to_s_raw.y.round());
            let visual = self
                .registry_handle
                .build_edge(edge.kind.as_str(), &edge.data)
                .unwrap_or_else(|| Box::new(crate::visual::PlaceholderEdgeVisual));
            let selected =
                selection.contains(crate::selection::SelectItem::Edge(*eid));
            // Project the live waypoints (mid-drag this is what the
            // tool just mutated) into screen space, with the same
            // pixel-snap as the endpoints so the polyline stays
            // aligned across zoom levels.
            let waypoints_screen: Vec<crate::scene::Pos> = edge
                .waypoints
                .iter()
                .map(|w| {
                    let s = ctx.viewport.world_to_screen(*w, sr);
                    crate::scene::Pos::new(s.x.round(), s.y.round())
                })
                .collect();
            visual.draw(ctx, from_s, to_s, &waypoints_screen, selected);
        }
        // Post-pass: junction dots. A small filled circle at any
        // port that hosts ≥3 incident wires. Drawn on top of the
        // edges (last in this layer) but still under the nodes
        // layer, so the icon body covers it where they overlap.
        let theme = lunco_theme::active(ctx.ui.ctx());
        let dot_color = theme.tokens.text;
        for ((node_id, port_id), count) in &endpoint_counts {
            if *count < 3 {
                continue;
            }
            let Some(node) = scene.node(*node_id) else { continue };
            let Some(port) = node.ports.iter().find(|p| p.id == *port_id) else { continue };
            let world = crate::scene::Pos::new(
                node.rect.min.x + port.local_offset.x,
                node.rect.min.y + port.local_offset.y,
            );
            let p = ctx.viewport.world_to_screen(world, sr);
            let center = egui::pos2(p.x.round(), p.y.round());
            // Radius scaled with zoom so the dot stays visible at
            // wide-zoom and doesn't dominate when zoomed in.
            let r = (3.0 * ctx.viewport.zoom.clamp(0.5, 2.0)).max(2.5);
            ctx.ui
                .painter()
                .circle_filled(center, r, dot_color);
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
        // TODO(CQ-202): `build_node` allocates a fresh `Box<dyn NodeVisual>`
        // (re-parsing `node.data`) for every node every frame. Cache the
        // built visual keyed by (kind, data identity), rebuilding only on
        // edit. Blocked on a key: `Scene::Node.data` is an inline
        // `serde_json::Value`, so there's no stable `Arc` pointer to key on
        // yet — make `NodeData` Arc-backed first, then the cache is an
        // O(1) ptr-keyed lookup. Same applies to `build_edge` above.
        // See docs/code-quality-remediation.md (CQ-202).
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
        let theme = lunco_theme::active(ctx.ui.ctx());
        let painter = ctx.ui.painter();
        let outline = theme.tokens.accent;
        for item in selection.iter() {
            if let crate::selection::SelectItem::Node(nid) = *item {
                if let Some(node) = scene.node(nid) {
                    let halo_rect = node.visual_rect.unwrap_or(node.rect);
                    let r = ctx.viewport.world_rect_to_screen(halo_rect, sr);
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

/// Renders the active tool's in-flight preview — ghost edge from a
/// port during drag-to-connect, rubber-band rect during band-select,
/// any future drop-target glow. Reads from `DrawCtx.extras`, which
/// the canvas populates with `Option<ToolPreview>`.
#[derive(Default)]
pub struct ToolPreviewLayer;

impl Layer for ToolPreviewLayer {
    fn draw(&mut self, ctx: &mut DrawCtx, _scene: &Scene, _selection: &Selection) {
        let Some(preview_opt) = ctx.extras.downcast_ref::<Option<crate::tool::ToolPreview>>()
        else {
            return;
        };
        let Some(preview) = preview_opt else { return };
        let theme = lunco_theme::active(ctx.ui.ctx());
        let ghost_edge = theme.tokens.accent;
        let snap_target_ring = theme.tokens.success;
        let snap_guide = theme.tokens.warning.alpha(180);
        let rubber_band_fill = theme.tokens.accent.alpha(40);
        let rubber_band_stroke = theme.tokens.accent;
        let painter = ctx.ui.painter();
        let sr = ctx.screen_rect;
        match preview {
            crate::tool::ToolPreview::GhostEdge {
                from_world,
                to_world,
                snap_target,
            } => {
                let a = ctx.viewport.world_to_screen(*from_world, sr);
                let b = ctx.viewport.world_to_screen(*to_world, sr);
                painter.line_segment(
                    [egui::pos2(a.x, a.y), egui::pos2(b.x, b.y)],
                    egui::Stroke::new(2.0, ghost_edge),
                );
                painter.circle_filled(
                    egui::pos2(a.x, a.y),
                    4.0,
                    ghost_edge,
                );
                if let Some(t) = snap_target {
                    let s = ctx.viewport.world_to_screen(*t, sr);
                    painter.circle_stroke(
                        egui::pos2(s.x, s.y),
                        8.0,
                        egui::Stroke::new(2.0, snap_target_ring),
                    );
                }
            }
            crate::tool::ToolPreview::GhostEdgeWithBends {
                from_world,
                bends,
                to_world,
                snap_target,
            } => {
                // Build the screen-space control polyline:
                // [from] → user-placed bends → [to]. Then auto-insert
                // an L-bend on the trailing segment so the cursor-end
                // of the preview matches the orthogonal routing the
                // final wire will use. With no user bends, this gives
                // the classic L: horizontal-then-vertical (or the
                // reverse, choosing whichever has the larger initial
                // delta).
                let mut pts: Vec<egui::Pos2> =
                    Vec::with_capacity(3 + bends.len());
                let a = ctx.viewport.world_to_screen(*from_world, sr);
                pts.push(egui::pos2(a.x, a.y));
                for b in bends {
                    let s = ctx.viewport.world_to_screen(*b, sr);
                    pts.push(egui::pos2(s.x, s.y));
                }
                let b = ctx.viewport.world_to_screen(*to_world, sr);
                // Auto-L between the last placed point and the
                // pointer.
                let last = *pts.last().unwrap();
                let dx = (b.x - last.x).abs();
                let dy = (b.y - last.y).abs();
                if dx > 0.5 && dy > 0.5 {
                    // Choose pivot orientation so the first leg of
                    // the L runs along the dominant axis — feels
                    // natural and matches Dymola's "first segment
                    // follows the port stub axis" convention.
                    let pivot = if dx >= dy {
                        egui::pos2(b.x, last.y)
                    } else {
                        egui::pos2(last.x, b.y)
                    };
                    pts.push(pivot);
                }
                pts.push(egui::pos2(b.x, b.y));
                let stroke = egui::Stroke::new(2.0, ghost_edge);
                for w in pts.windows(2) {
                    painter.line_segment([w[0], w[1]], stroke);
                }
                painter.circle_filled(pts[0], 4.0, ghost_edge);
                for p in &pts[1..pts.len().saturating_sub(1)] {
                    painter.circle_filled(*p, 3.0, ghost_edge);
                }
                if let Some(t) = snap_target {
                    let s = ctx.viewport.world_to_screen(*t, sr);
                    painter.circle_stroke(
                        egui::pos2(s.x, s.y),
                        8.0,
                        egui::Stroke::new(2.0, snap_target_ring),
                    );
                }
            }
            crate::tool::ToolPreview::SnapGuides { x, y } => {
                // Thin dashed lines through the snapped coordinate
                // span the visible viewport. World-space x/y; clip to
                // screen bounds via the viewport transform.
                let screen_min = ctx.viewport.screen_to_world(
                    crate::scene::Pos::new(sr.min.x, sr.min.y),
                    sr,
                );
                let screen_max = ctx.viewport.screen_to_world(
                    crate::scene::Pos::new(sr.max.x, sr.max.y),
                    sr,
                );
                let stroke = egui::Stroke::new(1.0, snap_guide);
                if let Some(gx) = x {
                    let top = ctx.viewport.world_to_screen(
                        crate::scene::Pos::new(*gx, screen_min.y),
                        sr,
                    );
                    let bot = ctx.viewport.world_to_screen(
                        crate::scene::Pos::new(*gx, screen_max.y),
                        sr,
                    );
                    painter.line_segment(
                        [egui::pos2(top.x, top.y), egui::pos2(bot.x, bot.y)],
                        stroke,
                    );
                }
                if let Some(gy) = y {
                    let left = ctx.viewport.world_to_screen(
                        crate::scene::Pos::new(screen_min.x, *gy),
                        sr,
                    );
                    let right = ctx.viewport.world_to_screen(
                        crate::scene::Pos::new(screen_max.x, *gy),
                        sr,
                    );
                    painter.line_segment(
                        [
                            egui::pos2(left.x, left.y),
                            egui::pos2(right.x, right.y),
                        ],
                        stroke,
                    );
                }
            }
            crate::tool::ToolPreview::RubberBand(r) => {
                let sr_rect = ctx.viewport.world_rect_to_screen(*r, sr);
                let rect = egui::Rect::from_min_max(
                    egui::pos2(sr_rect.min.x, sr_rect.min.y),
                    egui::pos2(sr_rect.max.x, sr_rect.max.y),
                );
                painter.rect_filled(rect, 2.0, rubber_band_fill);
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, rubber_band_stroke),
                    egui::StrokeKind::Outside,
                );
            }
        }
    }
    fn name(&self) -> &'static str {
        "tool_preview"
    }
}
