//! Regression for extracting bounds from a typed Modelica input.
//!
//! This is a pure indexer contract, so keep its source inline. Shipped model
//! behavior belongs to authored Rhai/USDA scenarios and must not make this
//! Rust test depend on the assets tree.

const SOURCE: &str = r#"
model BoundsFixture
  model Valve
    input Real opening(min = 0, max = 100);
  end Valve;
  Valve valve;
end BoundsFixture;
"#;

#[test]
fn bounds_extraction_finds_valve_opening_min_max() {
    let ast = lunco_modelica_ast::parse_to_ast(SOURCE, "bounds_fixture.mo").expect("parses");
    let mut index = lunco_modelica_core::index::ModelicaIndex::new();
    index.rebuild_from_ast(&ast, SOURCE);
    let entry = index
        .find_component_by_leaf("opening")
        .expect("opening not in index");
    let mn: Option<f64> = entry.modifications.get("min").and_then(|s| s.parse().ok());
    let mx: Option<f64> = entry.modifications.get("max").and_then(|s| s.parse().ok());
    assert_eq!(mn, Some(0.0), "expected opening.min=0, got {mn:?}");
    assert_eq!(mx, Some(100.0), "expected opening.max=100, got {mx:?}");
}

#[test]
fn description_comments_populate_index_entries() {
    let source = r#"
model DescriptionFixture "A model description"
  parameter Real max_rate = 1.0 "mass flow rate";
  input Real throttle = 0.0 "Throttle command";
  Real propellant "Propellant remaining";
  output Real thrust "Thrust output";
equation
  propellant = max_rate;
  thrust = throttle * max_rate;
end DescriptionFixture;
"#;
    let ast = lunco_modelica_ast::parse_to_ast(source, "description_fixture.mo")
        .expect("description fixture parses");
    let mut index = lunco_modelica_core::index::ModelicaIndex::new();
    index.rebuild_from_ast(&ast, source);
    for (name, needle) in [
        ("max_rate", "mass flow"),
        ("throttle", "Throttle"),
        ("propellant", "Propellant"),
        ("thrust", "Thrust"),
    ] {
        let entry = index
            .find_component_by_leaf(name)
            .unwrap_or_else(|| panic!("no component '{name}' in index"));
        assert!(
            entry.description.contains(needle),
            "'{name}' description should contain '{needle}', got: {:?}",
            entry.description
        );
    }
}
