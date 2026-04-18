//! Validation: can the AST-op infrastructure build + extend realistic
//! multi-class Modelica documents that connect components across types?
//!
//! If any of these fail we need to fix the foundation before wiring the
//! diagram panel (Phase α) to it. Passing means the ops scale to real
//! MSL-shaped circuits.

use lunco_doc::{DocumentHost, DocumentId};
use lunco_modelica::document::{ModelicaChange, ModelicaDocument, ModelicaOp};
use lunco_modelica::pretty::{ComponentDecl, ConnectEquation, Line, Placement, PortRef};

fn doc(source: &str) -> DocumentHost<ModelicaDocument> {
    DocumentHost::new(ModelicaDocument::new(
        DocumentId::new(1),
        source.to_string(),
    ))
}

fn reparse_ok(source: &str) -> bool {
    rumoca_phase_parse::parse_to_ast(source, "test.mo").is_ok()
}

// ─────────────────────────────────────────────────────────────────────
// 1. Multi-component circuit: add heterogeneous components + connect
// ─────────────────────────────────────────────────────────────────────

#[test]
fn build_rc_circuit_from_empty_model() {
    // Start with just a model skeleton. Populate it with MSL-shaped
    // components and connect them — exactly what the diagram panel
    // will do on drag+drop + wire.
    let mut host = doc("model Circuit\nend Circuit;\n");

    let components = vec![
        ("Modelica.Electrical.Analog.Sources.ConstantVoltage", "V1", vec![("V", "10")], Placement::at(-40.0, 0.0)),
        ("Modelica.Electrical.Analog.Basic.Resistor",         "R1", vec![("R", "100")], Placement::at(0.0, 20.0)),
        ("Modelica.Electrical.Analog.Basic.Capacitor",        "C1", vec![("C", "0.001")], Placement::at(0.0, -20.0)),
        ("Modelica.Electrical.Analog.Basic.Ground",           "GND", vec![], Placement::at(40.0, 0.0)),
    ];
    for (ty, name, mods, pos) in components {
        host.apply(ModelicaOp::AddComponent {
            class: "Circuit".into(),
            decl: ComponentDecl {
                type_name: ty.into(),
                name: name.into(),
                modifications: mods.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                placement: Some(pos),
            },
        }).expect("AddComponent should succeed");
    }

    // Four connects, a couple with line annotations to exercise that path.
    let connects = vec![
        (("V1", "p"), ("R1", "p"), None),
        (("R1", "n"), ("C1", "p"), Some(Line { points: vec![(10.0, 20.0), (10.0, -20.0)] })),
        (("C1", "n"), ("GND", "p"), None),
        (("V1", "n"), ("GND", "p"), Some(Line { points: vec![(-40.0, -30.0), (40.0, -30.0), (40.0, -10.0)] })),
    ];
    for ((fc, fp), (tc, tp), line) in connects {
        host.apply(ModelicaOp::AddConnection {
            class: "Circuit".into(),
            eq: ConnectEquation {
                from: PortRef::new(fc, fp),
                to: PortRef::new(tc, tp),
                line,
            },
        }).expect("AddConnection should succeed");
    }

    // Must still parse.
    let final_source = host.document().source();
    assert!(
        reparse_ok(final_source),
        "final RC circuit must reparse:\n---\n{}\n---",
        final_source
    );

    // Cached AST must reflect every component + connect.
    let ast = host.document().ast().ast().expect("AST cache parse ok");
    let circuit = ast.classes.get("Circuit").expect("class Circuit");
    assert_eq!(circuit.components.len(), 4, "4 components expected");
    for name in ["V1", "R1", "C1", "GND"] {
        assert!(circuit.components.contains_key(name), "component {} missing", name);
    }
    assert_eq!(circuit.equations.len(), 4, "4 connect equations expected");
}

// ─────────────────────────────────────────────────────────────────────
// 2. Multi-class file: target a specific class by name
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_component_to_one_class_in_multi_class_file() {
    // A file with THREE top-level classes. Adding to "Beta" must not
    // touch Alpha or Gamma.
    let source = "\
model Alpha
  Real a;
end Alpha;

model Beta
  Real b;
end Beta;

model Gamma
  Real c;
end Gamma;
";
    let mut host = doc(source);

    host.apply(ModelicaOp::AddComponent {
        class: "Beta".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "new_in_beta".into(),
            modifications: vec![],
            placement: None,
        },
    }).unwrap();

    let final_source = host.document().source();
    assert!(reparse_ok(final_source), "must reparse");

    let ast = host.document().ast().ast().unwrap();
    let alpha = ast.classes.get("Alpha").unwrap();
    let beta  = ast.classes.get("Beta").unwrap();
    let gamma = ast.classes.get("Gamma").unwrap();

    assert_eq!(alpha.components.len(), 1, "Alpha untouched");
    assert_eq!(gamma.components.len(), 1, "Gamma untouched");
    assert_eq!(beta.components.len(), 2, "Beta got the new component");
    assert!(beta.components.contains_key("new_in_beta"));
}

// ─────────────────────────────────────────────────────────────────────
// 3. Connect across types (user-defined connector between two classes)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn connect_components_of_different_types_within_one_model() {
    // Two components of *different* connector-having types,
    // connected together — like an MSL Voltage source + Resistor.
    // This is the normal diagram case.
    let source = "\
connector Pin
  Real v;
  flow Real i;
end Pin;

model Src
  Pin p;
end Src;

model Load
  Pin p;
end Load;

model Circuit
  Src source1;
  Load load1;
end Circuit;
";
    let mut host = doc(source);

    host.apply(ModelicaOp::AddConnection {
        class: "Circuit".into(),
        eq: ConnectEquation {
            from: PortRef::new("source1", "p"),
            to: PortRef::new("load1", "p"),
            line: None,
        },
    }).unwrap();

    let final_source = host.document().source();
    assert!(reparse_ok(final_source), "cross-type connect must reparse");
    assert!(
        final_source.contains("connect(source1.p, load1.p)"),
        "connect text present:\n{}", final_source
    );

    let ast = host.document().ast().ast().unwrap();
    let circuit = ast.classes.get("Circuit").unwrap();
    assert_eq!(circuit.equations.len(), 1);
    // Other classes unchanged.
    assert_eq!(ast.classes.get("Pin").unwrap().components.len(), 2);
    assert_eq!(ast.classes.get("Src").unwrap().components.len(), 1);
    assert_eq!(ast.classes.get("Load").unwrap().components.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────
// 4. Nested classes — qualified-path resolution
// ─────────────────────────────────────────────────────────────────────

#[test]
fn nested_class_resolved_by_qualified_path() {
    let source = "\
package Pkg
  model Inner
    Real a;
  end Inner;
end Pkg;
";
    let mut host = doc(source);

    host.apply(ModelicaOp::AddComponent {
        class: "Pkg.Inner".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "b".into(),
            modifications: vec![],
            placement: None,
        },
    })
    .expect("qualified nested-class path must resolve");

    let final_source = host.document().source();
    assert!(reparse_ok(final_source), "must reparse");
    let ast = host.document().ast().ast().unwrap();
    let inner = ast
        .classes
        .get("Pkg")
        .and_then(|p| p.classes.get("Inner"))
        .unwrap();
    assert_eq!(inner.components.len(), 2);
    assert!(inner.components.contains_key("b"));
}

#[test]
fn bare_name_does_not_match_nested_class() {
    // `Inner` is not a top-level class — using the bare name without
    // the `Pkg.` prefix must still fail (there is no shadowing /
    // implicit scope lookup in our op resolver).
    let source = "\
package Pkg
  model Inner
    Real a;
  end Inner;
end Pkg;
";
    let mut host = doc(source);
    let err = host.apply(ModelicaOp::AddComponent {
        class: "Inner".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "b".into(),
            modifications: vec![],
            placement: None,
        },
    });
    assert!(err.is_err());
}

#[test]
fn deeply_nested_qualified_path_resolves() {
    let source = "\
package A
  package B
    model C
      Real x;
    end C;
  end B;
end A;
";
    let mut host = doc(source);
    host.apply(ModelicaOp::AddComponent {
        class: "A.B.C".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "y".into(),
            modifications: vec![],
            placement: None,
        },
    })
    .unwrap();
    let ast = host.document().ast().ast().unwrap();
    let c = ast
        .classes
        .get("A")
        .and_then(|a| a.classes.get("B"))
        .and_then(|b| b.classes.get("C"))
        .unwrap();
    assert!(c.components.contains_key("x"));
    assert!(c.components.contains_key("y"));
}

#[test]
fn qualified_path_with_missing_intermediate_segment_errors() {
    let source = "\
package A
  model X
    Real a;
  end X;
end A;
";
    let mut host = doc(source);
    let err = host
        .apply(ModelicaOp::AddComponent {
            class: "A.MissingMid.X".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap_err();
    // Error message should name the specific missing segment.
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("MissingMid") || msg.contains("A.MissingMid"),
        "error message mentions missing segment: {}",
        msg
    );
}

#[test]
fn nested_class_connect_equation_lands_in_right_class() {
    let source = "\
package Pkg
  model Circuit
    Real a;
    Real b;
  end Circuit;
end Pkg;
";
    let mut host = doc(source);
    host.apply(ModelicaOp::AddConnection {
        class: "Pkg.Circuit".into(),
        eq: ConnectEquation {
            from: PortRef::new("a", "p"),
            to: PortRef::new("b", "n"),
            line: None,
        },
    })
    .unwrap();

    let ast = host.document().ast().ast().unwrap();
    let circuit = ast.classes.get("Pkg").unwrap().classes.get("Circuit").unwrap();
    assert_eq!(circuit.equations.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────
// 5. Model with inheritance — extends clause must not interfere
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_component_works_in_class_with_extends() {
    let source = "\
partial model Base
  Real shared;
end Base;

model Derived
  extends Base;
  Real local;
end Derived;
";
    let mut host = doc(source);

    host.apply(ModelicaOp::AddComponent {
        class: "Derived".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "added".into(),
            modifications: vec![("start".into(), "1.0".into())],
            placement: None,
        },
    }).unwrap();

    let final_source = host.document().source();
    assert!(reparse_ok(final_source), "must reparse after adding to extending class");

    let ast = host.document().ast().ast().unwrap();
    let derived = ast.classes.get("Derived").unwrap();
    assert_eq!(derived.extends.len(), 1, "extends preserved");
    assert!(derived.components.contains_key("local"));
    assert!(derived.components.contains_key("added"));
}

// ─────────────────────────────────────────────────────────────────────
// 6. Undo spans all op types — panels rely on full undo history
// ─────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────
// 7. Remove / Set ops — diagram delete / drag / parameter-edit
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_component_deletes_single_line_decl() {
    let mut host = doc("model M\n  Real a;\n  Real b;\n  Real c;\nend M;\n");
    host.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "b".into(),
    }).unwrap();
    assert_eq!(host.document().source(), "model M\n  Real a;\n  Real c;\nend M;\n");
    assert!(reparse_ok(host.document().source()));
}

#[test]
fn remove_component_deletes_decl_with_annotation() {
    let src = "model M\n  Resistor R1(R=100) annotation(Placement(transformation(extent={{-10,-10},{10,10}})));\n  Real a;\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "R1".into(),
    }).unwrap();
    assert_eq!(host.document().source(), "model M\n  Real a;\nend M;\n");
}

#[test]
fn remove_component_is_invertible() {
    let src = "model M\n  Real a;\n  Real b;\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "a".into(),
    }).unwrap();
    host.undo().unwrap();
    assert_eq!(host.document().source(), src);
}

#[test]
fn remove_connection_deletes_single_connect() {
    let src = "model M\n  Real a;\n  Real b;\nequation\n  connect(a.p, b.n);\n  connect(a.n, b.p);\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::RemoveConnection {
        class: "M".into(),
        from: PortRef::new("a", "p"),
        to: PortRef::new("b", "n"),
    }).unwrap();
    let expected = "model M\n  Real a;\n  Real b;\nequation\n  connect(a.n, b.p);\nend M;\n";
    assert_eq!(host.document().source(), expected);
    assert!(reparse_ok(host.document().source()));
}

#[test]
fn remove_connection_matches_either_direction() {
    let src = "model M\n  Real a;\n  Real b;\nequation\n  connect(a.p, b.n);\nend M;\n";
    let mut host = doc(src);
    // Args swapped — should still find the equation.
    host.apply(ModelicaOp::RemoveConnection {
        class: "M".into(),
        from: PortRef::new("b", "n"),
        to: PortRef::new("a", "p"),
    }).unwrap();
    assert!(!host.document().source().contains("connect"));
}

#[test]
fn remove_connection_with_line_annotation() {
    let src = "model M\n  Real a;\n  Real b;\nequation\n  connect(a.p, b.n) annotation(Line(points={{0,0},{10,10}}));\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::RemoveConnection {
        class: "M".into(),
        from: PortRef::new("a", "p"),
        to: PortRef::new("b", "n"),
    }).unwrap();
    let final_src = host.document().source();
    assert!(!final_src.contains("connect"), "connect line gone: {}", final_src);
    assert!(!final_src.contains("annotation"), "Line annotation gone too: {}", final_src);
}

#[test]
fn set_placement_inserts_annotation_when_missing() {
    let mut host = doc("model M\n  Resistor R1(R=100);\nend M;\n");
    host.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "R1".into(),
        placement: Placement::at(5.0, 5.0),
    }).unwrap();
    let src = host.document().source();
    assert!(src.contains("annotation(Placement(transformation(extent={{-5,-5},{15,15}})))"),
        "placement inserted: {}", src);
    assert!(reparse_ok(src));
}

#[test]
fn set_placement_replaces_existing_placement() {
    let src = "model M\n  Resistor R1(R=100) annotation(Placement(transformation(extent={{-10,-10},{10,10}})));\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "R1".into(),
        placement: Placement::at(50.0, -20.0),
    }).unwrap();
    let out = host.document().source();
    assert!(out.contains("{{40,-30},{60,-10}}"), "new placement extent: {}", out);
    assert!(reparse_ok(out));
}

#[test]
fn set_placement_preserves_other_annotations() {
    // annotation already has Dialog — we add Placement alongside it
    let src = "model M\n  Resistor R1(R=100) annotation(Dialog(tab=\"foo\"));\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "R1".into(),
        placement: Placement::at(0.0, 0.0),
    }).unwrap();
    let out = host.document().source();
    assert!(out.contains("Dialog"), "Dialog annotation preserved: {}", out);
    assert!(out.contains("Placement"), "Placement added: {}", out);
    assert!(reparse_ok(out));
}

#[test]
fn set_parameter_replaces_existing_value() {
    let mut host = doc("model M\n  Resistor R1(R=100);\nend M;\n");
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "42".into(),
    }).unwrap();
    let out = host.document().source();
    assert!(out.contains("R1(R=42)") || out.contains("R1(R =42)") || out.contains("R1(R= 42)") || out.contains("R1(R = 42)"),
        "R replaced with 42: {}", out);
    assert!(reparse_ok(out));
}

#[test]
fn set_parameter_inserts_when_list_missing() {
    let mut host = doc("model M\n  Resistor R1;\nend M;\n");
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "100".into(),
    }).unwrap();
    let out = host.document().source();
    assert!(out.contains("R1(R=100)"), "list created: {}", out);
    assert!(reparse_ok(out));
}

#[test]
fn set_parameter_appends_when_list_present_but_param_missing() {
    let mut host = doc("model M\n  Resistor R1(R=100);\nend M;\n");
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "T".into(),
        value: "293".into(),
    }).unwrap();
    let out = host.document().source();
    assert!(out.contains("R=100"), "original param kept: {}", out);
    assert!(out.contains("T=293"), "new param appended: {}", out);
    assert!(reparse_ok(out));
}

#[test]
fn set_parameter_is_invertible() {
    let src = "model M\n  Resistor R1(R=100);\nend M;\n";
    let mut host = doc(src);
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "42".into(),
    }).unwrap();
    host.undo().unwrap();
    assert_eq!(host.document().source(), src);
}

// ─────────────────────────────────────────────────────────────────────
// 8. Structured change events — consumers patch incrementally
// ─────────────────────────────────────────────────────────────────────

fn collect_changes(host: &DocumentHost<ModelicaDocument>, since: u64) -> Vec<(u64, ModelicaChange)> {
    host.document()
        .changes_since(since)
        .expect("not too far behind")
        .map(|(g, c)| (*g, c.clone()))
        .collect()
}

#[test]
fn add_component_emits_component_added_change() {
    let mut host = doc("model M\nend M;\n");
    host.apply(ModelicaOp::AddComponent {
        class: "M".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "x".into(),
            modifications: vec![],
            placement: None,
        },
    }).unwrap();

    let changes = collect_changes(&host, 0);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, 1);
    assert_eq!(changes[0].1, ModelicaChange::ComponentAdded {
        class: "M".into(),
        name: "x".into(),
    });
}

#[test]
fn add_connection_emits_connection_added_change() {
    let mut host = doc("model M\n  Real a;\n  Real b;\nend M;\n");
    host.apply(ModelicaOp::AddConnection {
        class: "M".into(),
        eq: ConnectEquation {
            from: PortRef::new("a", "p"),
            to: PortRef::new("b", "n"),
            line: None,
        },
    }).unwrap();
    let changes = collect_changes(&host, 0);
    assert_eq!(changes.len(), 1);
    match &changes[0].1 {
        ModelicaChange::ConnectionAdded { class, from, to } => {
            assert_eq!(class, "M");
            assert_eq!(from, &PortRef::new("a", "p"));
            assert_eq!(to, &PortRef::new("b", "n"));
        }
        other => panic!("expected ConnectionAdded, got {:?}", other),
    }
}

#[test]
fn remove_component_emits_component_removed_change() {
    let mut host = doc("model M\n  Real a;\nend M;\n");
    host.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "a".into(),
    }).unwrap();
    let changes = collect_changes(&host, 0);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].1, ModelicaChange::ComponentRemoved {
        class: "M".into(),
        name: "a".into(),
    });
}

#[test]
fn set_parameter_emits_parameter_changed() {
    let mut host = doc("model M\n  Resistor R1(R=100);\nend M;\n");
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "42".into(),
    }).unwrap();
    let changes = collect_changes(&host, 0);
    assert_eq!(changes[0].1, ModelicaChange::ParameterChanged {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "42".into(),
    });
}

#[test]
fn set_placement_emits_placement_changed() {
    let mut host = doc("model M\n  Resistor R1;\nend M;\n");
    host.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "R1".into(),
        placement: Placement::at(10.0, -5.0),
    }).unwrap();
    let changes = collect_changes(&host, 0);
    assert_eq!(changes[0].1, ModelicaChange::PlacementChanged {
        class: "M".into(),
        component: "R1".into(),
        placement: Placement::at(10.0, -5.0),
    });
}

#[test]
fn edit_text_emits_text_replaced() {
    let mut host = doc("model M\nend M;\n");
    host.apply(ModelicaOp::EditText {
        range: 0..5,
        replacement: "class".into(),
    }).unwrap();
    let changes = collect_changes(&host, 0);
    assert_eq!(changes[0].1, ModelicaChange::TextReplaced);
}

#[test]
fn undo_emits_text_replaced() {
    // Undo reapplies the inverse EditText, which is text-level.
    // Consumers handling TextReplaced → rebuild is the contract.
    let mut host = doc("model M\nend M;\n");
    host.apply(ModelicaOp::AddComponent {
        class: "M".into(),
        decl: ComponentDecl {
            type_name: "Real".into(),
            name: "x".into(),
            modifications: vec![],
            placement: None,
        },
    }).unwrap();
    // Snapshot the generation after the forward op.
    let after_forward = host.generation();
    host.undo().unwrap();
    // Undo pushed a new change AFTER the forward one.
    let changes = collect_changes(&host, after_forward);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].1, ModelicaChange::TextReplaced);
}

#[test]
fn consumer_polls_only_new_changes() {
    // Simulate a panel that already saw generations 1 and 2, asks for
    // changes since generation 2, expects only generations >= 3.
    let mut host = doc("model M\nend M;\n");
    for name in ["a", "b", "c", "d"] {
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: name.into(),
                modifications: vec![],
                placement: None,
            },
        }).unwrap();
    }
    let seen_tail: Vec<u64> = host.document()
        .changes_since(2)
        .unwrap()
        .map(|(g, _)| *g)
        .collect();
    assert_eq!(seen_tail, vec![3, 4]);
}

#[test]
fn too_far_behind_returns_none() {
    let mut host = doc("model M\nend M;\n");
    // Push more than CHANGE_HISTORY_CAPACITY changes.
    let cap = lunco_modelica::document::CHANGE_HISTORY_CAPACITY;
    for i in 0..(cap + 10) {
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: format!("v{}", i),
                modifications: vec![],
                placement: None,
            },
        }).unwrap();
    }
    // Asking for changes since generation 0 — too far behind.
    assert!(host.document().changes_since(0).is_none(),
        "consumer that lagged beyond retention must get None");
    // Asking for a recent generation — still serviceable.
    let recent = host.generation() - 2;
    assert!(host.document().changes_since(recent).is_some());
}

#[test]
fn mixed_op_sequence_emits_changes_in_order() {
    let mut host = doc("model M\nend M;\n");
    host.apply(ModelicaOp::AddComponent {
        class: "M".into(),
        decl: ComponentDecl {
            type_name: "Resistor".into(),
            name: "R1".into(),
            modifications: vec![("R".into(), "100".into())],
            placement: None,
        },
    }).unwrap();
    host.apply(ModelicaOp::SetPlacement {
        class: "M".into(),
        name: "R1".into(),
        placement: Placement::at(0.0, 0.0),
    }).unwrap();
    host.apply(ModelicaOp::SetParameter {
        class: "M".into(),
        component: "R1".into(),
        param: "R".into(),
        value: "50".into(),
    }).unwrap();
    host.apply(ModelicaOp::RemoveComponent {
        class: "M".into(),
        name: "R1".into(),
    }).unwrap();

    let changes = collect_changes(&host, 0);
    assert_eq!(changes.len(), 4);
    assert!(matches!(changes[0].1, ModelicaChange::ComponentAdded { .. }));
    assert!(matches!(changes[1].1, ModelicaChange::PlacementChanged { .. }));
    assert!(matches!(changes[2].1, ModelicaChange::ParameterChanged { .. }));
    assert!(matches!(changes[3].1, ModelicaChange::ComponentRemoved { .. }));
    // Generations are consecutive.
    for i in 1..changes.len() {
        assert_eq!(changes[i].0, changes[i - 1].0 + 1);
    }
}

#[test]
fn mixed_op_sequence_undoes_back_to_start() {
    let initial = "model Circuit\nend Circuit;\n";
    let mut host = doc(initial);

    host.apply(ModelicaOp::AddComponent {
        class: "Circuit".into(),
        decl: ComponentDecl {
            type_name: "Resistor".into(),
            name: "R1".into(),
            modifications: vec![("R".into(), "100".into())],
            placement: Some(Placement::at(0.0, 0.0)),
        },
    }).unwrap();
    host.apply(ModelicaOp::AddComponent {
        class: "Circuit".into(),
        decl: ComponentDecl {
            type_name: "Capacitor".into(),
            name: "C1".into(),
            modifications: vec![("C".into(), "0.001".into())],
            placement: Some(Placement::at(20.0, 0.0)),
        },
    }).unwrap();
    host.apply(ModelicaOp::AddConnection {
        class: "Circuit".into(),
        eq: ConnectEquation {
            from: PortRef::new("R1", "n"),
            to: PortRef::new("C1", "p"),
            line: None,
        },
    }).unwrap();

    // Full undo → identical to starting source.
    for _ in 0..3 { host.undo().unwrap(); }
    assert_eq!(host.document().source(), initial);

    // Redo → identical to post-op state.
    for _ in 0..3 { host.redo().unwrap(); }
    assert!(reparse_ok(host.document().source()));
    let ast = host.document().ast().ast().unwrap();
    let circuit = ast.classes.get("Circuit").unwrap();
    assert_eq!(circuit.components.len(), 2);
    assert_eq!(circuit.equations.len(), 1);
}
