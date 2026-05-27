//! Integration test: `SetParameter` and `SetPlacement` flow through
//! `Document::apply` end-to-end after the AST-canonical seam landed in
//! `op_to_patch`.
//!
//! Verifies:
//! 1. `host.apply(...)` returns Ok on a well-formed op.
//! 2. The post-apply source still parses cleanly.
//! 3. After a forced AST refresh, the new modification / Placement is
//!    visible in the parsed `Component`.
//!
//! These are the regressions any future change to `op_to_patch`'s
//! AST-canonical path must keep clear. Headless — no Bevy.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::Placement;
use rumoca_phase_parse::parse_to_ast;

/// Construct a `DocumentHost<ModelicaDocument>` with `source` and a
/// **fresh, sync-parsed** SyntaxCache wired in.
///
/// `ModelicaDocument::new` defers parsing to the engine plugin, which
/// isn't installed in headless integration tests. Calling
/// `refresh_ast_now()` afterwards no-ops with an "engine not installed"
/// warning. Production canvas / inspector flows always have a parsed
/// AST before they emit `SetParameter` / `SetPlacement`, so this
/// helper short-circuits the engine path via `from_parts` —
/// equivalent to "the engine already parsed".
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

#[test]
fn set_parameter_through_apply_writes_modification_to_ast() {
    let mut h = host("model M\n  Real k;\nend M;\n");
    h.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "k".into(),
        param: "fixed".into(),
        value: "true".into(),
    })
    .expect("apply SetParameter");

    // Source still parses.
    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("post-apply parse: {e:?}\n=== src ===\n{}\n", h.document().source()));
    let comp = sd
        .classes
        .get("M")
        .expect("class M still present")
        .components
        .get("k")
        .expect("component k still present");
    assert!(
        comp.modifications.contains_key("fixed"),
        "expected `fixed` modification, got: {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
    assert_eq!(h.generation(), 1, "exactly one gen bump per apply");
}

#[test]
fn set_parameter_start_through_apply_routes_to_dedicated_field() {
    let mut h = host("model M\n  Real k;\nend M;\n");
    h.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "k".into(),
        param: "start".into(),
        value: "1.5".into(),
    })
    .expect("apply SetParameter");

    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("post-apply parse: {e:?}\n=== src ===\n{}\n", h.document().source()));
    let comp = sd.classes.get("M").unwrap().components.get("k").unwrap();
    assert!(
        comp.start_is_modification,
        "start_is_modification flag not set after apply"
    );
    assert!(
        !comp.modifications.contains_key("start"),
        "start should not be in modifications map (would emit duplicate): {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
}

#[test]
fn set_placement_through_apply_writes_annotation_to_ast() {
    let mut h = host("model M\n  Real x;\nend M;\n");
    h.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "x".into(),
        placement: Placement::at(10.0, 20.0),
    })
    .expect("apply SetPlacement");

    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("post-apply parse: {e:?}\n=== src ===\n{}\n", h.document().source()));
    let comp = sd.classes.get("M").unwrap().components.get("x").unwrap();
    let has_placement = comp.annotation.iter().any(|expr| {
        matches!(
            expr,
            rumoca_compile::parsing::ast::Expression::ClassModification { target, .. }
                if target.parts.len() == 1 && &*target.parts[0].ident.text == "Placement"
        )
    });
    assert!(
        has_placement,
        "expected Placement entry in component.annotation; got {} entries",
        comp.annotation.len()
    );
}

#[test]
fn set_placement_replaces_existing_placement_through_apply() {
    let mut h = host(
        "model M\n  Real x annotation(Placement(transformation(extent={{-10,-10},{10,10}})));\nend M;\n",
    );
    h.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "x".into(),
        placement: Placement::at(50.0, 75.0),
    })
    .expect("apply SetPlacement");

    let sd = parse_to_ast(h.document().source(), "test.mo")
        .unwrap_or_else(|e| panic!("post-apply parse: {e:?}\n=== src ===\n{}\n", h.document().source()));
    let comp = sd.classes.get("M").unwrap().components.get("x").unwrap();
    let placements: Vec<_> = comp
        .annotation
        .iter()
        .filter(|expr| {
            matches!(
                expr,
                rumoca_compile::parsing::ast::Expression::ClassModification { target, .. }
                    if target.parts.len() == 1 && &*target.parts[0].ident.text == "Placement"
            )
        })
        .collect();
    assert_eq!(
        placements.len(),
        1,
        "expected exactly one Placement (no duplicate from the AST-canonical path); source:\n{}",
        h.document().source()
    );
}

#[test]
fn set_parameter_unknown_class_returns_validation_error() {
    let mut h = host("model M\n  Real k;\nend M;\n");
    let err = h
        .apply(ModelicaOp::SetParameter {
            class: "Nope".into(),
            component: "k".into(),
            param: "fixed".into(),
            value: "true".into(),
        })
        .expect_err("apply should fail on unknown class");
    // We surface AstMutError as DocumentError::ValidationFailed for
    // parity with the legacy `compute_*_patch` error type. Match
    // loosely to keep the test resilient to message tweaks.
    let msg = format!("{err:?}");
    assert!(
        msg.contains("Nope") || msg.to_lowercase().contains("class"),
        "expected error to mention the missing class, got: {msg}"
    );
}
