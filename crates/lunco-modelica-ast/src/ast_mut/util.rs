//! AST and text utility helpers.

use super::errors::AstMutError;
use crate::ast_extract::string_literal_value;
use crate::pretty;
use rumoca_core::Token;
use rumoca_ir_ast::{ClassDef, ComponentReference, Expression, StoredDefinition};
use std::sync::Arc;

/// Resolve a dotted-qualified class path against a parsed `StoredDefinition`.
///
/// Strips the file's `within` clause prefix at segment boundary
/// before walking — same rule as the read-path
/// the shared [`crate::strip_within_prefix`] rule so source editing and
/// Modelica projections agree on package-qualified names.
pub fn lookup_class_mut<'a>(
    sd: &'a mut StoredDefinition,
    qualified: &str,
) -> Result<&'a mut ClassDef, AstMutError> {
    if qualified.is_empty() {
        return Err(AstMutError::ClassNotFound(qualified.into()));
    }
    // Resolve the within-stripped path *before* taking the mutable
    // borrow on `sd.classes`. The strip only needs an immutable
    // borrow of `sd.within`; if we did it inside the walk Rust would
    // complain about overlapping borrows of `sd`.
    let local_path: String = crate::strip_within_prefix(qualified, sd.within.as_ref()).to_string();
    let mut parts = local_path.split('.');
    let head = parts
        .next()
        .expect("split always yields at least one piece");
    let mut current = sd
        .classes
        .get_mut(head)
        .ok_or_else(|| AstMutError::ClassNotFound(qualified.to_string()))?;
    for part in parts {
        current = current
            .classes
            .get_mut(part)
            .ok_or_else(|| AstMutError::ClassNotFound(qualified.to_string()))?;
    }
    Ok(current)
}

/// Match a parsed `ComponentReference` against a `pretty::PortRef`.
pub(super) fn matches_port_ref(cref: &ComponentReference, port: &pretty::PortRef) -> bool {
    if port.port.is_empty() {
        cref.parts.len() == 1 && cref.parts[0].ident.text.as_ref() == port.component.as_str()
    } else {
        cref.parts.len() == 2
            && cref.parts[0].ident.text.as_ref() == port.component.as_str()
            && cref.parts[1].ident.text.as_ref() == port.port.as_str()
    }
}

/// True when `expr` is a graphics-array entry whose head identifier matches `name`.
pub(super) fn is_graphic_entry_named(expr: &Expression, name: &str) -> bool {
    match expr {
        Expression::FunctionCall { comp, .. } => {
            comp.parts.len() == 1 && &*comp.parts[0].ident.text == name
        }
        Expression::ClassModification { target, .. } => {
            target.parts.len() == 1 && &*target.parts[0].ident.text == name
        }
        _ => false,
    }
}

/// Look up a named argument / modification by key inside a graphics-array entry.
pub(super) fn graphic_entry_arg<'a>(expr: &'a Expression, key: &str) -> Option<&'a Expression> {
    match expr {
        Expression::FunctionCall { args, .. } => {
            for a in args {
                if let Expression::NamedArgument { name, value, .. } = a {
                    if &*name.text == key {
                        return Some(value.as_ref());
                    }
                }
            }
            None
        }
        Expression::ClassModification { modifications, .. } => {
            for m in modifications {
                if let Expression::Modification { target, value, .. } = m {
                    if target.parts.len() == 1 && &*target.parts[0].ident.text == key {
                        return Some(value.as_ref());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Predicate: is `expr` a `LunCoAnnotations.PlotNode(...)` (or bare
/// `PlotNode(...)`) whose `signal=` matches?
pub(super) fn plot_node_signal_matches(expr: &Expression, target_signal: &str) -> bool {
    if !crate::ast_extract::is_plot_node_record_call(expr) {
        return false;
    }
    matches!(
        graphic_entry_arg(expr, "signal"),
        Some(v) if string_literal_value(v) == Some(target_signal.to_string())
    )
}

/// Read a two-number Modelica array as a 2D point.
pub(super) fn point_pair(e: &Expression) -> Option<(f32, f32)> {
    if let Expression::Array { elements, .. } = e {
        if elements.len() == 2 {
            let x = number_literal_value(&elements[0])?;
            let y = number_literal_value(&elements[1])?;
            return Some((x as f32, y as f32));
        }
    }
    None
}

/// Read a numeric terminal or negated numeric terminal.
pub(super) fn number_literal_value(e: &Expression) -> Option<f64> {
    match e {
        Expression::Terminal { token, .. } => token.text.parse::<f64>().ok(),
        Expression::Unary {
            op: rumoca_core::OpUnary::Minus,
            rhs,
            ..
        } => number_literal_value(rhs).map(|v| -v),
        _ => None,
    }
}

/// A trimmed `Text(...)` graphic.
pub(super) struct TextSpec {
    /// Left extent coordinate.
    pub(super) x1: f32,
    /// Bottom extent coordinate.
    pub(super) y1: f32,
    /// Right extent coordinate.
    pub(super) x2: f32,
    /// Top extent coordinate.
    pub(super) y2: f32,
    /// Text displayed by the graphic.
    pub(super) text: String,
}

/// Read the extent and text string from a `Text(...)` graphic.
pub(super) fn read_text_spec(expr: &Expression) -> TextSpec {
    let mut spec = TextSpec {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 0.0,
        text: String::new(),
    };
    match graphic_entry_arg(expr, "extent") {
        Some(Expression::Array {
            elements: outer, ..
        }) if outer.len() == 2 => {
            if let (Some((x1, y1)), Some((x2, y2))) = (point_pair(&outer[0]), point_pair(&outer[1]))
            {
                spec.x1 = x1;
                spec.y1 = y1;
                spec.x2 = x2;
                spec.y2 = y2;
            }
        }
        _ => {}
    }
    if let Some(v) = graphic_entry_arg(expr, "textString") {
        if let Some(s) = string_literal_value(v) {
            spec.text = s;
        }
    }
    spec
}

/// Render a [`TextSpec`] as a Modelica `Text(...)` graphic.
pub(super) fn render_text_spec(spec: &TextSpec) -> String {
    let escaped = spec.text.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "Text(extent={{{{{},{}}},{{{},{}}}}}, textString=\"{}\")",
        spec.x1, spec.y1, spec.x2, spec.y2, escaped
    )
}

/// True when `expr` is `Name(...)` at the top level.
pub(super) fn is_annotation_entry_named(expr: &Expression, name: &str) -> bool {
    if let Expression::ClassModification { target, .. } = expr {
        target.parts.len() == 1 && &*target.parts[0].ident.text == name
    } else {
        false
    }
}

/// Construct a parser token for generated AST fragments.
pub(super) fn synth_token(text: impl Into<Arc<str>>) -> Token {
    Token {
        text: text.into(),
        location: Default::default(),
        token_number: 0,
        token_type: 0,
    }
}
