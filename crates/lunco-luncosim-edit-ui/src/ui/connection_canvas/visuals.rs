//! Node + edge visuals for the USD connection canvas.
//!
//! Deliberately minimal — a titled card with input dots on the left and output
//! dots on the right, acausal connector diamonds, and an orthogonal coloured wire. No SVG icons, no animation:
//! this canvas is about topology (what's wired to what), not iconography.

use bevy_egui::egui;
use lunco_canvas::{DrawCtx, EdgeVisual, Node, NodeVisual, Pos};

use super::projection::{UsdPrimNodeData, UsdWireData, WireKind};

/// Card visual for a `"usd.prim"` node.
pub(crate) struct UsdPrimNodeVisual {
    pub type_name: String,
    pub programs: String,
    pub accent: Option<super::projection::DiagramAccent>,
    pub is_body: bool,
    pub is_boundary: bool,
}

impl NodeVisual for UsdPrimNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool) {
        let sr = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(sr.min.x, sr.min.y),
            egui::pos2(sr.max.x, sr.max.y),
        );
        let painter = ctx.ui.painter().clone().with_clip_rect(ctx.ui.clip_rect());
        let theme = lunco_theme::active(ctx.ui.ctx());
        let t = &theme.tokens;
        use super::projection::DiagramAccent;
        let program_color = match self.accent {
            Some(DiagramAccent::Model) => theme.schematic.class_model_badge,
            Some(DiagramAccent::Block) => theme.schematic.class_block_badge,
            Some(DiagramAccent::Record) => theme.schematic.class_record_badge,
            Some(DiagramAccent::Package) => theme.schematic.class_package_badge,
            Some(DiagramAccent::Class) => theme.schematic.class_class_badge,
            Some(DiagramAccent::Warning) => t.warning,
            None => t.node_border,
        };

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
        if !self.programs.is_empty() {
            painter.line_segment(
                [rect.left_top(), rect.right_top()],
                egui::Stroke::new(2.0, program_color),
            );
            if rect.height() > 28.0 && rect.width() > 46.0 {
                let mut font =
                    lunco_theme::TypographyRole::Caption.font_id(ctx.ui.style().as_ref());
                font.size *= ctx.viewport.zoom.clamp(0.65, 1.0);
                let galley = painter.layout_no_wrap(self.programs.clone(), font, program_color);
                let size = egui::vec2(
                    (galley.size().x + 12.0).min(rect.width() - 12.0),
                    galley.size().y + 4.0,
                );
                let badge = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.center().x - size.x * 0.5,
                        rect.min.y + 29.0_f32.min((rect.height() - 14.0).max(18.0)),
                    ),
                    size,
                );
                painter.rect_filled(badge, 3.0, program_color.gamma_multiply(0.18));
                painter
                    .clone()
                    .with_clip_rect(badge.intersect(ctx.ui.clip_rect()))
                    .galley(
                        egui::pos2(badge.center().x - galley.size().x * 0.5, badge.min.y + 2.0),
                        galley,
                        program_color,
                    );
            }
        }

        // Titles and backend badges share a fixed screen-space header above
        // the authored port rows, including on tall multi-port cards.
        if rect.height() > 12.0 {
            let card_painter = painter
                .clone()
                .with_clip_rect(rect.intersect(ctx.ui.clip_rect()));
            let mut font = lunco_theme::TypographyRole::Label.font_id(ctx.ui.style().as_ref());
            font.size *= ctx.viewport.zoom.clamp(0.65, 1.0);
            card_painter.text(
                egui::pos2(rect.center().x, rect.min.y + 6.0),
                egui::Align2::CENTER_TOP,
                &node.label,
                font,
                t.text,
            );
            if rect.height() > 40.0 && ctx.viewport.zoom >= 1.0 {
                let caption = lunco_theme::TypographyRole::Caption.font_id(ctx.ui.style().as_ref());
                if node.ports.iter().any(|port| port.kind.as_str() == "input") {
                    let side = if self.is_boundary {
                        egui::Align2::RIGHT_TOP
                    } else {
                        egui::Align2::LEFT_TOP
                    };
                    let x = if self.is_boundary {
                        rect.max.x - 9.0
                    } else {
                        rect.min.x + 9.0
                    };
                    card_painter.text(
                        egui::pos2(x, rect.min.y + 29.0),
                        side,
                        "IN",
                        caption.clone(),
                        t.port_input,
                    );
                }
                if node.ports.iter().any(|port| port.kind.as_str() == "output") {
                    let side = if self.is_boundary {
                        egui::Align2::LEFT_TOP
                    } else {
                        egui::Align2::RIGHT_TOP
                    };
                    let x = if self.is_boundary {
                        rect.min.x + 9.0
                    } else {
                        rect.max.x - 9.0
                    };
                    card_painter.text(
                        egui::pos2(x, rect.min.y + 29.0),
                        side,
                        "OUT",
                        caption.clone(),
                        t.port_output,
                    );
                }
                let caption_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + 38.0, rect.min.y),
                    egui::pos2(rect.max.x - 38.0, rect.max.y),
                );
                if self.programs.is_empty() {
                    card_painter
                        .clone()
                        .with_clip_rect(caption_rect.intersect(ctx.ui.clip_rect()))
                        .text(
                            egui::pos2(rect.center().x, rect.min.y + 29.0),
                            egui::Align2::CENTER_TOP,
                            &self.type_name,
                            lunco_theme::TypographyRole::Caption.font_id(ctx.ui.style().as_ref()),
                            t.text_subdued,
                        );
                }
            }
        }

        // Causal ports use circles; acausal connectors use hollow diamonds.
        // Labels retain the authored USD property leaf and connector kind.
        let zoom = ctx.viewport.zoom;
        let port_font = lunco_theme::TypographyRole::DenseData.font_id(ctx.ui.style().as_ref());
        let readable_rows = super::projection::PORT_ROW_H * zoom >= port_font.size * 1.25;
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
                "acausal" => theme.schematic.wire_unknown,
                _ => t.node_border,
            };
            if port.kind.as_str() == "acausal" {
                let center = egui::pos2(p.x, p.y);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + egui::vec2(0.0, -r),
                        center + egui::vec2(r, 0.0),
                        center + egui::vec2(0.0, r),
                        center + egui::vec2(-r, 0.0),
                    ],
                    fill,
                    egui::Stroke::new(1.5, col),
                ));
            } else {
                painter.circle_filled(egui::pos2(p.x, p.y), r, col);
                painter.circle_stroke(
                    egui::pos2(p.x, p.y),
                    r,
                    egui::Stroke::new(1.0, t.port_outline),
                );
            }
            if readable_rows {
                let (anchor, offset) = if port.local_offset.x < node.rect.width() * 0.5 {
                    (egui::Align2::LEFT_CENTER, egui::vec2(9.0, 0.0))
                } else {
                    (egui::Align2::RIGHT_CENTER, egui::vec2(-9.0, 0.0))
                };
                let mut label_rect = rect.intersect(ctx.ui.clip_rect());
                if !self.is_boundary {
                    if port.local_offset.x >= node.rect.width() * 0.5 {
                        label_rect.min.x = rect.center().x;
                    } else {
                        label_rect.max.x = rect.center().x;
                    }
                }
                painter.clone().with_clip_rect(label_rect).text(
                    egui::pos2(p.x + offset.x, p.y + offset.y),
                    anchor,
                    if port.kind.as_str() == "acausal" {
                        format!(
                            "{} · acausal",
                            port.id
                                .as_str()
                                .split_once(':')
                                .map(|(_, name)| name)
                                .unwrap_or(port.id.as_str())
                        )
                    } else {
                        port.id
                            .as_str()
                            .split_once(':')
                            .map(|(_, name)| name)
                            .unwrap_or(port.id.as_str())
                            .to_string()
                    },
                    port_font.clone(),
                    t.text_subdued,
                );
            }
        }
    }

    fn debug_name(&self) -> &str {
        "usd.prim"
    }
}

/// Orthogonal wire visual for a `"usd.wire"` edge, coloured by wire kind.
pub(crate) struct UsdWireVisual {
    pub kind: WireKind,
}

impl EdgeVisual for UsdWireVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from_screen: Pos,
        to_screen: Pos,
        waypoints_screen: &[Pos],
        selected: bool,
    ) {
        let theme = lunco_theme::active(ctx.ui.ctx());
        // Wire-by-domain is exactly what `SchematicTokens` models for Modelica;
        // Causal dataflow uses signal colour, joints use mechanical colour,
        // and unclassified acausal networks use the shared neutral wire token.
        let base = match self.kind {
            WireKind::Dataflow => theme.schematic.wire_signal,
            WireKind::Acausal => theme.schematic.wire_unknown,
            WireKind::Joint => theme.schematic.wire_mechanical,
        };
        let col = if selected {
            theme.tokens.node_border_selected
        } else {
            base
        };
        let width = if selected { 2.5 } else { 1.6 };
        let mut points = Vec::with_capacity(waypoints_screen.len() + 2);
        points.push(egui::pos2(from_screen.x, from_screen.y));
        points.extend(
            waypoints_screen
                .iter()
                .map(|point| egui::pos2(point.x, point.y)),
        );
        points.push(egui::pos2(to_screen.x, to_screen.y));
        let painter = ctx.ui.painter();
        for segment in points.windows(2) {
            painter.line_segment([segment[0], segment[1]], egui::Stroke::new(width, col));
        }

        // Arrowhead at the sink so signal direction reads at a glance.
        let a = points[points.len() - 2];
        let b = points[points.len() - 1];
        let dir = b - a;
        let len = dir.length();
        if self.kind == WireKind::Dataflow && len > 1.0 {
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
    let backends: std::collections::BTreeSet<_> = data
        .programs
        .iter()
        .map(|program| program.backend.as_str())
        .collect();
    let features = backends.into_iter().collect::<Vec<_>>().join(" · ");
    UsdPrimNodeVisual {
        type_name: data.type_name.clone(),
        programs: features,
        accent: data.accent,
        is_body: data.is_body,
        is_boundary: data.boundary.is_some(),
    }
}

/// Build the concrete edge visual from the typed payload (registry factory).
pub(crate) fn edge_visual(data: &UsdWireData) -> UsdWireVisual {
    UsdWireVisual { kind: data.kind }
}
