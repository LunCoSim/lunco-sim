//! TDD contract tests for the topology helpers in
//! [`lunco_modelica::ast_mut`]: `add_component`, `remove_component`,
//! `add_connection`, `remove_connection`.
//!
//! Same shape as the batch-1 tests — parse → mutate → emit → reparse →
//! assert. Headless. Pinned end-to-end via
//! `tests/op_to_patch_topology.rs` (separate file, runs the full
//! `host.apply()` route).

use lunco_modelica::ast_mut::{self, AstMutError};
use lunco_modelica::pretty::{ComponentDecl, ConnectEquation, PortRef};
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, Component, Equation};

fn mutate_and_reparse_class<F>(
    source: &str,
    class_name: &str,
    op: F,
) -> rumoca_compile::parsing::ast::ClassDef
where
    F: FnOnce(&mut ClassDef),
{
    let mut sd = parse_to_ast(source, "test.mo").expect("first parse");
    let class = ast_mut::lookup_class_mut(&mut sd, class_name).expect("class lookup");
    op(class);
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "test.mo").unwrap_or_else(|e| {
        panic!("post-mutation reparse failed: {e:?}\n=== regen ===\n{regen}\n=============")
    });
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{regen}"))
        .clone()
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
    let class = mutate_and_reparse_class("model M\nend M;\n", "M", |c| {
        ast_mut::add_component(c, &decl("Real", "x")).expect("add");
    });
    let comp: &Component = class.components.get("x").expect("x added");
    // Type ref resolves to "Real" — single-segment component reference.
    assert_eq!(comp.components_type_first_part(), Some("Real"));
}

#[test]
fn add_component_preserves_existing_components() {
    let class = mutate_and_reparse_class(
        "model M\n  Real a;\nend M;\n",
        "M",
        |c| {
            ast_mut::add_component(c, &decl("Real", "b")).expect("add");
        },
    );
    assert!(class.components.contains_key("a"), "existing component dropped");
    assert!(class.components.contains_key("b"), "new component missing");
}

#[test]
fn add_component_duplicate_returns_error() {
    let mut sd = parse_to_ast("model M\n  Real x;\nend M;\n", "t.mo").unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::add_component(c, &decl("Real", "x")).unwrap_err();
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
    let class = mutate_and_reparse_class(
        "model M\n  Real a;\n  Real b;\nend M;\n",
        "M",
        |c| {
            ast_mut::remove_component(c, "a").expect("remove");
        },
    );
    assert!(!class.components.contains_key("a"), "a still present");
    assert!(class.components.contains_key("b"), "b dropped");
}

#[test]
fn remove_component_unknown_returns_error() {
    let mut sd = parse_to_ast("model M\n  Real a;\nend M;\n", "t.mo").unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::remove_component(c, "nope").unwrap_err();
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
        "model M\n  SI.Position a;\n  SI.Position b;\nend M;\n",
        "M",
        |c| {
            ast_mut::add_connection(
                c,
                &ConnectEquation { from: port("a", "p"), to: port("b", "q"), line: None },
            )
            .expect("add_connection");
        },
    );
    assert!(
        class.equations.iter().any(|eq| is_connect(eq, "a", "p", "b", "q")),
        "expected connect(a.p, b.q) in equations"
    );
}

#[test]
fn add_connection_creates_equation_section_when_missing() {
    // Source has no `equation` keyword. After add, regen's equation
    // section should contain our connect.
    let class = mutate_and_reparse_class(
        "model M\n  SI.Position a;\nend M;\n",
        "M",
        |c| {
            ast_mut::add_connection(
                c,
                &ConnectEquation { from: port("a", "p"), to: port("a", "q"), line: None },
            )
            .expect("add_connection");
        },
    );
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
        |c| {
            ast_mut::remove_connection(c, &port("a", ""), &port("b", "")).expect("remove");
        },
    );
    // Expecting no remaining connect for (a,b). `remove_connection`
    // matches on `component.port`; with empty port the AST ref is
    // single-segment, so we use a slightly different comparison.
    let any_connect = class.equations.iter().any(|eq| matches!(eq, Equation::Connect { .. }));
    assert!(!any_connect, "connect equation still present after remove");
}

#[test]
fn remove_connection_preserves_other_connections() {
    let mut sd = parse_to_ast(
        "model M\n  Real a;\n  Real b;\n  Real c;\nequation\n  connect(a, b);\n  connect(b, c);\nend M;\n",
        "test.mo",
    )
    .unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::remove_connection(c, &port("a", ""), &port("b", "")).expect("remove");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"));
    let connects: Vec<_> = sd2
        .classes
        .get("M")
        .unwrap()
        .equations
        .iter()
        .filter(|eq| matches!(eq, Equation::Connect { .. }))
        .collect();
    assert_eq!(connects.len(), 1, "expected 1 remaining connect, got {}", connects.len());
}

#[test]
fn remove_connection_missing_returns_error() {
    let mut sd = parse_to_ast(
        "model M\n  Real a;\n  Real b;\nequation\n  connect(a, b);\nend M;\n",
        "t.mo",
    )
    .unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::remove_connection(c, &port("x", "p"), &port("y", "q")).unwrap_err();
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
