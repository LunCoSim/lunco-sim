//! TDD contract tests for the topology helpers in
//! [`lunco_modelica::ast_mut`]: `add_component`, `remove_component`,
//! `add_connection`, `remove_connection`.
//!
//! Shape: parse → mutate → **splice the patch into the source** → reparse →
//! assert. That middle step is the point: a mutation's product is a byte patch
//! against the original text, never a re-emission of the class (see
//! `ast_mut/edit.rs`). Headless. Pinned end-to-end via
//! `tests/op_to_patch_topology.rs`, which runs the full `host.apply()` route,
//! and `tests/ast_mut_preserves_untouched_source.rs`, which pins what a patch
//! may *not* touch.

use lunco_modelica::ast_mut::{self, AstMutError, Edit};
use lunco_modelica::pretty::{ComponentDecl, ConnectEquation, PortRef};
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, Component, Equation};

/// Run `op` against `class_name`, apply the resulting splice to `source`, and
/// reparse — the same route `Document::apply` takes.
fn mutate_and_reparse_class<F>(
    source: &str,
    class_name: &str,
    op: F,
) -> rumoca_compile::parsing::ast::ClassDef
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    let (range, replacement, _) =
        ast_mut::class_patch(source, &sd, class_name, op).expect("class_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    let sd2 = parse_to_ast(&patched, "test.mo").unwrap_or_else(|e| {
        panic!("post-splice reparse failed: {e:?}\n=== patched ===\n{patched}\n=============")
    });
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{patched}"))
        .clone()
}

/// Run `op` for its error, discarding any patch.
fn mutate_err<F>(source: &str, class_name: &str, op: F) -> AstMutError
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    ast_mut::class_patch(source, &sd, class_name, op)
        .expect_err("expected the mutation to fail")
}

fn decl(type_name: &str, name: &str) -> ComponentDecl {
    ComponentDecl {
        type_name: type_name.to_string(),
        name: name.to_string(),
        modifications: Vec::new(),
        placement: None,
    }
}

fn port(component: &str, port: &str) -> PortRef {
    PortRef {
        component: component.to_string(),
        port: port.to_string(),
    }
}

fn is_connect(eq: &Equation, from_comp: &str, from_port: &str, to_comp: &str, to_port: &str) -> bool {
    matches!(
        eq,
        Equation::Connect { lhs, rhs, .. }
            if lhs.parts.len() == 2
                && &*lhs.parts[0].ident.text == from_comp
                && &*lhs.parts[1].ident.text == from_port
                && rhs.parts.len() == 2
                && &*rhs.parts[0].ident.text == to_comp
                && &*rhs.parts[1].ident.text == to_port
    )
}

// ─────────────────────────────────────────────────────────────────────
// add_component
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_component_inserts_into_empty_class() {
    let class = mutate_and_reparse_class("model M\nend M;\n", "M", |c, e| {
        ast_mut::add_component(c, e, &decl("Real", "x"))
    });
    let comp: &Component = class.components.get("x").expect("x added");
    // Type ref resolves to "Real" — single-segment component reference.
    assert_eq!(comp.components_type_first_part(), Some("Real"));
}

#[test]
fn add_component_preserves_existing_components() {
    let class = mutate_and_reparse_class("model M\n  Real a;\nend M;\n", "M", |c, e| {
        ast_mut::add_component(c, e, &decl("Real", "b"))
    });
    assert!(class.components.contains_key("a"), "existing component dropped");
    assert!(class.components.contains_key("b"), "new component missing");
}

#[test]
fn add_component_duplicate_returns_error() {
    let err = mutate_err("model M\n  Real x;\nend M;\n", "M", |c, e| {
        ast_mut::add_component(c, e, &decl("Real", "x"))
    });
    match err {
        AstMutError::DuplicateComponent { class, component } => {
            assert_eq!(class, "M");
            assert_eq!(component, "x");
        }
        other => panic!("expected DuplicateComponent, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// remove_component
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_component_drops_target() {
    let class = mutate_and_reparse_class("model M\n  Real a;\n  Real b;\nend M;\n", "M", |c, e| {
        ast_mut::remove_component(c, e, "a")
    });
    assert!(!class.components.contains_key("a"), "a still present");
    assert!(class.components.contains_key("b"), "b dropped");
}

#[test]
fn remove_component_unknown_returns_error() {
    let err = mutate_err("model M\n  Real a;\nend M;\n", "M", |c, e| {
        ast_mut::remove_component(c, e, "nope")
    });
    assert!(matches!(
        err,
        AstMutError::ComponentNotFound { component, .. } if component == "nope"
    ));
}

// ─────────────────────────────────────────────────────────────────────
// add_connection
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_connection_appends_to_equation_section() {
    let class = mutate_and_reparse_class(
        "model M\n  SI.Position a;\n  SI.Position b;\nequation\n  connect(a.z, b.z);\nend M;\n",
        "M",
        |c, e| {
            ast_mut::add_connection(
                c,
                e,
                &ConnectEquation { from: port("a", "p"), to: port("b", "q"), line: None },
            )
        },
    );
    assert!(
        class.equations.iter().any(|eq| is_connect(eq, "a", "p", "b", "q")),
        "expected connect(a.p, b.q) in equations"
    );
    assert!(
        class.equations.iter().any(|eq| is_connect(eq, "a", "z", "b", "z")),
        "the existing connect was dropped"
    );
}

#[test]
fn add_connection_creates_equation_section_when_missing() {
    // Source has no `equation` keyword — the splice has to introduce one.
    let class = mutate_and_reparse_class("model M\n  SI.Position a;\nend M;\n", "M", |c, e| {
        ast_mut::add_connection(
            c,
            e,
            &ConnectEquation { from: port("a", "p"), to: port("a", "q"), line: None },
        )
    });
    assert!(
        class.equations.iter().any(|eq| is_connect(eq, "a", "p", "a", "q")),
        "expected connect equation in (newly created) equation section"
    );
}

// ─────────────────────────────────────────────────────────────────────
// remove_connection
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_connection_drops_matching_equation() {
    let class = mutate_and_reparse_class(
        "model M\n  Real a;\n  Real b;\nequation\n  connect(a, b);\nend M;\n",
        "M",
        |c, e| ast_mut::remove_connection(c, e, &port("a", ""), &port("b", "")),
    );
    // Expecting no remaining connect for (a,b). `remove_connection`
    // matches on `component.port`; with empty port the AST ref is
    // single-segment, so we use a slightly different comparison.
    let any_connect = class.equations.iter().any(|eq| matches!(eq, Equation::Connect { .. }));
    assert!(!any_connect, "connect equation still present after remove");
}

#[test]
fn remove_connection_preserves_other_connections() {
    let class = mutate_and_reparse_class(
        "model M\n  Real a;\n  Real b;\n  Real c;\nequation\n  connect(a, b);\n  connect(b, c);\nend M;\n",
        "M",
        |c, e| ast_mut::remove_connection(c, e, &port("a", ""), &port("b", "")),
    );
    let connects: Vec<_> = class
        .equations
        .iter()
        .filter(|eq| matches!(eq, Equation::Connect { .. }))
        .collect();
    assert_eq!(connects.len(), 1, "expected 1 remaining connect, got {}", connects.len());
}

#[test]
fn remove_connection_missing_returns_error() {
    let err = mutate_err(
        "model M\n  Real a;\n  Real b;\nequation\n  connect(a, b);\nend M;\n",
        "M",
        |c, e| ast_mut::remove_connection(c, e, &port("x", "p"), &port("y", "q")),
    );
    match err {
        AstMutError::ConnectionNotFound { class, from, to } => {
            assert_eq!(class, "M");
            assert_eq!(from, "x.p");
            assert_eq!(to, "y.q");
        }
        other => panic!("expected ConnectionNotFound, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Component type-name accessor — keep tests resilient to rumoca's
// inner Component shape changes.
// ─────────────────────────────────────────────────────────────────────

trait ComponentExt {
    fn components_type_first_part(&self) -> Option<&str>;
}

impl ComponentExt for Component {
    fn components_type_first_part(&self) -> Option<&str> {
        // `type_name.name: Vec<Token>` holds the segments of a dotted
        // type ref. First-part check is sufficient for the primitive
        // type assertions in these tests.
        self.type_name.name.first().map(|t| &*t.text)
    }
}
