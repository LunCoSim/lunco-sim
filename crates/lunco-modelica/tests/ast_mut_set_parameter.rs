//! TDD contract tests for [`lunco_modelica::ast_mut::set_parameter`].
//!
//! Per `AGENTS.md` §1, these land before the helper is wired into
//! `document::op_to_patch`. They exercise the helper *via the AST* —
//! parse → mutate → emit → reparse → assert structural change. No
//! source-byte assertions; the round-trip rig from
//! `tests/ast_roundtrip.rs` already validates emitter idempotency.
//!
//! ## Why integration-style and not unit
//!
//! The helper's contract is "post-mutation, the AST has the right
//! modification entry, and `to_modelica` round-trips it cleanly." That
//! contract is only meaningful when measured end-to-end through
//! rumoca's parser and emitter — testing `IndexMap::insert` in
//! isolation would be testing rumoca, not us.

use lunco_modelica::ast_mut::{self, AstMutError};
use rumoca_phase_parse::parse_to_ast;

/// End-to-end harness: parse `source`, run `op` on `class_name`, emit,
/// reparse, return the post-mutation `Component` of `component_name`.
/// Panics with a useful message on any step failure so individual
/// tests stay tight.
fn mutate_and_reparse<F>(
    source: &str,
    class_name: &str,
    component_name: &str,
    op: F,
) -> rumoca_compile::parsing::ast::Component
where
    F: FnOnce(&mut rumoca_compile::parsing::ast::ClassDef),
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

// ─────────────────────────────────────────────────────────────────────
// lookup_class_mut — path resolution
// ─────────────────────────────────────────────────────────────────────

#[test]
fn lookup_class_mut_top_level() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let cls = ast_mut::lookup_class_mut(&mut sd, "Foo").expect("found");
    assert_eq!(&*cls.name.text, "Foo");
}

#[test]
fn lookup_class_mut_nested() {
    let src = "package P\n  model Inner\n  end Inner;\nend P;\n";
    let mut sd = parse_to_ast(src, "t.mo").unwrap();
    let cls = ast_mut::lookup_class_mut(&mut sd, "P.Inner").expect("nested found");
    assert_eq!(&*cls.name.text, "Inner");
}

#[test]
fn lookup_class_mut_missing_returns_class_not_found() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let err = ast_mut::lookup_class_mut(&mut sd, "Bar").unwrap_err();
    assert!(matches!(err, AstMutError::ClassNotFound(s) if s == "Bar"));
}

#[test]
fn lookup_class_mut_empty_path_rejected() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let err = ast_mut::lookup_class_mut(&mut sd, "").unwrap_err();
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}

// ─────────────────────────────────────────────────────────────────────
// set_parameter — happy paths (generic modifications map)
// ─────────────────────────────────────────────────────────────────────
//
// `start` has a dedicated `Component.start` field (see helper docstring);
// it's tested separately below. The "happy path" tests below use
// non-special attribute names (`fixed`, `min`, `nominal`) which always
// route through the generic `modifications: IndexMap` field.

#[test]
fn set_parameter_inserts_new_modification_on_unmodified_component() {
    let comp = mutate_and_reparse(
        "model M\n  Real k;\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "fixed", "true").expect("set_parameter");
        },
    );
    assert!(
        comp.modifications.contains_key("fixed"),
        "modifications after mutation: {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
}

#[test]
fn set_parameter_replaces_existing_modification() {
    // Pre-existing `min = 0.5`; replacement should overwrite, not
    // duplicate. (Idempotency check from the helper's perspective.)
    let comp = mutate_and_reparse(
        "model M\n  Real k(min = 0.5);\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "min", "2.0").expect("set_parameter");
        },
    );
    let rendered = format!("{:?}", comp.modifications.get("min").expect("min present"));
    assert!(
        rendered.contains("2.0") && !rendered.contains("0.5"),
        "expected 2.0 in modification, got: {rendered}"
    );
}

#[test]
fn set_parameter_preserves_other_modifications() {
    // Two existing modifications — replacing one must not drop the
    // other.
    let comp = mutate_and_reparse(
        "model M\n  Real k(min = 0.0, max = 10.0);\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "min", "1.0").expect("set_parameter");
        },
    );
    assert!(
        comp.modifications.contains_key("min"),
        "min dropped: {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
    assert!(
        comp.modifications.contains_key("max"),
        "max dropped: {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
}

#[test]
fn set_parameter_does_not_disturb_sibling_components() {
    // Mutation on `a` shouldn't touch `b`.
    let mut sd = parse_to_ast(
        "model M\n  Real a;\n  Real b(fixed = true);\nend M;\n",
        "t.mo",
    )
    .unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::set_parameter(class, "a", "fixed", "false").expect("set_parameter");
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "t.mo")
        .unwrap_or_else(|e| panic!("reparse failed: {e:?}\n=== regen ===\n{regen}"));
    let b = sd2
        .classes
        .get("M")
        .unwrap()
        .components
        .get("b")
        .expect("b still exists");
    assert!(
        b.modifications.contains_key("fixed"),
        "b's fixed modification dropped"
    );
}

// ─────────────────────────────────────────────────────────────────────
// set_parameter — `start` (dedicated field route)
// ─────────────────────────────────────────────────────────────────────
//
// `start` is special: rumoca lifts it out of the modifications map
// into `Component.start: Expression` + `start_is_modification: bool`.
// Writing to the map alone produces `(start = X)(start = Y)` on emit,
// which fails to reparse. The helper routes "start" to the dedicated
// field; these tests pin that contract.

#[test]
fn set_parameter_start_writes_to_dedicated_field() {
    let comp = mutate_and_reparse(
        "model M\n  Real k;\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "start", "1.5").expect("set_parameter");
        },
    );
    // `start` field populated, flag set, no duplicate in modifications.
    assert!(
        comp.start_is_modification,
        "start_is_modification flag not set"
    );
    assert!(
        !comp.modifications.contains_key("start"),
        "start should not be in modifications map: {:?}",
        comp.modifications.keys().collect::<Vec<_>>()
    );
    let rendered = format!("{:?}", comp.start);
    assert!(rendered.contains("1.5"), "expected 1.5 in start, got: {rendered}");
}

#[test]
fn set_parameter_start_replaces_existing_start_value() {
    // Pre-existing `start = 0.5` parsed into the dedicated field;
    // mutation replaces it with `2.0`, no duplicate emission.
    let comp = mutate_and_reparse(
        "model M\n  Real k(start = 0.5);\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "start", "2.0").expect("set_parameter");
        },
    );
    let rendered = format!("{:?}", comp.start);
    assert!(
        rendered.contains("2.0") && !rendered.contains("0.5"),
        "expected 2.0 in start, got: {rendered}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// set_parameter — error paths
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_parameter_unknown_component_returns_error() {
    let mut sd = parse_to_ast("model M\n  Real k;\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::set_parameter(class, "nonexistent", "start", "1.0").unwrap_err();
    match err {
        AstMutError::ComponentNotFound { class: c, component } => {
            assert_eq!(c, "M");
            assert_eq!(component, "nonexistent");
        }
        other => panic!("expected ComponentNotFound, got {other:?}"),
    }
}

#[test]
fn set_parameter_unparseable_value_returns_error() {
    // `@@@` is not a Modelica expression; the stub-class parse fails
    // and the error surfaces with the offending text.
    let mut sd = parse_to_ast("model M\n  Real k;\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::set_parameter(class, "k", "start", "@@@").unwrap_err();
    match err {
        AstMutError::ValueParseFailed { value } => assert_eq!(value, "@@@"),
        other => panic!("expected ValueParseFailed, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// set_parameter — value-shape coverage
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_parameter_accepts_integer_literal() {
    let comp = mutate_and_reparse(
        "model M\n  Integer n;\nend M;\n",
        "M",
        "n",
        |class| {
            ast_mut::set_parameter(class, "n", "min", "42").expect("set_parameter");
        },
    );
    assert!(comp.modifications.contains_key("min"));
}

#[test]
fn set_parameter_accepts_boolean_literal() {
    let comp = mutate_and_reparse(
        "model M\n  Real k;\nend M;\n",
        "M",
        "k",
        |class| {
            ast_mut::set_parameter(class, "k", "fixed", "true").expect("set_parameter");
        },
    );
    assert!(comp.modifications.contains_key("fixed"));
}

#[test]
fn set_parameter_accepts_array_literal() {
    // Use `nominal` rather than `start` so we exercise the
    // generic-modifications-map route; the `start` route is covered
    // separately above.
    let comp = mutate_and_reparse(
        "model M\n  Real x[3];\nend M;\n",
        "M",
        "x",
        |class| {
            ast_mut::set_parameter(class, "x", "nominal", "{1.0, 2.0, 3.0}")
                .expect("set_parameter");
        },
    );
    assert!(comp.modifications.contains_key("nominal"));
}
