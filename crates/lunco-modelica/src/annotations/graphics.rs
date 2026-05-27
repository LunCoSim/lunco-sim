//! Graphics primitives for Modelica Icons and Diagrams.

use rumoca_compile::parsing::ast::Expression;
use serde::{Deserialize, Serialize};
use super::types::{Point, Extent, Color, LinePattern, FilledShape, Arrow, EllipseClosure};

/// One graphics primitive from an Icon/Diagram `graphics={...}` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GraphicItem {
    Rectangle(Rectangle),
    Line(Line),
    Polygon(Polygon),
    Text(Text),
    Ellipse(Ellipse),
    Bitmap(Bitmap),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rectangle {
    pub shape: FilledShape,
    pub extent: Extent,
    pub origin: Point,
    pub rotation: f64,
    pub radius: f64, // corner radius, mm
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent_dynamic: Option<DynExtent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub points: Vec<Point>,
    pub color: Option<Color>,
    pub pattern: LinePattern,
    pub thickness: f64,
    pub origin: Point,
    pub rotation: f64,
    pub arrow: [Arrow; 2],
    pub arrow_size: f64,
    pub smooth_bezier: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_string_dynamic: Option<DynExpr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ellipse {
    pub shape: FilledShape,
    pub extent: Extent,
    pub origin: Point,
    pub rotation: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub closure: EllipseClosure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bitmap {
    pub extent: Extent,
    pub filename: Option<String>,
    pub image_source: Option<String>,
    pub origin: Point,
    pub rotation: f64,
}

/// LunCo vendor annotation: embedded plot tile bound to a runtime signal.
/// Lives in `annotation(__LunCo(plotNodes={LunCoAnnotations.PlotNode(...)}))`,
/// alongside (not inside) `Diagram(graphics=...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LunCoPlotNode {
    pub extent: Extent,
    pub signal: String,
    pub title: String,
}

/// True when `expr` is a `LunCoAnnotations.PlotNode(...)` (or bare
/// `PlotNode(...)`, the `import LunCoAnnotations.*` form) record
/// reference as found inside `__LunCo(plotNodes={...})`. Used by
/// both the read-side parser and the write-side AST mutators.
pub fn is_plot_node_record_call(expr: &Expression) -> bool {
    let parts = match expr {
        Expression::FunctionCall { comp, .. } => &comp.parts,
        Expression::ClassModification { target, .. } => &target.parts,
        _ => return false,
    };
    parts.last().map(|t| &*t.ident.text) == Some("PlotNode")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DynExpr {
    Const(f64),
    StringLit(String),
    Var(String),
    Neg(Box<DynExpr>),
    Add(Box<DynExpr>, Box<DynExpr>),
    Sub(Box<DynExpr>, Box<DynExpr>),
    Mul(Box<DynExpr>, Box<DynExpr>),
    Div(Box<DynExpr>, Box<DynExpr>),
    StringCall(Box<DynExpr>),
}

#[derive(Debug, Clone)]
pub enum DynValue {
    Number(f64),
    Text(String),
}

impl DynValue {
    pub fn to_display(&self) -> String {
        match self {
            DynValue::Number(n) => {
                let s = format!("{n:.2}");
                let trimmed = s.trim_end_matches('0').trim_end_matches('.');
                trimmed.to_string()
            }
            DynValue::Text(s) => s.clone(),
        }
    }
}

impl DynExpr {
    pub fn eval(&self, resolve: &dyn Fn(&str) -> Option<f64>) -> Option<DynValue> {
        match self {
            DynExpr::Const(v) => Some(DynValue::Number(*v)),
            DynExpr::StringLit(s) => Some(DynValue::Text(s.clone())),
            DynExpr::Var(name) => resolve(name).map(DynValue::Number),
            DynExpr::Neg(x) => match x.eval(resolve)? {
                DynValue::Number(n) => Some(DynValue::Number(-n)),
                DynValue::Text(_) => None,
            },
            DynExpr::Add(a, b) => {
                let av = a.eval(resolve)?;
                let bv = b.eval(resolve)?;
                match (&av, &bv) {
                    (DynValue::Number(x), DynValue::Number(y)) => Some(DynValue::Number(x + y)),
                    _ => Some(DynValue::Text(format!("{}{}", av.to_display(), bv.to_display()))),
                }
            }
            DynExpr::Sub(a, b) => match (a.eval(resolve)?, b.eval(resolve)?) {
                (DynValue::Number(x), DynValue::Number(y)) => Some(DynValue::Number(x - y)),
                _ => None,
            },
            DynExpr::Mul(a, b) => match (a.eval(resolve)?, b.eval(resolve)?) {
                (DynValue::Number(x), DynValue::Number(y)) => Some(DynValue::Number(x * y)),
                _ => None,
            },
            DynExpr::Div(a, b) => match (a.eval(resolve)?, b.eval(resolve)?) {
                (DynValue::Number(x), DynValue::Number(y)) => {
                    if y == 0.0 { None } else { Some(DynValue::Number(x / y)) }
                }
                _ => None,
            },
            DynExpr::StringCall(x) => Some(DynValue::Text(x.eval(resolve)?.to_display())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynExtent {
    pub x1: DynExpr,
    pub y1: DynExpr,
    pub x2: DynExpr,
    pub y2: DynExpr,
}

impl DynExtent {
    pub fn eval(&self, resolve: &dyn Fn(&str) -> Option<f64>) -> Option<Extent> {
        let to_num = |dv: DynValue| match dv {
            DynValue::Number(n) => Some(n),
            DynValue::Text(_) => None,
        };
        let x1 = to_num(self.x1.eval(resolve)?)?;
        let y1 = to_num(self.y1.eval(resolve)?)?;
        let x2 = to_num(self.x2.eval(resolve)?)?;
        let y2 = to_num(self.y2.eval(resolve)?)?;
        Some(Extent {
            p1: Point { x: x1, y: y1 },
            p2: Point { x: x2, y: y2 },
        })
    }
}

pub struct LineRoute {
    pub points: Vec<(f32, f32)>,
    pub smooth_bezier: bool,
    pub color: Option<[u8; 3]>,
    pub thickness: Option<f32>,
}
