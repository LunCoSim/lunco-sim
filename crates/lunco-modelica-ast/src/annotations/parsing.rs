//! Inner extractors and expression walkers for Modelica annotations.

use super::graphics::*;
use super::layers::*;
use super::placement::*;
use super::types::*;
use rumoca_core::{OpBinary, OpUnary};
use rumoca_ir_ast::{ClassDef, Expression, TerminalType};
use std::collections::HashSet;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Public extractors
// ---------------------------------------------------------------------------

/// Extract the `Placement(...)` annotation from a component's annotation list.
pub fn extract_placement(annotations: &[Expression]) -> Option<Placement> {
    let placement_call = find_call(annotations, "Placement")?;
    let placement_args = call_args(placement_call)?;
    let transformation =
        find_call(placement_args, "transformation").and_then(extract_transformation)?;
    Some(Placement { transformation })
}

/// Extract the `Icon(...)` annotation from a class's annotation list.
pub fn extract_icon(annotations: &[Expression]) -> Option<Icon> {
    extract_icon_with_visibility(annotations, &HashSet::new())
}

/// Same as [`extract_icon`] but skips graphic primitives whose
/// `visible=` flag resolves to `false`.
pub fn extract_icon_with_visibility(
    annotations: &[Expression],
    falsy_params: &HashSet<String>,
) -> Option<Icon> {
    let icon_call = find_call(annotations, "Icon")?;
    let icon_args = call_args(icon_call)?;
    Some(Icon {
        coordinate_system: extract_coordinate_system(icon_args).unwrap_or_default(),
        graphics: extract_graphics_with_visibility(icon_args, falsy_params),
    })
}

/// Extract the standard `Diagram(coordinateSystem=..., graphics={...})`
/// annotation from a class's annotation list.
///
/// This maps *only* the Modelica `Diagram` annotation. LunCo's live
/// plot tiles live in the orthogonal `__LunCo(plotNodes={...})` vendor
/// annotation and are extracted separately via
/// [`extract_lunco_plot_nodes`] — a class with plot tiles but no
/// `Diagram` block (e.g. a pure behaviour model) correctly returns
/// `None` here while still surfacing its plot nodes through that call.
pub fn extract_diagram(annotations: &[Expression]) -> Option<Diagram> {
    let diagram_call = find_call(annotations, "Diagram")?;
    let diagram_args = call_args(diagram_call)?;
    Some(Diagram {
        coordinate_system: extract_coordinate_system(diagram_args).unwrap_or_default(),
        graphics: extract_graphics(diagram_args),
    })
}

/// Extract `__LunCo(plotNodes={LunCoAnnotations.PlotNode(...), ...})`
/// from a class annotation list. Returns an empty Vec if the vendor
/// annotation or its `plotNodes` array is missing.
pub fn extract_lunco_plot_nodes(annotations: &[Expression]) -> Vec<LunCoPlotNode> {
    let Some(call) = find_call(annotations, "__LunCo") else {
        return Vec::new();
    };
    let Some(args) = call_args(call) else {
        return Vec::new();
    };
    let Some(plot_nodes_arr) = named_arg(args, "plotNodes") else {
        return Vec::new();
    };
    let Some(elements) = array_elements(plot_nodes_arr) else {
        return Vec::new();
    };
    elements
        .iter()
        .filter_map(extract_lunco_plot_node_record)
        .collect()
}

fn extract_lunco_plot_node_record(expr: &Expression) -> Option<LunCoPlotNode> {
    if !crate::ast_extract::is_plot_node_record_call(expr) {
        return None;
    }
    let args = call_args(expr)?;
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    let signal = named_arg(args, "signal").and_then(crate::ast_extract::string_literal_value)?;
    if signal.is_empty() {
        return None;
    }
    let title = named_arg(args, "title")
        .and_then(crate::ast_extract::string_literal_value)
        .unwrap_or_default();
    Some(LunCoPlotNode {
        extent,
        signal,
        title,
    })
}

/// Extract the `experiment(...)` annotation from a class's annotation
/// list. Returns `None` when the call is absent. Returns `Some` even
/// when no recognized fields are inside — the *presence* of the call
/// is itself meaningful (Dymola / OMEdit treat experiment-tagged
/// classes as simulation roots regardless of which fields are set).
pub fn extract_experiment(annotations: &[Expression]) -> Option<Experiment> {
    let call = find_call(annotations, "experiment")?;
    let args = call_args(call).unwrap_or(&[]);
    Some(Experiment {
        start_time: named_arg(args, "StartTime").and_then(extract_number),
        stop_time: named_arg(args, "StopTime").and_then(extract_number),
        tolerance: named_arg(args, "Tolerance").and_then(extract_number),
        interval: named_arg(args, "Interval").and_then(extract_number),
        number_of_intervals: named_arg(args, "NumberOfIntervals").and_then(extract_number),
    })
}

// ---------------------------------------------------------------------------
// Inheritance logic
// ---------------------------------------------------------------------------

/// Walk a class's `extends` chain and merge inherited + local icon
/// graphics into a single [`Icon`]. The merge algorithm (recurse into
/// bases first, append local graphics last, inherit the coordinate
/// system from the first source that declares one) is shared by index and
/// runtime callers. The resolver and the source of `falsy_params` remain
/// owned by those callers. The resolver receives the declaring scope and the
/// authored base reference, and returns the exact native-resolved identity.
pub fn project_inherited_icon<F>(
    class_name: &str,
    class: &ClassDef,
    resolver: &mut F,
    falsy_params: &HashSet<String>,
    visited: &mut HashSet<String>,
) -> Option<Icon>
where
    F: FnMut(&str, &str) -> Option<(String, Arc<ClassDef>)>,
{
    if !visited.insert(class_name.to_string()) {
        return None; // cycle
    }

    let mut merged_graphics: Vec<GraphicItem> = Vec::new();
    let mut inherited_cs: Option<CoordinateSystem> = None;
    for ext in &class.extends {
        let base_name = ext.base_name.to_string();

        let Some((resolved_name, base_class)) = resolver(class_name, &base_name) else {
            continue;
        };
        if let Some(base_icon) = project_inherited_icon(
            &resolved_name,
            base_class.as_ref(),
            resolver,
            falsy_params,
            visited,
        ) {
            merged_graphics.extend(base_icon.graphics);
            inherited_cs = Some(base_icon.coordinate_system);
        }
    }

    let local = extract_icon_with_visibility(&class.annotation, falsy_params);
    let local_cs = local.as_ref().map(|i| i.coordinate_system);
    if let Some(icon) = local {
        merged_graphics.extend(icon.graphics);
    }

    if merged_graphics.is_empty() && local_cs.is_none() && inherited_cs.is_none() {
        return None;
    }

    Some(Icon {
        coordinate_system: local_cs.or(inherited_cs).unwrap_or_default(),
        graphics: merged_graphics,
    })
}

// ---------------------------------------------------------------------------
// Inner helpers
// ---------------------------------------------------------------------------

fn extract_transformation(call: &Expression) -> Option<Transformation> {
    let args = call_args(call)?;
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    let origin = named_arg(args, "origin")
        .and_then(extract_point)
        .unwrap_or(Point { x: 0.0, y: 0.0 });
    let rotation = named_arg(args, "rotation")
        .and_then(extract_number)
        .unwrap_or(0.0);
    Some(Transformation {
        extent,
        origin,
        rotation,
    })
}

fn extract_coordinate_system(args: &[Expression]) -> Option<CoordinateSystem> {
    let cs_call = find_call(args, "coordinateSystem")?;
    let cs_args = call_args(cs_call)?;
    let extent = named_arg(cs_args, "extent").and_then(extract_extent)?;
    Some(CoordinateSystem { extent })
}

fn extract_graphics(args: &[Expression]) -> Vec<GraphicItem> {
    extract_graphics_with_visibility(args, &HashSet::new())
}

fn extract_graphics_with_visibility(
    args: &[Expression],
    falsy_params: &HashSet<String>,
) -> Vec<GraphicItem> {
    let Some(graphics_array) = named_arg(args, "graphics") else {
        return Vec::new();
    };
    let Some(elements) = array_elements(graphics_array) else {
        return Vec::new();
    };
    elements
        .iter()
        .filter_map(|e| extract_graphic_item_filtered(e, falsy_params))
        .collect()
}

fn extract_graphic_item_filtered(
    expr: &Expression,
    falsy_params: &HashSet<String>,
) -> Option<GraphicItem> {
    let name = call_name(expr)?;
    let args = call_args(expr)?;
    if is_visibility_falsy(args, falsy_params) {
        return None;
    }
    match name {
        "Rectangle" => Some(GraphicItem::Rectangle(extract_rectangle(args)?)),
        "Line" => Some(GraphicItem::Line(extract_line(args)?)),
        "Polygon" => Some(GraphicItem::Polygon(extract_polygon(args)?)),
        "Text" => Some(GraphicItem::Text(extract_text(args)?)),
        "Ellipse" => Some(GraphicItem::Ellipse(extract_ellipse(args)?)),
        "Bitmap" => Some(GraphicItem::Bitmap(extract_bitmap(args)?)),
        _ => None,
    }
}

fn is_visibility_falsy(args: &[Expression], falsy_params: &HashSet<String>) -> bool {
    let Some(vis) = named_arg(args, "visible") else {
        return false;
    };
    eval_visibility_falsy(vis, falsy_params)
}

fn eval_visibility_falsy(expr: &Expression, falsy_params: &HashSet<String>) -> bool {
    match expr {
        Expression::Terminal {
            terminal_type,
            token,
            ..
        } => matches!(terminal_type, TerminalType::Bool) && token.text.as_ref() == "false",
        Expression::ComponentReference(cref) => cref
            .parts
            .first()
            .map(|p| falsy_params.contains(p.ident.text.as_ref()))
            .unwrap_or(false),
        Expression::Unary { .. } => false,
        Expression::Parenthesized { inner, .. } => eval_visibility_falsy(inner, falsy_params),
        _ => false,
    }
}

fn extract_rectangle(args: &[Expression]) -> Option<Rectangle> {
    let raw_extent = named_arg(args, "extent")?;
    let (extent, extent_dynamic) = extract_extent_with_dynamic(raw_extent)?;
    Some(Rectangle {
        shape: extract_filled_shape(args),
        extent,
        extent_dynamic,
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
        radius: named_arg(args, "radius")
            .and_then(extract_number)
            .unwrap_or(0.0),
    })
}

fn extract_extent_with_dynamic(expr: &Expression) -> Option<(Extent, Option<DynExtent>)> {
    if call_name(expr) == Some("DynamicSelect") {
        let cargs = call_args(expr).unwrap_or(&[]);
        let static_extent = cargs.first().and_then(extract_extent)?;
        let dyn_extent = cargs.get(1).and_then(extract_dyn_extent);
        Some((static_extent, dyn_extent))
    } else {
        Some((extract_extent(expr)?, None))
    }
}

fn extract_dyn_extent(expr: &Expression) -> Option<DynExtent> {
    let outer = array_elements(expr)?;
    if outer.len() != 2 {
        return None;
    }
    let p1 = array_elements(&outer[0])?;
    let p2 = array_elements(&outer[1])?;
    if p1.len() != 2 || p2.len() != 2 {
        return None;
    }
    Some(DynExtent {
        x1: expr_to_dyn(&p1[0])?,
        y1: expr_to_dyn(&p1[1])?,
        x2: expr_to_dyn(&p2[0])?,
        y2: expr_to_dyn(&p2[1])?,
    })
}

pub fn extract_line_points(annotation: &[Expression]) -> Vec<(f32, f32)> {
    extract_line_full(annotation)
        .map(|r| r.points)
        .unwrap_or_default()
}

pub fn extract_line_route(annotation: &[Expression]) -> Option<(Vec<(f32, f32)>, bool)> {
    extract_line_full(annotation).map(|r| (r.points, r.smooth_bezier))
}

pub fn extract_line_full(annotation: &[Expression]) -> Option<LineRoute> {
    let line_call = find_call(annotation, "Line")?;
    let line_args = call_args(line_call)?;
    let line = extract_line(line_args)?;
    let points: Vec<(f32, f32)> = line
        .points
        .iter()
        .map(|p| (p.x as f32, p.y as f32))
        .collect();
    let color = line.color.map(|c| [c.r, c.g, c.b]);
    let thickness = if (line.thickness - 0.25).abs() > f64::EPSILON {
        Some(line.thickness as f32)
    } else {
        None
    };
    Some(LineRoute {
        points,
        smooth_bezier: line.smooth_bezier,
        color,
        thickness,
    })
}

fn extract_line(args: &[Expression]) -> Option<Line> {
    let points_arg = named_arg(args, "points")?;
    let points = if call_name(points_arg) == Some("DynamicSelect") {
        let cargs = call_args(points_arg).unwrap_or(&[]);
        cargs.first().and_then(extract_point_array)?
    } else {
        extract_point_array(points_arg)?
    };
    let arrow = {
        let mut out = [Arrow::None, Arrow::None];
        if let Some(arr) = named_arg(args, "arrow") {
            if let Some(elems) = array_elements(arr) {
                for (i, e) in elems.iter().take(2).enumerate() {
                    if let Some(id) = extract_enum_ident(e) {
                        if let Some(a) = Arrow::from_ident(&id) {
                            out[i] = a;
                        }
                    }
                }
            }
        }
        out
    };
    Some(Line {
        points,
        color: named_arg(args, "color").and_then(extract_color),
        pattern: named_arg(args, "pattern")
            .and_then(extract_enum_ident)
            .and_then(|s| LinePattern::from_ident(&s))
            .unwrap_or_default(),
        thickness: named_arg(args, "thickness")
            .and_then(extract_number)
            .unwrap_or(0.25),
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
        arrow,
        arrow_size: named_arg(args, "arrowSize")
            .and_then(extract_number)
            .unwrap_or(3.0),
        smooth_bezier: named_arg(args, "smooth")
            .and_then(extract_enum_ident)
            .map(|s| s.rsplit('.').next().unwrap_or(&s) == "Bezier")
            .unwrap_or(false),
    })
}

fn extract_polygon(args: &[Expression]) -> Option<Polygon> {
    let points = named_arg(args, "points").and_then(extract_point_array)?;
    Some(Polygon {
        shape: extract_filled_shape(args),
        points,
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
    })
}

fn extract_text(args: &[Expression]) -> Option<Text> {
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    let text_string_arg = named_arg(args, "textString")?;
    let (text_string, text_string_dynamic) = if call_name(text_string_arg) == Some("DynamicSelect")
    {
        let cargs = call_args(text_string_arg).unwrap_or(&[]);
        let s = cargs
            .first()
            .and_then(crate::ast_extract::string_literal_value)
            .unwrap_or_default();
        let d = cargs.get(1).and_then(expr_to_dyn);
        (s, d)
    } else {
        (
            crate::ast_extract::string_literal_value(text_string_arg).unwrap_or_default(),
            None,
        )
    };
    Some(Text {
        extent,
        text_string,
        text_string_dynamic,
        font_size: named_arg(args, "fontSize")
            .and_then(extract_number)
            .unwrap_or(0.0),
        text_color: named_arg(args, "textColor").and_then(extract_color),
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
    })
}

fn extract_ellipse(args: &[Expression]) -> Option<Ellipse> {
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    Some(Ellipse {
        shape: extract_filled_shape(args),
        extent,
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
        start_angle: named_arg(args, "startAngle")
            .and_then(extract_number)
            .unwrap_or(0.0),
        end_angle: named_arg(args, "endAngle")
            .and_then(extract_number)
            .unwrap_or(360.0),
        closure: named_arg(args, "closure")
            .and_then(extract_enum_ident)
            .and_then(|s| EllipseClosure::from_ident(&s))
            .unwrap_or_default(),
    })
}

fn extract_bitmap(args: &[Expression]) -> Option<Bitmap> {
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    Some(Bitmap {
        extent,
        filename: named_arg(args, "fileName").and_then(crate::ast_extract::string_literal_value),
        image_source: named_arg(args, "imageSource")
            .and_then(crate::ast_extract::string_literal_value),
        origin: named_arg(args, "origin")
            .and_then(extract_point)
            .unwrap_or(Point { x: 0.0, y: 0.0 }),
        rotation: named_arg(args, "rotation")
            .and_then(extract_number)
            .unwrap_or(0.0),
    })
}

fn extract_filled_shape(args: &[Expression]) -> FilledShape {
    FilledShape {
        line_color: named_arg(args, "lineColor").and_then(extract_color),
        fill_color: named_arg(args, "fillColor").and_then(extract_color),
        line_pattern: named_arg(args, "linePattern")
            .and_then(extract_enum_ident)
            .and_then(|s| LinePattern::from_ident(&s))
            .unwrap_or_default(),
        fill_pattern: named_arg(args, "fillPattern")
            .and_then(extract_enum_ident)
            .and_then(|s| FillPattern::from_ident(&s))
            .unwrap_or_default(),
        line_thickness: named_arg(args, "lineThickness")
            .and_then(extract_number)
            .unwrap_or(0.25),
    }
}

// ---------------------------------------------------------------------------
// Low-level expression walkers
// ---------------------------------------------------------------------------

fn find_call<'a>(exprs: &'a [Expression], name: &str) -> Option<&'a Expression> {
    exprs.iter().find(|e| call_name(e) == Some(name))
}

fn call_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::ClassModification { target, .. } => {
            target.parts.last().map(|t| t.ident.text.as_ref())
        }
        Expression::FunctionCall { comp, .. } => comp.parts.last().map(|t| t.ident.text.as_ref()),
        _ => None,
    }
}

fn call_args(expr: &Expression) -> Option<&[Expression]> {
    match expr {
        Expression::ClassModification { modifications, .. } => Some(modifications),
        Expression::FunctionCall { args, .. } => Some(args),
        _ => None,
    }
}

fn named_arg<'a>(args: &'a [Expression], name: &str) -> Option<&'a Expression> {
    args.iter().find_map(|e| match e {
        Expression::Modification { target, value, .. } => {
            (target.parts.last().map(|t| t.ident.text.as_ref()) == Some(name))
                .then_some(value.as_ref())
        }
        Expression::NamedArgument {
            name: arg_name,
            value,
            ..
        } => (arg_name.text.as_ref() == name).then_some(value),
        _ => None,
    })
}

fn extract_extent(expr: &Expression) -> Option<Extent> {
    let elements = array_elements(expr)?;
    if elements.len() != 2 {
        return None;
    }
    let p1 = extract_point(&elements[0])?;
    let p2 = extract_point(&elements[1])?;
    Some(Extent { p1, p2 })
}

fn extract_point(expr: &Expression) -> Option<Point> {
    let elements = array_elements(expr)?;
    if elements.len() != 2 {
        return None;
    }
    let x = extract_number(&elements[0])?;
    let y = extract_number(&elements[1])?;
    Some(Point { x, y })
}

fn extract_point_array(expr: &Expression) -> Option<Vec<Point>> {
    let elements = array_elements(expr)?;
    elements.iter().map(extract_point).collect()
}

fn extract_color(expr: &Expression) -> Option<Color> {
    let elements = array_elements(expr)?;
    if elements.len() != 3 {
        return None;
    }
    let r = extract_number(&elements[0])? as u8;
    let g = extract_number(&elements[1])? as u8;
    let b = extract_number(&elements[2])? as u8;
    Some(Color { r, g, b })
}

/// Numeric literal (with optional unary minus). Delegates to the
/// canonical `lunco_modelica_ast::ast_extract::numeric_of` so the Terminal/Unary decoding
/// lives in one place.
fn extract_number(expr: &Expression) -> Option<f64> {
    crate::ast_extract::numeric_of(expr)
}

fn extract_enum_ident(expr: &Expression) -> Option<String> {
    match expr {
        Expression::ComponentReference(cref) => {
            let s = cref
                .parts
                .iter()
                .map(|p| p.ident.text.as_ref())
                .collect::<Vec<_>>()
                .join(".");
            Some(s)
        }
        _ => None,
    }
}

fn array_elements(expr: &Expression) -> Option<&[Expression]> {
    match expr {
        Expression::Array { elements, .. } => Some(elements),
        _ => None,
    }
}

fn expr_to_dyn(expr: &Expression) -> Option<DynExpr> {
    match expr {
        Expression::Terminal {
            terminal_type,
            token,
            ..
        } => match terminal_type {
            TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                token.text.parse().ok().map(DynExpr::Const)
            }
            TerminalType::String => Some(DynExpr::StringLit(token.text.to_string())),
            _ => None,
        },
        Expression::ComponentReference(cref) => {
            // Runtime signal identities retain every array subscript. Dropping
            // them makes distinct valves resolve to the same nonexistent scalar.
            Some(DynExpr::Var(cref.to_string()))
        }
        Expression::Unary {
            op: OpUnary::Minus,
            rhs,
            ..
        } => expr_to_dyn(rhs).map(|e| DynExpr::Neg(Box::new(e))),
        Expression::Unary { .. } => None,
        Expression::Binary { op, lhs, rhs, .. } => {
            let l = expr_to_dyn(lhs)?;
            let r = expr_to_dyn(rhs)?;
            match op {
                OpBinary::Add => Some(DynExpr::Add(Box::new(l), Box::new(r))),
                OpBinary::Sub => Some(DynExpr::Sub(Box::new(l), Box::new(r))),
                OpBinary::Mul => Some(DynExpr::Mul(Box::new(l), Box::new(r))),
                OpBinary::Div => Some(DynExpr::Div(Box::new(l), Box::new(r))),
                _ => None,
            }
        }
        Expression::FunctionCall { comp, args, .. } => {
            let n = comp.parts.last()?.ident.text.as_ref();
            if n == "String" && args.len() == 1 {
                expr_to_dyn(&args[0]).map(|e| DynExpr::StringCall(Box::new(e)))
            } else {
                None
            }
        }
        Expression::Parenthesized { inner, .. } => expr_to_dyn(inner),
        _ => None,
    }
}
