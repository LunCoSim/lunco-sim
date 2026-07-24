//! **The splice contract: an op may only change the bytes it means to change.**
//!
//! Structural ops used to be turned into text by re-emitting the whole class
//! through rumoca's `to_modelica()`. That round-trip is lossy, so a single
//! canvas drag rewrote every sibling declaration in the class:
//!
//! ```text
//! parameter Real m(start = 1, min = 0, unit = "kg") = 5   ->   m(min = 0, unit = "kg") = 1
//! parameter Real k = 2.0                                  ->   k = 0.0
//! ```
//!
//! Wrong numbers, no error, from moving an icon. These tests pin the fix: for
//! each op, every line the op did not target must be byte-identical afterwards.
//! See `docs/architecture/29-rumoca-workarounds.md` §5.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::{self, Placement, PortRef};

/// A class carrying everything the old emitter destroyed: a multi-modifier
/// declaration with a binding *and* a description, a plain bound parameter,
/// comments, and a connect with a routed line.
const SRC: &str = r#"within Foo.Bar;

model Sys "the system"
  // a comment on the mass
  parameter Real m(start = 1, min = 0, unit = "kg") = 5 "the mass";
  parameter Real k = 2.0 "stiffness";
  Real x(start = 0.1);
  Real a annotation(Placement(transformation(extent={{-10,-10},{10,10}})));
  Real b;
equation
  // conservation
  der(x) = -k * x / m;
  connect(a, b) annotation(Line(points={{0,0},{1,1}}, color={0,0,255}));
end Sys;
"#;

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

/// Apply `op` and return the resulting source.
fn apply(op: ModelicaOp) -> String {
    let mut h = host(SRC);
    h.apply(op).expect("op applies");
    let out = h.document().source().to_string();
    // Whatever we spliced, the result must still parse.
    rumoca_phase_parse::parse_to_ast(&out, "test.mo")
        .unwrap_or_else(|e| panic!("post-op source does not parse: {e:?}\n=== src ===\n{out}"));
    out
}

/// Every line of the original that is not named in `touched` must survive
/// byte-for-byte.
fn assert_only_touched(after: &str, touched: &[&str]) {
    for line in SRC.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || touched.iter().any(|t| trimmed.contains(t)) {
            continue;
        }
        assert!(
            after.lines().any(|l| l == line),
            "op rewrote a line it did not target.\n  lost: {line:?}\n=== after ===\n{after}"
        );
    }
}

#[test]
fn set_placement_leaves_every_other_declaration_alone() {
    let after = apply(ModelicaOp::SetPlacement {
        class: "Foo.Bar.Sys".into(),
        name: "a".into(),
        placement: Placement::at(20.0, 30.0),
    });
    // Only `a`'s line may change.
    assert_only_touched(&after, &["Real a annotation"]);
    // And the values that used to be destroyed are intact.
    assert!(
        after.contains(r#"parameter Real m(start = 1, min = 0, unit = "kg") = 5 "the mass";"#),
        "`m` was rewritten:\n{after}"
    );
    assert!(
        after.contains(r#"parameter Real k = 2.0 "stiffness";"#),
        "`k` was rewritten:\n{after}"
    );
    assert!(
        after.contains("// a comment on the mass"),
        "comment lost:\n{after}"
    );
    assert!(after.contains("// conservation"), "comment lost:\n{after}");
    // The placement itself actually moved.
    assert!(
        after.contains("{{10,20},{30,40}}"),
        "placement not written:\n{after}"
    );
}

#[test]
fn set_parameter_writes_one_modifier_and_keeps_the_others() {
    let after = apply(ModelicaOp::SetParameter {
        class: "Foo.Bar.Sys".into(),
        component: "m".into(),
        param: "min".into(),
        value: "-3".into(),
    });
    assert_only_touched(&after, &["parameter Real m"]);
    assert!(
        after.contains(r#"parameter Real m(start = 1, min = -3, unit = "kg") = 5 "the mass";"#),
        "expected only `min` to change:\n{after}"
    );
}

#[test]
fn set_parameter_binding_keeps_the_start_modifier() {
    // The exact conflation the old emitter got wrong: writing the binding must
    // not swallow `start`, and writing `start` must not swallow the binding.
    let after = apply(ModelicaOp::SetParameter {
        class: "Foo.Bar.Sys".into(),
        component: "m".into(),
        param: "".into(),
        value: "42".into(),
    });
    assert!(
        after.contains(r#"parameter Real m(start = 1, min = 0, unit = "kg") = 42 "the mass";"#),
        "binding write corrupted the declaration:\n{after}"
    );

    let after = apply(ModelicaOp::SetParameter {
        class: "Foo.Bar.Sys".into(),
        component: "m".into(),
        param: "start".into(),
        value: "9".into(),
    });
    assert!(
        after.contains(r#"parameter Real m(start = 9, min = 0, unit = "kg") = 5 "the mass";"#),
        "start write corrupted the declaration:\n{after}"
    );
}

#[test]
fn set_parameter_adds_a_modifier_list_when_there_is_none() {
    let after = apply(ModelicaOp::SetParameter {
        class: "Foo.Bar.Sys".into(),
        component: "k".into(),
        param: "min".into(),
        value: "0".into(),
    });
    assert_only_touched(&after, &["parameter Real k"]);
    assert!(
        after.contains(r#"parameter Real k(min = 0) = 2.0 "stiffness";"#),
        "expected a fresh modifier list on `k`:\n{after}"
    );
}

#[test]
fn add_component_inserts_and_touches_nothing_else() {
    let after = apply(ModelicaOp::AddComponent {
        class: "Foo.Bar.Sys".into(),
        decl: pretty::ComponentDecl {
            type_name: "Real".into(),
            name: "fresh".into(),
            modifications: Vec::new(),
            placement: None,
        },
    });
    assert_only_touched(&after, &[]);
    assert!(
        after.contains("Real fresh;"),
        "component not added:\n{after}"
    );
}

#[test]
fn remove_component_deletes_only_its_own_line() {
    let after = apply(ModelicaOp::RemoveComponent {
        class: "Foo.Bar.Sys".into(),
        name: "b".into(),
    });
    assert_only_touched(&after, &["Real b;"]);
    assert!(
        !after.contains("Real b;"),
        "component not removed:\n{after}"
    );
    // The neighbouring declaration is untouched, not merged into the gap.
    assert!(
        after.contains("Real a annotation("),
        "neighbour damaged:\n{after}"
    );
}

#[test]
fn set_connection_line_actually_writes_the_route() {
    // Under the AST-only scheme this op was a silent no-op: rumoca's
    // `Equation::Connect` has no annotation field, so there was nothing to
    // mutate and the whole class was rewritten for nothing.
    let after = apply(ModelicaOp::SetConnectionLine {
        class: "Foo.Bar.Sys".into(),
        from: PortRef {
            component: "a".into(),
            port: String::new(),
        },
        to: PortRef {
            component: "b".into(),
            port: String::new(),
        },
        points: vec![(5.0, 6.0), (7.0, 8.0)],
    });
    assert_only_touched(&after, &["connect(a, b)"]);
    assert!(
        after.contains("points={{5,6},{7,8}}"),
        "line route was not written:\n{after}"
    );
    // The hand-authored colour on the same Line survives the re-route.
    assert!(
        after.contains("color={0,0,255}"),
        "re-routing dropped the authored colour:\n{after}"
    );
}

#[test]
fn reverse_connection_swaps_endpoints_only() {
    let after = apply(ModelicaOp::ReverseConnection {
        class: "Foo.Bar.Sys".into(),
        from: PortRef {
            component: "a".into(),
            port: String::new(),
        },
        to: PortRef {
            component: "b".into(),
            port: String::new(),
        },
    });
    assert_only_touched(&after, &["connect(a, b)"]);
    assert!(
        after.contains("connect(b, a)"),
        "endpoints not swapped:\n{after}"
    );
    assert!(
        after.contains("points={{0,0},{1,1}}"),
        "swap disturbed the line annotation:\n{after}"
    );
}

#[test]
fn add_connection_appends_to_the_equation_section() {
    let after = apply(ModelicaOp::AddConnection {
        class: "Foo.Bar.Sys".into(),
        eq: pretty::ConnectEquation {
            from: PortRef {
                component: "x".into(),
                port: String::new(),
            },
            to: PortRef {
                component: "k".into(),
                port: String::new(),
            },
            line: None,
        },
    });
    assert_only_touched(&after, &[]);
    assert!(
        after.contains("connect(x, k);"),
        "connection not added:\n{after}"
    );
}

#[test]
fn set_experiment_adds_a_class_annotation_without_disturbing_the_body() {
    let after = apply(ModelicaOp::SetExperimentAnnotation {
        class: "Foo.Bar.Sys".into(),
        start_time: 0.0,
        stop_time: 10.0,
        tolerance: 1e-6,
        interval: 0.01,
    });
    assert_only_touched(&after, &[]);
    assert!(
        after.contains("annotation(experiment(StartTime=0, StopTime=10"),
        "experiment annotation not written:\n{after}"
    );
}
