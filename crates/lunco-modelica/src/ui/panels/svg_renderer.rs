use std::sync::OnceLock;

use bevy_egui::egui;
use usvg::{Node, Options, Transform, Tree};

/// Shared [`usvg::Options`] built once per process.
///
/// usvg's default `Options` has an empty font database, so any SVG
/// that uses `<text>` with `font-family: sans-serif` (every MSL
/// component icon) logs a warning per parse: `WARN usvg::text: No
/// match for 'sans-serif' font-family.`
///
/// Fix: populate the font DB once with system fonts and set a
/// concrete fallback for the three generic families usvg consults.
/// `load_system_fonts()` is slow on first call (tens-of-ms to a few
/// hundred, depending on OS) so we cache the whole `Options`.
fn svg_options() -> &'static Options<'static> {
    static OPTIONS: OnceLock<Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut opt = Options::default();
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        // Pick a sensible default for each generic family. The exact
        // name only has to exist in the DB after `load_system_fonts`
        // — usvg falls back to any matching family if the named one
        // is missing, so these are best-effort hints.
        db.set_sans_serif_family("DejaVu Sans");
        db.set_serif_family("DejaVu Serif");
        db.set_monospace_family("DejaVu Sans Mono");
        opt
    })
}

type ParsedTreeCache = std::sync::Mutex<
    std::collections::HashMap<(usize, usize), Option<std::sync::Arc<Tree>>>,
>;

fn parsed_tree_cache() -> &'static ParsedTreeCache {
    use std::sync::OnceLock;
    static CACHE: OnceLock<ParsedTreeCache> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Parsed-SVG cache keyed by the input bytes pointer (plus length as
/// a collision guard). Callers load SVG bytes through an `Arc<Vec<u8>>`
/// cache elsewhere, so the pointer is stable across frames for the
/// same asset. Caching the parsed [`Tree`] here means we no longer
/// re-run usvg's XML+path parser per icon per frame (7 nodes at 60 Hz
/// = 420 parses/sec — large MSL icons stacked enough of those that
/// the UI thread fell behind and the app appeared frozen).
///
/// Entries live forever: the icon set is fixed at build time, and
/// the parsed trees are tens of KB each at most.
fn parsed_tree(svg_data: &[u8]) -> Option<std::sync::Arc<Tree>> {
    let cache = parsed_tree_cache();
    let key = (svg_data.as_ptr() as usize, svg_data.len());
    if let Ok(map) = cache.lock() {
        if let Some(cached) = map.get(&key) {
            return cached.clone();
        }
    }
    let parsed = Tree::from_data(svg_data, svg_options())
        .ok()
        .map(std::sync::Arc::new);
    if let Ok(mut map) = cache.lock() {
        map.insert(key, parsed.clone());
    }
    parsed
}

/// Pre-parse an SVG byte buffer into the [`parsed_tree`] cache so
/// the first canvas paint that needs it doesn't pay usvg's
/// XML+path parse cost (50-300ms per Modelica icon — measured
/// 567ms summed across an Add of one Resistor that pulled three
/// fresh icons in the same frame). Idempotent — re-calling with
/// the same Arc backing is a hashmap-hit no-op.
///
/// The cache key is `(bytes.as_ptr(), bytes.len())`. Callers must
/// pre-warm by calling this with the **same** `Arc<Vec<u8>>` the
/// canvas's `svg_bytes_for` returns at render time, otherwise the
/// pointers won't match and the cache won't hit. The `prewarm_svg_bytes`
/// path in `canvas_diagram.rs` populates that exact cache; this
/// function uses the same `Arc` it stored.
pub fn prewarm_parse(svg_bytes: &std::sync::Arc<Vec<u8>>) {
    let bytes: &[u8] = svg_bytes.as_ref();
    parsed_tree(bytes); // populate cache; throw the result away
}

/// Translates a usvg::Tree into a list of egui::Shape primitives, scaled to fit in `rect`.
pub fn draw_svg_to_egui(
    painter: &egui::Painter,
    rect: egui::Rect,
    svg_data: &[u8],
) {
    draw_svg_to_egui_oriented(
        painter,
        rect,
        svg_data,
        SvgOrientation::default(),
    );
}

/// Per-instance orientation parameters for the SVG renderer. Same
/// shape as [`crate::icon_paint::IconOrientation`] but kept separate
/// to avoid circular crate-internal coupling between the SVG path
/// (icons that ship as pre-rasterised assets) and the typed
/// `paint_graphics` path (icons authored in source). The two paths
/// converge at the canvas projector, which constructs both from the
/// same `IconTransform` field on the node.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvgOrientation {
    pub rotation_deg: f32,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

/// Same as [`draw_svg_to_egui`] but applies an instance-level rotation
/// + mirror around the rect's centre. Used by the canvas projector to
/// honour `Placement(transformation(rotation, extent={{x_high,…},…}))`
/// for MSL components — without this, an MSL Sensor whose Placement
/// reverses its X extent (so the flange port appears on the right
/// edge) still rendered its body axis-aligned, contradicting where
/// the wire actually entered.
pub fn draw_svg_to_egui_oriented(
    painter: &egui::Painter,
    rect: egui::Rect,
    svg_data: &[u8],
    orientation: SvgOrientation,
) {
    let Some(tree) = parsed_tree(svg_data) else {
        return;
    };

    let size = tree.size();
    let scale_x = rect.width() / size.width() as f32;
    let scale_y = rect.height() / size.height() as f32;
    let scale = scale_x.min(scale_y);

    // Center the SVG within the rect if aspect ratios differ
    let dx = rect.left() + (rect.width() - size.width() as f32 * scale) / 2.0;
    let dy = rect.top() + (rect.height() - size.height() as f32 * scale) / 2.0;

    // Build the orientation transform around the rect's centre, in
    // screen coordinates. Compose right-to-left:
    //   T(centre) · Rotation · MirrorY (Modelica→screen) · MirrorReq
    //              · MirrorY · T(-centre)
    // Simplified — we just apply mirror flags as ±1 scales and rotate,
    // both in screen space (Y already flipped by the SVG's natural
    // top-left origin, so mirror_y on screen == mirror_y in Modelica).
    let cx = rect.center().x;
    let cy = rect.center().y;
    let (sx, sy) = (
        if orientation.mirror_x { -1.0 } else { 1.0 },
        if orientation.mirror_y { -1.0 } else { 1.0 },
    );
    // Modelica rotation is CCW in +Y-up frame → on +Y-down screen it
    // becomes CW. Negate the angle so the visual matches the source.
    let theta = -orientation.rotation_deg.to_radians();
    let (sn, cs) = theta.sin_cos();
    // Linear part: rotation · scale.
    let a = cs * sx;
    let b = sn * sx;
    let c = -sn * sy;
    let d = cs * sy;
    // Translation part: T(centre) - linear · centre.
    let tx = cx - (a * cx + c * cy);
    let ty = cy - (b * cx + d * cy);
    let orient_xform = Transform::from_row(a, b, c, d, tx, ty);

    let scale_xform = Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    // Apply orientation AFTER the scale-and-place: scale_xform maps
    // SVG-natural coords to screen; orient_xform then rotates the
    // screen output around the rect centre.
    let combined = orient_xform.pre_concat(scale_xform);

    render_node(painter, tree.root(), combined);
}

fn render_node(painter: &egui::Painter, node: &usvg::Group, transform: Transform) {
    for child in node.children() {
        match child {
            Node::Group(ref group) => {
                let local_transform = transform.pre_concat(group.transform());
                render_node(painter, group, local_transform);
            }
            Node::Path(ref path) => {
                render_path(painter, path, transform);
            }
            _ => {}
        }
    }
}

fn render_path(painter: &egui::Painter, path: &usvg::Path, transform: Transform) {
    if !path.is_visible() {
        return;
    }

    let mut points = Vec::new();
    
    for segment in path.data().segments() {
        match segment {
            usvg::tiny_skia_path::PathSegment::MoveTo(p) => {
                if !points.is_empty() {
                    draw_points(painter, &points, path, transform);
                    points.clear();
                }
                points.push(egui::pos2(p.x, p.y));
            }
            usvg::tiny_skia_path::PathSegment::LineTo(p) => {
                points.push(egui::pos2(p.x, p.y));
            }
            usvg::tiny_skia_path::PathSegment::QuadTo(p1, p) => {
                points.push(egui::pos2(p1.x, p1.y));
                points.push(egui::pos2(p.x, p.y));
            }
            usvg::tiny_skia_path::PathSegment::CubicTo(p1, p2, p) => {
                points.push(egui::pos2(p1.x, p1.y));
                points.push(egui::pos2(p2.x, p2.y));
                points.push(egui::pos2(p.x, p.y));
            }
            usvg::tiny_skia_path::PathSegment::Close => {
                if let Some(&first) = points.first() {
                    points.push(first);
                }
                draw_points(painter, &points, path, transform);
                points.clear();
            }
        }
    }

    if !points.is_empty() {
        draw_points(painter, &points, path, transform);
    }
}

fn draw_points(painter: &egui::Painter, points: &[egui::Pos2], path: &usvg::Path, transform: Transform) {
    let mapped_points: Vec<egui::Pos2> = points.iter().map(|p| {
        let mut pt = usvg::tiny_skia_path::Point::from_xy(p.x, p.y);
        transform.map_point(&mut pt);
        egui::pos2(pt.x, pt.y)
    }).collect();

    if let Some(ref fill) = path.fill() {
        if let usvg::Paint::Color(c) = fill.paint() {
            let color = egui::Color32::from_rgba_unmultiplied(c.red, c.green, c.blue, (fill.opacity().get() * 255.0) as u8);
            if mapped_points.len() >= 3 {
                painter.add(egui::Shape::convex_polygon(mapped_points.clone(), color, egui::Stroke::NONE));
            }
        }
    }

    if let Some(ref stroke) = path.stroke() {
        if let usvg::Paint::Color(c) = stroke.paint() {
            let color = egui::Color32::from_rgba_unmultiplied(c.red, c.green, c.blue, (stroke.opacity().get() * 255.0) as u8);
            let sx = (transform.sx * transform.sx + transform.kx * transform.kx).sqrt() as f32;
            let egui_stroke = egui::Stroke::new(stroke.width().get() as f32 * sx, color);
            painter.add(egui::Shape::line(mapped_points, egui_stroke));
        }
    }
}
