//! Paint primitives + wire helpers — leaf functions used by the
//! canvas's [`NodeVisual`] / [`EdgeVisual`] paint paths. No Bevy
//! world access; all inputs are owned values + the egui painter.

use bevy_egui::egui;

/// Perpendicular distance from point `p` to segment `a`→`b`, in
/// screen pixels. Used for hit-testing wire hover.
pub(super) fn dist_point_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.x * ab.x + ab.y * ab.y;
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((ap.x * ab.x + ap.y * ab.y) / len_sq).clamp(0.0, 1.0);
    let proj = egui::pos2(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).length()
}

/// Paint a compact tooltip near `pointer` showing `text`. Uses the
/// wire's own color for the accent border so the user's eye links
/// the tooltip to the wire they're hovering.
pub(super) fn paint_wire_tooltip(
    painter: &egui::Painter,
    pointer: egui::Pos2,
    text: &str,
    accent: egui::Color32,
) {
    // Draw on a Tooltip-order layer rather than the painter's own
    // layer so the tooltip sits ABOVE any node icons that might
    // overlap it. Wires are drawn before nodes (so ports sit on
    // top visually), which would otherwise occlude the edge
    // tooltip when the hover point is near a component body.
    let ctx = painter.ctx().clone();
    let top = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("lunco_modelica_wire_tooltip"),
        ))
        .with_clip_rect(painter.clip_rect());
    let font = lunco_theme::TypographyRole::DenseData.font_id(ctx.global_style().as_ref());
    let galley = top.layout_no_wrap(
        text.to_string(),
        font,
        egui::Color32::from_rgb(235, 235, 240),
    );
    let pad = egui::vec2(6.0, 3.0);
    // Offset so the tooltip doesn't sit under the cursor.
    let min = egui::pos2(pointer.x + 12.0, pointer.y + 12.0);
    let rect = egui::Rect::from_min_size(min, galley.size() + pad * 2.0);
    top.rect_filled(
        rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(20, 22, 28, 235),
    );
    top.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, accent),
        egui::StrokeKind::Inside,
    );
    top.galley(rect.min + pad, galley, egui::Color32::PLACEHOLDER);
}

/// Selection-state brightener — shifts each channel ~30% toward white
/// while preserving hue. Used so wires keep their domain colour even
/// while highlighted.
pub(super) fn brighten(c: egui::Color32) -> egui::Color32 {
    let lift = |v: u8| (v as u16 + 80).min(255) as u8;
    egui::Color32::from_rgb(lift(c.r()), lift(c.g()), lift(c.b()))
}

/// Paint a dashed rectangle outline. Used for partial classes so
/// the source template remains distinguishable. Dashes are expressed in screen pixels because the
/// caller has already transformed to screen-space — so the dash
/// pattern stays the same visual size regardless of zoom. `radius`
/// is currently unused (corners are sampled as-if straight for
/// simplicity); revisit if the corner elision gets noticed.
pub(super) fn paint_dashed_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    _radius: f32,
    stroke: egui::Stroke,
) {
    let dash_len = 4.0;
    let gap_len = 3.0;
    let period = dash_len + gap_len;
    // Walk each of the four edges, emitting dash-sized segments.
    let edges = [
        (rect.min, egui::pos2(rect.max.x, rect.min.y)), // top
        (egui::pos2(rect.max.x, rect.min.y), rect.max), // right
        (rect.max, egui::pos2(rect.min.x, rect.max.y)), // bottom
        (egui::pos2(rect.min.x, rect.max.y), rect.min), // left
    ];
    for (a, b) in edges {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < f32::EPSILON {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let mut t = 0.0_f32;
        while t < len {
            let end = (t + dash_len).min(len);
            painter.line_segment(
                [
                    egui::pos2(a.x + ux * t, a.y + uy * t),
                    egui::pos2(a.x + ux * end, a.y + uy * end),
                ],
                stroke,
            );
            t += period;
        }
    }
}

/// Squared perpendicular distance from `p` to the finite segment
/// `(a,b)`. Endpoint-clamped — clicking past the end doesn't count.
pub(super) fn segment_dist_sq(
    p: lunco_canvas::Pos,
    a: lunco_canvas::Pos,
    b: lunco_canvas::Pos,
) -> f32 {
    let ax = b.x - a.x;
    let ay = b.y - a.y;
    let len_sq = ax * ax + ay * ay;
    if len_sq < f32::EPSILON {
        let dx = p.x - a.x;
        let dy = p.y - a.y;
        return dx * dx + dy * dy;
    }
    let t = (((p.x - a.x) * ax + (p.y - a.y) * ay) / len_sq).clamp(0.0, 1.0);
    let foot_x = a.x + t * ax;
    let foot_y = a.y + t * ay;
    let dx = p.x - foot_x;
    let dy = p.y - foot_y;
    dx * dx + dy * dy
}
