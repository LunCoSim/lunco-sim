//! Sanity test for the visual-fixture file `assets/models/AnnotatedRocketStage.mo`.
//!
//! Verifies that the slice-1 annotation extractor pulls real data out of
//! the file the user will visually check in the canvas. If this test
//! fails, the fixture and the extractor have drifted apart — fix
//! whichever side is wrong before touching the renderer.

use lunco_modelica::annotations::{
    extract_diagram, extract_icon, extract_placement, GraphicItem,
};
use rumoca_phase_parse::parse_to_ast;

const SOURCE: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

#[test]
fn fixture_parses_and_extracts() {
    let ast = parse_to_ast(SOURCE, "AnnotatedRocketStage.mo").expect("parse");
    let pkg = ast
        .classes
        .iter()
        .next()
        .map(|(_, c)| c)
        .expect("package class");

    // The package should contain Engine, Tank, Gimbal, RocketStage.
    let leaf_names: Vec<&str> = pkg.classes.keys().map(|k| k.as_str()).collect();
    for expected in ["Engine", "Tank", "Gimbal", "RocketStage"] {
        assert!(
            leaf_names.contains(&expected),
            "missing class {expected} (have {leaf_names:?})"
        );
    }

    // Engine icon: 1 rect, 1 polygon, 2 lines, 1 text.
    let engine = &pkg.classes["Engine"];
    let engine_icon = extract_icon(&engine.annotation).expect("engine Icon");
    let mut rects = 0;
    let mut polys = 0;
    let mut lines = 0;
    let mut texts = 0;
    for g in &engine_icon.graphics {
        match g {
            GraphicItem::Rectangle(_) => rects += 1,
            GraphicItem::Polygon(_) => polys += 1,
            GraphicItem::Line(_) => lines += 1,
            GraphicItem::Text(_) => texts += 1,
        }
    }
    assert_eq!((rects, polys, lines, texts), (1, 1, 2, 1));

    // RocketStage: each component should resolve a Placement.
    let stage = &pkg.classes["RocketStage"];
    for cname in ["tank", "engine", "gimbal"] {
        let comp = &stage.components[cname];
        let p = extract_placement(&comp.annotation)
            .unwrap_or_else(|| panic!("{cname} missing Placement"));
        // sanity — extents are non-degenerate
        assert!(p.transformation.extent.p1 != p.transformation.extent.p2);
    }

    // Gimbal carries rotation=15.
    let gimbal_p =
        extract_placement(&stage.components["gimbal"].annotation).expect("gimbal placement");
    assert_eq!(gimbal_p.transformation.rotation, 15.0);

    // RocketStage Diagram should have 1 text + 2 lines.
    let diag = extract_diagram(&stage.annotation).expect("stage Diagram");
    let text_count = diag
        .graphics
        .iter()
        .filter(|g| matches!(g, GraphicItem::Text(_)))
        .count();
    let line_count = diag
        .graphics
        .iter()
        .filter(|g| matches!(g, GraphicItem::Line(_)))
        .count();
    assert_eq!(text_count, 1);
    assert_eq!(line_count, 2);
}
