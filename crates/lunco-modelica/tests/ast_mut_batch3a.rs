//! TDD contract tests for batch-3a helpers in
//! [`lunco_modelica::ast_mut`]: `add_variable`, `remove_variable`,
//! `add_class`, `remove_class`.
//!
//! Shape: parse → mutate → **splice the patch into the source** → reparse →
//! assert. A mutation's product is a byte patch against the original text, not
//! a re-emission of the class (see `ast_mut/edit.rs`). Headless. Integration via
//! `host.apply` lives in `tests/op_to_patch_batch3a.rs`.

use lunco_modelica::ast_mut::{self, AstMutError, Edit};
use lunco_modelica::pretty::{CausalitySpec, ClassKindSpec, VariabilitySpec, VariableDecl};
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use rumoca_phase_parse::parse_to_ast;

fn mutate_class<F>(source: &str, class_name: &str, op: F) -> ClassDef
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    let (range, replacement, _) =
        ast_mut::class_patch(source, &sd, class_name, op).expect("class_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    let sd2 = parse_to_ast(&patched, "test.mo")
        .unwrap_or_else(|e| panic!("post-splice reparse: {e:?}\n=== patched ===\n{patched}"));
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{patched}"))
        .clone()
}

fn mutate_doc<F>(source: &str, op: F) -> StoredDefinition
where
    F: FnOnce(&mut StoredDefinition, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    let (range, replacement, _) = ast_mut::document_patch(source, &sd, op).expect("document_patch");
    let mut patched = source.to_string();
    patched.replace_range(range, &replacement);
    parse_to_ast(&patched, "test.mo")
        .unwrap_or_else(|e| panic!("post-splice reparse: {e:?}\n=== patched ===\n{patched}"))
}

/// Run a class mutation for its error, discarding any patch.
fn class_err<F>(source: &str, class_name: &str, op: F) -> AstMutError
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    ast_mut::class_patch(source, &sd, class_name, op).expect_err("expected failure")
}

/// Run a document mutation for its error, discarding any patch.
fn doc_err<F>(source: &str, op: F) -> AstMutError
where
    F: FnOnce(&mut StoredDefinition, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let sd = parse_to_ast(source, "test.mo").expect("first parse");
    ast_mut::document_patch(source, &sd, op).expect_err("expected failure")
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

fn parameter(type_name: &str, name: &str, value: &str) -> VariableDecl {
    VariableDecl {
        name: name.to_string(),
        type_name: type_name.to_string(),
        causality: CausalitySpec::None,
        variability: VariabilitySpec::Parameter,
        flow: false,
        modifications: Vec::new(),
        value: Some(value.to_string()),
        description: String::new(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// add_variable
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_variable_inserts_into_empty_class() {
    let class = mutate_class("model M\nend M;\n", "M", |c, e| {
        ast_mut::add_variable(c, e, &var("Real", "x"))
    });
    assert!(class.components.contains_key("x"), "x missing after add");
}

#[test]
fn add_variable_with_parameter_variability_round_trips() {
    let class = mutate_class("model M\nend M;\n", "M", |c, e| {
        ast_mut::add_variable(c, e, &parameter("Real", "k", "1.5"))
    });
    let comp = class.components.get("k").expect("k present");
    // Variability variants carry the keyword Token; match the variant
    // tag without binding the inner.
    assert!(
        matches!(
            comp.variability,
            rumoca_compile::parsing::Variability::Parameter(_)
        ),
        "expected parameter variability, got {:?}",
        comp.variability
    );
}

#[test]
fn add_variable_duplicate_returns_error() {
    let err = class_err("model M\n  Real x;\nend M;\n", "M", |c, e| {
        ast_mut::add_variable(c, e, &var("Real", "x"))
    });
    assert!(matches!(
        err,
        AstMutError::DuplicateComponent { component, .. } if component == "x"
    ));
}

// ─────────────────────────────────────────────────────────────────────
// remove_variable (alias of remove_component)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_variable_drops_target() {
    let class = mutate_class("model M\n  Real a;\n  Real b;\nend M;\n", "M", |c, e| {
        ast_mut::remove_variable(c, e, "a")
    });
    assert!(!class.components.contains_key("a"));
    assert!(class.components.contains_key("b"));
}

#[test]
fn remove_variable_unknown_returns_error() {
    let err = class_err("model M\nend M;\n", "M", |c, e| {
        ast_mut::remove_variable(c, e, "nope")
    });
    assert!(matches!(
        err,
        AstMutError::ComponentNotFound { component, .. } if component == "nope"
    ));
}

// ─────────────────────────────────────────────────────────────────────
// add_class
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_class_at_top_level() {
    let sd = mutate_doc("model Host\nend Host;\n", |sd, e| {
        ast_mut::add_class(sd, e, "", "Foo", ClassKindSpec::Model, "", false)
    });
    assert!(sd.classes.contains_key("Foo"), "top-level class not added");
    assert!(sd.classes.contains_key("Host"), "existing class dropped");
}

#[test]
fn add_class_inside_existing_parent() {
    let sd = mutate_doc("package P\nend P;\n", |sd, e| {
        ast_mut::add_class(sd, e, "P", "Inner", ClassKindSpec::Model, "", false)
    });
    let p = sd.classes.get("P").expect("parent present");
    assert!(p.classes.contains_key("Inner"), "nested class not added");
}

#[test]
fn add_class_duplicate_at_top_level_returns_error() {
    let err = doc_err("model Foo end Foo;\n", |sd, e| {
        ast_mut::add_class(sd, e, "", "Foo", ClassKindSpec::Model, "", false)
    });
    assert!(matches!(
        err,
        AstMutError::DuplicateClass { name, .. } if name == "Foo"
    ));
}

#[test]
fn add_class_unknown_parent_returns_class_not_found() {
    let err = doc_err("model Host end Host;\n", |sd, e| {
        ast_mut::add_class(sd, e, "Nope", "Inner", ClassKindSpec::Model, "", false)
    });
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}

// ─────────────────────────────────────────────────────────────────────
// remove_class
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_class_top_level() {
    let sd = mutate_doc("model Foo end Foo;\nmodel Bar end Bar;\n", |sd, e| {
        ast_mut::remove_class(sd, e, "Foo")
    });
    assert!(!sd.classes.contains_key("Foo"));
    assert!(sd.classes.contains_key("Bar"));
}

#[test]
fn remove_class_nested() {
    let sd = mutate_doc(
        "package P\n  model A\n  end A;\n  model B\n  end B;\nend P;\n",
        |sd, e| ast_mut::remove_class(sd, e, "P.A"),
    );
    let p = sd.classes.get("P").unwrap();
    assert!(!p.classes.contains_key("A"));
    assert!(p.classes.contains_key("B"));
}

#[test]
fn remove_class_unknown_returns_error() {
    let err = doc_err("model Foo end Foo;\n", |sd, e| {
        ast_mut::remove_class(sd, e, "Bar")
    });
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}

#[test]
fn remove_class_empty_path_rejected() {
    let err = doc_err("model Foo end Foo;\n", |sd, e| {
        ast_mut::remove_class(sd, e, "")
    });
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}
