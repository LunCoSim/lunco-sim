//! Regression: the workbench's Telemetry slider must clamp
//! `valve.opening` to its declared `min=0, max=1` (MLS §4.8.4).
//! This previously silently failed because the bounds extractor
//! gated on the COMPONENT's causality, but `RealInput` carries the
//! input causality on the connector TYPE, not the component — so
//! every input typed via a `RealInput`/`RealOutput`-style connector
//! had its bounds invisibly dropped.

const SRC: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

#[test]
fn bounds_extraction_finds_valve_opening_min_max() {
    let ast = rumoca_phase_parse::parse_to_ast(SRC, "AnnotatedRocketStage.mo")
        .expect("parses");
    let bounds = lunco_modelica::ast_extract::extract_parameter_bounds_from_ast(&ast);
    let (mn, mx) = bounds
        .get("opening")
        .copied()
        .unwrap_or_else(|| panic!("opening not in bounds; keys: {:?}", bounds.keys().collect::<Vec<_>>()));
    assert_eq!(mn, Some(0.0), "expected opening.min=0, got {mn:?}");
    assert_eq!(mx, Some(1.0), "expected opening.max=1, got {mx:?}");
}
