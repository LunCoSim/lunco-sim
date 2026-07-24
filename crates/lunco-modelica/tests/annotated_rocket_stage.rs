//! Sanity test for the visual-fixture file `assets/models/AnnotatedRocketStage.mo`.
//!
//! Verifies that the slice-1 annotation extractor pulls real data out of
//! the file the user will visually check in the canvas. If this test
//! fails, the fixture and the extractor have drifted apart — fix
//! whichever side is wrong before touching the renderer.

use lunco_modelica::annotations::{extract_diagram, extract_icon, extract_placement, GraphicItem};
use rumoca_phase_parse::parse_to_ast;

fn source() -> &'static str {
    lunco_modelica::models::get_model("AnnotatedRocketStage.mo")
        .expect("bundled AnnotatedRocketStage.mo")
}

#[test]
fn fixture_parses_and_extracts() {
    let ast = parse_to_ast(source(), "AnnotatedRocketStage.mo").expect("parse");
    let pkg = ast
        .classes
        .iter()
        .next()
        .map(|(_, c)| c)
        .expect("package class");

    // The package should contain the vendor-annotation record
    // package plus the four physical classes and the top-level
    // RocketStage assembly.
    let leaf_names: Vec<&str> = pkg.classes.keys().map(|k| k.as_str()).collect();
    for expected in [
        "LunCoAnnotations",
        "Tank",
        "Valve",
        "Engine",
        "Airframe",
        "RocketStage",
    ] {
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
            GraphicItem::Ellipse(_) | GraphicItem::Bitmap(_) => {}
        }
    }
    assert_eq!((rects, polys, lines, texts), (1, 1, 2, 1));

    // RocketStage: each component should resolve a Placement.
    let stage = &pkg.classes["RocketStage"];
    for cname in ["tank", "valve", "engine", "airframe"] {
        let comp = &stage.components[cname];
        let p = extract_placement(&comp.annotation)
            .unwrap_or_else(|| panic!("{cname} missing Placement"));
        // sanity — extents are non-degenerate
        assert!(p.transformation.extent.p1 != p.transformation.extent.p2);
    }

    // RocketStage Diagram: header text + 5 Rectangle/Text placeholder
    // pairs for the plot regions (one per __LunCo plot node).
    let diag = extract_diagram(&stage.annotation).expect("stage Diagram");
    let text_count = diag
        .graphics
        .iter()
        .filter(|g| matches!(g, GraphicItem::Text(_)))
        .count();
    let rect_count = diag
        .graphics
        .iter()
        .filter(|g| matches!(g, GraphicItem::Rectangle(_)))
        .count();
    assert_eq!(text_count, 6, "1 header + 5 plot labels");
    assert_eq!(rect_count, 5, "5 plot region rectangles");

    // The vendor `__LunCo(plotNodes=…)` annotation is orthogonal to
    // `Diagram` and is extracted separately. It should carry one typed
    // `LunCoAnnotations.PlotNode` per plot region, `signal=` preserved.
    let plot_nodes = lunco_modelica::annotations::extract_lunco_plot_nodes(&stage.annotation);
    assert_eq!(plot_nodes.len(), 5);
    let signals: Vec<&str> = plot_nodes.iter().map(|p| p.signal.as_str()).collect();
    for expected in [
        "tank.m",
        "airframe.altitude",
        "airframe.velocity",
        "airframe.thrust_in",
        "airframe.acceleration",
    ] {
        assert!(
            signals.contains(&expected),
            "missing plot signal `{expected}` (have {signals:?})"
        );
    }
}
