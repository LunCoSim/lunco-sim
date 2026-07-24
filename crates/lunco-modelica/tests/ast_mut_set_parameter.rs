//! TDD contract tests for [`lunco_modelica::ast_mut::set_parameter`].
//!
//! Shape: parse → mutate → **splice the patch into the source** → reparse →
//! assert the structural change. The middle step is what ships: a mutation's
//! product is a byte patch against the original text, never a re-emission of
//! the class (see `ast_mut/edit.rs`).
//!
//! ## Why integration-style and not unit
//!
//! The helper's contract is "post-splice, the source reparses and carries the
//! right modification". That is only meaningful measured end-to-end through
//! rumoca's parser — testing `IndexMap::insert` in isolation would be testing
//! rumoca, not us.

use lunco_modelica::ast_mut::{self, AstMutError, Edit};
use rumoca_phase_parse::parse_to_ast;

/// End-to-end harness: parse `source`, run `op` on `class_name`, apply the
/// splice, reparse, return the post-mutation `Component` of `component_name`.
fn mutate_and_reparse<F>(
    source: &str,
    class_name: &str,
    component_name: &str,
    op: F,
) -> rumoca_compile::parsing::ast::Component
where
    F: FnOnce(&mut rumoca_compile::parsing::ast::ClassDef, &mut Edit<'_>),
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

/// Run `set_parameter` for its error, discarding any patch.
fn set_parameter_err(
    source: &str,
    class_name: &str,
    component: &str,
    param: &str,
    value: &str,
) -> AstMutError {
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    ast_mut::class_patch(source, &sd, class_name, |c, e| {
        ast_mut::set_parameter(c, e, component, param, value)
    })
    .expect_err("expected set_parameter to fail")
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
    let comp = mutate_and_reparse("model M\n  Real k;\nend M;\n", "M", "k", |class, e| {
        ast_mut::set_parameter(class, e, "k", "fixed", "true").expect("set_parameter");
    });
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
        |class, e| {
            ast_mut::set_parameter(class, e, "k", "min", "2.0").expect("set_parameter");
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
        |class, e| {
            ast_mut::set_parameter(class, e, "k", "min", "1.0").expect("set_parameter");
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
    // Mutation on `a` shouldn't touch `b`. The splice engine makes this
    // structural: `b`'s bytes are never in the patch at all.
    let b = mutate_and_reparse(
        "model M\n  Real a;\n  Real b(fixed = true);\nend M;\n",
        "M",
        "b",
        |class, e| {
            ast_mut::set_parameter(class, e, "a", "fixed", "false").expect("set_parameter");
        },
    );
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
    let comp = mutate_and_reparse("model M\n  Real k;\nend M;\n", "M", "k", |class, e| {
        ast_mut::set_parameter(class, e, "k", "start", "1.5").expect("set_parameter");
    });
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
    assert!(
        rendered.contains("1.5"),
        "expected 1.5 in start, got: {rendered}"
    );
}

#[test]
fn set_parameter_start_replaces_existing_start_value() {
    // Pre-existing `start = 0.5` parsed into the dedicated field;
    // mutation replaces it with `2.0`, no duplicate emission.
    let comp = mutate_and_reparse(
        "model M\n  Real k(start = 0.5);\nend M;\n",
        "M",
        "k",
        |class, e| {
            ast_mut::set_parameter(class, e, "k", "start", "2.0").expect("set_parameter");
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
    let err = set_parameter_err(
        "model M\n  Real k;\nend M;\n",
        "M",
        "nonexistent",
        "start",
        "1.0",
    );
    match err {
        AstMutError::ComponentNotFound {
            class: c,
            component,
        } => {
            assert_eq!(c, "M");
            assert_eq!(component, "nonexistent");
        }
        other => panic!("expected ComponentNotFound, got {other:?}"),
    }
}

#[test]
fn set_parameter_unparseable_value_returns_error() {
    // `@@@` is not a Modelica expression. The value is parsed *before* any
    // splice is recorded, so a bad value can't leave a half-written edit.
    let err = set_parameter_err("model M\n  Real k;\nend M;\n", "M", "k", "start", "@@@");
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
    let comp = mutate_and_reparse("model M\n  Integer n;\nend M;\n", "M", "n", |class, e| {
        ast_mut::set_parameter(class, e, "n", "min", "42").expect("set_parameter");
    });
    assert!(comp.modifications.contains_key("min"));
}

#[test]
fn set_parameter_accepts_boolean_literal() {
    let comp = mutate_and_reparse("model M\n  Real k;\nend M;\n", "M", "k", |class, e| {
        ast_mut::set_parameter(class, e, "k", "fixed", "true").expect("set_parameter");
    });
    assert!(comp.modifications.contains_key("fixed"));
}

#[test]
fn set_parameter_accepts_array_literal() {
    // Use `nominal` rather than `start` so we exercise the
    // generic-modifications-map route; the `start` route is covered
    // separately above.
    let comp = mutate_and_reparse("model M\n  Real x[3];\nend M;\n", "M", "x", |class, e| {
        ast_mut::set_parameter(class, e, "x", "nominal", "{1.0, 2.0, 3.0}").expect("set_parameter");
    });
    assert!(comp.modifications.contains_key("nominal"));
}
