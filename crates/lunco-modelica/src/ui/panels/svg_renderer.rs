use bevy_egui::egui;
use usvg::{Node, Tree, Options, Transform};

/// Translates a usvg::Tree into a list of egui::Shape primitives, scaled to fit in `rect`.
pub fn draw_svg_to_egui(
    painter: &egui::Painter,
    rect: egui::Rect,
    svg_data: &[u8],
) {
    let opt = Options::default();
    let tree = match Tree::from_data(svg_data, &opt) {
        Ok(t) => t,
        Err(_) => return,
    };

    let size = tree.size();
    let scale_x = rect.width() / size.width() as f32;
    let scale_y = rect.height() / size.height() as f32;
    let scale = scale_x.min(scale_y);

    // Center the SVG within the rect if aspect ratios differ
    let dx = rect.left() + (rect.width() - size.width() as f32 * scale) / 2.0;
    let dy = rect.top() + (rect.height() - size.height() as f32 * scale) / 2.0;

    render_node(painter, tree.root(), Transform::from_row(scale, 0.0, 0.0, scale, dx, dy));
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
