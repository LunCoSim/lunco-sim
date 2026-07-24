//! Integration test: A.2 batch 2 topology ops (`AddComponent`,
//! `RemoveComponent`, `AddConnection`, `RemoveConnection`) through
//! `Document::apply` after the AST-canonical seam landed in
//! `op_to_patch`. Companion to `op_to_patch_set_parameter_placement.rs`.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::{ComponentDecl, ConnectEquation, PortRef};
use rumoca_phase_parse::parse_to_ast;

fn host(source: &str) -> DocumentHost<ModelicaDocument> {
    let syntax = Arc::new(SyntaxCache::from_source(source, 0));
    let doc = ModelicaDocument::from_parts(
        DocumentId::new(1),
        source.to_string(),
        DocumentOrigin::untitled("test.mo"),
        syntax,
    );
    DocumentHost::new(doc)
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

#[test]
fn add_component_through_apply() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::AddComponent {
        class: "M".into(),
        decl: decl("Real", "x"),
    })
    .expect("apply AddComponent");

    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("parse: {e:?}\n=== src ===\n{}", h.document().source()));
    assert!(
        sd.classes.get("M").unwrap().components.contains_key("x"),
        "x not in components after AddComponent; src:\n{}",
        h.document().source()
    );
}

#[test]
fn add_component_duplicate_through_apply_returns_error() {
    let mut h = host("model M\n  Real x;\nend M;\n");
    let err = h
        .apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: decl("Real", "x"),
        })
        .expect_err("duplicate add must fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("already exists") || msg.to_lowercase().contains("duplicate"),
        "expected duplicate-component error, got: {msg}"
    );
}

#[test]
fn remove_component_through_apply() {
    let mut h = host("model M\n  Real a;\n  Real b;\nend M;\n");
    h.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "a".into(),
    })
    .expect("apply RemoveComponent");

    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    let comps = &sd.classes.get("M").unwrap().components;
    assert!(!comps.contains_key("a"), "a still present");
    assert!(comps.contains_key("b"), "b dropped");
}

#[test]
fn add_connection_through_apply() {
    let mut h = host("model M\n  Real a;\n  Real b;\nend M;\n");
    h.apply(ModelicaOp::AddConnection {
        class: "M".into(),
        eq: ConnectEquation {
            from: port("a", ""),
            to: port("b", ""),
            line: None,
        },
    })
    .expect("apply AddConnection");

    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("parse: {e:?}\n=== src ===\n{}", h.document().source()));
    let any_connect = sd
        .classes
        .get("M")
        .unwrap()
        .equations
        .iter()
        .any(|eq| matches!(eq, rumoca_compile::parsing::ast::Equation::Connect { .. }));
    assert!(
        any_connect,
        "no connect equation after AddConnection; src:\n{}",
        h.document().source()
    );
}

#[test]
fn remove_connection_through_apply() {
    let mut h = host("model M\n  Real a;\n  Real b;\nequation\n  connect(a, b);\nend M;\n");
    h.apply(ModelicaOp::RemoveConnection {
        class: "M".into(),
        from: port("a", ""),
        to: port("b", ""),
    })
    .expect("apply RemoveConnection");

    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    let any_connect = sd
        .classes
        .get("M")
        .unwrap()
        .equations
        .iter()
        .any(|eq| matches!(eq, rumoca_compile::parsing::ast::Equation::Connect { .. }));
    assert!(
        !any_connect,
        "connect equation still present after RemoveConnection"
    );
}
