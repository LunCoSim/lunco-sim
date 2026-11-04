use super::ModelicaCompiler;
use lunco_modelica_ast::resolved_interface::{ModelicaCausality, ModelicaScalarBound};

#[test]
fn source_edit_interface_admits_exact_literals_and_resolved_access() {
    let source = "model EditProbe
      parameter Integer count=2;
      parameter Real gain=2*3;
      final parameter Real fixedGain=4;
      protected parameter Real hidden=7;
      public output Real value;
      equation value=count+gain+fixedGain+hidden;
      end EditProbe;";
    let mut compiler = ModelicaCompiler::new();
    let compiled = compiler
        .compile_str_after_source_root_admission("EditProbe", source, "edit_probe.mo", &[])
        .unwrap();
    let interface = compiler
        .interface_for_parameter_edits(
            "EditProbe",
            source,
            "edit_probe.mo",
            &[],
            &compiled.dae,
            &[],
        )
        .unwrap();
    assert_eq!(
        interface.declarations["count"]
            .parameter_edit_source(3.0)
            .unwrap(),
        "3"
    );
    assert!(
        interface.declarations["count"]
            .parameter_edit_source(1.5)
            .unwrap_err()
            .contains("Integer")
    );
    assert!(
        interface.declarations["gain"]
            .parameter_edit_source(9.0)
            .is_ok()
    );
    for name in ["fixedGain", "hidden"] {
        assert!(
            interface.declarations[name]
                .parameter_edit_source(9.0)
                .unwrap_err()
                .contains("public, non-final")
        );
    }
}

#[test]
fn checked_interface_owns_conditional_presence_units_bounds_and_state_observables() {
    let source = r#"
package P
  connector Signal = input Real(unit="V");
  model Base
    parameter Boolean enabled=true;
    parameter Integer n;
    parameter Real k=2*3;
    constant Real calibration=7;
    Signal u(min=0,max=k) if enabled;
    Real x[n](each start=0,each fixed=true,each max=n);
    output Real force_y=k;
    output Real sample;
  equation
    der(x)=fill(k,n);
    sample=if enabled then u+x[1] else x[1];
  end Base;
  model Derived
    extends Base;
  end Derived;
end P;
"#;
    let mut compiler = ModelicaCompiler::new();
    let authored = vec![
        ("n".into(), 3.0),
        ("k".into(), 9.0),
        ("u".into(), 4.0),
        ("force_y".into(), 0.0),
    ];
    let external = ["force_y".to_string()].into_iter().collect();
    let pre = compiler
        .admit_authored_bindings_after_source_root_admission(
            "P.Derived",
            source,
            "interface.mo",
            &[],
            &authored,
            &external,
        )
        .expect("only parameter candidates are needed before compilation");
    assert!(pre.initial_inputs.is_empty());
    assert_eq!(
        pre.parameter_overrides,
        vec![("n".into(), 3.0), ("k".into(), 9.0)]
    );
    let compiled = compiler
        .compile_str_multi_after_source_root_admission(
            "P.Derived",
            source,
            "interface.mo",
            &[],
            &pre.parameter_overrides,
        )
        .expect("unbound dimension is bound before the first strict compile");
    let interface = ModelicaCompiler::interface_from_checked_dae(
        "P.Derived",
        &compiled.dae,
        &pre.parameter_overrides,
    )
    .expect("checked scalar facts");
    assert_eq!(
        interface.declarations["u"].causality,
        ModelicaCausality::Input
    );
    assert!(
        !interface.declarations["u"].observable,
        "inputs do not advertise participant output samples"
    );
    assert!(
        !interface.declarations["k"].observable,
        "parameters remain in the parameter inventory"
    );
    assert!(
        !interface.declarations["calibration"].observable,
        "constants have no live output sample"
    );
    assert!(
        interface.declarations["force_y"].observable,
        "inherited numeric outputs remain published"
    );
    assert!(interface.declarations["sample"].observable);
    assert_eq!(interface.declarations["u"].unit.as_deref(), Some("V"));
    assert_eq!(
        interface.declarations["u"].maximum,
        Some(ModelicaScalarBound::Known(9.0))
    );
    assert!(interface.declarations["x[3]"].observable);
    assert_eq!(
        interface.declarations["x[3]"].maximum,
        Some(ModelicaScalarBound::Known(3.0))
    );
    assert_eq!(
        interface.declarations["x[3]"].causality,
        ModelicaCausality::None
    );
    assert!(interface.declarations["k"].is_tunable);
    let admitted = ModelicaCompiler::admit_authored_bindings_from_checked_dae(
        &compiled.dae,
        &authored,
        &external,
    )
    .expect("native physical sink remains outside Modelica bindings");
    assert_eq!(admitted.initial_inputs, vec![("u".into(), 4.0)]);
    for invalid in [
        ("missing".into(), 1.0),
        ("n".into(), 3.5),
        ("u".into(), f64::NAN),
    ] {
        assert!(
            ModelicaCompiler::admit_authored_bindings_from_checked_dae(
                &compiled.dae,
                &[invalid],
                &external
            )
            .is_err()
        );
    }
    let changed = vec![("n".into(), 3.0), ("k".into(), 8.0)];
    let reused =
        ModelicaCompiler::interface_from_checked_dae("P.Derived", &compiled.dae, &changed).unwrap();
    assert_eq!(
        reused.declarations["u"].maximum,
        Some(ModelicaScalarBound::Known(8.0))
    );
    let disabled = vec![
        ("n".into(), 3.0),
        ("k".into(), 9.0),
        ("enabled".into(), 0.0),
    ];
    let compiled = compiler
        .compile_str_multi_after_source_root_admission(
            "P.Derived",
            source,
            "interface.mo",
            &[],
            &disabled,
        )
        .expect("compiler resolves disabled conditional presence");
    let interface =
        ModelicaCompiler::interface_from_checked_dae("P.Derived", &compiled.dae, &disabled)
            .unwrap();
    assert!(!interface.declarations.contains_key("u"));
}

#[test]
fn checked_parameter_paths_preserve_quoted_names_and_indexed_overlays() {
    let source = r#"
model Child
  parameter Real gain=2;
  input Real internal_input=0;
  output Real y=gain+internal_input;
end Child;
model References
  parameter Real gains[2]={1,2};
  parameter Real 'quoted.gain'=3;
  Child child;
  output Real y=gains[2]+'quoted.gain'+child.y;
end References;
"#;
    let mut compiler = ModelicaCompiler::new();
    let values = vec![
        ("gains[2]".into(), 8.0),
        ("'quoted.gain'".into(), 9.0),
        ("child.gain".into(), 4.0),
    ];
    assert_eq!(
        ModelicaCompiler::parameter_binding_names(&values).unwrap(),
        vec!["'quoted.gain'", "child.gain"]
    );
    let compiled = compiler
        .compile_str_multi_after_source_root_admission(
            "References",
            source,
            "references.mo",
            &[],
            &values,
        )
        .expect("unindexed root modifications and indexed tunable overlays coexist");
    let admitted = ModelicaCompiler::admit_authored_bindings_from_checked_dae(
        &compiled.dae,
        &values,
        &Default::default(),
    )
    .expect("checked parameter coordinates");
    assert_eq!(admitted.parameter_overrides, values);
    assert!(
        ModelicaCompiler::admit_authored_bindings_from_checked_dae(
            &compiled.dae,
            &[("child.internal_input".into(), 1.0)],
            &Default::default(),
        )
        .is_err(),
        "nested inputs require defining equations, not runtime input overlays"
    );
    assert!(
        ModelicaCompiler::validate_parameter_bindings(&compiled.dae, &[("gains[3]".into(), 7.0)],)
            .is_err(),
        "checked scalar identity rejects an out-of-range element"
    );
}

#[test]
fn initialization_dependent_bound_is_distinct_from_an_absent_bound() {
    let source = "model Deferred parameter Real limit(fixed=false); input Real u(max=limit); output Real y=u; initial equation limit=5; end Deferred;";
    let compiled = ModelicaCompiler::new()
        .compile_str("Deferred", source, "deferred.mo")
        .expect("initialization determines the bound parameter");
    let interface = ModelicaCompiler::interface_from_checked_dae("Deferred", &compiled.dae, &[])
        .expect("the bound remains explicitly dependent on initialization");
    assert_eq!(interface.declarations["u"].minimum, None);
    assert_eq!(
        interface.declarations["u"].maximum,
        Some(ModelicaScalarBound::InitializationDependent)
    );
}

#[test]
fn checked_input_groups_preserve_shape_order_and_parameter_binding_defaults() {
    use lunco_modelica_ast::resolved_interface::{ModelicaInputDefaults, ModelicaScalarType};
    let source = r#"
model InputGroups
  parameter Real k=2;
  parameter Real limit(fixed=false);
  input Real u[2,2]=[k,k+1;k+2,k+3];
  input Real missing[2](each start=7);
  input Real deferred[2]={limit,limit+1};
  input Boolean flags[2]={true,false};
  input Real scalar=k;
  output Real y=sum(u)+sum(missing)+sum(deferred)+scalar;
initial equation
  limit=5;
end InputGroups;
"#;
    let mut compiler = ModelicaCompiler::new();
    let parameters = vec![("k".into(), 9.0)];
    let compiled = compiler
        .compile_str_multi_after_source_root_admission(
            "InputGroups",
            source,
            "input-groups.mo",
            &[],
            &parameters,
        )
        .expect("checked input bindings compile without numerical samples");
    let interface =
        ModelicaCompiler::interface_from_checked_dae("InputGroups", &compiled.dae, &parameters)
            .expect("checked group metadata");
    let u = interface
        .input_groups
        .iter()
        .find(|group| group.name == "u")
        .unwrap();
    assert_eq!(u.dimensions, vec![2, 2]);
    assert_eq!(u.scalar_names, vec!["u[1,1]", "u[1,2]", "u[2,1]", "u[2,2]"]);
    assert_eq!(u.scalar_type, Some(ModelicaScalarType::Real));
    assert!(!u.aggregate_name_is_indexed);
    assert_eq!(
        u.default_values,
        Some(ModelicaInputDefaults::Known(vec![9.0, 10.0, 11.0, 12.0]))
    );
    let missing = interface
        .input_groups
        .iter()
        .find(|group| group.name == "missing")
        .unwrap();
    assert_eq!(
        missing.default_values, None,
        "start guesses do not supply missing input defaults"
    );
    let deferred = interface
        .input_groups
        .iter()
        .find(|group| group.name == "deferred")
        .unwrap();
    assert_eq!(
        deferred.default_values,
        Some(ModelicaInputDefaults::InitializationDependent)
    );
    let flags = interface
        .input_groups
        .iter()
        .find(|group| group.name == "flags")
        .unwrap();
    assert_eq!(flags.scalar_type, Some(ModelicaScalarType::Boolean));
    assert_eq!(
        flags.default_values,
        Some(ModelicaInputDefaults::Known(vec![1.0, 0.0]))
    );
    let scalar = interface
        .input_groups
        .iter()
        .find(|group| group.name == "scalar")
        .unwrap();
    assert!(scalar.dimensions.is_empty());
    assert_eq!(scalar.scalar_names, vec!["scalar"]);
    let changed = vec![("k".into(), 8.0)];
    let reused =
        ModelicaCompiler::interface_from_checked_dae("InputGroups", &compiled.dae, &changed)
            .unwrap();
    let u = reused
        .input_groups
        .iter()
        .find(|group| group.name == "u")
        .unwrap();
    assert_eq!(
        u.default_values,
        Some(ModelicaInputDefaults::Known(vec![8.0, 9.0, 10.0, 11.0]))
    );
}
