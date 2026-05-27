//! TDD contract tests for batch-3a helpers in
//! [`lunco_modelica::ast_mut`]: `add_variable`, `remove_variable`,
//! `add_class`, `remove_class`.
//!
//! Same shape as batch 1+2 — parse → mutate → emit → reparse →
//! assert. Headless. Integration via `host.apply` lives in
//! `tests/op_to_patch_batch3a.rs`.

use lunco_modelica::ast_mut::{self, AstMutError};
use lunco_modelica::pretty::{
    CausalitySpec, ClassKindSpec, VariabilitySpec, VariableDecl,
};
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};

fn mutate_class<F>(source: &str, class_name: &str, op: F) -> ClassDef
where
    F: FnOnce(&mut ClassDef),
{
    let mut sd = parse_to_ast(source, "test.mo").expect("first parse");
    let class = ast_mut::lookup_class_mut(&mut sd, class_name).expect("class lookup");
    op(class);
    let regen = sd.to_modelica();
    let sd2 = parse_to_ast(&regen, "test.mo")
        .unwrap_or_else(|e| panic!("post-mutation reparse: {e:?}\n=== regen ===\n{regen}"));
    sd2.classes
        .get(class_name)
        .unwrap_or_else(|| panic!("class `{class_name}` missing after reparse:\n{regen}"))
        .clone()
}

fn mutate_doc<F>(source: &str, op: F) -> StoredDefinition
where
    F: FnOnce(&mut StoredDefinition),
{
    let mut sd = parse_to_ast(source, "test.mo").expect("first parse");
    op(&mut sd);
    let regen = sd.to_modelica();
    parse_to_ast(&regen, "test.mo")
        .unwrap_or_else(|e| panic!("reparse: {e:?}\n=== regen ===\n{regen}"))
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
    let class = mutate_class("model M\nend M;\n", "M", |c| {
        ast_mut::add_variable(c, &var("Real", "x")).expect("add_variable");
    });
    assert!(class.components.contains_key("x"), "x missing after add");
}

#[test]
fn add_variable_with_parameter_variability_round_trips() {
    let class = mutate_class("model M\nend M;\n", "M", |c| {
        ast_mut::add_variable(c, &parameter("Real", "k", "1.5")).expect("add_variable");
    });
    let comp = class.components.get("k").expect("k present");
    // Variability variants carry the keyword Token; match the variant
    // tag without binding the inner.
    assert!(
        matches!(
            comp.variability,
            rumoca_compile::parsing::ast::Variability::Parameter(_)
        ),
        "expected parameter variability, got {:?}",
        comp.variability
    );
}

#[test]
fn add_variable_duplicate_returns_error() {
    let mut sd = parse_to_ast("model M\n  Real x;\nend M;\n", "t.mo").unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::add_variable(c, &var("Real", "x")).unwrap_err();
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
    let class = mutate_class(
        "model M\n  Real a;\n  Real b;\nend M;\n",
        "M",
        |c| {
            ast_mut::remove_variable(c, "a").expect("remove_variable");
        },
    );
    assert!(!class.components.contains_key("a"));
    assert!(class.components.contains_key("b"));
}

#[test]
fn remove_variable_unknown_returns_error() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let c = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::remove_variable(c, "nope").unwrap_err();
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
    let sd = mutate_doc("", |sd| {
        ast_mut::add_class(sd, "", "Foo", ClassKindSpec::Model, "", false)
            .expect("add_class");
    });
    assert!(sd.classes.contains_key("Foo"), "top-level class not added");
}

#[test]
fn add_class_inside_existing_parent() {
    let sd = mutate_doc("package P\nend P;\n", |sd| {
        ast_mut::add_class(sd, "P", "Inner", ClassKindSpec::Model, "", false)
            .expect("add_class nested");
    });
    let p = sd.classes.get("P").expect("parent present");
    assert!(p.classes.contains_key("Inner"), "nested class not added");
}

#[test]
fn add_class_duplicate_at_top_level_returns_error() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let err = ast_mut::add_class(&mut sd, "", "Foo", ClassKindSpec::Model, "", false)
        .unwrap_err();
    assert!(matches!(
        err,
        AstMutError::DuplicateClass { name, .. } if name == "Foo"
    ));
}

#[test]
fn add_class_unknown_parent_returns_class_not_found() {
    let mut sd = parse_to_ast("", "t.mo").unwrap();
    let err = ast_mut::add_class(&mut sd, "Nope", "Inner", ClassKindSpec::Model, "", false)
        .unwrap_err();
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}

// ─────────────────────────────────────────────────────────────────────
// remove_class
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_class_top_level() {
    let sd = mutate_doc("model Foo end Foo;\nmodel Bar end Bar;\n", |sd| {
        ast_mut::remove_class(sd, "Foo").expect("remove_class");
    });
    assert!(!sd.classes.contains_key("Foo"));
    assert!(sd.classes.contains_key("Bar"));
}

#[test]
fn remove_class_nested() {
    let sd = mutate_doc(
        "package P\n  model A\n  end A;\n  model B\n  end B;\nend P;\n",
        |sd| {
            ast_mut::remove_class(sd, "P.A").expect("remove_class nested");
        },
    );
    let p = sd.classes.get("P").unwrap();
    assert!(!p.classes.contains_key("A"));
    assert!(p.classes.contains_key("B"));
}

#[test]
fn remove_class_unknown_returns_error() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let err = ast_mut::remove_class(&mut sd, "Bar").unwrap_err();
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}

#[test]
fn remove_class_empty_path_rejected() {
    let mut sd = parse_to_ast("model Foo end Foo;\n", "t.mo").unwrap();
    let err = ast_mut::remove_class(&mut sd, "").unwrap_err();
    assert!(matches!(err, AstMutError::ClassNotFound(_)));
}
