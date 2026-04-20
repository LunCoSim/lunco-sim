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
//! ## Scope
//!
//! - Reads the class's *own* annotation only (no `extends` graphics merge).
//! - Color resolution is literal `{r,g,b}` only; named/`DynamicSelect`
//!   colors fall through to `None`.
//! - Full MLS Annex D primitive set: `Rectangle`, `Line`, `Polygon`,
//!   `Text`, `Ellipse`, `Bitmap`. `Line` now carries `arrow[0..1]` and
//!   `arrowSize`. `Ellipse` arcs (startAngle/endAngle/closure) are
//!   parsed; renderer currently fills a full ellipse and ignores arc
//!   bounds (follow-up slice).

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
    Ellipse(Ellipse),
    Bitmap(Bitmap),
}

/// MLS Annex D `Arrow` enum — line endcap style. Default `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Arrow {
    #[default]
    None,
    Open,
    Filled,
    Half,
}

impl Arrow {
    fn from_ident(s: &str) -> Option<Self> {
        // Accept both bare leaf ("Filled") and dotted form
        // ("Arrow.Filled") — Modelica source can use either depending
        // on whether the enum is used inside its declaring type.
        let leaf = s.rsplit('.').next().unwrap_or(s);
        match leaf {
            "None" => Some(Self::None),
            "Open" => Some(Self::Open),
            "Filled" => Some(Self::Filled),
            "Half" => Some(Self::Half),
            _ => None,
        }
    }
}

/// MLS Annex D `EllipseClosure` enum — how a partial ellipse arc is
/// closed. Default `Chord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EllipseClosure {
    None,
    #[default]
    Chord,
    Radial,
}

impl EllipseClosure {
    fn from_ident(s: &str) -> Option<Self> {
        let leaf = s.rsplit('.').next().unwrap_or(s);
        match leaf {
            "None" => Some(Self::None),
            "Chord" => Some(Self::Chord),
            "Radial" => Some(Self::Radial),
            _ => None,
        }
    }
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
    /// Arrow style at the line's start (`arrow[0]`) and end
    /// (`arrow[1]`). Defaults `(None, None)`.
    pub arrow: [Arrow; 2],
    /// Arrow-head length in diagram units. Default 3.0 per MLS.
    pub arrow_size: f64,
    /// Smooth line interpolation flag (`smooth=Smooth.Bezier`). When
    /// true, the renderer may draw a cubic-Bezier through the points
    /// instead of straight segments. Currently parsed only — the
    /// renderer falls back to polyline; smooth rendering is a
    /// follow-up. Kept in the AST so Save-As round-trips don't lose
    /// the attribute.
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
}

/// MLS Annex D `Ellipse` primitive — a filled-shape ellipse fitted to
/// `extent`. `start_angle` / `end_angle` define an arc; `closure`
/// selects how a partial arc is closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ellipse {
    pub shape: FilledShape,
    pub extent: Extent,
    pub origin: Point,
    pub rotation: f64,
    /// Arc start angle (degrees, CCW). Default 0.
    pub start_angle: f64,
    /// Arc end angle (degrees, CCW). Default 360 (full ellipse).
    pub end_angle: f64,
    /// How a partial arc is closed.
    pub closure: EllipseClosure,
}

/// MLS Annex D `Bitmap` primitive — an embedded raster image.
///
/// Exactly one of `filename` (modelica:// or file path URI) and
/// `image_source` (base64-encoded raw bytes) is typically set; MLS
/// allows both but filename takes precedence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bitmap {
    pub extent: Extent,
    /// Resource URI (`modelica://Package.Name/path/to/img.png`) or raw
    /// filesystem path. The renderer resolves `modelica://` to the
    /// package's on-disk directory via `lunco_assets::msl_dir`.
    pub filename: Option<String>,
    /// Base64-encoded raw image bytes. When present and `filename` is
    /// absent, used as the image source. Supported formats: whatever
    /// the `image` crate decodes.
    pub image_source: Option<String>,
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

/// Build the ordered list of qualified-name candidates to try when
/// resolving an `extends` clause written in the source of `class_name`.
///
/// Implements a simplified version of MLS § 5 lookup: the bare name
/// is rewritten against every enclosing package from innermost to
/// outermost, then the bare name itself is the last fallback. Dotted
/// names are treated similarly — we try them verbatim first (their
/// prefix might already be a full qualified path), then apply the
/// scope chain using just the tail segment.
///
/// Example — `class_name =
/// "Modelica.Clocked.ClockSignals.Clocks.Logical.ConjunctiveClock"`,
/// `base_name = "PartialLogicalClock"` yields:
///
/// 1. `PartialLogicalClock` (as given)
/// 2. `Modelica.Clocked.ClockSignals.Clocks.Logical.PartialLogicalClock`
/// 3. `Modelica.Clocked.ClockSignals.Clocks.PartialLogicalClock`
/// 4. `Modelica.Clocked.ClockSignals.PartialLogicalClock`
/// 5. `Modelica.Clocked.PartialLogicalClock`
/// 6. `Modelica.PartialLogicalClock`
fn build_extends_candidates(
    class_name: &str,
    base_name: &str,
    imports: &[rumoca_session::parsing::ast::Import],
) -> Vec<String> {
    let mut out = Vec::new();
    // 1. As-given.
    out.push(base_name.to_string());

    // 2. Import-alias expansion (MLS §13.2.1). Takes precedence over
    //    scope-chain because imports are explicit user intent. The
    //    head-segment of `base_name` is what imports bind against;
    //    the remaining tail is appended to the import's resolved
    //    target path.
    //
    //    Examples (head="SI", tail="Voltage" from `extends SI.Voltage`):
    //     - `import SI = Modelica.Units.SI;`     → Modelica.Units.SI.Voltage
    //     - `import Modelica.Units.SI;`          → Modelica.Units.SI.Voltage (qualified uses last segment)
    //     - `import Modelica.Units.SI.*;`        → Modelica.Units.SI.SI.Voltage — wrong; for `.*`, only bare (head-only) names match
    //     - `import Modelica.Units.SI.{Voltage};`→ Modelica.Units.SI.Voltage (if head="Voltage")
    let (head, tail) = match base_name.split_once('.') {
        Some((h, t)) => (h, Some(t)),
        None => (base_name, None),
    };
    for imp in imports {
        use rumoca_session::parsing::ast::Import;
        let import_path_name = |path: &rumoca_session::parsing::ast::Name| -> String {
            path.name
                .iter()
                .map(|t| t.text.as_ref())
                .collect::<Vec<_>>()
                .join(".")
        };
        match imp {
            Import::Renamed { alias, path, .. } => {
                if alias.text.as_ref() == head {
                    let resolved = import_path_name(path);
                    let full = match tail {
                        Some(t) => format!("{resolved}.{t}"),
                        None => resolved,
                    };
                    if full != base_name {
                        out.push(full);
                    }
                }
            }
            Import::Qualified { path, .. } => {
                let last = path
                    .name
                    .last()
                    .map(|t| t.text.as_ref())
                    .unwrap_or("");
                if last == head {
                    let resolved = import_path_name(path);
                    let full = match tail {
                        Some(t) => format!("{resolved}.{t}"),
                        None => resolved,
                    };
                    if full != base_name {
                        out.push(full);
                    }
                }
            }
            Import::Unqualified { path, .. } => {
                // `import A.B.*;` brings every name in `A.B` into the
                // local scope as a bare identifier. Only matches when
                // `base_name` has no dots (tail is None).
                if tail.is_none() {
                    let resolved = import_path_name(path);
                    let full = format!("{resolved}.{head}");
                    if full != base_name {
                        out.push(full);
                    }
                }
            }
            Import::Selective { path, names, .. } => {
                if names.iter().any(|n| n.text.as_ref() == head) {
                    let resolved = import_path_name(path);
                    let full = match tail {
                        Some(t) => format!("{resolved}.{head}.{t}"),
                        None => format!("{resolved}.{head}"),
                    };
                    if full != base_name {
                        out.push(full);
                    }
                }
            }
        }
    }

    // Parent packages of the class doing the extending, innermost
    // first. For `A.B.C.D.E` we want prefixes `A.B.C.D`, `A.B.C`,
    // `A.B`, `A`.
    let class_parts: Vec<&str> = class_name.split('.').collect();
    if class_parts.len() > 1 {
        // MLS §5.3.1: look up the *first* identifier of a dotted name
        // against each enclosing scope, then descend. For
        // `Interfaces.PartialClock` from inside
        // `Modelica.Clocked.ClockSignals.Clocks.Logical.X`, that
        // means trying `<pkg>.Interfaces.PartialClock` at every
        // enclosing package — preserving the full dotted tail, not
        // stripping to just the leaf.
        for stop in (1..class_parts.len()).rev() {
            let pkg = class_parts[..stop].join(".");
            let candidate = format!("{pkg}.{base_name}");
            if candidate != base_name {
                out.push(candidate);
            }
        }
    }
    out
}

/// Extract the `Icon(...)` annotation **merged with inherited graphics
/// from every `extends` base class**, per MLS Annex D.
///
/// Inheritance rules:
/// - Base classes' graphics render **first** (underneath the derived
///   class's graphics). The derived class's `Text(textString="%name")`
///   therefore overlays the inherited icon body — which is exactly
///   the layering MSL relies on for sensors, blocks, etc.
/// - The derived class's `coordinateSystem` wins when set; otherwise
///   it falls through to the deepest inherited one, else the default.
///
/// `resolver` is how the caller looks up an `extends` target by its
/// name string. The caller chooses whether that name is (a) bare as
/// written in source, (b) pre-qualified into a canonical dotted path,
/// or (c) resolved via a combined local-AST + class-cache walker.
/// Return `None` from the resolver for any class the caller can't
/// locate — the extractor silently skips that branch (partial merge
/// is better than no merge).
///
/// `visited` guards against cycles. The caller may pass an empty set;
/// the extractor will populate it. Reusable across sibling calls if
/// you want to detect cross-class cycles at workspace scope.
///
/// Returns `None` only if the class has no authored Icon AND no
/// inherited Icon. A class with an authored Icon but no base
/// classes still gets an `Icon { graphics: [...] }` exactly as
/// [`extract_icon`] would produce.
pub fn extract_icon_inherited<F>(
    class_name: &str,
    class: &rumoca_session::parsing::ast::ClassDef,
    resolver: &mut F,
    visited: &mut std::collections::HashSet<String>,
) -> Option<Icon>
where
    F: FnMut(&str)
        -> Option<std::sync::Arc<rumoca_session::parsing::ast::ClassDef>>,
{
    if !visited.insert(class_name.to_string()) {
        return None; // cycle
    }

    // Accumulate base-class graphics (front of the draw list) and pick
    // the deepest coordinate system along the way. We iterate extends
    // in source order so the final draw order matches "first-extended
    // base, then next base, then this class's own primitives".
    let mut merged_graphics: Vec<GraphicItem> = Vec::new();
    let mut inherited_cs: Option<CoordinateSystem> = None;
    for ext in &class.extends {
        let base_name: String = ext
            .base_name
            .name
            .iter()
            .map(|t| t.text.as_ref())
            .collect::<Vec<&str>>()
            .join(".");

        // Scope-chain lookup (MLS § 5): a bare `extends
        // PartialLogicalClock` is resolved against the enclosing
        // packages of the class doing the extending. Given
        // `Modelica.Clocked.ClockSignals.Clocks.Logical.ConjunctiveClock`
        // extending `PartialLogicalClock`, try:
        //
        //   1. Modelica.Clocked.ClockSignals.Clocks.Logical.PartialLogicalClock
        //   2. Modelica.Clocked.ClockSignals.Clocks.PartialLogicalClock
        //   3. Modelica.Clocked.ClockSignals.PartialLogicalClock
        //   …
        //   N. Modelica.PartialLogicalClock
        //   N+1. PartialLogicalClock (as given)
        //
        // First hit wins. Dotted `base_name`s (already qualified or
        // partially qualified) get the same treatment — we also try
        // the name as-written first so e.g. `Modelica.Icons.Partial`
        // is used verbatim before any scope-chain rewriting.
        let candidates = build_extends_candidates(class_name, &base_name, &class.imports);
        let mut hit: Option<(String, std::sync::Arc<rumoca_session::parsing::ast::ClassDef>)> = None;
        for candidate in candidates {
            if let Some(base_class) = resolver(&candidate) {
                hit = Some((candidate, base_class));
                break;
            }
        }
        let Some((resolved_name, base_class)) = hit else { continue };
        if let Some(base_icon) = extract_icon_inherited(
            &resolved_name,
            base_class.as_ref(),
            resolver,
            visited,
        ) {
            merged_graphics.extend(base_icon.graphics);
            inherited_cs = Some(base_icon.coordinate_system);
        }
    }

    // Append this class's own graphics on top.
    let local = extract_icon(&class.annotation);
    let local_cs = local.as_ref().map(|i| i.coordinate_system);
    if let Some(icon) = local {
        merged_graphics.extend(icon.graphics);
    }

    // Class has no authored Icon AND none inherited → None.
    if merged_graphics.is_empty() && local_cs.is_none() && inherited_cs.is_none() {
        return None;
    }

    Some(Icon {
        coordinate_system: local_cs
            .or(inherited_cs)
            .unwrap_or_default(),
        graphics: merged_graphics,
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
        "Ellipse" => Some(GraphicItem::Ellipse(extract_ellipse(args)?)),
        "Bitmap" => Some(GraphicItem::Bitmap(extract_bitmap(args)?)),
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
    // `arrow` is an `Arrow[2]` literal: `arrow={Arrow.None, Arrow.Filled}`.
    // Each element parses as a qualified enum ident; missing slots keep
    // the default `Arrow::None`.
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
        filename: named_arg(args, "fileName").and_then(extract_string),
        image_source: named_arg(args, "imageSource").and_then(extract_string),
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

    /// Parse a Modelica source and return the full AST. Used by the
    /// extends-inheritance tests that need to resolve parent classes.
    fn parse_source(source: &str) -> rumoca_session::parsing::ast::StoredDefinition {
        parse_to_ast(source, "test.mo").expect("parse")
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
        // Full MLS Annex D primitive set is now recognised: `Ellipse`
        // and `Bitmap` along with Rectangle/Line/Polygon/Text. Only
        // genuinely unknown primitive names (e.g. a typo like
        // `Rectangl` or a custom `MyShape`) are dropped — the Icon
        // keeps parsing, just without those entries.
        let src = r#"
model M
  annotation(Icon(graphics={
    Ellipse(extent={{-10,-10},{10,10}}),
    Rectangle(extent={{-5,-5},{5,5}}),
    Bitmap(extent={{-20,-20},{20,20}}, fileName="x.png"),
    MyShape(extent={{-30,-30},{30,30}})
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        assert_eq!(icon.graphics.len(), 3, "MyShape should be dropped");
        assert!(matches!(icon.graphics[0], GraphicItem::Ellipse(_)));
        assert!(matches!(icon.graphics[1], GraphicItem::Rectangle(_)));
        assert!(matches!(icon.graphics[2], GraphicItem::Bitmap(_)));
    }

    #[test]
    fn parses_line_arrow_and_smooth() {
        let src = r#"
model M
  annotation(Icon(graphics={
    Line(
      points={{-10,0},{10,0}},
      arrow={Arrow.None, Arrow.Filled},
      arrowSize=5,
      smooth=Smooth.Bezier
    )
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        let GraphicItem::Line(l) = &icon.graphics[0] else {
            panic!("expected Line");
        };
        assert_eq!(l.arrow[0], Arrow::None);
        assert_eq!(l.arrow[1], Arrow::Filled);
        assert_eq!(l.arrow_size, 5.0);
        assert!(l.smooth_bezier);
    }

    #[test]
    fn parses_ellipse_with_arc_bounds() {
        let src = r#"
model M
  annotation(Icon(graphics={
    Ellipse(
      extent={{-10,-10},{10,10}},
      startAngle=0,
      endAngle=180,
      closure=EllipseClosure.Chord
    )
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        let GraphicItem::Ellipse(e) = &icon.graphics[0] else {
            panic!("expected Ellipse");
        };
        assert_eq!(e.start_angle, 0.0);
        assert_eq!(e.end_angle, 180.0);
        assert_eq!(e.closure, EllipseClosure::Chord);
    }

    #[test]
    fn extends_icon_inheritance_merges_graphics() {
        // Simulate: PartialSensor authors a Rectangle + Text("%name"),
        // SpeedSensor extends PartialSensor and adds a Line. The merged
        // Icon should contain all three primitives, base first.
        let src = r#"
partial model PartialSensor
  annotation(Icon(graphics={
    Rectangle(extent={{-100,-100},{100,100}}),
    Text(extent={{-100,110},{100,80}}, textString="%name")
  }));
end PartialSensor;

model SpeedSensor
  extends PartialSensor;
  annotation(Icon(graphics={
    Line(points={{-70,0},{70,0}})
  }));
end SpeedSensor;
"#;
        let ast = parse_source(src);
        let child = ast.classes.get("SpeedSensor").expect("SpeedSensor class");

        // Resolver: clone the class into an Arc so the callback
        // lifetime doesn't fight the AST borrow.
        use std::collections::HashSet;
        use std::sync::Arc;
        let mut resolver = |name: &str| -> Option<Arc<rumoca_session::parsing::ast::ClassDef>> {
            ast.classes
                .get(name)
                .or_else(|| ast.classes.get(name.rsplit('.').next().unwrap_or(name)))
                .map(|c| Arc::new(c.clone()))
        };
        let mut visited = HashSet::new();
        let icon = extract_icon_inherited("SpeedSensor", child, &mut resolver, &mut visited)
            .expect("merged icon");

        assert_eq!(icon.graphics.len(), 3, "expected 2 inherited + 1 local");
        assert!(matches!(icon.graphics[0], GraphicItem::Rectangle(_)));
        assert!(matches!(icon.graphics[1], GraphicItem::Text(_)));
        assert!(matches!(icon.graphics[2], GraphicItem::Line(_)));
    }

    #[test]
    fn extends_icon_inheritance_detects_cycles() {
        // A → B → A. Extractor must not infinite-loop. Both classes
        // would need Icon annotations for this to be a real MSL case,
        // but the cycle check must fire regardless.
        let src = r#"
partial model A
  extends B;
  annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})}));
end A;

partial model B
  extends A;
  annotation(Icon(graphics={Line(points={{-5,0},{5,0}})}));
end B;
"#;
        let ast = parse_source(src);
        let class_a = ast.classes.get("A").expect("A class");
        use std::collections::HashSet;
        use std::sync::Arc;
        let mut resolver = |name: &str| -> Option<Arc<rumoca_session::parsing::ast::ClassDef>> {
            ast.classes.get(name).map(|c| Arc::new(c.clone()))
        };
        let mut visited = HashSet::new();
        // Must terminate.
        let _ = extract_icon_inherited("A", class_a, &mut resolver, &mut visited);
    }

    #[test]
    fn extends_import_alias_resolves() {
        // `extends SI.Voltage;` with `import SI = Modelica.Units.SI;`
        // must expand to `Modelica.Units.SI.Voltage` as a candidate
        // before scope-chain fallbacks.
        let src = r#"
model User
  import SI = Modelica.Units.SI;
  extends SI.Voltage;
end User;
"#;
        let ast = parse_source(src);
        let user = ast.classes.get("User").expect("User class");
        let ext = &user.extends[0];
        let base_name: String = ext
            .base_name
            .name
            .iter()
            .map(|t| t.text.as_ref())
            .collect::<Vec<_>>()
            .join(".");
        let cands = build_extends_candidates("User", &base_name, &user.imports);
        assert!(
            cands.iter().any(|c| c == "Modelica.Units.SI.Voltage"),
            "expected import-alias expansion, got {cands:?}"
        );
    }

    #[test]
    fn parses_bitmap_with_filename_and_image_source() {
        let src = r#"
model M
  annotation(Icon(graphics={
    Bitmap(
      extent={{-20,-20},{20,20}},
      fileName="modelica://Modelica.Blocks.Images/icon.png",
      imageSource="iVBORw0KG=="
    )
  }));
end M;
"#;
        let icon = extract_icon(&class_annotations(src)).expect("icon");
        let GraphicItem::Bitmap(b) = &icon.graphics[0] else {
            panic!("expected Bitmap");
        };
        assert_eq!(
            b.filename.as_deref(),
            Some("modelica://Modelica.Blocks.Images/icon.png")
        );
        assert_eq!(b.image_source.as_deref(), Some("iVBORw0KG=="));
    }
}
