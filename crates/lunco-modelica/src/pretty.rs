//! Subset Modelica pretty-printer.
//!
//! # Scope
//!
//! This module is **not** a full round-trip AST→source serializer. Its only
//! job is to emit snippets of Modelica text for *new* nodes that AST-level
//! document ops are about to splice into an existing source buffer:
//!
//! - a fresh component declaration,
//! - a fresh `connect(...)` equation,
//! - the `annotation(Placement(...))` and `annotation(Line(...))` fragments
//!   that accompany them.
//!
//! Existing nodes keep their original source text byte-for-byte — the
//! Document stays text-canonical, and AST ops only patch the regions they
//! actually change. That keeps comments and formatting intact around edits.
//!
//! If a future op needs to emit a Modelica construct that isn't covered
//! here yet, add a dedicated printer with tests rather than reaching for
//! a general AST walker — growing on demand is deliberate.
//!
//! # Coordinate system
//!
//! `Placement` coordinates are in the standard Modelica diagram space
//! (-100..100, +Y up). The printer does not flip or scale — the caller is
//! responsible for translating UI coordinates into Modelica coordinates.

use std::fmt::Write as _;
use std::sync::RwLock;

// ---------------------------------------------------------------------------
// Formatting options
// ---------------------------------------------------------------------------

/// Indentation preferences applied by the pretty-printer.
///
/// The library default is two-space / four-space so pure-Rust tests
/// have predictable output. Application code (the workbench binary)
/// is free to install a different policy at startup via
/// [`set_options`] — for instance, tab-indented output that matches
/// how Dymola and hand-authored MSL packages ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrettyOptions {
    /// Indent used on the first line of a component declaration or
    /// connect equation.
    pub indent: String,
    /// Indent used on the `annotation(...)` continuation line.
    pub continuation_indent: String,
}

impl PrettyOptions {
    /// Preset: tab indentation (`"\t"` / `"\t\t"`) — the convention
    /// in most hand-authored MSL packages and what the workbench
    /// installs as the user-facing default.
    pub fn tabs() -> Self {
        Self {
            indent: "\t".into(),
            continuation_indent: "\t\t".into(),
        }
    }

    /// Preset: two-space / four-space. Library default; also the
    /// preset tests assume for stable output.
    pub fn two_space() -> Self {
        Self {
            indent: "  ".into(),
            continuation_indent: "    ".into(),
        }
    }
}

impl Default for PrettyOptions {
    fn default() -> Self {
        Self::two_space()
    }
}

static OPTIONS: RwLock<Option<PrettyOptions>> = RwLock::new(None);

/// Current formatting options. Falls back to
/// [`PrettyOptions::default`] (tabs / double-tabs) when nothing has
/// been installed yet.
///
/// Designed as a process-wide setting so every op path (ops from the
/// diagram panel, ops from scripts, ops from tests) produces
/// consistent output without having to thread an options parameter
/// through every call.
pub fn options() -> PrettyOptions {
    OPTIONS
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// Install new formatting options. Subsequent pretty-prints use these
/// values. Panics if the lock is poisoned.
pub fn set_options(opts: PrettyOptions) {
    let mut guard = OPTIONS.write().expect("pretty options lock poisoned");
    *guard = Some(opts);
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

/// A component declaration to be emitted inside a class body.
///
/// Produces text of the form:
///
/// ```modelica
///   Modelica.Electrical.Analog.Basic.Resistor R1(R=100) annotation(
///     Placement(transformation(extent={{-10,-10},{10,10}}))
///   );
/// ```
///
/// The printer emits a single semicolon-terminated line, indented by two
/// spaces. Callers that need different indentation can post-process.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComponentDecl {
    /// Fully-qualified or imported type name
    /// (e.g. `"Modelica.Electrical.Analog.Basic.Resistor"` or `"Resistor"`).
    pub type_name: String,
    /// Instance name (e.g. `"R1"`).
    pub name: String,
    /// Parameter / modifier list in declaration order. Each entry is
    /// `(name, value_expression)`. Values are emitted verbatim; callers
    /// are responsible for quoting strings, formatting numbers, etc.
    pub modifications: Vec<(String, String)>,
    /// Optional diagram placement.
    pub placement: Option<Placement>,
}

/// Diagram placement for a component.
///
/// Maps to `annotation(Placement(transformation(extent={{x1,y1},{x2,y2}})))`.
/// The printer builds the extent as `(x - w/2, y - h/2)..(x + w/2, y + h/2)`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    /// Centre X in Modelica diagram coordinates (-100..100).
    pub x: f32,
    /// Centre Y in Modelica diagram coordinates (-100..100, +Y up).
    pub y: f32,
    /// Extent width (default 20).
    pub width: f32,
    /// Extent height (default 20).
    pub height: f32,
}

impl Placement {
    /// Centered placement with the standard 20x20 extent.
    pub fn at(x: f32, y: f32) -> Self {
        Self { x, y, width: 20.0, height: 20.0 }
    }
}

/// A `connect` equation to be emitted inside an `equation` section.
///
/// Produces:
///
/// ```modelica
///   connect(R1.p, C1.n) annotation(Line(points={{0,0},{10,10}}));
/// ```
///
/// Without `line`, emits the annotation-free form.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectEquation {
    /// Source port (`component.port`).
    pub from: PortRef,
    /// Target port (`component.port`).
    pub to: PortRef,
    /// Optional wire-routing polyline. `None` elides the annotation.
    pub line: Option<Line>,
}

/// A reference to a component port by instance + port name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PortRef {
    /// Component instance name.
    pub component: String,
    /// Port name on that component.
    pub port: String,
}

impl PortRef {
    /// Construct from owned strings.
    pub fn new(component: impl Into<String>, port: impl Into<String>) -> Self {
        Self { component: component.into(), port: port.into() }
    }
}

impl std::fmt::Display for PortRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.port.is_empty() {
            write!(f, "{}", self.component)
        } else if self.component.is_empty() {
            write!(f, "{}", self.port)
        } else {
            write!(f, "{}.{}", self.component, self.port)
        }
    }
}

/// Wire polyline attached to a `connect(...) annotation(Line(...))`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    /// Polyline vertices in Modelica diagram coordinates.
    pub points: Vec<(f32, f32)>,
}

// ---------------------------------------------------------------------------
// Number formatting
// ---------------------------------------------------------------------------

/// Format a float the way Modelica tools do: no trailing `.0` for integer
/// values, no scientific notation, trimmed trailing zeros. This matches
/// the shape of numbers found in hand-authored MSL `annotation(...)`
/// clauses so diffs between our output and existing sources stay small.
fn fmt_num(n: f32) -> String {
    if n.is_nan() {
        return "0".to_string();
    }
    if n == n.trunc() && n.abs() < 1e9 {
        return format!("{}", n as i64);
    }
    let mut s = format!("{:.6}", n);
    // Trim trailing zeros after the decimal point, but keep at least one
    // digit after the dot so `0.1` doesn't become `0.`.
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    }
    s
}

/// Like [`fmt_num`] but preserves full `f64` precision. Used for solver
/// configuration (`Tolerance`, `Interval`) where small values such as
/// `1e-7` must survive the round-trip — an `f32` cast *or* the fixed
/// `{:.6}` format in [`fmt_num`] silently collapses them to `0`.
///
/// Rust's default `f64` `Display` already emits the shortest round-tripping
/// decimal and never switches to scientific notation, which is exactly the
/// hand-authored Modelica shape — so we lean on it directly.
fn fmt_num_f64(n: f64) -> String {
    if n.is_nan() {
        return "0".to_string();
    }
    if n == n.trunc() && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    format!("{}", n)
}

fn fmt_point(x: f32, y: f32) -> String {
    format!("{{{},{}}}", fmt_num(x), fmt_num(y))
}

fn fmt_points(points: &[(f32, f32)]) -> String {
    let parts: Vec<String> = points.iter().map(|(x, y)| fmt_point(*x, *y)).collect();
    format!("{{{}}}", parts.join(","))
}

// ---------------------------------------------------------------------------
// Annotation printers
// ---------------------------------------------------------------------------

/// LunCo vendor extension: a plot tile pinned to the diagram. See
/// [`crate::annotations::LunCoPlotNode`] for the read-side mirror.
/// Position is in Modelica diagram coordinates (`{{x1,y1},{x2,y2}}`,
/// the same convention every other graphic primitive uses).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LunCoPlotNodeSpec {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub signal: String,
    pub title: String,
}

/// Render a single
/// `LunCoAnnotations.PlotNode(extent={{…}}, signal="…", title="…")`
/// record as it appears inside `__LunCo(plotNodes={…})`. `title` is
/// omitted entirely when empty so round-tripping a plot without a
/// custom label leaves a clean source line.
pub fn lunco_plot_node_inner(p: &LunCoPlotNodeSpec) -> String {
    let mut s = format!(
        "LunCoAnnotations.PlotNode(extent={{{},{}}}, signal=\"{}\"",
        fmt_point(p.x1, p.y1),
        fmt_point(p.x2, p.y2),
        // Modelica strings: backslash + quote escapes only.
        p.signal.replace('\\', "\\\\").replace('"', "\\\""),
    );
    if !p.title.is_empty() {
        let _ = write!(
            s,
            ", title=\"{}\"",
            p.title.replace('\\', "\\\\").replace('"', "\\\""),
        );
    }
    s.push(')');
    s
}

/// Render a `Placement(transformation(extent={{x1,y1},{x2,y2}}))` fragment
/// *without* the enclosing `annotation(...)` wrapper.
pub fn placement_inner(p: &Placement) -> String {
    let hw = p.width * 0.5;
    let hh = p.height * 0.5;
    format!(
        "Placement(transformation(extent={{{},{}}}))",
        fmt_point(p.x - hw, p.y - hh),
        fmt_point(p.x + hw, p.y + hh),
    )
}

/// Render a `Line(points={{...}})` fragment *without* the enclosing
/// `annotation(...)` wrapper.
pub fn line_inner(line: &Line) -> String {
    format!("Line(points={})", fmt_points(&line.points))
}

// ---------------------------------------------------------------------------
// Component declaration
// ---------------------------------------------------------------------------

/// Emit a component declaration (trailing newline included).
///
/// Uses the indent strings from the current [`options()`]. When the
/// declaration has an `annotation(Placement(...))`, the annotation is
/// placed on its own continuation line so individual source lines
/// stay short enough to fit in a reasonable editor viewport. Modelica
/// treats whitespace (including newlines) as insignificant between
/// tokens, so the statement is still a single declaration.
pub fn component_decl(decl: &ComponentDecl) -> String {
    let opts = options();
    let mut s = String::new();
    s.push_str(&opts.indent);
    s.push_str(&decl.type_name);
    s.push(' ');
    s.push_str(&decl.name);
    if !decl.modifications.is_empty() {
        s.push('(');
        for (i, (name, value)) in decl.modifications.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            // `write!` into String is infallible.
            let _ = write!(s, "{}={}", name, value);
        }
        s.push(')');
    }
    if let Some(p) = &decl.placement {
        s.push('\n');
        s.push_str(&opts.continuation_indent);
        s.push_str("annotation(");
        s.push_str(&placement_inner(p));
        s.push(')');
    }
    s.push_str(";\n");
    s
}

// ---------------------------------------------------------------------------
// Connect equation
// ---------------------------------------------------------------------------

/// Emit a `connect(...)` equation (trailing newline included).
///
/// As with component declarations, a trailing `annotation(Line(...))`
/// goes on its own continuation line so the main connect statement
/// stays short and readable. Indentation follows [`options()`].
pub fn connect_equation(eq: &ConnectEquation) -> String {
    let opts = options();
    let mut s = String::new();
    s.push_str(&opts.indent);
    let _ = write!(
        s,
        "connect({}, {})",
        eq.from, eq.to,
    );
    if let Some(line) = &eq.line {
        s.push('\n');
        s.push_str(&opts.continuation_indent);
        s.push_str("annotation(");
        s.push_str(&line_inner(line));
        s.push(')');
    }
    s.push_str(";\n");
    s
}

// ---------------------------------------------------------------------------
// Class-level authoring (Layer 2)
// ---------------------------------------------------------------------------

/// Modelica class restriction keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClassKindSpec {
    Model,
    Block,
    Connector,
    Package,
    Record,
    Function,
    Type,
}

impl ClassKindSpec {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Block => "block",
            Self::Connector => "connector",
            Self::Package => "package",
            Self::Record => "record",
            Self::Function => "function",
            Self::Type => "type",
        }
    }
}

/// Variable causality flag (Modelica `input`/`output` prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CausalitySpec {
    #[default]
    None,
    Input,
    Output,
}

/// Variable variability prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VariabilitySpec {
    #[default]
    Continuous,
    Discrete,
    Parameter,
    Constant,
}

/// One variable declaration in a class body. Renders as one line:
/// `{prefixes} <Type> <name>(modifications) = <value> "<description>";`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VariableDecl {
    pub name: String,
    pub type_name: String,
    pub causality: CausalitySpec,
    pub variability: VariabilitySpec,
    pub flow: bool,
    pub modifications: Vec<(String, String)>,
    /// RHS of the `= ...` binding (e.g. `"4000"`, `"m_initial"`). `None` omits the binding.
    pub value: Option<String>,
    pub description: String,
}

/// One equation in a class equation section. `lhs = None` emits a
/// statement (e.g. `assert(...)`) without an `=`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquationDecl {
    pub lhs: Option<String>,
    pub rhs: String,
}

/// Modelica diagram graphic primitive (Icon or Diagram layer).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GraphicSpec {
    Rectangle {
        x1: f32, y1: f32, x2: f32, y2: f32,
        line_color: [u8; 3],
        fill_color: [u8; 3],
        fill_pattern: FillPattern,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        line_color: [u8; 3],
        fill_color: [u8; 3],
        fill_pattern: FillPattern,
    },
    Line {
        points: Vec<(f32, f32)>,
        color: [u8; 3],
        thickness: f32,
        pattern: LinePattern,
    },
    Text {
        x1: f32, y1: f32, x2: f32, y2: f32,
        text: String,
        color: [u8; 3],
        font_size: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FillPattern {
    #[default]
    None,
    Solid,
}

impl FillPattern {
    fn keyword(self) -> &'static str {
        match self {
            Self::None => "FillPattern.None",
            Self::Solid => "FillPattern.Solid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LinePattern {
    #[default]
    Solid,
    Dash,
    Dot,
}

impl LinePattern {
    fn keyword(self) -> &'static str {
        match self {
            Self::Solid => "LinePattern.Solid",
            Self::Dash => "LinePattern.Dash",
            Self::Dot => "LinePattern.Dot",
        }
    }
}

fn fmt_color(c: [u8; 3]) -> String {
    format!("{{{},{},{}}}", c[0], c[1], c[2])
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render the body of one graphic without surrounding whitespace.
pub fn graphic_inner(g: &GraphicSpec) -> String {
    match g {
        GraphicSpec::Rectangle { x1, y1, x2, y2, line_color, fill_color, fill_pattern } => {
            format!(
                "Rectangle(extent={{{},{}}}, lineColor={}, fillColor={}, fillPattern={})",
                fmt_point(*x1, *y1), fmt_point(*x2, *y2),
                fmt_color(*line_color), fmt_color(*fill_color),
                fill_pattern.keyword(),
            )
        }
        GraphicSpec::Polygon { points, line_color, fill_color, fill_pattern } => {
            format!(
                "Polygon(points={}, lineColor={}, fillColor={}, fillPattern={})",
                fmt_points(points),
                fmt_color(*line_color), fmt_color(*fill_color),
                fill_pattern.keyword(),
            )
        }
        GraphicSpec::Line { points, color, thickness, pattern } => {
            format!(
                "Line(points={}, color={}, thickness={}, pattern={})",
                fmt_points(points),
                fmt_color(*color), fmt_num(*thickness),
                pattern.keyword(),
            )
        }
        GraphicSpec::Text { x1, y1, x2, y2, text, color, font_size } => {
            format!(
                "Text(extent={{{},{}}}, textString=\"{}\", textColor={}, fontSize={})",
                fmt_point(*x1, *y1), fmt_point(*x2, *y2),
                escape_string(text),
                fmt_color(*color), fmt_num(*font_size),
            )
        }
    }
}

/// Emit a class header `<kind> <Name> "<desc>"` on one line — caller
/// is responsible for body content and the trailing `end <Name>;`.
pub fn class_header(name: &str, kind: ClassKindSpec, description: &str, partial: bool) -> String {
    let mut s = String::new();
    if partial {
        s.push_str("partial ");
    }
    s.push_str(kind.keyword());
    s.push(' ');
    s.push_str(name);
    if !description.is_empty() {
        let _ = write!(s, " \"{}\"", escape_string(description));
    }
    s
}

/// Emit a complete empty class block: header + `end <Name>;` separated by
/// a single newline. Used by `AddClass`.
pub fn class_block_empty(name: &str, kind: ClassKindSpec, description: &str, partial: bool) -> String {
    format!(
        "{}\n{}end {};\n",
        class_header(name, kind, description, partial),
        options().indent,
        name,
    )
}

/// Emit a short-class definition (one line with trailing newline).
/// Form: `<kind> <Name> = [prefixes...] <Base>(modifications);`
pub fn short_class_decl(
    name: &str,
    kind: ClassKindSpec,
    base: &str,
    prefixes: &[String],
    modifications: &[(String, String)],
) -> String {
    let opts = options();
    let mut s = String::new();
    s.push_str(&opts.indent);
    s.push_str(kind.keyword());
    s.push(' ');
    s.push_str(name);
    s.push_str(" = ");
    for p in prefixes {
        s.push_str(p);
        s.push(' ');
    }
    s.push_str(base);
    if !modifications.is_empty() {
        s.push('(');
        for (i, (k, v)) in modifications.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{}={}", k, v);
        }
        s.push(')');
    }
    s.push_str(";\n");
    s
}

/// Emit a variable declaration (one logical line + trailing newline).
pub fn variable_decl(decl: &VariableDecl) -> String {
    let opts = options();
    let mut s = String::new();
    s.push_str(&opts.indent);
    if decl.flow {
        s.push_str("flow ");
    }
    match decl.variability {
        VariabilitySpec::Continuous => {}
        VariabilitySpec::Discrete => s.push_str("discrete "),
        VariabilitySpec::Parameter => s.push_str("parameter "),
        VariabilitySpec::Constant => s.push_str("constant "),
    }
    match decl.causality {
        CausalitySpec::None => {}
        CausalitySpec::Input => s.push_str("input "),
        CausalitySpec::Output => s.push_str("output "),
    }
    s.push_str(&decl.type_name);
    s.push(' ');
    s.push_str(&decl.name);
    if !decl.modifications.is_empty() {
        s.push('(');
        for (i, (k, v)) in decl.modifications.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{}={}", k, v);
        }
        s.push(')');
    }
    if let Some(value) = &decl.value {
        let _ = write!(s, " = {}", value);
    }
    if !decl.description.is_empty() {
        let _ = write!(s, " \"{}\"", escape_string(&decl.description));
    }
    s.push_str(";\n");
    s
}

/// Emit one equation line. `lhs = None` emits the RHS verbatim as a
/// statement (e.g. `assert(...)`).
pub fn equation_decl(eq: &EquationDecl) -> String {
    let opts = options();
    let mut s = String::new();
    s.push_str(&opts.indent);
    if let Some(lhs) = &eq.lhs {
        let _ = write!(s, "{} = {}", lhs, eq.rhs);
    } else {
        s.push_str(&eq.rhs);
    }
    s.push_str(";\n");
    s
}

/// Render the `experiment(...)` annotation body without the
/// `annotation(...)` wrapper.
pub fn experiment_inner(start_time: f64, stop_time: f64, tolerance: f64, interval: f64) -> String {
    format!(
        "experiment(StartTime={}, StopTime={}, Tolerance={}, Interval={})",
        fmt_num_f64(start_time),
        fmt_num_f64(stop_time),
        fmt_num_f64(tolerance),
        fmt_num_f64(interval),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_formatter_strips_trailing_zeros_and_integer_point() {
        assert_eq!(fmt_num(0.0), "0");
        assert_eq!(fmt_num(10.0), "10");
        assert_eq!(fmt_num(-20.0), "-20");
        assert_eq!(fmt_num(0.5), "0.5");
        assert_eq!(fmt_num(0.001), "0.001");
        assert_eq!(fmt_num(100.25), "100.25");
    }

    #[test]
    fn experiment_inner_preserves_small_tolerance() {
        // Regression: an `as f32` cast + `{:.6}` format used to write
        // Tolerance/Interval of 1e-7 back to the .mo source as `0`.
        let s = experiment_inner(0.0, 10.0, 1e-7, 1e-7);
        assert_eq!(
            s,
            "experiment(StartTime=0, StopTime=10, Tolerance=0.0000001, Interval=0.0000001)"
        );
    }

    #[test]
    fn placement_inner_matches_modelica_shape() {
        let p = Placement::at(10.0, -20.0);
        assert_eq!(
            placement_inner(&p),
            "Placement(transformation(extent={{0,-30},{20,-10}}))"
        );
    }

    #[test]
    fn placement_custom_extent() {
        let p = Placement {
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 40.0,
        };
        assert_eq!(
            placement_inner(&p),
            "Placement(transformation(extent={{-20,-20},{20,20}}))"
        );
    }

    #[test]
    fn component_decl_no_modifications_no_placement() {
        let d = ComponentDecl {
            type_name: "Modelica.Electrical.Analog.Basic.Ground".into(),
            name: "GND".into(),
            modifications: vec![],
            placement: None,
        };
        assert_eq!(
            component_decl(&d),
            "  Modelica.Electrical.Analog.Basic.Ground GND;\n"
        );
    }

    #[test]
    fn component_decl_with_modifications() {
        let d = ComponentDecl {
            type_name: "Resistor".into(),
            name: "R1".into(),
            modifications: vec![("R".into(), "100".into())],
            placement: None,
        };
        assert_eq!(component_decl(&d), "  Resistor R1(R=100);\n");
    }

    #[test]
    fn component_decl_with_multiple_modifications_in_order() {
        let d = ComponentDecl {
            type_name: "Capacitor".into(),
            name: "C1".into(),
            modifications: vec![
                ("C".into(), "0.001".into()),
                ("v(start=0)".into(), "0".into()),
            ],
            placement: None,
        };
        assert_eq!(
            component_decl(&d),
            "  Capacitor C1(C=0.001, v(start=0)=0);\n"
        );
    }

    #[test]
    fn component_decl_with_placement_uses_continuation_line() {
        let d = ComponentDecl {
            type_name: "Resistor".into(),
            name: "R1".into(),
            modifications: vec![("R".into(), "100".into())],
            placement: Some(Placement::at(0.0, 0.0)),
        };
        assert_eq!(
            component_decl(&d),
            "  Resistor R1(R=100)\n    annotation(Placement(transformation(extent={{-10,-10},{10,10}})));\n"
        );
    }

    #[test]
    fn connect_equation_without_line() {
        let eq = ConnectEquation {
            from: PortRef::new("R1", "p"),
            to: PortRef::new("C1", "n"),
            line: None,
        };
        assert_eq!(connect_equation(&eq), "  connect(R1.p, C1.n);\n");
    }

    #[test]
    fn connect_equation_with_line_uses_continuation_line() {
        let eq = ConnectEquation {
            from: PortRef::new("R1", "p"),
            to: PortRef::new("C1", "n"),
            line: Some(Line {
                points: vec![(0.0, 0.0), (10.0, 10.0), (20.0, 10.0)],
            }),
        };
        assert_eq!(
            connect_equation(&eq),
            "  connect(R1.p, C1.n)\n    annotation(Line(points={{0,0},{10,10},{20,10}}));\n"
        );
    }

    #[test]
    fn tabs_preset_produces_tab_indented_output() {
        // Use a scope-local options install so we don't pollute other
        // tests (RwLock serialises, but two tests expecting different
        // global state can still race). We restore the default on
        // exit.
        set_options(PrettyOptions::tabs());
        let d = ComponentDecl {
            type_name: "Real".into(),
            name: "x".into(),
            modifications: vec![],
            placement: Some(Placement::at(0.0, 0.0)),
        };
        let out = component_decl(&d);
        set_options(PrettyOptions::default());
        assert_eq!(
            out,
            "\tReal x\n\t\tannotation(Placement(transformation(extent={{-10,-10},{10,10}})));\n"
        );
    }

    #[test]
    fn emitted_component_decl_reparses_to_matching_ast() {
        // Round-trip sanity: the printer's output should parse cleanly
        // inside a well-formed model. This is the real guard against
        // silly escaping bugs.
        let d = ComponentDecl {
            type_name: "Real".into(),
            name: "x".into(),
            modifications: vec![("start".into(), "1.5".into())],
            placement: None,
        };
        let body = component_decl(&d);
        let source = format!("model M\n{}end M;\n", body);
        let ast = rumoca_phase_parse::parse_to_ast(&source, "test.mo")
            .expect("emitted component decl should parse");
        let class = ast.classes.get("M").expect("class M");
        assert!(
            class.components.contains_key("x"),
            "component x should appear in the parsed AST: keys={:?}",
            class.components.keys().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn emitted_connect_reparses_to_matching_equation() {
        let eq = ConnectEquation {
            from: PortRef::new("a", "p"),
            to: PortRef::new("b", "n"),
            line: None,
        };
        let body = connect_equation(&eq);
        let source = format!(
            "model M\n  Real a;\n  Real b;\nequation\n{}end M;\n",
            body
        );
        let res = rumoca_phase_parse::parse_to_ast(&source, "test.mo");
        assert!(res.is_ok(), "connect(...) should parse: {:?}", res.err());
    }
}
