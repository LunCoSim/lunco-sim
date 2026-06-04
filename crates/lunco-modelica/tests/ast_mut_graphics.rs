//! TDD contract tests for batch-3b graphics-tree helpers:
//! `add_plot_node`, `remove_plot_node`, `set_plot_node_extent`,
//! `set_plot_node_title`, `set_diagram_text_extent`,
//! `set_diagram_text_string`, `remove_diagram_text`,
//! `add_named_graphic`.
//!
//! Same pattern: parse → mutate → emit → reparse → assert. Headless.
//! Integration via `host.apply` lives in `tests/op_to_patch_graphics.rs`.

use std::sync::Arc;

use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
use lunco_modelica::ast_mut::{self, AstMutError};
use lunco_modelica::document::{ModelicaDocument, ModelicaOp, SyntaxCache};
use lunco_modelica::pretty::{
    FillPattern, GraphicSpec, LinePattern, LunCoPlotNodeSpec,
};
use rumoca_phase_parse::parse_to_ast;
use rumoca_compile::parsing::ast::Expression;

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

fn plot(signal: &str, title: &str) -> LunCoPlotNodeSpec {
    LunCoPlotNodeSpec {
        x1: -10.0,
        y1: -10.0,
        x2: 10.0,
        y2: 10.0,
        signal: signal.to_string(),
        title: title.to_string(),
    }
}

fn rect() -> GraphicSpec {
    GraphicSpec::Rectangle {
        x1: -50.0,
        y1: -50.0,
        x2: 50.0,
        y2: 50.0,
        line_color: [0, 0, 0],
        fill_color: [255, 255, 255],
        fill_pattern: FillPattern::Solid,
    }
}

#[allow(dead_code)]
fn line() -> GraphicSpec {
    GraphicSpec::Line {
        points: vec![(0.0, 0.0), (10.0, 10.0)],
        color: [0, 0, 0],
        thickness: 0.25,
        pattern: LinePattern::Solid,
    }
}

/// Count `LunCoAnnotations.PlotNode(...)` records (or bare
/// `PlotNode(...)`) inside the class's
/// `annotation(__LunCo(plotNodes={...}))` array.
fn count_plot_nodes(class_src: &str, class: &str) -> usize {
    let sd = parse_to_ast(class_src, "test.mo").unwrap();
    let class_def = sd.classes.get(class).expect("class present");
    for entry in &class_def.annotation {
        let Expression::ClassModification { target, modifications, .. } = entry else { continue };
        if !(target.parts.len() == 1 && &*target.parts[0].ident.text == "__LunCo") {
            continue;
        }
        for m in modifications {
            let Expression::Modification { target: t, value, .. } = m else { continue };
            if !(t.parts.len() == 1 && &*t.parts[0].ident.text == "plotNodes") {
                continue;
            }
            let Expression::Array { elements, .. } = value.as_ref() else { continue };
            return elements
                .iter()
                .filter(|e| {
                    let parts = match e {
                        Expression::FunctionCall { comp, .. } => &comp.parts,
                        Expression::ClassModification { target, .. } => &target.parts,
                        _ => return false,
                    };
                    parts.last().map(|p| &*p.ident.text) == Some("PlotNode")
                })
                .count();
        }
    }
    0
}

// ─────────────────────────────────────────────────────────────────────
// add_plot_node
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_plot_node_creates_diagram_section() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_plot_node(class, &plot("x", "X")).expect("add_plot_node");
    let regen = sd.to_modelica();
    assert_eq!(
        count_plot_nodes(&regen, "M"),
        1,
        "regen:\n{regen}"
    );
}

#[test]
fn add_plot_node_replaces_same_signal() {
    // Adding the same signal twice should produce one entry, not two —
    // the helper de-dupes by signal.
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_plot_node(class, &plot("x", "old")).unwrap();
    ast_mut::add_plot_node(class, &plot("x", "new")).unwrap();
    let regen = sd.to_modelica();
    assert_eq!(count_plot_nodes(&regen, "M"), 1);
    assert!(regen.contains("\"new\""), "new title not present:\n{regen}");
}

// ─────────────────────────────────────────────────────────────────────
// remove_plot_node
// ─────────────────────────────────────────────────────────────────────

#[test]
fn remove_plot_node_drops_matching_entry() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_plot_node(class, &plot("x", "")).unwrap();
    ast_mut::add_plot_node(class, &plot("y", "")).unwrap();
    ast_mut::remove_plot_node(class, "x").expect("remove");
    let regen = sd.to_modelica();
    assert_eq!(count_plot_nodes(&regen, "M"), 1);
    assert!(regen.contains("\"y\""), "y dropped");
    assert!(!regen.contains("\"x\""), "x still present");
}

#[test]
fn remove_plot_node_unknown_returns_error() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::remove_plot_node(class, "missing").unwrap_err();
    assert!(matches!(
        err,
        AstMutError::PlotNodeNotFound { signal, .. } if signal == "missing"
    ));
}

// ─────────────────────────────────────────────────────────────────────
// set_plot_node_extent / set_plot_node_title
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_plot_node_extent_updates_in_place() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_plot_node(class, &plot("x", "")).unwrap();
    ast_mut::set_plot_node_extent(class, "x", 100.0, 200.0, 300.0, 400.0).expect("extent");
    let regen = sd.to_modelica();
    // Numbers should appear; emitter may format as `100` or `100.0`.
    assert!(
        regen.contains("100") && regen.contains("400"),
        "extent not updated:\n{regen}"
    );
    assert_eq!(count_plot_nodes(&regen, "M"), 1);
}

#[test]
fn set_plot_node_title_updates_in_place() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    ast_mut::add_plot_node(class, &plot("x", "old")).unwrap();
    ast_mut::set_plot_node_title(class, "x", "new").expect("title");
    let regen = sd.to_modelica();
    assert!(regen.contains("\"new\""), "new title missing:\n{regen}");
    assert!(!regen.contains("\"old\""), "old title still present:\n{regen}");
}

// ─────────────────────────────────────────────────────────────────────
// set_diagram_text_*  /  remove_diagram_text
// ─────────────────────────────────────────────────────────────────────

#[test]
fn set_diagram_text_string_updates_indexed_entry() {
    // Two Text entries; index 1 should be updated, index 0 untouched.
    let mut h = host(
        "model M\nannotation(Diagram(graphics={Text(extent={{0,0},{1,1}}, textString=\"a\"), Text(extent={{2,2},{3,3}}, textString=\"b\")}));\nend M;\n",
    );
    h.apply(ModelicaOp::SetDiagramTextString {
        class: "M".into(),
        index: 1,
        text: "B".into(),
    })
    .expect("apply SetDiagramTextString");
    let src = h.document().source();
    assert!(src.contains("\"a\""), "first Text changed unexpectedly:\n{src}");
    assert!(src.contains("\"B\""), "second Text not updated:\n{src}");
}

#[test]
fn remove_diagram_text_drops_indexed_entry() {
    let mut h = host(
        "model M\nannotation(Diagram(graphics={Text(extent={{0,0},{1,1}}, textString=\"a\"), Text(extent={{2,2},{3,3}}, textString=\"b\")}));\nend M;\n",
    );
    h.apply(ModelicaOp::RemoveDiagramText {
        class: "M".into(),
        index: 0,
    })
    .expect("apply RemoveDiagramText");
    let src = h.document().source();
    assert!(!src.contains("\"a\""), "first Text still present:\n{src}");
    assert!(src.contains("\"b\""), "second Text dropped:\n{src}");
}

#[test]
fn remove_diagram_text_out_of_range_returns_error() {
    let mut sd = parse_to_ast("model M\nend M;\n", "t.mo").unwrap();
    let class = ast_mut::lookup_class_mut(&mut sd, "M").unwrap();
    let err = ast_mut::remove_diagram_text(class, 5).unwrap_err();
    assert!(matches!(
        err,
        AstMutError::DiagramTextIndexOutOfRange { index: 5, .. }
    ));
}

// ─────────────────────────────────────────────────────────────────────
// add_named_graphic (Icon / Diagram)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_icon_graphic_through_apply() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::AddIconGraphic {
        class: "M".into(),
        graphic: rect(),
    })
    .expect("apply AddIconGraphic");
    let src = h.document().source();
    assert!(src.contains("Icon"), "Icon section missing:\n{src}");
    assert!(src.contains("Rectangle"), "Rectangle missing:\n{src}");
}

#[test]
fn add_diagram_graphic_through_apply() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::AddDiagramGraphic {
        class: "M".into(),
        graphic: rect(),
    })
    .expect("apply AddDiagramGraphic");
    let src = h.document().source();
    assert!(src.contains("Diagram"), "Diagram section missing:\n{src}");
    assert!(src.contains("Rectangle"), "Rectangle missing:\n{src}");
}

// ─────────────────────────────────────────────────────────────────────
// Integration via host.apply for the plot ops
// ─────────────────────────────────────────────────────────────────────

#[test]
fn add_plot_node_through_apply() {
    let mut h = host("model M\nend M;\n");
    h.apply(ModelicaOp::AddPlotNode {
        class: "M".into(),
        plot: plot("x", "X"),
    })
    .expect("apply AddPlotNode");
    let src = h.document().source().to_string();
    assert!(src.contains("LunCoAnnotations.PlotNode"), "record missing:\n{src}");
    assert!(src.contains("__LunCo(plotNodes"), "vendor annotation missing:\n{src}");
    assert!(src.contains("\"x\""));
    assert_eq!(count_plot_nodes(&src, "M"), 1);
}

#[test]
fn remove_plot_node_through_apply() {
    // Pre-seed source with a plot node so the test does a single
    // apply — chaining apply(Add) + apply(Remove) doesn't work in
    // headless tests because the SyntaxCache stays stale between
    // applies (no debounced reparse driver). Production paths re-run
    // the parser between ops via `ui/ast_refresh.rs`.
    let mut h = host(
        "model M\nannotation(__LunCo(plotNodes={LunCoAnnotations.PlotNode(extent={{0,0},{1,1}}, signal=\"x\")}));\nend M;\n",
    );
    h.apply(ModelicaOp::RemovePlotNode {
        class: "M".into(),
        signal_path: "x".into(),
    })
    .expect("apply RemovePlotNode");
    let src = h.document().source().to_string();
    assert!(!src.contains("LunCoAnnotations.PlotNode"), "node not removed:\n{src}");
    assert_eq!(count_plot_nodes(&src, "M"), 0);
}

#[test]
fn set_plot_node_extent_through_apply() {
    // Same single-apply pattern as remove_plot_node_through_apply.
    let mut h = host(
        "model M\nannotation(__LunCo(plotNodes={LunCoAnnotations.PlotNode(extent={{0,0},{1,1}}, signal=\"x\")}));\nend M;\n",
    );
    h.apply(ModelicaOp::SetPlotNodeExtent {
        class: "M".into(),
        signal_path: "x".into(),
        x1: 100.0,
        y1: 200.0,
        x2: 300.0,
        y2: 400.0,
    })
    .expect("apply SetPlotNodeExtent");
    let src = h.document().source();
    assert!(src.contains("400"), "extent not updated:\n{src}");
}
