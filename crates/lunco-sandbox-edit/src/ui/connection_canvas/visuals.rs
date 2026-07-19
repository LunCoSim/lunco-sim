//! Node + edge visuals for the USD connection canvas.
//!
//! Deliberately minimal — a titled card with input dots on the left and output
//! dots on the right, and a straight coloured wire. No SVG icons, no animation:
//! this canvas is about topology (what's wired to what), not iconography.

use bevy_egui::egui;
use lunco_canvas::{DrawCtx, EdgeVisual, Node, NodeVisual, Pos};

use super::projection::{UsdPrimNodeData, UsdWireData, WireKind};

/// Card visual for a `"usd.prim"` node.
pub(crate) struct UsdPrimNodeVisual {
    pub type_name: String,
    pub is_body: bool,
}

impl NodeVisual for UsdPrimNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool) {
        let sr = ctx.viewport.world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(sr.min.x, sr.min.y),
            egui::pos2(sr.max.x, sr.max.y),
        );
        let painter = ctx.ui.painter().clone().with_clip_rect(ctx.ui.clip_rect());
        let theme = lunco_theme::active(ctx.ui.ctx());
        let t = &theme.tokens;

        let fill = if selected {
            t.node_card_selected
        } else if self.is_body {
            t.node_card_body
        } else {
            t.node_card
        };
        painter.rect_filled(rect, 6.0, fill);
        let stroke_col = if selected {
            t.node_border_selected
        } else {
            t.node_border
        };
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(if selected { 2.0 } else { 1.0 }, stroke_col),
            egui::StrokeKind::Outside,
        );

        // Titles — leaf name (bold-ish) over the type name. Hidden when the node
        // is too small on screen (zoomed out) to keep the canvas legible.
        if rect.height() > 22.0 {
            painter.text(
                egui::pos2(rect.center().x, rect.min.y + rect.height() * 0.38),
                egui::Align2::CENTER_CENTER,
                &node.label,
                egui::FontId::proportional((rect.height() * 0.22).clamp(9.0, 15.0)),
                t.text,
            );
            if !self.type_name.is_empty() && rect.height() > 40.0 {
                painter.text(
                    egui::pos2(rect.center().x, rect.min.y + rect.height() * 0.68),
                    egui::Align2::CENTER_CENTER,
                    &self.type_name,
                    egui::FontId::proportional((rect.height() * 0.16).clamp(8.0, 11.0)),
                    t.text_subdued,
                );
            }
        }

        // Ports — a coloured dot per connector. Joint anchors (`~`-prefixed)
        // carry the physics wires but aren't drawn (they'd clutter the card
        // centre and aren't user-connectable).
        let zoom = ctx.viewport.zoom;
        let r = (4.0 * zoom).clamp(2.5, 6.0);
        for port in &node.ports {
            if port.id.as_str().starts_with('~') {
                continue;
            }
            let world = Pos::new(
                node.rect.min.x + port.local_offset.x,
                node.rect.min.y + port.local_offset.y,
            );
            let p = ctx.viewport.world_to_screen(world, ctx.screen_rect);
            let col = match port.kind.as_str() {
                "input" => t.port_input,
                "output" => t.port_output,
                _ => t.node_border,
            };
            painter.circle_filled(egui::pos2(p.x, p.y), r, col);
            painter.circle_stroke(
                egui::pos2(p.x, p.y),
                r,
                egui::Stroke::new(1.0, t.port_outline),
            );
        }
    }

    fn debug_name(&self) -> &str {
        "usd.prim"
    }
}

/// Straight wire visual for a `"usd.wire"` edge, coloured by wire kind.
pub(crate) struct UsdWireVisual {
    pub kind: WireKind,
}

impl EdgeVisual for UsdWireVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from_screen: Pos,
        to_screen: Pos,
        _waypoints_screen: &[Pos],
        selected: bool,
    ) {
        let theme = lunco_theme::active(ctx.ui.ctx());
        // Wire-by-domain is exactly what `SchematicTokens` models for Modelica;
        // USD dataflow is a signal connection and a joint is a mechanical one,
        // so they read in the same colours as their schematic counterparts.
        let base = match self.kind {
            WireKind::Dataflow => theme.schematic.wire_signal,
            WireKind::Joint => theme.schematic.wire_mechanical,
        };
        let col = if selected {
            theme.tokens.node_border_selected
        } else {
            base
        };
        let width = if selected { 2.5 } else { 1.6 };
        let a = egui::pos2(from_screen.x, from_screen.y);
        let b = egui::pos2(to_screen.x, to_screen.y);
        let painter = ctx.ui.painter();
        painter.line_segment([a, b], egui::Stroke::new(width, col));

        // Arrowhead at the sink so signal direction reads at a glance.
        let dir = b - a;
        let len = dir.length();
        if len > 1.0 {
            let d = dir / len;
            let n = egui::vec2(-d.y, d.x);
            let tip = b - d * 8.0;
            let head = 5.0;
            painter.add(egui::Shape::convex_polygon(
                vec![b, tip + n * head, tip - n * head],
                col,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// Build the concrete node visual from the typed payload (registry factory).
pub(crate) fn node_visual(data: &UsdPrimNodeData) -> UsdPrimNodeVisual {
    UsdPrimNodeVisual {
        type_name: data.type_name.clone(),
        is_body: data.is_body,
    }
}

/// Build the concrete edge visual from the typed payload (registry factory).
pub(crate) fn edge_visual(data: &UsdWireData) -> UsdWireVisual {
    UsdWireVisual { kind: data.kind }
}
