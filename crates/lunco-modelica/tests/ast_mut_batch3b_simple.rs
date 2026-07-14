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

/// Apply a document-level mutation's splice to `source` and return the patched
/// text — the same route `Document::apply` takes.
fn doc_patched<F>(source: &str, mutate: F) -> String
where
    F: FnOnce(
        &mut rumoca_compile::parsing::ast::StoredDefinition,
        &mut ast_mut::Edit<'_>,
    ) -> Result<(), ast_mut::AstMutError>,
{
    let sd = parse_to_ast(source, "t.mo").expect("first parse");
    let (range, replacement, _) =
        ast_mut::document_patch(source, &sd, mutate).expect("document_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    patched
}

/// Same for a single-class mutation.
fn class_patched<F>(source: &str, class: &str, mutate: F) -> String
where
    F: FnOnce(
        &mut rumoca_compile::parsing::ast::ClassDef,
        &mut ast_mut::Edit<'_>,
    ) -> Result<(), ast_mut::AstMutError>,
{
    let sd = parse_to_ast(source, "t.mo").expect("first parse");
    let (range, replacement, _) =
        ast_mut::class_patch(source, &sd, class, mutate).expect("class_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    patched
}

#[test]
fn add_short_class_at_top_level() {
    let patched = doc_patched("model Host\nend Host;\n", |sd, e| {
        ast_mut::add_short_class(
            sd,
            e,
            "",
            "MyConnector",
            ClassKindSpec::Connector,
            "Real",
            &[],
            &[("unit".to_string(), "\"V\"".to_string())],
        )
    });
    let sd2 = parse_to_ast(&patched, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== patched ===\n{patched}"));
    assert!(
        sd2.classes.contains_key("MyConnector"),
        "MyConnector missing after add_short_class:\n{patched}"
    );
    assert!(
        sd2.classes.contains_key("Host"),
        "the existing class was dropped:\n{patched}"
    );
}

#[test]
fn add_short_class_nested() {
    let patched = doc_patched("package P\nend P;\n", |sd, e| {
        ast_mut::add_short_class(sd, e, "P", "Pin", ClassKindSpec::Connector, "Real", &[], &[])
    });
    let sd2 = parse_to_ast(&patched, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== patched ===\n{patched}"));
    assert!(sd2.classes.get("P").unwrap().classes.contains_key("Pin"));
}

#[test]
fn add_short_class_duplicate_returns_error() {
    let source = "connector A = Real;\n";
    let sd = parse_to_ast(source, "t.mo").unwrap();
    let err = ast_mut::document_patch(source, &sd, |sd, e| {
        ast_mut::add_short_class(sd, e, "", "A", ClassKindSpec::Connector, "Real", &[], &[])
    })
    .expect_err("duplicate must fail");
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
    let patched = class_patched("model M\n  Real x;\nend M;\n", "M", |c, e| {
        ast_mut::add_equation(
            c,
            e,
            &EquationDecl {
                lhs: Some("x".into()),
                rhs: "time".into(),
            },
        )
    });
    let sd2 = parse_to_ast(&patched, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== patched ===\n{patched}"));
    let equations = &sd2.classes.get("M").unwrap().equations;
    assert!(
        !equations.is_empty(),
        "expected at least one equation, got 0; patched:\n{patched}"
    );
}

#[test]
fn add_equation_creates_equation_section_when_missing() {
    let patched = class_patched("model M\nend M;\n", "M", |c, e| {
        ast_mut::add_equation(
            c,
            e,
            &EquationDecl {
                lhs: Some("y".into()),
                rhs: "1".into(),
            },
        )
    });
    assert!(
        patched.contains("equation"),
        "expected an `equation` section to be created; got:\n{patched}"
    );
    parse_to_ast(&patched, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== patched ===\n{patched}"));
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
