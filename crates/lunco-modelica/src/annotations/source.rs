//! Source-side readers for annotations that the Modelica AST does not retain.
//!
//! Rumoca currently drops annotations attached to `connect` equations while
//! parsing. The authored source is still the canonical document, so the
//! projection layer reads that annotation from the exact connect statement
//! span. The annotation expression itself is parsed by the normal Modelica
//! parser and then passed through the same typed extractor as class and
//! component annotations; this is not a second annotation grammar.

use super::LineRoute;
use crate::ast_mut::{parsing::FRAGMENT_CLASS_NAME, text};
use rumoca_compile::parsing::ast::{ComponentReference, Equation, StoredDefinition};
use std::collections::HashMap;
use std::ops::Range;

/// The endpoint key used by diagram projections.
pub type ConnectEndpointKey = ((String, String), (String, String));

/// Read authored `Line(...)` routes for one parsed document.
///
/// When `target_class` is provided, only that class is visited. This keeps
/// equally-named connections in sibling classes from sharing one UI key.
/// Without a target, all classes are visited for callers that need a document
/// inventory; the first route for a duplicate endpoint key is retained.
pub fn connect_line_routes(
    ast: &StoredDefinition,
    source: &str,
    target_class: Option<&str>,
) -> HashMap<ConnectEndpointKey, LineRoute> {
    let mut routes = HashMap::new();
    if let Some(target) = target_class {
        if let Some(class) = crate::diagram::find_class_by_qualified_name(ast, target) {
            collect_class_routes(class, source, &mut routes, false);
        }
    } else {
        for class in ast.classes.values() {
            collect_class_routes(class, source, &mut routes, true);
        }
    }
    routes
}

/// Read the route attached to a particular `Equation::Connect` source span.
/// The location is the equation's first endpoint token, which is the span
/// Rumoca exposes for a connect equation.
pub fn line_route_for_connect(source: &str, location: usize) -> Option<LineRoute> {
    let statement = connect_statement(source, location)?;
    let (_, annotation_group) = text::annotation_clause(source, statement)?;
    let annotation_args = &source[annotation_group.start + 1..annotation_group.end - 1];
    let stub = format!(
        "model {FRAGMENT_CLASS_NAME}\n  annotation({annotation_args});\nend {FRAGMENT_CLASS_NAME};\n"
    );
    let parsed = crate::ast_mut::parsing::parse_stub_cached(&stub)?;
    let class = parsed.classes.get(FRAGMENT_CLASS_NAME)?;
    super::extract_line_full(&class.annotation)
}

fn collect_class_routes(
    class: &rumoca_compile::parsing::ast::ClassDef,
    source: &str,
    routes: &mut HashMap<ConnectEndpointKey, LineRoute>,
    include_nested: bool,
) {
    for equation in &class.equations {
        let Equation::Connect { lhs, rhs } = equation else {
            continue;
        };
        let Some(location) = lhs.get_location().map(|location| location.start as usize) else {
            continue;
        };
        let Some(route) = line_route_for_connect(source, location) else {
            continue;
        };
        routes
            .entry(canonical_endpoint_key(lhs, rhs))
            .or_insert(route);
    }
    if include_nested {
        for nested in class.classes.values() {
            collect_class_routes(nested, source, routes, true);
        }
    }
}

/// Canonicalise a connection endpoint pair so reversed `connect` arguments
/// address the same authored route.
pub fn canonical_endpoint_key(
    lhs: &ComponentReference,
    rhs: &ComponentReference,
) -> ConnectEndpointKey {
    let lhs = endpoint_key(lhs);
    let rhs = endpoint_key(rhs);
    if lhs <= rhs {
        (lhs, rhs)
    } else {
        (rhs, lhs)
    }
}

fn endpoint_key(reference: &ComponentReference) -> (String, String) {
    (
        reference
            .parts
            .first()
            .map(|part| part.ident.text.to_string())
            .unwrap_or_default(),
        reference
            .parts
            .get(1)
            .map(|part| part.ident.text.to_string())
            .unwrap_or_default(),
    )
}

fn connect_statement(source: &str, location: usize) -> Option<Range<usize>> {
    let mut cursor = text::line_start(source, location);
    loop {
        let keyword = text::find_keyword(source, cursor..source.len(), "connect")?;
        let end = text::statement_end(source, keyword)?;
        if end > location {
            return Some(keyword..end);
        }
        cursor = end;
    }
}
