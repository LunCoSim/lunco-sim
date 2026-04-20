//! Render Modelica `Icon`/`Diagram` graphics into an egui painter.
//!
//! Stateless, allocation-light, and decoupled from the canvas crate.
//! Callers (the canvas's per-component visual, a future Icon preview,
//! tests) hand in:
//!
//! - the destination `egui::Rect` (the on-screen card the icon should
//!   fill),
//! - the source [`CoordinateSystem`] (Modelica diagram extent, +Y up,
//!   typically `{{-100,-100},{100,100}}`),
//! - the slice of [`GraphicItem`]s from
//!   [`crate::annotations::Icon::graphics`] / `Diagram::graphics`.
//!
//! The painter maps source → destination with a uniform-scale fit
//! that preserves aspect ratio (Dymola/OMEdit do the same — non-square
//! icon extents stay centred in their host card rather than stretching).
//!
//! The Y axis is flipped: Modelica `+Y up` → egui `+Y down`. The
//! [`coord_xform`] helper is the only place that flip happens.

use bevy_egui::egui;

use crate::annotations::{
    Color, CoordinateSystem, Extent, FillPattern, GraphicItem, LinePattern, Line, Point,
    Polygon, Rectangle, Text,
};

/// Paint the full graphics list into `screen_rect`.
///
/// Items render in source order — later items paint on top of earlier
/// ones, matching Modelica's draw-order convention.
pub fn paint_graphics(
    painter: &egui::Painter,
    screen_rect: egui::Rect,
    coord_system: CoordinateSystem,
    graphics: &[GraphicItem],
) {
    let xform = coord_xform(coord_system.extent, screen_rect);
    for item in graphics {
        match item {
            GraphicItem::Rectangle(r) => paint_rectangle(painter, &xform, r),
            GraphicItem::Line(l) => paint_line(painter, &xform, l),
            GraphicItem::Polygon(p) => paint_polygon(painter, &xform, p),
            GraphicItem::Text(t) => paint_text(painter, &xform, t),
        }
    }
}

// ---------------------------------------------------------------------------
// Coordinate transform
// ---------------------------------------------------------------------------

/// Linear transform from Modelica diagram coordinates to egui screen
/// pixels. Built once per icon and reused across every primitive.
///
/// Aspect ratio is preserved; the icon is centred in `screen_rect` and
/// the smaller of the two axis scales is used so nothing gets cropped.
#[derive(Debug, Clone, Copy)]
pub struct CoordXform {
    /// Uniform world-units → pixels scale.
    pub scale: f32,
    /// Pixel offset of the source extent's centre after centring.
    pub offset: egui::Vec2,
    /// Centre of the source extent in Modelica units.
    pub src_center: egui::Vec2,
}

pub fn coord_xform(src: Extent, dst: egui::Rect) -> CoordXform {
    let src_w = (src.p2.x - src.p1.x).abs() as f32;
    let src_h = (src.p2.y - src.p1.y).abs() as f32;
    let scale = if src_w > 0.0 && src_h > 0.0 {
        (dst.width() / src_w).min(dst.height() / src_h)
    } else {
        1.0
    };
    let src_cx = ((src.p1.x + src.p2.x) * 0.5) as f32;
    let src_cy = ((src.p1.y + src.p2.y) * 0.5) as f32;
    CoordXform {
        scale,
        offset: dst.center().to_vec2(),
        src_center: egui::vec2(src_cx, src_cy),
    }
}

impl CoordXform {
    /// Map a Modelica point (+Y up) to an egui screen position (+Y down).
    pub fn to_screen(&self, p: Point) -> egui::Pos2 {
        let dx = p.x as f32 - self.src_center.x;
        let dy = p.y as f32 - self.src_center.y;
        egui::pos2(
            self.offset.x + dx * self.scale,
            // Y flip: Modelica +Y up → screen +Y down
            self.offset.y - dy * self.scale,
        )
    }

    /// Apply a graphic primitive's local origin + rotation, then
    /// project to screen. `origin` is in Modelica coords, `rotation`
    /// is degrees CCW (matches MLS Annex D).
    pub fn to_screen_rotated(
        &self,
        p: Point,
        origin: Point,
        rotation_deg: f64,
    ) -> egui::Pos2 {
        // Rotate around (0,0) in local space, then translate by origin,
        // then project to screen. CCW in Modelica's +Y-up frame becomes
        // CW once Y flips on screen — so we negate the angle in the
        // screen-space rotation and the visual matches the source.
        let theta = (rotation_deg as f32).to_radians();
        let (s, c) = theta.sin_cos();
        let lx = p.x as f32;
        let ly = p.y as f32;
        let rx = c * lx - s * ly;
        let ry = s * lx + c * ly;
        let world = Point {
            x: (origin.x as f32 + rx) as f64,
            y: (origin.y as f32 + ry) as f64,
        };
        self.to_screen(world)
    }
}

// ---------------------------------------------------------------------------
// Per-primitive painters
// ---------------------------------------------------------------------------

fn paint_rectangle(painter: &egui::Painter, xf: &CoordXform, r: &Rectangle) {
    // Build the four corners in local coords, rotate+translate, then
    // emit as a 4-vertex convex polygon. We don't use `painter.rect_*`
    // because rotation would not be applied.
    let Extent { p1, p2 } = r.extent;
    let corners_local = [
        Point { x: p1.x, y: p1.y },
        Point { x: p2.x, y: p1.y },
        Point { x: p2.x, y: p2.y },
        Point { x: p1.x, y: p2.y },
    ];
    let pts: Vec<egui::Pos2> = corners_local
        .iter()
        .map(|p| xf.to_screen_rotated(*p, r.origin, r.rotation))
        .collect();

    // Fast path: axis-aligned, no rotation, fill pattern is solid →
    // use the rounded-rect helper so corner radius works.
    if r.rotation == 0.0
        && r.radius > 0.0
        && matches!(r.shape.fill_pattern, FillPattern::Solid | FillPattern::None)
    {
        let min = pts[0].min(pts[2]);
        let max = pts[0].max(pts[2]);
        let rect = egui::Rect::from_min_max(min, max);
        let fill = if matches!(r.shape.fill_pattern, FillPattern::Solid) {
            color_or_default(r.shape.fill_color, egui::Color32::TRANSPARENT)
        } else {
            egui::Color32::TRANSPARENT
        };
        let radius_px = (r.radius as f32 * xf.scale).max(0.0);
        painter.rect_filled(rect, radius_px, fill);
        let stroke = stroke_for(r.shape.line_color, r.shape.line_pattern, r.shape.line_thickness, xf.scale);
        if stroke.width > 0.0 {
            painter.rect_stroke(rect, radius_px, stroke, egui::StrokeKind::Inside);
        }
        return;
    }

    // General path: rotated / patterned → polygon.
    let fill = if matches!(r.shape.fill_pattern, FillPattern::Solid) {
        color_or_default(r.shape.fill_color, egui::Color32::TRANSPARENT)
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = stroke_for(
        r.shape.line_color,
        r.shape.line_pattern,
        r.shape.line_thickness,
        xf.scale,
    );
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

fn paint_polygon(painter: &egui::Painter, xf: &CoordXform, p: &Polygon) {
    if p.points.len() < 3 {
        return;
    }
    let pts: Vec<egui::Pos2> = p
        .points
        .iter()
        .map(|pt| xf.to_screen_rotated(*pt, p.origin, p.rotation))
        .collect();
    let fill = if matches!(p.shape.fill_pattern, FillPattern::Solid) {
        color_or_default(p.shape.fill_color, egui::Color32::TRANSPARENT)
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = stroke_for(
        p.shape.line_color,
        p.shape.line_pattern,
        p.shape.line_thickness,
        xf.scale,
    );
    // `convex_polygon` is fine for the slice-1 menagerie (rect bells,
    // tank domes, gimbal tile) — concave polygons would render their
    // fill incorrectly, but stroke is always right. A future slice can
    // tessellate via `egui::epaint::Shape::Path` with a `PathShape`
    // when a real concave case appears.
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

fn paint_line(painter: &egui::Painter, xf: &CoordXform, l: &Line) {
    if l.points.len() < 2 {
        return;
    }
    let pts: Vec<egui::Pos2> = l
        .points
        .iter()
        .map(|pt| xf.to_screen_rotated(*pt, l.origin, l.rotation))
        .collect();
    let stroke = stroke_for(l.color, l.pattern, l.thickness, xf.scale);
    if stroke.width <= 0.0 {
        return;
    }
    if matches!(l.pattern, LinePattern::Solid) {
        // Solid → one continuous Path so corners join cleanly.
        painter.add(egui::Shape::line(pts, stroke));
    } else {
        // Dashed/dotted: emit per-segment dashed runs. Cheap and
        // matches the canvas's existing dashed-rect style.
        let (dash, gap) = pattern_metrics(l.pattern, stroke.width);
        for win in pts.windows(2) {
            paint_dashed_segment(painter, win[0], win[1], stroke, dash, gap);
        }
    }
}

fn paint_text(painter: &egui::Painter, xf: &CoordXform, t: &Text) {
    if t.text_string.is_empty() {
        return;
    }
    let p1 = xf.to_screen_rotated(t.extent.p1, t.origin, t.rotation);
    let p2 = xf.to_screen_rotated(t.extent.p2, t.origin, t.rotation);
    let rect = egui::Rect::from_two_pos(p1, p2);
    // MLS: `fontSize=0` means "auto-fit to extent". We approximate
    // auto-fit as 80% of the extent's smaller dimension — readable
    // without overflowing on stubby labels.
    let font_size_px = if t.font_size > 0.0 {
        (t.font_size as f32 * xf.scale).max(6.0)
    } else {
        (rect.height().abs() * 0.8).clamp(6.0, 72.0)
    };
    let color = color_or_default(t.text_color, egui::Color32::from_gray(20));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &t.text_string,
        egui::FontId::proportional(font_size_px),
        color,
    );
}

// ---------------------------------------------------------------------------
// Color / stroke helpers
// ---------------------------------------------------------------------------

fn to_egui_color(c: Color) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

fn color_or_default(c: Option<Color>, default: egui::Color32) -> egui::Color32 {
    c.map(to_egui_color).unwrap_or(default)
}

/// Build a stroke from Modelica `lineColor` + `pattern` + `thickness`.
/// Returns a zero-width stroke if the pattern is `None` or no colour
/// resolved — the per-primitive painters short-circuit on that.
fn stroke_for(
    color: Option<Color>,
    pattern: LinePattern,
    thickness_mm: f64,
    scale_px_per_unit: f32,
) -> egui::Stroke {
    if matches!(pattern, LinePattern::None) || color.is_none() {
        return egui::Stroke::NONE;
    }
    // Modelica thickness is in mm at the diagram's coordinate scale;
    // multiplying by the icon's px-per-unit scale gives an on-screen
    // pixel width that visually matches what Dymola renders, then
    // floor at 0.75 px so hairlines stay visible at small icon sizes.
    let width = ((thickness_mm as f32) * scale_px_per_unit).max(0.75);
    egui::Stroke::new(width, color_or_default(color, egui::Color32::BLACK))
}

fn pattern_metrics(pattern: LinePattern, stroke_w: f32) -> (f32, f32) {
    let unit = stroke_w.max(1.0);
    match pattern {
        LinePattern::Solid | LinePattern::None => (unit, 0.0),
        LinePattern::Dash => (unit * 5.0, unit * 3.0),
        LinePattern::Dot => (unit, unit * 2.0),
        LinePattern::DashDot => (unit * 5.0, unit * 2.0), // approximation
        LinePattern::DashDotDot => (unit * 5.0, unit * 2.0), // approximation
    }
}

/// Paint a dashed segment from `a` to `b`. Dash lengths are in screen
/// pixels so they look right at any zoom.
fn paint_dashed_segment(
    painter: &egui::Painter,
    a: egui::Pos2,
    b: egui::Pos2,
    stroke: egui::Stroke,
    dash: f32,
    gap: f32,
) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON || dash <= 0.0 {
        painter.line_segment([a, b], stroke);
        return;
    }
    let nx = dx / len;
    let ny = dy / len;
    let step = dash + gap;
    let mut t = 0.0_f32;
    while t < len {
        let s = t;
        let e = (t + dash).min(len);
        painter.line_segment(
            [
                egui::pos2(a.x + nx * s, a.y + ny * s),
                egui::pos2(a.x + nx * e, a.y + ny * e),
            ],
            stroke,
        );
        t += step;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dst() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(100.0, 200.0), egui::pos2(300.0, 400.0))
    }

    fn default_cs() -> CoordinateSystem {
        CoordinateSystem::default()
    }

    #[test]
    fn xform_centre_maps_to_dst_centre() {
        let xf = coord_xform(default_cs().extent, dst());
        let p = xf.to_screen(Point { x: 0.0, y: 0.0 });
        assert!((p.x - 200.0).abs() < 1e-3);
        assert!((p.y - 300.0).abs() < 1e-3);
    }

    #[test]
    fn xform_y_is_flipped() {
        let xf = coord_xform(default_cs().extent, dst());
        // Modelica +Y top → screen lower y.
        let top = xf.to_screen(Point { x: 0.0, y: 100.0 });
        let bot = xf.to_screen(Point { x: 0.0, y: -100.0 });
        assert!(top.y < bot.y);
    }

    #[test]
    fn xform_aspect_preserved_landscape_dst() {
        // Square source into a wide destination should pick the smaller
        // (vertical) scale and centre horizontally.
        let dst = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 100.0));
        let xf = coord_xform(default_cs().extent, dst);
        // 200-unit source → 100 px (limited by height) → scale 0.5
        assert!((xf.scale - 0.5).abs() < 1e-3);
        // Source corner (-100,-100) bottom-left of source → screen (150, 100)
        // because horizontal centring puts the 100px-wide square at x=150..250.
        let bl = xf.to_screen(Point { x: -100.0, y: -100.0 });
        assert!((bl.x - 150.0).abs() < 1.0);
        assert!((bl.y - 100.0).abs() < 1.0);
    }

    #[test]
    fn rotation_90_swaps_axes() {
        // Apply a 90° CCW rotation to the local point (10, 0). In
        // Modelica frame it should land at (0, 10) — meaning on screen,
        // a point that was to the right of the origin ends up *above* it
        // (since +Y up). After Y-flip, "above" = smaller screen y.
        let xf = coord_xform(default_cs().extent, dst());
        let unrotated = xf.to_screen_rotated(
            Point { x: 10.0, y: 0.0 },
            Point { x: 0.0, y: 0.0 },
            0.0,
        );
        let rotated = xf.to_screen_rotated(
            Point { x: 10.0, y: 0.0 },
            Point { x: 0.0, y: 0.0 },
            90.0,
        );
        // Unrotated is to the right of centre, same y as centre.
        assert!(unrotated.x > 200.0);
        assert!((unrotated.y - 300.0).abs() < 1.0);
        // Rotated is above centre (smaller y), same x as centre.
        assert!((rotated.x - 200.0).abs() < 1.0);
        assert!(rotated.y < 300.0);
    }

    #[test]
    fn stroke_zero_for_pattern_none_or_no_color() {
        assert_eq!(
            stroke_for(Some(Color { r: 0, g: 0, b: 0 }), LinePattern::None, 0.5, 10.0).width,
            0.0
        );
        assert_eq!(
            stroke_for(None, LinePattern::Solid, 0.5, 10.0).width,
            0.0
        );
    }

    #[test]
    fn stroke_min_width_floored_at_hairline() {
        // 0.01 mm × 0.1 px/unit = 0.001 px — should clamp to 0.75.
        let s = stroke_for(
            Some(Color { r: 0, g: 0, b: 0 }),
            LinePattern::Solid,
            0.01,
            0.1,
        );
        assert!((s.width - 0.75).abs() < 1e-3);
    }
}
