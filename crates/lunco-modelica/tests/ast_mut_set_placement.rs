//! TDD contract tests for [`lunco_modelica::ast_mut::set_placement`].
//!
//! Same shape as `ast_mut_set_parameter.rs`: parse → mutate → emit →
//! reparse → assert. Verifies the round-trip preserves an annotation
//! identifying as `Placement(...)` on the target component, and that
//! sibling annotations / sibling components survive.

use lunco_modelica::ast_mut;
use lunco_modelica::pretty::Placement;
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, Component, Expression};

/// End-to-end harness — parse, run `op`, emit, reparse, return the
/// post-mutation `Component`.
fn mutate_and_reparse<F>(
    source: &str,
    class_name: &str,
    component_name: &str,
    op: F,
) -> Component
where
    F: FnOnce(&mut ClassDef),
{
    let mut sd = parse_to_ast(source, "test.mo").expect("first parse");
    let class = ast_mut::lookup_class_mut(&mut sd, class_name).expect("class lookup");
    op(class);
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "test.mo").unwrap_or_else(|e| {
        panic!(
            "post-mutation reparse failed: {e:?}\n=== regen ===\n{regen}\n============="
        )
    });
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{regen}"))
        .components
        .get(component_name)
        .unwrap_or_else(|| panic!("component `{component_name}` missing after reparse:\n{regen}"))
        .clone()
}

/// Predicate: does this annotation expression look like `Name(...)`?
/// Mirrors the helper inside `ast_mut` (which is private). Rumoca
/// parses annotation entries as `Expression::ClassModification`, not
/// `FunctionCall` — the diagnostic test
/// `ast_mut_diagnose_annotation_shape.rs` pinned the actual shape.
fn is_call_named(expr: &Expression, name: &str) -> bool {
    if let Expression::ClassModification { target, .. } = expr {
        target.parts.len() == 1 && &*target.parts[0].ident.text == name
    } else {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────
// set_placement — happy paths
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_placement_appends_when_no_annotation_exists() {
    let comp = mutate_and_reparse(
        "model M\n  Real x;\nend M;\n",
        "M",
        "x",
        |class| {
            ast_mut::set_placement(class, "x", &Placement::at(10.0, 20.0))
                .expect("set_placement");
        },
    );
    assert!(
        comp.annotation.iter().any(|e| is_call_named(e, "Placement")),
        "expected one Placement entry, got {} annotation(s)",
        comp.annotation.len()
    );
}

#[test]
fn set_placement_replaces_existing_placement() {
    // Pre-existing Placement at (0,0); after mutation, exactly one
    // Placement entry should remain, located at the new coordinates.
    let comp = mutate_and_reparse(
        "model M\n  Real x annotation(Placement(transformation(extent={{-10,-10},{10,10}})));\nend M;\n",
        "M",
        "x",
        |class| {
            ast_mut::set_placement(class, "x", &Placement::at(50.0, 75.0))
                .expect("set_placement");
        },
    );
    let placements: Vec<_> = comp
        .annotation
        .iter()
        .filter(|e| is_call_named(e, "Placement"))
        .collect();
    assert_eq!(
        placements.len(),
        1,
        "expected exactly one Placement after replace, got {}",
        placements.len()
    );
    // The numeric coordinates show up in the rendered Debug — easier
    // than matching the deeply nested FunctionCall shape by hand.
    let rendered = format!("{:?}", placements[0]);
    assert!(
        rendered.contains("40") && rendered.contains("60"),
        "expected new extents in placement, got: {rendered}"
    );
}

#[test]
fn set_placement_preserves_non_placement_annotations() {
    // Component has a `Dialog(...)` entry alongside `Placement(...)`.
    // After we replace `Placement`, `Dialog` must remain.
    let comp = mutate_and_reparse(
        "model M\n  Real x annotation(Dialog(group=\"k\"), Placement(transformation(extent={{-10,-10},{10,10}})));\nend M;\n",
        "M",
        "x",
        |class| {
            ast_mut::set_placement(class, "x", &Placement::at(0.0, 0.0))
                .expect("set_placement");
        },
    );
    assert!(
        comp.annotation.iter().any(|e| is_call_named(e, "Dialog")),
        "Dialog dropped after Placement replace; annotations: {:?}",
        comp.annotation.len()
    );
    assert!(
        comp.annotation.iter().any(|e| is_call_named(e, "Placement")),
        "Placement missing after replace"
    );
}

#[test]
fn set_placement_does_not_disturb_sibling_components() {
    let mut sd = parse_to_ast(
        "model M\n  Real a;\n  Real b annotation(Placement(transformation(extent={{0,0},{1,1}})));\nend M;\n",
        "test.mo",
    )
    .unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::set_placement(class, "a", &Placement::at(99.0, 99.0))
        .expect("set_placement");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "test.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"));
    let b = sd2.classes.get("M").unwrap().components.get("b").unwrap();
    assert!(
        b.annotation.iter().any(|e| is_call_named(e, "Placement")),
        "sibling component `b` lost its Placement annotation"
    );
}

// ─────────────────────────────────────────────────────────────────────
// set_placement — error paths
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_placement_unknown_component_returns_error() {
    let mut sd = parse_to_ast("model M\n  Real x;\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::set_placement(class, "nope", &Placement::at(0.0, 0.0))
        .unwrap_err();
    match err {
        ast_mut::AstMutError::ComponentNotFound { class: c, component } => {
            assert_eq!(c, "M");
            assert_eq!(component, "nope");
        }
        other => panic!("expected ComponentNotFound, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// set_placement — idempotency
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_placement_is_idempotent_on_repeated_calls() {
    // Calling set_placement twice with the same payload must produce
    // exactly one Placement entry, not duplicate. Pinned because the
    // implementation could regress to "always push" if the
    // find-and-replace branch breaks.
    let mut sd = parse_to_ast("model M\n  Real x;\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::set_placement(class, "x", &Placement::at(5.0, 5.0)).unwrap();
    ast_mut::set_placement(class, "x", &Placement::at(5.0, 5.0)).unwrap();
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"));
    let comp = sd2.classes.get("M").unwrap().components.get("x").unwrap();
    let count = comp
        .annotation
        .iter()
        .filter(|e| is_call_named(e, "Placement"))
        .count();
    assert_eq!(count, 1, "expected 1 Placement, got {count}");
}
