use super::*;

#[test]
fn multiple_bases_do_not_recursively_resolve_each_other() {
    let mut session = Session::default();
    session
        .add_document(
            "lookup.mo",
            r#"
package Library
  type Signal = Real;
  model B1 end B1;
  model B2 end B2;
  model B3 end B3;
  model B4 end B4;
  model B5 end B5;
  model Device
    extends B1;
    extends B2;
    extends B3;
    extends B4;
    extends B5;
    Signal value;
  end Device;
  package ContainerBase
    model ParentBody Real inherited; end ParentBody;
  end ContainerBase;
  package Container
    extends ContainerBase;
    model Child extends ParentBody; end Child;
  end Container;
  model Supplier
    model Unavailable Real wrong; end Unavailable;
  end Supplier;
  model Invalid extends Supplier; extends Unavailable; end Invalid;
  model Cycle1 extends Cycle2; end Cycle1;
  model Cycle2 extends Cycle1; end Cycle2;
end Library;
"#,
        )
        .unwrap();
    let interfaces: std::collections::HashMap<_, _> = [
        "Library",
        "Library.Signal",
        "Library.B1",
        "Library.B2",
        "Library.B3",
        "Library.B4",
        "Library.B5",
        "Library.Device",
    ]
    .into_iter()
    .map(|name| {
        (
            name.to_owned(),
            session.class_interface_query("lookup.mo", name).unwrap(),
        )
    })
    .collect();
    let mut lookups = 0;
    let candidates = crate::session::class_interface::type_resolution_candidates(
        "Library.Device",
        "Signal",
        crate::session::class_interface::LookupKind::Type,
        |name| {
            lookups += 1;
            interfaces.get(name).cloned()
        },
    );
    assert_eq!(
        candidates
            .iter()
            .find(|name| interfaces.contains_key(*name)),
        Some(&"Library.Signal".to_owned())
    );
    assert!(
        lookups < 200,
        "five independent bases needed {lookups} scope reads"
    );
    eprintln!("five independent bases: {lookups} scope reads");
    let child = session
        .class_component_declarations_query("Library.Container.Child")
        .unwrap();
    assert_eq!(child[0].component.name, "inherited");
    for invalid in ["Library.Invalid", "Library.Cycle1"] {
        assert!(
            session
                .class_component_declarations_query(invalid)
                .is_none(),
            "{invalid}"
        );
    }
}

#[test]
fn class_interface_queries_collect_imports_and_local_members() {
    let mut session = Session::default();
    let source = r#"
within Demo;
model Outer
  import Alias = Lib.Helper;
  import Lib.Components.Gain;
  import Lib.Components.{Limiter};
  import Lib.Components.*;
  extends Base;
  Gain gain;
  replaceable Alias helper constrainedby Lib.HelperBase;
  partial model Inner
  end Inner;
end Outer;
"#;

    session
        .add_document("input.mo", source)
        .expect("test source should parse");

    let class_interface = session
        .class_interface_query("input.mo", "Demo.Outer")
        .expect("class interface should exist");
    let imports = class_interface.import_map();
    assert_eq!(
        imports.explicit_bindings(),
        &[
            ("Alias".to_string(), "Lib.Helper".to_string()),
            ("Gain".to_string(), "Lib.Components.Gain".to_string()),
            ("Limiter".to_string(), "Lib.Components.Limiter".to_string()),
        ],
        "import map should preserve explicit import bindings in source order"
    );
    assert_eq!(
        imports.wildcard_paths(),
        &["Lib.Components".to_string()],
        "import map should preserve wildcard import prefixes"
    );
    assert_eq!(
        imports.resolve_candidates("Gain"),
        vec!["Lib.Components.Gain".to_string()],
        "import candidate resolution should deduplicate explicit and wildcard matches"
    );
    assert_eq!(
        imports.resolve_candidates("Alias.Inner"),
        vec![
            "Lib.Helper.Inner".to_string(),
            "Lib.Components.Alias.Inner".to_string(),
        ],
        "explicit and wildcard imports should anchor dotted paths"
    );
    assert_eq!(
        imports.resolve_candidates("Gain.Inner"),
        vec!["Lib.Components.Gain.Inner".to_string()],
        "qualified imports should anchor the first segment of a dotted path"
    );
    assert_eq!(
        imports.resolve_candidates("Continuous.PID"),
        vec!["Lib.Components.Continuous.PID".to_string()],
        "wildcard imports should anchor qualified references whose first segment comes from the imported package"
    );

    assert_eq!(class_interface.component_type("gain"), Some("Gain"));
    assert_eq!(class_interface.component_type("helper"), Some("Alias"));
    assert_eq!(
        class_interface
            .nested_class_interfaces()
            .iter()
            .map(|nested_class| nested_class.name().to_string())
            .collect::<Vec<_>>(),
        vec!["Inner".to_string()]
    );
    assert_eq!(class_interface.extends_bases(), &["Base".to_string()]);
    let helper = class_interface
        .component_interface("helper")
        .expect("helper component interface should exist");
    assert!(helper.is_replaceable());
    assert_eq!(helper.constrainedby(), Some("Lib.HelperBase"));
    let inner = class_interface
        .nested_class_interfaces()
        .first()
        .expect("nested class header should exist");
    assert!(inner.is_partial());
}

#[test]
fn class_type_resolution_candidates_query_uses_class_interface_scope() {
    let mut session = Session::default();
    let source = r#"
within Demo;
model Outer
  import Alias = Lib.Helper;
  import Lib.Components.Gain;
  import Lib.Components.*;
  model Inner
  end Inner;
  Gain gain;
  Alias helper;
end Outer;
"#;

    session
        .add_document("input.mo", source)
        .expect("test source should parse");

    assert_eq!(
        session.class_type_resolution_candidates_query("input.mo", "Demo.Outer", "Alias"),
        vec!["Demo.Outer.Alias".to_string(), "Lib.Helper".to_string(),],
        "type candidates follow local, imported, enclosing, and global scopes"
    );
    assert_eq!(
        session.class_type_resolution_candidates_query("input.mo", "Demo.Outer", "Gain"),
        vec![
            "Demo.Outer.Gain".to_string(),
            "Lib.Components.Gain".to_string(),
        ],
        "imported names precede outer and global scope names"
    );
    assert_eq!(
        session.class_type_resolution_candidates_query("input.mo", "Demo.Outer", "Inner"),
        vec![
            "Demo.Outer.Inner".to_string(),
            "Lib.Components.Inner".to_string(),
            "Demo.Inner".to_string(),
            "Inner".to_string(),
        ],
        "local declarations precede wildcard imports and enclosing scopes"
    );
    assert_eq!(
        session.class_type_resolution_candidates_query(
            "input.mo",
            "Demo.Outer",
            "Lib.Components.Gain",
        ),
        vec![
            "Demo.Outer.Lib.Components.Gain".to_string(),
            "Demo.Lib.Components.Gain".to_string(),
            "Lib.Components.Gain".to_string(),
        ],
        "relative qualified names should use lexical scopes before top-level resolution"
    );
    assert_eq!(
        session.class_type_resolution_candidates_query("input.mo", "Demo.Outer", "Continuous.PID"),
        vec![
            "Demo.Outer.Continuous.PID".to_string(),
            "Lib.Components.Continuous.PID".to_string(),
            "Demo.Continuous.PID".to_string(),
            "Continuous.PID".to_string(),
        ],
        "qualified names retain local, imported, enclosing, and global scope order"
    );
    assert_eq!(
        session.class_type_resolution_candidates_query(
            "input.mo",
            "Demo.Outer",
            ".Lib.Components.Gain",
        ),
        vec!["Lib.Components.Gain".to_string()],
        "leading-dot names should resolve as top-level absolute names"
    );
}

#[test]
fn class_local_completion_items_query_uses_class_interface_scope() {
    let mut session = Session::default();
    let source = r#"
within Demo;
model Outer
  parameter Real kp;
  input Real u;
  Real x;
  model Inner
  end Inner;
end Outer;
"#;

    session
        .add_document("input.mo", source)
        .expect("test source should parse");

    let items = session.class_local_completion_items_query("input.mo", "Demo.Outer");
    assert_eq!(
        items,
        vec![
            ClassLocalCompletionItem {
                name: "kp".to_string(),
                detail: "Real".to_string(),
                kind: ClassLocalCompletionKind::Constant,
            },
            ClassLocalCompletionItem {
                name: "u".to_string(),
                detail: "Real".to_string(),
                kind: ClassLocalCompletionKind::Property,
            },
            ClassLocalCompletionItem {
                name: "x".to_string(),
                detail: "Real".to_string(),
                kind: ClassLocalCompletionKind::Variable,
            },
            ClassLocalCompletionItem {
                name: "Inner".to_string(),
                detail: "Model".to_string(),
                kind: ClassLocalCompletionKind::Class,
            },
        ],
        "local completion query should surface components and nested classes with stable kinds"
    );
}

#[test]
fn enclosing_class_qualified_name_query_uses_session_owned_syntax() {
    let mut session = Session::default();
    let source = r#"
within Demo;
model Outer
  model Inner
    Real x;
  end Inner;
  Inner child;
end Outer;
"#;

    session
        .add_document("input.mo", source)
        .expect("test source should parse");

    assert_eq!(
        session.enclosing_class_qualified_name_query("input.mo", 2),
        Some("Demo.Outer".to_string()),
        "outer body lines should resolve to the outer class"
    );
    assert_eq!(
        session.enclosing_class_qualified_name_query("input.mo", 3),
        Some("Demo.Outer.Inner".to_string()),
        "nested body lines should resolve to the innermost class"
    );
}

#[test]
fn class_components_query_returns_ast_direct_components_only() {
    let mut session = Session::default();
    let source = r#"
model Plant
  model Motor
    output Real thrust;
  equation
    thrust = 1;
  end Motor;

  input Real omega_cmd[4];
  output Real position[3];
  Real public_state;
  parameter Real mass = 1;
protected
  Real hidden;
equation
  public_state = mass;
end Plant;
"#;

    session
        .add_document("plant.mo", source)
        .expect("test source should parse");

    let components = session
        .class_components_in_class_query("Plant")
        .expect("Plant components should exist");
    let names = components
        .iter()
        .map(|component| component.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["omega_cmd", "position", "public_state", "mass", "hidden"],
        "query should expose direct AST components and exclude nested class members"
    );
    assert!(components.iter().any(|component| {
        component.name == "omega_cmd"
            && matches!(component.causality, rumoca_core::Causality::Input(_))
    }));
    assert!(components.iter().any(|component| {
        component.name == "position"
            && matches!(component.causality, rumoca_core::Causality::Output(_))
            && component.shape == vec![3]
    }));
    assert!(components.iter().any(|component| {
        component.name == "mass"
            && matches!(
                component.variability,
                rumoca_core::Variability::Parameter(_)
            )
    }));
    assert!(
        components
            .iter()
            .any(|component| component.name == "hidden" && component.is_protected)
    );
    assert!(
        !components
            .iter()
            .any(|component| component.name == "thrust"),
        "nested Motor.thrust must not be reported as a Plant component"
    );
}

#[test]
fn class_interface_item_keys_survive_body_edits() {
    let mut session = Session::default();
    let source_v1 = r#"
within Demo;
model Outer
  model Inner
    Real x;
  equation
    x = 1;
  end Inner;
equation
  1 = 1;
end Outer;
"#;
    let source_v2 = r#"
within Demo;
model Outer
  model Inner
    Real x;
  algorithm
    x := 2;
  end Inner;
algorithm
  assert(true, "ok");
end Outer;
"#;

    session
        .add_document("test.mo", source_v1)
        .expect("initial source should parse");
    let keys_before = session
        .class_interface_index_query("test.mo")
        .expect("class interface index should build");
    let outer_before = keys_before
        .item_key_for_name("Demo.Outer")
        .expect("outer key should exist")
        .clone();
    let inner_before = keys_before
        .item_key_for_name("Demo.Outer.Inner")
        .expect("inner key should exist")
        .clone();

    let parse_error = session.update_document("test.mo", source_v2);
    assert!(parse_error.is_none(), "body edit should remain valid");
    let keys_after = session
        .class_interface_index_query("test.mo")
        .expect("class interface index should rebuild");

    assert_eq!(
        keys_after.item_key_for_name("Demo.Outer"),
        Some(&outer_before),
        "body edits should keep the outer scope key stable"
    );
    assert_eq!(
        keys_after.item_key_for_name("Demo.Outer.Inner"),
        Some(&inner_before),
        "body edits should keep nested scope keys stable"
    );
}

#[test]
fn class_interface_cache_is_invalidated_per_file() {
    let mut session = Session::default();
    session
        .add_document("a.mo", "model A\n  Real x;\nend A;\n")
        .expect("A should parse");
    session
        .add_document("b.mo", "model B\n  Real y;\nend B;\n")
        .expect("B should parse");

    let file_a = session.file_id("a.mo").expect("A should have a file id");
    let file_b = session.file_id("b.mo").expect("B should have a file id");
    let a_before = session.class_interface_query("a.mo", "A").unwrap();
    let b_before = session.class_interface_query("b.mo", "B").unwrap();
    assert!(Arc::ptr_eq(
        &b_before,
        &session.class_interface_query("b.mo", "B").unwrap()
    ));

    session
        .class_interface_index_query("a.mo")
        .expect("A class interface query should build");
    session
        .class_interface_index_query("b.mo")
        .expect("B class interface query should build");
    let b_fingerprint_before = session
        .query_state
        .ast
        .class_interface_query_cache
        .get(&file_b)
        .expect("B class interface cache should exist")
        .fingerprint;

    assert!(
        session
            .query_state
            .ast
            .class_interface_query_cache
            .contains_key(&file_a),
        "A class interface cache should be populated"
    );
    assert!(
        session
            .query_state
            .ast
            .class_interface_query_cache
            .contains_key(&file_b),
        "B class interface cache should be populated"
    );

    let parse_error = session.update_document("b.mo", "model B\n  Real y;\n  Real z;\nend B;\n");
    assert!(parse_error.is_none(), "B update should remain valid");

    assert!(
        session
            .query_state
            .ast
            .class_interface_query_cache
            .contains_key(&file_a),
        "editing B should keep A's class interface cache entry warm"
    );
    assert!(
        session
            .query_state
            .ast
            .class_interface_query_cache
            .contains_key(&file_b),
        "editing B should keep the stale class interface cache resident until B is queried again"
    );

    session
        .class_interface_index_query("b.mo")
        .expect("B class interface query should rebuild");
    let a_after = session.class_interface_query("a.mo", "A").unwrap();
    let b_after = session.class_interface_query("b.mo", "B").unwrap();
    assert!(Arc::ptr_eq(&a_before, &a_after));
    assert!(!Arc::ptr_eq(&b_before, &b_after));
    assert!(b_before.component_type("z").is_none());
    assert_eq!(b_after.component_type("z"), Some("Real"));
    assert_ne!(
        session
            .query_state
            .ast
            .class_interface_query_cache
            .get(&file_b)
            .expect("B class interface cache should still exist")
            .fingerprint,
        b_fingerprint_before,
        "B's class interface fingerprint should change after the structural edit"
    );
}
