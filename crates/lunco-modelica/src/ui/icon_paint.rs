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
    Arrow, Bitmap, Color, CoordinateSystem, Ellipse, EllipseClosure, Extent, FillPattern,
    GraphicItem, Line, LinePattern, Point, Polygon, Rectangle, Text,
};

// Palette used by the active `paint_graphics_*` call. Kept in a
// thread-local so the leaf helpers can apply theme remap without
// re-plumbing every internal signature. `None` ⇒ identity (no remap).
thread_local! {
    static ACTIVE_PALETTE: std::cell::RefCell<Option<lunco_theme::ModelicaIconPalette>> =
        const { std::cell::RefCell::new(None) };
}

fn with_active_palette<R>(palette: Option<&lunco_theme::ModelicaIconPalette>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_PALETTE.with(|cell| cell.replace(palette.cloned()));
    let result = f();
    ACTIVE_PALETTE.with(|cell| {
        *cell.borrow_mut() = prev;
    });
    result
}

fn remap_color(c: egui::Color32) -> egui::Color32 {
    ACTIVE_PALETTE.with(|cell| {
        if let Some(p) = cell.borrow().as_ref() {
            p.remap(c)
        } else {
            c
        }
    })
}

/// Resolve a `FillPattern` + `fillColor` into the colour used for the
/// fill polygon. Modelica defines several variants (Solid, gradients
/// like HorizontalCylinder/VerticalCylinder/Sphere, hatching like
/// Horizontal/Vertical/Cross/Forward/Backward/CrossDiag); only `None`
/// means "no fill".
///
/// Per MLS Annex D, missing `fillColor` defaults to **black**
/// (`{0,0,0}`) — *not* transparent. Many MSL icons rely on this:
/// the canonical PartialTorque arrowhead `Polygon(points={...},
/// fillPattern=FillPattern.Solid)` omits `fillColor` and expects a
/// solid black arrow. Defaulting to transparent here renders only
/// the stroke outline of every such primitive — visually broken.
///
/// We collapse all gradient/hatch variants to flat colour for now
/// (visual difference is minor at icon scale; rendering nothing is
/// much worse). Future polish: emit `egui::Mesh` with per-vertex
/// colour interpolation for cylinder/sphere patterns.
fn effective_fill_color(
    pattern: FillPattern,
    color: Option<Color>,
) -> egui::Color32 {
    match pattern {
        FillPattern::None => egui::Color32::TRANSPARENT,
        _ => color_or_default(color, egui::Color32::BLACK),
    }
}

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
    paint_graphics_with_orientation(
        painter,
        screen_rect,
        coord_system,
        IconOrientation::default(),
        graphics,
    );
}

/// Same as [`paint_graphics`] but applies an instance-level
/// orientation (rotation + axis mirroring) to every primitive before
/// the screen-rect mapping. Used by the canvas projector to honour the
/// `Placement(transformation(rotation, extent={{x_high,…},…}))` of the
/// instance the icon was placed by — without this, rotated/mirrored
/// MSL components rendered axis-aligned even though their *ports*
/// were already on the correct edges (the visible "the body and the
/// ports disagree" bug).
pub fn paint_graphics_with_orientation(
    painter: &egui::Painter,
    screen_rect: egui::Rect,
    coord_system: CoordinateSystem,
    orientation: IconOrientation,
    graphics: &[GraphicItem],
) {
    paint_graphics_full(
        painter,
        screen_rect,
        coord_system,
        orientation,
        None,
        graphics,
    );
}

/// Full-surface entry point: orientation + `%name`/`%class` text
/// substitution.
///
/// Callers that know the instance name (canvas projector — the
/// component's `origin` carries it) pass a [`TextSubstitution`] so
/// Modelica `Text(textString="%name")` primitives resolve to the
/// actual instance label instead of printing the literal `%name`.
pub fn paint_graphics_full(
    painter: &egui::Painter,
    screen_rect: egui::Rect,
    coord_system: CoordinateSystem,
    orientation: IconOrientation,
    substitution: Option<&TextSubstitution<'_>>,
    graphics: &[GraphicItem],
) {
    paint_graphics_with_resolver(
        painter,
        screen_rect,
        coord_system,
        orientation,
        substitution,
        None,
        graphics,
    );
}

/// Like [`paint_graphics_full`] but also accepts a per-instance
/// value resolver used by MLS §18 `DynamicSelect` to swap the
/// static text for an evaluated expression at simulation time.
/// The resolver receives the variable name as written in the
/// icon expression (e.g. `m`, `port.m_flow`) and is expected to
/// prefix it with the component's instance path before looking
/// it up in the live snapshot. `None` resolver → static text only.
pub fn paint_graphics_with_resolver(
    painter: &egui::Painter,
    screen_rect: egui::Rect,
    coord_system: CoordinateSystem,
    orientation: IconOrientation,
    substitution: Option<&TextSubstitution<'_>>,
    resolver: Option<&dyn Fn(&str) -> Option<f64>>,
    graphics: &[GraphicItem],
) {
    paint_graphics_themed(
        painter,
        screen_rect,
        coord_system,
        orientation,
        substitution,
        resolver,
        None,
        graphics,
    )
}

/// Same as [`paint_graphics_with_resolver`], but with an explicit
/// theme palette. Pass `Some(&theme.modelica_icons)` for the active
/// theme to apply remap; `None` for identity (legacy callers).
pub fn paint_graphics_themed(
    painter: &egui::Painter,
    screen_rect: egui::Rect,
    coord_system: CoordinateSystem,
    orientation: IconOrientation,
    substitution: Option<&TextSubstitution<'_>>,
    resolver: Option<&dyn Fn(&str) -> Option<f64>>,
    palette: Option<&lunco_theme::ModelicaIconPalette>,
    graphics: &[GraphicItem],
) {
    with_active_palette(palette, || {
        let xform = coord_xform_oriented(coord_system.extent, screen_rect, orientation);
        for item in graphics {
            match item {
                GraphicItem::Rectangle(r) => paint_rectangle(painter, &xform, r, resolver),
                GraphicItem::Line(l) => paint_line(painter, &xform, l),
                GraphicItem::Polygon(p) => paint_polygon(painter, &xform, p),
                GraphicItem::Text(t) => paint_text(painter, &xform, t, substitution, resolver),
                GraphicItem::Ellipse(e) => paint_ellipse(painter, &xform, e),
                GraphicItem::Bitmap(b) => paint_bitmap(painter, &xform, b),
            }
        }
    });
}

/// Substitutions the renderer applies to `Text.text_string` before
/// drawing.
///
/// Modelica uses `%name`, `%class`, `%<par>` in icon text to print
/// runtime identity / class name / parameter values. We handle the
/// first two; `%<par>` needs live parameter values and is a follow-up
/// once the parameter-editor wiring is in (those values flow through
/// the same bus the Inspector will listen to).
#[derive(Debug, Clone, Copy, Default)]
pub struct TextSubstitution<'a> {
    /// Instance name to substitute for `%name` (e.g. `"R1"`).
    pub name: Option<&'a str>,
    /// Class name to substitute for `%class` (e.g. `"Resistor"`).
    pub class_name: Option<&'a str>,
    /// Pre-formatted parameter (name, value) pairs for `%paramName`
    /// substitution. Values come from instance modifications when
    /// available, falling back to class defaults; both are formatted
    /// to short display strings (numbers as-written, enum refs as
    /// leaf, strings unquoted) by the indexer / projector before
    /// reaching this struct.
    pub parameters: Option<&'a [(String, String)]>,
}

impl<'a> TextSubstitution<'a> {
    /// Apply the substitutions to `s`. Modelica's substitution syntax
    /// is `%name`, `%class`, `%<paramName>`, and `%%` (literal `%`).
    /// We resolve `%name` and `%class` from the fields above; any
    /// other `%<ident>` is stripped (replaced with empty) so MSL
    /// icons don't display their literal placeholder text (`%R`,
    /// `%controllerType`, …) when the parameter resolver isn't wired.
    /// Plumbing parameter values through `paint_graphics` is a
    /// follow-up — until then "show nothing" beats "show
    /// `%controllerType` as a label".
    pub fn apply(&self, s: &str) -> String {
        // Walk the string, eat any `%ident`, look up known names.
        // Cheap manual scan — avoids a regex dep for one production
        // site. Treats `%%` as literal `%` per MLS Annex D.
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            if let Some(&'%') = chars.peek() {
                chars.next();
                out.push('%');
                continue;
            }
            // Collect an identifier following the `%`. Modelica
            // identifiers are letter/`_` start, alnum/`_` continue.
            let mut ident = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    ident.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if ident.is_empty() {
                // Lone `%` followed by punctuation — keep the `%`.
                out.push('%');
                continue;
            }
            match ident.as_str() {
                "name" => {
                    if let Some(n) = self.name {
                        out.push_str(n);
                    }
                }
                "class" => {
                    if let Some(c) = self.class_name {
                        out.push_str(c);
                    }
                }
                other => {
                    // `%paramName` — look up the parameter's
                    // formatted value. Missing parameter (or class
                    // didn't expose one) drops to empty so we don't
                    // print the literal placeholder.
                    if let Some(params) = self.parameters {
                        if let Some((_, value)) = params
                            .iter()
                            .find(|(name, _)| name == other)
                        {
                            out.push_str(value);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Per-instance orientation applied uniformly to every primitive.
/// Mirroring happens in the icon's local Modelica frame *before*
/// rotation, matching MLS Annex D.
#[derive(Debug, Clone, Copy, Default)]
pub struct IconOrientation {
    pub rotation_deg: f32,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

// ---------------------------------------------------------------------------
// Coordinate transform
// ---------------------------------------------------------------------------

/// Linear transform from Modelica diagram coordinates to egui screen
/// pixels. Built once per icon and reused across every primitive.
///
/// Aspect ratio is preserved; the icon is centred in `screen_rect` and
/// the smaller of the two axis scales is used so nothing gets cropped.
/// When the icon was placed with rotation/mirror, those folds in too
/// via [`coord_xform_oriented`].
#[derive(Debug, Clone, Copy)]
pub struct CoordXform {
    /// Uniform world-units → pixels scale.
    pub scale: f32,
    /// Pixel offset of the source extent's centre after centring.
    pub offset: egui::Vec2,
    /// Centre of the source extent in Modelica units.
    pub src_center: egui::Vec2,
    /// Per-instance rotation, in degrees CCW (Modelica +Y-up frame).
    /// 0 for the simple [`coord_xform`] path.
    pub rotation_deg: f32,
    /// Per-instance mirror flags, applied in the Modelica frame
    /// before rotation (and before the screen Y flip).
    pub mirror_x: bool,
    pub mirror_y: bool,
}

pub fn coord_xform(src: Extent, dst: egui::Rect) -> CoordXform {
    coord_xform_oriented(src, dst, IconOrientation::default())
}

pub fn coord_xform_oriented(
    src: Extent,
    dst: egui::Rect,
    orientation: IconOrientation,
) -> CoordXform {
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
        rotation_deg: orientation.rotation_deg,
        mirror_x: orientation.mirror_x,
        mirror_y: orientation.mirror_y,
    }
}

impl CoordXform {
    /// Map a Modelica point (+Y up) to an egui screen position (+Y down).
    /// Honours the orientation (mirror + rotate around the icon centre)
    /// stashed by [`coord_xform_oriented`].
    pub fn to_screen(&self, p: Point) -> egui::Pos2 {
        // 1. Translate to icon-centre-relative Modelica coords.
        let mut dx = p.x as f32 - self.src_center.x;
        let mut dy = p.y as f32 - self.src_center.y;
        // 2. Mirror in the Modelica frame (before rotation, MLS Annex D).
        if self.mirror_x { dx = -dx; }
        if self.mirror_y { dy = -dy; }
        // 3. Rotate CCW in Modelica's +Y-up frame.
        if self.rotation_deg != 0.0 {
            let theta = self.rotation_deg.to_radians();
            let (s, c) = theta.sin_cos();
            let nx = c * dx - s * dy;
            let ny = s * dx + c * dy;
            dx = nx;
            dy = ny;
        }
        egui::pos2(
            self.offset.x + dx * self.scale,
            // Y flip: Modelica +Y up → screen +Y down
            self.offset.y - dy * self.scale,
        )
    }

    /// Apply a graphic primitive's local origin + rotation, then
    /// project to screen. `origin` is in Modelica coords, `rotation`
    /// is degrees CCW (matches MLS Annex D). Per-instance orientation
    /// (rotation + mirror) gets applied by [`crate::ui::icon_paint::to_screen`] downstream.
    pub fn to_screen_rotated(
        &self,
        p: Point,
        origin: Point,
        rotation_deg: f64,
    ) -> egui::Pos2 {
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

fn paint_rectangle(
    painter: &egui::Painter,
    xf: &CoordXform,
    r: &Rectangle,
    resolver: Option<&dyn Fn(&str) -> Option<f64>>,
) {
    // Build the four corners in local coords, rotate+translate, then
    // emit as a 4-vertex convex polygon. We don't use `painter.rect_*`
    // because rotation would not be applied.
    //
    // MLS §18 DynamicSelect on `extent` lets the model author swap
    // the static rectangle bounds for an expression evaluated at
    // simulation time — used for e.g. a tank fluid-level bar that
    // shrinks as the tank empties. Falls back to the static extent
    // if the resolver isn't supplied or any corner fails to evaluate.
    let extent = r
        .extent_dynamic
        .as_ref()
        .zip(resolver)
        .and_then(|(de, resolve)| de.eval(resolve))
        .unwrap_or(r.extent);
    let Extent { p1, p2 } = extent;
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

    // Fast path: axis-aligned, no rotation, has rounded corners →
    // use the rounded-rect helper so corner radius works. Any
    // non-`None` fill pattern collapses to a flat colour via
    // `effective_fill_color`; the rounded helper doesn't paint
    // gradients, but since we don't synthesise them anywhere yet
    // that's fine.
    if r.rotation == 0.0 && r.radius > 0.0 {
        let min = pts[0].min(pts[2]);
        let max = pts[0].max(pts[2]);
        let rect = egui::Rect::from_min_max(min, max);
        let fill = effective_fill_color(r.shape.fill_pattern, r.shape.fill_color);
        let radius_px = (r.radius as f32 * xf.scale).max(0.0);
        painter.rect_filled(rect, radius_px, fill);
        let stroke = stroke_for(r.shape.line_color, r.shape.line_pattern, r.shape.line_thickness, xf.scale);
        if stroke.width > 0.0 {
            painter.rect_stroke(rect, radius_px, stroke, egui::StrokeKind::Inside);
        }
        return;
    }

    // General path: rotated → polygon. `effective_fill_color`
    // collapses gradient/hatch variants to flat colour.
    let fill = effective_fill_color(r.shape.fill_pattern, r.shape.fill_color);
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
    let stroke = stroke_for(
        p.shape.line_color,
        p.shape.line_pattern,
        p.shape.line_thickness,
        xf.scale,
    );

    // Fill: tessellate with lyon using EvenOdd winding so concave and
    // self-intersecting polygons (bowties, X-shapes, sensor markers)
    // render correctly — same rule Qt's `QPainterPath` uses by
    // default, which is what OMEdit/Dymola rely on. egui's built-in
    // `Shape::convex_polygon` fans from vertex 0 and gets the wrong
    // halves filled on anything non-convex.
    let fill_color = effective_fill_color(p.shape.fill_pattern, p.shape.fill_color);
    if fill_color != egui::Color32::TRANSPARENT {
        if let Some(mesh) = tessellate_polygon_evenodd(&pts, fill_color) {
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    // Stroke: trace the authored path verbatim — for self-intersecting
    // polygons this draws the visible "X" of the crossing, even though
    // the X-quadrants aren't filled. Matches OMEdit's stroke behaviour.
    if stroke.width > 0.0 {
        let mut closed = pts;
        if closed.first() != closed.last() {
            closed.push(closed[0]);
        }
        painter.add(egui::Shape::line(closed, stroke));
    }
}

/// Triangulate a (possibly concave / self-intersecting) polygon using
/// the EvenOdd fill rule, returning an `egui::Mesh` ready to draw.
/// Returns `None` for degenerate inputs (fewer than 3 unique points
/// or tessellator failure — both render as "no fill").
fn tessellate_polygon_evenodd(
    pts: &[egui::Pos2],
    color: egui::Color32,
) -> Option<egui::Mesh> {
    use lyon_path::Path;
    use lyon_tessellation::geometry_builder::BuffersBuilder;
    use lyon_tessellation::{
        FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
    };

    if pts.len() < 3 {
        return None;
    }

    // Strip any explicit closing-vertex duplicate — lyon closes the
    // path itself via `end(true)` and a duplicated point creates a
    // zero-length edge that some configurations treat as a degenerate
    // self-intersection.
    let trimmed: &[egui::Pos2] = if pts.len() >= 2 && pts.first() == pts.last() {
        &pts[..pts.len() - 1]
    } else {
        pts
    };
    if trimmed.len() < 3 {
        return None;
    }

    let mut builder = Path::builder();
    builder.begin(lyon_geom::point(trimmed[0].x, trimmed[0].y));
    for p in &trimmed[1..] {
        builder.line_to(lyon_geom::point(p.x, p.y));
    }
    builder.end(true);
    let path = builder.build();

    let mut buffers: VertexBuffers<lyon_geom::Point<f32>, u32> = VertexBuffers::new();
    let mut tessellator = FillTessellator::new();
    let mut buffer_builder = BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position());
    let opts = FillOptions::default()
        .with_fill_rule(FillRule::EvenOdd)
        .with_tolerance(0.5);
    if tessellator
        .tessellate_path(&path, &opts, &mut buffer_builder)
        .is_err()
    {
        return None;
    }

    if buffers.vertices.is_empty() || buffers.indices.is_empty() {
        return None;
    }

    let mut mesh = egui::Mesh::default();
    mesh.vertices = buffers
        .vertices
        .iter()
        .map(|v| egui::epaint::Vertex {
            pos: egui::pos2(v.x, v.y),
            uv: egui::epaint::WHITE_UV,
            color,
        })
        .collect();
    mesh.indices = buffers.indices.clone();
    Some(mesh)
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
        painter.add(egui::Shape::line(pts.clone(), stroke));
    } else {
        // Dashed/dotted: emit per-segment dashed runs. Cheap and
        // matches the canvas's existing dashed-rect style.
        let (dash, gap) = pattern_metrics(l.pattern, stroke.width);
        for win in pts.windows(2) {
            paint_dashed_segment(painter, win[0], win[1], stroke, dash, gap);
        }
    }

    // Arrow heads at start (pointing *backwards* along the first
    // segment) and end (forwards along the last segment). Signal
    // wires in MSL use this to indicate flow direction.
    let head_px = ((l.arrow_size as f32) * xf.scale).max(4.0);
    let color = color_or_default(l.color, egui::Color32::BLACK);
    if !matches!(l.arrow[0], Arrow::None) && pts.len() >= 2 {
        paint_arrow_head(painter, pts[1], pts[0], l.arrow[0], head_px, stroke.width, color);
    }
    if !matches!(l.arrow[1], Arrow::None) && pts.len() >= 2 {
        let n = pts.len();
        paint_arrow_head(
            painter,
            pts[n - 2],
            pts[n - 1],
            l.arrow[1],
            head_px,
            stroke.width,
            color,
        );
    }
}

/// Paint an arrow head at `tip` pointing away from `from`. Style
/// controls fill / open / half-wing shape; `head_px` is the overall
/// length of the head along the line.
fn paint_arrow_head(
    painter: &egui::Painter,
    from: egui::Pos2,
    tip: egui::Pos2,
    style: Arrow,
    head_px: f32,
    stroke_w: f32,
    color: egui::Color32,
) {
    let dx = tip.x - from.x;
    let dy = tip.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        return;
    }
    // Unit vector along the line (from → tip) and its perpendicular.
    let ux = dx / len;
    let uy = dy / len;
    let px = -uy;
    let py = ux;
    // Head half-width: arrows read best at ~½ the head length.
    let hw = head_px * 0.5;
    // Base of the triangle (behind the tip).
    let bx = tip.x - ux * head_px;
    let by = tip.y - uy * head_px;
    let left = egui::pos2(bx + px * hw, by + py * hw);
    let right = egui::pos2(bx - px * hw, by - py * hw);
    let stroke = egui::Stroke::new(stroke_w.max(1.0), color);
    match style {
        Arrow::None => {}
        Arrow::Filled => {
            painter.add(egui::Shape::convex_polygon(
                vec![tip, left, right],
                color,
                stroke,
            ));
        }
        Arrow::Open => {
            painter.line_segment([tip, left], stroke);
            painter.line_segment([tip, right], stroke);
        }
        Arrow::Half => {
            // Single wing on the "left" side (CCW perpendicular) —
            // the MLS-standard asymmetric arrow.
            painter.line_segment([tip, left], stroke);
        }
    }
}

fn paint_text(
    painter: &egui::Painter,
    xf: &CoordXform,
    t: &Text,
    substitution: Option<&TextSubstitution<'_>>,
    resolver: Option<&dyn Fn(&str) -> Option<f64>>,
) {
    // MLS §18 `DynamicSelect`: if the icon declared a dynamic
    // counterpart and we have a value resolver (i.e. simulation is
    // running and the canvas built a per-instance lookup), evaluate
    // it and use the result. Falls back to the static text on
    // unresolved variables or unsupported expression syntax — the
    // icon never goes blank.
    let dynamic_rendered: Option<String> = match (&t.text_string_dynamic, resolver) {
        (Some(expr), Some(resolve)) => expr.eval(resolve).map(|v| v.to_display()),
        _ => None,
    };
    let base = dynamic_rendered.as_deref().unwrap_or(t.text_string.as_str());
    if base.is_empty() {
        return;
    }
    // Substitute `%name` / `%class` before rendering. Most MSL icons
    // set `textString="%name"` so this is where "Resistor" becomes
    // "R1" across the entire diagram.
    let rendered: String = match substitution {
        Some(sub) => sub.apply(base),
        None => base.to_string(),
    };
    if rendered.is_empty() {
        return;
    }
    let p1 = xf.to_screen_rotated(t.extent.p1, t.origin, t.rotation);
    let p2 = xf.to_screen_rotated(t.extent.p2, t.origin, t.rotation);
    let rect = egui::Rect::from_two_pos(p1, p2);
    // MLS: `fontSize=0` means "auto-fit to extent". `rect` is the
    // AABB of the two transformed extent corners — when the parent
    // has a non-axis-aligned rotation (e.g. 270°), the AABB's
    // width/height swap, so picking either alone gives different
    // sizes for the same authored Text on rotated vs unrotated
    // instances. Use the SHORTER dimension so rotation is invariant.
    // 0.7× and a [10, 48] clamp keep MSL %name labels readable at
    // fit-zoom without dwarfing the component icons.
    // No clamp — text scales linearly with the rect (which scales
    // with the icon, which scales with viewport zoom). A label that
    // fits its node at zoom X also fits at zoom 2X. Clamping caused
    // labels to overflow the icon at high zoom or freeze at low.
    let color = color_or_default(t.text_color, egui::Color32::from_gray(20));
    let font_size_px = if t.font_size > 0.0 {
        t.font_size as f32 * xf.scale
    } else {
        // MLS §18: `fontSize=0` means "size the font to fit the
        // extent". OMEdit / Dymola interpret this as fit BOTH
        // dimensions: start at extent height (the natural cap height
        // for one-line text), then shrink uniformly if the rendered
        // text width exceeds the extent width. Without the width
        // shrink, wide MSL diagram labels like
        // `extent={{-98,59},{-31,51}}` "reference speed generation"
        // (67 wide × 8 tall) render at a 8-icon-unit font that's far
        // too wide for the 67-unit extent, so the label visibly
        // overflows or wraps past its authored frame.
        let w = rect.width().abs();
        let h = rect.height().abs();
        let mut size = if w < 0.5 {
            h * 0.95
        } else if h < 0.5 {
            w * 0.95
        } else {
            h * 0.95
        };
        if w >= 0.5 && size > 1.0 {
            // Egui galley measure: cheap no-wrap layout to get the
            // rendered width at this font size. If it overruns the
            // extent, scale down by the ratio.
            let galley = painter.layout_no_wrap(
                rendered.clone(),
                egui::FontId::proportional(size),
                color,
            );
            let measured_w = galley.size().x;
            if measured_w > w {
                size *= w / measured_w;
            }
        }
        size
    };
    if font_size_px < 1.0 {
        return;
    }
    // Total rotation = primitive's own `rotation` + the parent
    // instance's orientation (carried on `xf.rotation_deg`). Modelica
    // and screen Y are flipped, so the screen-space angle is the
    // negative of the Modelica CCW angle. Snap multiples of 90° to
    // exact PI/2 so rotated labels look crisp (no sub-pixel jitter
    // from float trig).
    let total_rot_modelica = t.rotation as f32 + xf.rotation_deg;
    let mut angle_screen = -total_rot_modelica.to_radians();
    let snapped = (angle_screen / std::f32::consts::FRAC_PI_2).round();
    if (angle_screen - snapped * std::f32::consts::FRAC_PI_2).abs() < 1e-3 {
        angle_screen = snapped * std::f32::consts::FRAC_PI_2;
    }
    if angle_screen.abs() < 1e-4 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &rendered,
            egui::FontId::proportional(font_size_px),
            color,
        );
    } else {
        // Build a Galley and emit a rotated TextShape — egui's
        // higher-level `painter.text` fixes angle to 0.
        let galley = painter.layout_no_wrap(
            rendered,
            egui::FontId::proportional(font_size_px),
            color,
        );
        // TextShape rotates around `pos` (top-left). To centre the
        // rotated label on `rect.center()`, push `pos` back by the
        // rotated half-size vector — places the post-rotation centre
        // exactly on rect.center.
        let half = galley.size() * 0.5;
        let (sin, cos) = angle_screen.sin_cos();
        let rotated_half = egui::vec2(
            cos * half.x - sin * half.y,
            sin * half.x + cos * half.y,
        );
        let pos = rect.center() - rotated_half;
        let mut shape = egui::epaint::TextShape::new(pos, galley, color);
        shape.angle = angle_screen;
        painter.add(egui::Shape::Text(shape));
    }
}

/// Paint an Ellipse fitted to `e.extent`. Honours partial-arc
/// `startAngle` / `endAngle` (degrees, MLS convention: 0° = +x,
/// CCW), and `closure`:
/// - `None`  → open arc, stroke only, no fill
/// - `Chord` → close end→start with a chord, fill the bow region
/// - `Radial`→ close with two radii, fill the pie slice
///
/// The arc is built as a polyline in icon-local space so it inherits
/// both the primitive's own rotation and the instance-level
/// orientation (mirror + rotate) through `to_screen_rotated`.
fn paint_ellipse(painter: &egui::Painter, xf: &CoordXform, e: &Ellipse) {
    const SEGMENTS: usize = 64;
    let Extent { p1, p2 } = e.extent;
    let cx = (p1.x + p2.x) * 0.5;
    let cy = (p1.y + p2.y) * 0.5;
    let rx = (p2.x - p1.x).abs() * 0.5;
    let ry = (p2.y - p1.y).abs() * 0.5;
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }

    let start_rad = (e.start_angle as f64).to_radians();
    let end_rad = (e.end_angle as f64).to_radians();
    let span_deg = (e.end_angle - e.start_angle).abs();
    let is_full = span_deg >= 359.999 || span_deg <= 0.001;

    // Sample density proportional to the arc fraction so a thin arc
    // doesn't get over-tessellated and a full circle keeps its 64
    // segments. Minimum 8 keeps small arcs smooth.
    let arc_fraction = if is_full {
        1.0
    } else {
        ((end_rad - start_rad).abs() / std::f64::consts::TAU).min(1.0)
    };
    let steps = ((SEGMENTS as f64) * arc_fraction).ceil() as usize;
    let steps = steps.max(8);

    let pts: Vec<egui::Pos2> = if is_full {
        (0..SEGMENTS)
            .map(|i| {
                let theta = (i as f64) * (std::f64::consts::TAU / SEGMENTS as f64);
                let x = cx + rx * theta.cos();
                let y = cy + ry * theta.sin();
                xf.to_screen_rotated(Point { x, y }, e.origin, e.rotation)
            })
            .collect()
    } else {
        (0..=steps)
            .map(|i| {
                let t = (i as f64) / (steps as f64);
                let theta = start_rad + (end_rad - start_rad) * t;
                let x = cx + rx * theta.cos();
                let y = cy + ry * theta.sin();
                xf.to_screen_rotated(Point { x, y }, e.origin, e.rotation)
            })
            .collect()
    };

    let fill = effective_fill_color(e.shape.fill_pattern, e.shape.fill_color);
    let stroke = stroke_for(
        e.shape.line_color,
        e.shape.line_pattern,
        e.shape.line_thickness,
        xf.scale,
    );

    if is_full {
        // Full ellipse: fill as a convex 64-vertex polygon, stroke as
        // a closed ring.
        if fill != egui::Color32::TRANSPARENT {
            painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                fill,
                egui::Stroke::NONE,
            ));
        }
        if stroke.width > 0.0 {
            let mut ring = pts;
            if let Some(first) = ring.first().copied() {
                ring.push(first);
            }
            painter.add(egui::Shape::line(ring, stroke));
        }
        return;
    }

    // Partial arc — closure rule decides shape.
    match e.closure {
        EllipseClosure::None => {
            // Stroke-only open polyline. No fill (an open path
            // doesn't enclose a region — Modelica spec is clear
            // that `closure=None` means "don't close").
            if stroke.width > 0.0 {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        EllipseClosure::Chord => {
            // Close end→start with a chord. Fill the enclosed bow.
            if fill != egui::Color32::TRANSPARENT {
                if let Some(mesh) = tessellate_polygon_evenodd(&pts, fill) {
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
            if stroke.width > 0.0 {
                let mut ring = pts;
                if let Some(first) = ring.first().copied() {
                    ring.push(first);
                }
                painter.add(egui::Shape::line(ring, stroke));
            }
        }
        EllipseClosure::Radial => {
            // Pie slice — close with two radii to the centre.
            let centre = xf.to_screen_rotated(
                Point { x: cx, y: cy },
                e.origin,
                e.rotation,
            );
            let mut pie = Vec::with_capacity(pts.len() + 1);
            pie.extend_from_slice(&pts);
            pie.push(centre);
            if fill != egui::Color32::TRANSPARENT {
                if let Some(mesh) = tessellate_polygon_evenodd(&pie, fill) {
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
            if stroke.width > 0.0 {
                let mut ring = pie;
                if let Some(first) = ring.first().copied() {
                    ring.push(first);
                }
                painter.add(egui::Shape::line(ring, stroke));
            }
        }
    }
}

/// Paint a Bitmap primitive.
///
/// Supports `filename="modelica://Package.Name/path/img.png"` (resolved
/// via `lunco_assets::msl_dir`) and `filename="modelica://Package.Name/file.png"`
/// (same). Base64 `imageSource` is not yet wired — decoding inline
/// buffers every frame is the wrong shape; when we need it, the
/// base64 will be decoded once and cached by hash.
///
/// If the image fails to load (missing asset, decode error), the
/// Bitmap renders as a labelled placeholder so users see a visible
/// marker where the image should be — easier to debug than a silent
/// blank.
fn paint_bitmap(painter: &egui::Painter, xf: &CoordXform, b: &Bitmap) {
    let p1 = xf.to_screen_rotated(b.extent.p1, b.origin, b.rotation);
    let p2 = xf.to_screen_rotated(b.extent.p2, b.origin, b.rotation);
    let rect = egui::Rect::from_two_pos(p1, p2);

    if let Some(name) = b.filename.as_ref() {
        if let Some(tex) = texture_for_bitmap(painter.ctx(), name) {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), rect, uv, egui::Color32::WHITE);
            return;
        }
    }

    // Placeholder when no filename / image_source we can load.
    let fill = egui::Color32::from_rgba_unmultiplied(200, 200, 210, 40);
    let stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(120, 120, 140, 160),
    );
    painter.rect_filled(rect, 2.0, fill);
    painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    let label = b
        .filename
        .as_deref()
        .map(|s| {
            s.rsplit('/')
                .next()
                .unwrap_or(s)
                .to_string()
        })
        .unwrap_or_else(|| "[image]".to_string());
    let font_size = (rect.height().abs() * 0.3).clamp(8.0, 14.0);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &label,
        egui::FontId::monospace(font_size),
        egui::Color32::from_gray(100),
    );
}

/// Texture cache for Bitmap primitives. Keyed by the raw filename
/// string so `modelica://Pkg/icon.png` and a plain `icon.png` hit
/// different slots (they resolve differently).
fn texture_for_bitmap(
    ctx: &egui::Context,
    filename: &str,
) -> Option<egui::TextureHandle> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    // The cache stores `Option<TextureHandle>` so a failed load is
    // remembered and not retried every frame (failure is usually a
    // missing asset, not a transient error).
    static CACHE: OnceLock<Mutex<HashMap<String, Option<egui::TextureHandle>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(slot) = map.get(filename) {
            return slot.clone();
        }
    }

    let bytes = load_bitmap_bytes(filename)?;
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i.to_rgba8(),
        Err(_) => {
            if let Ok(mut map) = cache.lock() {
                map.insert(filename.to_string(), None);
            }
            return None;
        }
    };
    let size = [img.width() as usize, img.height() as usize];
    let pixels: Vec<egui::Color32> = img
        .pixels()
        .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    let color_img = egui::ColorImage {
        size,
        pixels,
        source_size: egui::vec2(size[0] as f32, size[1] as f32),
    };
    let handle = ctx.load_texture(
        format!("modelica-bitmap:{filename}"),
        color_img,
        egui::TextureOptions::LINEAR,
    );
    if let Ok(mut map) = cache.lock() {
        map.insert(filename.to_string(), Some(handle.clone()));
    }
    Some(handle)
}

/// Resolve a Modelica `fileName` value to raw bytes on disk.
///
/// - `modelica://Pkg/path/img.png` → `<msl_root>/Pkg/path/img.png`
///   (walking the package-as-directory convention).
/// - Plain relative path → tried under the MSL root as-is (best
///   effort).
fn load_bitmap_bytes(filename: &str) -> Option<Vec<u8>> {
    let rel = match filename.strip_prefix("modelica://") {
        Some(tail) => tail.to_string(),
        None => filename.to_string(),
    };
    let msl_root = lunco_assets::msl_dir();
    let candidate = msl_root.join(&rel);
    // Route through lunco-storage — `std::fs` is clippy-banned in domain
    // crates and absent on wasm. `FileStorage` reads native disk; on wasm
    // it errors → `.ok()` → `None`, which the icon renderer already
    // tolerates (bitmap simply doesn't draw).
    use lunco_storage::Storage;
    lunco_storage::FileStorage::new()
        .read_sync(&lunco_storage::StorageHandle::File(candidate))
        .ok()
}

// ---------------------------------------------------------------------------
// Color / stroke helpers
// ---------------------------------------------------------------------------

fn to_egui_color(c: Color) -> egui::Color32 {
    remap_color(egui::Color32::from_rgb(c.r, c.g, c.b))
}

fn color_or_default(c: Option<Color>, default: egui::Color32) -> egui::Color32 {
    // Authored colours go through the palette; the workbench-provided
    // default (e.g. MLS Annex D's implicit black) also gets remapped
    // so a `Polygon` without `fillColor` still draws correctly under
    // a dark theme instead of an invisible all-black blob.
    c.map(to_egui_color).unwrap_or_else(|| remap_color(default))
}

/// Build a stroke from Modelica `lineColor` + `pattern` + `thickness`.
/// Returns a zero-width stroke only when the pattern is explicitly
/// `None`. Missing `lineColor` defaults to **black** per MLS Annex D
/// — the previous "no colour → no stroke" check was a regression
/// hiding every primitive without an explicit `color=` (e.g.
/// `SpringDamper`'s zigzag `Line`, generic frame lines, default
/// outlines on shapes that only set fillColor).
fn stroke_for(
    color: Option<Color>,
    pattern: LinePattern,
    thickness_mm: f64,
    scale_px_per_unit: f32,
) -> egui::Stroke {
    if matches!(pattern, LinePattern::None) {
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
