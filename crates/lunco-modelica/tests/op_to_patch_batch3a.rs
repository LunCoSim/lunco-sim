//! Integration tests for batch-3a (`AddVariable`, `RemoveVariable`,
//! `AddClass`, `RemoveClass`) through `Document::apply`.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::{CausalitySpec, ClassKindSpec, VariabilitySpec, VariableDecl};
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

fn var(type_name: &str, name: &str) -> VariableDecl {
    VariableDecl {
        name: name.to_string(),
        type_name: type_name.to_string(),
        causality: CausalitySpec::None,
        variability: VariabilitySpec::Continuous,
        flow: false,
        modifications: Vec::new(),
        value: None,
        description: String::new(),
    }
}

#[test]
fn add_variable_through_apply() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::AddVariable {
        class: "M".into(),
        decl: var("Real", "x"),
    })
    .expect("apply AddVariable");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    assert!(sd.classes.get("M").unwrap().components.contains_key("x"));
}

#[test]
fn remove_variable_through_apply() {
    let mut h = host("model M\n  Real a;\n  Real b;\nend M;\n");
    h.apply(ModelicaOp::RemoveVariable {
        class: "M".into(),
        name: "a".into(),
    })
    .expect("apply RemoveVariable");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    let comps = &sd.classes.get("M").unwrap().components;
    assert!(!comps.contains_key("a"));
    assert!(comps.contains_key("b"));
}

#[test]
fn add_class_top_level_through_apply() {
    // The `from_parts` path requires a non-empty buffer to reparse —
    // start with a placeholder class and add a sibling.
    let mut h = host("model Existing end Existing;\n");
    h.apply(ModelicaOp::AddClass {
        parent: "".into(),
        name: "Foo".into(),
        kind: ClassKindSpec::Model,
        description: "".into(),
        partial: false,
    })
    .expect("apply AddClass");
    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== src ===\n{}", h.document().source()));
    assert!(
        sd.classes.contains_key("Foo"),
        "Foo missing; src:\n{}",
        h.document().source()
    );
    assert!(sd.classes.contains_key("Existing"), "Existing dropped");
}

#[test]
fn add_class_nested_through_apply() {
    let mut h = host("package P\nend P;\n");
    h.apply(ModelicaOp::AddClass {
        parent: "P".into(),
        name: "Inner".into(),
        kind: ClassKindSpec::Model,
        description: "".into(),
        partial: false,
    })
    .expect("apply nested AddClass");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    let p = sd.classes.get("P").unwrap();
    assert!(p.classes.contains_key("Inner"));
}

#[test]
fn remove_class_top_level_through_apply() {
    let mut h = host("model A end A;\nmodel B end B;\n");
    h.apply(ModelicaOp::RemoveClass {
        qualified: "A".into(),
    })
    .expect("apply RemoveClass");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    assert!(!sd.classes.contains_key("A"));
    assert!(sd.classes.contains_key("B"));
}

#[test]
fn remove_class_nested_through_apply() {
    let mut h = host("package P\n  model A\n  end A;\n  model B\n  end B;\nend P;\n");
    h.apply(ModelicaOp::RemoveClass {
        qualified: "P.A".into(),
    })
    .expect("apply nested RemoveClass");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    let p = sd.classes.get("P").unwrap();
    assert!(!p.classes.contains_key("A"));
    assert!(p.classes.contains_key("B"));
}
