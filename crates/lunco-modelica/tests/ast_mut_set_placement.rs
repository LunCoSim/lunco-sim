//! TDD contract tests for [`lunco_modelica::ast_mut::set_placement`].
//!
//! Same shape as `ast_mut_set_parameter.rs`: parse → mutate → **splice** →
//! reparse → assert. Verifies the patched source carries a `Placement(...)`
//! annotation on the target component, and that sibling annotations / sibling
//! components survive.

use lunco_modelica::ast_mut::{self, AstMutError, Edit};
use lunco_modelica::pretty::Placement;
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, Component, Expression};

/// End-to-end harness — parse, run `op`, apply its splice, reparse, return the
/// post-mutation `Component`.
fn mutate_and_reparse<F>(
    source: &str,
    class_name: &str,
    component_name: &str,
    op: F,
) -> Component
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>),
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    let (range, replacement, _) = ast_mut::class_patch(source, &sd, class_name, |c, e| {
        op(c, e);
        Ok(())
    })
    .expect("class_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    let sd2 = parse_to_ast(&patched, "test.mo").unwrap_or_else(|e| {
        panic!("post-splice reparse failed: {e:?}\n=== patched ===\n{patched}\n=============")
    });
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{patched}"))
        .components
        .get(component_name)
        .unwrap_or_else(|| panic!("component `{component_name}` missing after reparse:\n{patched}"))
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
        |class, e| {
            ast_mut::set_placement(class, e,"x", &Placement::at(10.0, 20.0))
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
        |class, e| {
            ast_mut::set_placement(class, e,"x", &Placement::at(50.0, 75.0))
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
        |class, e| {
            ast_mut::set_placement(class, e,"x", &Placement::at(0.0, 0.0))
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
    let b = mutate_and_reparse(
        "model M\n  Real a;\n  Real b annotation(Placement(transformation(extent={{0,0},{1,1}})));\nend M;\n",
        "M",
        "b",
        |class, e| {
            ast_mut::set_placement(class, e, "a", &Placement::at(99.0, 99.0))
                .expect("set_placement");
        },
    );
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
    let source = "model M\n  Real x;\nend M;\n";
    let sd = parse_to_ast(source, "t.mo").unwrap();
    let err = ast_mut::class_patch(source, &sd, "M", |c, e| {
        ast_mut::set_placement(c, e, "nope", &Placement::at(0.0, 0.0))
    })
    .expect_err("unknown component must fail");
    match err {
        AstMutError::ComponentNotFound { class: c, component } => {
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
    // Placing twice must leave exactly one Placement entry, not duplicate.
    // Each apply re-parses, so the second one sees the first one's Placement
    // in the source and replaces it rather than appending.
    let once = mutate_and_reparse("model M\n  Real x;\nend M;\n", "M", "x", |class, e| {
        ast_mut::set_placement(class, e, "x", &Placement::at(5.0, 5.0)).unwrap();
    });
    assert_eq!(
        once.annotation
            .iter()
            .filter(|e| is_call_named(e, "Placement"))
            .count(),
        1
    );

    let twice = mutate_and_reparse(
        "model M\n  Real x annotation(Placement(transformation(extent={{0,0},{10,10}})));\nend M;\n",
        "M",
        "x",
        |class, e| {
            ast_mut::set_placement(class, e, "x", &Placement::at(5.0, 5.0)).unwrap();
        },
    );
    let count = twice
        .annotation
        .iter()
        .filter(|e| is_call_named(e, "Placement"))
        .count();
    assert_eq!(count, 1, "expected 1 Placement, got {count}");
}
