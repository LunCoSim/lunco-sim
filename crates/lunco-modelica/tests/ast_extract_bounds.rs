//! Regression: the workbench's Telemetry slider must clamp
//! `valve.opening` to its declared `min=0, max=100` (MLS §4.8.4).
//! This previously silently failed because the bounds extractor
//! gated on the COMPONENT's causality, but `RealInput` carries the
//! input causality on the connector TYPE, not the component — so
//! every input typed via a `RealInput`/`RealOutput`-style connector
//! had its bounds invisibly dropped. Bounds now live on the
//! per-doc [`lunco_modelica::index::ModelicaIndex`] via the
//! component's `modifications` map; this test verifies the same
//! end-to-end guarantee through that surface.

fn src() -> &'static str {
    lunco_modelica::models::get_model("AnnotatedRocketStage.mo")
        .expect("bundled AnnotatedRocketStage.mo")
}

#[test]
fn bounds_extraction_finds_valve_opening_min_max() {
    let ast = rumoca_phase_parse::parse_to_ast(src(), "AnnotatedRocketStage.mo")
        .expect("parses");
    let mut index = lunco_modelica::index::ModelicaIndex::new();
    index.rebuild_from_ast(&ast, src());
    let entry = index
        .find_component_by_leaf("opening")
        .expect("opening not in index");
    let mn: Option<f64> = entry
        .modifications
        .get("min")
        .and_then(|s| s.parse().ok());
    let mx: Option<f64> = entry
        .modifications
        .get("max")
        .and_then(|s| s.parse().ok());
    assert_eq!(mn, Some(0.0), "expected opening.min=0, got {mn:?}");
    assert_eq!(mx, Some(100.0), "expected opening.max=100, got {mx:?}");
}
