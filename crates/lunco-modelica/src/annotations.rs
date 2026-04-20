//! Typed extractors for Modelica graphical annotations.
//!
//! Rumoca preserves `annotation(...)` clauses on every class/component as a
//! `Vec<Expression>`, but does not decompose them into typed nodes. This
//! module walks those raw expressions and produces typed structs for the
//! subset of MLS Annex D we actually render: `Placement`, and the four most
//! common `Icon`/`Diagram` graphics primitives — `Rectangle`, `Line`,
//! `Polygon`, `Text`.
//!
//! ## Shape of an annotation expression
//!
//! `annotation(Placement(transformation(extent={{-10,-10},{10,10}})))` lands
//! in the AST as a class-modification tree. The arguments inside an
//! annotation can appear as either `Expression::ClassModification` or
//! `Expression::FunctionCall` (rumoca notes they are syntactically
//! identical), and named arguments can appear as either
//! `Expression::Modification { target, value }` or
//! `Expression::NamedArgument { name, value }`. The helpers below normalize
//! both forms so call sites do not have to.
//!
//! ## Scope of slice 1
//!
//! - Reads the class's *own* annotation only (no `extends` graphics merge).
//! - Color resolution is literal `{r,g,b}` only; named/`DynamicSelect`
//!   colors fall through to `None`.
//! - `Ellipse` and `Bitmap` are not yet emitted; they will be added in a
//!   later slice once the rendering path is in place.

use rumoca_session::parsing::ast::{Expression, OpUnary, TerminalType};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Common types
// ---------------------------------------------------------------------------

/// 2D point in Modelica diagram coordinates (millimetres, Y-up).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Axis-aligned bounding box in Modelica diagram coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    pub p1: Point,
    pub p2: Point,
}

/// RGB colour as 0..=255 components (matches Modelica `lineColor` / `fillColor` arrays).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// MLS Annex D `FillPattern` enumeration (subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillPattern {
    #[default]
    None,
    Solid,
    Horizontal,
    Vertical,
    Cross,
    Forward,
    Backward,
    CrossDiag,
    HorizontalCylinder,
    VerticalCylinder,
    Sphere,
}

impl FillPattern {
    fn from_ident(ident: &str) -> Option<Self> {
        // Accepts both bare `Solid` and `FillPattern.Solid` tail-matched.
        let tail = ident.rsplit('.').next().unwrap_or(ident);
        Some(match tail {
            "None" => Self::None,
            "Solid" => Self::Solid,
            "Horizontal" => Self::Horizontal,
            "Vertical" => Self::Vertical,
            "Cross" => Self::Cross,
            "Forward" => Self::Forward,
            "Backward" => Self::Backward,
            "CrossDiag" => Self::CrossDiag,
            "HorizontalCylinder" => Self::HorizontalCylinder,
            "VerticalCylinder" => Self::VerticalCylinder,
            "Sphere" => Self::Sphere,
            _ => return None,
        })
    }
}

/// MLS Annex D `LinePattern` enumeration (subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LinePattern {
    None,
    #[default]
    Solid,
    Dash,
    Dot,
    DashDot,
    DashDotDot,
}

impl LinePattern {
    fn from_ident(ident: &str) -> Option<Self> {
        let tail = ident.rsplit('.').next().unwrap_or(ident);
        Some(match tail {
            "None" => Self::None,
            "Solid" => Self::Solid,
            "Dash" => Self::Dash,
            "Dot" => Self::Dot,
            "DashDot" => Self::DashDot,
            "DashDotDot" => Self::DashDotDot,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// Decoded `Placement(transformation(...), [iconTransformation(...)])` annotation.
///
/// Slice 1 only carries the diagram-level `transformation`. The icon-level
/// transformation (used for inner connector placement) will land later
/// alongside connector rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub transformation: Transformation,
}

/// `transformation(extent=..., origin=..., rotation=...)` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transformation {
    pub extent: Extent,
    /// Defaults to (0, 0) per MLS Annex D when not given.
    pub origin: Point,
    /// Degrees CCW. Defaults to 0.
    pub rotation: f64,
}

// ---------------------------------------------------------------------------
// Icon / Diagram / Graphics
// ---------------------------------------------------------------------------

/// Coordinate system for an Icon or Diagram layer.
///
/// Defaults to `extent={{-100,-100},{100,100}}` per MLS Annex D.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoordinateSystem {
    pub extent: Extent,
}

impl Default for CoordinateSystem {
    fn default() -> Self {
        Self {
            extent: Extent {
                p1: Point { x: -100.0, y: -100.0 },
                p2: Point { x: 100.0, y: 100.0 },
            },
        }
    }
}

/// Decoded `Icon(coordinateSystem=..., graphics={...})` annotation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Icon {
    pub coordinate_system: CoordinateSystem,
    pub graphics: Vec<GraphicItem>,
}

/// Decoded `Diagram(coordinateSystem=..., graphics={...})` annotation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Diagram {
    pub coordinate_system: CoordinateSystem,
    pub graphics: Vec<GraphicItem>,
}

/// One graphics primitive from an Icon/Diagram `graphics={...}` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GraphicItem {
    Rectangle(Rectangle),
    Line(Line),
    Polygon(Polygon),
    Text(Text),
}

/// Properties common to filled shapes (rectangles, polygons).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FilledShape {
    pub line_color: Option<Color>,
    pub fill_color: Option<Color>,
    pub line_pattern: LinePattern,
    pub fill_pattern: FillPattern,
    pub line_thickness: f64, // mm; defaults to 0.25 per MLS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rectangle {
    pub shape: FilledShape,
    pub extent: Extent,
    pub origin: Point,
    pub rotation: f64,
    pub radius: f64, // corner radius, mm
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub points: Vec<Point>,
    pub color: Option<Color>,
    pub pattern: LinePattern,
    pub thickness: f64,
    pub origin: Point,
    pub rotation: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub shape: FilledShape,
    pub points: Vec<Point>,
    pub origin: Point,
    pub rotation: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub extent: Extent,
    pub text_string: String,
    pub font_size: f64, // 0 = auto-fit
    pub text_color: Option<Color>,
    pub origin: Point,
    pub rotation: f64,
}

// ---------------------------------------------------------------------------
// Public extractors
// ---------------------------------------------------------------------------

/// Extract the `Placement(...)` annotation from a component's annotation list.
pub fn extract_placement(annotations: &[Expression]) -> Option<Placement> {
    let placement_call = find_call(annotations, "Placement")?;
    let placement_args = call_args(placement_call)?;
    let transformation = find_call(placement_args, "transformation")
        .and_then(extract_transformation)?;
    Some(Placement { transformation })
}

/// Extract the `Icon(...)` annotation from a class's annotation list.
pub fn extract_icon(annotations: &[Expression]) -> Option<Icon> {
    let icon_call = find_call(annotations, "Icon")?;
    let icon_args = call_args(icon_call)?;
    Some(Icon {
        coordinate_system: extract_coordinate_system(icon_args).unwrap_or_default(),
        graphics: extract_graphics(icon_args),
    })
}

/// Extract the `Diagram(...)` annotation from a class's annotation list.
pub fn extract_diagram(annotations: &[Expression]) -> Option<Diagram> {
    let diagram_call = find_call(annotations, "Diagram")?;
    let diagram_args = call_args(diagram_call)?;
    Some(Diagram {
        coordinate_system: extract_coordinate_system(diagram_args).unwrap_or_default(),
        graphics: extract_graphics(diagram_args),
    })
}

// ---------------------------------------------------------------------------
// Inner extractors
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
    Some(Transformation { extent, origin, rotation })
}

fn extract_coordinate_system(args: &[Expression]) -> Option<CoordinateSystem> {
    let cs_call = find_call(args, "coordinateSystem")?;
    let cs_args = call_args(cs_call)?;
    let extent = named_arg(cs_args, "extent").and_then(extract_extent)?;
    Some(CoordinateSystem { extent })
}

fn extract_graphics(args: &[Expression]) -> Vec<GraphicItem> {
    let Some(graphics_array) = named_arg(args, "graphics") else {
        return Vec::new();
    };
    let Some(elements) = array_elements(graphics_array) else {
        return Vec::new();
    };
    elements.iter().filter_map(extract_graphic_item).collect()
}

fn extract_graphic_item(expr: &Expression) -> Option<GraphicItem> {
    let name = call_name(expr)?;
    let args = call_args(expr)?;
    match name {
        "Rectangle" => Some(GraphicItem::Rectangle(extract_rectangle(args)?)),
        "Line" => Some(GraphicItem::Line(extract_line(args)?)),
        "Polygon" => Some(GraphicItem::Polygon(extract_polygon(args)?)),
        "Text" => Some(GraphicItem::Text(extract_text(args)?)),
        // Ellipse/Bitmap intentionally skipped in slice 1.
        _ => None,
    }
}

fn extract_rectangle(args: &[Expression]) -> Option<Rectangle> {
    let extent = named_arg(args, "extent").and_then(extract_extent)?;
    Some(Rectangle {
        shape: extract_filled_shape(args),
        extent,
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

fn extract_line(args: &[Expression]) -> Option<Line> {
    let points = named_arg(args, "points").and_then(extract_point_array)?;
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
    let text_string = named_arg(args, "textString")
        .and_then(extract_string)
        .unwrap_or_default();
    Some(Text {
        extent,
        text_string,
        font_size: named_arg(args, "fontSize")
            .and_then(extract_number)
            .unwrap_or(0.0),
        text_color: named_arg(args, "textColor")
            .or_else(|| named_arg(args, "lineColor"))
            .and_then(extract_color),
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
        line_pattern: named_arg(args, "pattern")
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
// Generic Expression helpers
// ---------------------------------------------------------------------------

/// Name of a call-shaped expression — accepts both `FunctionCall` and the
/// syntactically identical `ClassModification`.
fn call_name(expr: &Expression) -> Option<&str> {
    let comp = match expr {
        Expression::FunctionCall { comp, .. } => comp,
        Expression::ClassModification { target, .. } => target,
        _ => return None,
    };
    let part = comp.parts.first()?;
    Some(part.ident.text.as_ref())
}

fn call_args(expr: &Expression) -> Option<&[Expression]> {
    match expr {
        Expression::FunctionCall { args, .. } => Some(args.as_slice()),
        Expression::ClassModification { modifications, .. } => Some(modifications.as_slice()),
        _ => None,
    }
}

/// Find the first call-shaped expression in `list` whose head name matches
/// `name`.
fn find_call<'a>(list: &'a [Expression], name: &str) -> Option<&'a Expression> {
    list.iter().find(|e| call_name(e) == Some(name))
}

/// Find the value of a named argument inside an argument list. Accepts both
/// `NamedArgument { name, value }` (function-call form) and
/// `Modification { target, value }` (class-modification form).
fn named_arg<'a>(list: &'a [Expression], name: &str) -> Option<&'a Expression> {
    for expr in list {
        match expr {
            Expression::NamedArgument { name: n, value } if n.text.as_ref() == name => {
                return Some(value.as_ref());
            }
            Expression::Modification { target, value }
                if target
                    .parts
                    .first()
                    .map(|p| p.ident.text.as_ref() == name)
                    .unwrap_or(false) =>
            {
                return Some(value.as_ref());
            }
            _ => {}
        }
    }
    None
}

/// Strip a `Parenthesized` wrapper if present.
fn unwrap_paren(expr: &Expression) -> &Expression {
    match expr {
        Expression::Parenthesized { inner } => unwrap_paren(inner.as_ref()),
        other => other,
    }
}

fn array_elements(expr: &Expression) -> Option<&[Expression]> {
    match unwrap_paren(expr) {
        Expression::Array { elements, .. } => Some(elements.as_slice()),
        _ => None,
    }
}

fn extract_number(expr: &Expression) -> Option<f64> {
    match unwrap_paren(expr) {
        Expression::Terminal { terminal_type, token } => match terminal_type {
            TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                token.text.parse::<f64>().ok()
            }
            _ => None,
        },
        Expression::Unary { op, rhs } => {
            let v = extract_number(rhs.as_ref())?;
            match op {
                OpUnary::Minus(_) => Some(-v),
                OpUnary::Plus(_) => Some(v),
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_string(expr: &Expression) -> Option<String> {
    match unwrap_paren(expr) {
        Expression::Terminal { terminal_type: TerminalType::String, token } => {
            Some(token.text.as_ref().to_string())
        }
        _ => None,
    }
}

/// Extract a single Modelica enumeration identifier. Accepts component
/// references like `Solid`, `FillPattern.Solid`, or
/// `Modelica.Mechanics.Rotational.Types.Init.Free` — only the leaf identifier
/// matters for the `from_ident` mappers above.
fn extract_enum_ident(expr: &Expression) -> Option<String> {
    match unwrap_paren(expr) {
        Expression::ComponentReference(comp) => {
            let parts: Vec<&str> = comp.parts.iter().map(|p| p.ident.text.as_ref()).collect();
            Some(parts.join("."))
        }
        _ => None,
    }
}

fn extract_point(expr: &Expression) -> Option<Point> {
    let elems = array_elements(expr)?;
    if elems.len() != 2 {
        return None;
    }
    Some(Point {
        x: extract_number(&elems[0])?,
        y: extract_number(&elems[1])?,
    })
}

fn extract_extent(expr: &Expression) -> Option<Extent> {
    let outer = array_elements(expr)?;
    if outer.len() != 2 {
        return None;
    }
    Some(Extent {
        p1: extract_point(&outer[0])?,
        p2: extract_point(&outer[1])?,
    })
}

fn extract_point_array(expr: &Expression) -> Option<Vec<Point>> {
    let elems = array_elements(expr)?;
    elems.iter().map(extract_point).collect()
}

fn extract_color(expr: &Expression) -> Option<Color> {
    let elems = array_elements(expr)?;
    if elems.len() != 3 {
        return None;
    }
    let to_u8 = |e: &Expression| extract_number(e).map(|v| v.clamp(0.0, 255.0) as u8);
    Some(Color {
        r: to_u8(&elems[0])?,
        g: to_u8(&elems[1])?,
        b: to_u8(&elems[2])?,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rumoca_phase_parse::parse_to_ast;

    /// Parse a tiny Modelica source and return the annotation expressions
    /// attached to its single class.
    fn class_annotations(source: &str) -> Vec<Expression> {
        let ast = parse_to_ast(source, "test.mo").expect("parse");
        let (_name, class) = ast.classes.iter().next().expect("one class");
        class.annotation.clone()
    }

    /// Same, but for the first component of the first class.
    fn component_annotations(source: &str) -> Vec<Expression> {
        let ast = parse_to_ast(source, "test.mo").expect("parse");
        let (_name, class) = ast.classes.iter().next().expect("one class");
        let (_cname, comp) = class.components.iter().next().expect("one component");
        comp.annotation.clone()
    }

    #[test]
    fn placement_simple_extent() {
        let src = r#"
model M
  Real x annotation(Placement(transformation(extent={{-10,-10},{10,10}})));
end M;
"#;
        let p = extract_placement(&component_annotations(src)).expect("placement");
        assert_eq!(p.transformation.extent.p1, Point { x: -10.0, y: -10.0 });
        assert_eq!(p.transformation.extent.p2, Point { x: 10.0, y: 10.0 });
        assert_eq!(p.transformation.origin, Point { x: 0.0, y: 0.0 });
        assert_eq!(p.transformation.rotation, 0.0);
    }

    #[test]
    fn placement_with_origin_and_rotation() {
        let src = r#"
model M
  Real x annotation(Placement(transformation(
    extent={{-10,-10},{10,10}}, origin={50,-20}, rotation=90)));
end M;
"#;
        let p = extract_placement(&component_annotations(src)).expect("placement");
        assert_eq!(p.transformation.origin, Point { x: 50.0, y: -20.0 });
        assert_eq!(p.transformation.rotation, 90.0);
    }

    #[test]
    fn icon_with_rectangle_and_text() {
        let src = r#"
model M
  annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
    graphics={
      Rectangle(extent={{-80,40},{80,-40}},
                lineColor={0,0,255}, fillColor={255,255,0}, fillPattern=FillPattern.Solid),
      Text(extent={{-60,20},{60,-20}}, textString="hello", textColor={255,0,0})
    }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        assert_eq!(icon.coordinate_system.extent.p1, Point { x: -100.0, y: -100.0 });
        assert_eq!(icon.graphics.len(), 2);

        match &icon.graphics[0] {
            GraphicItem::Rectangle(r) => {
                assert_eq!(r.extent.p1, Point { x: -80.0, y: 40.0 });
                assert_eq!(r.shape.line_color, Some(Color { r: 0, g: 0, b: 255 }));
                assert_eq!(r.shape.fill_color, Some(Color { r: 255, g: 255, b: 0 }));
                assert_eq!(r.shape.fill_pattern, FillPattern::Solid);
            }
            other => panic!("expected Rectangle, got {other:?}"),
        }
        match &icon.graphics[1] {
            GraphicItem::Text(t) => {
                assert_eq!(t.text_string, "hello");
                assert_eq!(t.text_color, Some(Color { r: 255, g: 0, b: 0 }));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn icon_with_line_and_polygon() {
        let src = r#"
model M
  annotation(Icon(graphics={
    Line(points={{-100,0},{100,0}}, color={0,0,0}, thickness=0.5),
    Polygon(points={{0,50},{50,-50},{-50,-50}},
            lineColor={0,0,0}, fillColor={128,128,128}, fillPattern=FillPattern.Solid)
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        // No coordinateSystem given → default.
        assert_eq!(icon.coordinate_system, CoordinateSystem::default());
        assert_eq!(icon.graphics.len(), 2);
        match &icon.graphics[0] {
            GraphicItem::Line(l) => {
                assert_eq!(l.points.len(), 2);
                assert_eq!(l.points[0], Point { x: -100.0, y: 0.0 });
                assert_eq!(l.thickness, 0.5);
                assert_eq!(l.color, Some(Color { r: 0, g: 0, b: 0 }));
            }
            other => panic!("expected Line, got {other:?}"),
        }
        match &icon.graphics[1] {
            GraphicItem::Polygon(p) => {
                assert_eq!(p.points.len(), 3);
                assert_eq!(p.shape.fill_color, Some(Color { r: 128, g: 128, b: 128 }));
            }
            other => panic!("expected Polygon, got {other:?}"),
        }
    }

    #[test]
    fn diagram_extraction() {
        let src = r#"
model M
  annotation(Diagram(coordinateSystem(extent={{-50,-50},{50,50}}),
    graphics={Rectangle(extent={{-10,-10},{10,10}})}));
end M;
"#;
        let d = extract_diagram(&class_annotations(src)).expect("diagram");
        assert_eq!(d.coordinate_system.extent.p1, Point { x: -50.0, y: -50.0 });
        assert_eq!(d.graphics.len(), 1);
    }

    #[test]
    fn missing_annotations_return_none() {
        let src = r#"
model M
  Real x;
end M;
"#;
        assert!(extract_placement(&component_annotations(src)).is_none());
        assert!(extract_icon(&class_annotations(src)).is_none());
        assert!(extract_diagram(&class_annotations(src)).is_none());
    }

    #[test]
    fn unknown_graphic_kinds_are_skipped() {
        // Ellipse and Bitmap are not yet implemented — they should be
        // silently dropped, not cause the Icon to fail parsing.
        let src = r#"
model M
  annotation(Icon(graphics={
    Ellipse(extent={{-10,-10},{10,10}}),
    Rectangle(extent={{-5,-5},{5,5}}),
    Bitmap(extent={{-20,-20},{20,20}}, fileName="x.png")
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        assert_eq!(icon.graphics.len(), 1);
        assert!(matches!(icon.graphics[0], GraphicItem::Rectangle(_)));
    }
}
