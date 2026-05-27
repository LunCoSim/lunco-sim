//! TDD contract tests for the simpler batch-3b helpers:
//! `add_short_class` and `add_equation`.
//!
//! Annotation-tree ops (`AddPlotNode`, `Set*Extent`, etc.) need a
//! shared `Modification`-tree helper and ship in a separate session.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::ast_mut;
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::{ClassKindSpec, EquationDecl};
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

// ─────────────────────────────────────────────────────────────────────
// add_short_class — helper level
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_short_class_at_top_level() {
    let mut sd = parse_to_ast("", "t.mo").unwrap();
    ast_mut::add_short_class(
        &mut sd,
        "",
        "MyConnector",
        ClassKindSpec::Connector,
        "Real",
        &[],
        &[("unit".to_string(), "\"V\"".to_string())],
    )
    .expect("add_short_class");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"));
    assert!(
        sd2.classes.contains_key("MyConnector"),
        "MyConnector missing after add_short_class:\n{regen}"
    );
}

#[test]
fn add_short_class_nested() {
    let mut sd = parse_to_ast("package P\nend P;\n", "t.mo").unwrap();
    ast_mut::add_short_class(
        &mut sd,
        "P",
        "Pin",
        ClassKindSpec::Connector,
        "Real",
        &[],
        &[],
    )
    .expect("add_short_class nested");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo").expect("reparse");
    assert!(sd2.classes.get("P").unwrap().classes.contains_key("Pin"));
}

#[test]
fn add_short_class_duplicate_returns_error() {
    let mut sd = parse_to_ast("connector A = Real;\n", "t.mo").unwrap();
    let err = ast_mut::add_short_class(
        &mut sd,
        "",
        "A",
        ClassKindSpec::Connector,
        "Real",
        &[],
        &[],
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ast_mut::AstMutError::DuplicateClass { name, .. } if name == "A"
    ));
}

// ─────────────────────────────────────────────────────────────────────
// add_equation — helper level
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_equation_appends_simple_equation() {
    let mut sd = parse_to_ast("model M\n  Real x;\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_equation(
        class,
        &EquationDecl {
            lhs: Some("x".into()),
            rhs: "time".into(),
        },
    )
    .expect("add_equation");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"));
    let equations = &sd2.classes.get("M").unwrap().equations;
    assert!(
        !equations.is_empty(),
        "expected at least one equation, got 0; regen:\n{regen}"
    );
}

#[test]
fn add_equation_creates_equation_section_when_missing() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_equation(
        class,
        &EquationDecl {
            lhs: Some("y".into()),
            rhs: "1".into(),
        },
    )
    .expect("add_equation in empty class");
    let regen = sd.to_modelica();
    assert!(
        regen.contains("equation"),
        "expected `equation` keyword in regen; got:\n{regen}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Integration via host.apply
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_short_class_through_apply() {
    let mut h = host("model Existing end Existing;\n");
    h.apply(ModelicaOp::AddShortClass {
        parent: "".into(),
        name: "Pin".into(),
        kind: ClassKindSpec::Connector,
        base: "Real".into(),
        prefixes: vec![],
        modifications: vec![],
    })
    .expect("apply AddShortClass");
    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== src ===\n{}", h.document().source()));
    assert!(sd.classes.contains_key("Pin"));
}

#[test]
fn set_experiment_through_apply_inserts_when_absent() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::SetExperimentAnnotation {
        class: "M".into(),
        start_time: 0.0,
        stop_time: 1.0,
        tolerance: 1.0e-6,
        interval: 0.01,
    })
    .expect("apply SetExperimentAnnotation");
    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== src ===\n{}", h.document().source()));
    let class = sd.classes.get("M").unwrap();
    let has_experiment = class.annotation.iter().any(|expr| {
        matches!(
            expr,
            rumoca_compile::parsing::ast::Expression::ClassModification { target, .. }
                if target.parts.len() == 1 && &*target.parts[0].ident.text == "experiment"
        )
    });
    assert!(
        has_experiment,
        "expected experiment(...) in class annotation; src:\n{}",
        h.document().source()
    );
}

#[test]
fn set_experiment_through_apply_replaces_existing() {
    // Pre-existing experiment with StopTime=10; new value StopTime=5.
    let mut h = host(
        "model M\nannotation(experiment(StartTime=0, StopTime=10, Tolerance=1e-6, Interval=0.1));\nend M;\n",
    );
    h.apply(ModelicaOp::SetExperimentAnnotation {
        class: "M".into(),
        start_time: 0.0,
        stop_time: 5.0,
        tolerance: 1.0e-6,
        interval: 0.1,
    })
    .expect("apply replace");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    // Exactly one experiment entry — no duplicate.
    let count = sd
        .classes
        .get("M")
        .unwrap()
        .annotation
        .iter()
        .filter(|expr| {
            matches!(
                expr,
                rumoca_compile::parsing::ast::Expression::ClassModification { target, .. }
                    if target.parts.len() == 1 && &*target.parts[0].ident.text == "experiment"
            )
        })
        .count();
    assert_eq!(count, 1, "expected 1 experiment entry, got {count}; src:\n{}", h.document().source());
    // New value present somewhere in the source.
    assert!(
        h.document().source().contains("StopTime = 5") || h.document().source().contains("StopTime=5"),
        "new StopTime not in source:\n{}",
        h.document().source()
    );
}

#[test]
fn add_equation_through_apply() {
    let mut h = host("model M\n  Real x;\nend M;\n");
    h.apply(ModelicaOp::AddEquation {
        class: "M".into(),
        eq: EquationDecl {
            lhs: Some("x".into()),
            rhs: "1".into(),
        },
    })
    .expect("apply AddEquation");
    let sd = parse_to_ast(h.document().source(), "test.mo").expect("reparse");
    assert!(
        !sd.classes.get("M").unwrap().equations.is_empty(),
        "no equation present after AddEquation"
    );
}
