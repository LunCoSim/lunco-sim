//! AST and text utility helpers.

use std::sync::Arc;
use rumoca_compile::parsing::ast::{ClassDef, ComponentReference, Expression, StoredDefinition, Token};
use super::errors::AstMutError;
use crate::pretty;

/// Resolve a dotted-qualified class path against a parsed `StoredDefinition`.
///
/// Strips the file's `within` clause prefix at segment boundary
/// before walking — same rule as the read-path
/// `crate::diagram::find_class_by_qualified_name`. Both paths share
/// `crate::diagram::strip_within_prefix` so a future change to
/// within-handling can't silently diverge between read and write.
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
    let local_path: String =
        crate::diagram::strip_within_prefix(qualified, sd.within.as_ref()).to_string();
    let mut parts = local_path.split('.');
    let head = parts.next().expect("split always yields at least one piece");
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

/// Format an `f64` for emission.
pub fn fmt_f64(v: f64) -> String {
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// True when the expression is a `Line(...)` function-call entry.
pub fn expression_is_line_call(e: &Expression) -> bool {
    if let Expression::FunctionCall { comp, .. } = e {
        ref_is_simple(comp, "Line")
    } else {
        false
    }
}

/// True when the reference is a single-segment ident equal to `name`.
pub fn ref_is_simple(
    cref: &ComponentReference,
    name: &str,
) -> bool {
    cref.parts.len() == 1 && &*cref.parts[0].ident.text == name
}

/// Return the named-argument's name.
pub fn named_arg_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::NamedArgument { name, .. } => Some(&name.text),
        _ => None,
    }
}

/// From a freshly-parsed annotation Vec `[Line(points={...})]`, pluck the `points=` NamedArgument.
pub fn extract_points_named_argument(
    annotation: &[Expression],
) -> Option<Expression> {
    for e in annotation {
        if let Expression::FunctionCall { comp, args } = e {
            if !ref_is_simple(comp, "Line") {
                continue;
            }
            for a in args {
                if named_arg_name(a) == Some("points") {
                    return Some(a.clone());
                }
            }
        }
    }
    None
}

/// Match a parsed `ComponentReference` against a `pretty::PortRef`.
pub fn matches_port_ref(
    cref: &ComponentReference,
    port: &pretty::PortRef,
) -> bool {
    if port.port.is_empty() {
        cref.parts.len() == 1 && &*cref.parts[0].ident.text == port.component
    } else {
        cref.parts.len() == 2
            && &*cref.parts[0].ident.text == port.component
            && &*cref.parts[1].ident.text == port.port
    }
}

/// True when `expr` is a graphics-array entry whose head identifier matches `name`.
pub fn is_graphic_entry_named(expr: &Expression, name: &str) -> bool {
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
pub fn graphic_entry_arg<'a>(expr: &'a Expression, key: &str) -> Option<&'a Expression> {
    match expr {
        Expression::FunctionCall { args, .. } => {
            for a in args {
                if let Expression::NamedArgument { name, value } = a {
                    if &*name.text == key {
                        return Some(value.as_ref());
                    }
                }
            }
            None
        }
        Expression::ClassModification { modifications, .. } => {
            for m in modifications {
                if let Expression::Modification { target, value } = m {
                    if target.parts.len() == 1
                        && &*target.parts[0].ident.text == key
                    {
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
pub fn plot_node_signal_matches(expr: &Expression, target_signal: &str) -> bool {
    if !crate::annotations::is_plot_node_record_call(expr) {
        return false;
    }
    matches!(
        graphic_entry_arg(expr, "signal"),
        Some(v) if string_literal_value(v) == Some(target_signal.to_string())
    )
}

pub fn point_pair(e: &Expression) -> Option<(f32, f32)> {
    if let Expression::Array { elements, .. } = e {
        if elements.len() == 2 {
            let x = number_literal_value(&elements[0])?;
            let y = number_literal_value(&elements[1])?;
            return Some((x as f32, y as f32));
        }
    }
    None
}

pub fn number_literal_value(e: &Expression) -> Option<f64> {
    match e {
        Expression::Terminal { token, .. } => token.text.parse::<f64>().ok(),
        Expression::Unary { op, rhs }
            if matches!(op, rumoca_compile::parsing::ast::OpUnary::Minus(_)) =>
        {
            number_literal_value(rhs).map(|v| -v)
        }
        _ => None,
    }
}

/// Delegates to [`crate::ast_extract::string_literal_value`] — the
/// canonical decoder. Kept here as a re-export so existing
/// `super::util::string_literal_value` imports keep working.
pub fn string_literal_value(e: &Expression) -> Option<String> {
    crate::ast_extract::string_literal_value(e)
}

/// A trimmed `Text(...)` graphic.
pub struct TextSpec {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub text: String,
}

pub fn read_text_spec(expr: &Expression) -> TextSpec {
    let mut spec = TextSpec {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 0.0,
        text: String::new(),
    };
    if let Some(v) = graphic_entry_arg(expr, "extent") {
        if let Expression::Array { elements: outer, .. } = v {
            if outer.len() == 2 {
                if let (Some((x1, y1)), Some((x2, y2))) =
                    (point_pair(&outer[0]), point_pair(&outer[1]))
                {
                    spec.x1 = x1;
                    spec.y1 = y1;
                    spec.x2 = x2;
                    spec.y2 = y2;
                }
            }
        }
    }
    if let Some(v) = graphic_entry_arg(expr, "textString") {
        if let Some(s) = string_literal_value(v) {
            spec.text = s;
        }
    }
    spec
}

pub fn render_text_spec(spec: &TextSpec) -> String {
    let escaped = spec.text.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "Text(extent={{{{{},{}}},{{{},{}}}}}, textString=\"{}\")",
        spec.x1, spec.y1, spec.x2, spec.y2, escaped
    )
}

/// True when `expr` is `Name(...)` at the top level.
pub fn is_annotation_entry_named(expr: &Expression, name: &str) -> bool {
    if let Expression::ClassModification { target, .. } = expr {
        target.parts.len() == 1 && &*target.parts[0].ident.text == name
    } else {
        false
    }
}

pub fn synth_token(text: impl Into<Arc<str>>) -> Token {
    Token {
        text: text.into(),
        location: Default::default(),
        token_number: 0,
        token_type: 0,
    }
}

pub fn rewind_to_class_header_start(source: &str, name_start: usize) -> usize {
    let bytes = source.as_bytes();
    if name_start > bytes.len() {
        return name_start;
    }
    let mut i = name_start;
    loop {
        while i > 0 {
            match bytes[i - 1] {
                b' ' | b'\t' => i -= 1,
                _ => break,
            }
        }
        let word_end = i;
        while i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            i -= 1;
        }
        if i == word_end {
            break;
        }
    }
    i
}

pub fn advance_past_trailing_semicolon(source: &str, mut pos: usize) -> usize {
    let bytes = source.as_bytes();
    while pos < bytes.len() {
        match bytes[pos] {
            b' ' | b'\t' => pos += 1,
            b';' => {
                pos += 1;
                if pos < bytes.len() && bytes[pos] == b'\n' {
                    pos += 1;
                }
                break;
            }
            _ => break,
        }
    }
    pos
}

pub fn leading_indent(source: &str, byte_pos: usize) -> String {
    if byte_pos > source.len() {
        return String::new();
    }
    let bytes = source.as_bytes();
    let mut start = byte_pos;
    while start > 0 {
        let c = bytes[start - 1];
        if c == b' ' || c == b'\t' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == 0 || bytes[start - 1] == b'\n' {
        std::str::from_utf8(&bytes[start..byte_pos])
            .map(str::to_string)
            .unwrap_or_default()
    } else {
        String::new()
    }
}

pub fn ends_with_newline(source: &str, byte_end: usize) -> bool {
    byte_end > 0 && source.as_bytes().get(byte_end - 1) == Some(&b'\n')
}
